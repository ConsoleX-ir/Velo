//! URL inspection: HEAD first, bounded Range fallback, no body read.
//!
//! This module is intentionally independent of the download manager: it owns
//! its own reqwest `Client`, touches no job registry or concurrency slot, and
//! never creates a download record. All metadata is extracted from response
//! headers only; the response body is never consumed, so inspection cannot
//! trigger a file download.
//!
//! URLs and request headers are treated as untrusted and are never logged or
//! surfaced in error messages.

use std::time::Duration;

use reqwest::header::{
    self, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, RANGE, SERVER,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use url::Url;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 10;
const RANGE_PROBE: &str = "bytes=0-0";

/// Metadata learned about a URL without downloading its body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectMetadata {
    /// Final URL after redirects.
    pub(crate) effective_url: String,
    /// HTTP status of the response the metadata was drawn from.
    pub(crate) status: u16,
    /// Suggested file name, from `Content-Disposition` or the URL path.
    pub(crate) suggested_filename: Option<String>,
    /// `Content-Length`, when the server declares one.
    pub(crate) content_length: Option<u64>,
    /// `Content-Type`, when present.
    pub(crate) content_type: Option<String>,
    /// Whether the server advertises byte-range support (`Accept-Ranges: bytes`).
    pub(crate) accepts_ranges: Option<bool>,
    /// `Server` header, when present.
    pub(crate) server: Option<String>,
}

/// Inspection failure reported to the webview. No URL, query value, or
/// request header is embedded in the message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl InspectError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            code: "network_error",
            message: message.into(),
        }
    }

    fn timeout() -> Self {
        Self {
            code: "timeout",
            message: "The inspection request timed out.".into(),
        }
    }

    fn http_status(status: StatusCode) -> Self {
        Self {
            code: "http_status",
            message: format!("The server responded with HTTP {}.", status.as_u16()),
        }
    }
}

/// Inspect `url` without downloading its body.
///
/// Strategy: an HTTP HEAD request first. If HEAD is rejected or returns
/// insufficient size information, a bounded `Range: bytes=0-0` GET is used as
/// a fallback to the final (post-redirect) URL. The response body is never
/// read.
pub(crate) async fn inspect_url(url: &str) -> Result<InspectMetadata, InspectError> {
    let client = build_client();
    let parsed = validate_url(url)?;
    inspect_with_client(&client, parsed).await
}

/// `inspect_url` with an injected client, so tests can use short, deterministic
/// timeouts. Not part of the Tauri command surface.
pub(crate) async fn inspect_with_client(
    client: &reqwest::Client,
    url: Url,
) -> Result<InspectMetadata, InspectError> {
    // 1. HEAD request (follows redirects to the final URL).
    let head_response = match client.head(url.clone()).send().await {
        Ok(response) => response,
        Err(error) => return Err(network_error(&error)),
    };
    let status = head_response.status();
    let final_url = head_response.url().clone();

    if status.is_success() {
        let metadata = extract_metadata(&head_response);
        // A declared length is sufficient; otherwise probe with Range to try
        // to learn the size and range support from the final URL.
        if metadata.content_length.is_some() {
            return Ok(metadata);
        }
        return match range_fallback(client, final_url).await {
            Ok(range_metadata) => Ok(merge_metadata(metadata, range_metadata)),
            Err(_) => Ok(metadata),
        };
    }

    // HEAD rejected or unsupported (e.g. 405, 501): fall back to Range GET.
    range_fallback(client, final_url).await
}

/// Bounded Range GET to `url`. The body is never read; only headers are
/// inspected, then the response is dropped so the connection is closed.
async fn range_fallback(
    client: &reqwest::Client,
    url: Url,
) -> Result<InspectMetadata, InspectError> {
    let response = match client
        .get(url.clone())
        .header(RANGE, RANGE_PROBE)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return Err(network_error(&error)),
    };
    let status = response.status();
    if !status.is_success() {
        return Err(InspectError::http_status(status));
    }
    Ok(extract_metadata(&response))
}

