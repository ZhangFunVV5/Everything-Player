use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, REFERER, USER_AGENT};
use reqwest::{redirect, Client, Request, Url};
use serde::Deserialize;
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::time::Duration;

const API_ORIGIN: &str = "https://api.bilibili.com/";
const VIEW_PATH: &str = "/x/web-interface/view";
const PLAYURL_PATH: &str = "/x/player/playurl";
const ACCEPT_VALUE: &str = "application/json, text/plain, */*";
const REFERER_VALUE: &str = "https://www.bilibili.com/";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub const BILIBILI_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const BILIBILI_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BilibiliHttpError {
    ClientBuild,
    InvalidBvid,
    InvalidCid,
    EndpointRejected,
    Timeout,
    Transport,
    Redirect(u16),
    HttpStatus(u16),
    ResponseTooLarge { limit: usize },
    InvalidJson,
    Upstream { code: i64, message: String },
    MissingData,
}

impl Display for BilibiliHttpError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientBuild => formatter.write_str("无法初始化 Bilibili HTTP 客户端"),
            Self::InvalidBvid => formatter.write_str("BV 号无效"),
            Self::InvalidCid => formatter.write_str("CID 无效"),
            Self::EndpointRejected => formatter.write_str("Bilibili API endpoint 不在允许列表中"),
            Self::Timeout => formatter.write_str("Bilibili 请求超时"),
            Self::Transport => formatter.write_str("Bilibili 请求失败"),
            Self::Redirect(status) => write!(formatter, "Bilibili API 拒绝重定向（HTTP {status}）"),
            Self::HttpStatus(status) => write!(formatter, "Bilibili 请求失败（HTTP {status}）"),
            Self::ResponseTooLarge { limit } => {
                write!(formatter, "Bilibili 响应超过 {limit} 字节限制")
            }
            Self::InvalidJson => formatter.write_str("Bilibili 响应不是有效 JSON"),
            Self::Upstream { code, message } if message.is_empty() => {
                write!(formatter, "Bilibili 返回错误代码 {code}")
            }
            Self::Upstream { code, message } => {
                write!(formatter, "Bilibili 返回错误代码 {code}：{message}")
            }
            Self::MissingData => formatter.write_str("Bilibili 响应缺少 data"),
        }
    }
}

impl Error for BilibiliHttpError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiEndpoint {
    View,
    PlayUrl,
}

impl ApiEndpoint {
    const fn path(self) -> &'static str {
        match self {
            Self::View => VIEW_PATH,
            Self::PlayUrl => PLAYURL_PATH,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BilibiliEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<Value>,
}

/// Native-only client for the two allowlisted Bilibili JSON APIs.
///
/// The public API accepts validated identifiers, never a URL. Redirects are
/// disabled and the production origin is fixed to `https://api.bilibili.com`.
#[derive(Clone)]
pub struct BilibiliHttpClient {
    client: Client,
    origin: Url,
    header_timeout: Duration,
    max_response_bytes: usize,
    production_origin: bool,
}

impl BilibiliHttpClient {
    pub fn new() -> Result<Self, BilibiliHttpError> {
        let origin = Url::parse(API_ORIGIN).map_err(|_| BilibiliHttpError::ClientBuild)?;
        let client = build_reqwest_client(BILIBILI_REQUEST_TIMEOUT, true)?;
        Ok(Self {
            client,
            origin,
            header_timeout: BILIBILI_REQUEST_TIMEOUT,
            max_response_bytes: BILIBILI_MAX_RESPONSE_BYTES,
            production_origin: true,
        })
    }

    pub async fn fetch_view(&self, bvid: &str) -> Result<Value, BilibiliHttpError> {
        validate_bvid(bvid)?;
        #[cfg(all(debug_assertions, target_os = "windows"))]
        if std::env::var("P8_T5_PARITY").as_deref() == Ok("1") {
            match bvid {
                "BV1P8T5NET00" => return Err(BilibiliHttpError::Transport),
                "BV1P8T5TIME0" => {
                    tokio::time::sleep(BILIBILI_REQUEST_TIMEOUT).await;
                    return Err(BilibiliHttpError::Timeout);
                }
                "BV1P8T5PART0" | "BV1P8T5COVER" | "BV1P8T5EXPR0" | "BV1P8T5UPST0" => {
                    let request = self.build_request(ApiEndpoint::View, |url| {
                        url.query_pairs_mut().append_pair("bvid", "BV1Q541167Qg");
                    })?;
                    let mut view = self.execute(request).await?;
                    if bvid == "BV1P8T5COVER" {
                        view["pic"] = Value::String("https://example.com/p8-t5-missing-cover.jpg".to_owned());
                    } else if bvid == "BV1P8T5PART0" {
                        if let Some(pages) = view["pages"].as_array_mut() {
                            if let Some(first) = pages.first_mut() {
                                first["part"] = Value::String("P8-T5 first page".to_owned());
                            }
                            pages.push(serde_json::json!({"cid":999999999,"duration":1,"part":"P8-T5 second page (must not select)"}));
                        }
                    }
                    return Ok(view);
                }
                _ => {}
            }
        }
        let request = self.build_request(ApiEndpoint::View, |url| {
            url.query_pairs_mut().append_pair("bvid", bvid);
        })?;
        self.execute(request).await
    }

