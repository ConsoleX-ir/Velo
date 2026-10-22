use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

mod persistence;

use consolex_download_engine::{
    CancelDisposition, ControlError, DownloadError, DownloadHandle, DownloadId, DownloadManager,
    DownloadRequest, DownloadState, NetworkFailureKind, StartError,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use url::Url;

const DOWNLOAD_UPDATED_EVENT: &str = "download-updated";

#[derive(Clone)]
struct AppState {
    manager: DownloadManager,
    jobs: Arc<Mutex<HashMap<String, UiJob>>>,
    next_id: Arc<AtomicU64>,
    shutdown_started: Arc<AtomicBool>,
    exit_ready: Arc<AtomicBool>,
    store_path: PathBuf,
    store_write_lock: Arc<Mutex<()>>,
    last_store_write: Arc<Mutex<Instant>>,
}

struct UiJob {
    handle: Option<DownloadHandle>,
    url: Option<String>,
    destination: String,
    encrypted_url: Vec<u8>,
    download: DownloadDto,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DownloadDto {
    id: String,
    generation: u64,
    revision: u64,
    order: u64,
    file_name: String,
    state: UiDownloadState,
    bytes_written: u64,
    total_bytes: Option<u64>,
    speed_bytes_per_second: Option<u64>,
    eta_seconds: Option<u64>,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum UiDownloadState {
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
struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl AppState {
    fn new(manager: DownloadManager, store_path: PathBuf) -> Self {
        let mut jobs = HashMap::new();
        let mut next_order = 1_u64;
        match persistence::load(&store_path) {
            Ok(stored) => {
                for record in stored {
                    let mut download = record.download;
                    next_order = next_order.max(download.order.saturating_add(1));
                    let url = persistence::unprotect_url(&record.encrypted_url).ok();
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
                        download.state = UiDownloadState::Interrupted;
                        download.bytes_written = 0;
                        download.speed_bytes_per_second = None;
                        download.eta_seconds = None;
                        download.error = Some("Velo closed before this transfer finished. Retry starts from byte zero.".into());
                    }
                    if url.is_none() && !matches!(download.state, UiDownloadState::Completed) {
                        download.error = Some(
                            "This saved URL cannot be decrypted on this Windows account.".into(),
                        );
                    }
                    jobs.insert(
                        download.id.clone(),
                        UiJob {
                            handle: None,
                            url,
                            destination: record.destination,
                            encrypted_url: record.encrypted_url,
                            download,
                        },
                    );
                }
            }
            Err(error) => eprintln!("Could not read the Velo download history: {error}"),
        }
        Self {
            manager,
            jobs: Arc::new(Mutex::new(jobs)),
            next_id: Arc::new(AtomicU64::new(next_order)),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            exit_ready: Arc::new(AtomicBool::new(false)),
            store_path,
            store_write_lock: Arc::new(Mutex::new(())),
            last_store_write: Arc::new(Mutex::new(
                Instant::now() - std::time::Duration::from_secs(2),
            )),
        }
    }

    fn persist(&self, force: bool) {
        let _write_guard = self
            .store_write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let mut last = self
                .last_store_write
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !force && last.elapsed() < std::time::Duration::from_secs(1) {
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
            })
            .collect();
        if let Err(error) = persistence::save(&self.store_path, &records) {
            eprintln!("Could not save the Velo download history: {error}");
        }
    }

    fn start(
        &self,
        url: String,
        destination: String,
    ) -> Result<(DownloadDto, DownloadHandle), CommandError> {
        validate_url_input(&url)?;
        let encrypted_url = persistence::protect_url(&url).map_err(|_| {
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
            })
            .map_err(command_start_error)?;
        let snapshot = handle.latest_progress();
        let dto = DownloadDto {
            id: handle.id().as_str().to_owned(),
            generation: handle.generation(),
            revision: 1,
            order,
            file_name,
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
            },
        );
        self.persist(true);
        Ok((dto, handle))
    }

    async fn monitor<F>(&self, handle: DownloadHandle, publish: F)
    where
        F: Fn(DownloadDto) + Send + 'static,
    {
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
            let error = if terminal && snapshot.state == DownloadState::Failed {
                self.manager
                    .terminal_result(&handle)
                    .ok()
                    .flatten()
                    .and_then(|result| result.error.as_deref().map(safe_download_error))
            } else {
                None
            };
            let updated = update_job(self, &handle, snapshot, speed, error);
            if let Some(download) = updated {
                self.persist(terminal);
                publish(download);
            }
            if terminal {
                return;
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }
}

fn lock_jobs(
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

fn safe_download_error(error: &DownloadError) -> String {
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
    Some(dto.clone())
}

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Vec<DownloadDto> {
    let mut downloads: Vec<_> = lock_jobs(&state.jobs)
        .values()
        .map(|job| job.download.clone())
        .collect();
    downloads.sort_by_key(|download| download.order);
    downloads
}

#[tauri::command]
fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    destination: String,
) -> Result<DownloadDto, CommandError> {
    let state = state.inner().clone();
    let (download, handle) = state.start(url, destination)?;
    tauri::async_runtime::spawn(async move {
        let app_for_events = app.clone();
        state
            .monitor(handle, move |download| {
                let _ = app_for_events.emit(DOWNLOAD_UPDATED_EVENT, download);
            })
            .await;
    });
    Ok(download)
}

#[tauri::command]
fn cancel_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let jobs = lock_jobs(&state.jobs);
    let job = jobs
        .get(&id)
        .ok_or_else(|| CommandError::new("unknown_job", "This download is no longer available."))?;
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
    match state.manager.cancel(handle) {
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

#[tauri::command]
fn pause_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let jobs = lock_jobs(&state.jobs);
    let job = jobs
        .get(&id)
        .ok_or_else(|| CommandError::new("unknown_job", "This download is no longer available."))?;
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
    state.manager.pause(handle).map_err(|error| match error {
        ControlError::CannotPause => CommandError::new(
            "cannot_pause",
            "This download cannot be paused in its current state.",
        ),
        _ => CommandError::new("unknown_job", "This download is no longer available."),
    })
}

#[tauri::command]
fn resume_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let state = state.inner().clone();
    let jobs = lock_jobs(&state.jobs);
    let job = jobs
        .get(&id)
        .ok_or_else(|| CommandError::new("unknown_job", "This download is no longer available."))?;
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
    state
        .manager
        .resume(handle)
        .map_err(|_| CommandError::new("not_paused", "This download is not paused."))
}

#[tauri::command]
fn retry_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<DownloadDto, CommandError> {
    let state = state.inner().clone();
    let (url, destination, old_handle) = {
        let jobs = lock_jobs(&state.jobs);
        let old = jobs.get(&id).ok_or_else(|| {
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
        (
            old.url.clone().ok_or_else(|| {
                CommandError::new(
                    "url_unavailable",
                    "Velo cannot decrypt the saved URL on this account.",
                )
            })?,
            old.destination.clone(),
            old.handle.clone(),
        )
    };
    if let Some(handle) = old_handle {
        state.manager.acknowledge_terminal(&handle).map_err(|_| {
            CommandError::new("not_terminal", "The finished result is not available.")
        })?;
        if let Some(old_job) = lock_jobs(&state.jobs).get_mut(&id) {
            old_job.handle = None;
        }
    }
    let (download, handle) = state.start(url, destination)?;
    lock_jobs(&state.jobs).remove(&id);
    state.persist(true);
    tauri::async_runtime::spawn(async move {
        let app_for_events = app.clone();
        state
            .monitor(handle, move |download| {
                let _ = app_for_events.emit(DOWNLOAD_UPDATED_EVENT, download);
            })
            .await;
    });
    Ok(download)
}

#[tauri::command]
fn acknowledge_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let mut jobs = lock_jobs(&state.jobs);
    let job = jobs
        .get(&id)
        .ok_or_else(|| CommandError::new("unknown_job", "This download is no longer available."))?;
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
        state.manager.acknowledge_terminal(handle).map_err(|_| {
            CommandError::new("not_terminal", "The finished result is not available.")
        })?;
    }
    jobs.remove(&id);
    drop(jobs);
    state.persist(true);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let manager = tauri::async_runtime::block_on(async { DownloadManager::new() })?;
            let store_path = app.path().app_data_dir()?.join("downloads.json");
            app.manage(AppState::new(manager, store_path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads,
            start_download,
            cancel_download,
            pause_download,
            resume_download,
            retry_download,
            acknowledge_download
        ])
        .build(tauri::generate_context!())
        .expect("failed to build ConsoleX Velo")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                let state = app.state::<AppState>().inner().clone();
                if state.exit_ready.load(Ordering::Acquire) {
                    return;
                }
                api.prevent_exit();
                if !state.shutdown_started.swap(true, Ordering::AcqRel) {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        state.manager.shutdown().await;
                        state.exit_ready.store(true, Ordering::Release);
                        app.exit(0);
                    });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::atomic::AtomicU64, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn url_command_validation_rejects_unsupported_and_credential_urls() {
        assert!(validate_url_input("https://example.test/file").is_ok());
        assert!(validate_url_input("ftp://example.test/file").is_err());
        assert!(validate_url_input("https://user:secret@example.test/file").is_err());
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
        let state = AppState::new(
            DownloadManager::new().unwrap(),
            std::env::temp_dir().join("velo-test-history.json"),
        );
        let (initial, handle) = state
            .start(
                format!("http://{address}/file"),
                destination.to_string_lossy().into_owned(),
            )
            .unwrap();
        assert_eq!(initial.file_name, "service.bin");

        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_monitor = events.clone();
        let monitor_state = state.clone();
        let monitor_handle = handle.clone();
        let monitor = tokio::spawn(async move {
            monitor_state
                .monitor(monitor_handle, move |download| {
                    events_for_monitor.lock().unwrap().push(download);
                })
                .await;
        });

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
}