/// Merge range-probe metadata over a HEAD result, filling gaps without
/// discarding values HEAD already provided.
fn merge_metadata(head: InspectMetadata, range: InspectMetadata) -> InspectMetadata {
    InspectMetadata {
        effective_url: range.effective_url,
        status: range.status,
        suggested_filename: range.suggested_filename.or(head.suggested_filename),
        content_length: range.content_length.or(head.content_length),
        content_type: range.content_type.or(head.content_type),
        accepts_ranges: range.accepts_ranges.or(head.accepts_ranges),
        server: range.server.or(head.server),
    }
}

fn extract_metadata(response: &reqwest::Response) -> InspectMetadata {
    let headers = response.headers();
    InspectMetadata {
        effective_url: response.url().as_str().to_owned(),
        status: response.status().as_u16(),
        suggested_filename: suggested_filename(headers, response.url()),
        content_length: parse_content_length(headers),
        content_type: content_type(headers),
        accepts_ranges: accepts_ranges(headers),
        server: server(headers),
    }
}

/// `Content-Length`, when it is a single well-formed decimal. Malformed or
/// duplicate lengths are treated as unknown rather than failing inspection.
fn parse_content_length(headers: &header::HeaderMap) -> Option<u64> {
    let values: Vec<_> = headers.get_all(CONTENT_LENGTH).iter().collect();
    let [value] = values.as_slice() else {
        return None;
    };
    let text = value.to_str().ok()?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok()
}

