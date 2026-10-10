//! Debug-only, synthetic media endpoint for the P7-T1 WebView2 transport proof.
//! This intentionally returns one complete byte buffer per protocol response,
//! matching the Tauri/Wry custom-protocol API under evaluation.

use std::{thread, time::Duration};

use tauri::http::{header, Method, Request, Response, StatusCode};
use tauri::UriSchemeResponder;

const SAMPLE_RATE: u32 = 8_000;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const RANGE_BODY_LIMIT: u64 = 256 * 1024;
const LONG_TRACK_SECONDS: u32 = 60 * 60;
const SHORT_TRACK_SECONDS: u32 = 4;

#[derive(Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

struct PlannedResponse {
    status: StatusCode,
    body_start: u64,
    body_end: Option<u64>,
    content_range: Option<String>,
}

pub fn respond(request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if path == "/slow.wav" {
        eprintln!(
            "P7-T1 media-proof slow-request-started method={method} range={}",
            range.as_deref().unwrap_or("<none>")
        );
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(3));
            let response = make_response(&method, &path, range.as_deref());
            log_response(&method, &path, range.as_deref(), &response);
            responder.respond(response);
        });
        return;
    }

    let response = make_response(&method, &path, range.as_deref());
    log_response(&method, &path, range.as_deref(), &response);
    responder.respond(response);
}

fn make_response(method: &Method, path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    if method == Method::OPTIONS {
        return base_response(StatusCode::NO_CONTENT, Vec::new(), 0, None);
    }

    if path == "/proof.html" && method == Method::GET {
        let body = include_str!("../test/media_transport_proof.html")
            .as_bytes()
            .to_vec();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .body(body)
            .expect("synthetic proof page response");
    }

    if method != Method::GET && method != Method::HEAD {
        return base_response(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), 0, None);
    }

    let seconds = match path {
        "/short.wav" | "/slow.wav" => SHORT_TRACK_SECONDS,
        "/long.wav" => LONG_TRACK_SECONDS,
        _ => return base_response(StatusCode::NOT_FOUND, Vec::new(), 0, None),
    };
    let total_bytes = wav_length(seconds);

    if method == Method::HEAD {
        return base_response(StatusCode::OK, Vec::new(), total_bytes, None);
    }

    let plan = match plan_get(total_bytes, range) {
        Ok(plan) => plan,
        Err(()) => {
            return base_response(
                StatusCode::RANGE_NOT_SATISFIABLE,
                Vec::new(),
                0,
                Some(format!("bytes */{total_bytes}")),
            )
        }
    };

    let end = plan.body_end.expect("GET response has a body range");
    let body = synthesize_wav_slice(seconds, plan.body_start, end);
    let content_length = body.len() as u64;
    base_response(plan.status, body, content_length, plan.content_range)
}

fn base_response(
    status: StatusCode,
    body: Vec<u8>,
    content_length: u64,
    content_range: Option<String>,
) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "audio/wav")
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Accept-Ranges, Content-Length, Content-Range",
        );
    if let Some(content_range) = content_range {
        response = response.header(header::CONTENT_RANGE, content_range);
    }
    response.body(body).expect("synthetic media response")
}

fn plan_get(total_bytes: u64, range_header: Option<&str>) -> Result<PlannedResponse, ()> {
    let Some(range_header) = range_header else {
        return Ok(PlannedResponse {
            status: StatusCode::OK,
            body_start: 0,
            body_end: Some(total_bytes - 1),
            content_range: None,
        });
    };

    let ByteRange { start, end } = parse_range(range_header, total_bytes)?;
    let capped_end = end.min(start.saturating_add(RANGE_BODY_LIMIT - 1));
    Ok(PlannedResponse {
        status: StatusCode::PARTIAL_CONTENT,
        body_start: start,
        body_end: Some(capped_end),
        content_range: Some(format!("bytes {start}-{capped_end}/{total_bytes}")),
    })
}

fn parse_range(value: &str, total_bytes: u64) -> Result<ByteRange, ()> {
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.is_empty() || value.contains(',') || total_bytes == 0 {
        return Err(());
    }

    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let count = suffix.min(total_bytes).min(RANGE_BODY_LIMIT);
        return Ok(ByteRange {
            start: total_bytes - count,
            end: total_bytes - 1,
        });
    }

    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= total_bytes {
        return Err(());
    }
    let end = if end.is_empty() {
        total_bytes - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?
    };
    if end < start {
        return Err(());
    }
    Ok(ByteRange {
        start,
        end: end.min(total_bytes - 1),
    })
}

fn wav_length(seconds: u32) -> u64 {
    44 + u64::from(SAMPLE_RATE)
        * u64::from(CHANNELS)
        * u64::from(BITS_PER_SAMPLE / 8)
        * u64::from(seconds)
}

