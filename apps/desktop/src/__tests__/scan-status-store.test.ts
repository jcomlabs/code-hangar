// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  MAX_RETAINED_TERMINAL_SCAN_STATUSES,
  mergeScanStatusSnapshot,
  scanStatusAnnouncementKind
} from "../scanStatusStore";
import type { ScanStatus } from "../types";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

function status(jobId: string, overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    jobId,
    state: "running",
    scanPhase: "scanning",
    scannedFiles: 10,
    indexedDocuments: 2,
    startedAtMs: 1,
    updatedAtMs: 10,
    partial: false,
    rootIds: [1],
    rootPaths: ["C:\\projects\\one"],
    currentPath: "C:\\projects\\one\\README.md",
    error: null,
    message: "Scanning local metadata.",
    ...overrides
  };
}

describe("bounded scan status snapshots", () => {
  it("returns the same store reference for a structurally identical snapshot", () => {
    const original = status("scan-1");
    const current = { [original.jobId]: original };
    const duplicate = {
      ...original,
      rootIds: [...original.rootIds],
      rootPaths: [...original.rootPaths]
    };

    expect(mergeScanStatusSnapshot(current, duplicate)).toBe(current);
  });

  it("publishes a new store when visual progress changes", () => {
    const original = status("scan-1");
    const current = { [original.jobId]: original };
    const progressed = status("scan-1", {
      scannedFiles: original.scannedFiles + 1,
      currentPath: "C:\\projects\\one\\docs\\next.md",
      updatedAtMs: original.updatedAtMs + 1
    });

    const next = mergeScanStatusSnapshot(current, progressed);
    expect(next).not.toBe(current);
    expect(next["scan-1"]).toBe(progressed);
  });

  it("retains only the 40 most recently updated terminal jobs", () => {
    let current: Record<string, ScanStatus> = {};
    for (let index = 0; index < 47; index += 1) {
      current = mergeScanStatusSnapshot(current, status(`terminal-${index}`, {
        state: "completed",
        scanPhase: "completed",
        updatedAtMs: index
      }));
    }

    const ids = Object.keys(current);
    expect(ids).toHaveLength(MAX_RETAINED_TERMINAL_SCAN_STATUSES);
    expect(ids).not.toContain("terminal-6");
    expect(ids).toContain("terminal-7");
    expect(ids).toContain("terminal-46");
  });

  it("preserves every running, queued and cancelling job while capping terminals", () => {
    const active = [
      ...Array.from({ length: 23 }, (_, index) => status(`running-${index}`)),
      ...Array.from({ length: 17 }, (_, index) => status(`cancelling-${index}`, {
        state: "cancelling",
        scanPhase: "cancelling"
      })),
      status("queued-1", { state: "queued", scanPhase: "queued" })
    ];
    const terminal = Array.from({ length: 55 }, (_, index) => status(`done-${index}`, {
      state: "completed",
      scanPhase: "completed",
      updatedAtMs: index
    }));
    const unbounded = Object.fromEntries([...active, ...terminal].map((item) => [item.jobId, item]));
    const next = mergeScanStatusSnapshot(unbounded, status("done-new", {
      state: "completed",
      scanPhase: "completed",
      updatedAtMs: 100
    }));

    expect(active.every((item) => next[item.jobId] === item)).toBe(true);
    expect(Object.values(next).filter((item) => !["queued", "running", "cancelling"].includes(item.state)))
      .toHaveLength(MAX_RETAINED_TERMINAL_SCAN_STATUSES);
  });

  it("keeps the same reference when an older incoming terminal is immediately pruned", () => {
    const retained = Object.fromEntries(
      Array.from({ length: MAX_RETAINED_TERMINAL_SCAN_STATUSES }, (_, index) => {
        const item = status(`retained-${index}`, {
          state: "completed",
          scanPhase: "completed",
          updatedAtMs: index + 100
        });
        return [item.jobId, item];
      })
    );

    const next = mergeScanStatusSnapshot(retained, status("too-old", {
      state: "completed",
      scanPhase: "completed",
      updatedAtMs: 1
    }));
    expect(next).toBe(retained);
  });
});

describe("semantic scan announcements", () => {
  it("does not announce volatile counters, paths or scan/persist batch transitions", () => {
    const previous = status("scan-1");
    const progressed = status("scan-1", {
      scannedFiles: 200,
      indexedDocuments: 80,
      updatedAtMs: 20,
      currentPath: "C:\\projects\\one\\src\\busy.ts",
      message: "Scanning another local metadata path."
    });
    const persisting = status("scan-1", {
      scanPhase: "persisting",
      updatedAtMs: 21,
      currentPath: null,
      message: "Persisting 500 metadata items to the local database."
    });

    expect(scanStatusAnnouncementKind(previous, progressed)).toBeNull();
    expect(scanStatusAnnouncementKind(progressed, persisting)).toBeNull();
  });

  it("announces semantic phase changes and low-frequency finalization milestones", () => {
    const scanning = status("scan-1");
    const finalizing = status("scan-1", {
      scanPhase: "finalizing",
      message: "Finalizing: rebuilding local Markdown links."
    });
    const nextMilestone = status("scan-1", {
      scanPhase: "finalizing",
      updatedAtMs: 20,
      message: "Finalizing: resolving local workflow references."
    });

    expect(scanStatusAnnouncementKind(scanning, finalizing)).toBe("phase");
    expect(scanStatusAnnouncementKind(finalizing, nextMilestone)).toBe("message");
  });

  it("announces errors and a terminal transition once", () => {
    const scanning = status("scan-1");
    const failed = status("scan-1", {
      state: "failed",
      scanPhase: "failed",
      error: "Disk read failed.",
      message: "Scan failed."
    });

    expect(scanStatusAnnouncementKind(scanning, failed)).toBe("error");
    expect(scanStatusAnnouncementKind(failed, { ...failed })).toBeNull();
    expect(scanStatusAnnouncementKind(scanning, status("scan-1", {
      state: "completed",
      scanPhase: "completed",
      message: "Inventory scan complete."
    }))).toBe("terminal");
  });
});

describe("scan polling and accessibility contract", () => {
  it("polls a stable job-id snapshot instead of restarting for every progress render", () => {
    const start = appSource.indexOf("useEffect(() => {\n    if (!runningJobKey) return;");
    const end = appSource.indexOf("useEffect(() => {\n    if (!scanCelebration) return;", start);
    const polling = appSource.slice(start, end);

    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    expect(polling).toContain('const runningJobIds = runningJobKey.split("|");');
    expect(polling).toContain("scanStatusAnnouncementKind(previousAnnouncement, status)");
    expect(polling).toContain("schedule(document.hidden ? 2_000 : 500)");
    expect(polling).not.toContain("status.currentPath");
    expect(polling).toContain("runningJobKey, setScanStatus]);");
    expect(polling).not.toContain("runningJobKey, runningScanStatuses");
  });

  it("keeps rapid visual progress outside the polite status live region", () => {
    const start = appSource.indexOf('<footer\n        className={[');
    const end = appSource.indexOf('<span className="statusbar-action-slot">', start);
    const footerStatus = appSource.slice(start, end);

    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    expect(footerStatus).toContain('<span className="statusbar-message" role="status" aria-live="polite" aria-atomic="true">{statusText}</span>');
    expect(footerStatus).toContain('<span className="statusbar-scan" aria-live="off"');
    expect(footerStatus).not.toContain("currentPath");
  });
});
