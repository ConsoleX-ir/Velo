//! Tauri shell for the ConsoleX Velo download service.
//!
//! All state, recovery, retry, and persistence logic lives in the tauri-free
//! `service` and `settings` modules so it can be tested without the webview,
//! mirroring the engine's boundary. This file only adapts the webview's
//! commands and lifecycle events onto those services, and owns the desktop
//! integration surfaces: the custom-frame window lifecycle, the system tray,
//! and the single-instance guard.

mod persistence;
mod service;
mod settings;

use std::sync::Arc;

use consolex_download_engine::DownloadManager;
use service::{AppState, UiEvent};
use settings::SettingsState;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, RunEvent, State, WindowEvent,
};

const DOWNLOAD_UPDATED_EVENT: &str = "download-updated";
const DOWNLOAD_REMOVED_EVENT: &str = "download-removed";
const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "velo-tray";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.hide();
    }
}

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Vec<service::DownloadDto> {
    state.list_downloads()
}

#[tauri::command]
async fn start_download(
    state: State<'_, AppState>,
    url: String,
    directory: String,
    file_name: String,
    speed_limit_bytes_per_second: Option<u64>,
) -> Result<service::DownloadDto, service::CommandError> {
    state
        .inner()
        .clone()
        .begin_download(url, directory, file_name, speed_limit_bytes_per_second)
}

#[tauri::command]
fn schedule_download(
    state: State<'_, AppState>,
    url: String,
    directory: String,
    file_name: String,
    scheduled_at_unix_ms: u64,
    speed_limit_bytes_per_second: Option<u64>,
) -> Result<service::DownloadDto, service::CommandError> {
    state.inner().clone().schedule_download(
        url,
        directory,
        file_name,
        scheduled_at_unix_ms,
        speed_limit_bytes_per_second,
    )
}

#[tauri::command]
fn edit_download_schedule(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    scheduled_at_unix_ms: u64,
) -> Result<service::DownloadDto, service::CommandError> {
    state
        .inner()
        .clone()
        .edit_schedule(&id, generation, scheduled_at_unix_ms)
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

#[tauri::command]
fn get_settings(settings: State<'_, SettingsState>) -> settings::SettingsDto {
    settings.get()
}

#[tauri::command]
fn set_default_directory(
    settings: State<'_, SettingsState>,
    directory: String,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    settings.set_default(&directory)
}

#[tauri::command]
fn clear_default_directory(
    settings: State<'_, SettingsState>,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    settings.clear_default()
}

#[tauri::command]
fn set_global_speed_limit(
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
    bytes_per_second: Option<u64>,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    let updated = settings.set_speed_limit(bytes_per_second)?;
    state.set_speed_limit(bytes_per_second);
    Ok(updated)
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "velo-show", "Show Velo", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "velo-hide", "Hide Velo", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, "velo-exit", "Exit Velo", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &separator, &exit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Left click restores the window; the menu is a right-click affair,
        // matching established Windows download managers.
        .show_menu_on_left_click(false)
        .tooltip("ConsoleX Velo")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "velo-show" => show_main_window(app),
            "velo-hide" => hide_main_window(app),
            // An explicit, orderly exit: the ExitRequested handler below
            // shuts the engine down before the process really ends.
            "velo-exit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        });
    // The bundled window icon doubles as the tray icon, so no separate asset
    // has to stay legible at both window and tray sizes by hand.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses the running
        // window instead of starting a second engine, tray, or history file.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let manager = tauri::async_runtime::block_on(async { DownloadManager::new() })?;
            let data_dir = app.path().app_data_dir()?;
            let store_path = data_dir.join("downloads.json");
            let settings = SettingsState::build(
                data_dir.join("settings.json"),
                app.path().download_dir().ok(),
            );
            manager.set_global_speed_limit_bytes_per_second(
                settings.get().global_speed_limit_bytes_per_second,
            );
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
            let state = AppState::build(manager, store_path, publish, service::RETRY_BACKOFF_BASE);
            app.manage(state);
            app.manage(settings);
            build_tray(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads,
            start_download,
            schedule_download,
            edit_download_schedule,
            get_history_notice,
            cancel_download,
            pause_download,
            resume_download,
            retry_download,
            acknowledge_download,
            get_settings,
            set_default_directory,
            clear_default_directory,
            set_global_speed_limit
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Closing the window hides it to the tray while active
                // downloads keep running; the tray's Exit entry ends the app.
                api.prevent_close();
                let _ = window.hide();
            }
        })
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
