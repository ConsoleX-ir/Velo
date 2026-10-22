import { describe, expect, it } from "vitest";
import { emptySpeedHistory, formatBytes, formatRate, isTerminal, mergeDownloads, progressPercent, recordSpeedSample, selectDownloads, speedHistoryPoints, suggestedFileName, validateUrl, type DownloadDto } from "./model";

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
    const items = [item(), item({ id: "cancelling", state: "cancelling" }), item({ id: "queued", state: "queued" }), item({ id: "done", state: "completed" })];
    expect(selectDownloads(items, "downloads", "active").map((download) => download.id)).toEqual(["job-1", "cancelling"]);
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
    expect(formatRate(0)).toBe("0 B/s");
  });

  it("uses a safe filename suggestion", () => {
    expect(suggestedFileName("https://example.test/a%3Fb.zip?token=secret")).toBe("a_b.zip");
    expect(suggestedFileName("https://example.test/")).toBe("download");
  });

  it("samples bounded real speed history, including zero, at a stable interval", () => {
    const first = recordSpeedSample(emptySpeedHistory(), "downloading", 512, 1_000);
    expect(first.samples).toEqual([512]);
    expect(recordSpeedSample(first, "downloading", 0, 1_500)).toBe(first);
    const stopped = recordSpeedSample(first, "downloading", 0, 2_000);
    expect(stopped.samples).toEqual([512, 0]);
    let history = stopped;
    for (let index = 0; index < 40; index += 1) {
      history = recordSpeedSample(history, "downloading", index + 1, 3_000 + index * 1_000);
    }
    expect(history.samples).toHaveLength(36);
    expect(history.samples.at(-1)).toBe(40);
    expect(recordSpeedSample(history, "completed", 800, 50_000).samples).toEqual([]);
    expect(recordSpeedSample(history, "cancelled", 800, 50_000).samples).toEqual([]);
    expect(recordSpeedSample(history, "downloading", null, 50_000)).toBe(history);
  });

  it("maps speed history into a bounded sparkline and handles zero or missing values", () => {
    expect(speedHistoryPoints([])).toBe("");
    expect(speedHistoryPoints([0])).toBe("66.0,39.0");
    expect(speedHistoryPoints([0, 10, 5])).toBe("0.0,39.0 66.0,3.0 132.0,21.0");
  });
});
