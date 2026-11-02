//! Tauri shell for the ConsoleX Velo download service.
//!
//! All state, recovery, retry, and persistence logic lives in the tauri-free
//! `service` and `settings` modules so it can be tested without the webview,
//! mirroring the engine's boundary. This file only adapts the webview's
//! commands and lifecycle events onto those services, and owns the desktop
//! integration surfaces: the custom-frame window lifecycle, the system tray,
//! and the single-instance guard.

mod inspect;
mod persistence;
mod service;
mod settings;

use std::sync::Arc;

use consolex_download_engine::DownloadManager;
use service::{AppState, UiEvent};
use settings::{CloseBehavior, SettingsState};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, RunEvent, State, WindowEvent,
};
use tauri_plugin_autostart::{AutoLaunchManager, MacosLauncher};
use tauri_plugin_notification::NotificationExt;

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

/// Inspects a URL without starting a download: HEAD first, bounded Range
/// fallback, headers only. Returns metadata such as the final URL, size, and
/// content type. Never touches the download manager or its concurrency slots.
#[tauri::command]
async fn inspect_url(url: String) -> Result<inspect::InspectMetadata, inspect::InspectError> {
    inspect::inspect_url(&url).await
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

#[tauri::command]
fn set_close_behavior(
    settings: State<'_, SettingsState>,
    behavior: String,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    let behavior = match behavior.as_str() {
        "minimize_to_tray" => CloseBehavior::MinimizeToTray,
        "exit_application" => CloseBehavior::ExitApplication,
        _ => {
            return Err(settings::SettingsError::invalid_setting(
                "Choose 'minimize_to_tray' or 'exit_application'.",
            ))
        }
    };
    settings.set_close_behavior(behavior)
}

#[tauri::command]
fn set_launch_at_startup(
    settings: State<'_, SettingsState>,
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    // Attempt OS registration first. Only persist the preference if the
    // OS actually honors it, so the stored value and the real registration
    // state never diverge and a failure is never reported as success.
    if let Some(autostart) = app.try_state::<AutoLaunchManager>() {
        let result = if enabled {
            autostart.enable()
        } else {
            autostart.disable()
        };
        if let Err(error) = result {
            return Err(settings::SettingsError::invalid_setting(format!(
                "Could not {0} Velo at system startup: {1}",
                if enabled { "register" } else { "unregister" },
                error
            )));
        }
    }
    settings.set_launch_at_startup(enabled)
}

#[tauri::command]
fn is_launch_at_startup(app: tauri::AppHandle) -> Result<bool, settings::SettingsError> {
    app.try_state::<AutoLaunchManager>()
        .map(|autostart| autostart.is_enabled())
        .transpose()
        .map(|maybe_enabled| maybe_enabled.unwrap_or(false))
        .map_err(|error| {
            settings::SettingsError::invalid_setting(format!(
                "Could not read startup registration state: {error}"
            ))
        })
}

#[tauri::command]
fn set_download_notifications(
    settings: State<'_, SettingsState>,
    enabled: bool,
) -> Result<settings::SettingsDto, settings::SettingsError> {
    settings.set_download_notifications(enabled)
}

#[tauri::command]
fn pause_all_downloads(state: State<'_, AppState>) -> Result<usize, service::CommandError> {
    Ok(state.pause_all_downloads())
}

#[tauri::command]
fn resume_all_downloads(state: State<'_, AppState>) -> Result<usize, service::CommandError> {
    Ok(state.resume_all_downloads())
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "velo-open", "Open Velo", true, None::<&str>)?;
    let pause_all = MenuItem::with_id(
        app,
        "velo-pause-all",
        "Pause All Downloads",
        true,
        None::<&str>,
    )?;
    let resume_all = MenuItem::with_id(
        app,
        "velo-resume-all",
        "Resume All Downloads",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, "velo-exit", "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause_all, &resume_all, &separator, &exit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Left click restores the window; the menu is a right-click affair,
        // matching established Windows download managers.
        .show_menu_on_left_click(false)
        .tooltip("ConsoleX Velo")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "velo-open" => show_main_window(app),
            "velo-pause-all" => {
                let _ = app.state::<AppState>().pause_all_downloads();
            }
            "velo-resume-all" => {
                let _ = app.state::<AppState>().resume_all_downloads();
            }
            // An explicit, orderly exit: the ExitRequested handler below
            // shuts the engine down before the process really ends.
            "velo-exit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            match event {
                // Left click restores and focuses the window.
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    show_main_window(tray.app_handle());
                }
                // Double-click also restores and focuses the window.
                TrayIconEvent::DoubleClick { .. } => {
                    show_main_window(tray.app_handle());
                }
                _ => {}
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
        .plugin(tauri_plugin_clipboard_manager::init())
        // Windows startup registration: Launch Velo at system startup.
        .plugin(tauri_plugin_autostart::init(MacosLauncher::default(), None))
        // Native desktop notifications for terminal download events.
        .plugin(tauri_plugin_notification::init())
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
            // Honor the persisted startup preference on every launch so the
            // stored value and the actual OS registration state stay aligned.
            if settings.get().launch_at_startup {
                if let Some(autostart) = app.try_state::<AutoLaunchManager>() {
                    let _ = autostart.enable();
                }
            }
            let handle = app.handle().clone();
            let publish: Arc<dyn Fn(UiEvent) + Send + Sync> = Arc::new(move |event| match event {
                UiEvent::Updated(download) => {
                    let _ = handle.emit(DOWNLOAD_UPDATED_EVENT, download.clone());
                    // Native desktop notification for terminal download events.
                    // At most one notification per terminal event because the
                    // service only emits UiEvent::Updated once per terminal
                    // transition (see service::monitor).
                    if matches!(
                        download.state,
                        service::UiDownloadState::Completed | service::UiDownloadState::Failed
                    ) {
                        if let Some(settings) = handle.try_state::<SettingsState>() {
                            if settings.get().download_notifications {
                                let title = match download.state {
                                    service::UiDownloadState::Completed => "Download complete",
                                    service::UiDownloadState::Failed => "Download failed",
                                    _ => unreachable!(),
                                };
                                let body = if download.file_name.is_empty() {
                                    "A download has finished."
                                } else {
                                    &download.file_name
                                };
                                let _ = handle
                                    .notification()
                                    .builder()
                                    .title(title)
                                    .body(body)
                                    .show();
                            }
                        }
                    }
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
            inspect_url,
            cancel_download,
            pause_download,
            resume_download,
            retry_download,
            acknowledge_download,
            get_settings,
            set_default_directory,
            clear_default_directory,
            set_global_speed_limit,
            set_close_behavior,
            set_launch_at_startup,
            is_launch_at_startup,
            set_download_notifications,
            pause_all_downloads,
            resume_all_downloads,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let settings = window.app_handle().state::<SettingsState>();
                match settings.get().close_behavior {
                    CloseBehavior::MinimizeToTray => {
                        // Hiding the window never interrupts active downloads.
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    CloseBehavior::ExitApplication => {
                        // Allow the close to proceed: ExitRequested below
                        // performs an orderly engine shutdown.
                    }
                }
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
