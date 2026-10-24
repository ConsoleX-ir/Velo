use std::{
    fs,
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_LENGTH, TRANSFER_ENCODING};
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use url::Url;

#[cfg_attr(not(windows), allow(unused_imports))]
use crate::{
    cleanup_or_original, parse_content_length, parse_content_range, parse_strong_etag,
    promote_no_replace, redirect_decision, temp_candidate, validate_entity_headers,
    validate_transfer_encoding, write_chunk_accounted, CancelDisposition, ControlError,
    DownloadEngine, DownloadError, DownloadHandle, DownloadId, DownloadManager,
    DownloadManagerConfig, DownloadRequest, DownloadState, NetworkFailureKind, OpenedTemp,
    StartError, TempGuard,
};

static DIR_ID: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "consolex-download-test-{}-{}",
            std::process::id(),
            DIR_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create unique test directory");
        // Canonicalize so the returned path matches the UNC form that
        // canonical_destination produces on Windows. Without this, path
        // comparisons in open_or_create_temp (same_parent) and assertions
        // (starts_with) fail because dir.path() is C:\... while
        // canonical_destination returns \\?\C:\...
        #[cfg(windows)]
        let path = fs::canonicalize(&path).expect("canonicalize test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn serve_once(response: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let _request = read_request(&mut stream).await;
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
    });
    (format!("http://{address}/file"), task)
}

async fn serve_redirect() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("accept redirect request");
        let _request = read_request(&mut first).await;
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{address}/final\r\nContent-Length: 3\r\nConnection: close\r\n\r\nold"
        );
        let _ = first.write_all(redirect.as_bytes()).await;
        let _ = first.shutdown().await;

        let (mut final_request, _) = listener.accept().await.expect("accept final request");
        let _request = read_request(&mut final_request).await;
        let final_response = response(
            "200 OK",
            "Content-Length: 3\r\nConnection: close\r\n",
            b"new",
        );
        let _ = final_request.write_all(&final_response).await;
        let _ = final_request.shutdown().await;
    });
    (format!("http://{address}/start"), task)
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\n{headers}\r\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

#[tokio::test]
async fn downloads_exact_bytes_and_reports_completion() {
    let payload = vec![42_u8; 1024 * 1024];
    let (url, server) = serve_once(response(
        "200 OK",
        &format!("Content-Length: {}\r\nConnection: close\r\n", payload.len()),
        &payload,
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("payload.bin");

    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(result.state, DownloadState::Completed);
    assert_eq!(result.bytes_written, payload.len() as u64);
    assert_eq!(fs::read(&target).unwrap(), payload);
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "temporary file was promoted"
    );
}

#[tokio::test]
async fn downloads_empty_body() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 0\r\nConnection: close\r\n",
        &[],
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("empty.bin");
    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(result.bytes_written, 0);
    assert_eq!(fs::read(&target).unwrap(), b"");
}

#[tokio::test]
async fn accepts_close_delimited_body_without_content_length() {
    let body = b"body without a declared length";
    let mut raw = b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    raw.extend_from_slice(body);
    let (url, server) = serve_once(raw).await;
    let dir = TestDir::new();
    let target = dir.path().join("unknown-length.bin");

    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(result.bytes_written, body.len() as u64);
    assert_eq!(fs::read(&target).unwrap(), body);
}

#[tokio::test]
async fn follows_redirect_and_saves_only_final_response() {
    let (url, server) = serve_redirect().await;
    let dir = TestDir::new();
    let target = dir.path().join("redirected.bin");
    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(result.bytes_written, 3);
    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[tokio::test]
async fn existing_destination_is_preserved() {
    let dir = TestDir::new();
    let target = dir.path().join("existing.bin");
    fs::write(&target, b"old bytes").unwrap();

    let error = DownloadEngine::new()
        .unwrap()
        .download("http://127.0.0.1:1/file", &target)
        .await
        .unwrap_err();
    assert!(matches!(error, DownloadError::DestinationConflict));
    assert_eq!(fs::read(&target).unwrap(), b"old bytes");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn rejects_http_error_and_cleans_temporary_file() {
    let (url, server) = serve_once(response(
        "404 Not Found",
        "Content-Length: 4\r\n\r\n",
        b"nope",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("missing.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(error, DownloadError::HttpStatus(404)));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn rejects_no_content_response() {
    let (url, server) =
        serve_once(response("204 No Content", "Content-Length: 0\r\n\r\n", b"")).await;
    let dir = TestDir::new();
    let target = dir.path().join("no-content.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(error, DownloadError::HttpStatus(204)));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn rejects_truncated_response_and_cleans_temporary_file() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 12\r\nConnection: close\r\n",
        b"short",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("truncated.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(
        error,
        DownloadError::IncompleteResponse {
            expected: 12,
            received: 5
        }
    ));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn invalid_url_fails_before_creating_output() {
    let dir = TestDir::new();
    let target = dir.path().join("invalid.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download("file:///secret", &target)
        .await
        .unwrap_err();
    assert!(matches!(error, DownloadError::InvalidUrl));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn concurrent_downloads_cannot_claim_or_replace_same_destination() {
    let body = b"one valid result";
    let (url, server) = serve_once(response(
        "200 OK",
        &format!("Content-Length: {}\r\nConnection: close\r\n", body.len()),
        body,
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("shared.bin");
    let engine = DownloadEngine::new().unwrap();
    let first = {
        let engine = engine.clone();
        let url = url.clone();
        let target = target.clone();
        tokio::spawn(async move { engine.download(&url, target).await })
    };
    let second = {
        let engine = engine.clone();
        let url = url.clone();
        let target = target.clone();
        tokio::spawn(async move { engine.download(&url, target).await })
    };

    let first = first.await.unwrap();
    let second = second.await.unwrap();
    server.await.unwrap();

    let successful = (if first.is_ok() { 1 } else { 0 }) + (if second.is_ok() { 1 } else { 0 });
    assert_eq!(successful, 1);
    let rejected = if first.is_err() { first } else { second };
    assert!(matches!(
        rejected.unwrap_err(),
        DownloadError::DestinationConflict
    ));
    assert_eq!(fs::read(&target).unwrap(), body);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn rejects_non_identity_content_encoding() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 4\r\nContent-Encoding: gzip\r\nConnection: close\r\n",
        b"data",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("encoded.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(error, DownloadError::InvalidResponse));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------
// Phase 1A hardening tests
// ---------------------------------------------------------------------------

fn entry_count(dir: &TestDir) -> usize {
    fs::read_dir(dir.path()).unwrap().count()
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut buffer = [0_u8; 512];
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") || request.len() >= 16 * 1024 {
            break;
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

/// Serve exactly one connection with a scripted handler.
async fn serve_with<F, Fut>(handler: F) -> (String, JoinHandle<()>)
where
    F: FnOnce(TcpStream) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept request");
        handler(stream).await;
    });
    (format!("http://{address}/file"), task)
}

/// Serve every connection until aborted. The handler maps (request path,
/// listener address) to a full raw response. Also counts accepted connections.
async fn serve_loop<F>(handler: F) -> (SocketAddr, Arc<AtomicUsize>, JoinHandle<()>)
where
    F: Fn(String, SocketAddr) -> Vec<u8> + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let handler = handler.clone();
            tokio::spawn(async move {
                let request = read_request(&mut stream).await;
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let _ = stream.write_all(&handler(path, address)).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (address, hits, task)
}

async fn serve_range_resume(
    initial_etag: Option<&'static str>,
    resumed_status: &'static str,
    resumed_headers: &'static str,
    resumed_body: &'static [u8],
) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let (mut first, _) = listener.accept().await.expect("accept initial request");
        requests.push(read_request(&mut first).await);
        let etag = initial_etag
            .map(|etag| format!("ETag: {etag}\r\n"))
            .unwrap_or_default();
        let initial = response(
            "200 OK",
            &format!("Content-Length: 10\r\n{etag}Connection: close\r\n"),
            b"abcde",
        );
        let _ = first.write_all(&initial).await;
        let _ = first.shutdown().await;

        if initial_etag.is_none() {
            return requests;
        }
        let (mut resumed, _) = listener.accept().await.expect("accept range request");
        requests.push(read_request(&mut resumed).await);
        let reply = response(resumed_status, resumed_headers, resumed_body);
        let _ = resumed.write_all(&reply).await;
        let _ = resumed.shutdown().await;
        requests
    });
    (format!("http://{address}/file"), task)
}

async fn serve_partial_range_sequence() -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, headers, body) in [
            (
                "200 OK",
                "Content-Length: 10\r\nETag: \"v1\"\r\nConnection: close\r\n",
                b"abcde".as_slice(),
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 5-7/10\r\nContent-Length: 3\r\nETag: \"v1\"\r\nConnection: close\r\n",
                b"fgh".as_slice(),
            ),
            (
                "206 Partial Content",
                "Content-Range: bytes 8-9/10\r\nContent-Length: 2\r\nETag: \"v1\"\r\nConnection: close\r\n",
                b"ij".as_slice(),
            ),
        ] {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            requests.push(read_request(&mut stream).await);
            let reply = response(status, headers, body);
            let _ = stream.write_all(&reply).await;
            let _ = stream.shutdown().await;
        }
        requests
    });
    (format!("http://{address}/file"), task)
}

async fn serve_bounded_range_sequence() -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for offset in 0..=3_u64 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            requests.push(read_request(&mut stream).await);
            let reply = if offset == 0 {
                response(
                    "200 OK",
                    "Content-Length: 10\r\nETag: \"v1\"\r\nConnection: close\r\n",
                    b"abcde",
                )
            } else {
                let start = 4 + offset;
                response(
                    "206 Partial Content",
                    &format!(
                        "Content-Range: bytes {start}-{start}/10\r\nContent-Length: 1\r\nETag: \"v1\"\r\nConnection: close\r\n"
                    ),
                    b"x",
                )
            };
            let _ = stream.write_all(&reply).await;
            let _ = stream.shutdown().await;
        }
        requests
    });
    (format!("http://{address}/file"), task)
}

