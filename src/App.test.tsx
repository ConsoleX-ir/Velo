import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, listenMock, windowMock, openDialogMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  windowMock: {
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
    isMaximized: vi.fn(async () => false),
    onResized: vi.fn(async () => () => undefined),
  },
  openDialogMock: vi.fn(),
}));

let eventListener: ((event: { payload: unknown }) => void) | undefined;
let removedListener: ((event: { payload: { id: string; generation: number } }) => void) | undefined;

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => windowMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialogMock, save: vi.fn() }));

import App from "./App";
import type { DownloadDto, SettingsDto } from "./model";

const defaultSettings: SettingsDto = {
  defaultDownloadDir: null,
  storedDirValid: false,
  osDefaultDownloadDir: "C:\\Users\\me\\Downloads",
  effectiveDownloadDir: "C:\\Users\\me\\Downloads",
};

const download = (patch: Partial<DownloadDto>): DownloadDto => ({
  id: "job-1",
  generation: 1,
  revision: 1,
  order: 1,
  fileName: "archive.zip",
  destination: "C:\\Users\\me\\Downloads\\archive.zip",
  state: "downloading",
  bytesWritten: 5,
  totalBytes: 10,
  speedBytesPerSecond: 4,
  etaSeconds: 2,
  error: null,
  ...patch,
});

