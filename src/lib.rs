//! A small, UI-independent single-stream HTTP/HTTPS download core.
//!
//! Each transfer streams into an exclusively-created sibling temporary file.
//! A successful response is flushed and length-checked before a no-clobber
//! promotion. This foundation intentionally has no queue, retries, cancellation,
//! persistence, or progress event system.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use thiserror::Error;
use tokio::{io::AsyncWriteExt, time::timeout};
use url::Url;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest allowed wait for response headers, and for each subsequent body read.
const READ_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(30);
const JOB_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_REDIRECTS: usize = 10;

/// Minimal lifecycle states exposed by this first engine increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Downloading,
    Completed,
    Failed,
}

/// Terminal data returned after a completed transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResult {
    pub state: DownloadState,
    pub destination: PathBuf,
    pub bytes_written: u64,
}

/// Typed failures. Messages intentionally avoid embedding the requested URL.
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("URL must be an absolute HTTP or HTTPS URL without embedded credentials")]
    InvalidUrl,
    #[error("destination path is invalid: {0}")]
    InvalidDestination(PathBuf),
    #[error("destination already exists or is reserved by another download")]
    DestinationConflict,
    #[error("failed to access {path} during {operation}: {source}")]
    Filesystem {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("HTTP server returned status {0}; only 200 OK is accepted")]
    HttpStatus(u16),
    #[error("HTTP response uses unsupported or invalid content semantics")]
    InvalidResponse,
    #[error("network request failed ({0})")]
    NetworkFailure(NetworkFailureKind),
    #[error("HTTP client initialization failed")]
    ClientInitialization,
    #[error("redirect policy rejected the response")]
    RedirectRejected,
    #[error("response ended before its declared Content-Length ({received} of {expected} bytes)")]
    IncompleteResponse { expected: u64, received: u64 },
    #[error("response byte count overflowed")]
    ByteCountOverflow,
    #[error("body did not make progress before the inactivity timeout")]
    BodyInactivityTimeout,
    #[error("failed to remove temporary file {path}: {source}; original failure: {original}")]
    CleanupFailure {
        path: PathBuf,
        #[source]
        source: io::Error,
        original: Box<DownloadError>,
    },
    #[error("destination was promoted but temporary-file cleanup failed at {path}: {source}")]
    PromotedCleanupFailure {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NetworkFailureKind {
    #[error("timeout")]
    Timeout,
    #[error("request or body failure")]
    RequestOrBody,
}

/// Reusable HTTP client and in-process destination reservation set.
#[derive(Clone)]
pub struct DownloadEngine {
    client: reqwest::Client,
    reservations: Arc<Mutex<HashSet<PathBuf>>>,
    read_inactivity: Duration,
}

impl DownloadEngine {
    pub fn new() -> Result<Self, DownloadError> {
        Self::with_timeouts(CONNECT_TIMEOUT, READ_INACTIVITY_TIMEOUT, JOB_DEADLINE)
    }

    /// Construct with explicit timeouts so tests can use short, deterministic values.
    fn with_timeouts(
        connect: Duration,
        read_inactivity: Duration,
        deadline: Duration,
    ) -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(deadline)
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(
                |attempt| match redirect_decision(attempt.previous(), attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(reason) => attempt.error(reason),
                },
            ))
            .build()
            .map_err(|_| DownloadError::ClientInitialization)?;

        Ok(Self {
            client,
            reservations: Arc::new(Mutex::new(HashSet::new())),
            read_inactivity,
        })
    }

    /// Download one HTTP/HTTPS resource to `destination` without replacing it.
    ///
    /// The destination's parent directory must already exist. Redirects are
    /// limited to ten hops and may not downgrade HTTPS to HTTP. Caller-supplied
    /// headers are deliberately not accepted in this increment.
    pub async fn download(
        &self,
        url: &str,
        destination: impl AsRef<Path>,
    ) -> Result<DownloadResult, DownloadError> {
        let parsed = validate_url(url)?;
        let (destination, reservation) = self.reserve_destination(destination.as_ref())?;

        let result = self.download_reserved(parsed, destination.clone()).await;
        drop(reservation);
        result
    }

    fn reserve_destination(
        &self,
        destination: &Path,
    ) -> Result<(PathBuf, Reservation), DownloadError> {
        let name = destination
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| DownloadError::InvalidDestination(destination.to_path_buf()))?;
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent =
            fs::canonicalize(parent).map_err(|source| DownloadError::Filesystem {
                operation: "resolve destination directory",
                path: parent.to_path_buf(),
                source,
            })?;
        let target = canonical_parent.join(name);

        match fs::symlink_metadata(&target) {
            Ok(_) => return Err(DownloadError::DestinationConflict),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DownloadError::Filesystem {
                    operation: "check destination",
                    path: target,
                    source,
                })
            }
        }

        let mut held = self
            .reservations
            .lock()
            .map_err(|_| DownloadError::Filesystem {
                operation: "reserve destination",
                path: target.clone(),
                source: io::Error::other("destination reservation lock was poisoned"),
            })?;
        if !held.insert(target.clone()) {
            return Err(DownloadError::DestinationConflict);
        }
        drop(held);

        Ok((
            target.clone(),
            Reservation {
                reservations: self.reservations.clone(),
                target,
            },
        ))
    }

    async fn download_reserved(
        &self,
        url: Url,
        destination: PathBuf,
    ) -> Result<DownloadResult, DownloadError> {
        let (temp_path, mut guard, file) = create_temp(&destination).await?;
        // Rebound after the guard so the handle is closed before the guard
        // (declared earlier, dropped later) removes the file.
        let mut file = file;
        let attempt = self.transfer(&url, &temp_path, &mut file).await;

        let bytes_written = match attempt {
            Ok(bytes) => bytes,
            Err(original) => {
                drop(file);
                guard.disarm();
                return Err(cleanup_or_original(&temp_path, original));
            }
        };

        let finalization = async {
            file.flush()
                .await
                .map_err(|source| DownloadError::Filesystem {
                    operation: "flush temporary file",
                    path: temp_path.clone(),
                    source,
                })?;
            file.sync_all()
                .await
                .map_err(|source| DownloadError::Filesystem {
                    operation: "sync temporary file",
                    path: temp_path.clone(),
                    source,
                })
        }
        .await;
        if let Err(original) = finalization {
            drop(file);
            guard.disarm();
            return Err(cleanup_or_original(&temp_path, original));
        }
        drop(file);

        // No `.await` sits between promotion and disarming, so the future cannot
        // be dropped in between.
        let promoted = promote_no_replace(&temp_path, &destination);
        guard.disarm();
        if let Err(original) = promoted {
            return Err(cleanup_or_original(&temp_path, original));
        }
        Ok(DownloadResult {
            state: DownloadState::Completed,
            destination,
            bytes_written,
        })
    }

    async fn transfer(
        &self,
        url: &Url,
        temp_path: &Path,
        file: &mut tokio::fs::File,
    ) -> Result<u64, DownloadError> {
        // The wait for response headers is bounded by the same inactivity limit
        // as body reads; otherwise a silent server would only be cut off by the
        // 24 h job deadline while holding the destination reservation.
        let response = timeout(
            self.read_inactivity,
            self.client
                .get(url.clone())
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .send(),
        )
        .await
        .map_err(|_| DownloadError::NetworkFailure(NetworkFailureKind::Timeout))?
        .map_err(|error| {
            if error.is_redirect() {
                DownloadError::RedirectRejected
            } else {
                DownloadError::NetworkFailure(if error.is_timeout() {
                    NetworkFailureKind::Timeout
                } else {
                    NetworkFailureKind::RequestOrBody
                })
            }
        })?;
        let status = response.status();
        if status != reqwest::StatusCode::OK {
            // Reqwest only applies the redirect policy to targets it knows how
            // to follow. An unsupported Location (for example file: or ftp:)
            // can therefore arrive here as an ordinary 3xx response.
            if status.is_redirection() && response.headers().contains_key(reqwest::header::LOCATION)
            {
                return Err(DownloadError::RedirectRejected);
            }
            return Err(DownloadError::HttpStatus(status.as_u16()));
        }

        let headers = response.headers();
        if headers.get(reqwest::header::CONTENT_RANGE).is_some() {
            return Err(DownloadError::InvalidResponse);
        }
        // Inspect every header value: `get` would only see the first one.
        for encoding in headers.get_all(reqwest::header::CONTENT_ENCODING) {
            let encoding = encoding
                .to_str()
                .map_err(|_| DownloadError::InvalidResponse)?;
            if !encoding.trim().eq_ignore_ascii_case("identity") {
                return Err(DownloadError::InvalidResponse);
            }
        }
        // The HTTP client only de-frames one `chunked` coding; other coding
        // sequences would be written to disk still encoded. Reject ambiguous
        // or repeated transfer codings.
        validate_transfer_encoding(headers)?;
        let expected = parse_content_length(headers)?;
        let mut written = 0_u64;
        let mut response = response;

        loop {
            let next = timeout(self.read_inactivity, response.chunk())
                .await
                .map_err(|_| DownloadError::BodyInactivityTimeout)?;
            let chunk = match next {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    // A deadline expiry is a timeout even when a declared length
                    // was not reached; only other failures are "short body".
                    if error.is_timeout() {
                        return Err(DownloadError::NetworkFailure(NetworkFailureKind::Timeout));
                    }
                    if let Some(expected) = expected.filter(|expected| *expected > written) {
                        return Err(DownloadError::IncompleteResponse {
                            expected,
                            received: written,
                        });
                    }
                    return Err(DownloadError::NetworkFailure(
                        NetworkFailureKind::RequestOrBody,
                    ));
                }
            };

            let chunk_len =
                u64::try_from(chunk.len()).map_err(|_| DownloadError::ByteCountOverflow)?;
            let next_written = written
                .checked_add(chunk_len)
                .ok_or(DownloadError::ByteCountOverflow)?;
            file.write_all(&chunk)
                .await
                .map_err(|source| DownloadError::Filesystem {
                    operation: "write temporary file",
                    path: temp_path.to_path_buf(),
                    source,
                })?;
            written = next_written;
        }

        if let Some(expected) = expected {
            if written != expected {
                return Err(DownloadError::IncompleteResponse {
                    expected,
                    received: written,
                });
            }
        }
        Ok(written)
    }
}

