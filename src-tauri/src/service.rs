use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use super::persistence;

use consolex_download_engine::{
    CancelDisposition, ControlError, DownloadError, DownloadHandle, DownloadId, DownloadManager,
    DownloadRequest, DownloadState, DurableResume, NetworkFailureKind, ResumeCheckpoint,
    StartError,
};
use serde::{Deserialize, Serialize};
use url::Url;

/// Automatic retries per accepted download for transient network failures
/// only. Bounded so a dead network cannot loop forever; manual retry remains
/// available afterwards.
const MAX_AUTO_RETRIES: u8 = 2;
pub(crate) const RETRY_BACKOFF_BASE: Duration = Duration::from_secs(2);
const INTERRUPTED_CHECKPOINT_MESSAGE: &str = "Velo closed before this transfer finished. Retry continues from the last verified checkpoint when the server allows it.";
const INTERRUPTED_BYTE_ZERO_MESSAGE: &str =
    "Velo closed before this transfer finished. Retry starts from byte zero.";

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) manager: DownloadManager,
    pub(crate) jobs: Arc<Mutex<HashMap<String, UiJob>>>,
    next_id: Arc<AtomicU64>,
    pub(crate) shutdown_started: Arc<AtomicBool>,
    pub(crate) exit_ready: Arc<AtomicBool>,
    store_path: PathBuf,
    store_write_lock: Arc<Mutex<()>>,
    last_store_write: Arc<Mutex<Instant>>,
    pub(crate) history_notice: Arc<Mutex<Option<String>>>,
    /// Single fan-out point for UI events so commands, monitors, and retry
    /// timers publish through the same channel.
    pub(crate) publish: Arc<dyn Fn(UiEvent) + Send + Sync>,
    /// Base delay for the first automatic retry; each attempt waits four
    /// times longer than the previous one. Injected for deterministic tests.
    retry_backoff_base: Duration,
}

/// Everything the webview is told about job lifecycle changes.
pub(crate) enum UiEvent {
    Updated(DownloadDto),
    Removed { id: String, generation: u64 },
}

pub(crate) struct UiJob {
    pub(crate) handle: Option<DownloadHandle>,
    pub(crate) url: Option<String>,
    pub(crate) destination: String,
    pub(crate) encrypted_url: Vec<u8>,
    pub(crate) download: DownloadDto,
    /// Verified durable-resume metadata for the current partial file. Kept in
    /// step with the engine so an abrupt exit never loses more than one
    /// persisted snapshot of progress.
    pub(crate) resume: Option<persistence::StoredResume>,
    pub(crate) auto_retries: u8,
    pub(crate) retry_timer: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadDto {
    pub(crate) id: String,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) order: u64,
    pub(crate) file_name: String,
    /// The absolute destination the transfer was started with, shown in the
    /// UI. `default` keeps older snapshots (which stored the destination
    /// separately) loading without change.
    #[serde(default)]
    pub(crate) destination: String,
    pub(crate) state: UiDownloadState,
    pub(crate) bytes_written: u64,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) speed_bytes_per_second: Option<u64>,
    pub(crate) eta_seconds: Option<u64>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UiDownloadState {
    Queued,
    Preparing,
    Downloading,
    Pausing,
    Paused,
    Interrupted,
    Cancelling,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
    /// A bounded automatic retry is scheduled. Not a terminal state: the
    /// download is still owned by Velo and must not be manually retried.
    Retrying,
}