async fn serve_gated_range() -> (
    String,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("read listener address");
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("accept initial GET");
        read_request(&mut first).await;
        let full = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"v1\"\r\nConnection: close\r\n",
            b"abcde",
        );
        let _ = first.write_all(&full).await;
        let _ = first.shutdown().await;

        let (mut range, _) = listener.accept().await.expect("accept continuation");
        read_request(&mut range).await;
        let headers = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nf";
        let _ = range.write_all(headers).await;
        let _ = started_tx.send(());
        let _ = release_rx.await;
        let _ = range.shutdown().await;
    });
    (
        format!("http://{address}/file"),
        task,
        started_rx,
        release_tx,
    )
}

async fn serve_range_redirect() -> (String, JoinHandle<()>, JoinHandle<Option<String>>) {
    let source = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source server");
    let source_address = source.local_addr().expect("read source address");
    let redirected = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind redirect target");
    let redirected_address = redirected.local_addr().expect("read redirect address");
    let source_task = tokio::spawn(async move {
        let (mut first, _) = source.accept().await.expect("accept initial GET");
        read_request(&mut first).await;
        let initial = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"v1\"\r\nConnection: close\r\n",
            b"abcde",
        );
        let _ = first.write_all(&initial).await;
        let _ = first.shutdown().await;

        let (mut range, _) = source.accept().await.expect("accept continuation");
        read_request(&mut range).await;
        let redirect = response(
            "302 Found",
            &format!("Location: http://{redirected_address}/other\r\nContent-Length: 0\r\nConnection: close\r\n"),
            b"",
        );
        let _ = range.write_all(&redirect).await;
        let _ = range.shutdown().await;
    });
    let target_task = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_millis(250), redirected.accept()).await {
            Ok(Ok((mut stream, _))) => Some(read_request(&mut stream).await),
            _ => None,
        }
    });
    (
        format!("http://{source_address}/file"),
        source_task,
        target_task,
    )
}

fn test_engine(read_inactivity_ms: u64, deadline_ms: u64) -> DownloadEngine {
    DownloadEngine::with_timeouts(
        Duration::from_secs(5),
        Duration::from_millis(read_inactivity_ms),
        Duration::from_millis(deadline_ms),
    )
    .unwrap()
}

/// Run `download` but fail the test instead of hanging if it never returns.
async fn download_within(
    engine: &DownloadEngine,
    url: &str,
    target: &Path,
    limit: Duration,
) -> Result<crate::DownloadResult, DownloadError> {
    tokio::time::timeout(limit, engine.download(url, target))
        .await
        .expect("download did not finish within the test limit (hung)")
}

// ----- HTTP status / headers -------------------------------------------------

#[tokio::test]
async fn resumes_truncated_body_from_exact_offset_with_strong_etag() {
    let (url, server) = serve_range_resume(
        Some("\"version-1\""),
        "206 Partial Content",
        "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"version-1\"\r\nConnection: close\r\n",
        b"fghij",
    )
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("resumed.bin");

    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    let requests = server.await.unwrap();

    assert_eq!(result.bytes_written, 10);
    assert_eq!(fs::read(&target).unwrap(), b"abcdefghij");
    assert!(requests[1]
        .lines()
        .any(|line| line.eq_ignore_ascii_case("range: bytes=5-")));
    assert!(requests[1]
        .lines()
        .any(|line| line.eq_ignore_ascii_case("if-range: \"version-1\"")));
    assert_eq!(entry_count(&dir), 1);
}

#[tokio::test]
async fn does_not_resume_interrupted_body_without_strong_etag() {
    let (url, server) = serve_range_resume(None, "", "", b"").await;
    let dir = TestDir::new();
    let target = dir.path().join("unvalidated.bin");

    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    let requests = server.await.unwrap();

    assert!(matches!(error, DownloadError::IncompleteResponse { .. }));
    assert_eq!(requests.len(), 1, "no Range request without a strong ETag");
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn continues_from_partial_content_range_without_resetting_bytes() {
    let (url, server) = serve_partial_range_sequence().await;
    let dir = TestDir::new();
    let target = dir.path().join("partial-ranges.bin");

    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    let requests = server.await.unwrap();

    assert_eq!(result.bytes_written, 10);
    assert_eq!(fs::read(&target).unwrap(), b"abcdefghij");
    assert!(requests[1]
        .lines()
        .any(|line| line.eq_ignore_ascii_case("range: bytes=5-")));
    assert!(requests[2]
        .lines()
        .any(|line| line.eq_ignore_ascii_case("range: bytes=8-")));
    assert_eq!(entry_count(&dir), 1);
}

#[tokio::test]
async fn range_continuation_request_count_is_bounded() {
    let (url, server) = serve_bounded_range_sequence().await;
    let dir = TestDir::new();
    let target = dir.path().join("resume-limit.bin");

    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    let requests = server.await.unwrap();

    assert!(matches!(error, DownloadError::IncompleteResponse { .. }));
    assert_eq!(requests.len(), 4, "one full GET plus three continuations");
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn rejects_range_resume_when_entity_tag_changes() {
    let (url, server) = serve_range_resume(
        Some("\"version-1\""),
        "206 Partial Content",
        "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"version-2\"\r\nConnection: close\r\n",
        b"fghij",
    )
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("changed.bin");

    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();

    assert!(matches!(error, DownloadError::InvalidResponse));
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn range_redirect_is_rejected_without_contacting_the_redirect_target() {
    let (url, source, target) = serve_range_redirect().await;
    let dir = TestDir::new();
    let destination = dir.path().join("redirect-range.bin");

    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &destination)
        .await
        .unwrap_err();
    source.await.unwrap();
    let target_request = target.await.unwrap();

    assert!(matches!(error, DownloadError::RedirectRejected));
    assert!(
        target_request.is_none(),
        "range headers must not cross redirects"
    );
    assert!(!destination.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn rejects_misaligned_or_ignored_range_responses_without_appending() {
    let cases = [
        (
            "206 Partial Content",
            "Content-Range: bytes 4-9/10\r\nContent-Length: 6\r\nETag: \"version-1\"\r\nConnection: close\r\n",
            b"efghij".as_slice(),
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 5-9/11\r\nContent-Length: 5\r\nETag: \"version-1\"\r\nConnection: close\r\n",
            b"fghij".as_slice(),
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nContent-Length: 4\r\nETag: \"version-1\"\r\nConnection: close\r\n",
            b"fghi".as_slice(),
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nTransfer-Encoding: chunked\r\nETag: \"version-1\"\r\nConnection: close\r\n",
            b"6\r\nfghijk\r\n0\r\n\r\n".as_slice(),
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nConnection: close\r\n",
            b"fghij".as_slice(),
        ),
        (
            "200 OK",
            "Content-Length: 10\r\nETag: \"version-1\"\r\nConnection: close\r\n",
            b"abcdefghij".as_slice(),
        ),
        (
            "416 Range Not Satisfiable",
            "Content-Range: bytes */10\r\nContent-Length: 0\r\nConnection: close\r\n",
            b"".as_slice(),
        ),
        (
            "503 Service Unavailable",
            "Retry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n",
            b"".as_slice(),
        ),
    ];
    for (status, headers, body) in cases {
        let (url, server) = serve_range_resume(Some("\"version-1\""), status, headers, body).await;
        let dir = TestDir::new();
        let target = dir.path().join("bad-range.bin");
        let error = DownloadEngine::new()
            .unwrap()
            .download(&url, &target)
            .await
            .unwrap_err();
        server.await.unwrap();

        match status {
            "416 Range Not Satisfiable" => {
                assert!(matches!(error, DownloadError::HttpStatus(416)));
            }
            "503 Service Unavailable" => {
                assert!(matches!(error, DownloadError::HttpStatus(503)));
            }
            _ => assert!(matches!(error, DownloadError::InvalidResponse)),
        }
        assert!(!target.exists());
        assert_eq!(entry_count(&dir), 0);
    }
}

#[test]
fn content_range_and_strong_etag_parsers_reject_ambiguous_values() {
    let mut headers = HeaderMap::new();
    headers.insert("content-range", HeaderValue::from_static("bytes 4-9/10"));
    assert_eq!(
        parse_content_range(&headers).unwrap(),
        crate::ContentRange {
            start: 4,
            end: 9,
            total: 10
        }
    );
    headers.insert("content-range", HeaderValue::from_static("bytes 9-4/10"));
    assert!(matches!(
        parse_content_range(&headers),
        Err(DownloadError::InvalidResponse)
    ));
    headers.insert("content-range", HeaderValue::from_static("BYTES 4-9/10"));
    assert_eq!(
        parse_content_range(&headers).unwrap(),
        crate::ContentRange {
            start: 4,
            end: 9,
            total: 10
        }
    );
    headers.remove("content-range");
    headers.insert("etag", HeaderValue::from_static("W/\"weak\""));
    assert_eq!(parse_strong_etag(&headers).unwrap(), None);
    headers.insert("etag", HeaderValue::from_static("\"strong\""));
    assert_eq!(
        parse_strong_etag(&headers).unwrap(),
        Some(HeaderValue::from_static("\"strong\""))
    );
    headers.append("etag", HeaderValue::from_static("\"second\""));
    assert_eq!(parse_strong_etag(&headers).unwrap(), None);
    headers.remove("etag");
    headers.insert("etag", HeaderValue::from_static("invalid"));
    assert_eq!(parse_strong_etag(&headers).unwrap(), None);
}

#[test]
fn entity_headers_reject_content_length_and_transfer_encoding_together() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("4"));
    headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
    assert!(matches!(
        validate_entity_headers(&headers),
        Err(DownloadError::InvalidResponse)
    ));
}

#[tokio::test]
async fn rejects_ambiguous_content_length_and_transfer_encoding_response() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 4\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n",
        b"4\r\ndata\r\n0\r\n\r\n",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("ambiguous-framing.bin");
    let result = DownloadEngine::new().unwrap().download(&url, &target).await;
    server.await.unwrap();

    assert!(matches!(result, Err(DownloadError::InvalidResponse)));
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn only_status_200_is_accepted() {
    let cases = [
        ("202 Accepted", "", 202_u16),
        (
            "206 Partial Content",
            "Content-Range: bytes 0-3/10\r\n",
            206,
        ),
        ("301 Moved Permanently", "", 301), // redirect without Location is final
        ("500 Internal Server Error", "", 500),
        ("503 Service Unavailable", "Retry-After: 1\r\n", 503),
    ];
    for (status, extra, code) in cases {
        let (url, server) = serve_once(response(
            status,
            &format!("Content-Length: 4\r\nConnection: close\r\n{extra}"),
            b"nope",
        ))
        .await;
        let dir = TestDir::new();
        let target = dir.path().join("status.bin");
        let error = DownloadEngine::new()
            .unwrap()
            .download(&url, &target)
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(
            matches!(error, DownloadError::HttpStatus(actual) if actual == code),
            "status {code}: got {error:?}"
        );
        assert!(!target.exists(), "status {code} must not create a file");
        assert_eq!(entry_count(&dir), 0, "status {code} left a temp file");
    }
}

#[tokio::test]
async fn rejects_200_with_content_range() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 4\r\nContent-Range: bytes 0-3/10\r\nConnection: close\r\n",
        b"data",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("ranged.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(error, DownloadError::InvalidResponse));
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn accepts_explicit_identity_content_encoding() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 4\r\nContent-Encoding: Identity\r\nConnection: close\r\n",
        b"data",
    ))
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("identity.bin");
    DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"data");
}

