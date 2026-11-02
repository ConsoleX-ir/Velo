// React 18 `act` only flushes async state updates when this flag is set.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, readTextMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  readTextMock: vi.fn(),
}));

let container: HTMLDivElement;
let root: Root;

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: readTextMock }));

import { AddDownloadDialog, type AddDownloadDialogProps } from "./AddDownloadDialog";
import type { SettingsDto } from "../model";

const defaultSettings: SettingsDto = {
  defaultDownloadDir: null,
  storedDirValid: false,
  osDefaultDownloadDir: "C:\\Users\\me\\Downloads",
  effectiveDownloadDir: "C:\\Users\\me\\Downloads",
  closeBehavior: "minimize_to_tray",
  launchAtStartup: false,
  downloadNotifications: true,
};

let clipboardResolve: ((value: string | null) => void) | null = null;

function renderDialog(props: Partial<AddDownloadDialogProps> = {}) {
  const settings = props.settings ?? defaultSettings;
  const onClose = props.onClose ?? vi.fn();
  const onStarted = props.onStarted ?? vi.fn();
  const onCommandError = props.onCommandError ?? vi.fn();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => {
    root.render(createElement(AddDownloadDialog, { settings, onClose, onStarted, onCommandError }));
  });
  return { onClose, onStarted, onCommandError };
}

function urlInput() {
  return container.querySelector<HTMLInputElement>("#download-url");
}

/** React-controlled inputs ignore plain value assignment; use the native setter. */
function typeInput(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Click "Inspect link" and flush the async inspection chain so the result renders. */
async function inspectLink() {
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Inspect link metadata"]')?.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

const inspectMetadata = {
  effectiveUrl: "https://cdn.example.test/file.zip",
  status: 200,
  suggestedFilename: "file.zip",
  contentLength: 1234,
  contentType: "application/zip",
  acceptsRanges: true,
  server: "example/1.0",
};

beforeEach(async () => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  clipboardResolve = null;
  readTextMock.mockImplementation(() => new Promise((resolve) => { clipboardResolve = resolve; }));
  invokeMock.mockImplementation((command: unknown) => {
    if (command === "inspect_url") return Promise.resolve(inspectMetadata);
    return Promise.resolve(null);
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  if (root) {
    await act(async () => root.unmount());
  }
  container.remove();
  vi.clearAllMocks();
});

describe("Add Download clipboard seeding", () => {
  it("populates the URL field from a valid HTTP clipboard URL on open", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("http://example.test/file.zip");
    await flush();
    expect(urlInput()?.value).toBe("http://example.test/file.zip");
  });

  it("populates the URL field from a valid HTTPS clipboard URL on open", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/file.zip");
    await flush();
    expect(urlInput()?.value).toBe("https://example.test/file.zip");
  });

  it("does not populate invalid clipboard contents", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("not a url");
    await flush();
    expect(urlInput()?.value).toBe("");
  });

  it("does not populate unsupported protocols", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("ftp://example.test/file.zip");
    await flush();
    expect(urlInput()?.value).toBe("");
  });

  it("leaves an empty clipboard alone", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("");
    await flush();
    expect(urlInput()?.value).toBe("");
  });

  it("never overwrites text the user has already entered", async () => {
    renderDialog({ settings: defaultSettings });
    const input = urlInput()!;
    await act(async () => {
      typeInput(input, "https://user-entered.test/file.zip");
      await Promise.resolve();
    });
    clipboardResolve?.("http://clipboard.test/file.zip");
    await flush();
    expect(urlInput()?.value).toBe("https://user-entered.test/file.zip");
  });

  it("fails silently when clipboard access is unavailable", async () => {
    clipboardResolve = null;
    readTextMock.mockRejectedValue(new Error("clipboard denied"));
    renderDialog({ settings: defaultSettings });
    await flush();
    expect(urlInput()?.value).toBe("");
  });
});

describe("Add Download URL inspection", () => {
  it("shows a loading state while inspecting", async () => {
    let resolveInspect!: (value: unknown) => void;
    invokeMock.mockImplementation((command: unknown) => {
      if (command === "inspect_url") return new Promise((resolve) => { resolveInspect = resolve; });
      return Promise.resolve(null);
    });
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/file.zip");
    await flush();
    const inspect = container.querySelector<HTMLButtonElement>('button[aria-label="Inspect link metadata"]');
    expect(inspect).toBeDefined();
    await act(async () => {
      inspect?.click();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("Inspecting…");
    await act(async () => {
      resolveInspect(inspectMetadata);
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("https://cdn.example.test/file.zip");
  });

  it("displays inspected metadata and marks unknown fields honestly", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/file.zip");
    await flush();
    await inspectLink();
    expect(container.textContent).toContain("https://cdn.example.test/file.zip");
    expect(container.textContent).toContain("1.2 KB");
    expect(container.textContent).toContain("application/zip");
    expect(container.textContent).toContain("Supported");
    expect(container.textContent).toContain("example/1.0");
    expect(container.textContent).toContain("200");
  });

  it("represents unavailable metadata as unknown instead of fabricating it", async () => {
    invokeMock.mockResolvedValueOnce({
      effectiveUrl: "https://example.test/file.zip",
      status: 200,
      suggestedFilename: null,
      contentLength: null,
      contentType: null,
      acceptsRanges: null,
      server: null,
    });
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/file.zip");
    await flush();
    await inspectLink();
    expect(container.textContent).toContain("—");
    expect(container.textContent).toContain("unknown");
    expect(container.textContent).not.toContain("1.2 KB");
  });

  it("reports an inspection failure without starting a download", async () => {
    invokeMock.mockRejectedValueOnce(new Error("The server responded with HTTP 404."));
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/missing.zip");
    await flush();
    await inspectLink();
    expect(container.textContent).toContain("The server responded with HTTP 404.");
    expect(invokeMock).not.toHaveBeenCalledWith(
      "start_download",
      expect.objectContaining({ url: "https://example.test/missing.zip" }),
    );
  });

  it("clears stale inspection when the URL changes", async () => {
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/first.zip");
    await flush();
    await inspectLink();
    expect(container.textContent).toContain("https://cdn.example.test/file.zip");

    const input = urlInput()!;
    await act(async () => {
      typeInput(input, "https://example.test/second.zip");
      await Promise.resolve();
    });
    expect(container.textContent).not.toContain("https://cdn.example.test/file.zip");
    expect(container.querySelector(".inspect-grid")).toBeNull();
  });

  it("starts the download explicitly through start_download, not inspection", async () => {
    invokeMock.mockResolvedValueOnce({
      id: "job-1", generation: 1, revision: 1, order: 1, fileName: "file.zip",
      destination: "C:\\Users\\me\\Downloads\\file.zip", state: "preparing", bytesWritten: 0,
      totalBytes: null, speedBytesPerSecond: null, etaSeconds: null, error: null,
    });
    renderDialog({ settings: defaultSettings });
    clipboardResolve?.("https://example.test/file.zip");
    await flush();
    await act(async () => { typeInput(urlInput()!, "https://example.test/file.zip"); });
    await act(async () => {
      typeInput(container.querySelector<HTMLInputElement>("#download-name")!, "file.zip");
    });
    const form = container.querySelector("form");
    await act(async () => {
      form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("start_download", {
      url: "https://example.test/file.zip",
      directory: "C:\\Users\\me\\Downloads",
      fileName: "file.zip",
      speedLimitBytesPerSecond: null,
    });
    expect(invokeMock).not.toHaveBeenCalledWith("inspect_url", expect.anything());
  });
});