/// Removes the temporary file if the download future is dropped (task abort,
/// an enclosing `timeout`, `select!`) before an explicit success or failure path
/// has taken over. Those paths disarm it after doing their own, error-reporting
/// cleanup or promotion.
struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl TempGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Reservation {
    reservations: Arc<Mutex<HashSet<PathBuf>>>,
    target: PathBuf,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.reservations.lock() {
            reservations.remove(&self.target);
        }
    }
}

/// Pure redirect policy. `previous` holds the original request URL plus every
/// hop already followed, so `previous.len() == hops_followed + 1`. Up to
/// `MAX_REDIRECTS` hops are followed; the next one is rejected.
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

/// Shared by the initial URL check and the redirect policy.
fn is_allowed_target(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn validate_url(raw: &str) -> Result<Url, DownloadError> {
    let url = Url::parse(raw).map_err(|_| DownloadError::InvalidUrl)?;
    if !is_allowed_target(&url) {
        return Err(DownloadError::InvalidUrl);
    }
    Ok(url)
}

fn parse_content_length(
    headers: &reqwest::header::HeaderMap,
) -> Result<Option<u64>, DownloadError> {
    let values: Vec<_> = headers
        .get_all(reqwest::header::CONTENT_LENGTH)
        .iter()
        .collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => {
            let text = value.to_str().map_err(|_| DownloadError::InvalidResponse)?;
            if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(DownloadError::InvalidResponse);
            }
            text.parse::<u64>()
                .map(Some)
                .map_err(|_| DownloadError::InvalidResponse)
        }
        _ => Err(DownloadError::InvalidResponse),
    }
}