#[tokio::test]
async fn rejects_unsupported_encoding_in_any_header_value() {
    let cases = [
        "Content-Encoding: br\r\n",
        "Content-Encoding: identity, gzip\r\n",
        "Content-Encoding: identity\r\nContent-Encoding: gzip\r\n",
        "Content-Encoding: gzip\r\nContent-Encoding: identity\r\n",
    ];
    for headers in cases {
        let (url, server) = serve_once(response(
            "200 OK",
            &format!("Content-Length: 4\r\nConnection: close\r\n{headers}"),
            b"data",
        ))
        .await;
        let dir = TestDir::new();
        let target = dir.path().join("encoded.bin");
        let result = DownloadEngine::new().unwrap().download(&url, &target).await;
        server.await.unwrap();
        assert!(
            matches!(result, Err(DownloadError::InvalidResponse)),
            "{headers:?} must be rejected, got {result:?}"
        );
        assert!(!target.exists(), "{headers:?} must not produce a file");
        assert_eq!(entry_count(&dir), 0, "{headers:?} left a temp file");
    }
}

fn length_headers(values: &[&[u8]]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for value in values {
        map.append(CONTENT_LENGTH, HeaderValue::from_bytes(value).unwrap());
    }
    map
}

#[test]
fn content_length_parser_accepts_only_one_plain_decimal() {
    assert_eq!(parse_content_length(&length_headers(&[])).unwrap(), None);
    assert_eq!(
        parse_content_length(&length_headers(&[b"0"])).unwrap(),
        Some(0)
    );
    assert_eq!(
        parse_content_length(&length_headers(&[b"18446744073709551615"])).unwrap(),
        Some(u64::MAX)
    );
    let malformed: [&[u8]; 10] = [
        b"",
        b"abc",
        b"-1",
        b"+5",
        b"5 5",
        b"5,5",
        b"0x10",
        b"1.5",
        b"18446744073709551616",
        b"\xff",
    ];
    for value in malformed {
        assert!(
            matches!(
                parse_content_length(&length_headers(&[value])),
                Err(DownloadError::InvalidResponse)
            ),
            "{value:?} must be rejected"
        );
    }
    for pair in [[&b"5"[..], &b"5"[..]], [&b"5"[..], &b"6"[..]]] {
        assert!(matches!(
            parse_content_length(&length_headers(&pair)),
            Err(DownloadError::InvalidResponse)
        ));
    }
}

#[tokio::test]
async fn malformed_content_length_never_produces_a_file() {
    let cases = [
        "Content-Length: abc\r\n",
        "Content-Length: -1\r\n",
        "Content-Length: 5\r\nContent-Length: 6\r\n",
        "Content-Length: 99999999999999999999999\r\n",
    ];
    for headers in cases {
        let (url, server) = serve_once(response(
            "200 OK",
            &format!("{headers}Connection: close\r\n"),
            b"hello",
        ))
        .await;
        let dir = TestDir::new();
        let target = dir.path().join("malformed.bin");
        let result = DownloadEngine::new().unwrap().download(&url, &target).await;
        server.await.unwrap();
        assert!(result.is_err(), "{headers:?} must fail, got {result:?}");
        assert!(!target.exists(), "{headers:?} must not produce a file");
        assert_eq!(entry_count(&dir), 0, "{headers:?} left a temp file");
    }
}

#[tokio::test]
async fn downloads_chunked_body_exact_bytes() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
        .to_vec();
    let (url, server) = serve_once(raw).await;
    let dir = TestDir::new();
    let target = dir.path().join("chunked.bin");
    let result = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(result.bytes_written, 11);
    assert_eq!(fs::read(&target).unwrap(), b"hello world");
}

#[tokio::test]
async fn requests_identity_content_encoding() {
    let (request_tx, request_rx) = oneshot::channel();
    let (url, server) = serve_with(|mut stream| async move {
        let request = read_request(&mut stream).await;
        let _ = request_tx.send(request);
        let body = response(
            "200 OK",
            "Content-Length: 2\r\nConnection: close\r\n",
            b"ok",
        );
        let _ = stream.write_all(&body).await;
        let _ = stream.shutdown().await;
    })
    .await;
    let dir = TestDir::new();
    DownloadEngine::new()
        .unwrap()
        .download(&url, dir.path().join("identity-request.bin"))
        .await
        .unwrap();
    server.await.unwrap();
    let request = request_rx.await.unwrap().to_ascii_lowercase();
    assert!(
        request.contains("accept-encoding: identity\r\n"),
        "{request:?}"
    );
}

