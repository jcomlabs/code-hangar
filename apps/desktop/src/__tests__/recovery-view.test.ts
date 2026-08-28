import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { FinalRemoveBatchPreview, MutationActivityLog, MutationStoredEntry } from "../types";
import {
  finalRemoveProjectActionEnabled,
  finalRemoveResultMessage,
  finalRemoveVolumeQualityLabel,
  formatRecoveryTimestamp,
  orderRecoveryStoredEntries,
  RECOVERY_REFRESH_CONFIRM_MS,
  RecoveryView,
  recoveryEmptyState,
  recoveryHasRecords,
  recoveryOperationMeta,
  recoveryPreviewItems,
  recoveryRestorableSummaryDetail,
  recoveryStoredEntryStatusLabel,
  shouldShowFinalCleanup,
  storedEntryDisclosureLabel,
  summarizeFinalRemovePreview,
  summarizeRecovery,
  technicalActivityDisclosureLabel,
  technicalActivityPathLabel
} from "../views/RecoveryView";

const finalRemovePreview: FinalRemoveBatchPreview = {
  previewId: "preview-1",
  previewDigest: `v2:${"a".repeat(64)}`,
  expiresAt: "2099-08-23T12:00:00Z",
  projects: [{
    groupId: "project-1", projectName: "Dead project", originalRoot: "C:\\dead-project",
    totalObjects: 3, ready: 1, needsArchiveV2: 1, blocked: 1,
    blockedSubtrees: [{ root: "linked-cache", count: 1, reasonCodes: ["unsupportedReparse"] }]
  }],
  objects: [
    {
      entryId: 1, groupId: "project-1", topologyGroupId: "topology-1", relativePath: "ready.txt",
      kind: "file", lifecycle: "held", eligibility: "ready", reasonCode: "archiveVerified", reason: "Verified",
      objectArchiveState: "objectCompleteVerified", heldVolumeId: "volume-c", heldVolumeLabel: "Windows (C:)",
      logicalBytes: 100, allocatedBytes: 4096, measurement: "exactStreams"
    },
    {
      entryId: 2, groupId: "project-1", topologyGroupId: "topology-2", relativePath: "legacy.txt",
      kind: "file", lifecycle: "held", eligibility: "needsArchiveV2", reasonCode: "legacyContentOnly", reason: "Legacy content only",
      objectArchiveState: "contentOnlyLegacy", heldVolumeId: "volume-c", heldVolumeLabel: "Windows (C:)",
      logicalBytes: 100, allocatedBytes: 4096, measurement: "exactStreams"
    },
    {
      entryId: 3, groupId: "project-1", topologyGroupId: "topology-3", relativePath: "linked-cache",
      kind: "directory", lifecycle: "held", eligibility: "blocked", reasonCode: "unsupportedReparse", reason: "Blocked",
      objectArchiveState: "none", heldVolumeId: "volume-c", heldVolumeLabel: "Windows (C:)",
      logicalBytes: 0, allocatedBytes: null, measurement: "unknown"
    }
  ],
  volumes: [{
    volumeId: "volume-c", label: "Windows (C:)", alreadyFreedFromSourceBytes: 4096,
    heldAllocatedBytes: 8192, projectedReleaseBytes: 8192, archiveRetainedAllocatedBytes: 2048,
    quality: "exactObjectAllocation"
  }],
  eligibleTopologyGroupIds: ["topology-1", "topology-2"],
  requiresElevation: true,
  maxDeleteObjects: 2,
  blockedObjects: 1,
  archivesRetained: true
};

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
  it("keeps the successful refresh acknowledgement transient", () => {
    expect(RECOVERY_REFRESH_CONFIRM_MS).toBe(2_000);
  });

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

