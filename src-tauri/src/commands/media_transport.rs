//! Pull-based native audio transport used instead of the non-cancellable,
//! complete-body Tauri URI-scheme responder.

use super::media::{MediaResolveState, MediaTokenError};
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::{Request as UpstreamRequest, Response};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH,
    CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_RANGE, LAST_MODIFIED, LOCATION, RANGE, REFERER,
    USER_AGENT,
};
use reqwest::{Method, Url};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;
use tokio::sync::{watch, Mutex as AsyncMutex};
use uuid::Uuid;

type UpstreamResponse = Response<Incoming>;

const AUDIO_HEADER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_IPC_CHUNK_BYTES: usize = 64 * 1024;
const MAX_UPSTREAM_BUFFER_BYTES: usize = MAX_IPC_CHUNK_BYTES;
// Hyper's max_buf_size limits header parsing, not the size of body data frames.
// A fixed HTTP/1 read size keeps each body frame below the pending-copy limit.
const UPSTREAM_READ_BYTES: usize = 16 * 1024;
const MAX_RANGE_HEADER_BYTES: usize = 256;
const MAX_IF_RANGE_HEADER_BYTES: usize = 1024;
const MAX_FORWARDED_HEADER_VALUE_BYTES: usize = 8 * 1024;
const ACCEPT_VALUE: &str = "*/*";
const REFERER_VALUE: &str = "https://www.bilibili.com/";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct MediaUpstreamClient {
    client: HyperClient<HttpsConnector<HttpConnector>, Empty<Bytes>>,
    header_timeout: Duration,
    allow_local_test_urls: bool,
}

impl MediaUpstreamClient {
    pub fn new() -> Result<Self, String> {
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http1()
            .build();
        let client = HyperClient::builder(TokioExecutor::new())
            .http1_read_buf_exact_size(UPSTREAM_READ_BYTES)
            .build(connector);
        Ok(Self {
            client,
            header_timeout: AUDIO_HEADER_TIMEOUT,
            allow_local_test_urls: false,
        })
    }

    #[cfg(test)]
    fn for_local_test() -> Self {
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client = HyperClient::builder(TokioExecutor::new())
            .http1_read_buf_exact_size(UPSTREAM_READ_BYTES)
            .build(connector);
        Self {
            client,
            header_timeout: Duration::from_secs(2),
            allow_local_test_urls: true,
        }
    }

