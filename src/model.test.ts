import { describe, expect, it } from "vitest";
import { formatBytes, isTerminal, mergeDownloads, progressPercent, selectDownloads, suggestedFileName, validateUrl, type DownloadDto } from "./model";

const item = (patch: Partial<DownloadDto> = {}): DownloadDto => ({
  id: "job-1",
  generation: 1,
  revision: 1,
  order: 1,
  fileName: "archive.zip",
  state: "downloading",
  bytesWritten: 5,
  totalBytes: 10,
  speedBytesPerSecond: 4,
  etaSeconds: 2,
  error: null,
  ...patch,
});

describe("download presentation model", () => {
  it("does not invent a percentage when total size is unknown", () => {
    expect(progressPercent(item({ totalBytes: null }))).toBeNull();
    expect(progressPercent(item({ state: "completed", bytesWritten: 10, totalBytes: 10 }))).toBe(100);
  });

  it("keeps the newest revision for a stable generation-bearing key", () => {
    expect(mergeDownloads([item()], [item({ revision: 3, bytesWritten: 8 })])[0].bytesWritten).toBe(8);
    expect(mergeDownloads([item({ revision: 3 })], [item({ revision: 2 })])[0].revision).toBe(3);
  });

  it("projects only supported queue and completed states", () => {
    const items = [item(), item({ id: "queued", state: "queued" }), item({ id: "done", state: "completed" })];
    expect(selectDownloads(items, "queue", "all").map((download) => download.id)).toEqual(["queued"]);
    expect(selectDownloads(items, "completed", "all").map((download) => download.id)).toEqual(["done"]);
    expect(selectDownloads(items, "categories", "all")).toEqual([]);
    expect(isTerminal("completed")).toBe(true);
    expect(isTerminal("downloading")).toBe(false);
  });

  it("formats sizes and validates URL schemes and credentials", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(validateUrl("https://example.test/file")).toBeNull();
    expect(validateUrl("ftp://example.test/file")).not.toBeNull();
    expect(validateUrl("https://user:pass@example.test/file")).not.toBeNull();
  });

  it("uses a safe filename suggestion", () => {
    expect(suggestedFileName("https://example.test/a%3Fb.zip?token=secret")).toBe("a_b.zip");
    expect(suggestedFileName("https://example.test/")).toBe("download");
  });
});