fn content_type(headers: &header::HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn accepts_ranges(headers: &header::HeaderMap) -> Option<bool> {
    let value = headers
        .get(ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())?;
    let text = value.trim();
    if text.eq_ignore_ascii_case("bytes") {
        return Some(true);
    }
    if text.eq_ignore_ascii_case("none") {
        return Some(false);
    }
    None
}

fn server(headers: &header::HeaderMap) -> Option<String> {
    headers
        .get(SERVER)
        .and_then(|value| value.to_str().ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Suggested file name: `Content-Disposition` first, then the URL path.
/// Honest about absence: returns `None` when no name can be determined.
fn suggested_filename(headers: &header::HeaderMap, url: &Url) -> Option<String> {
    if let Some(value) = headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
    {
        if let Some(name) = disposition_filename(value) {
            return Some(name);
        }
    }
    path_filename(url)
}

fn disposition_filename(value: &str) -> Option<String> {
    for parameter in value.split(';') {
        let parameter = parameter.trim();
        if let Some((key, raw_value)) = parameter.split_once('=') {
            let key = key.trim().to_lowercase();
            let raw_value = raw_value.trim();
            if key == "filename" {
                let name = raw_value
                    .strip_prefix('"')
                    .and_then(|quoted| quoted.strip_suffix('"'))
                    .unwrap_or(raw_value);
                if !name.is_empty() {
                    return Some(sanitize_name(name));
                }
            } else if key == "filename*" {
                // RFC 5987: charset'language'percent-encoded-value.
                let parts: Vec<&str> = raw_value.split('\'').collect();
                if parts.len() == 3 {
                    if let Some(decoded) = percent_decode(parts[2]) {
                        if !decoded.is_empty() {
                            return Some(sanitize_name(&decoded));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Percent-decode a RFC 5987 value (UTF-8).
fn percent_decode(input: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '%' {
            let hex: String = chars.by_ref().take(2).map(|(_, c)| c).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                    continue;
                }
            }
            bytes.extend_from_slice(&input.as_bytes()[index..]);
            break;
        } else {
            bytes.extend_from_slice(ch.to_string().as_bytes());
        }
    }
    String::from_utf8(bytes).ok()
}

fn path_filename(url: &Url) -> Option<String> {
    let segment = url.path().split('/').rfind(|s| !s.is_empty())?;
    let decoded = percent_decode_loose(segment);
    let sanitized = sanitize_name(&decoded);
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        return None;
    }
    Some(sanitized)
}

/// Lenient percent-decode for URL path segments; invalid sequences pass through.
fn percent_decode_loose(input: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some((_index, ch)) = chars.next() {
        if ch == '%' {
            let hex: String = chars.by_ref().take(2).map(|(_, c)| c).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                    continue;
                }
            }
            bytes.push(b'%');
        } else {
            bytes.extend_from_slice(ch.to_string().as_bytes());
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn sanitize_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = safe.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return String::new();
    }
    let limited = &trimmed[..trimmed.len().min(120)];
    limited.to_owned()
}

/// Builds the HTTP client used exclusively for inspection.
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(
            |attempt| match redirect_decision(attempt.previous(), attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(reason) => attempt.error(reason),
            },
        ))
        .build()
        .expect("inspection HTTP client builds")
}

/// Pure redirect policy mirrored on the download engine's: up to ten hops, no
/// HTTPS-to-HTTP downgrade, HTTP(S) only, no embedded credentials.
fn redirect_decision(previous: &[Url], next: &Url) -> Result<(), &'static str> {
    if previous.len() > MAX_REDIRECTS {
        return Err("redirect limit exceeded");
    }
    if !is_allowed_target(next) {
        return Err("redirect target must be HTTP(S) without credentials");
    }
    if previous
        .iter()
        .any(|url| url.scheme() == "https" && next.scheme() == "http")
    {
        return Err("HTTPS downgrade redirect rejected");
    }
    Ok(())
}

fn is_allowed_target(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn validate_url(raw: &str) -> Result<Url, InspectError> {
    let url = Url::parse(raw).map_err(|_| InspectError {
        code: "invalid_url",
        message: "Enter a complete HTTP or HTTPS URL.".into(),
    })?;
    if !is_allowed_target(&url) {
        return Err(InspectError {
            code: "invalid_url",
            message: "Only HTTP and HTTPS URLs without embedded credentials are supported.".into(),
        });
    }
    Ok(url)
}

fn network_error(error: &reqwest::Error) -> InspectError {
    if error.is_timeout() {
        return InspectError::timeout();
    }
    InspectError::network("Could not reach the server to inspect this link.")
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderMap;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    use super::*;

    async fn read_request(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut buffer = [0_u8; 512];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") || request.len() >= 16 * 1024
            {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = format!("HTTP/1.1 {status}\r\n{headers}\r\n").into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    /// Binds a loopback port, serves a single request with `reply`, then stops.
    /// Returns the port immediately (the handler runs in the background).
    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    /// Binds a loopback port, serves a single request with `reply`, then stops.
    /// Returns the port immediately (the handler runs in the background).
    async fn serve_one<F>(reply: F) -> u16
    where
        F: Fn(&str) -> Vec<u8> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let data = reply(&request);
            let _ = stream.write_all(&data).await;
            let _ = stream.shutdown().await;
        });
        port
    }

    /// Binds the given loopback port, serves `replies` in order across
    /// successive connections, then stops.
    async fn serve_sequence(port: u16, replies: Vec<Vec<u8>>) {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let remaining: Mutex<std::vec::IntoIter<Vec<u8>>> = Mutex::new(replies.into_iter());
        tokio::spawn(async move {
            while let Some(reply) = remaining.lock().await.next() {
                let (mut stream, _) = listener.accept().await.unwrap();
                let _request = read_request(&mut stream).await;
                let _ = stream.write_all(&reply).await;
                let _ = stream.shutdown().await;
            }
        });
    }

    /// Accepts one connection and immediately drops it without sending data,
    /// causing a connection-reset-style network error on the client side.
    async fn serve_connection_reset(port: u16) -> JoinHandle<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.shutdown().await;
            }
        })
    }

    /// Binds a loopback port, accepts one request, then stalls without sending
    /// response headers. Used to force a client-side timeout.
    async fn serve_stall() -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = stream.shutdown().await;
        });
        (port, server)
    }

    fn short_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(250))
            .timeout(Duration::from_millis(500))
            .redirect(reqwest::redirect::Policy::custom(
                |attempt| match redirect_decision(attempt.previous(), attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(reason) => attempt.error(reason),
                },
            ))
            .build()
            .unwrap()
    }

    fn base(port: u16) -> String {
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn successful_head_response_exposes_metadata() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Length: 1234\r\nContent-Type: application/zip\r\nAccept-Ranges: bytes\r\nServer: example/1.0\r\nConnection: close\r\n",
                b"",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/file.zip", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.status, 200);
        assert_eq!(metadata.content_length, Some(1234));
        assert_eq!(metadata.content_type.as_deref(), Some("application/zip"));
        assert_eq!(metadata.accepts_ranges, Some(true));
        assert_eq!(metadata.server.as_deref(), Some("example/1.0"));
        assert_eq!(metadata.effective_url, format!("{}/file.zip", base(port)));
    }

    #[tokio::test]
    async fn head_unsupported_falls_back_to_range() {
        let port = free_port().await;
        serve_sequence(port, vec![
            "HTTP/1.1 405 Method Not Allowed\r\nAllow: GET\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_bytes()
                .to_vec(),
            "HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nServer: range/2.0\r\nConnection: close\r\n\r\n"
                .as_bytes()
                .to_vec(),
        ])
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/file.bin", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.status, 200);
        assert_eq!(metadata.content_length, Some(5000));
        assert_eq!(metadata.accepts_ranges, Some(true));
        assert_eq!(metadata.server.as_deref(), Some("range/2.0"));
    }

    #[tokio::test]
    async fn redirect_resolves_to_final_url() {
        let port = free_port().await;
        let redirect = response(
            "302 Found",
            &format!(
                "Location: http://127.0.0.1:{port}/final\r\nContent-Length: 0\r\nConnection: close\r\n"
            ),
            b"",
        );
        let final_reply = response(
            "200 OK",
            "Content-Length: 9\r\nContent-Type: text/plain\r\nConnection: close\r\n",
            b"final-body",
        );
        serve_sequence(port, vec![redirect, final_reply]).await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/start", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.effective_url, format!("{}/final", base(port)));
        assert_eq!(metadata.content_length, Some(9));
        assert_eq!(metadata.status, 200);
    }

    #[tokio::test]
    async fn missing_content_length_reports_unknown() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Type: application/octet-stream\r\nConnection: close\r\n",
                b"",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/unknown.bin", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.content_length, None);
        assert_eq!(metadata.status, 200);
    }

    #[tokio::test]
    async fn missing_filename_returns_none() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Length: 4\r\nContent-Type: text/plain\r\nConnection: close\r\n",
                b"data",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.suggested_filename, None);
        assert_eq!(metadata.content_length, Some(4));
    }

    #[tokio::test]
    async fn content_disposition_supplies_filename() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Length: 4\r\nContent-Disposition: attachment; filename=\"archive.zip\"\r\nConnection: close\r\n",
                b"data",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/download", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.suggested_filename.as_deref(), Some("archive.zip"));
    }

    #[tokio::test]
    async fn server_ignores_range_reports_unknown_ranges() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Length: 100\r\nContent-Type: text/plain\r\nConnection: close\r\n",
                b"ignored-range",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/no-range.bin", base(port))).unwrap(),
        )
        .await
        .unwrap();
        // HEAD succeeds with a length, so no Range fallback is issued; Accept-Ranges is absent.
        assert_eq!(metadata.content_length, Some(100));
        assert_eq!(metadata.accepts_ranges, None);
    }

    #[tokio::test]
    async fn accepts_ranges_none_reported_as_false() {
        let port = serve_one(|_| {
            response(
                "200 OK",
                "Content-Length: 10\r\nAccept-Ranges: none\r\nConnection: close\r\n",
                b"",
            )
        })
        .await;
        let metadata = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/none.bin", base(port))).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(metadata.accepts_ranges, Some(false));
    }

    #[tokio::test]
    async fn unexpected_server_response_is_an_error() {
        // HEAD returns 500 (not success), so we fall back to Range GET which
        // also returns 500. Needs two connections: HEAD then Range probe.
        let port = free_port().await;
        serve_sequence(
            port,
            vec![
                response(
                    "500 Internal Server Error",
                    "Content-Length: 0\r\nConnection: close\r\n",
                    b"",
                ),
                response(
                    "500 Internal Server Error",
                    "Content-Length: 0\r\nConnection: close\r\n",
                    b"",
                ),
            ],
        )
        .await;
        let error = inspect_with_client(
            &short_client(),
            Url::parse(&format!("{}/broke", base(port))).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "http_status");
    }

    #[tokio::test]
    async fn connection_refused_is_a_network_error() {
        // Accept one connection then immediately drop it, so the client gets
        // a connection reset instead of a timeout.
        let port = free_port().await;
        serve_connection_reset(port).await;
        let error = inspect_with_client(
            &short_client(),
            Url::parse(&format!("http://127.0.0.1:{port}/gone")).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network_error");
    }

    #[tokio::test]
    async fn server_that_never_sends_headers_times_out() {
        let (port, server) = serve_stall().await;
        let error = inspect_with_client(
            &short_client(),
            Url::parse(&format!("http://127.0.0.1:{port}/stall")).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "timeout");
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn invalid_url_is_rejected() {
        for bad in [
            "ftp://example.com/file",
            "file:///etc/passwd",
            "http://user:pass@example.com/",
            "not a url",
        ] {
            let error = inspect_url(bad).await.unwrap_err();
            assert_eq!(error.code, "invalid_url", "{bad} must be rejected");
        }
    }

    #[tokio::test]
    async fn https_to_http_downgrade_redirect_is_rejected() {
        let source = Url::parse("https://example.com/start").unwrap();
        let target = Url::parse("http://example.com/insecure").unwrap();
        assert!(redirect_decision(std::slice::from_ref(&source), &target).is_err());
        let allowed = Url::parse("https://example.com/other").unwrap();
        assert!(redirect_decision(&[source], &allowed).is_ok());
    }

    #[test]
    fn content_length_parser_is_lenient() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, header::HeaderValue::from_static("0"));
        assert_eq!(parse_content_length(&headers), Some(0));
        headers.insert(CONTENT_LENGTH, header::HeaderValue::from_static("abc"));
        assert_eq!(parse_content_length(&headers), None);
        headers.append(CONTENT_LENGTH, header::HeaderValue::from_static("5"));
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn suggested_name_sanitizes_unsafe_characters() {
        assert_eq!(sanitize_name("weird<>:/\n|?*.zip"), "weird________.zip");
        assert_eq!(sanitize_name(".."), "");
        assert_eq!(sanitize_name("trailing... "), "trailing");
    }

    #[test]
    fn accepts_ranges_parses_bytes_and_none() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_RANGES, header::HeaderValue::from_static("bytes"));
        assert_eq!(accepts_ranges(&headers), Some(true));
        headers.insert(ACCEPT_RANGES, header::HeaderValue::from_static("none"));
        assert_eq!(accepts_ranges(&headers), Some(false));
        headers.remove(ACCEPT_RANGES);
        assert_eq!(accepts_ranges(&headers), None);
    }

    #[test]
    fn disposition_filename_parses_quoted_and_rfc5987() {
        assert_eq!(
            disposition_filename(r#"attachment; filename="archive.zip""#).as_deref(),
            Some("archive.zip")
        );
        assert_eq!(
            disposition_filename(r#"attachment; filename*=UTF-8''report%202024.pdf"#).as_deref(),
            Some("report 2024.pdf")
        );
        assert!(disposition_filename("inline").is_none());
    }
}
