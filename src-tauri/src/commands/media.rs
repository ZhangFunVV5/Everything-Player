use crate::bilibili::{
    choose_audio_track, extract_bvid, select_video_metadata, SelectedAudioTrack, VideoCollection,
};
use crate::bilibili_http::{BilibiliHttpClient, BilibiliHttpError};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

const INVALID_INPUT_MESSAGE: &str = "请输入有效的 BV 号或 Bilibili 视频链接";
const MISSING_CID_MESSAGE: &str = "未找到视频分 P 信息";
const NO_AUDIO_MESSAGE: &str = "该视频没有可用的 DASH 音轨";
const MEDIA_STATE_MESSAGE: &str = "媒体状态不可用";
const VIEW_FALLBACK_MESSAGE: &str = "视频不存在或当前无法访问";
const PLAY_FALLBACK_MESSAGE: &str = "无法取得播放信息";
const MEDIA_ADDRESS_SCHEME: &str = "media-token://";
const MEDIA_TOKEN_TTL: Duration = Duration::from_secs(30 * 60);
const MEDIA_TOKEN_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const EXPIRED_TOKEN_RETENTION: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativeMediaSource {
    pub(crate) primary_url: String,
    pub(crate) backup_urls: Vec<String>,
}

#[derive(Debug)]
enum MediaTokenRecord {
    Active {
        source: NativeMediaSource,
        expires_at: Instant,
    },
    Expired {
        expired_at: Instant,
    },
}

#[derive(Default)]
struct MediaTokenRegistry {
    records: HashMap<String, MediaTokenRecord>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MediaTokenCleanup {
    sources_expired: usize,
    token_records_removed: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MediaTokenError {
    InvalidToken,
    ExpiredToken,
    StateUnavailable,
}

#[derive(Clone, Default)]
pub struct MediaResolveState {
    registry: Arc<Mutex<MediaTokenRegistry>>,
}

impl MediaResolveState {
    pub(crate) fn store(&self, track: SelectedAudioTrack) -> Result<String, MediaResolveCommandError> {
        let token = Uuid::new_v4().to_string();
        let source = NativeMediaSource {
            primary_url: track.primary_url,
            backup_urls: track.backup_urls,
        };
        self.registry
            .lock()
            .map_err(|_| MediaResolveCommandError::new(MEDIA_STATE_MESSAGE))?
            .insert(token.clone(), source, Instant::now());
        Ok(format!("{MEDIA_ADDRESS_SCHEME}{token}"))
    }

    pub(crate) fn source_for_token(
        &self,
        token: &str,
    ) -> Result<NativeMediaSource, MediaTokenError> {
        self.registry
            .lock()
            .map_err(|_| MediaTokenError::StateUnavailable)?
            .source_for_token(token, Instant::now())
    }

    #[cfg(any(test, all(debug_assertions, target_os = "windows")))]
    pub(crate) fn expire_token_for_test(&self, token: &str) {
        let now = Instant::now() + MEDIA_TOKEN_TTL;
        let result = self
            .registry
            .lock()
            .expect("test media state")
            .source_for_token(token, now);
        assert_eq!(result, Err(MediaTokenError::ExpiredToken));
    }

    pub(crate) fn start_cleanup_task(&self) {
        let registry = Arc::clone(&self.registry);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(MEDIA_TOKEN_CLEANUP_INTERVAL).await;
                if let Ok(mut registry) = registry.lock() {
                    registry.cleanup_expired(Instant::now());
                }
            }
        });
    }
}

impl MediaTokenRegistry {
    fn insert(&mut self, token: String, source: NativeMediaSource, now: Instant) {
        self.records.insert(
            token,
            MediaTokenRecord::Active {
                source,
                expires_at: now + MEDIA_TOKEN_TTL,
            },
        );
    }

