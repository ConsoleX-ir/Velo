//! A UI-independent, managed single-stream HTTP/HTTPS download core.
//!
//! Each transfer streams into an exclusively-created sibling temporary file.
//! A successful response is flushed and length-checked before a no-clobber
//! promotion. Managed jobs use FIFO scheduling, cooperative cancellation,
//! coalesced progress snapshots, and retained terminal results. Each accepted
//! jobs may continue interrupted bodies with bounded, validator-checked Range
//! requests. Same-session pause/resume uses a strong-ETag checkpoint; app-level
//! history and byte-zero retries are implemented by the Tauri service.

#[cfg(test)]
use std::{collections::HashSet, sync::atomic::AtomicBool};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    future::Future,
    io,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::watch,
    task::JoinHandle,
    time::timeout,
};
use url::Url;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest allowed wait for response headers, and for each subsequent body read.
const READ_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(30);
const JOB_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_REDIRECTS: usize = 10;
const MAX_RESUME_REQUESTS: usize = 3;
const DEFAULT_MAX_ACTIVE: usize = 4;
const DEFAULT_MAX_RETAINED: usize = 100;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Caller-supplied stable identity for one accepted job.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DownloadId(String);

impl DownloadId {
    pub fn new(value: impl Into<String>) -> Result<Self, StartError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 {
            return Err(StartError::InvalidId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One HTTP/HTTPS GET and its no-clobber destination.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub id: DownloadId,
    pub url: String,
    pub destination: PathBuf,
    /// Optional partial-transfer state persisted by a previous session.
    /// Every field is re-verified on disk and against the server before any
    /// byte is appended; an invalid checkpoint degrades to a fresh transfer.
    pub resume: Option<DurableResume>,
}

/// Persisted partial-transfer state handed back to the manager after a
/// process restart. `persisted_bytes` is a lower bound on the temporary
/// file's length: progress may have outpaced the last persisted snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableResume {
    pub temp_path: PathBuf,
    pub persisted_bytes: u64,
    pub checkpoint: Option<ResumeCheckpoint>,
}

/// Durable-resume metadata observed for an in-flight job so the application
/// service can persist it between sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeInfo {
    pub temp_path: PathBuf,
    pub bytes_written: u64,
    pub checkpoint: Option<ResumeCheckpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadManagerConfig {
    pub max_active: usize,
    pub max_retained_jobs: usize,
}

impl Default for DownloadManagerConfig {
    fn default() -> Self {
        Self {
            max_active: DEFAULT_MAX_ACTIVE,
            max_retained_jobs: DEFAULT_MAX_RETAINED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Preparing,
    Downloading,
    Pausing,
    Paused,
    Cancelling,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub id: DownloadId,
    pub state: DownloadState,
    pub bytes_written: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DownloadHandle {
    id: DownloadId,
    generation: u64,
    progress: watch::Receiver<ProgressSnapshot>,
}

impl DownloadHandle {
    pub fn id(&self) -> &DownloadId {
        &self.id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn progress(&self) -> watch::Receiver<ProgressSnapshot> {
        self.progress.clone()
    }

    pub fn latest_progress(&self) -> ProgressSnapshot {
        self.progress.borrow().clone()
    }
}

#[derive(Debug)]
pub struct TerminalResult {
    pub id: DownloadId,
    pub state: DownloadState,
    pub destination: PathBuf,
    pub bytes_written: u64,
    pub error: Option<Arc<DownloadError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDisposition {
    Accepted,
    AlreadyCancelling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReport {
    Complete,
}

#[derive(Debug, Error)]
pub enum StartError {
    #[error("download ID must contain 1 to 128 bytes")]
    InvalidId,
    #[error("download ID is already retained")]
    DuplicateId,
    #[error("download generation space is exhausted")]
    GenerationExhausted,
    #[error("manager retained-job capacity has been reached")]
    CapacityReached,
    #[error("manager is shutting down")]
    ShuttingDown,
    #[error("invalid download request: {0}")]
    InvalidRequest(#[from] DownloadError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControlError {
    #[error("download ID is not currently registered")]
    UnknownId,
    #[error("download handle generation is stale")]
    StaleHandle,
    #[error("download is already terminal")]
    AlreadyTerminal,
    #[error("download has not reached a terminal state")]
    NotTerminal,
    #[error("cancellation is too late because finalization has begun")]
    TooLate,
    #[error("download cannot be paused in its current state")]
    CannotPause,
    #[error("download is not paused")]
    NotPaused,
    #[error("paused temporary data could not be removed")]
    CleanupFailed,
}

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("download manager limits must be greater than zero")]
    InvalidLimits,
    #[error("a Tokio runtime must be active when constructing the manager")]
    RuntimeUnavailable,
    #[error(transparent)]
    Engine(#[from] DownloadError),
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
    #[error("HTTP server returned unexpected status {0}")]
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
    #[error("file writer returned zero bytes before the chunk was complete")]
    WriteZero,
    #[error("download cancellation requested")]
    Cancelled,
    #[error("download worker panicked")]
    WorkerPanicked,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCheckpoint {
    pub effective_url: String,
    pub strong_etag: String,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerControl {
    Running,
    Pause,
    Cancel,
}

enum TransferExit {
    Complete(u64),
    Paused {
        bytes_written: u64,
        checkpoint: Option<ResumeCheckpoint>,
    },
    Cancelled,
}

fn interrupted_transfer(
    signal: WorkerControl,
    bytes_written: u64,
    effective_url: &Url,
    total_bytes: Option<u64>,
    etag: Option<&reqwest::header::HeaderValue>,
) -> TransferExit {
    match signal {
        WorkerControl::Cancel => TransferExit::Cancelled,
        WorkerControl::Running => TransferExit::Cancelled,
        WorkerControl::Pause => {
            let checkpoint = total_bytes
                .filter(|total| bytes_written > 0 && bytes_written < *total)
                .and_then(|total_bytes| {
                    let strong_etag = etag?.to_str().ok()?.to_owned();
                    Some(ResumeCheckpoint {
                        effective_url: effective_url.as_str().to_owned(),
                        strong_etag,
                        total_bytes,
                    })
                });
            TransferExit::Paused {
                bytes_written,
                checkpoint,
            }
        }
    }
}

/// Checkpoint visible to the registry after the first response. Requires the
/// same trio as every other resume: a strong entity tag, a known total, and
/// the effective (post-redirect) URL.
fn live_checkpoint(
    effective_url: &Url,
    total_bytes: Option<u64>,
    etag: Option<&reqwest::header::HeaderValue>,
) -> Option<ResumeCheckpoint> {
    let total_bytes = total_bytes?;
    if total_bytes == 0 {
        return None;
    }
    let strong_etag = etag?.to_str().ok()?.to_owned();
    Some(ResumeCheckpoint {
        effective_url: effective_url.as_str().to_owned(),
        strong_etag,
        total_bytes,
    })
}

struct CatchUnwind<F>(Pin<Box<F>>);

impl<F> Unpin for CatchUnwind<F> {}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = self.get_mut().0.as_mut();
        match catch_unwind(AssertUnwindSafe(|| future.poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

enum TransferEvent {
    Downloading {
        /// Validator checkpoint learned from the first response, published
        /// before any body byte is written so a crash cannot lose it.
        checkpoint: Option<ResumeCheckpoint>,
    },
    BytesWritten {
        bytes_written: u64,
        total_bytes: Option<u64>,
    },
}

async fn wait_for_control(receiver: &mut watch::Receiver<WorkerControl>) -> WorkerControl {
    loop {
        let control = *receiver.borrow();
        if control != WorkerControl::Running {
            return control;
        }
        if receiver.changed().await.is_err() {
            return WorkerControl::Cancel;
        }
    }
}

async fn cancellable<F: std::future::Future>(
    receiver: &mut Option<&mut watch::Receiver<WorkerControl>>,
    future: F,
) -> Result<F::Output, WorkerControl> {
    if let Some(receiver) = receiver.as_deref_mut() {
        tokio::select! {
            biased;
            control = wait_for_control(receiver) => Err(control),
            output = future => Ok(output),
        }
    } else {
        Ok(future.await)
    }
}

/// Reusable HTTP client and in-process destination reservation set.
#[derive(Clone)]
pub struct DownloadEngine {
    client: reqwest::Client,
    range_client: reqwest::Client,
    #[cfg(test)]
    reservations: Arc<Mutex<HashSet<PathBuf>>>,
    read_inactivity: Duration,
    #[cfg(test)]
    panic_next_worker: Arc<AtomicBool>,
}

#[cfg(test)]
impl DownloadEngine {
    fn panic_next_worker(&self) {
        self.panic_next_worker.store(true, Ordering::SeqCst);
    }
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
        // Continuations go directly to the effective URL established by the
        // initial request. Do not forward Range/If-Range to a redirect target.
        let range_client = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(deadline)
            .referer(false)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DownloadError::ClientInitialization)?;

        Ok(Self {
            client,
            range_client,
            #[cfg(test)]
            reservations: Arc::new(Mutex::new(HashSet::new())),
            read_inactivity,
            #[cfg(test)]
            panic_next_worker: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Download one HTTP/HTTPS resource to `destination` without replacing it.
    ///
    /// The destination's parent directory must already exist. Redirects are
    /// limited to ten hops and may not downgrade HTTPS to HTTP. Caller-supplied
    /// headers are deliberately not accepted in this increment.
    #[cfg(test)]
    async fn download(
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

    #[cfg(test)]
    fn reserve_destination(
        &self,
        destination: &Path,
    ) -> Result<(PathBuf, Reservation), DownloadError> {
        let target = self.canonical_destination(destination)?;
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

    fn canonical_destination(&self, destination: &Path) -> Result<PathBuf, DownloadError> {
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

        Ok(target)
    }

    #[cfg(test)]
    async fn download_reserved(
        &self,
        url: Url,
        destination: PathBuf,
    ) -> Result<DownloadResult, DownloadError> {
        let (temp_path, mut guard, file) = create_temp(&destination).await?;
        // Rebound after the guard so the handle is closed before the guard
        // (declared earlier, dropped later) removes the file.
        let mut file = file;
        let attempt = self
            .transfer(&url, &temp_path, &mut file, None, None, 0, |_| {})
            .await;

        let bytes_written = match attempt {
            Ok(TransferExit::Complete(bytes)) => bytes,
            Ok(TransferExit::Cancelled) => {
                drop(file);
                guard.disarm();
                return Err(cleanup_or_original(&temp_path, DownloadError::Cancelled));
            }
            Ok(TransferExit::Paused { .. }) => {
                drop(file);
                guard.disarm();
                return Err(cleanup_or_original(&temp_path, DownloadError::Cancelled));
            }
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

    // Keeping transfer's source, checkpoint, destination, control channel, and
    // progress callback explicit makes the streaming contract easy to audit.
    #[allow(clippy::too_many_arguments)]
    async fn transfer<F>(
        &self,
        url: &Url,
        temp_path: &Path,
        file: &mut tokio::fs::File,
        mut control: Option<&mut watch::Receiver<WorkerControl>>,
        checkpoint: Option<ResumeCheckpoint>,
        initial_bytes: u64,
        mut progress: F,
    ) -> Result<TransferExit, DownloadError>
    where
        F: FnMut(TransferEvent),
    {
        let mut written = initial_bytes;
        let started = Instant::now();
        let mut resume_url = checkpoint
            .as_ref()
            .and_then(|saved| Url::parse(&saved.effective_url).ok())
            .unwrap_or_else(|| url.clone());
        let mut expected_total = checkpoint.as_ref().map(|saved| saved.total_bytes);
        let mut etag: Option<reqwest::header::HeaderValue> = checkpoint
            .as_ref()
            .and_then(|saved| reqwest::header::HeaderValue::from_str(&saved.strong_etag).ok());
        let mut resume_requests = 0;
        let mut first_response = checkpoint.is_none();

        loop {
            if control
                .as_ref()
                .is_some_and(|receiver| *receiver.borrow() != WorkerControl::Running)
            {
                let signal = control
                    .as_ref()
                    .map(|receiver| *receiver.borrow())
                    .unwrap_or(WorkerControl::Cancel);
                return Ok(interrupted_transfer(
                    signal,
                    written,
                    &resume_url,
                    expected_total,
                    etag.as_ref(),
                ));
            }

            let is_resume = !first_response;
            let client = if is_resume {
                &self.range_client
            } else {
                &self.client
            };
            let mut request = client
                .get(resume_url.clone())
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
            if is_resume {
                let validator = etag.as_ref().ok_or(DownloadError::InvalidResponse)?;
                request = request
                    .header(reqwest::header::RANGE, format!("bytes={written}-"))
                    .header(reqwest::header::IF_RANGE, validator.clone());
            }

            let remaining = JOB_DEADLINE.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(DownloadError::NetworkFailure(NetworkFailureKind::Timeout));
            }
            let response_result = cancellable(
                &mut control,
                timeout(self.read_inactivity.min(remaining), request.send()),
            )
            .await;
            let response = match response_result {
                Err(signal) => {
                    return Ok(interrupted_transfer(
                        signal,
                        written,
                        &resume_url,
                        expected_total,
                        etag.as_ref(),
                    ))
                }
                Ok(Err(_)) => {
                    if is_resume && resume_requests < MAX_RESUME_REQUESTS {
                        resume_requests += 1;
                        continue;
                    }
                    return Err(DownloadError::NetworkFailure(NetworkFailureKind::Timeout));
                }
                Ok(Ok(Err(error))) => {
                    if is_resume && resume_requests < MAX_RESUME_REQUESTS && !error.is_redirect() {
                        resume_requests += 1;
                        continue;
                    }
                    return Err(if error.is_redirect() {
                        DownloadError::RedirectRejected
                    } else {
                        DownloadError::NetworkFailure(if error.is_timeout() {
                            NetworkFailureKind::Timeout
                        } else {
                            NetworkFailureKind::RequestOrBody
                        })
                    });
                }
                Ok(Ok(Ok(response))) => response,
            };

            let status = response.status();
            if first_response {
                if status != reqwest::StatusCode::OK {
                    if status.is_redirection()
                        && response.headers().contains_key(reqwest::header::LOCATION)
                    {
                        return Err(DownloadError::RedirectRejected);
                    }
                    return Err(DownloadError::HttpStatus(status.as_u16()));
                }
                if response
                    .headers()
                    .contains_key(reqwest::header::CONTENT_RANGE)
                {
                    return Err(DownloadError::InvalidResponse);
                }
                resume_url = response.url().clone();
                validate_entity_headers(response.headers())?;
                expected_total = parse_content_length(response.headers())?;
                etag = parse_strong_etag(response.headers())?;
                progress(TransferEvent::Downloading {
                    checkpoint: live_checkpoint(&resume_url, expected_total, etag.as_ref()),
                });
            } else {
                // A Range request must never be appended to a full response or
                // to a different representation/redirect target.
                if response.status().is_redirection() {
                    return Err(DownloadError::RedirectRejected);
                }
                if status == reqwest::StatusCode::OK {
                    // A server that ignores If-Range or reports a changed
                    // representation must never have its full body appended.
                    return Err(DownloadError::InvalidResponse);
                }
                if status != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(DownloadError::HttpStatus(status.as_u16()));
                }
                if response.url() != &resume_url {
                    return Err(DownloadError::InvalidResponse);
                }
                validate_entity_headers(response.headers())?;
                if parse_strong_etag(response.headers())? != etag {
                    return Err(DownloadError::InvalidResponse);
                }
            }

            let response_segment_end = if first_response {
                expected_total
            } else {
                let range = parse_content_range(response.headers())?;
                let total = expected_total.ok_or(DownloadError::InvalidResponse)?;
                if range.start != written || range.total != total {
                    return Err(DownloadError::InvalidResponse);
                }
                let segment_length = range
                    .end
                    .checked_sub(range.start)
                    .and_then(|length| length.checked_add(1))
                    .ok_or(DownloadError::InvalidResponse)?;
                if parse_content_length(response.headers())?
                    .is_some_and(|length| length != segment_length)
                {
                    return Err(DownloadError::InvalidResponse);
                }
                Some(range.end + 1)
            };

            let mut response = response;
            let mut body_error = None;
            loop {
                if control
                    .as_ref()
                    .is_some_and(|receiver| *receiver.borrow() != WorkerControl::Running)
                {
                    let signal = control
                        .as_ref()
                        .map(|receiver| *receiver.borrow())
                        .unwrap_or(WorkerControl::Cancel);
                    return Ok(interrupted_transfer(
                        signal,
                        written,
                        &resume_url,
                        expected_total,
                        etag.as_ref(),
                    ));
                }
                let remaining = JOB_DEADLINE.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    body_error = Some(DownloadError::NetworkFailure(NetworkFailureKind::Timeout));
                    break;
                }
                let next = cancellable(
                    &mut control,
                    timeout(self.read_inactivity.min(remaining), response.chunk()),
                )
                .await;
                let next = match next {
                    Err(signal) => {
                        return Ok(interrupted_transfer(
                            signal,
                            written,
                            &resume_url,
                            expected_total,
                            etag.as_ref(),
                        ))
                    }
                    Ok(Err(_)) => {
                        body_error = Some(DownloadError::BodyInactivityTimeout);
                        break;
                    }
                    Ok(Ok(next)) => next,
                };
                let chunk = match next {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) => break,
                    Err(error) => {
                        body_error = Some(if error.is_timeout() {
                            DownloadError::NetworkFailure(NetworkFailureKind::Timeout)
                        } else if let Some(end) = response_segment_end {
                            DownloadError::IncompleteResponse {
                                expected: end,
                                received: written,
                            }
                        } else {
                            DownloadError::NetworkFailure(NetworkFailureKind::RequestOrBody)
                        });
                        break;
                    }
                };

                let chunk_len =
                    u64::try_from(chunk.len()).map_err(|_| DownloadError::ByteCountOverflow)?;
                let after_chunk = written
                    .checked_add(chunk_len)
                    .ok_or(DownloadError::ByteCountOverflow)?;
                if response_segment_end.is_some_and(|end| after_chunk > end) {
                    return Err(DownloadError::InvalidResponse);
                }
                write_chunk_accounted(file, temp_path, &chunk, &mut written, |count| {
                    progress(TransferEvent::BytesWritten {
                        bytes_written: count,
                        total_bytes: expected_total,
                    });
                })
                .await?;
            }

            if let Some(error) = body_error {
                let can_resume = expected_total.is_some_and(|total| written < total)
                    && etag.is_some()
                    && resume_requests < MAX_RESUME_REQUESTS;
                if can_resume {
                    resume_requests += 1;
                    first_response = false;
                    continue;
                }
                return Err(error);
            }

            if let Some(end) = response_segment_end {
                if written != end {
                    let error = DownloadError::IncompleteResponse {
                        expected: end,
                        received: written,
                    };
                    let can_resume = expected_total.is_some_and(|total| written < total)
                        && etag.is_some()
                        && resume_requests < MAX_RESUME_REQUESTS;
                    if can_resume {
                        resume_requests += 1;
                        first_response = false;
                        continue;
                    }
                    return Err(error);
                }
            }

            if expected_total.is_some_and(|total| written == total) {
                return Ok(TransferExit::Complete(written));
            }
            if expected_total.is_none() {
                return Ok(TransferExit::Complete(written));
            }
            if etag.is_none() || resume_requests >= MAX_RESUME_REQUESTS {
                return Err(DownloadError::IncompleteResponse {
                    expected: expected_total.unwrap_or(written),
                    received: written,
                });
            }
            resume_requests += 1;
            first_response = false;
        }
    }
}

async fn write_chunk_accounted<W, F>(
    writer: &mut W,
    path: &Path,
    chunk: &[u8],
    written: &mut u64,
    mut on_write: F,
) -> Result<(), DownloadError>
where
    W: AsyncWrite + Unpin,
    F: FnMut(u64),
{
    let mut offset = 0;
    while offset < chunk.len() {
        let count =
            writer
                .write(&chunk[offset..])
                .await
                .map_err(|source| DownloadError::Filesystem {
                    operation: "write temporary file",
                    path: path.to_path_buf(),
                    source,
                })?;
        if count == 0 {
            return Err(DownloadError::WriteZero);
        }
        offset += count;
        *written = (*written)
            .checked_add(u64::try_from(count).map_err(|_| DownloadError::ByteCountOverflow)?)
            .ok_or(DownloadError::ByteCountOverflow)?;
        on_write(*written);
    }
    Ok(())
}

struct JobToken {
    id: DownloadId,
    generation: u64,
}

struct JobRecord {
    generation: u64,
    url: Url,
    destination: PathBuf,
    state: DownloadState,
    bytes_written: u64,
    total_bytes: Option<u64>,
    last_progress_at: Instant,
    progress_tx: watch::Sender<ProgressSnapshot>,
    control_tx: watch::Sender<WorkerControl>,
    cancel_requested: bool,
    active_slot_held: bool,
    temp_path: Option<PathBuf>,
    checkpoint: Option<ResumeCheckpoint>,
    resume: Option<DurableResume>,
    promoted: bool,
    terminal: Option<Arc<TerminalResult>>,
    task: Option<JoinHandle<()>>,
}

struct Registry {
    jobs: HashMap<DownloadId, JobRecord>,
    queue: VecDeque<DownloadId>,
    destinations: Vec<(PathBuf, DownloadId)>,
    next_generation: u64,
    active_slots: usize,
    closing: bool,
}

struct ManagerCore {
    engine: DownloadEngine,
    runtime: tokio::runtime::Handle,
    config: DownloadManagerConfig,
    registry: Mutex<Registry>,
    shutdown_done: watch::Sender<bool>,
    #[cfg(test)]
    finalization_gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    fail_next_cleanup: AtomicBool,
}

/// Shared in-memory job manager. Clones share admission limits, queue, and
/// retained results. A manager must be shut down to request cooperative
/// cancellation of its accepted jobs.
#[derive(Clone)]
pub struct DownloadManager {
    core: Arc<ManagerCore>,
}

impl DownloadManager {
    pub fn new() -> Result<Self, ManagerError> {
        Self::with_config(DownloadManagerConfig::default())
    }

    pub fn with_config(config: DownloadManagerConfig) -> Result<Self, ManagerError> {
        Self::with_engine(DownloadEngine::new()?, config)
    }

    pub fn with_engine(
        engine: DownloadEngine,
        config: DownloadManagerConfig,
    ) -> Result<Self, ManagerError> {
        if config.max_active == 0 || config.max_retained_jobs == 0 {
            return Err(ManagerError::InvalidLimits);
        }
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| ManagerError::RuntimeUnavailable)?;
        let (shutdown_done, _shutdown_rx) = watch::channel(false);
        Ok(Self {
            core: Arc::new(ManagerCore {
                engine,
                runtime,
                config,
                registry: Mutex::new(Registry {
                    jobs: HashMap::new(),
                    queue: VecDeque::new(),
                    destinations: Vec::new(),
                    next_generation: 1,
                    active_slots: 0,
                    closing: false,
                }),
                shutdown_done,
                #[cfg(test)]
                finalization_gate: Mutex::new(None),
                #[cfg(test)]
                fail_next_cleanup: AtomicBool::new(false),
            }),
        })
    }

    #[cfg(test)]
    fn pause_next_finalization(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .core
            .finalization_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((arrived_tx, release_rx));
        (arrived_rx, release_tx)
    }

    #[cfg(test)]
    fn fail_next_cleanup(&self) {
        self.core.fail_next_cleanup.store(true, Ordering::SeqCst);
    }

    /// Admission is synchronous: `Ok` means the request is retained and queued
    /// or dispatched. Preflight filesystem work is done before the registry
    /// critical section; reservation, ID, capacity, and queue insertion are
    /// committed atomically under that section.
    pub fn start(&self, request: DownloadRequest) -> Result<DownloadHandle, StartError> {
        let url = validate_url(&request.url).map_err(StartError::InvalidRequest)?;
        let destination = self
            .core
            .engine
            .canonical_destination(&request.destination)
            .map_err(StartError::InvalidRequest)?;
        let mut registry = lock_registry(&self.core);
        if registry.closing {
            return Err(StartError::ShuttingDown);
        }
        if registry.jobs.contains_key(&request.id) {
            return Err(StartError::DuplicateId);
        }
        if registry.jobs.len() >= self.core.config.max_retained_jobs {
            return Err(StartError::CapacityReached);
        }
        if registry
            .destinations
            .iter()
            .any(|(held, _)| destination_equal(held, &destination))
        {
            return Err(StartError::InvalidRequest(
                DownloadError::DestinationConflict,
            ));
        }

        let generation = registry.next_generation;
        registry.next_generation = registry
            .next_generation
            .checked_add(1)
            .ok_or(StartError::GenerationExhausted)?;
        // A durable resume is metadata only: nothing is verified against the
        // filesystem here so admission stays synchronous. The worker verifies
        // every field before appending a single byte.
        let durable_bytes = request.resume.as_ref().map(|resume| resume.persisted_bytes);
        let initial = ProgressSnapshot {
            id: request.id.clone(),
            state: DownloadState::Queued,
            bytes_written: durable_bytes.unwrap_or(0),
            total_bytes: None,
        };
        let (progress_tx, progress_rx) = watch::channel(initial);
        let (control_tx, _control_rx) = watch::channel(WorkerControl::Running);
        let id = request.id.clone();
        registry
            .destinations
            .push((destination.clone(), id.clone()));
        registry.jobs.insert(
            id.clone(),
            JobRecord {
                generation,
                url,
                destination,
                state: DownloadState::Queued,
                bytes_written: durable_bytes.unwrap_or(0),
                total_bytes: None,
                last_progress_at: Instant::now(),
                progress_tx,
                control_tx,
                cancel_requested: false,
                active_slot_held: false,
                temp_path: None,
                checkpoint: None,
                resume: request.resume,
                promoted: false,
                terminal: None,
                task: None,
            },
        );
        registry.queue.push_back(id.clone());
        dispatch_locked(&self.core, &mut registry);
        drop(registry);
        Ok(DownloadHandle {
            id,
            generation,
            progress: progress_rx,
        })
    }

    pub fn cancel(&self, handle: &DownloadHandle) -> Result<CancelDisposition, ControlError> {
        let mut registry = lock_registry(&self.core);
        let record = registered_record_mut(&mut registry, handle)?;
        if record.state.is_terminal() {
            return Err(ControlError::AlreadyTerminal);
        }
        if record.state == DownloadState::Finalizing {
            return Err(ControlError::TooLate);
        }
        if record.cancel_requested {
            return Ok(CancelDisposition::AlreadyCancelling);
        }

        record.cancel_requested = true;
        record.state = DownloadState::Cancelling;
        record.control_tx.send_replace(WorkerControl::Cancel);
        publish_state(record);
        let queued = registry.queue.iter().position(|id| id == &handle.id);
        if let Some(position) = queued {
            registry.queue.remove(position);
            commit_terminal_locked(
                &mut registry,
                &handle.id,
                handle.generation,
                DownloadState::Cancelled,
                None,
            );
            dispatch_locked(&self.core, &mut registry);
        } else if registry.jobs.get(&handle.id).is_some_and(|record| {
            record.generation == handle.generation
                && record.state == DownloadState::Cancelling
                && !record.active_slot_held
        }) {
            let temp_path = registry
                .jobs
                .get_mut(&handle.id)
                .and_then(|record| record.temp_path.take());
            if temp_path
                .as_deref()
                .is_some_and(|path| cleanup_only(path).is_err())
            {
                return Err(ControlError::CleanupFailed);
            }
            commit_terminal_locked(
                &mut registry,
                &handle.id,
                handle.generation,
                DownloadState::Cancelled,
                None,
            );
            dispatch_locked(&self.core, &mut registry);
        }
        drop(registry);
        Ok(CancelDisposition::Accepted)
    }

    pub fn pause(&self, handle: &DownloadHandle) -> Result<(), ControlError> {
        let mut registry = lock_registry(&self.core);
        let record = registered_record_mut(&mut registry, handle)?;
        match record.state {
            DownloadState::Paused | DownloadState::Pausing => return Ok(()),
            DownloadState::Queued => {
                registry.queue.retain(|id| id != &handle.id);
                let record = registry
                    .jobs
                    .get_mut(&handle.id)
                    .ok_or(ControlError::UnknownId)?;
                record.state = DownloadState::Paused;
                publish_state(record);
            }
            DownloadState::Preparing | DownloadState::Downloading => {
                let record = registry
                    .jobs
                    .get_mut(&handle.id)
                    .ok_or(ControlError::UnknownId)?;
                record.state = DownloadState::Pausing;
                record.control_tx.send_replace(WorkerControl::Pause);
                publish_state(record);
            }
            _ => return Err(ControlError::CannotPause),
        }
        Ok(())
    }

    pub fn resume(&self, handle: &DownloadHandle) -> Result<(), ControlError> {
        let mut registry = lock_registry(&self.core);
        let record = registered_record_mut(&mut registry, handle)?;
        if record.state != DownloadState::Paused {
            return Err(ControlError::NotPaused);
        }
        let record = registry
            .jobs
            .get_mut(&handle.id)
            .ok_or(ControlError::UnknownId)?;
        record.state = DownloadState::Queued;
        record.control_tx.send_replace(WorkerControl::Running);
        publish_state(record);
        registry.queue.push_back(handle.id.clone());
        dispatch_locked(&self.core, &mut registry);
        Ok(())
    }

    pub fn snapshot(&self, handle: &DownloadHandle) -> Result<ProgressSnapshot, ControlError> {
        let registry = lock_registry(&self.core);
        let record = registered_record(&registry, handle)?;
        let snapshot = record.progress_tx.borrow().clone();
        Ok(snapshot)
    }

    pub fn terminal_result(
        &self,
        handle: &DownloadHandle,
    ) -> Result<Option<Arc<TerminalResult>>, ControlError> {
        let registry = lock_registry(&self.core);
        let record = registered_record(&registry, handle)?;
        Ok(record.terminal.clone())
    }

    /// Durable-resume metadata for an in-flight job, for the application
    /// service to persist between sessions. `None` before a temporary file
    /// exists and once the job is terminal (its partial file is gone).
    pub fn resume_info(&self, handle: &DownloadHandle) -> Result<Option<ResumeInfo>, ControlError> {
        let registry = lock_registry(&self.core);
        let record = registered_record(&registry, handle)?;
        if record.terminal.is_some() {
            return Ok(None);
        }
        Ok(record.temp_path.as_ref().map(|temp_path| ResumeInfo {
            temp_path: temp_path.clone(),
            bytes_written: record.bytes_written,
            checkpoint: record.checkpoint.clone(),
        }))
    }

    pub fn acknowledge_terminal(&self, handle: &DownloadHandle) -> Result<(), ControlError> {
        let mut registry = lock_registry(&self.core);
        let record = registered_record(&registry, handle)?;
        if record.terminal.is_none() {
            return Err(ControlError::NotTerminal);
        }
        registry.jobs.remove(&handle.id);
        Ok(())
    }

    /// Close admission, cancel queued and cancellable active jobs, and wait
    /// until all accepted jobs have committed one terminal result. Concurrent
    /// calls share the same monotonic closing transition and are idempotent.
    pub async fn shutdown(&self) -> ShutdownReport {
        {
            let mut registry = lock_registry(&self.core);
            if !registry.closing {
                registry.closing = true;
                while let Some(id) = registry.queue.pop_front() {
                    if let Some(record) = registry.jobs.get_mut(&id) {
                        record.cancel_requested = true;
                        record.control_tx.send_replace(WorkerControl::Cancel);
                    }
                    if let Some(record) = registry.jobs.get_mut(&id) {
                        record.state = DownloadState::Cancelling;
                        publish_state(record);
                    }
                    let generation = registry
                        .jobs
                        .get(&id)
                        .map(|record| record.generation)
                        .unwrap_or_default();
                    commit_terminal_locked(
                        &mut registry,
                        &id,
                        generation,
                        DownloadState::Cancelled,
                        None,
                    );
                }
                for record in registry.jobs.values_mut() {
                    if record.active_slot_held
                        && !record.state.is_terminal()
                        && record.state != DownloadState::Finalizing
                    {
                        record.cancel_requested = true;
                        record.control_tx.send_replace(WorkerControl::Cancel);
                        record.state = DownloadState::Cancelling;
                        publish_state(record);
                    }
                }
                mark_shutdown_done(&self.core, &registry);
            }
        }
        let mut shutdown_rx = self.core.shutdown_done.subscribe();
        loop {
            if *shutdown_rx.borrow() {
                return ShutdownReport::Complete;
            }
            if shutdown_rx.changed().await.is_err() {
                return ShutdownReport::Complete;
            }
        }
    }
}

fn lock_registry(core: &ManagerCore) -> std::sync::MutexGuard<'_, Registry> {
    core.registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn registered_record<'a>(
    registry: &'a Registry,
    handle: &DownloadHandle,
) -> Result<&'a JobRecord, ControlError> {
    let record = registry
        .jobs
        .get(&handle.id)
        .ok_or(ControlError::UnknownId)?;
    if record.generation != handle.generation {
        return Err(ControlError::StaleHandle);
    }
    Ok(record)
}

fn registered_record_mut<'a>(
    registry: &'a mut Registry,
    handle: &DownloadHandle,
) -> Result<&'a mut JobRecord, ControlError> {
    let record = registry
        .jobs
        .get_mut(&handle.id)
        .ok_or(ControlError::UnknownId)?;
    if record.generation != handle.generation {
        return Err(ControlError::StaleHandle);
    }
    Ok(record)
}

fn publish_state(record: &mut JobRecord) {
    let id = record.progress_tx.borrow().id.clone();
    record.progress_tx.send_replace(ProgressSnapshot {
        id,
        state: record.state,
        bytes_written: record.bytes_written,
        total_bytes: record.total_bytes,
    });
}

fn dispatch_locked(core: &Arc<ManagerCore>, registry: &mut Registry) {
    while !registry.closing && registry.active_slots < core.config.max_active {
        let Some(id) = registry.queue.pop_front() else {
            break;
        };
        let Some(record) = registry.jobs.get_mut(&id) else {
            continue;
        };
        if record.state.is_terminal() || record.cancel_requested {
            continue;
        }
        record.state = DownloadState::Preparing;
        record.active_slot_held = true;
        publish_state(record);
        registry.active_slots += 1;

        let token = JobToken {
            id: id.clone(),
            generation: record.generation,
        };
        // Consume the durable resume exactly once: after the first dispatch
        // the adopted state lives in `temp_path`/`checkpoint` like any other
        // in-flight job, so a later pause/resume cycle cannot re-adopt a
        // stale persisted path.
        let durable = record.resume.take();
        if let Some(resume) = &durable {
            record.checkpoint = resume.checkpoint.clone();
        }
        let url = record.url.clone();
        let destination = record.destination.clone();
        let temp_path = record.temp_path.clone();
        let checkpoint = record.checkpoint.clone();
        let bytes_written = record.bytes_written;
        let control_rx = record.control_tx.subscribe();
        let core_for_task = Arc::clone(core);
        let core_for_recovery = Arc::clone(core);
        let recovery_token = JobToken {
            id: token.id.clone(),
            generation: token.generation,
        };
        let task = core.runtime.spawn(async move {
            let mut guarded = CatchUnwind(Box::pin(supervise_job(
                core_for_task,
                token,
                url,
                destination,
                temp_path,
                checkpoint,
                durable,
                bytes_written,
                control_rx,
            )));
            let result = (&mut guarded).await;
            drop(guarded);
            if result.is_err() {
                recover_panicked_job(&core_for_recovery, &recovery_token);
            }
        });
        if let Some(record) = registry.jobs.get_mut(&id) {
            record.task = Some(task);
        }
    }
}

enum WorkerOutput {
    Ready {
        temp_path: PathBuf,
        guard: TempGuard,
        bytes_written: u64,
    },
    Failed {
        error: DownloadError,
        temp: Option<(PathBuf, TempGuard)>,
    },
    Cancelled {
        temp: Option<(PathBuf, TempGuard)>,
    },
    Paused {
        temp_path: PathBuf,
        guard: TempGuard,
        bytes_written: u64,
        checkpoint: Option<ResumeCheckpoint>,
    },
}

// These parameters describe one worker invocation and are intentionally kept
// explicit at this orchestration boundary.
#[allow(clippy::too_many_arguments)]
async fn prepare_managed_transfer(
    core: Arc<ManagerCore>,
    token: JobToken,
    engine: DownloadEngine,
    url: Url,
    destination: PathBuf,
    existing_temp: Option<PathBuf>,
    checkpoint: Option<ResumeCheckpoint>,
    durable: Option<DurableResume>,
    existing_bytes: u64,
    mut control_rx: watch::Receiver<WorkerControl>,
) -> WorkerOutput {
    // A durable resume names the persisted partial file from a previous
    // session; same-session continuation uses the in-memory temp path. The
    // worker adopts a persisted file only after verifying it on disk, and
    // never trusts the persisted byte counter over the file's real length.
    let (requested_temp, expected_len, checkpoint) = match durable {
        Some(resume) => (
            Some(resume.temp_path),
            resume.persisted_bytes,
            resume.checkpoint.or(checkpoint),
        ),
        None => (existing_temp, existing_bytes, checkpoint),
    };
    let opened = match open_or_create_temp(&destination, requested_temp, expected_len).await {
        Ok(created) => created,
        Err(error) => return WorkerOutput::Failed { error, temp: None },
    };
    let (temp_path, mut guard, file, can_resume, adopted_len) = opened;
    {
        let mut registry = lock_registry(&core);
        if let Some(record) = registry.jobs.get_mut(&token.id) {
            if record.generation == token.generation {
                record.temp_path = Some(temp_path.clone());
            }
        }
    }
    #[cfg(test)]
    if engine.panic_next_worker.swap(false, Ordering::SeqCst) {
        // Leave cleanup to the supervisor to exercise that recovery path.
        guard.disarm();
        panic!("injected download worker panic");
    }
    let file = file;
    if *control_rx.borrow() == WorkerControl::Cancel {
        drop(file);
        return WorkerOutput::Cancelled {
            temp: Some((temp_path, guard)),
        };
    }

    let mut file = file;
    // A partial file that already reaches (or exceeds) the recorded total
    // cannot be verified by a range request, so its content is never trusted;
    // drop the checkpoint and restart from byte zero.
    let resume_checkpoint =
        checkpoint.filter(|checkpoint| can_resume && checkpoint.total_bytes > adopted_len);
    let initial_bytes = if resume_checkpoint.is_some() {
        adopted_len
    } else {
        0
    };
    if initial_bytes == 0 {
        if let Err(source) = file.set_len(0).await {
            drop(file);
            return WorkerOutput::Failed {
                error: DownloadError::Filesystem {
                    operation: "reset temporary file",
                    path: temp_path.clone(),
                    source,
                },
                temp: Some((temp_path, guard)),
            };
        }
    }
    let progress_core = Arc::clone(&core);
    let progress_token = JobToken {
        id: token.id.clone(),
        generation: token.generation,
    };
    let result = engine
        .transfer(
            &url,
            &temp_path,
            &mut file,
            Some(&mut control_rx),
            resume_checkpoint.clone(),
            initial_bytes,
            |event| match event {
                TransferEvent::Downloading { checkpoint } => {
                    set_downloading(&progress_core, &progress_token, checkpoint);
                }
                TransferEvent::BytesWritten {
                    bytes_written,
                    total_bytes,
                } => update_progress(&progress_core, &progress_token, bytes_written, total_bytes),
            },
        )
        .await;

    let result = match (result, resume_checkpoint.is_some()) {
        (Err(_), true) => {
            if let Err(source) = file.set_len(0).await {
                Err(DownloadError::Filesystem {
                    operation: "restart temporary file",
                    path: temp_path.clone(),
                    source,
                })
            } else {
                update_progress(&progress_core, &progress_token, 0, None);
                engine
                    .transfer(
                        &url,
                        &temp_path,
                        &mut file,
                        Some(&mut control_rx),
                        None,
                        0,
                        |event| match event {
                            TransferEvent::Downloading { checkpoint } => {
                                set_downloading(&progress_core, &progress_token, checkpoint)
                            }
                            TransferEvent::BytesWritten {
                                bytes_written,
                                total_bytes,
                            } => update_progress(
                                &progress_core,
                                &progress_token,
                                bytes_written,
                                total_bytes,
                            ),
                        },
                    )
                    .await
            }
        }
        (result, _) => result,
    };

    match result {
        Ok(TransferExit::Paused {
            bytes_written,
            checkpoint,
        }) => {
            if let Err(source) = file.flush().await {
                drop(file);
                return WorkerOutput::Failed {
                    error: DownloadError::Filesystem {
                        operation: "flush paused temporary file",
                        path: temp_path.clone(),
                        source,
                    },
                    temp: Some((temp_path, guard)),
                };
            }
            if let Err(source) = file.sync_all().await {
                drop(file);
                return WorkerOutput::Failed {
                    error: DownloadError::Filesystem {
                        operation: "sync paused temporary file",
                        path: temp_path.clone(),
                        source,
                    },
                    temp: Some((temp_path, guard)),
                };
            }
            drop(file);
            guard.disarm();
            WorkerOutput::Paused {
                temp_path,
                guard,
                bytes_written,
                checkpoint,
            }
        }
        Ok(TransferExit::Cancelled) => {
            drop(file);
            WorkerOutput::Cancelled {
                temp: Some((temp_path, guard)),
            }
        }
        Err(error) => {
            drop(file);
            WorkerOutput::Failed {
                error,
                temp: Some((temp_path, guard)),
            }
        }
        Ok(TransferExit::Complete(bytes_written)) => {
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
            drop(file);
            match finalization {
                Ok(()) => WorkerOutput::Ready {
                    temp_path,
                    guard,
                    bytes_written,
                },
                Err(error) => WorkerOutput::Failed {
                    error,
                    temp: Some((temp_path, guard)),
                },
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise_job(
    core: Arc<ManagerCore>,
    token: JobToken,
    url: Url,
    destination: PathBuf,
    temp_path: Option<PathBuf>,
    checkpoint: Option<ResumeCheckpoint>,
    durable: Option<DurableResume>,
    bytes_written: u64,
    control_rx: watch::Receiver<WorkerControl>,
) {
    let output = prepare_managed_transfer(
        Arc::clone(&core),
        JobToken {
            id: token.id.clone(),
            generation: token.generation,
        },
        core.engine.clone(),
        url,
        destination.clone(),
        temp_path,
        checkpoint,
        durable,
        bytes_written,
        control_rx,
    )
    .await;

    match output {
        WorkerOutput::Failed { error, temp } => {
            let error = match temp {
                Some((path, mut guard)) => {
                    let error = cleanup_error_or_original(&core, &path, error);
                    guard.disarm();
                    error
                }
                None => error,
            };
            commit_terminal(&core, &token, DownloadState::Failed, Some(Arc::new(error)));
        }
        WorkerOutput::Cancelled { temp } => {
            let cleanup_error = temp.and_then(|(path, mut guard)| {
                let result = cleanup_for_manager(&core, &path);
                guard.disarm();
                result.err().map(|source| DownloadError::CleanupFailure {
                    path,
                    source,
                    original: Box::new(DownloadError::Cancelled),
                })
            });
            if let Some(error) = cleanup_error {
                commit_terminal(&core, &token, DownloadState::Failed, Some(Arc::new(error)));
            } else {
                commit_terminal(&core, &token, DownloadState::Cancelled, None);
            }
        }
        WorkerOutput::Paused {
            temp_path,
            mut guard,
            bytes_written,
            checkpoint,
        } => {
            let mut registry = lock_registry(&core);
            let Some(record) = registry.jobs.get_mut(&token.id) else {
                guard.disarm();
                let _ = cleanup_only(&temp_path);
                return;
            };
            if record.generation != token.generation {
                guard.disarm();
                let _ = cleanup_only(&temp_path);
                return;
            }
            if record.cancel_requested {
                record.temp_path = None;
                guard.disarm();
                drop(registry);
                let cleanup_error = cleanup_for_manager(&core, &temp_path).err().map(|source| {
                    DownloadError::CleanupFailure {
                        path: temp_path.clone(),
                        source,
                        original: Box::new(DownloadError::Cancelled),
                    }
                });
                if let Some(error) = cleanup_error {
                    commit_terminal(&core, &token, DownloadState::Failed, Some(Arc::new(error)));
                } else {
                    commit_terminal(&core, &token, DownloadState::Cancelled, None);
                }
                return;
            }
            record.state = DownloadState::Paused;
            record.bytes_written = bytes_written;
            record.checkpoint = checkpoint;
            record.temp_path = Some(temp_path);
            record.task.take();
            let held_slot = record.active_slot_held;
            record.active_slot_held = false;
            publish_state(record);
            if held_slot {
                registry.active_slots = registry.active_slots.saturating_sub(1);
            }
            guard.disarm();
            dispatch_locked(&core, &mut registry);
            mark_shutdown_done(&core, &registry);
        }
        WorkerOutput::Ready {
            temp_path,
            mut guard,
            bytes_written,
        } => {
            let cancelled = {
                let mut registry = lock_registry(&core);
                let Some(record) = registry.jobs.get_mut(&token.id) else {
                    guard.disarm();
                    let _ = cleanup_only(&temp_path);
                    return;
                };
                if record.generation != token.generation {
                    guard.disarm();
                    let _ = cleanup_only(&temp_path);
                    return;
                }
                if record.cancel_requested {
                    record.state = DownloadState::Cancelling;
                    publish_state(record);
                    true
                } else {
                    record.state = DownloadState::Finalizing;
                    record.bytes_written = bytes_written;
                    publish_state(record);
                    false
                }
            };
            if cancelled {
                let error = cleanup_for_manager(&core, &temp_path).err().map(|source| {
                    DownloadError::CleanupFailure {
                        path: temp_path.clone(),
                        source,
                        original: Box::new(DownloadError::Cancelled),
                    }
                });
                guard.disarm();
                if let Some(error) = error {
                    commit_terminal(&core, &token, DownloadState::Failed, Some(Arc::new(error)));
                } else {
                    commit_terminal(&core, &token, DownloadState::Cancelled, None);
                }
                return;
            }

            #[cfg(test)]
            pause_before_promotion(&core).await;

            match promote_no_replace(&temp_path, &destination) {
                Ok(()) => {
                    guard.disarm();
                    if let Some(record) = lock_registry(&core).jobs.get_mut(&token.id) {
                        if record.generation == token.generation {
                            record.promoted = true;
                        }
                    }
                    commit_terminal(&core, &token, DownloadState::Completed, None);
                }
                Err(error) => {
                    let error = cleanup_error_or_original(&core, &temp_path, error);
                    guard.disarm();
                    commit_terminal(&core, &token, DownloadState::Failed, Some(Arc::new(error)));
                }
            }
        }
    }
}

fn recover_panicked_job(core: &Arc<ManagerCore>, token: &JobToken) {
    let (path, promoted) = {
        let registry = lock_registry(core);
        let Some(record) = registry.jobs.get(&token.id) else {
            return;
        };
        if record.generation != token.generation || record.terminal.is_some() {
            return;
        }
        (record.temp_path.clone(), record.promoted)
    };
    if promoted {
        commit_terminal(core, token, DownloadState::Completed, None);
        return;
    }
    let error = match path {
        Some(path) => cleanup_error_or_original(core, &path, DownloadError::WorkerPanicked),
        None => DownloadError::WorkerPanicked,
    };
    commit_terminal(core, token, DownloadState::Failed, Some(Arc::new(error)));
}

#[cfg(test)]
async fn pause_before_promotion(core: &Arc<ManagerCore>) {
    let gate = core
        .finalization_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((arrived, release)) = gate {
        let _ = arrived.send(());
        let _ = release.await;
    }
}

fn cleanup_only(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn cleanup_for_manager(core: &ManagerCore, path: &Path) -> Result<(), io::Error> {
    #[cfg(test)]
    if core.fail_next_cleanup.swap(false, Ordering::SeqCst) && path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "injected temporary-file cleanup failure",
        ));
    }
    let _ = core;
    cleanup_only(path)
}

fn cleanup_error_or_original(
    core: &ManagerCore,
    path: &Path,
    original: DownloadError,
) -> DownloadError {
    match cleanup_for_manager(core, path) {
        Ok(()) => original,
        Err(source) => DownloadError::CleanupFailure {
            path: path.to_path_buf(),
            source,
            original: Box::new(original),
        },
    }
}

fn commit_terminal(
    core: &Arc<ManagerCore>,
    token: &JobToken,
    state: DownloadState,
    error: Option<Arc<DownloadError>>,
) {
    let mut registry = lock_registry(core);
    commit_terminal_locked(&mut registry, &token.id, token.generation, state, error);
    dispatch_locked(core, &mut registry);
    mark_shutdown_done(core, &registry);
    drop(registry);
}

fn commit_terminal_locked(
    registry: &mut Registry,
    id: &DownloadId,
    generation: u64,
    state: DownloadState,
    error: Option<Arc<DownloadError>>,
) {
    let Some(record) = registry.jobs.get_mut(id) else {
        return;
    };
    if record.generation != generation || record.terminal.is_some() {
        return;
    }
    let result = Arc::new(TerminalResult {
        id: id.clone(),
        state,
        destination: record.destination.clone(),
        bytes_written: record.bytes_written,
        error,
    });
    record.state = state;
    record.terminal = Some(result);
    publish_state(record);
    let reservation = record.destination.clone();
    if record.active_slot_held {
        record.active_slot_held = false;
        registry.active_slots = registry.active_slots.saturating_sub(1);
    }
    record.task.take();
    registry
        .destinations
        .retain(|(held, held_id)| held_id != id || !destination_equal(held, &reservation));
}

fn mark_shutdown_done(core: &ManagerCore, registry: &Registry) {
    if registry.closing && registry.active_slots == 0 && registry.queue.is_empty() {
        core.shutdown_done.send_replace(true);
    }
}

fn set_downloading(
    core: &Arc<ManagerCore>,
    token: &JobToken,
    checkpoint: Option<ResumeCheckpoint>,
) {
    let registry = lock_registry(core);
    if let Some(record) = registry.jobs.get(&token.id) {
        if record.generation != token.generation || record.state.is_terminal() {
            return;
        }
    } else {
        return;
    }
    let mut registry = registry;
    if let Some(record) = registry.jobs.get_mut(&token.id) {
        record.state = if record.cancel_requested {
            DownloadState::Cancelling
        } else {
            DownloadState::Downloading
        };
        record.total_bytes = checkpoint.as_ref().map(|saved| saved.total_bytes);
        // Publish the validator before any body byte is written so a crash
        // right after the response headers still leaves a usable checkpoint.
        record.checkpoint = checkpoint;
        publish_state(record);
    }
}

fn update_progress(
    core: &Arc<ManagerCore>,
    token: &JobToken,
    bytes_written: u64,
    total_bytes: Option<u64>,
) {
    let mut registry = lock_registry(core);
    if let Some(record) = registry.jobs.get_mut(&token.id) {
        if record.generation != token.generation || record.state.is_terminal() {
            return;
        }
        record.bytes_written = bytes_written;
        record.total_bytes = total_bytes;
        let now = Instant::now();
        if now.duration_since(record.last_progress_at) >= PROGRESS_INTERVAL {
            record.last_progress_at = now;
            publish_state(record);
        }
    }
}

fn destination_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "Kernel32")]
        extern "system" {
            fn CompareStringOrdinal(
                string1: *const u16,
                count1: i32,
                string2: *const u16,
                count2: i32,
                ignore_case: i32,
            ) -> i32;
        }
        let left: Vec<u16> = left.as_os_str().encode_wide().collect();
        let right: Vec<u16> = right.as_os_str().encode_wide().collect();
        if left.len() > i32::MAX as usize || right.len() > i32::MAX as usize {
            return true;
        }
        // CompareStringOrdinal returns CSTR_EQUAL (2) for an ordinal,
        // case-insensitive match. This is conservative for case-sensitive dirs.
        let comparison = unsafe {
            CompareStringOrdinal(
                left.as_ptr(),
                left.len() as i32,
                right.as_ptr(),
                right.len() as i32,
                1,
            )
        };
        comparison == 2 || comparison == 0
    }
    #[cfg(not(windows))]
    {
        left == right
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

#[cfg(test)]
struct Reservation {
    reservations: Arc<Mutex<HashSet<PathBuf>>>,
    target: PathBuf,
}

#[cfg(test)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRange {
    start: u64,
    end: u64,
    total: u64,
}

fn parse_content_range(
    headers: &reqwest::header::HeaderMap,
) -> Result<ContentRange, DownloadError> {
    let values: Vec<_> = headers
        .get_all(reqwest::header::CONTENT_RANGE)
        .iter()
        .collect();
    let [value] = values.as_slice() else {
        return Err(DownloadError::InvalidResponse);
    };
    let value = value
        .to_str()
        .map_err(|_| DownloadError::InvalidResponse)?
        .trim();
    let (unit, range) = value
        .split_once(' ')
        .ok_or(DownloadError::InvalidResponse)?;
    if !unit.eq_ignore_ascii_case("bytes") || range.is_empty() {
        return Err(DownloadError::InvalidResponse);
    }
    let (span, total) = range
        .split_once('/')
        .ok_or(DownloadError::InvalidResponse)?;
    if total == "*" {
        return Err(DownloadError::InvalidResponse);
    }
    let (start, end) = span.split_once('-').ok_or(DownloadError::InvalidResponse)?;
    let parse = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(DownloadError::InvalidResponse);
        }
        value
            .parse::<u64>()
            .map_err(|_| DownloadError::InvalidResponse)
    };
    let result = ContentRange {
        start: parse(start)?,
        end: parse(end)?,
        total: parse(total)?,
    };
    if result.start > result.end || result.end >= result.total {
        return Err(DownloadError::InvalidResponse);
    }
    Ok(result)
}

/// Resume only with one syntactically strong entity tag. Weak validators are
/// useful for cache validation but cannot safely identify byte-for-byte ranges.
fn parse_strong_etag(
    headers: &reqwest::header::HeaderMap,
) -> Result<Option<reqwest::header::HeaderValue>, DownloadError> {
    let values: Vec<_> = headers.get_all(reqwest::header::ETAG).iter().collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => {
            let Ok(text) = value.to_str() else {
                return Ok(None);
            };
            let text = text.trim();

            if text.starts_with("W/") {
                return Ok(None);
            }
            let bytes = text.as_bytes();
            if bytes.len() < 2
                || bytes.first() != Some(&b'"')
                || bytes.last() != Some(&b'"')
                || bytes[1..bytes.len() - 1].contains(&b'"')
                || bytes.iter().any(|byte| *byte < 0x21 || *byte == 0x7f)
            {
                return Ok(None);
            }
            Ok(Some((*value).clone()))
        }
        _ => Ok(None),
    }
}

fn validate_entity_headers(headers: &reqwest::header::HeaderMap) -> Result<(), DownloadError> {
    if headers.contains_key(reqwest::header::CONTENT_LENGTH)
        && headers.contains_key(reqwest::header::TRANSFER_ENCODING)
    {
        // RFC 9112 says this framing ambiguity ought to be treated as an error.
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
    validate_transfer_encoding(headers)
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

/// Outcome of trying to adopt an existing partial file under this process.
enum AdoptError {
    /// The file vanished or is owned elsewhere (rename failed). Leave it
    /// alone and continue with a fresh temporary file.
    Unavailable,
}

/// Continues into the caller-known temporary file when it still validates, or
/// creates a fresh exclusive temporary file otherwise.
///
/// `expected_len` is the byte count recorded when the partial file was last
/// persisted. An adoptable file must be a regular file in the destination's
/// directory of at least that length: it may be longer because progress can
/// outpace the persisted snapshot, but never shorter. Adoption also renames
/// the file into a process-owned name, so two workers can never own the same
/// partial file. Returns the adopted byte length (0 for a fresh file).
async fn open_or_create_temp(
    destination: &Path,
    existing: Option<PathBuf>,
    expected_len: u64,
) -> Result<(PathBuf, TempGuard, tokio::fs::File, bool, u64), DownloadError> {
    if let Some(path) = existing {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let same_parent = path.parent().is_some_and(|candidate| candidate == parent);
        if same_parent {
            let metadata = tokio::fs::symlink_metadata(&path).await.ok();
            // `symlink_metadata` never follows a link, so a symlinked or
            // directory entry is rejected by the `is_file` check.
            let adoptable = metadata.as_ref().is_some_and(|metadata| {
                metadata.file_type().is_file() && metadata.len() >= expected_len
            });
            if adoptable {
                match adopt_existing_temp(parent, destination, &path).await {
                    Ok((adopted_path, adopted_len)) => {
                        return match tokio::fs::OpenOptions::new()
                            .write(true)
                            .append(true)
                            .open(&adopted_path)
                            .await
                        {
                            Ok(file) => Ok((
                                adopted_path.clone(),
                                TempGuard::new(adopted_path),
                                file,
                                true,
                                adopted_len,
                            )),
                            Err(source) => Err(DownloadError::Filesystem {
                                operation: "reopen temporary file",
                                path: adopted_path,
                                source,
                            }),
                        };
                    }
                    Err(AdoptError::Unavailable) => {}
                }
            } else if metadata.is_some() {
                // The partial file shrank, changed kind, or is not a regular
                // file: it can no longer be trusted, so remove it and start
                // from byte zero.
                let _ = cleanup_only(&path);
            }
        }
    }
    let (path, guard, file) = create_temp(destination).await?;
    Ok((path, guard, file, false, 0))
}

/// Renames a verified partial file into a fresh process-owned temporary name.
/// The rename is the ownership handoff: it succeeds for exactly one worker,
/// and a lost race leaves the winner's file untouched.
async fn adopt_existing_temp(
    parent: &Path,
    destination: &Path,
    path: &Path,
) -> Result<(PathBuf, u64), AdoptError> {
    let length = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata.len(),
        Err(_) => return Err(AdoptError::Unavailable),
    };
    for _ in 0..128 {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let Some(candidate) = temp_candidate(parent, destination, std::process::id(), id) else {
            continue;
        };
        match tokio::fs::rename(path, &candidate).await {
            Ok(()) => return Ok((candidate, length)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(AdoptError::Unavailable)
            }
            Err(_) => return Err(AdoptError::Unavailable),
        }
    }
    Err(AdoptError::Unavailable)
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

#[cfg(test)]
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
    // The same-directory temporary file makes this a same-volume move.
    // MoveFileW fails when the destination name already exists; Windows tests
    // exercise promotion and preservation of an existing target.
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
