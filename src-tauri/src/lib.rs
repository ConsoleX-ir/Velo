//! Tauri shell for the ConsoleX Velo download service.
//!
//! All state, recovery, retry, and persistence logic lives in the tauri-free
//! `service` module so it can be tested without the webview, mirroring the
//! engine's boundary. This file only adapts the webview's commands and
//! lifecycle events onto that service.

mod persistence;
mod service;

use std::sync::Arc;

use consolex_download_engine::DownloadManager;
use service::{AppState, UiEvent};
use tauri::{Emitter, Manager, RunEvent, State};

const DOWNLOAD_UPDATED_EVENT: &str = "download-updated";
const DOWNLOAD_REMOVED_EVENT: &str = "download-removed";

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Vec<service::DownloadDto> {
    state.list_downloads()
}

#[tauri::command]
async fn start_download(
    state: State<'_, AppState>,
    url: String,
    destination: String,
) -> Result<service::DownloadDto, service::CommandError> {
    state.inner().clone().begin_download(url, destination)
}

/// User-facing one-shot message when the saved history needed repair.
#[tauri::command]
fn get_history_notice(state: State<'_, AppState>) -> Option<String> {
    state.take_history_notice()
}

#[tauri::command]
fn cancel_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), service::CommandError> {
    state.inner().clone().cancel_download(&id, generation)
}

#[tauri::command]
fn pause_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), service::CommandError> {
    state.inner().clone().pause_download(&id, generation)
}

#[tauri::command]
fn resume_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), service::CommandError> {
    state.inner().clone().resume_download(&id, generation)
}

#[tauri::command]
async fn retry_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<service::DownloadDto, service::CommandError> {
    state.inner().clone().retry_download(&id, generation)
}

#[tauri::command]
fn acknowledge_download(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
) -> Result<(), service::CommandError> {
    state.inner().clone().acknowledge_download(&id, generation)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let manager = tauri::async_runtime::block_on(async { DownloadManager::new() })?;
            let store_path = app.path().app_data_dir()?.join("downloads.json");
            let handle = app.handle().clone();
            let publish: Arc<dyn Fn(UiEvent) + Send + Sync> = Arc::new(move |event| match event {
                UiEvent::Updated(download) => {
                    let _ = handle.emit(DOWNLOAD_UPDATED_EVENT, download);
                }
                UiEvent::Removed { id, generation } => {
                    // A named object, not a tuple: the webview reads payload.id.
                    let _ = handle.emit(
                        DOWNLOAD_REMOVED_EVENT,
                        serde_json::json!({ "id": id, "generation": generation }),
                    );
                }
            });
            app.manage(AppState::build(
                manager,
                store_path,
                publish,
                service::RETRY_BACKOFF_BASE,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads,
            start_download,
            get_history_notice,
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
                if state.exit_ready.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                api.prevent_exit();
                if !state
                    .shutdown_started
                    .swap(true, std::sync::atomic::Ordering::AcqRel)
                {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        state.manager.shutdown().await;
                        // Cancelled-at-exit jobs may still be flushing their
                        // terminal snapshots through the throttled writer; a
                        // final forced save pins whatever is current so the
                        // next launch restores honest, retryable records.
                        state.persist(true);
                        state
                            .exit_ready
                            .store(true, std::sync::atomic::Ordering::Release);
                        app.exit(0);
                    });
                }
            }
        });
}
