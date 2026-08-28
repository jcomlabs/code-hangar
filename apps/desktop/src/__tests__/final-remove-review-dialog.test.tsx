import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { fixtureApi } from "../fixtures";
import {
  FinalRemoveReviewDialog,
  finalRemoveReasonLabel,
  finalRemoveResultSummary,
  finalRemoveReviewSelection,
  formatSignedBytes
} from "../views/FinalRemoveReviewDialog";

describe("final cleanup review dialog", () => {
  it("selects eligible topology groups while keeping unsupported subtrees", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "allEligible" });
    expect(preview.previewDigest).toMatch(/^v2:[0-9a-f]{64}$/u);
    const selection = finalRemoveReviewSelection(preview, { kind: "project", groupId: "fixture-markdown-project" });

    expect(selection.selectedTopologyGroupIds).toEqual([
      "fixture-topology-readme",
      "fixture-topology-context"
    ]);
    expect(selection).toMatchObject({
      readyObjects: 1,
      needsArchiveV2Objects: 1,
      blockedObjects: 1,
      blockedSubtrees: 1,
      deleteObjects: 2,
      capacityBlockedObjects: 0,
      capacityLimit: 3,
      capacityExceeded: false,
      capacityOverflow: 0
    });
    expect(selection.selectedTopologyGroupIds).not.toContain("fixture-topology-reparse");
  });

  it("fails a whole topology group closed when one held member is blocked", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const adversarialPreview = {
      ...preview,
      objects: preview.objects.map((object) => object.entryId === 103
        ? { ...object, topologyGroupId: "fixture-topology-readme" }
        : object)
    };
    const selection = finalRemoveReviewSelection(adversarialPreview, { kind: "project", groupId: "fixture-markdown-project" });
    expect(selection.selectedTopologyGroupIds).not.toContain("fixture-topology-readme");
    expect(selection.selectedTopologyGroupIds).toEqual(["fixture-topology-context"]);
    expect(selection.readyObjects).toBe(0);
    expect(selection.needsArchiveV2Objects).toBe(1);
    expect(selection.blockedObjects).toBe(2);
  });

  it("renders one structured confirmation with volume truth and explicit archive retention", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "allEligible" });
    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview,
      scope: { kind: "allEligible" },
      busy: false,
      progress: null,
      result: null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));

    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-labelledby="final-remove-review-title"');
    expect(html).toContain('aria-describedby="final-remove-review-description"');
    expect(html).toContain("Recovery archives are kept");
    expect(html).toContain("Windows will ask once for this batch");
    expect(html).toContain("Projected final-cleanup effects by volume");
    expect(html).toContain("Blocked subtrees stay held");
    expect(html).toContain("Delete 3 eligible held objects");
    expect(html).toContain("Keep 2 blocked objects and all recovery archives");
    expect(html).toContain("Delete 3 held objects");
    expect(html).toContain("data-dialog-initial-focus");
    expect(html).not.toContain("enable final removal");
  });

  it("renders a partial result as deleted plus remaining, never as all-or-nothing success", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "allEligible" });
    const confirmation = await fixtureApi.mutationFinalRemoveConfirm(
      preview.previewId,
      preview.previewDigest,
      preview.eligibleTopologyGroupIds
    );
    const started = await fixtureApi.mutationFinalRemoveBatchStart({
      previewId: preview.previewId,
      previewDigest: preview.previewDigest,
      selectedTopologyGroupIds: preview.eligibleTopologyGroupIds,
      confirmationToken: confirmation.token
    });
    const status = await fixtureApi.mutationFinalRemoveBatchStatus(started.jobId);
    expect(status.result?.status).toBe("partial");
    expect(status.result?.archiveRetained).toBe(true);

    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview,
      scope: { kind: "allEligible" },
      busy: false,
      progress: status.progress,
      result: status.result ?? null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));
    expect(html).toContain("Cleanup completed partially");
    expect(html).toContain("Recovery archives were kept");
    expect(html).toContain("Objects that remain or need review");
    expect(html).toContain("Synthetic fixture failure");
    expect(html).toContain("Result by project");
    expect(html).toContain("Fixture Git-like Project");
    expect(html).toContain("build\\old-output.bin");
    expect(html).toContain("Released from holding");
    expect(html).toContain("Observed change");
    expect(html).toContain("+12.0 KiB");
    expect(html).toContain("Projects (D:)");
    expect(html).toContain("<td>0 B</td>");
  });

  it("offers cancellation before deletion and stops only at an atomic object/group boundary", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const baseProps = {
      preview,
      scope: { kind: "project" as const, groupId: "fixture-markdown-project" },
      busy: true,
      result: null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    };
    const preparing = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      ...baseProps,
      progress: { batchId: "batch", phase: "roundtrip", total: 2, completed: 0 }
    }));
    expect(preparing).toContain("Cancel archive preparation");
    expect(preparing).toContain("deletes zero held objects");
    expect(preparing).not.toContain("Stop after current object/group");

    const submitting = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      ...baseProps,
      canStop: false,
      progress: { batchId: "pending", phase: "waitingForUac", total: 2, completed: 0 }
    }));
    expect(submitting).toContain("Submitting verified batch…");
    expect(submitting).toContain("Waiting for the backend to return the immutable batch and job identity");
    expect(submitting).toContain('role="dialog" tabindex="-1"');
    expect(submitting).not.toContain("Cancel archive preparation");

    const deleting = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      ...baseProps,
      progress: { batchId: "batch", phase: "deleting", total: 2, completed: 1 }
    }));
    expect(deleting).toContain("Stop after current object/group");
    expect(deleting).toContain("Already deleted held copies will not return");
    expect(deleting).toContain("inseparable topology group");

    const disposingParents = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      ...baseProps,
      progress: { batchId: "batch", phase: "parentDisposition", total: 2, completed: 0 }
    }));
    expect(disposingParents).toContain("Applying verified parent dispositions");
    expect(disposingParents).toContain("Stop after current object/group");

    const stopping = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      ...baseProps,
      progress: { batchId: "batch", phase: "stoppingAfterCurrentTopologyGroup", total: 2, completed: 1 }
    }));
    expect(stopping).toContain("Stop requested — finishing current object/group…");
    expect(stopping).toContain("No new topology group will start");
    expect(stopping).toContain("disabled");
  });

  it("blocks confirmation when the selected objects exceed the backend-provided capacity", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "allEligible" });
    const capacityPreview = {
      ...preview,
      maxDeleteObjects: 2,
      objects: preview.objects.map((object) => object.entryId === 103
        ? {
            ...object,
            reasonCode: "capacityBlocked" as const,
            reason: "The verified transport cannot carry this object in the current batch.",
            remediation: "Choose a smaller verified project batch."
          }
        : object)
    };
    const selection = finalRemoveReviewSelection(capacityPreview, { kind: "allEligible" });
    expect(selection).toMatchObject({
      deleteObjects: 3,
      capacityBlockedObjects: 1,
      capacityLimit: 2,
      capacityExceeded: true,
      capacityOverflow: 1
    });

    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview: capacityPreview,
      scope: { kind: "allEligible" },
      busy: false,
      progress: null,
      result: null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));
    expect(html).toContain("Selection is larger than this verified batch can carry");
    expect(html).toContain("explicitly marks 1 held object as capacity-blocked");
    expect(html).toContain("at most 2 objects per batch");
    expect(html).toContain("Nothing can be confirmed from this review");
    expect(html).toContain("Why blocked objects stay held");
    expect(html).toContain("Selection exceeds the current verified transport capacity");
    expect(html).toContain("Choose a smaller verified project batch");
    expect(html).toContain('type="checkbox" disabled=""');
    expect(html).toContain('class="danger-button" disabled=""');
  });

  it("keeps an all-capacity-blocked preview reviewable with zero projected deletion", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const blockedPreview = {
      ...preview,
      eligibleTopologyGroupIds: [],
      blockedObjects: preview.objects.length,
      objects: preview.objects.map((object) => ({
        ...object,
        eligibility: "blocked" as const,
        reasonCode: "capacityBlocked" as const,
        reason: "The current verified transport is full.",
        remediation: "Split the selection into a smaller verified preview."
      }))
    };
    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview: blockedPreview,
      scope: { kind: "project", groupId: "fixture-markdown-project" },
      busy: false,
      progress: null,
      result: null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));
    expect(html).toContain("This preview is capacity-blocked");
    expect(html).toContain("explicitly marks 3 held objects as capacity-blocked");
    expect(html).toContain("at most 2 objects per batch");
    expect(html).toContain("Selection exceeds the current verified transport capacity");
    expect(html).toContain("Split the selection into a smaller verified preview");
    expect(html).toContain("Projected final-cleanup effects by volume");
    expect(html).toContain("No deletion confirmation is available");
    expect(html).toContain("Deletion unavailable");
    expect(html).not.toContain("Delete 0");
    expect(html).toContain('class="danger-button" disabled=""');
  });

  it("keeps signed observed free-space deltas and unresolved disposition states visible", async () => {
    expect(formatSignedBytes(-1_024)).toBe("−1.0 KiB");
    expect(formatSignedBytes(2_048)).toBe("+2.0 KiB");

    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const result = {
      batchId: "batch-unresolved",
      status: "interrupted" as const,
      requestedObjects: 1,
      deletedObjects: 0,
      keptObjects: 1,
      failedObjects: 0,
      projects: [{ groupId: "fixture-markdown-project", deleted: 0, kept: 1, failed: 0 }],
      volumes: [{ ...preview.volumes[0], projectedReleaseBytes: 0, observedDeltaBytes: -1_024 }],
      items: [{ entryId: 101, state: "deleteIntent" as const, reasonCode: "archiveVerified" as const }],
      archiveRetained: true as const
    };
    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview,
      scope: { kind: "project", groupId: "fixture-markdown-project" },
      busy: false,
      progress: { batchId: result.batchId, phase: "interrupted", total: 1, completed: 0 },
      result,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));
    expect(html).toContain("Fixture Markdown Project: README.md");
    expect(html).toContain("Deletion intent requires journal reconciliation");
    expect(html).toContain("Object-complete archive verified");
    expect(html).toContain("−1.0 KiB");
  });

  it("clamps progressbar values defensively when rendering an invalid raw count", async () => {
    const preview = await fixtureApi.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const html = renderToStaticMarkup(createElement(FinalRemoveReviewDialog, {
      preview,
      scope: { kind: "project", groupId: "fixture-markdown-project" },
      busy: true,
      progress: { batchId: "batch", phase: "deleting", total: 2, completed: 99 },
      result: null,
      error: null,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onStop: () => undefined
    }));
    expect(html).toContain('aria-valuemax="2"');
    expect(html).toContain('aria-valuenow="2"');
  });

  it("keeps reason codes stable and human-readable", () => {
    expect(finalRemoveReasonLabel("externalHardlink")).toContain("outside this cleanup group");
    expect(finalRemoveReasonLabel("insufficientSpace")).toContain("Not enough local space");
    expect(finalRemoveReasonLabel("permissionDenied")).toContain("Windows denied");
    expect(finalRemoveReasonLabel("helperUnsigned")).toContain("not signed");
    expect(finalRemoveReasonLabel("helperUntrusted")).toContain("not trusted");
    expect(finalRemoveReasonLabel("releaseManifestMismatch")).toContain("signed release manifest");
    expect(finalRemoveReasonLabel("uacCancelled")).toContain("no held object was deleted");
    expect(finalRemoveReasonLabel("capacityBlocked")).toContain("verified transport capacity");
    expect(finalRemoveResultSummary({
      batchId: "cancelled",
      status: "cancelled",
      requestedObjects: 2,
      deletedObjects: 0,
      keptObjects: 2,
      failedObjects: 0,
      projects: [],
      volumes: [],
      items: [],
      archiveRetained: true
    })).toContain("recovery archives were kept");
  });
});
