import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

let eventListener: ((event: { payload: unknown }) => void) | undefined;
let removedListener: ((event: { payload: { id: string; generation: number } }) => void) | undefined;

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

import App from "./App";

describe("application preferences", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("data-motion");
    invokeMock.mockResolvedValue([]);
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

  it("returns keyboard focus to the opener when the add dialog closes", async () => {
    const add = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("Add download"));
    expect(add).toBeDefined();
    await act(async () => { add?.focus(); add?.click(); });
    const cancel = [...container.querySelectorAll("button")].find((button) => button.textContent === "Cancel");
    expect(cancel).toBeDefined();
    await act(async () => cancel?.click());
    expect(document.activeElement).toBe(add);
  });

  it("keeps a newer event when the initial snapshot arrives afterward", async () => {
    await act(async () => root.unmount());
    let resolveSnapshot!: (items: import("./model").DownloadDto[]) => void;
    invokeMock.mockImplementation(() => new Promise((resolve) => { resolveSnapshot = resolve; }));
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(App));
      await Promise.resolve();
    });

    const download = (revision: number, bytesWritten: number): import("./model").DownloadDto => ({
      id: "job-1", generation: 1, revision, order: 1, fileName: "archive.zip", state: "downloading",
      bytesWritten, totalBytes: 10, speedBytesPerSecond: null, etaSeconds: null, error: null,
    });
    await act(async () => eventListener?.({ payload: download(2, 8) }));
    await act(async () => resolveSnapshot([download(1, 2)]));

    expect(container.textContent).toContain("8 B of 10 B");
    expect(container.textContent).not.toContain("2 B of 10 B");
  });

  it("never presents terminal speed as a live measurement", async () => {
    const completed = {
      id: "done", generation: 1, revision: 2, order: 2, fileName: "finished.zip", state: "completed" as const,
      bytesWritten: 10, totalBytes: 10, speedBytesPerSecond: 512, etaSeconds: null, error: null,
    };
    await act(async () => eventListener?.({ payload: completed }));
    expect(container.querySelector(".speed-chart")).toBeNull();
    expect(container.textContent).not.toContain("512 B/s");
  });

  it("renders a speed chart from a real-shaped active download event", async () => {
    const active = {
      id: "active", generation: 1, revision: 2, order: 3, fileName: "transfer.bin", state: "downloading" as const,
      bytesWritten: 512, totalBytes: null, speedBytesPerSecond: 512, etaSeconds: null, error: null,
    };
    await act(async () => eventListener?.({ payload: active }));
    expect(container.querySelector(".speed-chart-plot")?.getAttribute("role")).toBe("img");
    expect(container.querySelector(".speed-chart-heading strong")?.textContent).toBe("512 B/s");
    expect(container.textContent).toContain("SIZE UNKNOWN");
  });

  it("sends pause and resume commands for the current download generation", async () => {
    const active = {
      id: "active", generation: 4, revision: 1, order: 1, fileName: "transfer.bin", state: "downloading" as const,
      bytesWritten: 512, totalBytes: 1024, speedBytesPerSecond: 10, etaSeconds: 50, error: null,
    };
    await act(async () => eventListener?.({ payload: active }));
    const pause = container.querySelector<HTMLButtonElement>('[aria-label="Pause transfer.bin"]');
    await act(async () => pause?.click());
    expect(invokeMock).toHaveBeenCalledWith("pause_download", { id: "active", generation: 4 });

    await act(async () => eventListener?.({ payload: { ...active, state: "paused", revision: 2 } }));
    const resume = container.querySelector<HTMLButtonElement>('[aria-label="Resume transfer.bin"]');
    await act(async () => resume?.click());
    expect(invokeMock).toHaveBeenCalledWith("resume_download", { id: "active", generation: 4 });
  });

  it("renders a retrying download with only a cancel affordance wired to cancel_download", async () => {
    await act(async () => eventListener?.({ payload: {
      id: "job-9", generation: 3, revision: 5, order: 1, fileName: "resume.bin", state: "retrying" as const,
      bytesWritten: 100, totalBytes: 400, speedBytesPerSecond: null, etaSeconds: null, error: "connection reset",
    } }));

    expect(container.textContent).toContain("Retrying");
    expect(container.textContent).toContain("connection reset");
    expect(container.textContent).toContain("RETRY SCHEDULED");

    const cancel = container.querySelector<HTMLButtonElement>('[aria-label="Cancel resume.bin"]');
    expect(cancel).toBeDefined();
    expect(container.querySelector('[aria-label="Pause resume.bin"]')).toBeNull();
    expect(container.querySelector('[aria-label="Resume resume.bin"]')).toBeNull();
    expect([...container.querySelectorAll("button")].some((button) => button.textContent === "Retry")).toBe(false);
    expect([...container.querySelectorAll("button")].some((button) => button.textContent === "Dismiss")).toBe(false);

    await act(async () => cancel?.click());
    expect(invokeMock).toHaveBeenCalledWith("cancel_download", { id: "job-9", generation: 3 });
  });

  it("moves the downloads filter when ArrowRight is pressed on the selected radio", async () => {
    await act(async () => eventListener?.({ payload: {
      id: "done", generation: 1, revision: 1, order: 1, fileName: "wrapped.zip", state: "completed" as const,
      bytesWritten: 20, totalBytes: 20, speedBytesPerSecond: null, etaSeconds: null, error: null,
    } }));
    expect(container.textContent).toContain("wrapped.zip");

    const selected = () => container.querySelector<HTMLButtonElement>('[role="radiogroup"][aria-label="Filter downloads"] [role="radio"][aria-checked="true"]');
    expect(selected()?.textContent).toBe("All");

    await act(async () => {
      selected()?.focus();
      selected()?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
      await Promise.resolve();
    });

    expect(selected()?.textContent).toBe("Active");
    expect(container.textContent).not.toContain("wrapped.zip");
  });

  it("links a tooltip to the pause control after keyboard focus settles", async () => {
    await act(async () => eventListener?.({ payload: {
      id: "active", generation: 4, revision: 1, order: 1, fileName: "transfer.bin", state: "downloading" as const,
      bytesWritten: 512, totalBytes: 1024, speedBytesPerSecond: 10, etaSeconds: 50, error: null,
    } }));
    const pause = container.querySelector<HTMLButtonElement>('[aria-label="Pause transfer.bin"]');
    expect(pause).toBeDefined();

    await act(async () => {
      pause?.focus();
      await new Promise((resolve) => setTimeout(resolve, 350));
    });

    const tooltip = container.querySelector('[role="tooltip"]');
    expect(tooltip).toBeDefined();
    expect(tooltip?.textContent).toBe("Pause download");
    expect(pause?.getAttribute("aria-describedby")).toBe(tooltip?.id);
  });

  it("reports command failures as dismissable error toasts instead of banner entries", async () => {
    await act(async () => eventListener?.({ payload: {
      id: "active", generation: 4, revision: 1, order: 1, fileName: "transfer.bin", state: "downloading" as const,
      bytesWritten: 512, totalBytes: 1024, speedBytesPerSecond: 10, etaSeconds: 50, error: null,
    } }));
    invokeMock.mockRejectedValueOnce("engine refused to pause");

    await act(async () => {
      container.querySelector<HTMLButtonElement>('[aria-label="Pause transfer.bin"]')?.click();
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
    const stale = {
      id: "velo-1-1", generation: 3, revision: 4, order: 1, fileName: "retry-me.bin", state: "interrupted" as const,
      bytesWritten: 4096, totalBytes: 8192, speedBytesPerSecond: null, etaSeconds: null, error: null,
    };
    await act(async () => eventListener?.({ payload: stale }));
    expect(container.textContent).toContain("retry-me.bin");

    await act(async () =>
      removedListener?.({ payload: { id: "velo-1-1", generation: 3 } }),
    );
    expect(container.textContent).not.toContain("retry-me.bin");
  });
});