fn synthesize_wav_slice(seconds: u32, start: u64, end: u64) -> Vec<u8> {
    let total_bytes = wav_length(seconds);
    let end = end.min(total_bytes - 1);
    let mut output = vec![0_u8; usize::try_from(end - start + 1).expect("bounded response body")];
    let header = wav_header(seconds);

    let header_start = start.min(header.len() as u64);
    let header_end = (end + 1).min(header.len() as u64);
    if header_start < header_end {
        let destination = usize::try_from(header_start - start).expect("header output offset");
        let source_start = usize::try_from(header_start).expect("header source offset");
        let source_end = usize::try_from(header_end).expect("header source end");
        output[destination..destination + (source_end - source_start)]
            .copy_from_slice(&header[source_start..source_end]);
    }

    let data_start = start.max(header.len() as u64);
    let data_end = end + 1;
    if data_start < data_end {
        let period = tone_period();
        let data_offset = data_start - header.len() as u64;
        let destination = usize::try_from(data_start - start).expect("audio output offset");
        let count = usize::try_from(data_end - data_start).expect("audio output length");
        for (index, byte) in output[destination..destination + count]
            .iter_mut()
            .enumerate()
        {
            *byte = period[((data_offset as usize) + index) % period.len()];
        }
    }
    output
}

fn wav_header(seconds: u32) -> [u8; 44] {
    let data_length = u32::try_from(wav_length(seconds) - 44).expect("WAV data length");
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE / 8);
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(data_length + 36).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_length.to_le_bytes());
    header
}

fn tone_period() -> [u8; 40] {
    let mut bytes = [0_u8; 40];
    for sample in 0..20 {
        let phase = std::f64::consts::TAU * 400.0 * sample as f64 / f64::from(SAMPLE_RATE);
        let value = (phase.sin() * i16::MAX as f64 * 0.4) as i16;
        bytes[sample * 2..sample * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn log_response(method: &Method, path: &str, range: Option<&str>, response: &Response<Vec<u8>>) {
    let total = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit('/').next())
        .or_else(|| {
            response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
        })
        .unwrap_or("unknown");
    eprintln!(
        "P7-T1 media-proof method={method} path={path} range={} status={} track_bytes={total} response_body_bytes={}",
        range.unwrap_or("<none>"),
        response.status().as_u16(),
        response.body().len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_endpoint_supports_get_head_and_range_forms() {
        let total = wav_length(LONG_TRACK_SECONDS);

        let proof_page = make_response(&Method::GET, "/proof.html", None);
        assert_eq!(proof_page.status(), StatusCode::OK);
        assert!(proof_page.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("proof page content type")
            .starts_with("text/html"));

        let get = make_response(&Method::GET, "/short.wav", None);
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.body().len(), wav_length(SHORT_TRACK_SECONDS) as usize);
        assert_eq!(
            get.headers()[header::CONTENT_LENGTH],
            wav_length(SHORT_TRACK_SECONDS).to_string()
        );
        assert_eq!(&get.body()[0..4], b"RIFF");

        let head = make_response(&Method::HEAD, "/long.wav", None);
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.body().len(), 0);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], total.to_string());
        assert_eq!(head.headers()[header::ACCEPT_RANGES], "bytes");

        for (range, start, end) in [
            ("bytes=100-199", 100, 199),
            ("bytes=100-", 100, 100 + RANGE_BODY_LIMIT - 1),
            ("bytes=-200", total - 200, total - 1),
        ] {
            let response = make_response(&Method::GET, "/long.wav", Some(range));
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{range}");
            assert_eq!(response.body().len() as u64, end - start + 1, "{range}");
            assert_eq!(
                response.headers()[header::CONTENT_LENGTH],
                response.body().len().to_string(),
                "{range}"
            );
            assert_eq!(
                response.headers()[header::CONTENT_RANGE],
                format!("bytes {start}-{end}/{total}"),
                "{range}"
            );
            assert!(response.body().len() as u64 <= RANGE_BODY_LIMIT);
        }
    }

    #[test]
    fn invalid_and_multiple_ranges_return_416_without_audio_bytes() {
        for range in [
            "bytes=999999999-",
            "bytes=200-100",
            "bytes=-0",
            "items=0-1",
            "bytes=0-1,4-5",
        ] {
            let response = make_response(&Method::GET, "/long.wav", Some(range));
            assert_eq!(
                response.status(),
                StatusCode::RANGE_NOT_SATISFIABLE,
                "{range}"
            );
            assert!(response.body().is_empty());
            assert_eq!(
                response.headers()[header::CONTENT_RANGE],
                format!("bytes */{}", wav_length(LONG_TRACK_SECONDS))
            );
        }
    }

    #[test]
    fn un_ranged_get_materializes_the_entire_requested_representation() {
        let total = wav_length(LONG_TRACK_SECONDS);
        let plan = plan_get(total, None).expect("full response plan");
        assert_eq!(plan.status, StatusCode::OK);
        assert_eq!(plan.body_start, 0);
        assert_eq!(plan.body_end, Some(total - 1));
        assert_eq!(plan.body_end.unwrap() - plan.body_start + 1, total);
    }
}