impl From<DownloadState> for UiDownloadState {
    fn from(value: DownloadState) -> Self {
        match value {
            DownloadState::Queued => Self::Queued,
            DownloadState::Preparing => Self::Preparing,
            DownloadState::Downloading => Self::Downloading,
            DownloadState::Pausing => Self::Pausing,
            DownloadState::Paused => Self::Paused,
            DownloadState::Cancelling => Self::Cancelling,
            DownloadState::Finalizing => Self::Finalizing,
            DownloadState::Completed => Self::Completed,
            DownloadState::Failed => Self::Failed,
            DownloadState::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl CommandError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl AppState {
    #[cfg(test)]
    fn with_test_hooks(
        manager: DownloadManager,
        store_path: PathBuf,
        publish: Arc<dyn Fn(UiEvent) + Send + Sync>,
        retry_backoff_base: Duration,
    ) -> Self {
        Self::build(manager, store_path, publish, retry_backoff_base)
    }

    pub(crate) fn build(
        manager: DownloadManager,
        store_path: PathBuf,
        publish: Arc<dyn Fn(UiEvent) + Send + Sync>,
        retry_backoff_base: Duration,
    ) -> Self {
        let mut jobs = HashMap::new();
        let mut next_order = 1_u64;
        let notice: Option<String> = match persistence::load(&store_path) {
            Ok(loaded) => {
                let load_notice = loaded.notice;
                for record in loaded.records {
                    let mut download = record.download;
                    next_order = next_order.max(download.order.saturating_add(1));
                    // Records written before the DTO carried the destination
                    // (or any record missing it) still know it on the store.
                    if download.destination.is_empty() {
                        download.destination = record.destination.clone();
                    }
                    let url = restore_url(&record.encrypted_url).ok();
                    if matches!(download.state, UiDownloadState::Completed)
                        && fs::metadata(&record.destination)
                            .map(|metadata| metadata.len() != download.bytes_written)
                            .unwrap_or(true)
                    {
                        download.state = UiDownloadState::Failed;
                        download.error =
                            Some("The saved file is missing or its size changed.".into());
                    } else if !matches!(
                        download.state,
                        UiDownloadState::Completed
                            | UiDownloadState::Failed
                            | UiDownloadState::Cancelled
                            | UiDownloadState::Interrupted
                    ) {
                        // Anything still in flight when the process died —
                        // including a scheduled retry — becomes Interrupted.
                        download.state = UiDownloadState::Interrupted;
                        download.speed_bytes_per_second = None;
                        download.eta_seconds = None;
                        download.error = Some(INTERRUPTED_BYTE_ZERO_MESSAGE.into());
                    }
                    if url.is_none() && !matches!(download.state, UiDownloadState::Completed) {
                        download.error = Some(
                            "This saved URL cannot be decrypted on this Windows account.".into(),
                        );
                    }
                    // Durable resume only applies to retryable interrupted
                    // records with a decryptable URL, and only when the
                    // persisted partial still validates on disk.
                    let mut resume = record.resume.filter(|_| {
                        url.is_some() && matches!(download.state, UiDownloadState::Interrupted)
                    });
                    if let Some(stored) = &resume {
                        if !resume_metadata_valid(&record.destination, stored, &download) {
                            resume = None;
                        }
                    }
                    if matches!(download.state, UiDownloadState::Interrupted) {
                        if resume.is_some() {
                            download.error = Some(INTERRUPTED_CHECKPOINT_MESSAGE.into());
                        } else {
                            download.bytes_written = 0;
                            download.error = Some(INTERRUPTED_BYTE_ZERO_MESSAGE.into());
                        }
                    }
                    jobs.insert(
                        download.id.clone(),
                        UiJob {
                            handle: None,
                            url,
                            destination: record.destination,
                            encrypted_url: record.encrypted_url,
                            download,
                            resume,
                            auto_retries: 0,
                            retry_timer: None,
                        },
                    );
                }
                load_notice
            }
            Err(error) => {
                eprintln!("Could not read the Velo download history: {error}");
                Some(
                    "Velo could not read the saved download history. New activity will still be tracked; existing entries may be unavailable until this is fixed."
                        .into(),
                )
            }
        };
        Self {
            manager,
            jobs: Arc::new(Mutex::new(jobs)),
            next_id: Arc::new(AtomicU64::new(next_order)),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            exit_ready: Arc::new(AtomicBool::new(false)),
            store_path,
            store_write_lock: Arc::new(Mutex::new(())),
            last_store_write: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(2))),
            history_notice: Arc::new(Mutex::new(notice)),
            publish,
            retry_backoff_base,
        }
    }

    pub(crate) fn persist(&self, force: bool) {
        let _write_guard = self
            .store_write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let mut last = self
                .last_store_write
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !force && last.elapsed() < Duration::from_secs(1) {
                return;
            }
            *last = Instant::now();
        }
        let records: Vec<_> = lock_jobs(&self.jobs)
            .values()
            .map(|job| persistence::StoredDownload {
                download: job.download.clone(),
                destination: job.destination.clone(),
                encrypted_url: job.encrypted_url.clone(),
                resume: job.resume.clone(),
            })
            .collect();
        if let Err(error) = persistence::save(&self.store_path, &records) {
            eprintln!("Could not save the Velo download history: {error}");
        }
    }

    pub(crate) fn emit(&self, event: UiEvent) {
        (self.publish)(event);
    }

    pub(crate) fn start(
        &self,
        url: String,
        destination: String,
        resume: Option<DurableResume>,
    ) -> Result<(DownloadDto, DownloadHandle), CommandError> {
        validate_url_input(&url)?;
        let encrypted_url = store_url(&url).map_err(|_| {
            CommandError::new(
                "history_unavailable",
                "Velo could not protect this download URL for local history.",
            )
        })?;
        let path = PathBuf::from(&destination);
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(CommandError::new(
                "invalid_destination",
                "Choose a valid destination file.",
            ));
        }

