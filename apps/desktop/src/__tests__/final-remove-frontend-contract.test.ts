// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const recoveryView = readFileSync(new URL("../views/RecoveryView.tsx", import.meta.url), "utf8");
const reviewDialog = readFileSync(new URL("../views/FinalRemoveReviewDialog.tsx", import.meta.url), "utf8");
const api = readFileSync(new URL("../api.ts", import.meta.url), "utf8");

describe("final cleanup frontend contract", () => {
  it("keeps permanent removal complete but off until an exact owner activation", () => {
    expect(app).toContain("api.mutationFinalRemoveEnabled()");
    expect(app).toContain("api.mutationSetFinalRemoveEnabled(enabled, acknowledgement ?? null)");
    expect(app).toContain("finalRemoveEnabled={finalRemoveEnabled}");
    expect(app).toContain("onSetFinalRemoveEnabled={setFinalRemoveCapability}");
    expect(recoveryView).toContain("finalRemoveEnabled");
    expect(recoveryView).toContain("Off by default");
    expect(recoveryView).toContain("ENABLE PERMANENT REMOVAL");
    expect(recoveryView).toContain("analysis and Safe Manage never enable it for you");
    expect(recoveryView).toContain("Finish removing held projects");
  });

  it("starts one batch from a preview-and-group-bound confirmation", () => {
    expect(app).toContain("review.preview.previewId");
    expect(app).toContain("review.preview.previewDigest");
    expect(app).toContain("selectedTopologyGroupIds");
    expect(app.match(/api\.mutationFinalRemoveConfirm\(/gu)).toHaveLength(1);
    expect(app.match(/api\.mutationFinalRemoveBatchStart\(/gu)).toHaveLength(1);
    expect(app).not.toContain("api.mutationFinalRemoveStart(");
  });

  it("proves the persisted journal is idle before offering a fresh preview", () => {
    const dashboardCall = app.indexOf("api.mutationRecoveryDashboard()");
    const disabledCheck = app.indexOf("if (!finalRemoveEnabled)", dashboardCall);
    const previewCall = app.indexOf('api.mutationFinalRemovePreview({ kind: "allEligible" })');
    expect(dashboardCall).toBeGreaterThan(-1);
    expect(disabledCheck).toBeGreaterThan(dashboardCall);
    expect(previewCall).toBeGreaterThan(disabledCheck);
    expect(app).toContain('recoveryState !== "idle"');
    expect(app).toContain('recoveryState === "idle"');
    expect(app).toContain("reported idle while retaining a batch, job or phase identity");
    expect(app).toContain("isFinalRemovePhase");
    expect(app).toContain("preview.requiresElevation !== preview.objects.some");
    expect(app).toContain("Another cleanup batch cannot start until the journal reports idle");
    expect(api).toContain('"mutation_recovery_dashboard"');
    expect(api).toContain("cannot prove whether a final-cleanup batch is active or interrupted");
    expect(app).toContain("api.mutationFinalRemovePreview(scope)");
    expect(app).toContain('await refreshFinalRemovePreview() !== "ready"');
  });

  it("revalidates the backend object cap before confirmation", () => {
    expect(app).toContain("selectedObjectCount > review.preview.maxDeleteObjects");
    expect(app).toContain("Capacity blocked:");
    expect(reviewDialog).toContain("selection.capacityExceeded");
    expect(reviewDialog).toContain("Nothing can be confirmed from this review");
  });

  it("keeps topology validation linear and locks ambiguous batch-start responses", () => {
    expect(app).toContain("heldFinalRemoveMembersByTopology");
    expect(app).toContain("const heldMembersByTopology = heldFinalRemoveMembersByTopology(review.preview.objects)");
    const invoked = app.indexOf("batchStartInvoked = true");
    const started = app.indexOf("await api.mutationFinalRemoveBatchStart");
    expect(invoked).toBeGreaterThan(-1);
    expect(started).toBeGreaterThan(invoked);
    expect(app).toContain("assertFinalRemoveBatchStatus(status, started.batchId)");
    expect(app).toContain("terminal result during a non-terminal phase");
    expect(app).toContain("isExplicitlyEligibleFinalRemoveObject");
    expect(app).toContain("result.deletedObjects + result.keptObjects + result.failedObjects !== result.requestedObjects");
  });

  it("keeps persisted work operable without falsely enabling another batch", () => {
    expect(recoveryView).toContain("onStopFinalRemoveBatch");
    expect(recoveryView).toContain("Reconcile and stop persisted batch");
    expect(reviewDialog).toContain("Submitting verified batch…");
    expect(reviewDialog).toContain('tabIndex={-1}');
    expect(app).toContain("refreshOutcome === \"journalBlocked\"");
    expect(app).toContain("Keep the last backend-reported phase");
    expect(app).not.toContain('{ ...current, phase: "interrupted" }');
    expect(app).toContain("prevents duplicate history/preview refreshes");
  });

  it("keeps the legacy command only for compatibility and refuses it as a fallback", () => {
    expect(api).toContain('"mutation_final_remove_start"');
    expect(api).toContain('"mutation_final_remove_batch_start"');
    expect(api).toContain("Held objects remain restorable");
    expect(app).toContain("No legacy single-file deletion fallback will be used");
    expect(recoveryView).toContain("will not fall back to the older single-file delete path");
  });

  it("uses a labelled structured dialog and tells the truth about retained archives", () => {
    expect(reviewDialog).toContain('aria-labelledby="final-remove-review-title"');
    expect(reviewDialog).toContain('aria-describedby="final-remove-review-description"');
    expect(reviewDialog).toContain("Recovery archives are kept");
    expect(reviewDialog).toContain("Stop after current object/group");
    expect(reviewDialog).toContain("stoppingAfterCurrentTopologyGroup");
    expect(reviewDialog).toContain("No new topology group will start");
    expect(reviewDialog).toContain("Blocked subtrees stay held");
  });
});