    pub async fn fetch_playurl(&self, bvid: &str, cid: u64) -> Result<Value, BilibiliHttpError> {
        validate_bvid(bvid)?;
        if cid == 0 {
            return Err(BilibiliHttpError::InvalidCid);
        }

        let cid = cid.to_string();
        let request = self.build_request(ApiEndpoint::PlayUrl, |url| {
            url.query_pairs_mut()
                .append_pair("bvid", bvid)
                .append_pair("cid", &cid)
                .append_pair("fnval", "16")
                .append_pair("fnver", "0")
                .append_pair("fourk", "1");
        })?;
        self.execute(request).await
    }

    fn build_request(
        &self,
        endpoint: ApiEndpoint,
        add_query: impl FnOnce(&mut Url),
    ) -> Result<Request, BilibiliHttpError> {
        let mut url = self.origin.clone();
        url.set_path(endpoint.path());
        url.set_query(None);
        url.set_fragment(None);
        add_query(&mut url);

        if self.production_origin {
            validate_production_url(&url, endpoint)?;
        } else {
            validate_test_url(&url, endpoint)?;
        }

        self.client
            .get(url)
            .headers(api_headers())
            .build()
            .map_err(|_| BilibiliHttpError::EndpointRejected)
    }

    async fn execute(&self, request: Request) -> Result<Value, BilibiliHttpError> {
        let response = tokio::time::timeout(self.header_timeout, self.client.execute(request))
            .await
            .map_err(|_| BilibiliHttpError::Timeout)?
            .map_err(|error| {
                if error.is_timeout() {
                    BilibiliHttpError::Timeout
                } else {
                    BilibiliHttpError::Transport
                }
            })?;

        let status = response.status();
        if status.is_redirection() {
            return Err(BilibiliHttpError::Redirect(status.as_u16()));
        }
        if !status.is_success() {
            return Err(BilibiliHttpError::HttpStatus(status.as_u16()));
        }

        let body = read_limited_body(response, self.max_response_bytes).await?;
        let envelope: BilibiliEnvelope =
            serde_json::from_slice(&body).map_err(|_| BilibiliHttpError::InvalidJson)?;
        if envelope.code != 0 {
            return Err(BilibiliHttpError::Upstream {
                code: envelope.code,
                message: envelope.message,
            });
        }
        envelope.data.ok_or(BilibiliHttpError::MissingData)
    }

    #[cfg(test)]
    pub(crate) fn for_local_test(
        origin: Url,
        header_timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, BilibiliHttpError> {
        if origin.scheme() != "http"
            || origin.host_str() != Some("127.0.0.1")
            || origin.port().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return Err(BilibiliHttpError::EndpointRejected);
        }
        let client = build_reqwest_client(header_timeout, false)?;
        Ok(Self {
            client,
            origin,
            header_timeout,
            max_response_bytes,
            production_origin: false,
        })
    }
}

fn build_reqwest_client(
    connect_timeout: Duration,
    https_only: bool,
) -> Result<Client, BilibiliHttpError> {
    Client::builder()
        .connect_timeout(connect_timeout)
        .redirect(redirect::Policy::none())
        .referer(false)
        .https_only(https_only)
        .build()
        .map_err(|_| BilibiliHttpError::ClientBuild)
}

fn api_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_VALUE));
    headers.insert(REFERER, HeaderValue::from_static(REFERER_VALUE));
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers
}

fn validate_bvid(bvid: &str) -> Result<(), BilibiliHttpError> {
    let valid = bvid.strip_prefix("BV").is_some_and(|suffix| {
        suffix.len() == 10 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    });
    if valid {
        Ok(())
    } else {
        Err(BilibiliHttpError::InvalidBvid)
    }
}

