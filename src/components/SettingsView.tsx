import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { SegmentedControl } from "./ui/segmented";
import { Toggle } from "./ui/switch";
import { Icon } from "./icons";
import {
  setDefaultDirectory,
  clearDefaultDirectory,
  setGlobalSpeedLimit,
  setCloseBehavior,
  setLaunchAtStartup,
  setDownloadNotifications,
} from "../commands";
import type { SettingsDto } from "../model";

type SettingsViewProps = {
  settings: SettingsDto | null;
  onSettingsChanged: (settings: SettingsDto) => void;
  onSettingsError: (message: string) => void;
  theme: "dark" | "light";
  setTheme: (value: "dark" | "light") => void;
  reducedMotion: boolean;
  setReducedMotion: (value: boolean) => void;
};

/**
 * Preferences. The download-directory card is backed by the real settings
 * store (settings.json beside the download history): "Change…" opens the
 * native folder dialog and persists through set_default_directory, which
 * validates existence + writability in Rust before saving.
 */
export function SettingsView({
  settings,
  onSettingsChanged,
  onSettingsError,
  theme,
  setTheme,
  reducedMotion,
  setReducedMotion,
}: SettingsViewProps) {
  const [busy, setBusy] = useState(false);
  const defaultDir = settings?.defaultDownloadDir ?? null;
  const osDir = settings?.osDefaultDownloadDir ?? null;
  const effective = settings?.effectiveDownloadDir ?? null;
  const broken = Boolean(defaultDir && settings && !settings.storedDirValid);
  const [speedDraft, setSpeedDraft] = useState("");
  const closeBehavior = settings?.closeBehavior ?? "minimize_to_tray";
  const launchAtStartup = settings?.launchAtStartup ?? false;
  const downloadNotifications = settings?.downloadNotifications ?? true;
  useEffect(() => setSpeedDraft(settings?.globalSpeedLimitBytesPerSecond ? String(Math.round(settings.globalSpeedLimitBytesPerSecond / 1024)) : ""), [settings?.globalSpeedLimitBytesPerSecond]);

  async function changeFolder() {
    setBusy(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose the default download folder",
      });
      if (typeof selected === "string" && selected) {
        const next = await setDefaultDirectory(selected);
        onSettingsChanged(next);
      }
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The folder choice could not be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function useSystemFolder() {
    setBusy(true);
    try {
      const next = await clearDefaultDirectory();
      onSettingsChanged(next);
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The folder choice could not be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function changeSpeedLimit() {
    const kib = speedDraft === "" ? null : Number(speedDraft);
    if (kib !== null && (!Number.isFinite(kib) || kib < 1 || kib > 1_048_576)) return;
    setBusy(true);
    try {
      onSettingsChanged(await setGlobalSpeedLimit(kib === null ? null : kib * 1024));
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The speed limit could not be saved.");
    } finally { setBusy(false); }
  }

  async function changeCloseBehavior(behavior: "minimize_to_tray" | "exit_application") {
    setBusy(true);
    try {
      onSettingsChanged(await setCloseBehavior(behavior));
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The close behavior could not be saved.");
    } finally { setBusy(false); }
  }

  async function changeLaunchAtStartup(enabled: boolean) {
    setBusy(true);
    try {
      onSettingsChanged(await setLaunchAtStartup(enabled));
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The startup preference could not be saved.");
    } finally { setBusy(false); }
  }

  async function changeDownloadNotifications(enabled: boolean) {
    setBusy(true);
    try {
      onSettingsChanged(await setDownloadNotifications(enabled));
    } catch (cause) {
      onSettingsError(typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : "The notification preference could not be saved.");
    } finally { setBusy(false); }
  }

  return (
    <section className="settings-view" aria-label="Settings">
      <div className="settings-heading">
        <span className="page-eyebrow">Configuration</span>
        <h1>Settings</h1>
        <p>Where files land and how Velo looks — applied immediately, stored locally.</p>
      </div>

      <div className="settings-card">
        <div className="settings-group-label micro-label">Storage</div>
        <div className="settings-item">
          <div className="settings-icon"><Icon name="folder" /></div>
          <div className="settings-copy">
            <strong>Default download folder</strong>
            <span className="path-text" title={defaultDir ?? undefined}>
              {defaultDir ? "Saved choice" : "System Downloads folder"}
              {effective ? "" : " — not resolved yet"}
            </span>
            <span className="settings-path path-text" title={effective ?? undefined}>
              {effective ?? "Pick a folder before starting downloads"}
            </span>
          </div>
          <div className="settings-actions">
            {defaultDir && (
              <button type="button" className="secondary-button" onClick={() => void useSystemFolder()} disabled={busy}>
                Use system folder
              </button>
            )}
            <button type="button" className="primary-button" onClick={() => void changeFolder()} disabled={busy}>
              {busy ? "Saving…" : "Change…"}
            </button>
          </div>
        </div>
        {broken && (
          <div className="settings-warning" role="alert">
            <Icon name="alert" size={15} />
            <span>
              The saved folder no longer exists or is not writable. New downloads fall back to
              {osDir ? " the system Downloads folder" : " nothing until you pick a folder"} — nothing was lost.
              Choose a new folder above to fix it.
            </span>
          </div>
        )}
        <div className="settings-separator" />
        <div className="settings-group-label micro-label">Transfers</div>
        <div className="settings-item">
          <div className="settings-icon behavior"><Icon name="drive" /></div>
          <div className="settings-copy">
            <strong>Global download speed limit</strong>
            <span>Limits combined transfer speed across active downloads and applies to new jobs. Leave blank for unlimited.</span>
          </div>
          <label className="settings-speed-limit"><input aria-label="Global speed limit in KiB/s" type="number" min="1" max="1048576" value={speedDraft} disabled={busy} onChange={(event) => setSpeedDraft(event.target.value)} onBlur={() => void changeSpeedLimit()} onKeyDown={(event) => { if (event.key === "Enter") { event.currentTarget.blur(); } }} /><span>KiB/s</span></label>
        </div>
        <div className="settings-separator" />
        <div className="settings-group-label micro-label">System</div>
        <div className="settings-item">
          <div className="settings-icon behavior"><Icon name="shield" /></div>
          <div className="settings-copy">
            <strong>Close button behavior</strong>
            <span>Choose what happens when you close the window while downloads are active.</span>
          </div>
          <SegmentedControl
            aria-label="Close button behavior"
            className="segmented-control"
            value={closeBehavior}
            onChange={changeCloseBehavior}
            options={[
              { value: "minimize_to_tray", label: "Minimize to Tray", icon: <Icon name="minimize" size={14} /> },
              { value: "exit_application", label: "Exit Application", icon: <Icon name="close" size={14} /> },
            ]}
          />
        </div>
        <div className="settings-separator" />
        <div className="settings-item">
          <div className="settings-icon behavior"><Icon name="clock" /></div>
          <div className="settings-copy">
            <strong>Launch Velo at system startup</strong>
            <span>Starts Velo automatically when you log in to Windows.</span>
          </div>
          <Toggle checked={launchAtStartup} onCheckedChange={changeLaunchAtStartup} aria-label="Launch Velo at system startup" disabled={busy} />
        </div>
        <div className="settings-separator" />
        <div className="settings-group-label micro-label">Notifications</div>
        <div className="settings-item">
          <div className="settings-icon behavior"><Icon name="bell" /></div>
          <div className="settings-copy">
            <strong>Download notifications</strong>
            <span>Show a native notification when a download completes or fails.</span>
          </div>
          <Toggle checked={downloadNotifications} onCheckedChange={changeDownloadNotifications} aria-label="Download notifications" disabled={busy} />
        </div>
        <div className="settings-separator" />
        <div className="settings-group-label micro-label">Appearance</div>
        <div className="settings-item">
          <div className="settings-icon appearance"><Icon name="sun" /></div>
          <div className="settings-copy">
            <strong>Appearance</strong>
            <span>A dark and a light palette, tuned for long sessions.</span>
          </div>
          <SegmentedControl
            aria-label="Theme"
            className="segmented-control"
            value={theme}
            onChange={setTheme}
            options={[
              { value: "dark", label: "Dark", icon: <Icon name="moon" size={14} /> },
              { value: "light", label: "Light", icon: <Icon name="sun" size={14} /> },
            ]}
          />
        </div>
        <div className="settings-separator" />
        <div className="settings-group-label micro-label">Motion</div>
        <div className="settings-item">
          <div className="settings-icon motion"><Icon name="spark" /></div>
          <div className="settings-copy">
            <strong>Reduced motion</strong>
            <span>Removes interface transitions and animation.</span>
          </div>
          <Toggle checked={reducedMotion} onCheckedChange={setReducedMotion} aria-label="Reduced motion" />
        </div>
      </div>

      <div className="privacy-note">
        <div className="privacy-lock"><Icon name="shield" size={15} /></div>
        <div>
          <strong>Private by design</strong>
          <span>History stays on this device. Saved download URLs are protected with your Windows account, and Velo makes no network requests beyond your downloads.</span>
        </div>
      </div>
      <p className="settings-footnote">
        If Velo closes unexpectedly, interrupted transfers return as retryable records. When the server supports it,
        retries continue from the last verified checkpoint; otherwise they restart from byte zero.
      </p>
    </section>
  );
}