    fn source_for_token(
        &mut self,
        token: &str,
        now: Instant,
    ) -> Result<NativeMediaSource, MediaTokenError> {
        let Some(record) = self.records.get_mut(token) else {
            return Err(MediaTokenError::InvalidToken);
        };

        match record {
            MediaTokenRecord::Active { source, expires_at } if *expires_at > now => {
                Ok(source.clone())
            }
            MediaTokenRecord::Active { expires_at, .. } => {
                *record = MediaTokenRecord::Expired {
                    expired_at: *expires_at,
                };
                Err(MediaTokenError::ExpiredToken)
            }
            MediaTokenRecord::Expired { .. } => Err(MediaTokenError::ExpiredToken),
        }
    }

    fn cleanup_expired(&mut self, now: Instant) -> MediaTokenCleanup {
        let mut cleanup = MediaTokenCleanup::default();
        for record in self.records.values_mut() {
            if let MediaTokenRecord::Active { expires_at, .. } = record {
                if *expires_at <= now {
                    *record = MediaTokenRecord::Expired {
                        expired_at: *expires_at,
                    };
                    cleanup.sources_expired += 1;
                }
            }
        }

        let previous_len = self.records.len();
        self.records.retain(|_, record| match record {
            MediaTokenRecord::Active { .. } => true,
            MediaTokenRecord::Expired { expired_at } => now < *expired_at + EXPIRED_TOKEN_RETENTION,
        });
        cleanup.token_records_removed = previous_len - self.records.len();
        cleanup
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaResolveResponse {
    pub bvid: String,
    pub cid: u64,
    pub title: String,
    pub owner: String,
    pub page_title: String,
    pub duration: Option<Value>,
    pub cover_url: String,
    pub collection: Option<VideoCollection>,
    pub audio_url: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MediaResolveCommandError {
    pub message: String,
}

impl MediaResolveCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy)]
enum ResolveStage {
    View,
    PlayUrl,
}

#[tauri::command]
pub async fn media_resolve(
    input: String,
    client: State<'_, BilibiliHttpClient>,
    media: State<'_, MediaResolveState>,
) -> Result<MediaResolveResponse, MediaResolveCommandError> {
    resolve(client.inner(), media.inner(), &input).await
}

async fn resolve(
    client: &BilibiliHttpClient,
    media: &MediaResolveState,
    input: &str,
) -> Result<MediaResolveResponse, MediaResolveCommandError> {
    let extracted_bvid =
        extract_bvid(input).ok_or_else(|| MediaResolveCommandError::new(INVALID_INPUT_MESSAGE))?;
    let view = client
        .fetch_view(&extracted_bvid)
        .await
        .map_err(|error| map_http_error(error, ResolveStage::View))?;
    let metadata = select_video_metadata(&view, &extracted_bvid);
    let cid = metadata
        .cid
        .as_ref()
        .and_then(Value::as_u64)
        .filter(|cid| *cid != 0)
        .ok_or_else(|| MediaResolveCommandError::new(MISSING_CID_MESSAGE))?;
    let play = client
        .fetch_playurl(&metadata.bvid, cid)
        .await
        .map_err(|error| map_http_error(error, ResolveStage::PlayUrl))?;
    let audio =
        choose_audio_track(&play).ok_or_else(|| MediaResolveCommandError::new(NO_AUDIO_MESSAGE))?;
    let audio_url = media.store(audio)?;
    #[cfg(all(debug_assertions, target_os = "windows"))]
    if std::env::var("P8_T5_PARITY").as_deref() == Ok("1") && input == "BV1P8T5EXPR0" {
        let state = media.clone();
        let token = audio_url.trim_start_matches(MEDIA_ADDRESS_SCHEME).to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            state.expire_token_for_test(&token);
        });
    }
    #[cfg(all(debug_assertions, target_os = "windows"))]
    if std::env::var("P8_T5_PARITY").as_deref() == Ok("1") && input == "BV1P8T5UPST0" {
        let state = media.clone();
        let token = audio_url.trim_start_matches(MEDIA_ADDRESS_SCHEME).to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            // Keep the token valid; the next native HTTP open must fail at the upstream.
            if let Ok(mut registry) = state.registry.lock() {
                if let Some(MediaTokenRecord::Active { source, .. }) = registry.records.get_mut(&token) {
                    source.primary_url = "https://p8-t5-audio.invalid/unavailable.m4a".to_owned();
                    source.backup_urls.clear();
                }
            }
        });
    }

    Ok(MediaResolveResponse {
        bvid: metadata.bvid,
        cid,
        title: metadata.title,
        owner: metadata.owner,
        page_title: metadata.page_title,
        duration: metadata.duration,
        cover_url: metadata.cover_url,
        collection: metadata.collection,
        audio_url,
    })
}

