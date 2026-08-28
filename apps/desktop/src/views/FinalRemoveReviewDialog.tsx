import { useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { AlertTriangle, Archive, CheckCircle2, HardDrive, ShieldCheck, XCircle } from "lucide-react";
import { DIALOG_FOCUSABLE_SELECTOR, DIALOG_INITIAL_FOCUS_SELECTOR, nextDialogFocusIndex } from "../dialogFocus";
import type {
  FinalRemoveBatchPreview,
  FinalRemoveBatchProgress,
  FinalRemoveBatchResult,
  FinalRemoveObjectDecision,
  FinalRemoveProjectPreview,
  FinalRemoveReasonCode,
  FinalRemoveScope,
  FinalRemoveVolumeImpact
} from "../types";
import { displayLocalPath, formatBytes } from "../ui";

export interface FinalRemoveReviewSelection {
  projects: FinalRemoveProjectPreview[];
  objects: FinalRemoveObjectDecision[];
  selectedTopologyGroupIds: string[];
  readyObjects: number;
  needsArchiveV2Objects: number;
  blockedObjects: number;
  blockedSubtrees: number;
  volumeLabels: string[];
  deleteObjects: number;
  capacityBlockedObjects: number;
  capacityLimit: number | null;
  capacityExceeded: boolean;
  capacityOverflow: number;
}

function isExplicitlyEligible(object: FinalRemoveObjectDecision): boolean {
  return object.eligibility === "ready" || object.eligibility === "needsArchiveV2";
}

export function finalRemoveReviewSelection(
  preview: FinalRemoveBatchPreview,
  scope: FinalRemoveScope
): FinalRemoveReviewSelection {
  const selectedGroupIds = scope.kind === "project"
    ? new Set([scope.groupId])
    : scope.kind === "groups"
      ? new Set(scope.groupIds)
      : new Set(preview.projects.map((project) => project.groupId));
  const projects = preview.projects.filter((project) => selectedGroupIds.has(project.groupId));
  const objects = preview.objects.filter((object) => selectedGroupIds.has(object.groupId) && object.lifecycle === "held");
  const eligibleTopologyIds = new Set(preview.eligibleTopologyGroupIds);
  const heldMembersByTopology = new Map<string, FinalRemoveObjectDecision[]>();
  for (const object of preview.objects) {
    if (object.lifecycle !== "held") continue;
    const members = heldMembersByTopology.get(object.topologyGroupId) ?? [];
    members.push(object);
    heldMembersByTopology.set(object.topologyGroupId, members);
  }
  const candidateTopologyIds = new Set(objects
    .filter(isExplicitlyEligible)
    .map((object) => object.topologyGroupId));
  const selectedTopologyGroupIds = Array.from(candidateTopologyIds).filter((topologyGroupId) => {
    if (!eligibleTopologyIds.has(topologyGroupId)) return false;
    const heldMembers = heldMembersByTopology.get(topologyGroupId) ?? [];
    return heldMembers.length > 0
      && heldMembers.every((object) => selectedGroupIds.has(object.groupId) && isExplicitlyEligible(object));
  });
  const selectedTopologyIds = new Set(selectedTopologyGroupIds);
  const selectedObjects = objects.filter((object) => (
    isExplicitlyEligible(object) && selectedTopologyIds.has(object.topologyGroupId)
  ));
  const volumeLabels = Array.from(new Set(objects
    .filter((object) => isExplicitlyEligible(object) && selectedTopologyIds.has(object.topologyGroupId))
    .map((object) => object.heldVolumeLabel)));
  const capacityLimit = Number.isSafeInteger(preview.maxDeleteObjects) && preview.maxDeleteObjects >= 0
    ? preview.maxDeleteObjects
    : null;
  const capacityBlockedObjects = objects.filter((object) => (
    object.eligibility === "blocked" && object.reasonCode === "capacityBlocked"
  )).length;
  const capacityOverflow = capacityLimit === null
    ? selectedObjects.length + capacityBlockedObjects
    : Math.max(0, selectedObjects.length - capacityLimit, capacityBlockedObjects);
  const capacityExceeded = capacityLimit === null || capacityOverflow > 0;
  return {
    projects,
    objects,
    selectedTopologyGroupIds,
    readyObjects: selectedObjects.filter((object) => object.eligibility === "ready").length,
    needsArchiveV2Objects: selectedObjects.filter((object) => object.eligibility === "needsArchiveV2").length,
    blockedObjects: objects.length - selectedObjects.length,
    blockedSubtrees: projects.reduce((total, project) => total + project.blockedSubtrees.length, 0),
    volumeLabels,
    deleteObjects: selectedObjects.length,
    capacityBlockedObjects,
    capacityLimit,
    capacityExceeded,
    capacityOverflow
  };
}

export function finalRemoveReasonLabel(reason: FinalRemoveReasonCode): string {
  switch (reason) {
    case "archiveVerified": return "Object-complete archive verified";
    case "legacyContentOnly": return "Legacy backup covers content only";
    case "archiveMissing": return "Recovery archive is missing";
    case "archiveCorrupt": return "Recovery archive did not verify";
    case "unsupportedReparse": return "Reparse point is not supported";
    case "unsupportedEfs": return "Encrypted EFS object is not supported";
    case "unsupportedObjectStream": return "Windows object stream is not supported";
    case "externalHardlink": return "Hardlink exists outside this cleanup group";
    case "nonNtfs": return "Volume does not support the required NTFS proof";
    case "cloudOrRecall": return "Cloud-backed object cannot be opened without recall";
    case "locked": return "Object is locked";
    case "identityChanged": return "Object identity changed after preview";
    case "insufficientSpace": return "Not enough local space to verify the recovery archive";
    case "permissionDenied": return "Windows denied the required object access";
    case "helperUnsigned": return "Elevated helper is not signed";
    case "helperUntrusted": return "Elevated helper identity is not trusted";
    case "releaseManifestMismatch": return "Helper does not match the signed release manifest";
    case "uacCancelled": return "Windows approval was cancelled; no held object was deleted";
    case "capacityBlocked": return "Selection exceeds the current verified transport capacity";
    case "scratchCleanupPending": return "Temporary verification data needs cleanup";
    case "stopRequested": return "Stop preserved this not-yet-deleted object";
    case "interrupted": return "An earlier cleanup was interrupted";
  }
}

export function finalRemoveMeasurementLabel(impact: FinalRemoveVolumeImpact): string {
  switch (impact.quality) {
    case "exactObjectAllocation": return "Exact object allocation";
    case "estimated": return "Estimate — not an exact free-space promise";
    case "observedNoisy": return "Observed free-space change — other activity may affect it";
  }
}

export function finalRemoveResultSummary(result: FinalRemoveBatchResult): string {
  const kept = result.keptObjects + result.failedObjects;
  if (result.status === "completed") {
    return `${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted. Recovery archives were kept.`;
  }
  if (result.status === "cancelled") {
    return `Cleanup was cancelled after ${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted. ${kept} selected object${kept === 1 ? " remains" : "s remain"} held, and recovery archives were kept.`;
  }
  return `${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted; ${kept} selected object${kept === 1 ? " remains" : "s remain"} held or needs review. Recovery archives were kept.`;
}

function finalRemoveItemStateLabel(state: FinalRemoveBatchResult["items"][number]["state"]): string {
  switch (state) {
    case "planned": return "Held object was planned but not deleted";
    case "archiveFinalizing": return "Archive finalization did not reach a terminal proof";
    case "archiveVerified": return "Archive verified; held object remains";
    case "blocked": return "Held object is blocked";
    case "deleteIntent": return "Deletion intent requires journal reconciliation";
    case "deleteFailed": return "Deletion failed; held object needs review";
    case "kept": return "Held object was kept";
    case "deleted": return "Held object was deleted";
    case "reconciledDeleted": return "Held object deletion was reconciled";
  }
}

function useFinalRemoveDialogFocus(onCancel: () => void, canCancel: boolean) {
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const initial = dialogRef.current?.querySelector<HTMLElement>(DIALOG_INITIAL_FOCUS_SELECTOR);
    initial?.focus();
    return () => previouslyFocused?.focus();
  }, []);
  const onDialogKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape" && canCancel) {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR) ?? []);
    const currentIndex = focusable.indexOf(document.activeElement as HTMLElement);
    const nextIndex = nextDialogFocusIndex(focusable.length, currentIndex, event.shiftKey);
    event.preventDefault();
    if (nextIndex < 0) {
      dialogRef.current?.focus();
      return;
    }
    focusable[nextIndex]?.focus();
  };
  return { dialogRef, onDialogKeyDown };
}

