import { describe, expect, it } from "vitest";
import { scanProgressParts } from "../scanProgress";
import type { ScanStatus } from "../types";

function scanStatus(overrides: Partial<ScanStatus> = {}): ScanStatus {
  const now = Date.now();
  return {
    jobId: "scan-test",
    state: "running",
    scanPhase: "scanning",
    scannedFiles: 0,
    indexedDocuments: 0,
    startedAtMs: now - 10_000,
    updatedAtMs: now,
    estimatedTotalFiles: 100_000,
    estimatedTotalBytes: 1024,
    partial: false,
    rootIds: [1],
    rootPaths: ["C:\\test"],
    currentPath: null,
    error: null,
    message: "Scanning.",
    ...overrides
  };
}

describe("scan progress copy", () => {
  it("does not round an active scan up to 100 percent", () => {
    const progress = scanProgressParts(scanStatus({
      scannedFiles: 99_950
    }));

    expect(progress.progressText).toBe("99%");
    expect(progress.percent).toBeLessThan(100);
  });

  it("shows 100 percent only after completion", () => {
    const progress = scanProgressParts(scanStatus({
      state: "completed",
      scanPhase: "completed",
      scannedFiles: 100_000
    }));

    expect(progress.progressText).toBe("100%");
    expect(progress.percent).toBe(100);
  });

  it("does not show a scan percent while finalizing local metadata", () => {
    const progress = scanProgressParts(scanStatus({
      scanPhase: "finalizing",
      scannedFiles: 100_000
    }));

    expect(progress.progressText).toBe("Finalizing");
    expect(progress.percent).toBeNull();
    expect(progress.estimateText).toContain("tree sizes");
  });

  it("stops showing 99 percent once the fresh estimate has been reached", () => {
    const progress = scanProgressParts(scanStatus({
      scannedFiles: 100_000
    }));

    expect(progress.progressText).toBe("Wrapping up");
    expect(progress.percent).toBeNull();
    expect(progress.estimateText).toContain("persisting");
  });

  it("shows an exceeded estimate instead of pretending the scan is complete", () => {
    const progress = scanProgressParts(scanStatus({
      scannedFiles: 140_000
    }));

    expect(progress.progressText).toBe("Wrapping up");
    expect(progress.percent).toBeNull();
    expect(progress.countText).toContain("exceeded 100,000 estimate");
    expect(progress.estimateText).toContain("persisting");
  });

  it("marks partial scans as incomplete instead of complete", () => {
    const progress = scanProgressParts(scanStatus({
      state: "partial",
      scanPhase: "partial",
      scannedFiles: 33_500,
      partial: true,
      message: "Scan item limit reached."
    }));

    expect(progress.progressText).toBe("Partial 34%");
    expect(progress.countText).toContain("at least 33,500");
    expect(progress.estimateText).toContain("continue scan");
    expect(progress.percent).toBeLessThan(100);
  });

  it("explains when estimation is waiting on the filesystem", () => {
    const progress = scanProgressParts(scanStatus({
      scanPhase: "estimating",
      updatedAtMs: Date.now() - 12_000
    }));

    expect(progress.rateText).toContain("waiting on filesystem");
    expect(progress.estimateText).toContain("filesystem or cloud provider");
    expect(progress.detailText).toContain("try Stop");
    expect(progress.bottleneckText).toContain("filesystem");
  });

  it("explains when scanning has not received fresh filesystem updates", () => {
    const progress = scanProgressParts(scanStatus({
      scannedFiles: 25_000,
      updatedAtMs: Date.now() - 14_000,
      lastProgressAtMs: Date.now() - 14_000
    }));

    expect(progress.rateText).toContain("waiting on filesystem");
    expect(progress.estimateText).toContain("filesystem or cloud provider");
    expect(progress.bottleneckText).toContain("filesystem");
  });

  it("calls out OneDrive-backed roots separately from generic filesystem waits", () => {
    const progress = scanProgressParts(scanStatus({
      scannedFiles: 25_000,
      currentPath: "C:\\Users\\me\\OneDrive\\AI\\project\\file.md",
      updatedAtMs: Date.now() - 14_000,
      lastProgressAtMs: Date.now() - 14_000
    }));

    expect(progress.bottleneckText).toContain("OneDrive");
  });

  it("calls out SQLite writes during persisting", () => {
    const progress = scanProgressParts(scanStatus({
      scanPhase: "persisting",
      scannedFiles: 25_000
    }));

    expect(progress.bottleneckText).toContain("SQLite");
  });
});
