use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use consolex_download_engine::{
    CancelDisposition, ControlError, DownloadError, DownloadHandle, DownloadId, DownloadManager,
    DownloadRequest, DownloadState, NetworkFailureKind, StartError,
};
use serde::Serialize;
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
}

struct UiJob {
    handle: DownloadHandle,
    download: DownloadDto,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum UiDownloadState {
    Queued,
    Preparing,
    Downloading,
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
    fn new(manager: DownloadManager) -> Self {
        Self {
            manager,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            exit_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    fn start(
        &self,
        url: String,
        destination: String,
    ) -> Result<(DownloadDto, DownloadHandle), CommandError> {
        validate_url_input(&url)?;
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
                url,
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
                handle: handle.clone(),
                download: dto.clone(),
            },
        );
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
    if job.handle.generation() != handle.generation() {
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
    if job.handle.generation() != generation {
        return Err(CommandError::new(
            "stale_job",
            "This download view is out of date.",
        ));
    }
    match state.manager.cancel(&job.handle) {
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
fn acknowledge_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let mut jobs = lock_jobs(&state.jobs);
    let job = jobs
        .get(&id)
        .ok_or_else(|| CommandError::new("unknown_job", "This download is no longer available."))?;
    if job.handle.generation() != generation {
        return Err(CommandError::new(
            "stale_job",
            "This download view is out of date.",
        ));
    }
    if !matches!(
        job.download.state,
        UiDownloadState::Completed | UiDownloadState::Failed | UiDownloadState::Cancelled
    ) {
        return Err(CommandError::new(
            "not_terminal",
            "Only finished downloads can be dismissed.",
        ));
    }
    state
        .manager
        .acknowledge_terminal(&job.handle)
        .map_err(|_| CommandError::new("not_terminal", "The finished result is not available."))?;
    jobs.remove(&id);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let manager = tauri::async_runtime::block_on(async { DownloadManager::new() })?;
            app.manage(AppState::new(manager));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads,
            start_download,
            cancel_download,
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
        let state = AppState::new(DownloadManager::new().unwrap());
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