/** React-controlled inputs ignore plain value assignment; use the native setter. */
function typeInput(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("application shell", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("data-motion");
    invokeMock.mockImplementation((command: unknown) => {
      if (command === "get_settings") return Promise.resolve(defaultSettings);
      if (command === "get_history_notice") return Promise.resolve(null);
      return Promise.resolve([]);
    });
    listenMock.mockImplementation(async (event: string, callback: (payload: unknown) => void) => {
      if (event === "download-updated") {
        eventListener = callback as (event: { payload: unknown }) => void;
      } else if (event === "download-removed") {
        removedListener = callback as (event: { payload: { id: string; generation: number } }) => void;
      }
      return () => undefined;
    });
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(App));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("persists a deliberately selectable light theme", async () => {
    const settings = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Settings"));
    expect(settings).toBeDefined();
    await act(async () => settings?.click());

    const light = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Light"));
    expect(light).toBeDefined();
    await act(async () => light?.click());

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(localStorage.getItem("velo-theme")).toBe("light");
  });

  it("persists reduced motion and exposes the preference as a switch", async () => {
    const settings = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Settings"));
    await act(async () => settings?.click());

    const motion = container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="Reduced motion"]');
    expect(motion?.getAttribute("aria-checked")).toBe("false");
    await act(async () => motion?.click());

    expect(document.documentElement.dataset.motion).toBe("reduced");
    expect(localStorage.getItem("velo-motion")).toBe("reduced");
    expect(motion?.getAttribute("aria-checked")).toBe("true");
  });

  it("drives the real native window from the custom title bar controls", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Minimize window"]')?.click());
    expect(windowMock.minimize).toHaveBeenCalledTimes(1);

    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Maximize window"]')?.click());
    expect(windowMock.toggleMaximize).toHaveBeenCalledTimes(1);

    await act(async () => {
      const close = container.querySelector<HTMLButtonElement>('[aria-label^="Close window"]');
      close?.click();
    });
    expect(windowMock.close).toHaveBeenCalledTimes(1);
  });

  it("returns keyboard focus to the opener when the add dialog closes", async () => {
    const add = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Add download"));
    expect(add).toBeDefined();
    await act(async () => { add?.focus(); add?.click(); });
    const cancel = [...container.querySelectorAll("button")].find((button) => button.textContent === "Cancel");
    expect(cancel).toBeDefined();
    await act(async () => cancel?.click());
    expect(document.activeElement).toBe(add);
  });

  it("starts downloads through the native command with folder and file name kept separate", async () => {
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Add download"))?.click();
    });
    const url = container.querySelector<HTMLInputElement>("#download-url");
    expect(url).toBeDefined();
    await act(async () => typeInput(url!, "https://example.test/setup.exe"));
    const form = container.querySelector("form");
    expect(form).toBeDefined();
    invokeMock.mockResolvedValueOnce(download({ id: "fresh", state: "preparing" }));
    await act(async () => {
      form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("start_download", {
      url: "https://example.test/setup.exe",
      directory: "C:\\Users\\me\\Downloads",
      fileName: "setup.exe",
      speedLimitBytesPerSecond: null,
    });
    expect(container.textContent).toContain("Download started");
  });

  it("schedules downloads through the native command with a local time converted to an absolute instant", async () => {
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Add download"))?.click();
    });
    const url = container.querySelector<HTMLInputElement>("#download-url");
    await act(async () => typeInput(url!, "https://example.test/nightly.zip"));
    const date = new Date(Date.now() + 3_600_000);
    const pad = (part: number) => String(part).padStart(2, "0");
    const localValue = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
    const schedule = container.querySelector<HTMLInputElement>("#download-schedule");
    await act(async () => typeInput(schedule!, localValue));
    await act(async () => typeInput(container.querySelector<HTMLInputElement>("#download-speed-limit")!, "64"));
    const expectedMs = new Date(localValue).getTime();
    invokeMock.mockResolvedValueOnce(download({ id: "scheduled", state: "scheduled", scheduledAtUnixMs: expectedMs }));
    await act(async () => {
      container.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("schedule_download", {
      url: "https://example.test/nightly.zip",
      directory: "C:\\Users\\me\\Downloads",
      fileName: "nightly.zip",
      scheduledAtUnixMs: expectedMs,
      speedLimitBytesPerSecond: 64 * 1024,
    });
    expect(container.textContent).toContain("Download scheduled");
  });

  it("lets one download override the folder without touching the default", async () => {
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Add download"))?.click();
    });
    expect(container.textContent).toContain("Default location from Settings");
    openDialogMock.mockResolvedValueOnce("D:\\Archive");
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent === "Browse…")?.click();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("This download only — the default stays unchanged");
    expect(container.textContent).toContain("D:\\Archive");

    const url = container.querySelector<HTMLInputElement>("#download-url");
    await act(async () => typeInput(url!, "https://example.test/pack.zip"));
    invokeMock.mockResolvedValueOnce(download({ id: "fresh", fileName: "pack.zip" }));
    await act(async () => {
      container.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("start_download", {
      url: "https://example.test/pack.zip",
      directory: "D:\\Archive",
      fileName: "pack.zip",
      speedLimitBytesPerSecond: null,
    });
  });

  it("keeps a newer event when the initial snapshot arrives afterward", async () => {
    await act(async () => root.unmount());
    let resolveSnapshot!: (items: DownloadDto[]) => void;
    invokeMock.mockImplementation((command: unknown) => {
      if (command === "list_downloads") return new Promise((resolve) => { resolveSnapshot = resolve; });
      if (command === "get_settings") return Promise.resolve(defaultSettings);
      return Promise.resolve(null);
    });
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(App));
      await Promise.resolve();
    });

    await act(async () => eventListener?.({ payload: download({ revision: 2, bytesWritten: 8 }) }));
    await act(async () => resolveSnapshot([download({ revision: 1, bytesWritten: 2 })]));

    expect(container.textContent).toContain("8 B of 10 B");
    expect(container.textContent).not.toContain("2 B of 10 B");
  });

  it("never presents terminal speed as a live measurement", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "done", state: "completed", bytesWritten: 10, speedBytesPerSecond: 512,
    }) }));
    expect(container.querySelector(".row-spark")).toBeNull();
    expect(container.textContent).not.toContain("512 B/s");
  });

  it("renders an inline speed sparkline from a real-shaped active download event", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "active", revision: 2, bytesWritten: 512, totalBytes: null, speedBytesPerSecond: 512,
    }) }));
    // A second sample one sampling interval later completes the sparkline.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1_050));
    });
    await act(async () => eventListener?.({ payload: download({
      id: "active", revision: 3, bytesWritten: 1_024, totalBytes: null, speedBytesPerSecond: 768,
    }) }));
    expect(container.querySelector(".row-spark")?.getAttribute("role")).toBe("img");
    expect(container.querySelector(".row-rate")?.textContent).toBe("768 B/s");
    expect(container.textContent).toContain("size unknown");
  }, 5_000);

  it("shows where each transfer is saved", async () => {
    await act(async () => eventListener?.({ payload: download({ id: "path" }) }));
    const destination = container.querySelector(".row-destination .path-text");
    expect(destination?.getAttribute("title")).toBe("C:\\Users\\me\\Downloads\\archive.zip");
    expect(destination?.textContent).toBe("C:\\Users\\me\\Downloads");
  });

  it("sends pause and resume commands for the current download generation", async () => {
    await act(async () => eventListener?.({ payload: download({ generation: 4 }) }));
    const pause = container.querySelector<HTMLButtonElement>('[aria-label="Pause archive.zip"]');
    await act(async () => pause?.click());
    expect(invokeMock).toHaveBeenCalledWith("pause_download", { id: "job-1", generation: 4 });

    await act(async () => eventListener?.({ payload: download({ generation: 4, revision: 2, state: "paused" }) }));
    const resume = container.querySelector<HTMLButtonElement>('[aria-label="Resume archive.zip"]');
    await act(async () => resume?.click());
    expect(invokeMock).toHaveBeenCalledWith("resume_download", { id: "job-1", generation: 4 });
  });

  it("renders a retrying download with only a cancel affordance wired to cancel_download", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "job-9", generation: 3, revision: 5, state: "retrying",
      bytesWritten: 100, totalBytes: 400, speedBytesPerSecond: null, etaSeconds: null,
      error: "connection reset",
    }) }));

    expect(container.querySelector(".state-pill.retrying")?.textContent).toContain("Retrying");
    expect(container.textContent).toContain("connection reset");

    const cancel = container.querySelector<HTMLButtonElement>('[aria-label="Cancel archive.zip"]');
    expect(cancel).toBeDefined();
    expect(container.querySelector('[aria-label^="Pause "]')).toBeNull();
    expect(container.querySelector('[aria-label^="Resume "]')).toBeNull();
    expect(container.querySelector('[aria-label^="Retry "]')).toBeNull();
    expect(container.querySelector('[aria-label^="Remove "]')).toBeNull();

    await act(async () => cancel?.click());
    expect(invokeMock).toHaveBeenCalledWith("cancel_download", { id: "job-9", generation: 3 });
  });

  it("offers the same actions in a right-click menu", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "job-7", generation: 2, revision: 3, state: "interrupted", error: "Velo closed before this transfer finished.",
    }) }));
    const row = container.querySelector("article.download-row");
    expect(row).toBeDefined();
    await act(async () => {
      row?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 60 }));
      await Promise.resolve();
    });
    const menu = container.querySelector('[role="menu"]');
    expect(menu).toBeDefined();
    const retry = menu?.querySelector<HTMLButtonElement>("button");
    expect(retry?.textContent).toContain("Retry");
    invokeMock.mockResolvedValueOnce(download({ id: "job-7b", generation: 3, revision: 1 }));
    await act(async () => retry?.click());
    expect(invokeMock).toHaveBeenCalledWith("retry_download", { id: "job-7", generation: 2 });
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("edits a scheduled download through the context menu and native command", async () => {
    const scheduledAtUnixMs = Date.now() + 3_600_000;
    await act(async () => eventListener?.({ payload: download({ state: "scheduled", scheduledAtUnixMs }) }));
    const row = container.querySelector("article.download-row");
    await act(async () => row?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 60 })));
    const edit = [...container.querySelectorAll<HTMLButtonElement>('[role="menu"] button')].find((button) => button.textContent?.includes("Edit schedule"));
    await act(async () => edit?.click());
    const input = container.querySelector<HTMLInputElement>("#edit-schedule-time");
    expect(input).toBeDefined();
    const date = new Date(Date.now() + 7_200_000);
    const pad = (part: number) => String(part).padStart(2, "0");
    const localValue = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
    await act(async () => typeInput(input!, localValue));
    const expectedMs = new Date(localValue).getTime();
    invokeMock.mockResolvedValueOnce(download({ state: "scheduled", revision: 2, scheduledAtUnixMs: expectedMs }));
    await act(async () => {
      container.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("edit_download_schedule", { id: "job-1", generation: 1, scheduledAtUnixMs: expectedMs });
  });

  it("moves the downloads filter when ArrowRight is pressed on the selected radio", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "done", state: "completed", bytesWritten: 20, totalBytes: 20, speedBytesPerSecond: null, etaSeconds: null,
    }) }));
    expect(container.textContent).toContain("archive.zip");

    const selected = () => container.querySelector<HTMLButtonElement>('[role="radiogroup"][aria-label="Filter downloads"] [role="radio"][aria-checked="true"]');
    expect(selected()?.textContent).toBe("All");

    await act(async () => {
      selected()?.focus();
      selected()?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
      await Promise.resolve();
    });

    expect(selected()?.textContent).toBe("Active");
    expect(container.textContent).not.toContain("archive.zip");
  });

  it("links a tooltip to the pause control after keyboard focus settles", async () => {
    await act(async () => eventListener?.({ payload: download({ generation: 4 }) }));
    const pause = container.querySelector<HTMLButtonElement>('[aria-label="Pause archive.zip"]');
    expect(pause).toBeDefined();

    await act(async () => {
      pause?.focus();
      await new Promise((resolve) => setTimeout(resolve, 350));
    });

    const tooltip = container.querySelector('[role="tooltip"]');
    expect(tooltip).toBeDefined();
    expect(pause?.getAttribute("aria-describedby")).toBe(tooltip?.id);
  });

  it("reports command failures as dismissable error toasts instead of banner entries", async () => {
    await act(async () => eventListener?.({ payload: download({ generation: 4 }) }));
    invokeMock.mockRejectedValueOnce("engine refused to pause");

    await act(async () => {
      container.querySelector<HTMLButtonElement>('[aria-label="Pause archive.zip"]')?.click();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const alert = container.querySelector(".toast-card[role='alert']");
    expect(alert).toBeDefined();
    expect(alert?.textContent).toContain("engine refused to pause");
    expect(container.querySelector(".connection-banner")).toBeNull();

    const close = alert?.querySelector<HTMLButtonElement>('button[aria-label="Dismiss notification"]');
    expect(close).toBeDefined();
    await act(async () => close?.click());
    expect(container.querySelector(".toast-card")).toBeNull();
  });

  it("surfaces the history recovery notice once as a persistent toast", async () => {
    await act(async () => root.unmount());
    invokeMock.mockImplementation((command: unknown) => {
      if (command === "get_history_notice") return Promise.resolve("Recovered 2 interrupted transfers from the last session.");
      if (command === "get_settings") return Promise.resolve(defaultSettings);
      return Promise.resolve([]);
    });
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(App));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const toast = container.querySelector(".toast-card");
    expect(toast?.textContent).toContain("Recovered 2 interrupted transfers from the last session.");
    expect(toast?.getAttribute("role")).toBeNull();
  });

  it("drops the retired card when the backend reports a retry replaced its job", async () => {
    await act(async () => eventListener?.({ payload: download({
      id: "velo-1-1", generation: 3, revision: 4, state: "interrupted", bytesWritten: 4096, totalBytes: 8192,
    }) }));
    expect(container.textContent).toContain("archive.zip");

    await act(async () => removedListener?.({ payload: { id: "velo-1-1", generation: 3 } }));
    expect(container.textContent).not.toContain("archive.zip");
  });

  it("persists a new default download folder chosen through the native dialog", async () => {
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Settings"))?.click();
    });
    expect(container.textContent).toContain("System Downloads folder");

    openDialogMock.mockResolvedValueOnce("D:\\Archive");
    invokeMock.mockResolvedValueOnce({
      defaultDownloadDir: "D:\\Archive",
      storedDirValid: true,
      osDefaultDownloadDir: "C:\\Users\\me\\Downloads",
      effectiveDownloadDir: "D:\\Archive",
    });
    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent === "Change…")?.click();
      await Promise.resolve();
    });

    expect(invokeMock).toHaveBeenCalledWith("set_default_directory", { directory: "D:\\Archive" });
    expect(container.textContent).toContain("Saved choice");
    expect(container.textContent).toContain("D:\\Archive");
  });

  it("warns and falls back when the saved default folder disappears", async () => {
    await act(async () => root.unmount());
    invokeMock.mockImplementation((command: unknown) => {
      if (command === "get_settings") {
        return Promise.resolve<SettingsDto>({
          defaultDownloadDir: "D:\\Gone",
          storedDirValid: false,
          osDefaultDownloadDir: "C:\\Users\\me\\Downloads",
          effectiveDownloadDir: "C:\\Users\\me\\Downloads",
        });
      }
      if (command === "get_history_notice") return Promise.resolve(null);
      return Promise.resolve([]);
    });
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(App));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    await act(async () => {
      [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Settings"))?.click();
    });
    const warning = container.querySelector(".settings-warning");
    expect(warning?.textContent).toContain("no longer exists");
    expect(container.textContent).toContain("C:\\Users\\me\\Downloads");
  });
});