fn map_http_error(error: BilibiliHttpError, stage: ResolveStage) -> MediaResolveCommandError {
    let fallback = || match stage {
        ResolveStage::View => VIEW_FALLBACK_MESSAGE.to_owned(),
        ResolveStage::PlayUrl => PLAY_FALLBACK_MESSAGE.to_owned(),
    };
    let message = match error {
        BilibiliHttpError::Upstream { message, .. } => safe_upstream_message(&message, fallback()),
        BilibiliHttpError::MissingData => fallback(),
        BilibiliHttpError::InvalidBvid => INVALID_INPUT_MESSAGE.to_owned(),
        BilibiliHttpError::InvalidCid => MISSING_CID_MESSAGE.to_owned(),
        BilibiliHttpError::ClientBuild => "无法初始化 Bilibili HTTP 客户端".to_owned(),
        BilibiliHttpError::EndpointRejected => "Bilibili API endpoint 不在允许列表中".to_owned(),
        BilibiliHttpError::Timeout => "Bilibili 请求超时".to_owned(),
        BilibiliHttpError::Transport => "Bilibili 请求失败".to_owned(),
        BilibiliHttpError::Redirect(status) => {
            format!("Bilibili API 拒绝重定向（HTTP {status}）")
        }
        BilibiliHttpError::HttpStatus(status) => {
            format!("Bilibili 请求失败（HTTP {status}）")
        }
        BilibiliHttpError::ResponseTooLarge { limit } => {
            format!("Bilibili 响应超过 {limit} 字节限制")
        }
        BilibiliHttpError::InvalidJson => "Bilibili 响应不是有效 JSON".to_owned(),
    };
    MediaResolveCommandError::new(message)
}

pub(super) fn map_view_error(error: BilibiliHttpError) -> String {
    map_http_error(error, ResolveStage::View).message
}