    async fn request(
        &self,
        method: Method,
        headers: HeaderMap,
        urls: impl Iterator<Item = String>,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<UpstreamResponse, AudioTransportError> {
        for raw_url in urls {
            let Ok(url) = self.validate_url(&raw_url) else {
                continue;
            };
            let response = match self
                .request_url(method.clone(), headers.clone(), url, cancel)
                .await
            {
                Ok(response) => response,
                Err(AudioTransportError::Cancelled) => return Err(AudioTransportError::Cancelled),
                Err(_) => continue,
            };
            if response.status().is_success()
                || response.status() == hyper::StatusCode::RANGE_NOT_SATISFIABLE
            {
                return Ok(response);
            }
            drop(response);
        }
        Err(AudioTransportError::UpstreamUnavailable)
    }

    async fn request_url(
        &self,
        method: Method,
        headers: HeaderMap,
        mut url: Url,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<UpstreamResponse, AudioTransportError> {
        let deadline = tokio::time::Instant::now() + self.header_timeout;
        for redirect_count in 0..=10 {
            if *cancel.borrow() {
                return Err(AudioTransportError::Cancelled);
            }
            let mut request = UpstreamRequest::builder()
                .method(method.clone())
                .uri(url.as_str())
                .body(Empty::<Bytes>::new())
                .map_err(|_| AudioTransportError::UpstreamUnavailable)?;
            *request.headers_mut() = headers.clone();
            let result = tokio::select! {
                biased;
                changed = cancel.changed() => {
                    let _ = changed;
                    return Err(AudioTransportError::Cancelled);
                }
                response = tokio::time::timeout_at(deadline, self.client.request(request)) => response,
            };
            let response = match result {
                Ok(Ok(response)) => response,
                Ok(Err(_)) | Err(_) => return Err(AudioTransportError::UpstreamUnavailable),
            };
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if redirect_count == 10 {
                    return Err(AudioTransportError::UpstreamUnavailable);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(AudioTransportError::UpstreamUnavailable)?;
                let next = url
                    .join(location)
                    .map_err(|_| AudioTransportError::UpstreamUnavailable)?;
                url = self.validate_url(next.as_str())?;
                drop(response);
                continue;
            }
            return Ok(response);
        }
        Err(AudioTransportError::UpstreamUnavailable)
    }

    fn validate_url(&self, raw_url: &str) -> Result<Url, AudioTransportError> {
        let url = Url::parse(raw_url).map_err(|_| AudioTransportError::UpstreamUnavailable)?;
        let https = url.scheme() == "https";
        let local_test_http = self.allow_local_test_urls
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some();
        if !(https || local_test_http)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(AudioTransportError::UpstreamUnavailable);
        }
        Ok(url)
    }
}

#[derive(Default, Clone)]
pub struct MediaTransferState {
    active: Arc<Mutex<BTreeMap<Uuid, Arc<ActiveAudioTransfer>>>>,
}

struct ActiveAudioTransfer {
    cancel: watch::Sender<bool>,
    body: AsyncMutex<TransferBody>,
}

#[derive(Default)]
struct TransferBody {
    response: Option<UpstreamResponse>,
    pending: Vec<u8>,
    pending_offset: usize,
}

fn store_upstream_chunk(body: &mut TransferBody, bytes: Bytes) -> Result<(), AudioTransportError> {
    if bytes.len() > MAX_UPSTREAM_BUFFER_BYTES {
        eprintln!("media audio read: upstream body frame exceeded {} bytes ({} bytes)", MAX_UPSTREAM_BUFFER_BYTES, bytes.len());
        return Err(AudioTransportError::BodyReadFailed);
    }
    body.pending = bytes.to_vec();
    Ok(())
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaAudioResponseHead {
    pub request_id: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq)]
enum AudioTransportError {
    InvalidToken,
    ExpiredToken,
    StateUnavailable,
    InvalidRequestId,
    DuplicateRequest,
    UnsupportedMethod,
    InvalidHeader,
    UpstreamUnavailable,
    Cancelled,
    TransferNotFound,
    BodyReadFailed,
}

impl AudioTransportError {
    fn message(&self) -> &'static str {
        match self {
            Self::InvalidToken => "媒体播放地址无效",
            Self::ExpiredToken => "播放地址已失效，请重新解析",
            Self::StateUnavailable => "媒体状态不可用",
            Self::InvalidRequestId => "媒体请求标识无效",
            Self::DuplicateRequest => "媒体请求已存在",
            Self::UnsupportedMethod => "不支持的媒体请求方法",
            Self::InvalidHeader => "媒体请求头无效",
            Self::UpstreamUnavailable => "Bilibili 音频源暂时不可用，请重新解析",
            Self::Cancelled => "媒体请求已取消",
            Self::TransferNotFound => "媒体请求已结束或不存在",
            Self::BodyReadFailed => "读取 Bilibili 音频失败",
        }
    }
}

impl MediaTransferState {
    fn insert(&self, request_id: Uuid) -> Result<Arc<ActiveAudioTransfer>, AudioTransportError> {
        let (cancel, _) = watch::channel(false);
        let transfer = Arc::new(ActiveAudioTransfer {
            cancel,
            body: AsyncMutex::new(TransferBody::default()),
        });
        let mut active = self
            .active
            .lock()
            .map_err(|_| AudioTransportError::StateUnavailable)?;
        if active.contains_key(&request_id) {
            return Err(AudioTransportError::DuplicateRequest);
        }
        active.insert(request_id, Arc::clone(&transfer));
        Ok(transfer)
    }

    fn get(&self, request_id: Uuid) -> Result<Arc<ActiveAudioTransfer>, AudioTransportError> {
        self.active
            .lock()
            .map_err(|_| AudioTransportError::StateUnavailable)?
            .get(&request_id)
            .cloned()
            .ok_or(AudioTransportError::TransferNotFound)
    }