#[tokio::test]
async fn rejects_truncated_chunked_body_and_cleans_up() {
    // First chunk is complete but the terminating zero-chunk never arrives.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                5\r\nhello\r\n"
        .to_vec();
    let (url, server) = serve_once(raw).await;
    let dir = TestDir::new();
    let target = dir.path().join("truncated-chunked.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&url, &target)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(
        matches!(
            error,
            DownloadError::NetworkFailure(NetworkFailureKind::RequestOrBody)
        ),
        "got {error:?}"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

// ----- redirects --------------------------------------------------------------

fn urls(list: &[&str]) -> Vec<Url> {
    list.iter().map(|u| Url::parse(u).unwrap()).collect()
}

#[test]
fn redirect_policy_blocks_https_downgrade_anywhere_in_the_chain() {
    let next = Url::parse("http://c.example/").unwrap();
    for previous in [
        urls(&["https://a.example/"]),
        urls(&["https://a.example/", "https://b.example/"]),
        urls(&["http://a.example/", "https://b.example/"]),
        urls(&["https://a.example/", "http://b.example/"]),
    ] {
        assert!(
            redirect_decision(&previous, &next).is_err(),
            "{previous:?} -> http must be rejected"
        );
    }
    let allowed = [
        (urls(&["http://a.example/"]), "http://b.example/"),
        (urls(&["http://a.example/"]), "https://b.example/"),
        (urls(&["https://a.example/"]), "https://b.example/"),
    ];
    for (previous, next) in allowed {
        let next = Url::parse(next).unwrap();
        assert!(
            redirect_decision(&previous, &next).is_ok(),
            "{previous:?} -> {next} must be allowed"
        );
    }
}

#[test]
fn redirect_policy_allows_ten_hops_and_rejects_the_eleventh() {
    let next = Url::parse("http://next.example/").unwrap();
    // `previous` always contains the original URL plus every hop already followed.
    let after_hops = |hops: usize| vec![Url::parse("http://start.example/").unwrap(); hops + 1];
    assert!(
        redirect_decision(&after_hops(9), &next).is_ok(),
        "10th hop must be allowed"
    );
    assert!(
        redirect_decision(&after_hops(10), &next).is_err(),
        "11th hop must be rejected"
    );
}

#[test]
fn redirect_policy_rejects_unsupported_schemes_and_userinfo() {
    let previous = urls(&["http://a.example/"]);
    for target in [
        "file:///etc/passwd",
        "ftp://b.example/file",
        "http://user:pass@b.example/",
        "https://user@b.example/",
    ] {
        let next = Url::parse(target).unwrap();
        assert!(
            redirect_decision(&previous, &next).is_err(),
            "{target} must be rejected"
        );
    }
}

fn chain_handler(hops: usize) -> impl Fn(String, SocketAddr) -> Vec<u8> + Send + Sync + 'static {
    move |path, address| {
        let n: usize = path.trim_start_matches('/').parse().unwrap_or(0);
        if n < hops {
            response(
                "302 Found",
                &format!(
                    "Location: http://{address}/{}\r\nContent-Length: 0\r\nConnection: close\r\n",
                    n + 1
                ),
                b"",
            )
        } else {
            response(
                "200 OK",
                "Content-Length: 4\r\nConnection: close\r\n",
                b"done",
            )
        }
    }
}

#[tokio::test]
async fn follows_ten_redirects_end_to_end() {
    let (address, _hits, server) = serve_loop(chain_handler(10)).await;
    let dir = TestDir::new();
    let target = dir.path().join("ten.bin");
    let result = DownloadEngine::new()
        .unwrap()
        .download(&format!("http://{address}/0"), &target)
        .await;
    server.abort();
    result.expect("a chain of exactly ten redirects is within the cap");
    assert_eq!(fs::read(&target).unwrap(), b"done");
}

#[tokio::test]
async fn rejects_eleven_redirects_and_loops_end_to_end() {
    for hops in [11, usize::MAX] {
        let (address, _hits, server) = serve_loop(chain_handler(hops)).await;
        let dir = TestDir::new();
        let target = dir.path().join("too-many.bin");
        let error = DownloadEngine::new()
            .unwrap()
            .download(&format!("http://{address}/0"), &target)
            .await
            .unwrap_err();
        server.abort();
        assert!(
            matches!(error, DownloadError::RedirectRejected),
            "hops={hops}: got {error:?}"
        );
        assert!(!target.exists());
        assert_eq!(entry_count(&dir), 0);
    }
}

#[tokio::test]
async fn redirect_to_unsupported_scheme_fails_without_saving_anything() {
    // The redirect policy rejects schemes outside HTTP(S) before another
    // request can be sent.
    for location in ["file:///etc/passwd", "ftp://127.0.0.1/file"] {
        let (url, server) = serve_once(response(
            "302 Found",
            &format!("Location: {location}\r\nContent-Length: 0\r\nConnection: close\r\n"),
            b"",
        ))
        .await;
        let dir = TestDir::new();
        let target = dir.path().join("scheme.bin");
        let result = DownloadEngine::new().unwrap().download(&url, &target).await;
        server.await.unwrap();
        assert!(
            matches!(result, Err(DownloadError::RedirectRejected)),
            "{location}: got {result:?}"
        );
        assert!(!target.exists());
        assert_eq!(entry_count(&dir), 0);
    }
}

#[tokio::test]
async fn redirect_target_with_userinfo_is_never_contacted() {
    let (address, hits, server) = serve_loop(|path, address| {
        if path == "/start" {
            response(
                "302 Found",
                &format!(
                    "Location: http://user:secret@{address}/final\r\nContent-Length: 0\r\nConnection: close\r\n"
                ),
                b"",
            )
        } else {
            response("200 OK", "Content-Length: 4\r\nConnection: close\r\n", b"leak")
        }
    })
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("userinfo.bin");
    let result = DownloadEngine::new()
        .unwrap()
        .download(&format!("http://{address}/start"), &target)
        .await;
    server.abort();
    assert!(
        matches!(result, Err(DownloadError::RedirectRejected)),
        "got {result:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "credentialed target was contacted"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

// ----- timeouts and network failures -----------------------------------------

#[tokio::test]
async fn connection_refused_is_a_network_failure_and_leaves_nothing_behind() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener); // nothing listens on this port any more
    let dir = TestDir::new();
    let target = dir.path().join("refused.bin");
    let error = DownloadEngine::new()
        .unwrap()
        .download(&format!("http://{address}/file"), &target)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            DownloadError::NetworkFailure(NetworkFailureKind::RequestOrBody)
        ),
        "got {error:?}"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn server_that_never_sends_headers_times_out() {
    let (url, server) = serve_with(|mut stream| async move {
        read_request(&mut stream).await;
        std::future::pending::<()>().await;
    })
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("no-headers.bin");
    let engine = test_engine(200, 3_600_000);
    let result = download_within(&engine, &url, &target, Duration::from_secs(5)).await;
    server.abort();
    assert!(
        matches!(
            result,
            Err(DownloadError::NetworkFailure(NetworkFailureKind::Timeout))
        ),
        "got {result:?}"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

async fn serve_partial_then_stall() -> (String, JoinHandle<()>, oneshot::Receiver<()>) {
    let (response_sent_tx, response_sent_rx) = oneshot::channel();
    let (url, server) = serve_with(move |mut stream| async move {
        read_request(&mut stream).await;
        let head = response(
            "200 OK",
            "Content-Length: 100\r\nConnection: close\r\n",
            b"short",
        );
        stream.write_all(&head).await.unwrap();
        let _ = response_sent_tx.send(());
        std::future::pending::<()>().await;
    })
    .await;
    (url, server, response_sent_rx)
}

#[tokio::test]
async fn stalled_body_hits_the_read_inactivity_timeout() {
    let (url, server, response_sent) = serve_partial_then_stall().await;
    let dir = TestDir::new();
    let target = dir.path().join("stalled.bin");
    let engine = test_engine(200, 3_600_000);
    let result = download_within(&engine, &url, &target, Duration::from_secs(5)).await;
    response_sent.await.unwrap();
    server.abort();
    assert!(
        matches!(result, Err(DownloadError::BodyInactivityTimeout)),
        "got {result:?}"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn job_deadline_during_body_is_reported_as_a_timeout() {
    let (url, server, response_sent) = serve_partial_then_stall().await;
    let dir = TestDir::new();
    let target = dir.path().join("deadline.bin");
    let engine = test_engine(3_600_000, 300);
    let result = download_within(&engine, &url, &target, Duration::from_secs(5)).await;
    response_sent.await.unwrap();
    server.abort();
    assert!(
        matches!(
            result,
            Err(DownloadError::NetworkFailure(NetworkFailureKind::Timeout))
        ),
        "a deadline expiry must not be reported as a short body: {result:?}"
    );
    assert_eq!(entry_count(&dir), 0);
}

#[tokio::test]
async fn slow_but_steady_body_is_not_cut_off_by_the_inactivity_timeout() {
    let (url, server) = serve_with(|mut stream| async move {
        read_request(&mut stream).await;
        let head = response("200 OK", "Content-Length: 9\r\nConnection: close\r\n", b"");
        let _ = stream.write_all(&head).await;
        for part in [b"abc", b"def", b"ghi"] {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = stream.write_all(part).await;
            let _ = stream.flush().await;
        }
        let _ = stream.shutdown().await;
    })
    .await;
    let dir = TestDir::new();
    let target = dir.path().join("slow.bin");
    // Total transfer (~450 ms) exceeds the inactivity limit; each gap does not.
    let engine = test_engine(1_000, 3_600_000);
    download_within(&engine, &url, &target, Duration::from_secs(10))
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"abcdefghi");
}

// ----- cleanup, drop, and destination races ----------------------------------

#[test]
fn cleanup_failure_reports_both_the_cleanup_error_and_the_original_error() {
    let dir = TestDir::new();
    let not_a_file = dir.path().join("a-directory");
    fs::create_dir(&not_a_file).unwrap();
    // remove_file on a directory fails with an error other than NotFound.
    let error = cleanup_or_original(&not_a_file, DownloadError::DestinationConflict);
    match error {
        DownloadError::CleanupFailure { path, original, .. } => {
            assert_eq!(path, not_a_file);
            assert!(matches!(*original, DownloadError::DestinationConflict));
        }
        other => panic!("expected CleanupFailure, got {other:?}"),
    }
}

#[test]
fn cleanup_of_an_already_missing_file_returns_the_original_error() {
    let dir = TestDir::new();
    let error = cleanup_or_original(&dir.path().join("gone"), DownloadError::InvalidResponse);
    assert!(matches!(error, DownloadError::InvalidResponse));
}

#[test]
fn opened_temp_closes_and_removes_file_when_abandoned() {
    let dir = TestDir::new();
    let path = dir.path().join("abandoned.part");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let opened = OpenedTemp {
        file: Some(file),
        guard: Some(TempGuard::new(path.clone())),
    };
    drop(opened);
    assert!(!path.exists());
}

#[test]
fn temporary_candidate_never_aliases_the_requested_destination() {
    let dir = TestDir::new();
    let destination = dir.path().join(".consolex-123-4.part");
    assert_eq!(
        temp_candidate(dir.path(), &destination, 123, 4),
        None,
        "a caller-selected path must not be used as its own temporary file"
    );
}

#[cfg(windows)]
#[test]
fn windows_promotion_moves_without_replacing_an_existing_destination() {
    let dir = TestDir::new();
    let destination = dir.path().join("final.bin");
    let first_temp = dir.path().join("first.part");
    fs::write(&first_temp, b"first payload").unwrap();

    promote_no_replace(&first_temp, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"first payload");
    assert!(!first_temp.exists());

    let second_temp = dir.path().join("second.part");
    fs::write(&second_temp, b"replacement payload").unwrap();
    fs::write(&destination, b"preserved payload").unwrap();
    let error = promote_no_replace(&second_temp, &destination).unwrap_err();
    assert!(matches!(error, DownloadError::DestinationConflict));
    assert_eq!(fs::read(&destination).unwrap(), b"preserved payload");
    assert_eq!(fs::read(&second_temp).unwrap(), b"replacement payload");
}

#[tokio::test]
async fn dropping_the_download_future_removes_the_temporary_file() {
    let (url, server, response_sent) = serve_partial_then_stall().await;
    let dir = TestDir::new();
    let target = dir.path().join("dropped.bin");
    let engine = DownloadEngine::new().unwrap();
    let task = {
        let engine = engine.clone();
        let url = url.clone();
        let target = target.clone();
        tokio::spawn(async move { engine.download(&url, target).await })
    };

    // The server received the request and sent an incomplete response; the
    // temporary file must exist before aborting the active transfer.
    response_sent.await.unwrap();
    assert_eq!(entry_count(&dir), 1, "temporary file was not created");

    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    server.abort();

    assert_eq!(entry_count(&dir), 0, "temporary file leaked after drop");
    assert!(!target.exists());

    // The in-process reservation must be released as well.
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 2\r\nConnection: close\r\n",
        b"ok",
    ))
    .await;
    engine.download(&url, &target).await.unwrap();
    server.await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"ok");
}

/// Serve 5 of 10 bytes, wait for `release`, then send the rest.
async fn serve_gated() -> (
    String,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
) {
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (url, server) = serve_with(|mut stream| async move {
        read_request(&mut stream).await;
        let head = response(
            "200 OK",
            "Content-Length: 10\r\nConnection: close\r\n",
            b"01234",
        );
        stream.write_all(&head).await.unwrap();
        let _ = started_tx.send(());
        let _ = release_rx.await;
        stream.write_all(b"56789").await.unwrap();
        let _ = stream.shutdown().await;
    })
    .await;
    (url, server, started_rx, release_tx)
}

#[tokio::test]
async fn destination_created_during_transfer_is_preserved_and_temp_removed() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let target = dir.path().join("raced.bin");
    let task = {
        let url = url.clone();
        let target = target.clone();
        tokio::spawn(async move { DownloadEngine::new().unwrap().download(&url, target).await })
    };
    started.await.unwrap();
    fs::write(&target, b"external").unwrap(); // appears after validation
    release.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    server.await.unwrap();
    assert!(
        matches!(error, DownloadError::DestinationConflict),
        "got {error:?}"
    );
    assert_eq!(fs::read(&target).unwrap(), b"external");
    assert_eq!(entry_count(&dir), 1, "temporary file was not cleaned up");
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_symlink_destination_is_never_followed() {
    let dir = TestDir::new();
    let victim = dir.path().join("victim.bin");
    let target = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&victim, &target).unwrap();
    let error = DownloadEngine::new()
        .unwrap()
        .download("http://127.0.0.1:1/file", &target)
        .await
        .unwrap_err();
    assert!(matches!(error, DownloadError::DestinationConflict));
    assert!(!victim.exists(), "symlink target must not be created");
    assert!(fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(entry_count(&dir), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_created_during_transfer_is_not_followed() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let victim = dir.path().join("victim.bin");
    let target = dir.path().join("raced-link.bin");
    let task = {
        let url = url.clone();
        let target = target.clone();
        tokio::spawn(async move { DownloadEngine::new().unwrap().download(&url, target).await })
    };
    started.await.unwrap();
    std::os::unix::fs::symlink(&victim, &target).unwrap();
    release.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    server.await.unwrap();
    assert!(
        matches!(error, DownloadError::DestinationConflict),
        "got {error:?}"
    );
    assert!(!victim.exists(), "symlink target must not be created");
    assert_eq!(entry_count(&dir), 1, "only the racing symlink may remain");
}

#[tokio::test]
async fn concurrent_downloads_to_distinct_destinations_do_not_share_temp_files() {
    let (address, _hits, server) = serve_loop(|path, _| {
        let n: u8 = path.trim_start_matches('/').parse().unwrap_or(0);
        let body = vec![n; 64 * 1024];
        response(
            "200 OK",
            &format!("Content-Length: {}\r\nConnection: close\r\n", body.len()),
            &body,
        )
    })
    .await;
    let dir = TestDir::new();
    let engine = DownloadEngine::new().unwrap();
    let mut tasks = Vec::new();
    for n in 0_u8..8 {
        let engine = engine.clone();
        let url = format!("http://{address}/{n}");
        let target = dir.path().join(format!("file-{n}.bin"));
        tasks.push(tokio::spawn(
            async move { engine.download(&url, target).await },
        ));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    server.abort();
    for n in 0_u8..8 {
        let bytes = fs::read(dir.path().join(format!("file-{n}.bin"))).unwrap();
        assert_eq!(bytes, vec![n; 64 * 1024], "file {n} has wrong content");
    }
    assert_eq!(entry_count(&dir), 8, "unexpected leftover temp files");
}

#[tokio::test]
async fn different_spellings_of_one_destination_still_conflict() {
    let (address, _hits, server) = serve_loop(|_, _| {
        response(
            "200 OK",
            "Content-Length: 4\r\nConnection: close\r\n",
            b"data",
        )
    })
    .await;
    let dir = TestDir::new();
    let url = format!("http://{address}/file");
    let engine = DownloadEngine::new().unwrap();
    let plain = dir.path().join("same.bin");
    let dotted = dir.path().join(".").join("same.bin");
    let first = {
        let (engine, url) = (engine.clone(), url.clone());
        tokio::spawn(async move { engine.download(&url, plain).await })
    };
    let second = {
        let (engine, url) = (engine.clone(), url.clone());
        tokio::spawn(async move { engine.download(&url, dotted).await })
    };
    let results = [first.await.unwrap(), second.await.unwrap()];
    server.abort();
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|r| matches!(r, Err(DownloadError::DestinationConflict)))
        .count();
    assert_eq!((ok, conflicts), (1, 1), "results: {results:?}");
    assert_eq!(fs::read(dir.path().join("same.bin")).unwrap(), b"data");
    assert_eq!(entry_count(&dir), 1);
}

#[tokio::test]
async fn rejects_transfer_coding_other_than_chunked() {
    // hyper de-frames `chunked` only; a leading `gzip` coding would be saved still encoded.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\nConnection: close\r\n\r\n\
                5\r\nhello\r\n0\r\n\r\n"
        .to_vec();
    let (url, server) = serve_once(raw).await;
    let dir = TestDir::new();
    let target = dir.path().join("te-gzip.bin");
    let result = DownloadEngine::new().unwrap().download(&url, &target).await;
    server.await.unwrap();
    assert!(
        matches!(result, Err(DownloadError::InvalidResponse)),
        "got {result:?}"
    );
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
}

#[test]
fn transfer_encoding_accepts_only_one_chunked_coding_without_content_length() {
    let mut headers = HeaderMap::new();
    assert!(validate_transfer_encoding(&headers).is_ok());

    headers.append(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
    assert!(validate_transfer_encoding(&headers).is_ok());
    headers.append(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
    assert!(matches!(
        validate_transfer_encoding(&headers),
        Err(DownloadError::InvalidResponse)
    ));
}

struct ShortWriter {
    bytes: Vec<u8>,
    max_write: usize,
    return_zero: bool,
}

impl AsyncWrite for ShortWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.return_zero {
            return Poll::Ready(Ok(0));
        }
        let count = buffer.len().min(self.max_write);
        self.bytes.extend_from_slice(&buffer[..count]);
        Poll::Ready(Ok(count))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn partial_writes_are_counted_exactly_and_zero_write_fails() {
    let mut writer = ShortWriter {
        bytes: Vec::new(),
        max_write: 2,
        return_zero: false,
    };
    let mut written = 0;
    let mut observed = Vec::new();
    write_chunk_accounted(
        &mut writer,
        Path::new("test.part"),
        b"abcdefg",
        &mut written,
        |count| observed.push(count),
    )
    .await
    .unwrap();
    assert_eq!(writer.bytes, b"abcdefg");
    assert_eq!(written, 7);
    assert_eq!(observed, vec![2, 4, 6, 7]);

    let mut zero_writer = ShortWriter {
        bytes: Vec::new(),
        max_write: 2,
        return_zero: true,
    };
    let error = write_chunk_accounted(
        &mut zero_writer,
        Path::new("test.part"),
        b"x",
        &mut 0,
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DownloadError::WriteZero));
}

fn managed_request(id: &str, url: String, destination: PathBuf) -> DownloadRequest {
    DownloadRequest {
        id: DownloadId::new(id).unwrap(),
        url,
        destination,
        resume: None,
    }
}

async fn wait_for_terminal(
    manager: &DownloadManager,
    handle: &DownloadHandle,
) -> Arc<crate::TerminalResult> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut progress = handle.progress();
        loop {
            if let Some(result) = manager.terminal_result(handle).unwrap() {
                return result;
            }
            progress.changed().await.unwrap();
        }
    })
    .await
    .expect("terminal result should be committed")
}

