//! A UI-independent, managed single-stream HTTP/HTTPS download core.
//!
//! Each transfer streams into an exclusively-created sibling temporary file.
//! A successful response is flushed and length-checked before a no-clobber
//! promotion. Managed jobs use FIFO scheduling, cooperative cancellation,
//! coalesced progress snapshots, and retained terminal results. Each accepted
//! job runs one attempt; persistence and retries are deferred.

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
    Cancelling,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadState {
    fn is_terminal(self) -> bool {
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

enum TransferExit {
    Complete(u64),
    Cancelled,
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
        total_bytes: Option<u64>,
    },
    BytesWritten {
        bytes_written: u64,
        total_bytes: Option<u64>,
    },
}

async fn wait_for_cancel(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn cancellable<F: std::future::Future>(
    receiver: &mut Option<&mut watch::Receiver<bool>>,
    future: F,
) -> Option<F::Output> {
    if let Some(receiver) = receiver.as_deref_mut() {
        tokio::select! {
            biased;
            _ = wait_for_cancel(receiver) => None,
            output = future => Some(output),
        }
    } else {
        Some(future.await)
    }
}

/// Reusable HTTP client and in-process destination reservation set.
#[derive(Clone)]
pub struct DownloadEngine {
    client: reqwest::Client,
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

        Ok(Self {
            client,
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
            .transfer(&url, &temp_path, &mut file, None, |_| {})
            .await;

        let bytes_written = match attempt {
            Ok(TransferExit::Complete(bytes)) => bytes,
            Ok(TransferExit::Cancelled) => {
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

    async fn transfer<F>(
        &self,
        url: &Url,
        temp_path: &Path,
        file: &mut tokio::fs::File,
        mut cancellation: Option<&mut watch::Receiver<bool>>,
        mut progress: F,
    ) -> Result<TransferExit, DownloadError>
    where
        F: FnMut(TransferEvent),
    {
        // The wait for response headers is bounded by the same inactivity limit
        // as body reads; otherwise a silent server would only be cut off by the
        // 24 h job deadline while holding the destination reservation.
        let response_result = cancellable(
            &mut cancellation,
            timeout(
                self.read_inactivity,
                self.client
                    .get(url.clone())
                    .header(reqwest::header::ACCEPT_ENCODING, "identity")
                    .send(),
            ),
        )
        .await;
        let response_result = match response_result {
            None => return Ok(TransferExit::Cancelled),
            Some(result) => result,
        };
        let response = response_result
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
        progress(TransferEvent::Downloading {
            total_bytes: expected,
        });
        let mut written = 0_u64;
        let mut response = response;

        loop {
            if cancellation
                .as_ref()
                .is_some_and(|receiver| *receiver.borrow())
            {
                return Ok(TransferExit::Cancelled);
            }
            let next = cancellable(
                &mut cancellation,
                timeout(self.read_inactivity, response.chunk()),
            )
            .await;
            let next = match next {
                None => return Ok(TransferExit::Cancelled),
                Some(Err(_)) => return Err(DownloadError::BodyInactivityTimeout),
                Some(Ok(next)) => next,
            };
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
            let after_chunk = written
                .checked_add(chunk_len)
                .ok_or(DownloadError::ByteCountOverflow)?;
            if let Some(expected) = expected {
                if after_chunk > expected {
                    return Err(DownloadError::IncompleteResponse {
                        expected,
                        received: written,
                    });
                }
            }

            write_chunk_accounted(file, temp_path, &chunk, &mut written, |count| {
                progress(TransferEvent::BytesWritten {
                    bytes_written: count,
                    total_bytes: expected,
                });
            })
            .await?;
        }

        if let Some(expected) = expected {
            if written != expected {
                return Err(DownloadError::IncompleteResponse {
                    expected,
                    received: written,
                });
            }
        }
        Ok(TransferExit::Complete(written))
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
    cancel_tx: watch::Sender<bool>,
    cancel_requested: bool,
    active_slot_held: bool,
    temp_path: Option<PathBuf>,
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
        let initial = ProgressSnapshot {
            id: request.id.clone(),
            state: DownloadState::Queued,
            bytes_written: 0,
            total_bytes: None,
        };
        let (progress_tx, progress_rx) = watch::channel(initial);
        let (cancel_tx, _cancel_rx) = watch::channel(false);
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
                bytes_written: 0,
                total_bytes: None,
                last_progress_at: Instant::now(),
                progress_tx,
                cancel_tx,
                cancel_requested: false,
                active_slot_held: false,
                temp_path: None,
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
        record.cancel_tx.send_replace(true);
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
        }
        drop(registry);
        Ok(CancelDisposition::Accepted)
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
                        record.cancel_tx.send_replace(true);
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
                        record.cancel_tx.send_replace(true);
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
        let url = record.url.clone();
        let destination = record.destination.clone();
        let cancel_rx = record.cancel_tx.subscribe();
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
                cancel_rx,
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
}

async fn prepare_managed_transfer(
    core: Arc<ManagerCore>,
    token: JobToken,
    engine: DownloadEngine,
    url: Url,
    destination: PathBuf,
    mut cancel_rx: watch::Receiver<bool>,
) -> WorkerOutput {
    #[allow(unused_mut)]
    let (temp_path, mut guard, file) = match create_temp(&destination).await {
        Ok(created) => created,
        Err(error) => {
            return WorkerOutput::Failed { error, temp: None };
        }
    };
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
    let mut file = file;
    if *cancel_rx.borrow() {
        drop(file);
        return WorkerOutput::Cancelled {
            temp: Some((temp_path, guard)),
        };
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
            Some(&mut cancel_rx),
            |event| match event {
                TransferEvent::Downloading { total_bytes } => {
                    set_downloading(&progress_core, &progress_token, total_bytes);
                }
                TransferEvent::BytesWritten {
                    bytes_written,
                    total_bytes,
                } => update_progress(&progress_core, &progress_token, bytes_written, total_bytes),
            },
        )
        .await;

    match result {
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

async fn supervise_job(
    core: Arc<ManagerCore>,
    token: JobToken,
    url: Url,
    destination: PathBuf,
    cancel_rx: watch::Receiver<bool>,
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
        cancel_rx,
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

fn set_downloading(core: &Arc<ManagerCore>, token: &JobToken, total_bytes: Option<u64>) {
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
        record.total_bytes = total_bytes;
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