    fn remove(&self, request_id: Uuid) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&request_id);
        }
    }

    fn cancel(&self, request_id: Uuid) -> Result<bool, AudioTransportError> {
        let transfer = self
            .active
            .lock()
            .map_err(|_| AudioTransportError::StateUnavailable)?
            .remove(&request_id);
        if let Some(transfer) = transfer {
            let _ = transfer.cancel.send(true);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[tauri::command]
pub async fn media_audio_open(
    request_id: String,
    token: String,
    method: String,
    range: Option<String>,
    if_range: Option<String>,
    media: State<'_, MediaResolveState>,
    upstream: State<'_, MediaUpstreamClient>,
    transfers: State<'_, MediaTransferState>,
) -> Result<MediaAudioResponseHead, String> {
    open_audio(
        &request_id,
        &token,
        &method,
        range.as_deref(),
        if_range.as_deref(),
        media.inner(),
        upstream.inner(),
        transfers.inner(),
    )
    .await
    .map_err(|error| error.message().to_owned())
}

#[tauri::command]
pub async fn media_audio_read(
    request_id: String,
    transfers: State<'_, MediaTransferState>,
) -> Result<tauri::ipc::Response, String> {
    read_audio_chunk(&request_id, transfers.inner())
        .await
        .map(tauri::ipc::Response::new)
        .map_err(|error| error.message().to_owned())
}

#[tauri::command]
pub async fn media_audio_cancel(
    request_id: String,
    transfers: State<'_, MediaTransferState>,
) -> Result<bool, String> {
    let request_id = parse_request_id(&request_id).map_err(|error| error.message().to_owned())?;
    transfers
        .cancel(request_id)
        .map_err(|error| error.message().to_owned())
}

async fn open_audio(
    request_id: &str,
    token: &str,
    method: &str,
    range: Option<&str>,
    if_range: Option<&str>,
    media: &MediaResolveState,
    upstream: &MediaUpstreamClient,
    transfers: &MediaTransferState,
) -> Result<MediaAudioResponseHead, AudioTransportError> {
    let request_id = parse_request_id(request_id)?;
    let method = match method {
        "GET" => Method::GET,
        "HEAD" => Method::HEAD,
        _ => return Err(AudioTransportError::UnsupportedMethod),
    };
    let source = media.source_for_token(token).map_err(map_token_error)?;
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_VALUE));
    headers.insert(REFERER, HeaderValue::from_static(REFERER_VALUE));
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    if let Some(range) = range {
        if range.len() > MAX_RANGE_HEADER_BYTES {
            return Err(AudioTransportError::InvalidHeader);
        }
        headers.insert(
            RANGE,
            HeaderValue::from_str(range).map_err(|_| AudioTransportError::InvalidHeader)?,
        );
    }
    if let Some(if_range) = if_range {
        if if_range.len() > MAX_IF_RANGE_HEADER_BYTES {
            return Err(AudioTransportError::InvalidHeader);
        }
        headers.insert(
            IF_RANGE,
            HeaderValue::from_str(if_range).map_err(|_| AudioTransportError::InvalidHeader)?,
        );
    }

    let transfer = transfers.insert(request_id)?;
    let mut cancel = transfer.cancel.subscribe();
    let urls = std::iter::once(source.primary_url).chain(source.backup_urls);
    let response = match upstream
        .request(method.clone(), headers, urls, &mut cancel)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            transfers.remove(request_id);
            return Err(error);
        }
    };
    if *cancel.borrow() {
        transfers.remove(request_id);
        return Err(AudioTransportError::Cancelled);
    }

    let mut response_headers = forwarded_headers(response.headers());
    if response.status().is_success()
        && response_headers
            .get("content-type")
            .map_or(true, |value| value == "application/octet-stream")
    {
        response_headers.insert("content-type".to_owned(), "audio/mp4".to_owned());
    }
    let result = MediaAudioResponseHead {
        request_id: request_id.to_string(),
        status: response.status().as_u16(),
        headers: response_headers,
    };

    if method == Method::HEAD {
        transfers.remove(request_id);
    } else {
        let mut body = transfer.body.lock().await;
        if *cancel.borrow() {
            drop(body);
            drop(response);
            transfers.remove(request_id);
            return Err(AudioTransportError::Cancelled);
        }
        body.response = Some(response);
    }
    Ok(result)
}