async fn serve_generated_body(
    body_len: u64,
    chunk_delay: Duration,
) -> (SocketAddr, Arc<AtomicUsize>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let peak_for_server = peak_active.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let active = active.clone();
            let peak = peak_for_server.clone();
            tokio::spawn(async move {
                let _ = read_request(&mut stream).await;
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);

                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
                );
                let mut ok = stream.write_all(header.as_bytes()).await.is_ok();
                let mut offset = 0_u64;
                let mut chunk = vec![0_u8; 64 * 1024];
                while ok && offset < body_len {
                    let size = (body_len - offset).min(chunk.len() as u64) as usize;
                    for (index, byte) in chunk[..size].iter_mut().enumerate() {
                        *byte = ((offset + index as u64) % 251) as u8;
                    }
                    ok = stream.write_all(&chunk[..size]).await.is_ok();
                    offset += size as u64;
                    if !chunk_delay.is_zero() {
                        tokio::time::sleep(chunk_delay).await;
                    }
                }
                let _ = stream.shutdown().await;
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    (address, peak_active, server)
}

fn verify_generated_file(path: &Path, expected_len: u64) {
    use std::io::Read;

    let mut file = fs::File::open(path).unwrap();
    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        for (index, actual) in buffer[..count].iter().enumerate() {
            assert_eq!(
                *actual,
                ((offset + index as u64) % 251) as u8,
                "content mismatch at byte {} in {}",
                offset + index as u64,
                path.display()
            );
        }
        offset += count as u64;
    }
    assert_eq!(
        offset,
        expected_len,
        "wrong output length: {}",
        path.display()
    );
}