fn safe_upstream_message(message: &str, fallback: String) -> String {
    let message = message.trim();
    let lower = message.to_ascii_lowercase();
    let sensitive = message.is_empty()
        || message.contains('/')
        || message.contains('\\')
        || [
            "cookie",
            "authorization",
            "bearer",
            "credential",
            "password",
            "secret",
            "token",
        ]
        .iter()
        .any(|marker| lower.contains(marker));
    if sensitive {
        fallback
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Url;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    struct MockApi {
        origin: Url,
        requests: Receiver<String>,
        thread: JoinHandle<()>,
    }

    impl MockApi {
        fn start(responses: Vec<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock API");
            let address = listener.local_addr().expect("mock API address");
            let (sender, requests) = mpsc::channel();
            let thread = thread::spawn(move || {
                for response in responses {
                    let (mut stream, _) = listener.accept().expect("accept mock API request");
                    sender
                        .send(read_request(&mut stream))
                        .expect("send captured request");
                    stream.write_all(&response).expect("write mock response");
                }
            });
            Self {
                origin: Url::parse(&format!("http://{address}/")).expect("mock API origin"),
                requests,
                thread,
            }
        }

        fn client(&self) -> BilibiliHttpClient {
            BilibiliHttpClient::for_local_test(
                self.origin.clone(),
                Duration::from_secs(2),
                64 * 1024,
            )
            .expect("mock API client")
        }

        fn finish(self) -> Vec<String> {
            self.thread.join().expect("mock API thread");
            self.requests.try_iter().collect()
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("request read timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read mock request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(bytes).expect("request is HTTP text")
    }

    fn json_response(body: Value) -> Vec<u8> {
        let body = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn view_data() -> Value {
        json!({
            "code": 0,
            "data": {
                "bvid": "BV1A2B3C4D5E",
                "title": "Synthetic Resolve Video",
                "owner": { "name": "Synthetic Resolve UP" },
                "pic": "http://example.com/resolve-cover.jpg",
                "duration": 999,
                "cid": 9099,
                "pages": [
                    { "cid": 9101, "part": "Synthetic First Page", "duration": 111 },
                    { "cid": 9102, "part": "Synthetic Second Page", "duration": 222 }
                ],
                "ugc_season": {
                    "id": 901,
                    "title": "Synthetic Collection",
                    "sections": [{
                        "id": 1,
                        "title": "Synthetic Section",
                        "episodes": [{
                            "bvid": "BV1F2G3H4J5K",
                            "title": "Synthetic Episode",
                            "page": { "duration": 31 },
                            "arc": { "pic": "http://example.com/episode.jpg" }
                        }]
                    }]
                }
            }
        })
    }

    fn play_data() -> Value {
        json!({
            "code": 0,
            "data": {
                "dash": {
                    "audio": [{
                        "baseUrl": "https://cdn.example/low",
                        "backupUrl": ["https://backup.example/low"],
                        "bandwidth": 64000
                    }],
                    "dolby": { "audio": [{
                        "base_url": "https://cdn.example/high",
                        "backup_url": ["https://backup.example/high-1", "https://backup.example/high-2"],
                        "bandwidth": 256000
                    }] }
                }
            }
        })
    }

    fn frozen_resolve_fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../test/fixtures/bilibili/resolve-contract.json"
        ))
        .expect("frozen Bilibili resolve fixture")
    }

    #[test]
    fn bilibili_media_resolve_returns_legacy_metadata_and_opaque_audio_address() {
        tauri::async_runtime::block_on(async {
            let api = MockApi::start(vec![json_response(view_data()), json_response(play_data())]);
            let client = api.client();
            let state = MediaResolveState::default();
            let result = resolve(
                &client,
                &state,
                "https://www.bilibili.com/video/BV1A2B3C4D5E?p=2",
            )
            .await
            .expect("resolved video");

            assert_eq!(result.bvid, "BV1A2B3C4D5E");
            assert_eq!(result.cid, 9101);
            assert_eq!(result.title, "Synthetic Resolve Video");
            assert_eq!(result.owner, "Synthetic Resolve UP");
            assert_eq!(result.page_title, "Synthetic First Page");
            assert_eq!(result.duration, Some(json!(111)));
            assert_eq!(result.cover_url, "https://example.com/resolve-cover.jpg");
            assert_eq!(
                result.collection.as_ref().expect("collection").id,
                Some(json!(901))
            );
            assert!(result.audio_url.starts_with(MEDIA_ADDRESS_SCHEME));
            assert!(!result.audio_url.contains("cdn.example"));

            let token = result
                .audio_url
                .strip_prefix(MEDIA_ADDRESS_SCHEME)
                .expect("opaque token");
            Uuid::parse_str(token).expect("UUID token");
            let registry = state.registry.lock().expect("native media state");
            assert_eq!(registry.records.len(), 1);
            drop(registry);
            assert_eq!(
                state.source_for_token(token),
                Ok(NativeMediaSource {
                    primary_url: "https://cdn.example/high".to_owned(),
                    backup_urls: vec![
                        "https://backup.example/high-1".to_owned(),
                        "https://backup.example/high-2".to_owned(),
                    ],
                })
            );
            let serialized = serde_json::to_string(&result).expect("response serialization");
            assert!(!serialized.contains("cdn.example"));
            assert!(!serialized.contains("backup.example"));

            let requests = api.finish();
            assert_eq!(requests.len(), 2);
            assert!(requests[0].starts_with("GET /x/web-interface/view?bvid=BV1A2B3C4D5E HTTP/1.1"));
            assert!(requests[1].starts_with(
                "GET /x/player/playurl?bvid=BV1A2B3C4D5E&cid=9101&fnval=16&fnver=0&fourk=1 HTTP/1.1"
            ));
        });
    }

    #[test]
    fn bilibili_mocked_fixture_matches_legacy_dto_headers_selection_and_privacy() {
        tauri::async_runtime::block_on(async {
            let fixture = frozen_resolve_fixture();
            let api = MockApi::start(vec![
                json_response(fixture["view"].clone()),
                json_response(fixture["playurl"].clone()),
            ]);
            let state = MediaResolveState::default();
            let result = resolve(
                &api.client(),
                &state,
                fixture["input"].as_str().expect("fixture input"),
            )
            .await
            .expect("fixture resolve result");

            let token = result
                .audio_url
                .strip_prefix(MEDIA_ADDRESS_SCHEME)
                .expect("opaque media address");
            Uuid::parse_str(token).expect("UUID media token");

            let mut public_dto = serde_json::to_value(&result).expect("serialize public DTO");
            public_dto["audioUrl"] = json!("<opaque-media-token>");
            assert_eq!(public_dto, fixture["expectedResponse"]);
            if std::env::var_os("BILIBILI_PARITY_JSON").is_some() {
                println!(
                    "BILIBILI_PARITY_DTO={}",
                    serde_json::to_string(&public_dto).expect("serialize parity DTO")
                );
            }

            let selected =
                choose_audio_track(&fixture["playurl"]["data"]).expect("selected fixture audio");
            assert_eq!(
                selected.bandwidth,
                Some(fixture["expectedNativeSource"]["bandwidth"].clone())
            );
            let registry = state.registry.lock().expect("native media state");
            let record = registry.records.get(token).expect("registered token");
            match record {
                MediaTokenRecord::Active { source, expires_at } => {
                    assert_eq!(
                        source,
                        &NativeMediaSource {
                            primary_url: fixture["expectedNativeSource"]["primaryUrl"]
                                .as_str()
                                .expect("fixture primary URL")
                                .to_owned(),
                            backup_urls: fixture["expectedNativeSource"]["backupUrls"]
                                .as_array()
                                .expect("fixture backup URLs")
                                .iter()
                                .map(|url| { url.as_str().expect("fixture backup URL").to_owned() })
                                .collect(),
                        }
                    );
                    assert!(*expires_at > Instant::now());
                    assert!(*expires_at <= Instant::now() + MEDIA_TOKEN_TTL);
                }
                MediaTokenRecord::Expired { .. } => panic!("new token is already expired"),
            }
            drop(registry);

            let serialized = serde_json::to_string(&result).expect("serialized public DTO");
            for forbidden in ["cdn.example", "backup.example"] {
                assert!(!serialized.contains(forbidden));
            }

            let requests = api.finish();
            assert_eq!(requests.len(), 2);
            assert!(requests[0].starts_with("GET /x/web-interface/view?bvid=BV1A2B3C4D5E HTTP/1.1"));
            assert!(requests[1].starts_with(
                "GET /x/player/playurl?bvid=BV1A2B3C4D5E&cid=9101&fnval=16&fnver=0&fourk=1 HTTP/1.1"
            ));
            for request in requests {
                let request = request.to_ascii_lowercase();
                assert!(request.contains("accept: application/json, text/plain, */*\r\n"));
                assert!(request.contains("referer: https://www.bilibili.com/\r\n"));
                assert!(request.contains("user-agent: mozilla/5.0"));
                assert!(!request.contains("\r\ncookie:"));
                assert!(!request.contains("\r\nauthorization:"));
            }
        });
    }

    #[test]
    fn media_token_expires_at_the_30_minute_boundary_and_distinguishes_unknown_tokens() {
        let issued_at = Instant::now();
        let expiry = issued_at + MEDIA_TOKEN_TTL;
        let source = NativeMediaSource {
            primary_url: "https://cdn.example/audio".to_owned(),
            backup_urls: vec!["https://backup.example/audio".to_owned()],
        };
        let mut registry = MediaTokenRegistry::default();
        registry.insert("opaque-token".to_owned(), source.clone(), issued_at);

        assert_eq!(
            registry.source_for_token("opaque-token", expiry - Duration::from_nanos(1)),
            Ok(source)
        );
        assert_eq!(
            registry.source_for_token("opaque-token", expiry),
            Err(MediaTokenError::ExpiredToken)
        );
        assert_eq!(
            registry.source_for_token("opaque-token", expiry + Duration::from_secs(1)),
            Err(MediaTokenError::ExpiredToken)
        );
        assert_eq!(
            registry.source_for_token("never-issued", expiry),
            Err(MediaTokenError::InvalidToken)
        );
        assert!(matches!(
            registry.records.get("opaque-token"),
            Some(MediaTokenRecord::Expired { expired_at }) if *expired_at == expiry
        ));
    }

    #[test]
    fn media_token_cleanup_drops_expired_urls_and_retains_only_bounded_expiry_metadata() {
        let issued_at = Instant::now();
        let expired_at = issued_at + MEDIA_TOKEN_TTL;
        let source = NativeMediaSource {
            primary_url: "https://private-cdn.example/audio".to_owned(),
            backup_urls: vec!["https://private-backup.example/audio".to_owned()],
        };
        let mut registry = MediaTokenRegistry::default();
        registry.insert("expired-token".to_owned(), source, issued_at);
        registry.insert(
            "active-token".to_owned(),
            NativeMediaSource {
                primary_url: "https://cdn.example/active".to_owned(),
                backup_urls: Vec::new(),
            },
            issued_at + MEDIA_TOKEN_TTL / 2,
        );

        let first_cleanup = registry.cleanup_expired(expired_at);
        assert_eq!(
            first_cleanup,
            MediaTokenCleanup {
                sources_expired: 1,
                token_records_removed: 0,
            }
        );
        assert!(matches!(
            registry.records.get("expired-token"),
            Some(MediaTokenRecord::Expired { expired_at: recorded }) if *recorded == expired_at
        ));
        assert!(matches!(
            registry.records.get("active-token"),
            Some(MediaTokenRecord::Active { .. })
        ));
        assert_eq!(
            registry.source_for_token("expired-token", expired_at),
            Err(MediaTokenError::ExpiredToken)
        );

        let final_cleanup = registry.cleanup_expired(expired_at + EXPIRED_TOKEN_RETENTION);
        assert_eq!(
            final_cleanup,
            MediaTokenCleanup {
                sources_expired: 1,
                token_records_removed: 1,
            }
        );
        assert!(!registry.records.contains_key("expired-token"));
        assert!(matches!(
            registry.source_for_token("expired-token", expired_at + EXPIRED_TOKEN_RETENTION),
            Err(MediaTokenError::InvalidToken)
        ));
    }

    #[test]
    fn bilibili_media_resolve_maps_invalid_input_missing_cid_and_no_audio() {
        tauri::async_runtime::block_on(async {
            let state = MediaResolveState::default();
            let production_client = BilibiliHttpClient::new().expect("production client");
            assert_eq!(
                resolve(&production_client, &state, "not-a-bvid").await,
                Err(MediaResolveCommandError::new(INVALID_INPUT_MESSAGE))
            );

            let api = MockApi::start(vec![json_response(json!({
                "code": 0,
                "data": { "bvid": "BV1A2B3C4D5E", "pages": [], "cid": null }
            }))]);
            assert_eq!(
                resolve(&api.client(), &state, "BV1A2B3C4D5E").await,
                Err(MediaResolveCommandError::new(MISSING_CID_MESSAGE))
            );
            let _ = api.finish();

            let api = MockApi::start(vec![
                json_response(view_data()),
                json_response(json!({ "code": 0, "data": { "dash": { "audio": [] } } })),
            ]);
            assert_eq!(
                resolve(&api.client(), &state, "BV1A2B3C4D5E").await,
                Err(MediaResolveCommandError::new(NO_AUDIO_MESSAGE))
            );
            let _ = api.finish();
            assert!(state
                .registry
                .lock()
                .expect("native media state")
                .records
                .is_empty());
        });
    }

    #[test]
    fn bilibili_media_resolve_maps_upstream_and_timeout_errors_without_urls() {
        tauri::async_runtime::block_on(async {
            let state = MediaResolveState::default();
            let api = MockApi::start(vec![json_response(json!({
                "code": -400,
                "message": "synthetic upstream error",
                "data": null
            }))]);
            assert_eq!(
                resolve(&api.client(), &state, "BV1A2B3C4D5E").await,
                Err(MediaResolveCommandError::new("synthetic upstream error"))
            );
            let _ = api.finish();

            assert_eq!(
                map_http_error(BilibiliHttpError::Timeout, ResolveStage::View),
                MediaResolveCommandError::new("Bilibili 请求超时")
            );
            assert_eq!(
                map_http_error(
                    BilibiliHttpError::Upstream {
                        code: -1,
                        message: String::new(),
                    },
                    ResolveStage::PlayUrl,
                ),
                MediaResolveCommandError::new(PLAY_FALLBACK_MESSAGE)
            );
            let rendered = map_http_error(BilibiliHttpError::Transport, ResolveStage::View).message;
            assert!(!rendered.contains("http://"));
            assert!(!rendered.contains("https://"));
            assert!(state
                .registry
                .lock()
                .expect("native media state")
                .records
                .is_empty());
        });
    }

    #[test]
    fn bilibili_media_resolve_maps_all_command_errors_to_stable_message_objects() {
        let cases = [
            (
                BilibiliHttpError::InvalidBvid,
                ResolveStage::View,
                INVALID_INPUT_MESSAGE,
            ),
            (
                BilibiliHttpError::InvalidCid,
                ResolveStage::PlayUrl,
                MISSING_CID_MESSAGE,
            ),
            (
                BilibiliHttpError::Timeout,
                ResolveStage::View,
                "Bilibili 请求超时",
            ),
            (
                BilibiliHttpError::HttpStatus(503),
                ResolveStage::View,
                "Bilibili 请求失败（HTTP 503）",
            ),
            (
                BilibiliHttpError::Transport,
                ResolveStage::View,
                "Bilibili 请求失败",
            ),
            (
                BilibiliHttpError::ResponseTooLarge { limit: 2048 },
                ResolveStage::View,
                "Bilibili 响应超过 2048 字节限制",
            ),
            (
                BilibiliHttpError::InvalidJson,
                ResolveStage::View,
                "Bilibili 响应不是有效 JSON",
            ),
            (
                BilibiliHttpError::MissingData,
                ResolveStage::View,
                VIEW_FALLBACK_MESSAGE,
            ),
            (
                BilibiliHttpError::MissingData,
                ResolveStage::PlayUrl,
                PLAY_FALLBACK_MESSAGE,
            ),
        ];

        for (error, stage, expected) in cases {
            let mapped = map_http_error(error, stage);
            assert_eq!(mapped.message, expected);
            assert_eq!(
                serde_json::to_value(&mapped).expect("serialize command error"),
                json!({ "message": expected })
            );
        }

        let upstream = map_http_error(
            BilibiliHttpError::Upstream {
                code: -400,
                message: "请求参数错误".to_owned(),
            },
            ResolveStage::View,
        );
        assert_eq!(upstream.message, "请求参数错误");

        for unsafe_message in [
            "https://cdn.example/audio.m4s",
            "media-token://11111111-1111-4111-8111-111111111111",
            "cookie=secret",
            "C:\\private\\credentials.txt",
        ] {
            let mapped = map_http_error(
                BilibiliHttpError::Upstream {
                    code: -1,
                    message: unsafe_message.to_owned(),
                },
                ResolveStage::PlayUrl,
            );
            assert_eq!(mapped.message, PLAY_FALLBACK_MESSAGE);
            assert!(!mapped.message.contains(unsafe_message));
        }
    }
}