async fn read_audio_chunk(
    request_id: &str,
    transfers: &MediaTransferState,
) -> Result<Vec<u8>, AudioTransportError> {
    let request_id = parse_request_id(request_id)?;
    let transfer = transfers.get(request_id)?;
    let mut cancel = transfer.cancel.subscribe();
    let mut body = transfer.body.lock().await;
    loop {
        if *cancel.borrow() {
            drop(body);
            transfers.remove(request_id);
            return Err(AudioTransportError::Cancelled);
        }
        if body.pending_offset < body.pending.len() {
            let end = (body.pending_offset + MAX_IPC_CHUNK_BYTES).min(body.pending.len());
            let chunk = body.pending[body.pending_offset..end].to_vec();
            body.pending_offset = end;
            if body.pending_offset == body.pending.len() {
                body.pending.clear();
                body.pending_offset = 0;
            }
            return Ok(chunk);
        }

        let response = body
            .response
            .as_mut()
            .ok_or(AudioTransportError::TransferNotFound)?;
        let next = tokio::select! {
            biased;
            changed = cancel.changed() => {
                let _ = changed;
                drop(body);
                transfers.remove(request_id);
                return Err(AudioTransportError::Cancelled);
            }
            result = response.frame() => result,
        };
        match next {
            Some(Ok(frame)) => match frame.into_data() {
                Ok(bytes) if bytes.is_empty() => continue,
                Ok(bytes) => match store_upstream_chunk(&mut body, bytes) {
                    Ok(()) => {}
                    Err(error) => {
                        drop(body);
                        let _ = transfer.cancel.send(true);
                        transfers.remove(request_id);
                        return Err(error);
                    }
                },
                Err(_trailers) => continue,
            },
            None => {
                drop(body);
                transfers.remove(request_id);
                return Ok(Vec::new());
            }
            Some(Err(error)) => {
                eprintln!(
                    "media audio read: Hyper body error (parse={}, incomplete={}, closed={}, cancelled={})",
                    error.is_parse(),
                    error.is_incomplete_message(),
                    error.is_closed(),
                    error.is_canceled()
                );
                drop(body);
                let _ = transfer.cancel.send(true);
                transfers.remove(request_id);
                return Err(AudioTransportError::BodyReadFailed);
            }
        }
    }
}

fn parse_request_id(value: &str) -> Result<Uuid, AudioTransportError> {
    Uuid::parse_str(value).map_err(|_| AudioTransportError::InvalidRequestId)
}

fn map_token_error(error: MediaTokenError) -> AudioTransportError {
    match error {
        MediaTokenError::InvalidToken => AudioTransportError::InvalidToken,
        MediaTokenError::ExpiredToken => AudioTransportError::ExpiredToken,
        MediaTokenError::StateUnavailable => AudioTransportError::StateUnavailable,
    }
}