fn validate_transfer_encoding(headers: &reqwest::header::HeaderMap) -> Result<(), DownloadError> {
    let values: Vec<_> = headers
        .get_all(reqwest::header::TRANSFER_ENCODING)
        .iter()
        .collect();
    match values.as_slice() {
        [] => Ok(()),
        [value] => {
            let coding = value.to_str().map_err(|_| DownloadError::InvalidResponse)?;
            if coding.trim().eq_ignore_ascii_case("chunked") {
                Ok(())
            } else {
                Err(DownloadError::InvalidResponse)
            }
        }
        _ => Err(DownloadError::InvalidResponse),
    }
}

async fn create_temp(
    destination: &Path,
) -> Result<(PathBuf, TempGuard, tokio::fs::File), DownloadError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..128 {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let Some(temp_path) = temp_candidate(parent, destination, std::process::id(), id) else {
            continue;
        };
        let open_path = temp_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&open_path)
                .map(|file| OpenedTemp {
                    file: Some(file),
                    guard: Some(TempGuard::new(open_path)),
                })
        })
        .await
        .map_err(|error| DownloadError::Filesystem {
            operation: "create exclusive temporary file",
            path: temp_path.clone(),
            source: io::Error::other(error.to_string()),
        })?;
        match result {
            Ok(created) => {
                let (file, guard) = created.into_parts();
                return Ok((temp_path, guard, tokio::fs::File::from_std(file)));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(DownloadError::Filesystem {
                    operation: "create exclusive temporary file",
                    path: temp_path,
                    source,
                })
            }
        }
    }
    Err(DownloadError::Filesystem {
        operation: "find a unique temporary filename",
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary name attempts exhausted",
        ),
    })
}