async fn observe_monotonic_progress(handle: DownloadHandle) {
    let mut progress = handle.progress();
    let mut previous = 0;
    loop {
        let current = progress.borrow().clone();
        assert!(current.bytes_written >= previous, "progress moved backward");
        previous = current.bytes_written;
        assert!(
            current
                .total_bytes
                .is_none_or(|total| current.bytes_written <= total),
            "progress exceeded the declared total"
        );
        if matches!(
            current.state,
            DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
        ) {
            return;
        }
        progress.changed().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "manual local benchmark; run with cargo test --release local_performance_scenarios -- --ignored --nocapture"]
async fn local_performance_scenarios() {
    async fn scenario(
        label: &str,
        body_len: u64,
        jobs: usize,
        max_active: usize,
        chunk_delay: Duration,
        cycle: usize,
    ) {
        let (address, peak_active, server) = serve_generated_body(body_len, chunk_delay).await;
        let dir = TestDir::new();
        let manager = DownloadManager::with_config(DownloadManagerConfig {
            max_active,
            max_retained_jobs: jobs.max(1),
        })
        .unwrap();
        let started = Instant::now();
        let mut handles = Vec::with_capacity(jobs);
        for index in 0..jobs {
            handles.push(
                manager
                    .start(managed_request(
                        &format!("{label}-{cycle}-{index}"),
                        format!("http://{address}/payload"),
                        dir.path().join(format!("payload-{index}.bin")),
                    ))
                    .unwrap(),
            );
        }
        let queued_at_admission = handles
            .iter()
            .filter(|handle| handle.latest_progress().state == DownloadState::Queued)
            .count();
        let observers: Vec<_> = handles
            .iter()
            .cloned()
            .map(|handle| tokio::spawn(observe_monotonic_progress(handle)))
            .collect();
        for handle in &handles {
            let terminal = wait_for_terminal(&manager, handle).await;
            assert_eq!(terminal.state, DownloadState::Completed);
            assert_eq!(terminal.bytes_written, body_len);
        }
        let elapsed = started.elapsed();
        for observer in observers {
            observer.await.unwrap();
        }
        server.abort();
        manager.shutdown().await;

        for index in 0..jobs {
            verify_generated_file(&dir.path().join(format!("payload-{index}.bin")), body_len);
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), jobs);
        let total_mib = body_len as f64 * jobs as f64 / (1024.0 * 1024.0);
        let throughput = total_mib / elapsed.as_secs_f64();
        let peak = peak_active.load(Ordering::SeqCst);
        assert!(peak <= max_active, "server observed {peak} active requests");
        if jobs > max_active {
            assert!(
                queued_at_admission > 0,
                "jobs beyond the cap were not queued"
            );
        }
        eprintln!(
            "PERF label={label} cycle={cycle} jobs={jobs} per_job_bytes={body_len} elapsed_ms={} total_mib={total_mib:.2} throughput_mib_s={throughput:.2} max_active_observed={peak} queued_at_admission={queued_at_admission} success={jobs} failure=0 output_integrity=exact temp_files=0",
            elapsed.as_millis()
        );
    }

    scenario("small-fast", 1 << 20, 1, 1, Duration::ZERO, 0).await;
    scenario("large-fast", 64 << 20, 1, 1, Duration::ZERO, 0).await;
    scenario(
        "four-throttled",
        16 << 20,
        4,
        4,
        Duration::from_millis(1),
        0,
    )
    .await;
    scenario("eight-queued", 8 << 20, 8, 4, Duration::from_millis(1), 0).await;

    // Reuse one manager, engine, server, and directory while acknowledging
    // each terminal result. This catches stale reservations and temp leakage.
    let body_len = 1 << 20;
    let (address, _, server) = serve_generated_body(body_len, Duration::ZERO).await;
    let dir = TestDir::new();
    let manager = DownloadManager::with_config(DownloadManagerConfig {
        max_active: 4,
        max_retained_jobs: 2,
    })
    .unwrap();
    let mut elapsed_ms = Vec::with_capacity(10);
    for cycle in 0..10 {
        let destination = dir.path().join(format!("repeat-{cycle}.bin"));
        let started = Instant::now();
        let handle = manager
            .start(managed_request(
                &format!("repeat-cycle-{cycle}"),
                format!("http://{address}/payload"),
                destination.clone(),
            ))
            .unwrap();
        let observer = tokio::spawn(observe_monotonic_progress(handle.clone()));
        let terminal = wait_for_terminal(&manager, &handle).await;
        let elapsed = started.elapsed();
        observer.await.unwrap();
        assert_eq!(terminal.state, DownloadState::Completed);
        assert_eq!(terminal.bytes_written, body_len);
        verify_generated_file(&destination, body_len);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), cycle + 1);
        manager.acknowledge_terminal(&handle).unwrap();
        elapsed_ms.push(elapsed.as_secs_f64() * 1000.0);
    }
    server.abort();
    manager.shutdown().await;
    elapsed_ms.sort_by(f64::total_cmp);
    let median_ms = (elapsed_ms[4] + elapsed_ms[5]) / 2.0;
    let p90_ms = elapsed_ms[8];
    let min_ms = elapsed_ms[0];
    let max_ms = elapsed_ms[9];
    let mut throughputs_mib_s: Vec<_> = elapsed_ms
        .iter()
        .map(|elapsed| (body_len as f64 / (1024.0 * 1024.0)) / (elapsed / 1000.0))
        .collect();
    throughputs_mib_s.sort_by(f64::total_cmp);
    let median_mib_s = (throughputs_mib_s[4] + throughputs_mib_s[5]) / 2.0;
    eprintln!(
        "PERF label=repeated-cycle cycles=10 bytes_per_cycle={body_len} elapsed_ms_min={min_ms:.2} median={median_ms:.2} p90={p90_ms:.2} max={max_ms:.2} median_throughput_mib_s={median_mib_s:.2} success=10 failure=0 output_integrity=exact leftover_temp_files=0"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 10);
}

#[tokio::test]
async fn manager_completes_retains_acknowledges_and_rejects_stale_generation() {
    let payload = b"managed payload";
    let (url, server) = serve_once(response(
        "200 OK",
        &format!("Content-Length: {}\r\nConnection: close\r\n", payload.len()),
        payload,
    ))
    .await;
    let dir = TestDir::new();
    let manager = DownloadManager::new().unwrap();
    let target = dir.path().join("managed.bin");
    let old = manager
        .start(managed_request("job-1", url, target.clone()))
        .unwrap();
    let terminal = wait_for_terminal(&manager, &old).await;
    server.await.unwrap();

    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(terminal.bytes_written, payload.len() as u64);
    assert_eq!(fs::read(&target).unwrap(), payload);
    assert_eq!(old.latest_progress().bytes_written, payload.len() as u64);
    assert!(matches!(manager.acknowledge_terminal(&old), Ok(())));
    assert!(matches!(
        manager.terminal_result(&old),
        Err(ControlError::UnknownId)
    ));

    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 2\r\nConnection: close\r\n",
        b"ok",
    ))
    .await;
    let new = manager
        .start(managed_request("job-1", url, dir.path().join("second.bin")))
        .unwrap();
    assert_ne!(old.generation(), new.generation());
    assert!(matches!(
        manager.cancel(&old),
        Err(ControlError::StaleHandle)
    ));
    assert!(matches!(
        manager.acknowledge_terminal(&old),
        Err(ControlError::StaleHandle)
    ));
    let terminal = wait_for_terminal(&manager, &new).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    manager.acknowledge_terminal(&new).unwrap();
}

#[tokio::test]
async fn manager_cancellation_during_range_continuation_cleans_partial_file() {
    let (url, server, continuation_started, release) = serve_gated_range().await;
    let dir = TestDir::new();
    let target = dir.path().join("cancel-range.bin");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(managed_request("cancel-range", url, target.clone()))
        .unwrap();

    continuation_started.await.unwrap();
    assert_eq!(entry_count(&dir), 1, "only the owned temp should exist");
    assert_eq!(
        manager.cancel(&handle).unwrap(),
        CancelDisposition::Accepted
    );
    release.send(()).unwrap();
    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();

    assert_eq!(terminal.state, DownloadState::Cancelled);
    assert!((5..=6).contains(&terminal.bytes_written));
    assert!(!target.exists());
    assert_eq!(entry_count(&dir), 0);
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn manager_pauses_to_a_strong_etag_checkpoint_and_resumes_at_exact_offset() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (range_started_tx, range_started_rx) = oneshot::channel();
    let (release_first_tx, release_first_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut initial, _) = listener.accept().await.unwrap();
        read_request(&mut initial).await;
        let first = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"abcde",
        );
        initial.write_all(&first).await.unwrap();
        initial.shutdown().await.unwrap();

        let (mut interrupted_range, _) = listener.accept().await.unwrap();
        let request = read_request(&mut interrupted_range).await;
        assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
        let partial = response(
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"f",
        );
        interrupted_range.write_all(&partial).await.unwrap();
        let _ = range_started_tx.send(());
        let _ = release_first_rx.await;
        drop(interrupted_range);

        let (mut resumed_range, _) = listener.accept().await.unwrap();
        let request = read_request(&mut resumed_range).await;
        assert!(request.to_ascii_lowercase().contains("range: bytes=6-"));
        assert!(request
            .to_ascii_lowercase()
            .contains("if-range: \"stable\""));
        let tail = response(
            "206 Partial Content",
            "Content-Range: bytes 6-9/10\r\nContent-Length: 4\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"ghij",
        );
        resumed_range.write_all(&tail).await.unwrap();
        resumed_range.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let target = dir.path().join("pause-resume.bin");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(managed_request(
            "pause-resume",
            format!("http://{address}/file"),
            target.clone(),
        ))
        .unwrap();
    range_started_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if crate::lock_registry(&manager.core)
                .jobs
                .get(handle.id())
                .is_some_and(|record| record.bytes_written == 6)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first range byte should be written before pausing");

    manager.pause(&handle).unwrap();
    let mut progress = handle.progress();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if handle.latest_progress().state == DownloadState::Paused {
                break;
            }
            progress.changed().await.unwrap();
        }
    })
    .await
    .expect("pause must wait for the partial file to be synced");
    assert_eq!(handle.latest_progress().bytes_written, 6);
    release_first_tx.send(()).unwrap();

    manager.resume(&handle).unwrap();
    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(terminal.bytes_written, 10);
    assert_eq!(fs::read(&target).unwrap(), b"abcdefghij");
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn manager_coalesces_progress_without_losing_terminal_result() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(managed_request(
            "progress",
            url,
            dir.path().join("progress.bin"),
        ))
        .unwrap();
    started.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if crate::lock_registry(&manager.core)
                .jobs
                .get(handle.id())
                .is_some_and(|record| record.bytes_written == 5)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first partial body should be written");
    let token = crate::JobToken {
        id: handle.id().clone(),
        generation: handle.generation(),
    };

    {
        let mut registry = crate::lock_registry(&manager.core);
        registry.jobs.get_mut(handle.id()).unwrap().last_progress_at =
            std::time::Instant::now() - Duration::from_secs(1);
    }
    crate::update_progress(&manager.core, &token, 5, Some(10));
    crate::update_progress(&manager.core, &token, 6, Some(10));
    assert_eq!(handle.latest_progress().bytes_written, 5);
    assert_eq!(
        crate::lock_registry(&manager.core)
            .jobs
            .get(handle.id())
            .unwrap()
            .bytes_written,
        6,
        "internal accounting remains exact when the public snapshot is throttled"
    );

    {
        let mut registry = crate::lock_registry(&manager.core);
        registry.jobs.get_mut(handle.id()).unwrap().last_progress_at =
            std::time::Instant::now() - Duration::from_secs(1);
    }
    crate::update_progress(&manager.core, &token, 7, Some(10));
    assert_eq!(handle.latest_progress().bytes_written, 7);

    {
        let mut registry = crate::lock_registry(&manager.core);
        registry.jobs.get_mut(handle.id()).unwrap().bytes_written = 5;
    }
    manager.cancel(&handle).unwrap();
    release.send(()).unwrap();
    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Cancelled);
    assert_eq!(
        terminal.bytes_written, 5,
        "actual successful writes are authoritative"
    );
    assert_eq!(handle.latest_progress().state, DownloadState::Cancelled);
    assert!(manager.terminal_result(&handle).unwrap().is_some());
    manager.acknowledge_terminal(&handle).unwrap();
}

