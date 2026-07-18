import { describe, expect, it } from "vitest";
import type { MutationActivityLog, MutationStoredEntry } from "../types";
import {
  finalRemoveEntryActionEnabled,
  formatRecoveryTimestamp,
  orderRecoveryStoredEntries,
  recoveryEmptyState,
  recoveryHasRecords,
  recoveryOperationMeta,
  recoveryPreviewItems,
  recoveryRestorableSummaryDetail,
  recoveryStoredEntryStatusLabel,
  shouldShowFinalRemoveOptIn,
  storedEntryDisclosureLabel,
  summarizeRecovery,
  technicalActivityDisclosureLabel,
  technicalActivityPathLabel
} from "../views/RecoveryView";

const activityLog: MutationActivityLog = {
  enabled: true,
  message: "History loaded.",
  operations: [
    {
      id: 7,
      kind: "restore",
      status: "completed",
      recoveredBytes: null,
      createdAt: "2026-07-01T21:34:24.676492300+00:00"
    },
    {
      id: 6,
      kind: "restore",
      status: "failed",
      recoveredBytes: 512,
      createdAt: "2026-07-01T21:33:24+00:00"
    }
  ],
  items: [],
  backups: [
    { id: 2, level: "standard", destination: "C:\\backup", manifestPath: "C:\\backup\\manifest.json", verified: true, createdAt: "2026-07-01T21:30:00+00:00" },
    { id: 1, level: "standard", destination: "D:\\backup", manifestPath: "D:\\backup\\manifest.json", verified: false, createdAt: "2026-07-01T21:20:00+00:00" }
  ],
  storedEntries: [
    { id: 2, originalPath: "C:\\project\\restored.txt", storedPath: "C:\\held\\restored.txt", spaceRecovered: 0, status: "restored" },
    { id: 1, originalPath: "C:\\project\\held.txt", storedPath: "C:\\held\\held.txt", spaceRecovered: 128, status: "quarantined" }
  ]
};

describe("recovery empty state", () => {
  it("explains the read-only build without implying a missing recovery record", () => {
    expect(recoveryEmptyState(false)).toMatchObject({
      title: "No recovery history in this build"
    });
    expect(recoveryEmptyState(false).detail).toContain("never creates held files");
  });

  it("points mutation builds back to mapping until recovery history exists", () => {
    expect(recoveryEmptyState(true)).toMatchObject({
      title: "Nothing to recover"
    });
    expect(recoveryEmptyState(true).detail).toContain("after a confirmed safe action creates recovery history");
  });
});

describe("recovery timestamps", () => {
  it("removes raw ISO nanoseconds from valid journal dates", () => {
    const formatted = formatRecoveryTimestamp("2026-07-01T21:34:24.676492300+00:00");
    expect(formatted).not.toContain("T21:34:24.676492300");
    expect(formatted).toContain("2026");
  });

  it("keeps an unknown backend timestamp visible instead of hiding it", () => {
    expect(formatRecoveryTimestamp("pending clock sync")).toBe("pending clock sync");
    expect(formatRecoveryTimestamp(null)).toBe("Earlier");
  });
});

describe("final removal visibility gate", () => {
  it("stays unavailable until the explicit opt-in is enabled", () => {
    expect(finalRemoveEntryActionEnabled(false, true, false, "quarantined")).toBe(false);
    expect(finalRemoveEntryActionEnabled(true, true, false, "quarantined")).toBe(true);
  });

  it("also refuses busy, read-only and non-held entries", () => {
    expect(finalRemoveEntryActionEnabled(true, false, false, "quarantined")).toBe(false);
    expect(finalRemoveEntryActionEnabled(true, true, true, "quarantined")).toBe(false);
    expect(finalRemoveEntryActionEnabled(true, true, false, "restored")).toBe(false);
  });

  it("hides the opt-in when no held file can use it, unless it is already on", () => {
    expect(shouldShowFinalRemoveOptIn(false, true, 0)).toBe(false);
    expect(shouldShowFinalRemoveOptIn(false, true, 1)).toBe(true);
    expect(shouldShowFinalRemoveOptIn(true, true, 0)).toBe(true);
    expect(shouldShowFinalRemoveOptIn(true, false, 1)).toBe(false);
  });
});

describe("recovery overview and progressive disclosure", () => {
  it("labels a content-mismatched restore as manual-review history, not held data", () => {
    expect(recoveryStoredEntryStatusLabel("restore_content_mismatch")).toBe(
      "Restore destination has different content"
    );
    expect(finalRemoveEntryActionEnabled(true, true, false, "restore_content_mismatch")).toBe(false);
  });

  it("summarizes what is actionable separately from local history", () => {
    const summary = summarizeRecovery(activityLog, 2);
    expect(summary).toEqual({
      heldFiles: 1,
      appListings: 2,
      restorableNow: 3,
      storedRecords: 2,
      resolvedStoredRecords: 1,
      verifiedBackups: 1,
      totalBackups: 2,
      diskActions: 2,
      failedActions: 1
    });
    expect(recoveryRestorableSummaryDetail(summary)).toBe("1 held file + 2 app listings");
    expect(recoveryHasRecords(activityLog, 0)).toBe(true);
  });

  it("keeps long collections collapsed until requested", () => {
    expect(recoveryPreviewItems([1, 2, 3, 4, 5], false)).toEqual([1, 2, 3]);
    expect(recoveryPreviewItems([1, 2, 3, 4, 5], true)).toEqual([1, 2, 3, 4, 5]);
    expect(storedEntryDisclosureLabel(6, 0, false)).toBe("Show 6 completed records");
    expect(storedEntryDisclosureLabel(6, 2, false)).toBe("Review 2 held files");
    expect(technicalActivityDisclosureLabel(42)).toBe("Show technical record (30 of 42)");
  });

  it("orders held files before completed history", () => {
    const entries: MutationStoredEntry[] = [
      { id: 9, originalPath: "restored", storedPath: "stored", spaceRecovered: 0, status: "restored" },
      { id: 3, originalPath: "held", storedPath: "stored", spaceRecovered: 20, status: "quarantined" }
    ];
    expect(orderRecoveryStoredEntries(entries).map((entry) => entry.id)).toEqual([3, 9]);
  });

  it("omits empty byte placeholders and cleans technical path prefixes", () => {
    const meta = recoveryOperationMeta(activityLog.operations[0]);
    expect(meta).toContain("2026");
    expect(meta).not.toContain("—");
    expect(technicalActivityPathLabel({
      id: 1,
      operationId: 7,
      action: "move",
      status: "done",
      fromPath: "\\\\?\\C:\\held\\file.txt",
      toPath: "\\\\?\\C:\\project\\file.txt"
    })).toBe("C:\\held\\file.txt -> C:\\project\\file.txt");
  });
});