fn temp_candidate(parent: &Path, destination: &Path, pid: u32, id: u64) -> Option<PathBuf> {
    let candidate = parent.join(format!(".consolex-{pid}-{id}.part"));
    (candidate != destination).then_some(candidate)
}

/// Keeps the file and cleanup guard together while an async task is waiting
/// for the blocking exclusive-open operation to finish. If that task is
/// abandoned, the file is closed before the guard removes its path.
struct OpenedTemp {
    file: Option<fs::File>,
    guard: Option<TempGuard>,
}

impl OpenedTemp {
    fn into_parts(mut self) -> (fs::File, TempGuard) {
        (
            self.file.take().expect("opened temp file is present"),
            self.guard.take().expect("temp cleanup guard is present"),
        )
    }
}

impl Drop for OpenedTemp {
    fn drop(&mut self) {
        // The guard field drops after this method, so close before removing.
        drop(self.file.take());
    }
}

fn cleanup_or_original(path: &Path, original: DownloadError) -> DownloadError {
    match fs::remove_file(path) {
        Ok(()) => original,
        Err(error) if error.kind() == io::ErrorKind::NotFound => original,
        Err(source) => DownloadError::CleanupFailure {
            path: path.to_path_buf(),
            source,
            original: Box::new(original),
        },
    }
}

#[cfg(windows)]
fn promote_no_replace(temp: &Path, destination: &Path) -> Result<(), DownloadError> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileW(existing: *const u16, new: *const u16) -> i32;
    }

    let mut source: Vec<u16> = temp.as_os_str().encode_wide().collect();
    let mut target: Vec<u16> = destination.as_os_str().encode_wide().collect();
    source.push(0);
    target.push(0);
    if source[..source.len() - 1].contains(&0) || target[..target.len() - 1].contains(&0) {
        return Err(DownloadError::InvalidDestination(destination.to_path_buf()));
    }
    // UNVERIFIED on Windows (needs Windows CI): the same-directory temporary
    // file should make this a same-volume move, and MoveFileW should fail when
    // the destination name already exists instead of replacing it.
    let result = unsafe { MoveFileW(source.as_ptr(), target.as_ptr()) };
    if result != 0 {
        Ok(())
    } else {
        let source = io::Error::last_os_error();
        if source.kind() == io::ErrorKind::AlreadyExists {
            Err(DownloadError::DestinationConflict)
        } else {
            Err(DownloadError::Filesystem {
                operation: "promote temporary file without replacement",
                path: destination.to_path_buf(),
                source,
            })
        }
    }
}

#[cfg(not(windows))]
fn promote_no_replace(temp: &Path, destination: &Path) -> Result<(), DownloadError> {
    fs::hard_link(temp, destination).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            DownloadError::DestinationConflict
        } else {
            DownloadError::Filesystem {
                operation: "promote temporary file without replacement",
                path: destination.to_path_buf(),
                source,
            }
        }
    })?;
    fs::remove_file(temp).map_err(|source| DownloadError::PromotedCleanupFailure {
        path: temp.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests;
