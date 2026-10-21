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
}

impl DownloadEngine {
    pub fn new() -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(24 * 60 * 60))
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("redirect limit exceeded");
                }
                if attempt
                    .previous()
                    .iter()
                    .any(|url| url.scheme() == "https" && attempt.url().scheme() == "http")
                {
                    return attempt.error("HTTPS downgrade redirect rejected");
                }
                attempt.follow()
            }))
            .build()
            .map_err(|_| DownloadError::ClientInitialization)?;

        Ok(Self {
            client,
            reservations: Arc::new(Mutex::new(HashSet::new())),
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
        let (temp_path, mut file) = create_temp(&destination).await?;
        let attempt = self.transfer(&url, &temp_path, &mut file).await;

        let bytes_written = match attempt {
            Ok(bytes) => bytes,
            Err(original) => {
                drop(file);
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
            return Err(cleanup_or_original(&temp_path, original));
        }
        drop(file);

        if let Err(original) = promote_no_replace(&temp_path, &destination) {
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
        let response = self.client.get(url.clone()).send().await.map_err(|error| {
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
            return Err(DownloadError::HttpStatus(status.as_u16()));
        }

        let headers = response.headers();
        if headers.get(reqwest::header::CONTENT_RANGE).is_some() {
            return Err(DownloadError::InvalidResponse);
        }
        if let Some(encoding) = headers.get(reqwest::header::CONTENT_ENCODING) {
            let encoding = encoding
                .to_str()
                .map_err(|_| DownloadError::InvalidResponse)?;
            if !encoding.trim().eq_ignore_ascii_case("identity") {
                return Err(DownloadError::InvalidResponse);
            }
        }
        let expected = parse_content_length(headers)?;
        let mut written = 0_u64;
        let mut response = response;

        loop {
            let next = timeout(Duration::from_secs(30), response.chunk())
                .await
                .map_err(|_| DownloadError::BodyInactivityTimeout)?;
            let chunk = match next {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    if let Some(expected) = expected.filter(|expected| *expected > written) {
                        return Err(DownloadError::IncompleteResponse {
                            expected,
                            received: written,
                        });
                    }
                    return Err(DownloadError::NetworkFailure(if error.is_timeout() {
                        NetworkFailureKind::Timeout
                    } else {
                        NetworkFailureKind::RequestOrBody
                    }));
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

fn validate_url(raw: &str) -> Result<Url, DownloadError> {
    let url = Url::parse(raw).map_err(|_| DownloadError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
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

async fn create_temp(destination: &Path) -> Result<(PathBuf, tokio::fs::File), DownloadError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..128 {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(".consolex-{}-{id}.part", std::process::id()));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
        {
            Ok(file) => return Ok((temp_path, file)),
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
    // MoveFileW is same-volume and fails if the target already exists.
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