fn forwarded_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    [
        ACCEPT_RANGES,
        CACHE_CONTROL,
        CONTENT_LENGTH,
        CONTENT_RANGE,
        CONTENT_TYPE,
        ETAG,
        LAST_MODIFIED,
    ]
    .into_iter()
    .filter_map(|name: HeaderName| {
        headers
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= MAX_FORWARDED_HEADER_VALUE_BYTES)
            .map(|value| (name.as_str().to_owned(), value.to_owned()))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bilibili::SelectedAudioTrack;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    struct MockServer {
        origin: Url,
        request: Receiver<String>,
        release: Option<Sender<()>>,
        closed: Option<Receiver<bool>>,
        thread: Option<JoinHandle<()>>,
    }

    impl MockServer {
        fn start(response: Vec<u8>) -> Self {
            Self::with_handler(move |stream| {
                let mut stream = stream;
                let _ = stream.write_all(&response);
            })
        }

        fn with_handler(handler: impl FnOnce(TcpStream) + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind media upstream mock");
            let address = listener.local_addr().expect("mock upstream address");
            let (request_tx, request) = mpsc::channel();
            let thread = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept media request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("request timeout");
                request_tx
                    .send(read_request(&mut stream))
                    .expect("capture request");
                handler(stream);
            });
            Self {
                origin: Url::parse(&format!("http://{address}/audio")).expect("mock URL"),
                request,
                release: None,
                closed: None,
                thread: Some(thread),
            }
        }

        fn delayed_headers() -> Self {
            let (release_tx, release_rx) = mpsc::channel();
            let mut server = Self::with_handler(move |mut stream| {
                let _ = release_rx.recv_timeout(Duration::from_secs(3));
                let response = raw_response("200 OK", "Content-Length: 4\r\n", "DATA");
                let _ = stream.write_all(&response);
                let mut byte = [0_u8; 1];
                let _ = stream.read(&mut byte);
            });
            server.release = Some(release_tx);
            server
        }

        fn delayed_body() -> Self {
            let (closed_tx, closed) = mpsc::channel();
            let server = Self::with_handler(move |mut stream| {
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nContent-Length: 16777216\r\nConnection: keep-alive\r\n\r\n";
                stream.write_all(head).expect("write response head");
                stream
                    .write_all(&vec![0x5a; MAX_IPC_CHUNK_BYTES / 8])
                    .expect("write first body chunk");
                stream.flush().expect("flush first chunk");
                let mut byte = [0_u8; 1];
                let closed = loop {
                    match stream.read(&mut byte) {
                        Ok(0) | Err(_) => break true,
                        Ok(_) => {}
                    }
                };
                let _ = closed_tx.send(closed);
            });
            Self {
                closed: Some(closed),
                ..server
            }
        }

        fn wait_request(&self) -> String {
            self.request
                .recv_timeout(Duration::from_secs(2))
                .expect("upstream request observed")
        }

        fn finish(mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
            if let Some(thread) = self.thread.take() {
                thread.join().expect("mock thread");
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(bytes).expect("HTTP request text")
    }

    fn raw_response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn registered_media(primary: &str, backups: &[String]) -> (MediaResolveState, String) {
        let media = MediaResolveState::default();
        let address = media
            .store(SelectedAudioTrack {
                primary_url: primary.to_owned(),
                backup_urls: backups.to_vec(),
                bandwidth: None,
            })
            .expect("register synthetic media source");
        let token = address
            .strip_prefix("media-token://")
            .expect("opaque token")
            .to_owned();
        (media, token)
    }

    fn request_id() -> String {
        Uuid::new_v4().to_string()
    }
    fn test_client() -> MediaUpstreamClient {
        MediaUpstreamClient::for_local_test()
    }

    #[test]
    fn media_transport_forwards_range_if_range_and_legacy_response_headers() {
        tauri::async_runtime::block_on(async {
            let server = MockServer::start(raw_response(
                "206 Partial Content",
                "Content-Type: application/octet-stream\r\nContent-Range: bytes 0-3/100\r\nAccept-Ranges: bytes\r\nCache-Control: public, max-age=60\r\nETag: etag-v1\r\nLast-Modified: synthetic-date\r\n",
                "DATA",
            ));
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let head = open_audio(
                &id,
                &token,
                "GET",
                Some("bytes=0-3"),
                Some("etag-v1"),
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("open range");
            assert_eq!(head.status, 206);
            assert_eq!(head.headers["content-range"], "bytes 0-3/100");
            assert_eq!(head.headers["accept-ranges"], "bytes");
            assert_eq!(head.headers["cache-control"], "public, max-age=60");
            assert_eq!(head.headers["etag"], "etag-v1");
            assert_eq!(head.headers["last-modified"], "synthetic-date");
            assert_eq!(head.headers["content-type"], "audio/mp4");
            assert!(!serde_json::to_string(&head).unwrap().contains("127.0.0.1"));

            let request = server.wait_request().to_ascii_lowercase();
            assert!(request.starts_with("get /audio http/1.1\r\n"));
            assert!(request.contains("range: bytes=0-3\r\n"));
            assert!(request.contains("if-range: etag-v1\r\n"));
            assert!(request.contains("accept: */*\r\n"));
            assert!(request.contains("referer: https://www.bilibili.com/\r\n"));
            assert!(request.contains("user-agent: mozilla/5.0"));
            assert_eq!(read_audio_chunk(&id, &transfers).await.unwrap(), b"DATA");
            assert!(read_audio_chunk(&id, &transfers).await.unwrap().is_empty());
            server.finish();
        });
    }

    #[test]
    fn media_transport_forwards_open_ended_and_suffix_ranges() {
        tauri::async_runtime::block_on(async {
            for range in ["bytes=100-", "bytes=-4"] {
                let server = MockServer::start(raw_response(
                    "206 Partial Content",
                    "Content-Type: audio/mp4\r\nContent-Range: bytes 100-103/1000\r\nAccept-Ranges: bytes\r\n",
                    "DATA",
                ));
                let (media, token) = registered_media(server.origin.as_str(), &[]);
                let transfers = MediaTransferState::default();
                let id = request_id();
                let head = open_audio(
                    &id,
                    &token,
                    "GET",
                    Some(range),
                    None,
                    &media,
                    &test_client(),
                    &transfers,
                )
                .await
                .expect("range response");
                assert_eq!(head.status, 206);
                assert_eq!(head.headers["content-range"], "bytes 100-103/1000");
                assert!(server
                    .wait_request()
                    .to_ascii_lowercase()
                    .contains(&format!("range: {range}\r\n")));
                assert_eq!(read_audio_chunk(&id, &transfers).await.unwrap(), b"DATA");
                assert!(read_audio_chunk(&id, &transfers).await.unwrap().is_empty());
                server.finish();
            }
        });
    }

    #[test]
    fn media_transport_handles_head_and_unsatisfied_ranges_without_url_exposure() {
        tauri::async_runtime::block_on(async {
            let head_response = b"HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nContent-Length: 100\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n".to_vec();
            let server = MockServer::start(head_response);
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let head = open_audio(
                &request_id(),
                &token,
                "HEAD",
                None,
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("HEAD request");
            assert_eq!(head.status, 200);
            assert_eq!(head.headers["content-length"], "100");
            assert_eq!(head.headers["accept-ranges"], "bytes");
            assert!(transfers.active.lock().unwrap().is_empty());
            assert!(server.wait_request().starts_with("HEAD /audio HTTP/1.1"));
            server.finish();

            let server = MockServer::start(raw_response(
                "416 Range Not Satisfiable",
                "Content-Range: bytes */100\r\nCache-Control: no-store\r\n",
                "",
            ));
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let response = open_audio(
                &id,
                &token,
                "GET",
                Some("bytes=100-200"),
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("416 metadata");
            assert_eq!(response.status, 416);
            assert_eq!(response.headers["content-range"], "bytes */100");
            assert!(!serde_json::to_string(&response)
                .unwrap()
                .contains("127.0.0.1"));
            assert!(server
                .wait_request()
                .to_ascii_lowercase()
                .contains("range: bytes=100-200\r\n"));
            let _ = transfers.cancel(Uuid::parse_str(&id).unwrap()).unwrap();
            server.finish();
        });
    }

    #[test]
    fn media_transport_retries_registered_backup_and_follows_redirects() {
        tauri::async_runtime::block_on(async {
            let primary = MockServer::start(raw_response(
                "503 Service Unavailable",
                "Content-Type: text/plain\r\n",
                "busy",
            ));
            let backup =
                MockServer::start(raw_response("200 OK", "Content-Type: audio/mp4\r\n", "OK"));
            let (media, token) =
                registered_media(primary.origin.as_str(), &[backup.origin.to_string()]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let head = open_audio(
                &id,
                &token,
                "GET",
                None,
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("backup source");
            assert_eq!(head.status, 200);
            assert!(primary.wait_request().starts_with("GET /audio HTTP/1.1"));
            assert!(backup.wait_request().starts_with("GET /audio HTTP/1.1"));
            assert_eq!(read_audio_chunk(&id, &transfers).await.unwrap(), b"OK");
            assert!(read_audio_chunk(&id, &transfers).await.unwrap().is_empty());
            primary.finish();
            backup.finish();

            let target =
                MockServer::start(raw_response("200 OK", "Content-Type: audio/mp4\r\n", "RD"));
            let location = target.origin.to_string();
            let redirect = MockServer::start(raw_response(
                "302 Found",
                &format!("Location: {location}\r\n"),
                "",
            ));
            let (media, token) = registered_media(redirect.origin.as_str(), &[]);
            let id = request_id();
            let head = open_audio(
                &id,
                &token,
                "GET",
                Some("bytes=0-1"),
                Some("etag-x"),
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("follow CDN redirect");
            assert_eq!(head.status, 200);
            assert!(redirect.wait_request().starts_with("GET /audio HTTP/1.1"));
            let forwarded = target.wait_request().to_ascii_lowercase();
            assert!(forwarded.contains("range: bytes=0-1\r\n"));
            assert!(forwarded.contains("if-range: etag-x\r\n"));
            assert_eq!(read_audio_chunk(&id, &transfers).await.unwrap(), b"RD");
            assert!(read_audio_chunk(&id, &transfers).await.unwrap().is_empty());
            redirect.finish();
            target.finish();
        });
    }

    #[test]
    fn media_transport_cancel_interrupts_header_wait_and_closes_active_upstream_body() {
        tauri::async_runtime::block_on(async {
            let server = MockServer::delayed_headers();
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let task_id = id.clone();
            let task_media = media.clone();
            let task_transfers = transfers.clone();
            let task = tauri::async_runtime::spawn(async move {
                open_audio(
                    &task_id,
                    &token,
                    "GET",
                    None,
                    None,
                    &task_media,
                    &test_client(),
                    &task_transfers,
                )
                .await
            });
            let _ = server.wait_request();
            let started = Instant::now();
            assert!(transfers.cancel(Uuid::parse_str(&id).unwrap()).unwrap());
            assert_eq!(
                task.await.expect("open task"),
                Err(AudioTransportError::Cancelled)
            );
            assert!(started.elapsed() < Duration::from_millis(300));
            server.finish();

            let server = MockServer::delayed_body();
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let _ = open_audio(
                &id,
                &token,
                "GET",
                None,
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("open streaming body");
            let first = read_audio_chunk(&id, &transfers).await.unwrap();
            assert!(!first.is_empty());
            assert!(first.len() <= MAX_IPC_CHUNK_BYTES);
            let started = Instant::now();
            let mut next_read = Box::pin(read_audio_chunk(&id, &transfers));
            loop {
                tokio::select! {
                    result = &mut next_read => {
                        let bytes = result.expect("body remains active");
                        assert!(!bytes.is_empty(), "upstream unexpectedly ended");
                        assert!(started.elapsed() < Duration::from_millis(300));
                        next_read = Box::pin(read_audio_chunk(&id, &transfers));
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => break,
                }
            }
            assert!(transfers.cancel(Uuid::parse_str(&id).unwrap()).unwrap());
            assert!(started.elapsed() < Duration::from_millis(100));
            assert_eq!(next_read.await, Err(AudioTransportError::Cancelled));
            assert_eq!(
                server
                    .closed
                    .as_ref()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2)),
                Ok(true)
            );
            assert_eq!(
                read_audio_chunk(&id, &transfers).await,
                Err(AudioTransportError::TransferNotFound)
            );
            server.finish();
        });
    }

    #[test]
    fn media_transport_long_unranged_get_uses_only_bounded_pull_chunks() {
        tauri::async_runtime::block_on(async {
            let total = 8 * 1024 * 1024;
            let (write_result_tx, write_result_rx) = mpsc::channel();
            let server = MockServer::with_handler(move |mut stream| {
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
                stream
                    .write_all(head.as_bytes())
                    .expect("write long response head");
                let body = vec![0x3c; total];
                let result = stream.write_all(&body).map_err(|error| error.kind());
                write_result_tx.send(result).expect("report long response write");
            });
            let (media, token) = registered_media(server.origin.as_str(), &[]);
            let transfers = MediaTransferState::default();
            let id = request_id();
            let head = open_audio(
                &id,
                &token,
                "GET",
                None,
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .expect("open full response as stream");
            assert_eq!(head.status, 200);
            assert_eq!(head.headers["content-length"], total.to_string());
            assert!(server.wait_request().starts_with("GET /audio HTTP/1.1"));
            let first_chunk = read_audio_chunk(&id, &transfers).await.unwrap();
            assert!(!first_chunk.is_empty());
            let mut total_received = first_chunk.len();
            let mut maximum_chunk = first_chunk.len();
            let transfer = transfers
                .get(Uuid::parse_str(&id).unwrap())
                .expect("un-ranged response remains open while pulling");
            let body = transfer.body.lock().await;
            assert!(body.response.is_some());
            assert!(body.pending.len() <= MAX_IPC_CHUNK_BYTES);
            assert!(total_received < total);
            drop(body);
            drop(transfer);
            loop {
                let chunk = read_audio_chunk(&id, &transfers).await.unwrap();
                if chunk.is_empty() {
                    break;
                }
                maximum_chunk = maximum_chunk.max(chunk.len());
                total_received += chunk.len();
            }
            assert_eq!(total_received, total);
            assert!(maximum_chunk <= UPSTREAM_READ_BYTES);
            assert_eq!(write_result_rx.recv_timeout(Duration::from_secs(5)).expect("long response write result"), Ok(()));
            server.finish();
        });
    }

    #[test]
    fn media_transport_rejects_an_upstream_chunk_above_the_configured_bound() {
        let mut body = TransferBody::default();
        store_upstream_chunk(
            &mut body,
            Bytes::from(vec![0x3c; MAX_UPSTREAM_BUFFER_BYTES]),
        )
        .expect("configured maximum is accepted");
        assert_eq!(body.pending.len(), MAX_UPSTREAM_BUFFER_BYTES);

        let oversized = Bytes::from(vec![0x5a; MAX_UPSTREAM_BUFFER_BYTES + 1]);
        assert_eq!(
            store_upstream_chunk(&mut body, oversized),
            Err(AudioTransportError::BodyReadFailed)
        );
        assert_eq!(body.pending.len(), MAX_UPSTREAM_BUFFER_BYTES);
    }

    #[test]
    fn media_transport_rejects_invalid_expired_and_non_registered_urls_without_leaking_urls() {
        tauri::async_runtime::block_on(async {
            let transfers = MediaTransferState::default();
            let client = test_client();
            let invalid_state = MediaResolveState::default();
            let invalid = open_audio(
                &request_id(),
                "not-a-registered-token",
                "GET",
                None,
                None,
                &invalid_state,
                &client,
                &transfers,
            )
            .await
            .unwrap_err();
            assert_eq!(invalid, AudioTransportError::InvalidToken);
            assert_eq!(invalid.message(), "媒体播放地址无效");

            let secret_source = "https://secret-cdn.example/audio";
            let (expired_state, expired_token) = registered_media(secret_source, &[]);
            expired_state.expire_token_for_test(&expired_token);
            let expired = open_audio(
                &request_id(),
                &expired_token,
                "GET",
                None,
                None,
                &expired_state,
                &client,
                &transfers,
            )
            .await
            .unwrap_err();
            assert_eq!(expired, AudioTransportError::ExpiredToken);
            assert_eq!(expired.message(), "播放地址已失效，请重新解析");
            assert!(!invalid.message().contains("secret-cdn"));
            assert!(!expired.message().contains("secret-cdn"));

            let (unregistered_url_state, url_token) =
                registered_media("http://example.com/audio", &[]);
            let rejected = open_audio(
                &request_id(),
                &url_token,
                "GET",
                None,
                None,
                &unregistered_url_state,
                &client,
                &transfers,
            )
            .await
            .unwrap_err();
            assert_eq!(rejected, AudioTransportError::UpstreamUnavailable);
        });
    }

    #[test]
    fn media_transport_rejects_invalid_methods_and_header_values() {
        tauri::async_runtime::block_on(async {
            let (media, token) = registered_media("https://cdn.example/audio", &[]);
            let transfers = MediaTransferState::default();
            let client = test_client();
            assert_eq!(
                open_audio(
                    &request_id(),
                    &token,
                    "TRACE",
                    None,
                    None,
                    &media,
                    &client,
                    &transfers
                )
                .await
                .unwrap_err(),
                AudioTransportError::UnsupportedMethod
            );
            assert_eq!(
                open_audio(
                    &request_id(),
                    &token,
                    "GET",
                    Some("bytes=0-\r\ninjected: yes"),
                    None,
                    &media,
                    &client,
                    &transfers
                )
                .await
                .unwrap_err(),
                AudioTransportError::InvalidHeader
            );
            assert_eq!(
                open_audio(
                    &request_id(),
                    &token,
                    "GET",
                    Some("bytes=0-1"),
                    Some("bad\r\nvalue"),
                    &media,
                    &client,
                    &transfers
                )
                .await
                .unwrap_err(),
                AudioTransportError::InvalidHeader
            );
            assert_eq!(
                open_audio(
                    &request_id(),
                    &token,
                    "GET",
                    Some(&"x".repeat(MAX_RANGE_HEADER_BYTES + 1)),
                    None,
                    &media,
                    &client,
                    &transfers
                )
                .await
                .unwrap_err(),
                AudioTransportError::InvalidHeader
            );
            assert_eq!(
                open_audio(
                    &request_id(),
                    &token,
                    "GET",
                    None,
                    Some(&"x".repeat(MAX_IF_RANGE_HEADER_BYTES + 1)),
                    &media,
                    &client,
                    &transfers
                )
                .await
                .unwrap_err(),
                AudioTransportError::InvalidHeader
            );
        });
    }

    #[test]
    fn media_transport_maps_exhausted_upstream_errors_without_exposing_urls() {
        tauri::async_runtime::block_on(async {
            let primary = MockServer::start(raw_response(
                "503 Service Unavailable",
                "",
                "primary secret",
            ));
            let backup = MockServer::start(raw_response("502 Bad Gateway", "", "backup secret"));
            let (media, token) = registered_media(
                primary.origin.as_str(),
                &[backup.origin.as_str().to_owned()],
            );
            let transfers = MediaTransferState::default();
            let error = open_audio(
                &request_id(),
                &token,
                "GET",
                None,
                None,
                &media,
                &test_client(),
                &transfers,
            )
            .await
            .unwrap_err();
            assert_eq!(error, AudioTransportError::UpstreamUnavailable);
            assert_eq!(error.message(), "Bilibili 音频源暂时不可用，请重新解析");
            let primary_request = primary.wait_request();
            let backup_request = backup.wait_request();
            assert!(primary_request.starts_with("GET /audio HTTP/1.1"));
            assert!(backup_request.starts_with("GET /audio HTTP/1.1"));
            let rendered = format!("{} {error:?}", error.message());
            for forbidden in ["127.0.0.1", "primary secret", "backup secret"] {
                assert!(!rendered.contains(forbidden));
            }
            assert!(transfers.active.lock().unwrap().is_empty());
            primary.finish();
            backup.finish();
        });
    }
}
