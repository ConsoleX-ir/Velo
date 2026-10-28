import { invoke } from "@tauri-apps/api/core";
import type { DownloadDto, SettingsDto } from "./model";

/**
 * The complete typed command surface Velo's webview is allowed to call.
 * Every function maps 1:1 onto a #[tauri::command] registered in
 * src-tauri/src/lib.rs; there is no generic shell, filesystem, or network
 * escape hatch here by design.
 */

export const listDownloads = (): Promise<DownloadDto[]> => invoke("list_downloads");

export const getHistoryNotice = (): Promise<string | null> => invoke("get_history_notice");

export const getSettings = (): Promise<SettingsDto> => invoke("get_settings");

export const setDefaultDirectory = (directory: string): Promise<SettingsDto> =>
  invoke("set_default_directory", { directory });

export const clearDefaultDirectory = (): Promise<SettingsDto> => invoke("clear_default_directory");

export const setGlobalSpeedLimit = (bytesPerSecond: number | null): Promise<SettingsDto> =>
  invoke("set_global_speed_limit", { bytesPerSecond });

export const startDownload = (url: string, directory: string, fileName: string, speedLimitBytesPerSecond: number | null): Promise<DownloadDto> =>
  invoke("start_download", { url, directory, fileName, speedLimitBytesPerSecond });

export const scheduleDownload = (url: string, directory: string, fileName: string, scheduledAtUnixMs: number, speedLimitBytesPerSecond: number | null): Promise<DownloadDto> =>
  invoke("schedule_download", { url, directory, fileName, scheduledAtUnixMs, speedLimitBytesPerSecond });

export const editDownloadSchedule = (id: string, generation: number, scheduledAtUnixMs: number): Promise<DownloadDto> =>
  invoke("edit_download_schedule", { id, generation, scheduledAtUnixMs });

export const cancelDownload = (id: string, generation: number): Promise<void> =>
  invoke("cancel_download", { id, generation });

export const pauseDownload = (id: string, generation: number): Promise<void> =>
  invoke("pause_download", { id, generation });

export const resumeDownload = (id: string, generation: number): Promise<void> =>
  invoke("resume_download", { id, generation });

export const retryDownload = (id: string, generation: number): Promise<DownloadDto> =>
  invoke("retry_download", { id, generation });

export const acknowledgeDownload = (id: string, generation: number): Promise<void> =>
  invoke("acknowledge_download", { id, generation });
