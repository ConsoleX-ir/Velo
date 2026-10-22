import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

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
    listenMock.mockResolvedValue(() => undefined);
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
});