fn validate_production_url(url: &Url, endpoint: ApiEndpoint) -> Result<(), BilibiliHttpError> {
    let valid = url.scheme() == "https"
        && url.host_str() == Some("api.bilibili.com")
        && url.port_or_known_default() == Some(443)
        && url.path() == endpoint.path()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none();
    if valid {
        Ok(())
    } else {
        Err(BilibiliHttpError::EndpointRejected)
    }
}

#[cfg(test)]
fn validate_test_url(url: &Url, endpoint: ApiEndpoint) -> Result<(), BilibiliHttpError> {
    let valid = url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.path() == endpoint.path()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none();
    if valid {
        Ok(())
    } else {
        Err(BilibiliHttpError::EndpointRejected)
    }
}

#[cfg(not(test))]
fn validate_test_url(_url: &Url, _endpoint: ApiEndpoint) -> Result<(), BilibiliHttpError> {
    Err(BilibiliHttpError::EndpointRejected)
}

async fn read_limited_body(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, BilibiliHttpError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(BilibiliHttpError::ResponseTooLarge { limit });
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| BilibiliHttpError::Transport)?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(BilibiliHttpError::ResponseTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod bilibili_http_tests {
    use super::*;
    use crate::bilibili::{choose_audio_track, select_video_metadata};
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    struct MockServer {
        origin: Url,
        request: Receiver<String>,
        thread: JoinHandle<()>,
    }

    impl MockServer {
        fn start(response: Vec<u8>, delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
            let address = listener.local_addr().expect("mock server address");
            let (sender, request) = mpsc::channel();
            let thread = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept mock request");
                let captured = read_request(&mut stream);
                sender.send(captured).expect("send captured request");
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                let _ = stream.write_all(&response);
            });
            Self {
                origin: Url::parse(&format!("http://{address}/")).expect("mock origin"),
                request,
                thread,
            }
        }

        fn start_with_delayed_body(body: String, delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
            let address = listener.local_addr().expect("mock server address");
            let (sender, request) = mpsc::channel();
            let thread = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept mock request");
                let captured = read_request(&mut stream);
                sender.send(captured).expect("send captured request");
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(headers.as_bytes())
                    .expect("write response headers");
                thread::sleep(delay);
                let _ = stream.write_all(body.as_bytes());
            });
            Self {
                origin: Url::parse(&format!("http://{address}/")).expect("mock origin"),
                request,
                thread,
            }
        }

        fn finish(self) -> String {
            let request = self.request.recv().expect("captured request");
            self.thread.join().expect("mock server thread");
            request
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

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn response_without_length(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}"
        )
        .into_bytes()
    }

    fn local_client(server: &MockServer) -> BilibiliHttpClient {
        BilibiliHttpClient::for_local_test(
            server.origin.clone(),
            Duration::from_secs(2),
            BILIBILI_MAX_RESPONSE_BYTES,
        )
        .expect("local test client")
    }

    #[test]
    fn bilibili_http_production_urls_are_https_and_allowlisted() {
        let client = BilibiliHttpClient::new().expect("production client");
        let view = client
            .build_request(ApiEndpoint::View, |url| {
                url.query_pairs_mut().append_pair("bvid", "BV1A2B3C4D5E");
            })
            .expect("view request");
        assert_eq!(view.url().scheme(), "https");
        assert_eq!(view.url().host_str(), Some("api.bilibili.com"));
        assert_eq!(view.url().path(), VIEW_PATH);
        assert_eq!(
            view.url().query_pairs().collect::<Vec<_>>(),
            vec![("bvid".into(), "BV1A2B3C4D5E".into())]
        );
        assert_eq!(BILIBILI_REQUEST_TIMEOUT, Duration::from_secs(15));

        for rejected in [
            "http://api.bilibili.com/x/web-interface/view?bvid=BV1A2B3C4D5E",
            "https://evil.example/x/web-interface/view?bvid=BV1A2B3C4D5E",
            "https://api.bilibili.com/x/other?bvid=BV1A2B3C4D5E",
            "https://user@api.bilibili.com/x/web-interface/view?bvid=BV1A2B3C4D5E",
        ] {
            let url = Url::parse(rejected).expect("rejected URL fixture");
            assert_eq!(
                validate_production_url(&url, ApiEndpoint::View),
                Err(BilibiliHttpError::EndpointRejected)
            );
        }
        assert_eq!(
            BilibiliHttpClient::for_local_test(
                Url::parse("http://example.com/").expect("URL fixture"),
                Duration::from_secs(1),
                1024,
            )
            .err(),
            Some(BilibiliHttpError::EndpointRejected)
        );
    }

    #[test]
    fn bilibili_http_sets_legacy_headers_and_fixed_query_parameters() {
        tauri::async_runtime::block_on(async {
            let server = MockServer::start(
                response("200 OK", r#"{"code":0,"data":{"dash":{}}}"#),
                Duration::ZERO,
            );
            let client = local_client(&server);
            let data = client
                .fetch_playurl("BV1A2B3C4D5E", 9101)
                .await
                .expect("play-url response");
            assert_eq!(data, serde_json::json!({ "dash": {} }));

            let request = server.finish();
            let request_lower = request.to_ascii_lowercase();
            assert!(request.starts_with(
                "GET /x/player/playurl?bvid=BV1A2B3C4D5E&cid=9101&fnval=16&fnver=0&fourk=1 HTTP/1.1\r\n"
            ));
            assert!(request_lower.contains("accept: application/json, text/plain, */*\r\n"));
            assert!(request_lower.contains("referer: https://www.bilibili.com/\r\n"));
            assert!(request_lower.contains(&format!(
                "user-agent: {}\r\n",
                USER_AGENT_VALUE.to_ascii_lowercase()
            )));
            assert!(!request_lower.contains("\r\ncookie:"));
            assert!(!request_lower.contains("\r\nauthorization:"));
        });
    }

    #[test]
    fn bilibili_http_validates_identifiers_before_transport() {
        let client = BilibiliHttpClient::new().expect("production client");
        tauri::async_runtime::block_on(async {
            assert_eq!(
                client.fetch_view("not-a-bvid").await,
                Err(BilibiliHttpError::InvalidBvid)
            );
            assert_eq!(
                client.fetch_playurl("BV1A2B3C4D5E", 0).await,
                Err(BilibiliHttpError::InvalidCid)
            );
        });
    }

    #[test]
    fn bilibili_http_times_out_while_waiting_for_response_headers() {
        tauri::async_runtime::block_on(async {
            let server = MockServer::start(
                response("200 OK", r#"{"code":0,"data":{}}"#),
                Duration::from_millis(200),
            );
            let client = BilibiliHttpClient::for_local_test(
                server.origin.clone(),
                Duration::from_millis(40),
                1024,
            )
            .expect("local test client");
            assert_eq!(
                client.fetch_view("BV1A2B3C4D5E").await,
                Err(BilibiliHttpError::Timeout)
            );
            let _ = server.finish();

            let body = r#"{"code":0,"data":{"ok":true}}"#.to_owned();
            let server = MockServer::start_with_delayed_body(body, Duration::from_millis(100));
            let client = BilibiliHttpClient::for_local_test(
                server.origin.clone(),
                Duration::from_millis(40),
                1024,
            )
            .expect("local test client");
            assert_eq!(
                client.fetch_view("BV1A2B3C4D5E").await,
                Ok(serde_json::json!({ "ok": true }))
            );
            let _ = server.finish();
        });
    }

    #[test]
    fn bilibili_http_refuses_redirects_without_following_location() {
        tauri::async_runtime::block_on(async {
            let redirect = b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/should-not-follow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
            let server = MockServer::start(redirect, Duration::ZERO);
            let client = local_client(&server);
            assert_eq!(
                client.fetch_view("BV1A2B3C4D5E").await,
                Err(BilibiliHttpError::Redirect(302))
            );
            let request = server.finish();
            assert!(request.starts_with("GET /x/web-interface/view?bvid=BV1A2B3C4D5E "));
        });
    }

    #[test]
    fn bilibili_http_enforces_streaming_response_size_limit() {
        tauri::async_runtime::block_on(async {
            let body = format!(r#"{{"code":0,"data":"{}"}}"#, "x".repeat(128));
            let server =
                MockServer::start(response_without_length("200 OK", &body), Duration::ZERO);
            let client = BilibiliHttpClient::for_local_test(
                server.origin.clone(),
                Duration::from_secs(2),
                64,
            )
            .expect("local test client");
            assert_eq!(
                client.fetch_view("BV1A2B3C4D5E").await,
                Err(BilibiliHttpError::ResponseTooLarge { limit: 64 })
            );
            let _ = server.finish();
        });
    }

    #[test]
    fn bilibili_http_reports_status_json_upstream_and_missing_data_errors() {
        tauri::async_runtime::block_on(async {
            let cases = [
                (
                    response("503 Service Unavailable", r#"{"error":"busy"}"#),
                    BilibiliHttpError::HttpStatus(503),
                ),
                (
                    response("200 OK", "not-json"),
                    BilibiliHttpError::InvalidJson,
                ),
                (
                    response(
                        "200 OK",
                        r#"{"code":-400,"message":"synthetic upstream error","data":null}"#,
                    ),
                    BilibiliHttpError::Upstream {
                        code: -400,
                        message: "synthetic upstream error".to_owned(),
                    },
                ),
                (
                    response("200 OK", r#"{"code":0,"data":null}"#),
                    BilibiliHttpError::MissingData,
                ),
            ];

            for (response, expected) in cases {
                let server = MockServer::start(response, Duration::ZERO);
                let client = local_client(&server);
                assert_eq!(client.fetch_view("BV1A2B3C4D5E").await, Err(expected));
                let _ = server.finish();
            }
        });
    }

    #[test]
    fn bilibili_http_errors_do_not_echo_urls_or_secrets() {
        let rendered = [
            BilibiliHttpError::EndpointRejected,
            BilibiliHttpError::Transport,
            BilibiliHttpError::Timeout,
        ]
        .map(|error| error.to_string())
        .join(" ");
        assert!(!rendered.contains("http://"));
        assert!(!rendered.contains("https://"));
        assert!(!rendered.contains("token"));
        assert!(!rendered.contains("cookie"));
    }

    #[test]
    #[ignore = "explicit opt-in live Bilibili check; never part of default or CI tests"]
    fn bilibili_live_public_video_metadata_and_playurl_observation() {
        tauri::async_runtime::block_on(async {
            const PUBLIC_BVID: &str = "BV1Q541167Qg";
            let client = BilibiliHttpClient::new().expect("production Bilibili client");
            let view = client
                .fetch_view(PUBLIC_BVID)
                .await
                .expect("live view response");
            let metadata = select_video_metadata(&view, PUBLIC_BVID);
            let cid = metadata
                .cid
                .as_ref()
                .and_then(Value::as_u64)
                .filter(|cid| *cid != 0)
                .expect("live first-page CID");
            let play = client
                .fetch_playurl(&metadata.bvid, cid)
                .await
                .expect("live play-url response");
            let selected = choose_audio_track(&play).expect("live DASH audio");
            let candidate = find_audio_candidate(&play, &selected.primary_url)
                .expect("selected live audio descriptor");
            let codec = candidate
                .get("codecs")
                .or_else(|| candidate.get("codec"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mime_type = candidate
                .get("mimeType")
                .or_else(|| candidate.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let bandwidth = selected
                .bandwidth
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "missing".to_owned());
            let page_count = view
                .get("pages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let collection_sections = metadata
                .collection
                .as_ref()
                .map_or(0, |collection| collection.sections.len());
            let audio_candidate_count = audio_candidates(&play).len();
            let backup_count = selected.backup_urls.len();
            let headers = api_headers();

            assert!(headers.contains_key(ACCEPT));
            assert!(headers.contains_key(REFERER));
            assert!(headers.contains_key(USER_AGENT));

            println!(
                "bilibili-live-observation bvid={} cid={} pages={} collection_sections={} audio_candidates={} codec={} mime_type={} bandwidth={} backup_candidates={} accept=legacy-json referer=bilibili user_agent=browser-like cookies=false authorization=false media_download=false",
                metadata.bvid,
                cid,
                page_count,
                collection_sections,
                audio_candidate_count,
                codec,
                mime_type,
                bandwidth,
                backup_count,
            );
        });
    }

    fn audio_candidates(play: &Value) -> Vec<&serde_json::Map<String, Value>> {
        let dash = play.get("dash").and_then(Value::as_object);
        let mut candidates = Vec::new();
        if let Some(audio) = dash
            .and_then(|dash| dash.get("audio"))
            .and_then(Value::as_array)
        {
            candidates.extend(audio.iter().filter_map(Value::as_object));
        }
        if let Some(audio) = dash
            .and_then(|dash| dash.get("dolby"))
            .and_then(Value::as_object)
            .and_then(|dolby| dolby.get("audio"))
            .and_then(Value::as_array)
        {
            candidates.extend(audio.iter().filter_map(Value::as_object));
        }
        if let Some(audio) = dash
            .and_then(|dash| dash.get("flac"))
            .and_then(Value::as_object)
            .and_then(|flac| flac.get("audio"))
        {
            if let Some(audio) = audio.as_object() {
                candidates.push(audio);
            }
            if let Some(audio) = audio.as_array() {
                candidates.extend(audio.iter().filter_map(Value::as_object));
            }
        }
        candidates
    }

    fn find_audio_candidate<'a>(
        play: &'a Value,
        primary_url: &str,
    ) -> Option<&'a serde_json::Map<String, Value>> {
        audio_candidates(play).into_iter().find(|candidate| {
            candidate
                .get("baseUrl")
                .or_else(|| candidate.get("base_url"))
                .and_then(Value::as_str)
                == Some(primary_url)
        })
    }
}
