use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use crate::{DownloadEngine, DownloadError, DownloadState};

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
        let mut request = vec![0_u8; 4096];
        let _ = stream.read(&mut request).await;
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
        let mut request = vec![0_u8; 4096];
        let _ = first.read(&mut request).await;
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{address}/final\r\nContent-Length: 3\r\nConnection: close\r\n\r\nold"
        );
        let _ = first.write_all(redirect.as_bytes()).await;
        let _ = first.shutdown().await;

        let (mut final_request, _) = listener.accept().await.expect("accept final request");
        let _ = final_request.read(&mut request).await;
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