        let order = self.next_id.fetch_add(1, Ordering::Relaxed);
        let caller_id =
            DownloadId::new(format!("velo-{}-{order}", std::process::id())).map_err(|_| {
                CommandError::new("invalid_request", "The download request is invalid.")
            })?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Download".to_owned());
        let handle = self
            .manager
            .start(DownloadRequest {
                id: caller_id,
                url: url.clone(),
                destination: path,
                resume: resume.clone(),
            })
            .map_err(command_start_error)?;
        let snapshot = handle.latest_progress();
        let dto = DownloadDto {
            id: handle.id().as_str().to_owned(),
            generation: handle.generation(),
            revision: 1,
            order,
            file_name,
            destination: destination.clone(),
            state: snapshot.state.into(),
            bytes_written: snapshot.bytes_written,
            total_bytes: snapshot.total_bytes,
            speed_bytes_per_second: None,
            eta_seconds: None,
            error: None,
        };
        lock_jobs(&self.jobs).insert(
            dto.id.clone(),
            UiJob {
                handle: Some(handle.clone()),
                url: Some(url),
                destination,
                encrypted_url,
                download: dto.clone(),
                // Retained until the engine adopts (and renames) the partial
                // file; the monitor then refreshes it from the engine.
                resume: resume.map(|resume| persistence::StoredResume {
                    temp_path: resume.temp_path.to_string_lossy().into_owned(),
                    checkpoint: resume
                        .checkpoint
                        .map(|checkpoint| persistence::StoredCheckpoint {
                            effective_url: checkpoint.effective_url,
                            strong_etag: checkpoint.strong_etag,
                            total_bytes: checkpoint.total_bytes,
                        }),
                }),
                auto_retries: 0,
                retry_timer: None,
            },
        );
        self.persist(true);
        Ok((dto, handle))
    }

    // ---- command surface (wrapped one-to-one by the Tauri shell) ------------

    pub(crate) fn list_downloads(&self) -> Vec<DownloadDto> {
        let mut downloads: Vec<_> = lock_jobs(&self.jobs)
            .values()
            .map(|job| job.download.clone())
            .collect();
        downloads.sort_by_key(|download| download.order);
        downloads
    }

    pub(crate) fn begin_download(
        &self,
        url: String,
        directory: String,
        file_name: String,
    ) -> Result<DownloadDto, CommandError> {
        // The webview hands over a folder plus a bare file name; the native
        // side composes and validates the final path so the engine keeps
        // receiving one absolute destination under its own safety rules.
        let destination = super::settings::join_destination(&directory, &file_name).map_err(
            |error| match error {
                super::settings::JoinDestinationError::InvalidDirectory => CommandError::new(
                    "invalid_destination",
                    "Choose a valid folder for this download.",
                ),
                super::settings::JoinDestinationError::InvalidFileName(message) => {
                    CommandError::new("invalid_file_name", message)
                }
            },
        )?;
        let destination_text = destination.to_string_lossy().into_owned();
        let (download, handle) = self.start(url, destination_text, None)?;
        tokio::spawn({
            let state = self.clone();
            async move { state.monitor(handle).await }
        });
        Ok(download)
    }

    pub(crate) fn take_history_notice(&self) -> Option<String> {
        self.history_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn cancel_download(&self, id: &str, generation: u64) -> Result<(), CommandError> {
        // A scheduled retry owns no active transfer: cancel the pending timer
        // and close the record out. The retired engine handle is acknowledged
        // so the engine retains nothing for this job.
        let retrying_handle = {
            let mut jobs = lock_jobs(&self.jobs);
            let job = jobs.get_mut(id).ok_or_else(|| {
                CommandError::new("unknown_job", "This download is no longer available.")
            })?;
            if job.download.generation != generation {
                return Err(CommandError::new(
                    "stale_job",
                    "This download view is out of date.",
                ));
            }
            if job.download.state == UiDownloadState::Retrying {
                if let Some(timer) = job.retry_timer.take() {
                    timer.abort();
                }
                let handle = job.handle.take();
                job.download.revision = job.download.revision.saturating_add(1);
                job.download.state = UiDownloadState::Cancelled;
                job.download.error = Some("The automatic retry was cancelled.".into());
                let stored = job.resume.take();
                let dto = job.download.clone();
                drop(jobs);
                if let Some(stored) = stored {
                    let _ = fs::remove_file(&stored.temp_path);
                }
                if let Some(handle) = handle {
                    let _ = self.manager.acknowledge_terminal(&handle);
                }
                self.persist(true);
                self.emit(UiEvent::Updated(dto));
                return Ok(());
            }
            job.handle.clone()
        };
        let Some(handle) = retrying_handle else {
            return Err(CommandError::new(
                "interrupted",
                "This transfer has no active engine handle.",
            ));
        };
        if handle.generation() != generation {
            return Err(CommandError::new(
                "stale_job",
                "This download view is out of date.",
            ));
        }
        match self.manager.cancel(&handle) {
            Ok(CancelDisposition::Accepted | CancelDisposition::AlreadyCancelling) => Ok(()),
            Err(ControlError::TooLate) => Err(CommandError::new(
                "too_late",
                "The file is already being finalized.",
            )),
            Err(ControlError::AlreadyTerminal) => Err(CommandError::new(
                "already_finished",
                "This download has already finished.",
            )),
            Err(_) => Err(CommandError::new(
                "unknown_job",
                "This download is no longer available.",
            )),
        }
    }

    pub(crate) fn pause_download(&self, id: &str, generation: u64) -> Result<(), CommandError> {
        let jobs = lock_jobs(&self.jobs);
        let job = jobs.get(id).ok_or_else(|| {
            CommandError::new("unknown_job", "This download is no longer available.")
        })?;
        let Some(handle) = job.handle.as_ref() else {
            return Err(CommandError::new(
                "interrupted",
                "This transfer has no active engine handle.",
            ));
        };
        if handle.generation() != generation {
            return Err(CommandError::new(
                "stale_job",
                "This download view is out of date.",
            ));
        }
        self.manager.pause(handle).map_err(|error| match error {
            ControlError::CannotPause => CommandError::new(
                "cannot_pause",
                "This download cannot be paused in its current state.",
            ),
            _ => CommandError::new("unknown_job", "This download is no longer available."),
        })
    }

    pub(crate) fn resume_download(&self, id: &str, generation: u64) -> Result<(), CommandError> {
        let jobs = lock_jobs(&self.jobs);
        let job = jobs.get(id).ok_or_else(|| {
            CommandError::new("unknown_job", "This download is no longer available.")
        })?;
        let Some(handle) = job.handle.as_ref() else {
            return Err(CommandError::new(
                "interrupted",
                "This transfer has no active engine handle.",
            ));
        };
        if handle.generation() != generation {
            return Err(CommandError::new(
                "stale_job",
                "This download view is out of date.",
            ));
        }
        self.manager
            .resume(handle)
            .map_err(|_| CommandError::new("not_paused", "This download is not paused."))
    }

    pub(crate) fn retry_download(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<DownloadDto, CommandError> {
        let (url, destination, resume, old_handle) = {
            let jobs = lock_jobs(&self.jobs);
            let old = jobs.get(id).ok_or_else(|| {
                CommandError::new("unknown_job", "This download is no longer available.")
            })?;
            if old.download.generation != generation {
                return Err(CommandError::new(
                    "stale_job",
                    "This download view is out of date.",
                ));
            }
            if !matches!(
                old.download.state,
                UiDownloadState::Failed | UiDownloadState::Cancelled | UiDownloadState::Interrupted
            ) {
                return Err(CommandError::new(
                    "not_retryable",
                    "Only interrupted or failed downloads can be retried.",
                ));
            }
            let resume = old
                .resume
                .as_ref()
                .and_then(|stored| durable_from_stored(stored, old.download.bytes_written));
            (
                old.url.clone().ok_or_else(|| {
                    CommandError::new(
                        "url_unavailable",
                        "Velo cannot decrypt the saved URL on this account.",
                    )
                })?,
                old.destination.clone(),
                resume,
                old.handle.clone(),
            )
        };
        if let Some(handle) = old_handle {
            self.manager.acknowledge_terminal(&handle).map_err(|_| {
                CommandError::new("not_terminal", "The finished result is not available.")
            })?;
            if let Some(old_job) = lock_jobs(&self.jobs).get_mut(id) {
                old_job.handle = None;
            }
        }
        let (download, handle) = self.start(url, destination, resume)?;
        lock_jobs(&self.jobs).remove(id);
        self.persist(true);
        self.emit(UiEvent::Removed {
            id: id.to_owned(),
            generation,
        });
        tokio::spawn({
            let state = self.clone();
            async move { state.monitor(handle).await }
        });
        Ok(download)
    }

    pub(crate) fn acknowledge_download(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<(), CommandError> {
        let mut jobs = lock_jobs(&self.jobs);
        let job = jobs.get_mut(id).ok_or_else(|| {
            CommandError::new("unknown_job", "This download is no longer available.")
        })?;
        if job.download.generation != generation {
            return Err(CommandError::new(
                "stale_job",
                "This download view is out of date.",
            ));
        }
        if !matches!(
            job.download.state,
            UiDownloadState::Completed
                | UiDownloadState::Failed
                | UiDownloadState::Cancelled
                | UiDownloadState::Interrupted
        ) {
            return Err(CommandError::new(
                "not_terminal",
                "Only finished downloads can be dismissed.",
            ));
        }
        if let Some(handle) = job.handle.as_ref() {
            self.manager.acknowledge_terminal(handle).map_err(|_| {
                CommandError::new("not_terminal", "The finished result is not available.")
            })?;
        }
        // Dismissing an interrupted record also abandons its partial file.
        let stored = job.resume.take();
        jobs.remove(id);
        drop(jobs);
        if let Some(stored) = stored {
            let _ = fs::remove_file(&stored.temp_path);
        }
        self.persist(true);
        Ok(())
    }

    pub(crate) async fn monitor(&self, handle: DownloadHandle) {
        let mut progress = handle.progress();
        let mut last_bytes = progress.borrow().bytes_written;
        let mut last_update = Instant::now();
        let mut speed = None;

        loop {
            let snapshot = progress.borrow().clone();
            let now = Instant::now();
            if snapshot.bytes_written > last_bytes {
                let elapsed = now.duration_since(last_update).as_secs_f64();
                if elapsed > 0.0 {
                    speed = Some(
                        ((snapshot.bytes_written - last_bytes) as f64 / elapsed).round() as u64,
                    );
                }
                last_bytes = snapshot.bytes_written;
                last_update = now;
            }
            if snapshot.state != DownloadState::Downloading {
                speed = None;
            }

            let terminal = matches!(
                snapshot.state,
                DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
            );
            let mut terminal_error = None;
            if terminal {
                terminal_error = self
                    .manager
                    .terminal_result(&handle)
                    .ok()
                    .flatten()
                    .and_then(|result| result.error.clone());
            }
            let error = terminal_error
                .as_deref()
                .map(safe_download_error)
                .filter(|_| snapshot.state == DownloadState::Failed);
            let transient_failure = snapshot.state == DownloadState::Failed;
            let updated = update_job(self, &handle, snapshot, speed, error);
            if let Some(download) = updated {
                self.persist(terminal);
                self.emit(UiEvent::Updated(download.clone()));
                if terminal {
                    if transient_failure
                        && terminal_error.as_deref().is_some_and(is_transient_failure)
                        && self.schedule_auto_retry(&handle)
                    {
                        // The scheduled retry now owns this job; stop monitoring
                        // the terminal engine handle.
                        return;
                    }
                    return;
                }
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }

    /// Moves a transiently failed download into the scheduled-retry state and
    /// arms its backoff timer. Returns false when the retry budget is spent.
    fn schedule_auto_retry(&self, handle: &DownloadHandle) -> bool {
        let retrying = {
            let mut jobs = lock_jobs(&self.jobs);
            let Some(job) = jobs.get_mut(handle.id().as_str()) else {
                return false;
            };
            if job
                .handle
                .as_ref()
                .is_none_or(|registered| registered.generation() != handle.generation())
            {
                return false;
            }
            if job.auto_retries >= MAX_AUTO_RETRIES {
                return false;
            }
            job.auto_retries += 1;
            let attempt = job.auto_retries;
            let backoff = self
                .retry_backoff_base
                .saturating_mul(4_u32.saturating_pow(u32::from(attempt) - 1));
            job.download.revision = job.download.revision.saturating_add(1);
            job.download.state = UiDownloadState::Retrying;
            job.download.speed_bytes_per_second = None;
            job.download.eta_seconds = None;
            job.download.error = Some(format!(
                "Connection lost. Retrying automatically \u{2014} attempt {} of {}.",
                attempt + 1,
                u16::from(MAX_AUTO_RETRIES) + 1
            ));
            let timer = tokio::spawn(schedule_retry_timer(
                self.clone(),
                handle.id().as_str().to_owned(),
                handle.generation(),
                backoff,
            ));
            job.retry_timer = Some(timer);
            job.download.clone()
        };
        self.persist(true);
        self.emit(UiEvent::Updated(retrying));
        true
    }

    /// Runs one scheduled retry: replaces the terminal engine job with a new
    /// one that adopts the verified partial data. No-ops if the user cancelled
    /// or dismissed the pending retry while the timer was sleeping.
    fn fire_scheduled_retry(&self, id: &str, generation: u64) {
        let (url, destination, resume, old_handle) = {
            let mut jobs = lock_jobs(&self.jobs);
            let Some(job) = jobs.get_mut(id) else {
                return;
            };
            if job.download.generation != generation
                || job.download.state != UiDownloadState::Retrying
            {
                return;
            }
            let resume = job.resume.as_ref().and_then(|stored| {
                let bytes = job.download.bytes_written;
                durable_from_stored(stored, bytes)
            });
            (
                job.url.clone(),
                job.destination.clone(),
                resume,
                job.handle.take(),
            )
        };
        if let Some(handle) = old_handle {
            let _ = self.manager.acknowledge_terminal(&handle);
        }
        let Some(url) = url else {
            self.fail_scheduled_retry(
                id,
                generation,
                "The saved URL cannot be decrypted on this Windows account.",
            );
            return;
        };
        match self.start(url, destination, resume) {
            Ok((download, handle)) => {
                {
                    let mut jobs = lock_jobs(&self.jobs);
                    jobs.remove(id);
                }
                self.emit(UiEvent::Removed {
                    id: id.to_owned(),
                    generation,
                });
                self.persist(true);
                let monitor_state = self.clone();
                tokio::spawn(async move { monitor_state.monitor(handle).await });
                self.emit(UiEvent::Updated(download));
            }
            Err(error) if error.code == "shutting_down" => {
                // The app is closing: leave an honest retryable record.
                let mut jobs = lock_jobs(&self.jobs);
                if let Some(job) = jobs.get_mut(id) {
                    if job.download.generation == generation {
                        job.download.state = UiDownloadState::Interrupted;
                        job.download.error = Some(INTERRUPTED_BYTE_ZERO_MESSAGE.into());
                        let dto = job.download.clone();
                        drop(jobs);
                        self.persist(true);
                        self.emit(UiEvent::Updated(dto));
                    }
                }
            }
            Err(error) => {
                self.fail_scheduled_retry(id, generation, &error.message);
            }
        }
    }

    fn fail_scheduled_retry(&self, id: &str, generation: u64, message: &str) {
        let mut jobs = lock_jobs(&self.jobs);
        if let Some(job) = jobs.get_mut(id) {
            if job.download.generation != generation
                || job.download.state != UiDownloadState::Retrying
            {
                return;
            }
            job.download.revision = job.download.revision.saturating_add(1);
            job.download.state = UiDownloadState::Failed;
            job.download.error = Some(message.to_owned());
            let dto = job.download.clone();
            drop(jobs);
            self.persist(true);
            self.emit(UiEvent::Updated(dto));
        }
    }
}

pub(crate) fn lock_jobs(
    jobs: &Mutex<HashMap<String, UiJob>>,
) -> std::sync::MutexGuard<'_, HashMap<String, UiJob>> {
    jobs.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn validate_url_input(value: &str) -> Result<(), CommandError> {
    let parsed = Url::parse(value)
        .map_err(|_| CommandError::new("invalid_url", "Enter a complete HTTP or HTTPS URL."))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CommandError::new(
            "invalid_url",
            "Only HTTP and HTTPS URLs without embedded credentials are supported.",
        ));
    }
    Ok(())
}

fn command_start_error(error: StartError) -> CommandError {
    match error {
        StartError::InvalidRequest(error) => {
            CommandError::new("invalid_request", safe_download_error(&error))
        }
        StartError::DuplicateId => {
            CommandError::new("duplicate_id", "This download is already registered.")
        }
        StartError::GenerationExhausted => {
            CommandError::new("capacity", "The download manager cannot accept more jobs.")
        }
        StartError::CapacityReached => CommandError::new(
            "capacity",
            "The download list is full. Dismiss a finished item and try again.",
        ),
        StartError::ShuttingDown => {
            CommandError::new("shutting_down", "The download manager is closing.")
        }
        StartError::InvalidId => {
            CommandError::new("invalid_request", "The download request is invalid.")
        }
    }
}

pub(crate) fn safe_download_error(error: &DownloadError) -> String {
    match error {
        DownloadError::InvalidUrl => "The URL is invalid or unsupported.".into(),
        DownloadError::InvalidDestination(_) => "The selected destination is not available.".into(),
        DownloadError::DestinationConflict => {
            "A download already uses this destination, or the file already exists.".into()
        }
        DownloadError::Filesystem { .. } => {
            "A filesystem operation failed. Check the destination and available space.".into()
        }
        DownloadError::HttpStatus(status) => {
            format!("The server returned HTTP {status} instead of a downloadable file.")
        }
        DownloadError::InvalidResponse => {
            "The server returned an invalid or unsupported response.".into()
        }
        DownloadError::NetworkFailure(NetworkFailureKind::Timeout) => {
            "The connection timed out.".into()
        }
        DownloadError::NetworkFailure(NetworkFailureKind::RequestOrBody) => {
            "The network connection was interrupted.".into()
        }
        DownloadError::ClientInitialization => {
            "The secure HTTP client could not be initialized.".into()
        }
        DownloadError::RedirectRejected => {
            "The server redirected to a URL that is not allowed.".into()
        }
        DownloadError::IncompleteResponse { .. } => {
            "The response ended before the complete file was received.".into()
        }
        DownloadError::ByteCountOverflow => "The response exceeded the supported file size.".into(),
        DownloadError::WriteZero => "The destination stopped accepting file data.".into(),
        DownloadError::Cancelled => "The download was cancelled.".into(),
        DownloadError::WorkerPanicked => "The download worker stopped unexpectedly.".into(),
        DownloadError::BodyInactivityTimeout => "The server stopped sending data.".into(),
        DownloadError::CleanupFailure { .. } => {
            "The transfer failed and its temporary file could not be removed.".into()
        }
        DownloadError::PromotedCleanupFailure { .. } => {
            "The file was saved, but temporary-file cleanup failed.".into()
        }
    }
}

fn update_job(
    state: &AppState,
    handle: &DownloadHandle,
    snapshot: consolex_download_engine::ProgressSnapshot,
    speed: Option<u64>,
    error: Option<String>,
) -> Option<DownloadDto> {
    let mut jobs = lock_jobs(&state.jobs);
    let job = jobs.get_mut(handle.id().as_str())?;
    if job
        .handle
        .as_ref()
        .is_none_or(|registered| registered.generation() != handle.generation())
    {
        return None;
    }
    let dto = &mut job.download;
    dto.revision = dto.revision.saturating_add(1);
    dto.state = snapshot.state.into();
    dto.bytes_written = snapshot.bytes_written;
    dto.total_bytes = snapshot.total_bytes;
    dto.speed_bytes_per_second = speed.filter(|_| snapshot.state == DownloadState::Downloading);
    dto.eta_seconds = dto.speed_bytes_per_second.and_then(|rate| {
        let remaining = dto.total_bytes?.saturating_sub(dto.bytes_written);
        (rate > 0).then(|| (remaining as f64 / rate as f64).ceil() as u64)
    });
    dto.error = error;
    let terminal = snapshot.state.is_terminal();
    if terminal {
        // The engine has removed or promoted the temporary file: drop any
        // persisted resume metadata and clear the partial if it lingers.
        if let Some(stored) = job.resume.take() {
            drop(jobs);
            let _ = fs::remove_file(&stored.temp_path);
            return some_locked_dto(state, handle);
        }
    } else if let Ok(Some(info)) = state.manager.resume_info(handle) {
        job.resume = Some(persistence::StoredResume {
            temp_path: info.temp_path.to_string_lossy().into_owned(),
            checkpoint: info
                .checkpoint
                .map(|checkpoint| persistence::StoredCheckpoint {
                    effective_url: checkpoint.effective_url,
                    strong_etag: checkpoint.strong_etag,
                    total_bytes: checkpoint.total_bytes,
                }),
        });
    }
    Some(dto.clone())
}

/// Rebuilds the DTO after the jobs lock was released mid-update.
fn some_locked_dto(state: &AppState, handle: &DownloadHandle) -> Option<DownloadDto> {
    let jobs = lock_jobs(&state.jobs);
    jobs.get(handle.id().as_str())
        .filter(|job| {
            job.handle
                .as_ref()
                .is_none_or(|registered| registered.generation() == handle.generation())
        })
        .map(|job| job.download.clone())
}

fn is_transient_failure(error: &DownloadError) -> bool {
    match error {
        DownloadError::NetworkFailure(_)
        | DownloadError::BodyInactivityTimeout
        | DownloadError::IncompleteResponse { .. } => true,
        DownloadError::HttpStatus(status) => *status >= 500,
        _ => false,
    }
}

/// Converts persisted resume metadata back into engine input. The engine
/// re-verifies everything against the filesystem and the server.
fn durable_from_stored(
    stored: &persistence::StoredResume,
    bytes_written: u64,
) -> Option<DurableResume> {
    if stored.temp_path.is_empty() || bytes_written == 0 {
        return None;
    }
    Some(DurableResume {
        temp_path: PathBuf::from(&stored.temp_path),
        persisted_bytes: bytes_written,
        checkpoint: stored
            .checkpoint
            .as_ref()
            .map(|checkpoint| ResumeCheckpoint {
                effective_url: checkpoint.effective_url.clone(),
                strong_etag: checkpoint.strong_etag.clone(),
                total_bytes: checkpoint.total_bytes,
            }),
    })
}

/// Startup-time validation of persisted resume metadata. Mirrors the rules
/// the engine enforces again at adoption: the partial must be a regular file
/// next to the destination, at least as long as the persisted counter, and —
/// when a checkpoint exists — strictly shorter than the recorded total.
fn resume_metadata_valid(
    destination: &str,
    stored: &persistence::StoredResume,
    download: &DownloadDto,
) -> bool {
    if download.bytes_written == 0 {
        return false;
    }
    let temp_path = Path::new(&stored.temp_path);
    if !temp_path.is_absolute() {
        return false;
    }
    let same_parent = match (Path::new(destination).parent(), temp_path.parent()) {
        (Some(destination_parent), Some(temp_parent)) => destination_parent == temp_parent,
        _ => false,
    };
    if !same_parent {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(temp_path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() < download.bytes_written {
        return false;
    }
    match &stored.checkpoint {
        Some(checkpoint) => {
            checkpoint.total_bytes > download.bytes_written
                && checkpoint.total_bytes > metadata.len()
        }
        None => false,
    }
}

async fn schedule_retry_timer(state: AppState, id: String, generation: u64, delay: Duration) {
    tokio::time::sleep(delay).await;
    state.fire_scheduled_retry(&id, generation);
}

fn store_url(value: &str) -> std::io::Result<Vec<u8>> {
    #[cfg(test)]
    {
        persistence::test_protection::protect(value)
    }
    #[cfg(not(test))]
    {
        persistence::protect_url(value)
    }
}

fn restore_url(value: &[u8]) -> std::io::Result<String> {
    #[cfg(test)]
    {
        persistence::test_protection::unprotect(value)
    }
    #[cfg(not(test))]
    {
        persistence::unprotect_url(value)
    }
}

#[cfg(test)]
pub(crate) mod tests {

    use super::*;
    use std::{fs, sync::atomic::AtomicU64, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn url_command_validation_rejects_unsupported_and_credential_urls() {
        assert!(validate_url_input("https://example.test/file").is_ok());
        assert!(validate_url_input("ftp://example.test/file").is_err());
        assert!(validate_url_input("https://user:secret@example.test/file").is_err());
    }

    #[tokio::test]
    async fn begin_download_rejects_unsafe_destination_composition() {
        let dir = std::env::temp_dir().join(format!(
            "velo-join-guard-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let (state, _updates, _removals) =
            test_state(dir.join("history.json"), Duration::from_millis(20));
        let error = state
            .begin_download(
                "https://example.test/file".into(),
                dir.to_string_lossy().into_owned(),
                "../escape.bin".into(),
            )
            .unwrap_err();
        assert_eq!(error.code, "invalid_file_name");
        state.manager.shutdown().await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn command_error_mapping_never_includes_sensitive_url_or_path() {
        let error = DownloadError::Filesystem {
            operation: "create temporary file",
            path: PathBuf::from(r"C:\private\secret.bin"),
            source: std::io::Error::other("denied"),
        };
        let message = safe_download_error(&error);
        assert!(!message.contains("secret.bin"));
        assert!(!message.contains("denied"));
    }

    #[tokio::test]
    async fn application_service_runs_a_real_local_download_and_publishes_terminal_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhello velo",
                )
                .await
                .unwrap();
        });

        let dir = std::env::temp_dir().join(format!(
            "velo-app-service-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("service.bin");
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_publish = events.clone();
        let state = AppState::with_test_hooks(
            DownloadManager::new().unwrap(),
            std::env::temp_dir().join("velo-test-history.json"),
            Arc::new(move |event| match event {
                UiEvent::Updated(download) => {
                    events_for_publish.lock().unwrap().push(download);
                }
                UiEvent::Removed { .. } => {}
            }),
            Duration::from_secs(2),
        );
        let (initial, handle) = state
            .start(
                format!("http://{address}/file"),
                destination.to_string_lossy().into_owned(),
                None,
            )
            .unwrap();
        assert_eq!(initial.file_name, "service.bin");

        let monitor_state = state.clone();
        let monitor_handle = handle.clone();
        let monitor = tokio::spawn(async move { monitor_state.monitor(monitor_handle).await });

        let mut progress = handle.progress();
        while !matches!(
            progress.borrow().state,
            DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
        ) {
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    progress.changed().await.unwrap();
                    if matches!(
                        progress.borrow().state,
                        DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
                    ) {
                        break;
                    }
                }
            })
            .await
            .expect("application download should finish");
        }
        monitor.await.unwrap();
        server.await.unwrap();

        let terminal = state.manager.terminal_result(&handle).unwrap().unwrap();
        assert_eq!(terminal.state, DownloadState::Completed);
        assert_eq!(fs::read(&destination).unwrap(), b"hello velo");
        {
            let published = events.lock().unwrap();
            assert!(published
                .iter()
                .any(|event| event.state == UiDownloadState::Completed));
            assert_eq!(published.last().unwrap().bytes_written, 10);
        }
        state.manager.acknowledge_terminal(&handle).unwrap();
        state.manager.shutdown().await;
        fs::remove_dir_all(dir).unwrap();
    }

    // ---- bounded automatic retry, cancellation, and durable restore ---------

    type TestState = (
        AppState,
        Arc<Mutex<Vec<DownloadDto>>>,
        Arc<Mutex<Vec<(String, u64)>>>,
    );

    fn test_state(store_path: std::path::PathBuf, backoff: Duration) -> TestState {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let removals = Arc::new(Mutex::new(Vec::new()));
        let updates_for_publish = updates.clone();
        let removals_for_publish = removals.clone();
        let state = AppState::with_test_hooks(
            DownloadManager::new().unwrap(),
            store_path,
            Arc::new(move |event| match event {
                UiEvent::Updated(download) => {
                    updates_for_publish.lock().unwrap().push(download);
                }
                UiEvent::Removed { id, generation } => {
                    removals_for_publish.lock().unwrap().push((id, generation));
                }
            }),
            backoff,
        );
        (state, updates, removals)
    }

    fn state_sequence(updates: &Mutex<Vec<DownloadDto>>, id: &str) -> Vec<UiDownloadState> {
        updates
            .lock()
            .unwrap()
            .iter()
            .filter(|download| download.id == id)
            .map(|download| download.state)
            .collect()
    }

    fn state_sequence_all(updates: &Mutex<Vec<DownloadDto>>) -> Vec<UiDownloadState> {
        updates
            .lock()
            .unwrap()
            .iter()
            .map(|download| download.state)
            .collect()
    }

    /// Waits until the newest published update is `expected` and stays so
    /// through a settle window longer than the largest injected backoff. The
    /// id is deliberately not filtered: a scheduled retry replaces the record
    /// with a fresh job id, and the chain's outcome is the last update.
    async fn wait_for_terminal_state(
        updates: &Mutex<Vec<DownloadDto>>,
        expected: UiDownloadState,
    ) -> UiDownloadState {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let latest = |updates: &Mutex<Vec<DownloadDto>>| {
                    updates
                        .lock()
                        .unwrap()
                        .last()
                        .map(|download| (download.state, download.revision))
                };
                if latest(updates).is_some_and(|(state, _)| state == expected) {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    if latest(updates).is_some_and(|(state, _)| state == expected) {
                        return expected;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the job should reach the expected terminal state")
    }

    /// Accepts `failures` connections, answering each with the given HTTP
    /// status, then one final connection with a 200 and the fixture body.
    async fn serve_retry_sequence(failures: &[&str]) -> (String, JoinHandle<()>) {
        let failures: Vec<String> = failures.iter().map(|status| status.to_string()).collect();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for status in failures {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).await.unwrap();
                let head =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"final\"\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(b"0123456789").await;
            let _ = stream.shutdown().await;
        });
        (format!("http://{address}/file"), task)
    }

    #[tokio::test]
    async fn transient_failures_are_retried_with_a_bounded_schedule_and_then_succeed() {
        let dir = std::env::temp_dir().join(format!(
            "velo-auto-retry-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let (url, server) =
            serve_retry_sequence(&["503 Service Unavailable", "502 Bad Gateway"]).await;
        let (state, updates, _removals) =
            test_state(dir.join("history.json"), Duration::from_millis(20));
        state
            .begin_download(
                url,
                dir.to_string_lossy().into_owned(),
                "out.bin".to_owned(),
            )
            .unwrap();
        let terminal = wait_for_terminal_state(&updates, UiDownloadState::Completed).await;
        server.await.unwrap();
        assert_eq!(terminal, UiDownloadState::Completed);
        let mut chain = state_sequence_all(&updates);
        chain.dedup_by(|a, b| a == b);
        assert_eq!(
            chain,
            vec![
                UiDownloadState::Preparing,
                UiDownloadState::Failed,
                UiDownloadState::Retrying,
                UiDownloadState::Preparing,
                UiDownloadState::Failed,
                UiDownloadState::Retrying,
                UiDownloadState::Preparing,
                UiDownloadState::Downloading,
                UiDownloadState::Completed,
            ]
        );
        assert_eq!(fs::read(dir.join("out.bin")).unwrap(), b"0123456789");
        state.manager.shutdown().await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn server_errors_that_are_not_transient_are_never_retried() {
        let dir = std::env::temp_dir().join(format!(
            "velo-no-retry-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let (state, updates, _removals) =
            test_state(dir.join("history.json"), Duration::from_millis(20));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            let _ = stream.shutdown().await;
        });
        let url = format!("http://{address}/missing.bin");
        let download = state
            .begin_download(
                url,
                dir.to_string_lossy().into_owned(),
                "out.bin".to_owned(),
            )
            .unwrap();
        let terminal = wait_for_terminal_state(&updates, UiDownloadState::Failed).await;
        server.await.unwrap();
        assert_eq!(terminal, UiDownloadState::Failed);
        assert!(state_sequence(&updates, &download.id)
            .iter()
            .all(|state| *state != UiDownloadState::Retrying));
        state.manager.shutdown().await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelling_a_scheduled_retry_stops_the_timer_before_it_fires() {
        let dir = std::env::temp_dir().join(format!(
            "velo-cancel-retry-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let (url, server) = serve_retry_sequence(&["503 Service Unavailable"]).await;
        let (state, updates, _removals) = test_state(
            dir.join("history.json"),
            Duration::from_millis(60_000), // long enough to always win the race
        );
        let download = state
            .begin_download(
                url,
                dir.to_string_lossy().into_owned(),
                "out.bin".to_owned(),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let retrying = updates
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|download| download.state == UiDownloadState::Retrying);
                if retrying {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the job should enter the retrying state");
        state
            .cancel_download(&download.id, download.generation)
            .unwrap();
        let terminal = wait_for_terminal_state(&updates, UiDownloadState::Cancelled).await;
        assert_eq!(terminal, UiDownloadState::Cancelled);
        // Give the (aborted) timer every chance to misfire.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!fs::exists(dir.join("out.bin")).unwrap());
        state.manager.shutdown().await;
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_restored_interrupted_record_resumes_from_its_persisted_checkpoint_on_retry() {
        let dir = std::env::temp_dir().join(format!(
            "velo-restore-resume-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        // Canonicalize so the parent directory matches the UNC path the engine
        // derives from fs::canonicalize inside canonical_destination. Without
        // this, open_or_create_temp rejects the partial because the regular
        // C:\ parent doesn't equal the \\?\ canonical parent.
        let dir = fs::canonicalize(&dir).unwrap();
        let partial = dir.join(".consolex-persisted-42.part");
        fs::write(&partial, b"01234").unwrap();
        let destination = dir.join("restored.bin");

        // The retry must go to the real server that continues from byte five.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let source_url = format!("http://{address}/file");
        let server = tokio::spawn(async move {
            let (mut range, _) = listener.accept().await.unwrap();
            let request = read_request_text(&mut range).await;
            let lowered = request.to_ascii_lowercase();
            assert!(lowered.contains("range: bytes=5-"));
            assert!(lowered.contains("if-range: \"stable\""));
            let head = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-9/10\r\nContent-Length: 5\r\nETag: \"stable\"\r\nConnection: close\r\n\r\n";
            let _ = range.write_all(head.as_bytes()).await;
            let _ = range.write_all(b"56789").await;
            let _ = range.shutdown().await;
        });

        // A v2 snapshot as the previous session would have written it: an
        // interrupted record with verified partial data and a checkpoint.
        let record = serde_json::json!({
            "download": {
                "id": "velo-restored",
                "generation": 7,
                "revision": 9,
                "order": 1,
                "fileName": "restored.bin",
                "state": "interrupted",
                "bytesWritten": 5,
                "totalBytes": 10,
                "speedBytesPerSecond": null,
                "etaSeconds": null,
                "error": "Velo closed before this transfer finished."
            },
            "destination": destination.to_string_lossy(),
            "encrypted_url": source_url.bytes().collect::<Vec<u8>>(),
            "resume": {
                "temp_path": partial.to_string_lossy(),
                "checkpoint": {
                    "effective_url": source_url,
                    "strong_etag": "\"stable\"",
                    "total_bytes": 10
                }
            }
        });
        let store = serde_json::json!({ "version": 2, "downloads": [record] });
        let store_path = dir.join("history.json");
        fs::write(&store_path, serde_json::to_vec(&store).unwrap()).unwrap();

        let (state, updates, removals) = test_state(store_path, Duration::from_millis(20));
        let restored = state.list_downloads();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].state, UiDownloadState::Interrupted);
        assert_eq!(restored[0].bytes_written, 5);

        let replacement = state
            .retry_download("velo-restored", 7)
            .expect("a restored interrupted record is retryable");
        assert_ne!(replacement.id, "velo-restored");
        assert_eq!(replacement.bytes_written, 5);
        let terminal = wait_for_terminal_state(&updates, UiDownloadState::Completed).await;
        server.await.unwrap();
        assert_eq!(terminal, UiDownloadState::Completed);
        assert_eq!(fs::read(&destination).unwrap(), b"0123456789");
        assert!(
            !partial.exists(),
            "the persisted partial was adopted by rename"
        );
        assert_eq!(
            removals.lock().unwrap().clone(),
            vec![("velo-restored".to_owned(), 7_u64)]
        );
        state
            .acknowledge_download(&replacement.id, replacement.generation)
            .unwrap();
        state.manager.shutdown().await;
        fs::remove_dir_all(dir).unwrap();
    }

    async fn read_request_text(stream: &mut tokio::net::TcpStream) -> String {
        let mut buffer = vec![0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    }

    #[tokio::test]
    async fn a_malformed_history_snapshot_surfaces_a_one_shot_notice() {
        let dir = std::env::temp_dir().join(format!(
            "velo-notice-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let store_path = dir.join("history.json");
        fs::write(&store_path, b"{ truncated nonsense").unwrap();
        let (state, _updates, _removals) = test_state(store_path, Duration::from_millis(20));
        let notice = state.take_history_notice();
        assert!(notice.is_some());
        assert!(notice.unwrap().contains("backup"));
        assert!(state.take_history_notice().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