describe("project and batch final cleanup", () => {
  it("uses per-project eligibility instead of a persistent global arm", () => {
    const project = finalRemovePreview.projects[0];
    expect(finalRemoveProjectActionEnabled(project, true, false)).toBe(true);
    expect(finalRemoveProjectActionEnabled({ ...project, ready: 0, needsArchiveV2: 0, blocked: 3 }, true, false)).toBe(true);
    expect(finalRemoveProjectActionEnabled({ ...project, ready: 0, needsArchiveV2: 0, blocked: 0, totalObjects: 0 }, true, false)).toBe(false);
  });

  it("refuses project cleanup while busy, expired or in a read-only edition", () => {
    expect(finalRemoveProjectActionEnabled(finalRemovePreview.projects[0], false, false)).toBe(false);
    expect(finalRemoveProjectActionEnabled(finalRemovePreview.projects[0], true, true)).toBe(false);
    expect(finalRemoveProjectActionEnabled(finalRemovePreview.projects[0], true, false, true)).toBe(false);
  });

  it("keeps cleanup central for held records and for a server preview", () => {
    expect(shouldShowFinalCleanup(1, null, false, "backend unavailable")).toBe(true);
    expect(shouldShowFinalCleanup(0, finalRemovePreview, false, null)).toBe(true);
    expect(shouldShowFinalCleanup(0, null, true, null)).toBe(true);
    expect(shouldShowFinalCleanup(0, null, false, "read-only")).toBe(false);
    expect(shouldShowFinalCleanup(0, null, false, "dashboard command missing", false, true)).toBe(true);
  });

  it("starts with the irreversible capability visibly off and no cleanup action", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true, mutationMessage: null, mutationActivity: activityLog, mutationBusy: false, finalRemoveExecutionUnknown: false,
      finalRemoveProgress: null, finalRemoveJobId: null, finalRemoveBatchId: null,
      finalRemovePreview: null, finalRemovePreviewLoading: false,
      finalRemoveUnavailableReason: "Permanent removal is off.", finalRemoveResult: null,
      finalRemoveEnabled: false, finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined,
      onSetFinalRemoveEnabled: async () => true, onStopFinalRemoveBatch: () => undefined,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("PERMANENT REMOVAL");
    expect(html).toContain("Off by default");
    expect(html).toContain("Enable permanent removal…");
    expect(html).not.toContain("Finish removing held projects");
    expect(html).not.toContain("Review batch cleanup…");
  });

  it("separates ready, archive-finalization and blocked object counts", () => {
    expect(summarizeFinalRemovePreview(finalRemovePreview)).toEqual({ ready: 1, needsArchiveV2: 1, blocked: 1, blockedSubtrees: 1 });
    expect(finalRemoveVolumeQualityLabel("exactObjectAllocation")).toContain("Exact object allocation");
    expect(finalRemoveVolumeQualityLabel("estimated")).toContain("not an exact");
  });

  it("reports a partial batch without hiding retained archives", () => {
    expect(finalRemoveResultMessage({
      batchId: "batch-1", status: "partial", requestedObjects: 3, deletedObjects: 1,
      keptObjects: 1, failedObjects: 1, projects: [], volumes: [], items: [], archiveRetained: true
    })).toBe("1 held object was deleted; 2 remain held or need review. All recovery archives were kept.");
  });

  it("renders final cleanup before recovery history after explicit capability activation", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true, mutationMessage: null, mutationActivity: activityLog, mutationBusy: false, finalRemoveExecutionUnknown: false,
      finalRemoveProgress: null, finalRemoveJobId: null, finalRemoveBatchId: null,
      finalRemovePreview, finalRemovePreviewLoading: false, finalRemoveUnavailableReason: null, finalRemoveResult: null,
      finalRemoveEnabled: true, finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Finish removing held projects");
    expect(html).toContain("Review batch cleanup…");
    expect(html).toContain("Exact object allocation from the preview");
    expect(html).not.toContain("Final removal (irreversible)");
    expect(html).toContain("Enabled for explicit final-cleanup reviews");
    expect(html.indexOf("Finish removing held projects")).toBeLessThan(html.indexOf("RECOVERY BACKUPS"));
  });

  it("surfaces a backend-provided capacity block while keeping the review available", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true, mutationMessage: null, mutationActivity: activityLog, mutationBusy: false, finalRemoveExecutionUnknown: false,
      finalRemoveProgress: null, finalRemoveJobId: null, finalRemoveBatchId: null,
      finalRemovePreview: { ...finalRemovePreview, maxDeleteObjects: 1 }, finalRemovePreviewLoading: false,
      finalRemoveUnavailableReason: null, finalRemoveResult: null,
      finalRemoveEnabled: true, finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("The full selection exceeds this preview&#x27;s verified batch capacity");
    expect(html).toContain("At most 1 can be confirmed in one batch");
    expect(html).toContain("Review batch cleanup…");
  });

  it("keeps the review entry point visible when every held object is capacity-blocked", () => {
    const blockedPreview: FinalRemoveBatchPreview = {
      ...finalRemovePreview,
      projects: finalRemovePreview.projects.map((project) => ({
        ...project,
        ready: 0,
        needsArchiveV2: 0,
        blocked: project.totalObjects
      })),
      objects: finalRemovePreview.objects.map((object) => ({
        ...object,
        eligibility: "blocked",
        reasonCode: "capacityBlocked"
      })),
      eligibleTopologyGroupIds: [],
      blockedObjects: finalRemovePreview.objects.length
    };
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true, mutationMessage: null, mutationActivity: activityLog, mutationBusy: false, finalRemoveExecutionUnknown: false,
      finalRemoveProgress: null, finalRemoveJobId: null, finalRemoveBatchId: null,
      finalRemovePreview: blockedPreview, finalRemovePreviewLoading: false, finalRemoveUnavailableReason: null, finalRemoveResult: null,
      finalRemoveEnabled: true, finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Review batch cleanup…");
    expect(html).toContain("Review why blocked…");
    expect(html).toContain("explicitly capacity-blocked by this preview");
    expect(html).toContain("0 B projected release — no eligible deletion on this volume in the current preview");
    expect(html).not.toContain("8.0 KiB projected release");
  });

  it("shows a specific unavailable reason and refuses a legacy single-entry fallback", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true, mutationMessage: null, mutationActivity: activityLog, mutationBusy: false, finalRemoveExecutionUnknown: false,
      finalRemoveProgress: null, finalRemoveJobId: null, finalRemoveBatchId: null,
      finalRemovePreview: null, finalRemovePreviewLoading: false,
      finalRemoveUnavailableReason: "Object archive v2 command is missing.", finalRemoveResult: null,
      finalRemoveEnabled: true, finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Final cleanup is not available in this backend");
    expect(html).toContain("Object archive v2 command is missing.");
    expect(html).toContain("will not fall back to the older single-file delete path");
    expect(html).not.toContain(">Final remove<");
  });

  it("keeps a persisted interrupted batch visible and stoppable even with empty history", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true,
      mutationMessage: null,
      mutationActivity: { enabled: true, message: "Current", operations: [], items: [], backups: [], storedEntries: [] },
      mutationBusy: true,
      finalRemoveExecutionUnknown: true,
      finalRemoveProgress: { batchId: "batch-persisted", phase: "roundtrip", total: 4, completed: 2 },
      finalRemoveJobId: "job-persisted",
      finalRemoveBatchId: "batch-persisted",
      finalRemovePreview: null,
      finalRemovePreviewLoading: false,
      finalRemoveUnavailableReason: "Persisted final-cleanup work requires reconciliation.",
      finalRemoveResult: null,
      finalRemoveEnabled: false,
      finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Final cleanup is awaiting journal reconciliation");
    expect(html).toContain("batch-persisted");
    expect(html).toContain("Cancel archive preparation");
    expect(html).not.toContain("Nothing to recover");
  });

  it("keeps in-session unknown work visible by immutable identity even without an unavailable-reason string", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true,
      mutationMessage: null,
      mutationActivity: { enabled: true, message: "Current", operations: [], items: [], backups: [], storedEntries: [] },
      mutationBusy: true,
      finalRemoveExecutionUnknown: true,
      finalRemoveProgress: { batchId: "batch-timeout", phase: "deleting", total: 8, completed: 3 },
      finalRemoveJobId: "job-timeout",
      finalRemoveBatchId: "batch-timeout",
      finalRemovePreview: null,
      finalRemovePreviewLoading: true,
      finalRemoveUnavailableReason: null,
      finalRemoveResult: null,
      finalRemoveEnabled: false,
      finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Final cleanup is awaiting journal reconciliation");
    expect(html).toContain("batch-timeout");
    expect(html).toContain("job-timeout");
    expect(html).toContain("Stop after current object/group");
    expect(html).toContain("did not reach a proven terminal journal state");
  });

  it("shows an acknowledged stop as pending at the topology-group boundary", () => {
    const html = renderToStaticMarkup(createElement(RecoveryView, {
      mutationAvailable: true,
      mutationMessage: null,
      mutationActivity: { enabled: true, message: "Current", operations: [], items: [], backups: [], storedEntries: [] },
      mutationBusy: true,
      finalRemoveExecutionUnknown: true,
      finalRemoveProgress: { batchId: "batch-stopping", phase: "stoppingAfterCurrentTopologyGroup", total: 8, completed: 3 },
      finalRemoveJobId: "job-stopping",
      finalRemoveBatchId: "batch-stopping",
      finalRemovePreview: null,
      finalRemovePreviewLoading: true,
      finalRemoveUnavailableReason: "The batch is still reaching a terminal boundary.",
      finalRemoveResult: null,
      finalRemoveEnabled: false,
      finalRemoveCapabilityLoading: false,
      advancedMode: false, projects: [], appRemovals: [], restoreAppRemoval: async () => undefined,
      refreshMutationActivity: async () => true, runMutationRestore: () => undefined,
      runMutationRestoreElsewhere: () => undefined, onReviewFinalRemove: () => undefined, onStopFinalRemoveBatch: () => undefined,
      onSetFinalRemoveEnabled: async () => true,
      onDiscoverProjects: () => undefined, onOpenScanFolders: () => undefined, currentFile: null,
      onFileHistoryMutated: () => undefined, setStatusText: () => undefined
    }));
    expect(html).toContain("Stop requested — finishing current object/group…");
    expect(html).toContain("stop requested; finishing the current safe object/group boundary");
    expect(html).toContain("disabled");
  });
});

describe("recovery overview and progressive disclosure", () => {
  it("labels a content-mismatched restore as manual-review history, not held data", () => {
    expect(recoveryStoredEntryStatusLabel("restore_content_mismatch")).toBe(
      "Restore destination has different content"
    );
    expect(recoveryStoredEntryStatusLabel("permanently_deleted")).toBe(
      "Held copy deleted; recovery archive kept"
    );
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