#[tokio::test]
async fn manager_admission_is_atomic_and_retention_frees_only_on_acknowledgement() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let manager = DownloadManager::with_config(DownloadManagerConfig {
        max_active: 1,
        max_retained_jobs: 2,
    })
    .unwrap();
    let first_path = dir.path().join("same.bin");
    let first = manager
        .start(managed_request("one", url.clone(), first_path.clone()))
        .unwrap();
    started.await.unwrap();

    let conflict = manager.start(managed_request(
        "two",
        url.clone(),
        dir.path().join(".").join("same.bin"),
    ));
    assert!(matches!(
        conflict,
        Err(StartError::InvalidRequest(
            DownloadError::DestinationConflict
        ))
    ));

    let queued = manager
        .start(managed_request(
            "three",
            url.clone(),
            dir.path().join("queued.bin"),
        ))
        .unwrap();
    assert_eq!(queued.latest_progress().state, DownloadState::Queued);
    assert!(matches!(
        manager.start(managed_request(
            "four",
            url.clone(),
            dir.path().join("four.bin")
        )),
        Err(StartError::CapacityReached)
    ));
    assert_eq!(
        manager.cancel(&queued).unwrap(),
        CancelDisposition::Accepted
    );
    assert_eq!(
        wait_for_terminal(&manager, &queued).await.state,
        DownloadState::Cancelled
    );
    assert!(matches!(
        manager.start(managed_request(
            "four",
            url.clone(),
            dir.path().join("four.bin")
        )),
        Err(StartError::CapacityReached)
    ));
    manager.acknowledge_terminal(&queued).unwrap();
    let fourth = manager
        .start(managed_request(
            "four",
            url.clone(),
            dir.path().join("four.bin"),
        ))
        .unwrap();
    assert_eq!(fourth.latest_progress().state, DownloadState::Queued);
    manager.cancel(&fourth).unwrap();
    wait_for_terminal(&manager, &fourth).await;
    manager.acknowledge_terminal(&fourth).unwrap();

    manager.cancel(&first).unwrap();
    release.send(()).unwrap();
    assert_eq!(
        wait_for_terminal(&manager, &first).await.state,
        DownloadState::Cancelled
    );
    server.await.unwrap();
    assert!(!first_path.exists());
    assert_eq!(entry_count(&dir), 0);
    manager.acknowledge_terminal(&first).unwrap();
}

#[tokio::test]
async fn concurrent_starts_atomically_reject_destination_and_id_conflicts() {
    let (address, _hits, server) = serve_loop(|_, _| {
        response(
            "200 OK",
            "Content-Length: 2\r\nConnection: close\r\n",
            b"ok",
        )
    })
    .await;
    let dir = TestDir::new();
    let manager = DownloadManager::new().unwrap();
    let url = format!("http://{address}/file");

    let barrier = Arc::new(Barrier::new(8));
    let mut starts = Vec::new();
    for index in 0..8 {
        let manager = manager.clone();
        let barrier = Arc::clone(&barrier);
        let url = url.clone();
        let target = dir.path().join("shared.bin");
        starts.push(std::thread::spawn(move || {
            barrier.wait();
            manager.start(managed_request(&format!("path-{index}"), url, target))
        }));
    }
    let accepted: Vec<_> = starts
        .into_iter()
        .map(|task| task.join().unwrap())
        .filter_map(Result::ok)
        .collect();
    assert_eq!(accepted.len(), 1, "same destination must have one winner");
    let terminal = wait_for_terminal(&manager, &accepted[0]).await;
    assert_eq!(terminal.state, DownloadState::Completed);
    manager.acknowledge_terminal(&accepted[0]).unwrap();

    let barrier = Arc::new(Barrier::new(8));
    let mut starts = Vec::new();
    for index in 0..8 {
        let manager = manager.clone();
        let barrier = Arc::clone(&barrier);
        let url = url.clone();
        let target = dir.path().join(format!("id-{index}.bin"));
        starts.push(std::thread::spawn(move || {
            barrier.wait();
            manager.start(managed_request("shared-id", url, target))
        }));
    }
    let accepted: Vec<_> = starts
        .into_iter()
        .map(|task| task.join().unwrap())
        .filter_map(Result::ok)
        .collect();
    assert_eq!(accepted.len(), 1, "duplicate ID must have one winner");
    let terminal = wait_for_terminal(&manager, &accepted[0]).await;
    assert_eq!(terminal.state, DownloadState::Completed);
    manager.acknowledge_terminal(&accepted[0]).unwrap();
    server.abort();
}

#[tokio::test]
async fn manager_dispatches_queued_jobs_in_fifo_order() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_started_tx, first_started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (paths_tx, mut paths_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut first_started_tx = Some(first_started_tx);
        let mut release_rx = Some(release_rx);
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            paths_tx.send(path).unwrap();
            if index == 0 {
                let _ = first_started_tx.take().unwrap().send(());
                let _ = release_rx.take().unwrap().await;
            }
            let body = index.to_string();
            let raw = response(
                "200 OK",
                "Content-Length: 1\r\nConnection: close\r\n",
                body.as_bytes(),
            );
            let _ = stream.write_all(&raw).await;
            let _ = stream.shutdown().await;
        }
    });
    let dir = TestDir::new();
    let manager = DownloadManager::with_config(DownloadManagerConfig {
        max_active: 1,
        max_retained_jobs: 4,
    })
    .unwrap();
    let first = manager
        .start(managed_request(
            "fifo-1",
            format!("http://{address}/one"),
            dir.path().join("one.bin"),
        ))
        .unwrap();
    first_started_rx.await.unwrap();
    let second = manager
        .start(managed_request(
            "fifo-2",
            format!("http://{address}/two"),
            dir.path().join("two.bin"),
        ))
        .unwrap();
    let third = manager
        .start(managed_request(
            "fifo-3",
            format!("http://{address}/three"),
            dir.path().join("three.bin"),
        ))
        .unwrap();
    assert_eq!(second.latest_progress().state, DownloadState::Queued);
    assert_eq!(third.latest_progress().state, DownloadState::Queued);
    release_tx.send(()).unwrap();

    for handle in [&first, &second, &third] {
        assert_eq!(
            wait_for_terminal(&manager, handle).await.state,
            DownloadState::Completed
        );
        manager.acknowledge_terminal(handle).unwrap();
    }
    server.await.unwrap();
    let paths = [
        paths_rx.recv().await.unwrap(),
        paths_rx.recv().await.unwrap(),
        paths_rx.recv().await.unwrap(),
    ];
    assert_eq!(paths, ["/one", "/two", "/three"]);
}

#[tokio::test]
async fn manager_cancellation_is_too_late_after_finalizing_wins() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 4\r\nConnection: close\r\n",
        b"done",
    ))
    .await;
    let dir = TestDir::new();
    let manager = DownloadManager::new().unwrap();
    let (finalizing, release) = manager.pause_next_finalization();
    let handle = manager
        .start(managed_request(
            "finalizing",
            url,
            dir.path().join("final.bin"),
        ))
        .unwrap();
    finalizing.await.unwrap();
    assert_eq!(handle.latest_progress().state, DownloadState::Finalizing);
    assert!(matches!(
        manager.cancel(&handle),
        Err(ControlError::TooLate)
    ));

    let manager_a = manager.clone();
    let shutdown_a = tokio::spawn(async move { manager_a.shutdown().await });
    let shutdown_b = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    while !crate::lock_registry(&manager.core).closing {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        manager.start(managed_request(
            "after-close",
            "http://127.0.0.1:1/file".to_owned(),
            dir.path().join("late.bin")
        )),
        Err(StartError::ShuttingDown)
    ));
    release.send(()).unwrap();
    let terminal = wait_for_terminal(&manager, &handle).await;
    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(shutdown_a.await.unwrap(), crate::ShutdownReport::Complete);
    assert_eq!(shutdown_b.await.unwrap(), crate::ShutdownReport::Complete);
    server.await.unwrap();
    manager.acknowledge_terminal(&handle).unwrap();
}

#[tokio::test]
async fn manager_worker_panic_is_terminalized_and_temp_is_cleaned() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 2\r\nConnection: close\r\n",
        b"ok",
    ))
    .await;
    let dir = TestDir::new();
    let engine = DownloadEngine::new().unwrap();
    engine.panic_next_worker();
    let manager = DownloadManager::with_engine(engine, DownloadManagerConfig::default()).unwrap();
    let handle = manager
        .start(managed_request("panic", url, dir.path().join("panic.bin")))
        .unwrap();
    let result = wait_for_terminal(&manager, &handle).await;
    assert_eq!(result.state, DownloadState::Failed);
    assert!(matches!(
        result.error.as_deref(),
        Some(DownloadError::WorkerPanicked)
    ));
    assert!(!dir.path().join("panic.bin").exists());
    assert_eq!(entry_count(&dir), 0);
    server.abort();
    manager.acknowledge_terminal(&handle).unwrap();
    assert_eq!(manager.shutdown().await, crate::ShutdownReport::Complete);
}

#[tokio::test]
async fn manager_preserves_cleanup_failure_when_worker_panics() {
    let (url, server) = serve_once(response(
        "200 OK",
        "Content-Length: 2\r\nConnection: close\r\n",
        b"ok",
    ))
    .await;
    let dir = TestDir::new();
    let engine = DownloadEngine::new().unwrap();
    engine.panic_next_worker();
    let manager = DownloadManager::with_engine(engine, DownloadManagerConfig::default()).unwrap();
    manager.fail_next_cleanup();
    let handle = manager
        .start(managed_request(
            "panic-cleanup",
            url,
            dir.path().join("panic.bin"),
        ))
        .unwrap();
    let result = wait_for_terminal(&manager, &handle).await;
    assert_eq!(result.state, DownloadState::Failed);
    assert!(matches!(
        result.error.as_deref(),
        Some(DownloadError::CleanupFailure { original, .. })
            if matches!(original.as_ref(), DownloadError::WorkerPanicked)
    ));
    assert!(!dir.path().join("panic.bin").exists());
    assert_eq!(entry_count(&dir), 1, "failed cleanup path must be retained");
    server.abort();
    manager.acknowledge_terminal(&handle).unwrap();
    assert_eq!(manager.shutdown().await, crate::ShutdownReport::Complete);
}