function selectedVolumes(preview: FinalRemoveBatchPreview, selection: FinalRemoveReviewSelection) {
  const selectedTopologyIds = new Set(selection.selectedTopologyGroupIds);
  if (selectedTopologyIds.size === 0) {
    return preview.volumes.map((volume) => ({ ...volume, projectedReleaseBytes: 0 }));
  }
  const volumeIds = new Set(selection.objects
    .filter((object) => isExplicitlyEligible(object) && selectedTopologyIds.has(object.topologyGroupId))
    .map((object) => object.heldVolumeId));
  return preview.volumes.filter((volume) => volumeIds.has(volume.volumeId));
}

function blockedReasonSummaries(selection: FinalRemoveReviewSelection) {
  const summaries = new Map<FinalRemoveReasonCode, { count: number; detail: string }>();
  for (const object of selection.objects) {
    if (isExplicitlyEligible(object)) continue;
    const current = summaries.get(object.reasonCode);
    summaries.set(object.reasonCode, {
      count: (current?.count ?? 0) + 1,
      detail: current?.detail ?? object.remediation ?? object.reason
    });
  }
  return Array.from(summaries, ([reasonCode, value]) => ({ reasonCode, ...value }));
}

function VolumeImpactTable({ volumes, result = false }: { volumes: FinalRemoveVolumeImpact[]; result?: boolean }) {
  if (volumes.length === 0) {
    return <p className="muted final-remove-no-volume">No volume impact is available for this selection.</p>;
  }
  return (
    <div className="final-remove-volume-scroll">
      <table className="final-remove-volume-table">
        <caption>{result ? "Recorded final-cleanup effects by volume" : "Projected final-cleanup effects by volume"}</caption>
        <thead>
          <tr>
            <th scope="col">Volume</th>
            <th scope="col">Already freed at move</th>
            <th scope="col">{result ? "Held before" : "Held now"}</th>
            <th scope="col">{result ? "Released from holding" : "Projected release"}</th>
            <th scope="col">Archive kept</th>
            {result ? <th scope="col">Observed change</th> : null}
          </tr>
        </thead>
        <tbody>
          {volumes.map((volume) => (
            <tr key={volume.volumeId}>
              <th scope="row">{volume.label}<small>{finalRemoveMeasurementLabel(volume)}</small></th>
              <td>{formatBytes(volume.alreadyFreedFromSourceBytes)}</td>
              <td>{formatBytes(volume.heldAllocatedBytes)}</td>
              <td>{formatBytes(volume.projectedReleaseBytes)}</td>
              <td>{formatBytes(volume.archiveRetainedAllocatedBytes)}</td>
              {result ? <td>{volume.observedDeltaBytes == null ? "Not measured" : formatSignedBytes(volume.observedDeltaBytes)}</td> : null}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function formatSignedBytes(value: number): string {
  if (!Number.isFinite(value)) return "Unknown";
  if (value === 0) return "0 B";
  return `${value > 0 ? "+" : "−"}${formatBytes(Math.abs(value))}`;
}

export function FinalRemoveReviewDialog({
  preview,
  scope,
  busy,
  canStop = true,
  progress,
  result,
  error,
  onCancel,
  onConfirm,
  onStop
}: {
  preview: FinalRemoveBatchPreview;
  scope: FinalRemoveScope;
  busy: boolean;
  canStop?: boolean;
  progress: FinalRemoveBatchProgress | null;
  result: FinalRemoveBatchResult | null;
  error: string | null;
  onCancel: () => void;
  onConfirm: (selectedTopologyGroupIds: string[]) => void | Promise<void>;
  onStop: () => void | Promise<void>;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const stopButtonRef = useRef<HTMLButtonElement>(null);
  const resultRef = useRef<HTMLDivElement>(null);
  const selection = useMemo(() => finalRemoveReviewSelection(preview, scope), [preview, scope]);
  const volumes = useMemo(() => selectedVolumes(preview, selection), [preview, selection]);
  const blockedReasons = useMemo(() => blockedReasonSummaries(selection), [selection]);
  const previewObjectsByEntryId = useMemo(
    () => new Map(preview.objects.map((object) => [object.entryId, object])),
    [preview.objects]
  );
  const previewProjectsByGroupId = useMemo(
    () => new Map(preview.projects.map((project) => [project.groupId, project])),
    [preview.projects]
  );
  const deleteCount = selection.deleteObjects;
  const selectionLabel = selection.projects.length === 1
    ? selection.projects[0]?.projectName ?? "selected project"
    : `${selection.projects.length} projects`;
  const volumeLabel = selection.volumeLabels.length > 0 ? selection.volumeLabels.join(" and ") : "the listed volumes";
  const deletionStarted = progress?.phase === "parentDisposition"
    || progress?.phase === "deleting"
    || progress?.phase === "cleaningDirs"
    || progress?.phase === "finished";
  const stopPending = progress?.phase === "stoppingAfterCurrentTopologyGroup";
  const executionUnknown = progress?.phase === "interrupted" || progress?.phase === "finished";
  const canCancel = !busy || result !== null || executionUnknown;
  const { dialogRef, onDialogKeyDown } = useFinalRemoveDialogFocus(onCancel, canCancel);
  const stopLabel = !canStop
    ? "Submitting verified batch…"
    : stopPending
    ? "Stop requested — finishing current object/group…"
    : executionUnknown
    ? "Reconcile and stop batch"
    : deletionStarted
      ? "Stop after current object/group"
      : "Cancel archive preparation";
  useEffect(() => {
    if (result) {
      resultRef.current?.focus();
    } else if (busy) {
      if (canStop && !stopPending) {
        stopButtonRef.current?.focus();
      } else {
        dialogRef.current?.focus();
      }
    }
  }, [busy, canStop, dialogRef, result, stopPending]);

  return (
    <div className="dialog-backdrop final-remove-backdrop" role="presentation" onMouseDown={() => { if (canCancel) onCancel(); }}>
      <div
        ref={dialogRef}
        className="command-dialog final-remove-review-dialog"
        role="dialog"
        tabIndex={-1}
        aria-modal="true"
        aria-labelledby="final-remove-review-title"
        aria-describedby="final-remove-review-description"
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onDialogKeyDown}
      >
        <header className="dialog-header final-remove-dialog-header">
          <div>
            <h2 id="final-remove-review-title">{result ? "Final cleanup result" : `Review final cleanup — ${selectionLabel}`}</h2>
            <p id="final-remove-review-description">
              {result
                ? "This result distinguishes deleted held copies from objects that remain. Recovery archives are not deleted by this action."
                : "Delete only held objects that this immutable preview proves eligible. Blocked objects and every recovery archive stay on disk."}
            </p>
          </div>
          {canCancel ? (
            <button type="button" className="icon-button" onClick={onCancel} aria-label={result ? "Close final cleanup result" : executionUnknown ? "Close review and keep final cleanup blocked" : "Cancel final cleanup"}>
              <XCircle size={17} />
            </button>
          ) : null}
        </header>

        {result ? (
          <div ref={resultRef} className={`final-remove-result ${result.status}`} role="status" aria-live="polite" tabIndex={-1}>
            <div className="final-remove-result-heading">
              {result.status === "completed" ? <CheckCircle2 size={20} /> : <AlertTriangle size={20} />}
              <div>
                <strong>{result.status === "completed" ? "Cleanup completed" : result.status === "partial" ? "Cleanup completed partially" : "Cleanup needs review"}</strong>
                <p>{finalRemoveResultSummary(result)}</p>
              </div>
            </div>
            <div className="final-remove-count-grid" aria-label="Final cleanup result counts">
              <span><strong>{result.deletedObjects}</strong> deleted</span>
              <span><strong>{result.keptObjects}</strong> kept</span>
              <span><strong>{result.failedObjects}</strong> failed</span>
            </div>
            <VolumeImpactTable volumes={result.volumes} result />
            {result.projects.length > 0 ? (
              <section className="final-remove-project-results" aria-labelledby="final-remove-project-results-heading">
                <h3 id="final-remove-project-results-heading">Result by project</h3>
                <ul>
                  {result.projects.map((project) => {
                    const previewProject = previewProjectsByGroupId.get(project.groupId);
                    return (
                      <li key={project.groupId}>
                        <strong>{previewProject?.projectName ?? `Project group ${project.groupId}`}</strong>
                        {previewProject ? <span>{displayLocalPath(previewProject.originalRoot)}</span> : null}
                        <small>{project.deleted} deleted · {project.kept} kept · {project.failed} failed</small>
                      </li>
                    );
                  })}
                </ul>
              </section>
            ) : null}
            {result.items.some((item) => item.state !== "deleted" && item.state !== "reconciledDeleted") ? (
              <section className="final-remove-exceptions" aria-labelledby="final-remove-result-exceptions">
                <h3 id="final-remove-result-exceptions">Objects that remain or need review</h3>
                <ul>
                  {result.items.filter((item) => item.state !== "deleted" && item.state !== "reconciledDeleted").map((item) => {
                    const object = previewObjectsByEntryId.get(item.entryId);
                    const project = object ? previewProjectsByGroupId.get(object.groupId) : undefined;
                    return (
                      <li key={item.entryId}>
                        <strong>
                          {object
                            ? `${project?.projectName ?? object.groupId}: ${displayLocalPath(object.relativePath)}`
                            : `Entry #${item.entryId} (project and path unavailable in this preview)`}
                        </strong>
                        <span>
                          {finalRemoveItemStateLabel(item.state)}
                          {item.reasonCode ? ` — ${finalRemoveReasonLabel(item.reasonCode)}` : ""}
                          {item.error ? ` — ${item.error}` : ""}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </section>
            ) : null}
            <div className="confirm-action-actions final-remove-dialog-actions">
              <button data-dialog-initial-focus type="button" className="primary-button" onClick={onCancel}>Close</button>
            </div>
          </div>
        ) : (
          <>
            <div className="final-remove-count-grid" aria-label="Final cleanup eligibility">
              <span className="ready"><ShieldCheck size={16} /><strong>{selection.readyObjects}</strong> ready</span>
              <span className="needs"><Archive size={16} /><strong>{selection.needsArchiveV2Objects}</strong> need archive v2</span>
              <span className="blocked"><AlertTriangle size={16} /><strong>{selection.blockedObjects}</strong> blocked</span>
            </div>

            <div className="final-remove-archive-note">
              <Archive size={18} aria-hidden="true" />
              <div>
                <strong>Recovery archives are kept</strong>
                <p>This action deletes eligible copies from the holding area. It does not erase their verified recovery archives.</p>
              </div>
            </div>

            {selection.capacityBlockedObjects > 0 ? (
              <p className="final-remove-uac-note"><AlertTriangle size={17} /> This preview is capacity-blocked, so no deletion authority or Windows approval can be requested. Review the capacity limit and remediation below.</p>
            ) : deleteCount === 0 ? (
              <p className="final-remove-uac-note"><AlertTriangle size={17} /> No held object is eligible for deletion in this review. Windows approval will not be requested; use the blocked reasons below to decide what can be remediated.</p>
            ) : preview.requiresElevation && selection.needsArchiveV2Objects > 0 ? (
              <p className="final-remove-uac-note"><ShieldCheck size={17} /> Windows will ask once for this batch so Code Hangar can finalize and round-trip-verify the required object archives. Cancelling that prompt deletes nothing.</p>
            ) : (
              <p className="final-remove-uac-note"><ShieldCheck size={17} /> Every selected object already has the archive proof required by this preview; no archive-finalization prompt is projected.</p>
            )}

            <VolumeImpactTable volumes={volumes} />

            {selection.capacityExceeded ? (
              <section className="final-remove-capacity-blocked" role="alert" aria-labelledby="final-remove-capacity-heading">
                <AlertTriangle size={18} aria-hidden="true" />
                <div>
                  <h3 id="final-remove-capacity-heading">Selection is larger than this verified batch can carry</h3>
                  <p>
                    {selection.capacityBlockedObjects > 0
                      ? `This scoped preview explicitly marks ${selection.capacityBlockedObjects} held object${selection.capacityBlockedObjects === 1 ? "" : "s"} as capacity-blocked. `
                      : `This review selects ${deleteCount} eligible held object${deleteCount === 1 ? "" : "s"}. `}
                    {selection.capacityLimit === null
                      ? "The backend did not provide a valid maximum object count."
                      : selection.capacityBlockedObjects > 0
                        ? `The current preview permits at most ${selection.capacityLimit} object${selection.capacityLimit === 1 ? "" : "s"} per batch.`
                        : `The current preview permits at most ${selection.capacityLimit} object${selection.capacityLimit === 1 ? "" : "s"} per batch (${selection.capacityOverflow} over the limit).`}
                  </p>
                  <p>Nothing can be confirmed from this review. Review a smaller project where possible; a project still above the limit stays held until the backend returns a narrower verified preview.</p>
                </div>
              </section>
            ) : null}

            {selection.projects.some((project) => project.blockedSubtrees.length > 0) ? (
              <section className="final-remove-blocked" aria-labelledby="final-remove-blocked-heading">
                <h3 id="final-remove-blocked-heading">Blocked subtrees stay held</h3>
                <ul>
                  {selection.projects.flatMap((project) => project.blockedSubtrees.map((subtree) => (
                    <li key={`${project.groupId}:${subtree.root}`}>
                      <strong>{project.projectName}: {displayLocalPath(subtree.root)}</strong>
                      <span>{subtree.count} object{subtree.count === 1 ? "" : "s"} — {subtree.reasonCodes.map(finalRemoveReasonLabel).join(", ")}</span>
                    </li>
                  )))}
                </ul>
                <p>The project folder and any parent directories needed to contain these subtrees will remain.</p>
              </section>
            ) : null}

            {blockedReasons.length > 0 ? (
              <section className="final-remove-blocked" aria-labelledby="final-remove-blocked-reasons-heading">
                <h3 id="final-remove-blocked-reasons-heading">Why blocked objects stay held</h3>
                <ul>
                  {blockedReasons.map((summary) => (
                    <li key={summary.reasonCode}>
                      <strong>{summary.count} object{summary.count === 1 ? "" : "s"}: {finalRemoveReasonLabel(summary.reasonCode)}</strong>
                      <span>{summary.detail}</span>
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}

            {progress ? (
              <div className="final-remove-progress" role="status" aria-live="polite">
                <div>
                  <strong>{finalRemovePhaseLabel(progress.phase)}</strong>
                  <span>{progress.completed} of {progress.total}</span>
                </div>
                <div
                  className="final-remove-progress-track"
                  role="progressbar"
                  aria-label="Final cleanup progress"
                  aria-valuemin={0}
                  aria-valuemax={Math.max(1, Number.isFinite(progress.total) ? progress.total : 1)}
                  aria-valuenow={Math.min(
                    Math.max(1, Number.isFinite(progress.total) ? progress.total : 1),
                    Math.max(0, Number.isFinite(progress.completed) ? progress.completed : 0)
                  )}
                >
                  <span style={{ width: `${progress.total > 0 ? Math.min(100, (progress.completed / progress.total) * 100) : 0}%` }} />
                </div>
                {progress.currentPath ? <small>{displayLocalPath(progress.currentPath)}</small> : null}
              </div>
            ) : null}

            {error ? <p className="scan-error final-remove-error" role="alert">{error}</p> : null}

            {deleteCount > 0 ? (
              <label className="confirm-action-acknowledge final-remove-acknowledge">
                <input
                  type="checkbox"
                  checked={acknowledged}
                  disabled={busy || selection.capacityExceeded}
                  onChange={(event) => setAcknowledged(event.target.checked)}
                />
                <span>
                  Delete {deleteCount} eligible held object{deleteCount === 1 ? "" : "s"} from {volumeLabel}. Keep {selection.blockedObjects} blocked object{selection.blockedObjects === 1 ? "" : "s"} and all recovery archives.
                </span>
              </label>
            ) : (
              <p className="confirm-action-acknowledge final-remove-acknowledge" role="status">
                No deletion confirmation is available. All {selection.blockedObjects} held object{selection.blockedObjects === 1 ? " remains" : "s remain"}, together with every recovery archive.
              </p>
            )}

            <div className="confirm-action-actions final-remove-dialog-actions">
              {busy ? (
                <button ref={stopButtonRef} type="button" className="secondary-button" disabled={!canStop || stopPending} onClick={() => void onStop()}>
                  {stopLabel}
                </button>
              ) : (
                <button data-dialog-initial-focus type="button" className="secondary-button" onClick={onCancel}>Cancel</button>
              )}
              <button
                type="button"
                className="danger-button"
                disabled={busy || !acknowledged || deleteCount === 0 || selection.capacityExceeded}
                onClick={() => void onConfirm(selection.selectedTopologyGroupIds)}
              >
                {busy
                  ? (stopPending ? "Stopping at a safe boundary…" : deletionStarted ? "Cleanup in progress…" : "Preparing verified archives…")
                  : deleteCount === 0
                    ? "Deletion unavailable"
                    : `Delete ${deleteCount} held object${deleteCount === 1 ? "" : "s"}`}
              </button>
            </div>
            {busy ? (
              <p className="muted final-remove-stop-note">
                {!canStop
                  ? "Waiting for the backend to return the immutable batch and job identity. Code Hangar cannot target a stop command yet, and no second cleanup batch can start."
                  : stopPending
                  ? "The stop is latched. No new topology group will start; if one is already in exact-handle disposition, Code Hangar finishes that inseparable group, then preserves every remaining held object and all recovery archives."
                  : executionUnknown
                  ? "The last known batch state is interrupted or unknown. Another cleanup batch stays blocked until this job reaches a terminal journal state."
                  : deletionStarted
                    ? "Already deleted held copies will not return to the holding area. Stopping finishes only the current object or inseparable topology group, then keeps every not-yet-deleted object and all recovery archives."
                    : "In the recorded pre-delete phases, cancelling archive preparation deletes zero held objects. If the journal advances before the stop request is accepted, Code Hangar will report any partial disposition instead of claiming a zero-delete cancellation. Recovery archives stay."}
              </p>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}

function finalRemovePhaseLabel(phase: FinalRemoveBatchProgress["phase"]): string {
  switch (phase) {
    case "waitingForUac": return "Waiting for the single Windows approval";
    case "verifyingArchives": return "Verifying object archives";
    case "roundtrip": return "Checking a synthetic restore round-trip";
    case "parentDisposition": return "Applying verified parent dispositions";
    case "deleting": return "Deleting eligible held objects";
    case "cleaningDirs": return "Cleaning eligible empty directories";
    case "stoppingAfterCurrentTopologyGroup": return "Stop requested — finishing a safe object/group boundary";
    case "finished": return "Final cleanup finished";
    case "interrupted": return "Final cleanup was interrupted";
  }
}

export { VolumeImpactTable };