#[tokio::test]
async fn concurrent_shutdown_cancels_queued_jobs_and_is_idempotent() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let manager = DownloadManager::with_config(DownloadManagerConfig {
        max_active: 1,
        max_retained_jobs: 4,
    })
    .unwrap();
    let active = manager
        .start(managed_request(
            "active",
            url.clone(),
            dir.path().join("active.bin"),
        ))
        .unwrap();
    let queued = manager
        .start(managed_request(
            "queued",
            url,
            dir.path().join("queued.bin"),
        ))
        .unwrap();
    started.await.unwrap();

    let first = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    let second = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    release.send(()).unwrap();
    assert_eq!(first.await.unwrap(), crate::ShutdownReport::Complete);
    assert_eq!(second.await.unwrap(), crate::ShutdownReport::Complete);
    server.await.unwrap();
    assert_eq!(
        wait_for_terminal(&manager, &active).await.state,
        DownloadState::Cancelled
    );
    assert_eq!(
        wait_for_terminal(&manager, &queued).await.state,
        DownloadState::Cancelled
    );
    assert!(matches!(
        manager.start(managed_request(
            "late",
            "http://127.0.0.1:1/file".to_owned(),
            dir.path().join("late.bin")
        )),
        Err(StartError::ShuttingDown)
    ));
    manager.acknowledge_terminal(&active).unwrap();
    manager.acknowledge_terminal(&queued).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn manager_reserves_windows_destinations_case_insensitively() {
    let (url, server, started, release) = serve_gated().await;
    let dir = TestDir::new();
    let manager = DownloadManager::new().unwrap();
    let first = manager
        .start(managed_request(
            "case-a",
            url.clone(),
            dir.path().join("Case.bin"),
        ))
        .unwrap();
    started.await.unwrap();
    assert!(matches!(
        manager.start(managed_request("case-b", url, dir.path().join("case.bin"))),
        Err(StartError::InvalidRequest(
            DownloadError::DestinationConflict
        ))
    ));
    manager.cancel(&first).unwrap();
    release.send(()).unwrap();
    assert_eq!(
        wait_for_terminal(&manager, &first).await.state,
        DownloadState::Cancelled
    );
    server.await.unwrap();
    manager.acknowledge_terminal(&first).unwrap();
}

// --- Durable cross-restart resume ------------------------------------------

use crate::{DurableResume, ResumeCheckpoint};

fn durable_resume(
    temp_path: PathBuf,
    persisted_bytes: u64,
    effective_url: &str,
    strong_etag: &str,
    total_bytes: u64,
) -> DurableResume {
    DurableResume {
        temp_path,
        persisted_bytes,
        checkpoint: Some(ResumeCheckpoint {
            effective_url: effective_url.to_owned(),
            strong_etag: strong_etag.to_owned(),
            total_bytes,
        }),
    }
}

#[tokio::test]
async fn manager_adopts_a_persisted_partial_and_resumes_cross_restart() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        // Only the range request should arrive: the partial bytes were
        // persisted by the previous session, so no initial GET is made.
        let (mut range, _) = listener.accept().await.unwrap();
        let request = read_request(&mut range).await;
        let lowered = request.to_ascii_lowercase();
        assert!(lowered.contains("range: bytes=5-"));
        assert!(lowered.contains("if-range: \"stable\""));
        let tail = response(
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"fghij",
        );
        range.write_all(&tail).await.unwrap();
        range.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let persisted = dir.path().join(".consolex-previous-session-7.part");
    fs::write(&persisted, b"abcde").unwrap();
    let url = format!("http://{address}/file");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(DownloadRequest {
            id: DownloadId::new("durable-resume").unwrap(),
            url: url.clone(),
            destination: dir.path().join("durable.bin"),
            resume: Some(durable_resume(persisted.clone(), 5, &url, "\"stable\"", 10)),
        })
        .unwrap();

    // Adoption moves the partial into a process-owned temporary name.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(info) = manager.resume_info(&handle).unwrap() {
                if info.temp_path != persisted {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the persisted partial should be adopted under a new name");
    assert!(!persisted.exists(), "adoption must rename, not copy");

    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(terminal.bytes_written, 10);
    assert_eq!(
        fs::read(dir.path().join("durable.bin")).unwrap(),
        b"abcdefghij"
    );
    assert!(manager.resume_info(&handle).unwrap().is_none());
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn manager_discards_a_shrunk_persisted_partial_and_restarts_from_zero() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut fresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut fresh).await;
        // The partial is shorter than its persisted counter, so no Range
        // request may be sent: the transfer restarts from byte zero.
        assert!(!request.to_ascii_lowercase().contains("range:"));
        let full = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"abcdefghij",
        );
        fresh.write_all(&full).await.unwrap();
        fresh.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let persisted = dir.path().join(".consolex-shrunk-3.part");
    fs::write(&persisted, b"abc").unwrap();
    let url = format!("http://{address}/file");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(DownloadRequest {
            id: DownloadId::new("shrunk-partial").unwrap(),
            url,
            destination: dir.path().join("shrunk.bin"),
            resume: Some(durable_resume(
                persisted.clone(),
                6,
                &format!("http://{address}/file"),
                "\"stable\"",
                10,
            )),
        })
        .unwrap();

    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    let final_bytes = fs::read(dir.path().join("shrunk.bin")).unwrap();
    assert_eq!(final_bytes, b"abcdefghij");
    assert!(!persisted.exists(), "the untrustworthy partial is removed");
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn manager_restarts_from_zero_when_a_persisted_partial_meets_the_total() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut fresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut fresh).await;
        assert!(!request.to_ascii_lowercase().contains("range:"));
        let full = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"abcdefghij",
        );
        fresh.write_all(&full).await.unwrap();
        fresh.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let persisted = dir.path().join(".consolex-complete-10.part");
    fs::write(&persisted, b"0123456789").unwrap();
    let url = format!("http://{address}/file");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(DownloadRequest {
            id: DownloadId::new("overlong-partial").unwrap(),
            url,
            destination: dir.path().join("overlong.bin"),
            resume: Some(durable_resume(
                persisted.clone(),
                10,
                &format!("http://{address}/file"),
                "\"stable\"",
                10,
            )),
        })
        .unwrap();

    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    // The unverifiable tail is never trusted: the file is the server's body.
    assert_eq!(
        fs::read(dir.path().join("overlong.bin")).unwrap(),
        b"abcdefghij"
    );
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn manager_continues_normally_when_the_persisted_partial_is_missing() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut fresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut fresh).await;
        assert!(!request.to_ascii_lowercase().contains("range:"));
        let full = response(
            "200 OK",
            "Content-Length: 5\r\nConnection: close\r\n",
            b"hello",
        );
        fresh.write_all(&full).await.unwrap();
        fresh.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let missing = dir.path().join(".consolex-gone-1.part");
    let url = format!("http://{address}/file");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(DownloadRequest {
            id: DownloadId::new("missing-partial").unwrap(),
            url,
            destination: dir.path().join("missing.bin"),
            resume: Some(DurableResume {
                temp_path: missing,
                persisted_bytes: 2,
                checkpoint: None,
            }),
        })
        .unwrap();

    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(fs::read(dir.path().join("missing.bin")).unwrap(), b"hello");
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn resume_info_publishes_a_live_checkpoint_before_body_bytes_arrive() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (range_seen_tx, range_seen_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut initial, _) = listener.accept().await.unwrap();
        read_request(&mut initial).await;
        let head = response(
            "200 OK",
            "Content-Length: 10\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"abcde",
        );
        initial.write_all(&head).await.unwrap();
        let _ = initial.shutdown().await;

        // The engine must continue in-job from byte five; by the time this
        // request arrives, the checkpoint for the first response is already
        // visible to the registry.
        let (mut range, _) = listener.accept().await.unwrap();
        let request = read_request(&mut range).await;
        assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
        let _ = range_seen_tx.send(());
        let tail = response(
            "206 Partial Content",
            "Content-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"stable\"\r\nConnection: close\r\n",
            b"fghij",
        );
        range.write_all(&tail).await.unwrap();
        range.shutdown().await.unwrap();
    });

    let dir = TestDir::new();
    let url = format!("http://{address}/file");
    let manager = DownloadManager::new().unwrap();
    let handle = manager
        .start(managed_request(
            "live-checkpoint",
            url.clone(),
            dir.path().join("live.bin"),
        ))
        .unwrap();
    range_seen_rx.await.unwrap();

    let info = manager.resume_info(&handle).unwrap();
    let info = info.expect("an in-flight job with a temporary file reports resume info");
    assert!(info.temp_path.starts_with(dir.path()));
    assert!(info
        .temp_path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".consolex-")));
    assert_eq!(info.bytes_written, 5);
    let checkpoint = info
        .checkpoint
        .expect("the first response published a live checkpoint");
    assert_eq!(checkpoint.strong_etag, "\"stable\"");
    assert_eq!(checkpoint.total_bytes, 10);
    let effective = Url::parse(&url).unwrap();
    assert_eq!(checkpoint.effective_url, effective.as_str());

    let terminal = wait_for_terminal(&manager, &handle).await;
    server.await.unwrap();
    assert_eq!(terminal.state, DownloadState::Completed);
    assert_eq!(
        fs::read(dir.path().join("live.bin")).unwrap(),
        b"abcdefghij"
    );
    assert!(manager.resume_info(&handle).unwrap().is_none());
    manager.acknowledge_terminal(&handle).unwrap();
    manager.shutdown().await;
}
