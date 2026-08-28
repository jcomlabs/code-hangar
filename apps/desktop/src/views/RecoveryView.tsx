import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Archive, ArchiveRestore, CheckCircle2, ChevronDown, ChevronUp, HardDrive, History, LockKeyhole, RefreshCcw, Shield, Trash2 } from "lucide-react";
import { ConceptHelp } from "../BeginnerHelp";
import type {
  FinalRemoveBatchPreview,
  FinalRemoveBatchProgress,
  FinalRemoveBatchResult,
  FinalRemoveProjectPreview,
  FinalRemoveScope,
  FinalRemoveVolumeImpact,
  MutationActivityItem,
  MutationActivityLog,
  MutationActivityOperation,
  MutationStoredEntry,
  PersistedAppRemoval,
  ProjectSummary
} from "../types";
import { displayLocalPath, formatBytes, formatOptionalBytes, formatTimestamp } from "../ui";
import { PreviousVersions } from "./project-center/PreviousVersions";

const RECOVERY_PREVIEW_LIMIT = 3;
const TECHNICAL_ACTIVITY_LIMIT = 30;
export const RECOVERY_REFRESH_CONFIRM_MS = 2_000;

export function RecoveryView({
  mutationAvailable,
  mutationMessage,
  mutationActivity,
  mutationBusy,
  finalRemoveExecutionUnknown,
  finalRemoveProgress,
  finalRemoveJobId,
  finalRemoveBatchId,
  finalRemovePreview,
  finalRemovePreviewLoading,
  finalRemoveUnavailableReason,
  finalRemoveResult,
  finalRemoveEnabled,
  finalRemoveCapabilityLoading,
  advancedMode,
  projects,
  appRemovals,
  restoreAppRemoval,
  refreshMutationActivity,
  runMutationRestore,
  runMutationRestoreElsewhere,
  onReviewFinalRemove,
  onSetFinalRemoveEnabled,
  onStopFinalRemoveBatch,
  onDiscoverProjects,
  onOpenScanFolders,
  currentFile,
  onFileHistoryMutated,
  setStatusText
}: {
  mutationAvailable: boolean;
  mutationMessage: string | null;
  mutationActivity: MutationActivityLog | null;
  mutationBusy: boolean;
  finalRemoveExecutionUnknown: boolean;
  finalRemoveProgress: FinalRemoveBatchProgress | null;
  finalRemoveJobId: string | null;
  finalRemoveBatchId: string | null;
  finalRemovePreview: FinalRemoveBatchPreview | null;
  finalRemovePreviewLoading: boolean;
  finalRemoveUnavailableReason: string | null;
  finalRemoveResult: FinalRemoveBatchResult | null;
  finalRemoveEnabled: boolean;
  finalRemoveCapabilityLoading: boolean;
  advancedMode: boolean;
  projects: ProjectSummary[];
  appRemovals: PersistedAppRemoval[];
  restoreAppRemoval: (id: string, projectName: string) => Promise<void>;
  refreshMutationActivity: () => Promise<boolean>;
  runMutationRestore: (entryId: number) => void;
  runMutationRestoreElsewhere: (entryId: number) => void;
  onReviewFinalRemove: (scope: FinalRemoveScope) => void;
  onSetFinalRemoveEnabled: (enabled: boolean, acknowledgement?: string | null) => Promise<boolean>;
  onStopFinalRemoveBatch: () => void | Promise<void>;
  onDiscoverProjects: () => void;
  onOpenScanFolders: () => void;
  currentFile?: { nodeId: number; displayName: string } | null;
  onFileHistoryMutated: (nodeId: number) => void | Promise<void>;
  setStatusText: (value: string) => void;
}) {
  const [showFullHeldList, setShowFullHeldList] = useState(false);
  const [showAllOperations, setShowAllOperations] = useState(false);
  const [showAllBackups, setShowAllBackups] = useState(false);
  const [showAllAppRemovals, setShowAllAppRemovals] = useState(false);
  const [showTechnicalActivity, setShowTechnicalActivity] = useState(false);
  const [restoringAppRemovalId, setRestoringAppRemovalId] = useState<string | null>(null);
  const [refreshState, setRefreshState] = useState<"idle" | "loading" | "done">("idle");
  const [showFinalRemoveActivation, setShowFinalRemoveActivation] = useState(false);
  const [finalRemoveAcknowledgement, setFinalRemoveAcknowledgement] = useState("");
  useEffect(() => {
    if (refreshState !== "done") return;
    const timer = window.setTimeout(() => setRefreshState("idle"), RECOVERY_REFRESH_CONFIRM_MS);
    return () => window.clearTimeout(timer);
  }, [refreshState]);
  const heldGroups = useMemo(
    () => groupStoredEntriesByProject(mutationActivity?.storedEntries ?? [], projects),
    [mutationActivity?.storedEntries, projects]
  );
  const summary = useMemo(
    () => mutationActivity ? summarizeRecovery(mutationActivity, appRemovals.length) : null,
    [appRemovals.length, mutationActivity]
  );
  const finalCleanup = useMemo(
    () => finalRemovePreview ? summarizeFinalRemovePreview(finalRemovePreview) : null,
    [finalRemovePreview]
  );
  const finalCleanupEligibleVolumeIds = useMemo(() => {
    if (!finalRemovePreview) return new Set<string>();
    const heldByTopology = new Map<string, FinalRemoveBatchPreview["objects"]>();
    for (const object of finalRemovePreview.objects) {
      if (object.lifecycle !== "held") continue;
      const members = heldByTopology.get(object.topologyGroupId) ?? [];
      members.push(object);
      heldByTopology.set(object.topologyGroupId, members);
    }
    const safeTopologyIds = new Set(finalRemovePreview.eligibleTopologyGroupIds.filter((topologyGroupId) => {
      const members = heldByTopology.get(topologyGroupId) ?? [];
      return members.length > 0 && members.every((object) => (
        object.eligibility === "ready" || object.eligibility === "needsArchiveV2"
      ));
    }));
    return new Set(finalRemovePreview.objects
      .filter((object) => object.lifecycle === "held" && safeTopologyIds.has(object.topologyGroupId))
      .map((object) => object.heldVolumeId));
  }, [finalRemovePreview]);
  const orderedStoredEntries = useMemo(
    () => orderRecoveryStoredEntries(mutationActivity?.storedEntries ?? []),
    [mutationActivity?.storedEntries]
  );
  const visibleOperations = recoveryPreviewItems(mutationActivity?.operations ?? [], showAllOperations);
  const visibleBackups = recoveryPreviewItems(mutationActivity?.backups ?? [], showAllBackups);
  const visibleAppRemovals = recoveryPreviewItems(appRemovals, showAllAppRemovals);
  const hasRecoveryRecords = recoveryHasRecords(mutationActivity, appRemovals.length);
  const emptyState = recoveryEmptyState(mutationAvailable);
  const finalRemovePreviewExpired = Boolean(finalRemovePreview && Date.parse(finalRemovePreview.expiresAt) <= Date.now());
  const finalCleanupSelectedObjects = finalCleanup ? finalCleanup.ready + finalCleanup.needsArchiveV2 : 0;
  const capacityBlockedObjects = finalRemovePreview?.objects.filter((object) => (
    object.lifecycle === "held" && object.eligibility === "blocked" && object.reasonCode === "capacityBlocked"
  )).length ?? 0;
  const finalCleanupCapacityExceeded = Boolean(finalRemovePreview && (
    !Number.isSafeInteger(finalRemovePreview.maxDeleteObjects)
    || finalRemovePreview.maxDeleteObjects < 0
    || finalCleanupSelectedObjects > finalRemovePreview.maxDeleteObjects
    || capacityBlockedObjects > 0
  ));
  const persistedCleanupCanStop = Boolean(finalRemoveExecutionUnknown && finalRemoveJobId && finalRemoveBatchId);
  const persistedCleanupDeletionStarted = finalRemoveProgress?.phase === "parentDisposition"
    || finalRemoveProgress?.phase === "deleting"
    || finalRemoveProgress?.phase === "cleaningDirs"
    || finalRemoveProgress?.phase === "finished";
  const persistedCleanupStopPending = finalRemoveProgress?.phase === "stoppingAfterCurrentTopologyGroup";
  const heldEntryCount = summary?.heldFiles ?? 0;
  const hasFinalCleanupRecords = Boolean(finalRemovePreview?.projects.length);
  const showFinalCleanup = (finalRemoveEnabled || finalRemoveExecutionUnknown || Boolean(finalRemoveResult)) && shouldShowFinalCleanup(
    heldEntryCount,
    finalRemovePreview,
    finalRemovePreviewLoading,
    finalRemoveUnavailableReason,
    finalRemoveExecutionUnknown || Boolean(finalRemoveResult),
    mutationAvailable
  );
  const nothingToRecover = Boolean(
    mutationActivity
    && !hasRecoveryRecords
    && !hasFinalCleanupRecords
    && !finalRemoveExecutionUnknown
    && !finalRemoveResult
    && !showFinalCleanup
  );
  const refreshHistory = async () => {
    setRefreshState("loading");
    try {
      const refreshed = await refreshMutationActivity();
      setRefreshState(refreshed ? "done" : "idle");
    } catch {
      setRefreshState("idle");
    }
  };
  const restoreAppListing = async (removal: PersistedAppRemoval) => {
    setRestoringAppRemovalId(removal.id);
    try {
      await restoreAppRemoval(removal.id, removal.projectName);
    } finally {
      setRestoringAppRemovalId(null);
    }
  };
  const updateFinalRemoveCapability = async (enabled: boolean) => {
    const changed = await onSetFinalRemoveEnabled(
      enabled,
      enabled ? finalRemoveAcknowledgement : null
    );
    if (!changed) return;
    setFinalRemoveAcknowledgement("");
    setShowFinalRemoveActivation(false);
  };

  return (
    <section className="pane-section compact recovery-view">
      {summary && hasRecoveryRecords ? (
        <div className="preview-banner" data-help="This area explains verified backups, recoverable held files and the local history needed to recover interrupted disk actions.">
          {recoveryStatusBanner(summary)}
        </div>
      ) : null}
      {hasRecoveryRecords || mutationAvailable ? (
        <div className="button-row activity-toolbar">
          <button type="button" className="secondary-button activity-refresh-button" disabled={refreshState === "loading" || (mutationBusy && !finalRemoveExecutionUnknown)} data-help="Reload local recovery history, verified backups and stored entries. History remains available while an uncertain batch is being reconciled." onClick={() => void refreshHistory()}>
            <RefreshCcw size={14} className={refreshState === "loading" ? "spin" : ""} />
            <span>{refreshState === "loading" ? "Checking history…" : "Refresh history"}</span>
          </button>
          {refreshState === "done" ? (
            <span className="activity-refresh-status" role="status" aria-live="polite"><CheckCircle2 size={14} /> History is current</span>
          ) : null}
        </div>
      ) : null}
      {mutationMessage ? <p className="mutation-message" role="status" aria-live="polite" data-help="Latest recovery or safe-action status message.">{mutationMessage}</p> : null}
      {currentFile ? (
        <section className="recovery-file-history" aria-label={`Previous versions of ${currentFile.displayName}`}>
          <div>
            <span>CURRENT FILE</span>
            <strong>{currentFile.displayName}</strong>
            <small>Verified versions created by Code Hangar edits. Restoring one first saves the version it replaces.</small>
          </div>
          <PreviousVersions
            nodeId={currentFile.nodeId}
            onFileMutated={onFileHistoryMutated}
            setStatusText={setStatusText}
          />
        </section>
      ) : null}
      {mutationAvailable ? (
        <section className={`activity-card final-remove-capability ${finalRemoveEnabled ? "enabled" : "disabled"}`} aria-labelledby="final-remove-capability-heading">
          <div className="activity-card-heading">
            <div>
              <span className="final-cleanup-eyebrow"><LockKeyhole size={14} aria-hidden="true" /> PERMANENT REMOVAL</span>
              <h2 id="final-remove-capability-heading">{finalRemoveEnabled ? "Enabled for explicit final-cleanup reviews" : "Off by default"}</h2>
              <p className="muted help-copy">
                {finalRemoveEnabled
                  ? "This only makes immutable final-cleanup previews available. Every operation still needs an eligible held object, a valid verified backup, a fresh Risk Report and a new exact confirmation."
                  : "Held projects remain restorable. Enable this only when you deliberately want the later, irreversible stage after backup and holding; analysis and Safe Manage never enable it for you."}
              </p>
            </div>
            {finalRemoveEnabled ? (
              <button
                type="button"
                className="secondary-button"
                disabled={finalRemoveCapabilityLoading || mutationBusy || finalRemoveExecutionUnknown}
                onClick={() => void updateFinalRemoveCapability(false)}
              >
                {finalRemoveCapabilityLoading ? "Saving…" : "Turn off"}
              </button>
            ) : (
              <button
                type="button"
                className="danger-outline"
                disabled={finalRemoveCapabilityLoading || mutationBusy || finalRemoveExecutionUnknown}
                aria-expanded={showFinalRemoveActivation}
                onClick={() => setShowFinalRemoveActivation((current) => !current)}
              >
                Enable permanent removal…
              </button>
            )}
          </div>
          {!finalRemoveEnabled && showFinalRemoveActivation ? (
            <div className="final-remove-activation" role="group" aria-label="Enable permanent removal">
              <AlertTriangle size={18} aria-hidden="true" />
              <div>
                <strong>This does not delete anything now, but it unlocks an irreversible workflow.</strong>
                <p>Type <code>ENABLE PERMANENT REMOVAL</code> exactly. The capability can be turned off again, and no batch can run without its own current preview and confirmation.</p>
                <label className="change-access-name">
                  Activation phrase
                  <input
                    value={finalRemoveAcknowledgement}
                    onChange={(event) => setFinalRemoveAcknowledgement(event.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
                <div className="button-row compact">
                  <button
                    type="button"
                    className="danger-button"
                    disabled={finalRemoveCapabilityLoading || finalRemoveAcknowledgement !== "ENABLE PERMANENT REMOVAL"}
                    onClick={() => void updateFinalRemoveCapability(true)}
                  >
                    {finalRemoveCapabilityLoading ? "Enabling…" : "Enable capability"}
                  </button>
                  <button type="button" className="secondary-button" disabled={finalRemoveCapabilityLoading} onClick={() => {
                    setFinalRemoveAcknowledgement("");
                    setShowFinalRemoveActivation(false);
                  }}>
                    Cancel
                  </button>
                </div>
              </div>
            </div>
          ) : null}
        </section>
      ) : null}
      {showFinalCleanup ? (
        <section className="activity-card final-cleanup-card" aria-labelledby="final-cleanup-heading" data-help="Review eligible held objects by stable project group. Unsupported subtrees stay held; recovery archives are kept.">
          <div className="activity-card-heading final-cleanup-heading">
            <div>
              <span className="final-cleanup-eyebrow"><Trash2 size={14} aria-hidden="true" /> FINAL CLEANUP</span>
              <h2 id="final-cleanup-heading">Finish removing held projects</h2>
              <p className="muted help-copy">Delete only held copies proved eligible by an object-archive-v2 preview. Blocked subtrees and all recovery archives remain.</p>
            </div>
            {finalRemovePreview && finalRemovePreview.objects.some((object) => object.lifecycle === "held") ? (
              <button
                type="button"
                className="danger-outline final-cleanup-batch-button"
                disabled={mutationBusy || finalRemovePreviewLoading || finalRemovePreviewExpired}
                aria-label={`Review final cleanup for all ${finalRemovePreview.projects.length} listed projects${finalCleanupCapacityExceeded ? "; selection exceeds the current batch capacity" : ""}`}
                onClick={() => onReviewFinalRemove({ kind: "allEligible" })}
              >
                Review batch cleanup…
              </button>
            ) : null}
          </div>

          {finalRemovePreviewLoading ? (
            <p className="final-cleanup-capability" role="status" aria-live="polite"><RefreshCcw size={15} className="spin" /> Checking object-archive eligibility and volume allocation…</p>
          ) : null}

          {finalRemoveExecutionUnknown || (!finalRemovePreviewLoading && finalRemoveUnavailableReason) ? (
            <div
              className="final-cleanup-unavailable"
              role={finalRemoveExecutionUnknown ? "region" : "status"}
              aria-label={finalRemoveExecutionUnknown ? "Final cleanup journal reconciliation" : undefined}
            >
              <AlertTriangle size={18} aria-hidden="true" />
              <div>
                <strong>{finalRemoveExecutionUnknown ? "Final cleanup is awaiting journal reconciliation" : "Final cleanup is not available in this backend"}</strong>
                <p>{finalRemoveUnavailableReason ?? "The last final-cleanup request did not reach a proven terminal journal state. Reconcile this exact batch before starting another one."}</p>
                <p>Held copies remain restorable. Code Hangar will not fall back to the older single-file delete path.</p>
                {finalRemoveExecutionUnknown ? (
                  <>
                    <p>
                      Persisted batch: {finalRemoveBatchId ?? "identity unavailable"} · job: {finalRemoveJobId ?? "identity unavailable"}
                      {finalRemoveProgress ? ` · ${recoveryFinalRemovePhaseLabel(finalRemoveProgress.phase)} (${finalRemoveProgress.completed} of ${finalRemoveProgress.total})` : ""}
                    </p>
                    {persistedCleanupCanStop ? (
                      <button type="button" className="secondary-button danger-outline" disabled={persistedCleanupStopPending} onClick={() => void onStopFinalRemoveBatch()}>
                        {persistedCleanupStopPending
                          ? "Stop requested — finishing current object/group…"
                          : !finalRemoveProgress || finalRemoveProgress.phase === "interrupted" || finalRemoveProgress.phase === "finished"
                          ? "Reconcile and stop persisted batch"
                          : persistedCleanupDeletionStarted
                            ? "Stop after current object/group"
                            : "Cancel archive preparation"}
                      </button>
                    ) : (
                      <p>The dashboard did not provide both immutable batch and job identities, so Code Hangar will not send a stop command to an ambiguous target. Refresh history after backend recovery.</p>
                    )}
                  </>
                ) : null}
              </div>
            </div>
          ) : null}

          {finalRemovePreview && finalCleanup ? (
            <>
              <div className="final-cleanup-summary" aria-label="Final cleanup eligibility overview">
                <div className="ready">
                  <Shield size={17} aria-hidden="true" />
                  <span>READY</span>
                  <strong>{finalCleanup.ready}</strong>
                  <small>Archive proof complete</small>
                </div>
                <div className="needs">
                  <Archive size={17} aria-hidden="true" />
                  <span>NEEDS ARCHIVE V2</span>
                  <strong>{finalCleanup.needsArchiveV2}</strong>
                  <small>{finalCleanup.needsArchiveV2 > 0 ? "One Windows approval for the batch" : "No archive finalization needed"}</small>
                </div>
                <div className="blocked">
                  <AlertTriangle size={17} aria-hidden="true" />
                  <span>BLOCKED</span>
                  <strong>{finalCleanup.blocked}</strong>
                  <small>{finalCleanup.blockedSubtrees} subtree{finalCleanup.blockedSubtrees === 1 ? "" : "s"} stay held</small>
                </div>
              </div>

              {finalCleanupCapacityExceeded ? (
                <div className="final-cleanup-capacity" role="alert">
                  <AlertTriangle size={18} aria-hidden="true" />
                  <div>
                    <strong>The full selection exceeds this preview's verified batch capacity</strong>
                    <p>
                      {capacityBlockedObjects > 0
                        ? `${capacityBlockedObjects} held object${capacityBlockedObjects === 1 ? " is" : "s are"} explicitly capacity-blocked by this preview. `
                        : `${finalCleanupSelectedObjects} eligible held object${finalCleanupSelectedObjects === 1 ? " is" : "s are"} selected. `}
                      {Number.isSafeInteger(finalRemovePreview.maxDeleteObjects) && finalRemovePreview.maxDeleteObjects >= 0
                        ? `At most ${finalRemovePreview.maxDeleteObjects} can be confirmed in one batch.`
                        : "The backend did not provide a valid maximum object count."}
                    </p>
                    <p>Open the review to see the block, then use smaller project reviews where possible. Objects above the limit remain held.</p>
                  </div>
                </div>
              ) : null}

              <div className="final-cleanup-volume-list" aria-label="Projected release by volume">
                {finalRemovePreview.volumes.map((volume) => (
                  <div className="final-cleanup-volume" key={volume.volumeId}>
                    <HardDrive size={16} aria-hidden="true" />
                    <div>
                      <strong>{volume.label}</strong>
                      <span>{formatBytes(finalCleanupEligibleVolumeIds.has(volume.volumeId) ? volume.projectedReleaseBytes : 0)} projected release{finalCleanupEligibleVolumeIds.has(volume.volumeId) ? "" : " — no eligible deletion on this volume in the current preview"} · {formatBytes(volume.archiveRetainedAllocatedBytes)} archive allocation kept</span>
                      <small>{finalRemoveVolumeQualityLabel(volume.quality)}</small>
                    </div>
                  </div>
                ))}
              </div>

              <div className="final-cleanup-project-list" aria-label="Projects available for final cleanup">
                {finalRemovePreview.projects.map((project) => {
                  const eligible = project.ready + project.needsArchiveV2;
                  return (
                    <div className="final-cleanup-project" key={project.groupId}>
                      <div>
                        <strong>{project.projectName}</strong>
                        <span>{eligible} eligible · {project.blocked} blocked · {project.totalObjects} total</span>
                        <small title={displayLocalPath(project.originalRoot)}>{displayLocalPath(project.originalRoot)}</small>
                      </div>
                      <button
                        type="button"
                        className="secondary-button"
                        disabled={!finalRemoveProjectActionEnabled(project, mutationAvailable, mutationBusy, finalRemovePreviewExpired)}
                        aria-label={`Review final cleanup for ${project.projectName}; ${eligible} eligible and ${project.blocked} blocked objects`}
                        onClick={() => onReviewFinalRemove({ kind: "project", groupId: project.groupId })}
                      >
                        {eligible > 0 ? "Review cleanup…" : project.blocked > 0 ? "Review why blocked…" : "Nothing held"}
                      </button>
                    </div>
                  );
                })}
              </div>

              <p className={`final-cleanup-expiry ${finalRemovePreviewExpired ? "is-expired" : ""}`} role={finalRemovePreviewExpired ? "status" : undefined}>
                <ArchiveRestore size={14} aria-hidden="true" />
                {finalRemovePreviewExpired
                  ? "This preview expired. Refresh history before opening final cleanup."
                  : <>Preview expires {formatRecoveryTimestamp(finalRemovePreview.expiresAt)}. A changed identity requires a new review.</>}
              </p>
            </>
          ) : null}

          {finalRemoveResult ? (
            <div className={`final-cleanup-last-result ${finalRemoveResult.status}`} role="status" aria-live="polite">
              {finalRemoveResult.status === "completed" ? <CheckCircle2 size={18} aria-hidden="true" /> : <AlertTriangle size={18} aria-hidden="true" />}
              <div>
                <strong>Latest batch: {finalRemoveResult.status}</strong>
                <span>{finalRemoveResultMessage(finalRemoveResult)}</span>
              </div>
            </div>
          ) : null}
        </section>
      ) : null}
      {nothingToRecover ? (
        <div className="recovery-empty-panel" data-help="There are no local recovery records, held files, verified backups or AI-app removal backups in this profile.">
          <strong>{emptyState.title}</strong>
          <p>{emptyState.detail}</p>
          <div className="recovery-empty-actions" aria-label="Safe next steps">
            <button type="button" className="secondary-button" onClick={onDiscoverProjects} data-help="Open passive project discovery. It searches local folders for candidates without changing files.">
              Find projects
            </button>
            <button type="button" className="secondary-button" onClick={onOpenScanFolders} data-help="Open Scan Folders to review roots, missing folders and rescan options.">
              Review scan folders
            </button>
          </div>
        </div>
      ) : null}
      {summary && hasRecoveryRecords ? (
        <div className="recovery-summary-grid" aria-label="Recovery overview">
          <div className={`recovery-summary-item ${summary.restorableNow > 0 ? "attention" : "resolved"}`}>
            <ArchiveRestore size={18} aria-hidden="true" />
            <div>
              <span>RESTORABLE NOW</span>
              <strong>{summary.restorableNow}</strong>
              <small>{recoveryRestorableSummaryDetail(summary)}</small>
            </div>
          </div>
          <div className="recovery-summary-item resolved">
            <CheckCircle2 size={18} aria-hidden="true" />
            <div>
              <span>STORED RECORDS</span>
              <strong>{summary.storedRecords}</strong>
              <small>{summary.storedRecords === 0 ? "No file history" : `${summary.resolvedStoredRecords} resolved`}</small>
            </div>
          </div>
          <div className="recovery-summary-item">
            <Shield size={18} aria-hidden="true" />
            <div>
              <span>RECOVERY BACKUPS</span>
              <strong>{summary.verifiedBackups}</strong>
              <small>{summary.totalBackups === 0 ? "No backup records" : `${summary.verifiedBackups} content record${summary.verifiedBackups === 1 ? "" : "s"} verified; object-v2 proof is separate`}</small>
            </div>
          </div>
          <div className={`recovery-summary-item ${summary.failedActions > 0 ? "attention" : ""}`}>
            <History size={18} aria-hidden="true" />
            <div>
              <span>DISK ACTIONS</span>
              <strong>{summary.diskActions}</strong>
              <small>{summary.failedActions > 0 ? `${summary.failedActions} failed` : summary.diskActions > 0 ? "Recorded locally" : "No disk history"}</small>
            </div>
          </div>
        </div>
      ) : null}
      {hasRecoveryRecords ? (
        <div className="activity-stack">
          {appRemovals.length > 0 ? (
            <section className="activity-card" data-help="Projects you removed from their AI apps. Each app registration was backed up before removal, so Restore can put it back.">
              <h3>AI app listings ready to restore</h3>
              <p className="muted help-copy">Restore brings a project listing back from its verified local backup. Reopen that AI app afterwards to see it.</p>
              {visibleAppRemovals.map((removal) => (
                <div className="activity-row" key={removal.id}>
                  <div>
                    <strong>{removal.projectName}</strong>
                    <span>{removal.records.map((record) => record.app).join(", ") || "AI app"}</span>
                    <small>Removed {formatRemovedAt(removal.removedAtUnix)}</small>
                  </div>
                  <div className="activity-actions">
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!mutationAvailable || mutationBusy || restoringAppRemovalId !== null}
                      data-help="Restore this project's AI-app registration from its verified backup. Reopen the app afterwards to see it listed again."
                      onClick={() => void restoreAppListing(removal)}
                    >
                      {restoringAppRemovalId === removal.id ? "Restoring…" : "Restore"}
                    </button>
                  </div>
                </div>
              ))}
              {appRemovals.length > RECOVERY_PREVIEW_LIMIT ? (
                <button type="button" className="secondary-button recovery-disclosure-button" aria-expanded={showAllAppRemovals} onClick={() => setShowAllAppRemovals((value) => !value)}>
                  {showAllAppRemovals ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                  <span>{showAllAppRemovals ? "Show fewer app listings" : `Show all ${appRemovals.length} app listings`}</span>
                </button>
              ) : null}
            </section>
          ) : null}

          {mutationActivity?.storedEntries.length ? (
          <section className="activity-card" data-help="Stored-file records distinguish copies currently held in recovery from completed restores and cleanup history. Restore refuses to overwrite an occupied destination.">
            <div className="heading-with-help">
              <h3>Held copies and restore history</h3>
              <ConceptHelp concept="backup" />
            </div>
            <p className="muted help-copy">
              {summary && summary.heldFiles > 0
                ? `${summary.heldFiles} file${summary.heldFiles === 1 ? " is" : "s are"} currently held and ready for review.`
                : `Nothing is currently held. ${summary?.resolvedStoredRecords ?? 0} completed record${summary?.resolvedStoredRecords === 1 ? " remains" : "s remain"} as local history.`}
            </p>
            <div className="held-group-list">
              {heldGroups.map((group) => (
                <div className="activity-row compact-row" key={group.key} data-help={`Stored-file records for ${group.label}. ${group.count} records and ${group.quarantined} currently held. Current reclaimable allocation is shown only in the object-archive preview above.`}>
                  <div>
                    <strong>{group.label}</strong>
                    <span>{storedGroupStatusLabel(group.count, group.quarantined)}</span>
                    <small>{displayLocalPath(group.samplePath)}</small>
                  </div>
                  <small>{group.quarantined > 0 ? "Review cleanup above" : "History"}</small>
                </div>
              ))}
            </div>
            <button
              type="button"
              className="secondary-button recovery-disclosure-button"
              aria-expanded={showFullHeldList}
              data-help="Review individual stored-file records. Files still held show restore controls; completed records remain concise history."
              onClick={() => setShowFullHeldList((value) => !value)}
            >
              {showFullHeldList ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
              <span>{storedEntryDisclosureLabel(mutationActivity.storedEntries.length, summary?.heldFiles ?? 0, showFullHeldList)}</span>
            </button>
            {showFullHeldList ? orderedStoredEntries.map((entry) => {
              const isHeld = entry.status === "quarantined";
              return (
                <div className={`activity-row stored-entry-row ${isHeld ? "is-held" : "is-resolved"}`} key={entry.id}>
                  <div>
                    <strong title={displayLocalPath(entry.originalPath)}>{displayLocalPath(entry.originalPath)}</strong>
                    <span>{advancedMode ? `${recoveryStoredEntryStatusLabel(entry.status)} · entry #${entry.id}` : recoveryStoredEntryStatusLabel(entry.status)}</span>
                    <small>{displayLocalPath(entry.storedPath)}</small>
                  </div>
                  <div className="activity-actions">
                    <small>{formatOptionalBytes(entry.size)} logical size{isHeld ? " · current allocation is in the cleanup preview" : ""}</small>
                    {isHeld ? (
                      <>
                        <button type="button" className="secondary-button" aria-label={`Restore ${displayLocalPath(entry.originalPath)} to its original path`} disabled={!mutationAvailable || mutationBusy} data-help="Restore this stored entry to its original path if that path is free. This changes files on disk." onClick={() => void runMutationRestore(entry.id)}>Restore</button>
                        <button type="button" className="secondary-button" aria-label={`Restore ${displayLocalPath(entry.originalPath)} to another folder`} disabled={!mutationAvailable || mutationBusy} data-help="Choose a different destination folder. Code Hangar preserves the stored relative path and refuses to overwrite an existing file." onClick={() => void runMutationRestoreElsewhere(entry.id)}>Restore elsewhere...</button>
                      </>
                    ) : null}
                  </div>
                </div>
              );
            }) : null}
          </section>
          ) : null}

          {mutationActivity?.operations.length ? (
          <section className="activity-card" data-help="This is the local recovery history for moves to recovery, restores and final removals. It is written only by editions that can perform local disk actions.">
            <h3>Recent disk actions</h3>
            <p className="muted help-copy">{mutationActivity.message}</p>
            {visibleOperations.map((operation) => (
              <div className="activity-row" key={operation.id}>
                <div>
                  <strong>{recoveryOperationKindLabel(operation.kind)}</strong>
                  <span>{advancedMode ? `${recoveryStatusLabel(operation.status)} · operation #${operation.id}` : recoveryStatusLabel(operation.status)}</span>
                  {operation.error ? <small className="scan-error">{operation.error}</small> : null}
                </div>
                <small>{recoveryOperationMeta(operation)}</small>
              </div>
            ))}
            {mutationActivity.operations.length > RECOVERY_PREVIEW_LIMIT ? (
              <button type="button" className="secondary-button recovery-disclosure-button" aria-expanded={showAllOperations} onClick={() => setShowAllOperations((value) => !value)}>
                {showAllOperations ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                <span>{showAllOperations ? "Show fewer actions" : `Show all ${mutationActivity.operations.length} actions`}</span>
              </button>
            ) : null}
          </section>
          ) : null}

          {mutationActivity?.backups.length ? (
          <section className="activity-card" data-help="Backup records are local copies and manifests chosen by the user. They do not change project source files.">
            <div className="heading-with-help">
              <h3>Backup records</h3>
              <ConceptHelp concept="backup" />
            </div>
            <p className="muted help-copy">{summary?.verifiedBackups ?? 0} of {mutationActivity.backups.length} recorded backup{mutationActivity.backups.length === 1 ? " is" : "s are"} verified.</p>
            {visibleBackups.map((backup) => (
              <div className="activity-row" key={backup.id}>
                <div>
                  <strong>{displayLocalPath(backup.destination)}</strong>
                  <span>{advancedMode ? `${backup.level} · backup #${backup.id} · ${backup.verified ? "verified" : "not verified"}` : `${backup.level} · ${backup.verified ? "verified" : "not verified"}`}</span>
                  <small>{displayLocalPath(backup.manifestPath)}</small>
                </div>
                <small>{formatOptionalBytes(backup.totalBytes)} · {formatRecoveryTimestamp(backup.createdAt)}</small>
              </div>
            ))}
            {mutationActivity.backups.length > RECOVERY_PREVIEW_LIMIT ? (
              <button type="button" className="secondary-button recovery-disclosure-button" aria-expanded={showAllBackups} onClick={() => setShowAllBackups((value) => !value)}>
                {showAllBackups ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                <span>{showAllBackups ? "Show fewer backups" : `Show all ${mutationActivity.backups.length} backups`}</span>
              </button>
            ) : null}
          </section>
          ) : null}

          {advancedMode && mutationActivity?.items.length ? (
          <section className="activity-card" data-help="The technical record shows concrete item-level paths touched by journaled operations. It stays collapsed until requested.">
            <div className="activity-card-heading">
              <div>
                <h3>Technical activity record</h3>
                <p className="muted help-copy">Item-level paths for diagnosis. Main recovery decisions do not require this list.</p>
              </div>
              <button type="button" className="secondary-button recovery-disclosure-button compact" aria-expanded={showTechnicalActivity} onClick={() => setShowTechnicalActivity((value) => !value)}>
                {showTechnicalActivity ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                <span>{showTechnicalActivity ? "Hide technical record" : technicalActivityDisclosureLabel(mutationActivity.items.length)}</span>
              </button>
            </div>
            {showTechnicalActivity && mutationActivity.items.length > TECHNICAL_ACTIVITY_LIMIT ? (
              <p className="muted help-copy">Showing the latest {TECHNICAL_ACTIVITY_LIMIT} of {mutationActivity.items.length} item-level records.</p>
            ) : null}
            {showTechnicalActivity ? mutationActivity.items.slice(0, TECHNICAL_ACTIVITY_LIMIT).map((item) => (
              <div className="activity-row compact-row" key={item.id}>
                <div>
                  <strong>{humanizeIdentifier(item.action)}</strong>
                  <span>{recoveryStatusLabel(item.status)} · operation #{item.operationId}</span>
                  <small className="activity-technical-path" title={technicalActivityPathLabel(item)}>{technicalActivityPathLabel(item)}</small>
                </div>
                <small>{formatOptionalBytes(item.bytes)}</small>
              </div>
            )) : null}
          </section>
          ) : null}
        </div>
      ) : null}
      {!mutationActivity && appRemovals.length === 0 ? (
        <p className="muted result-empty">Activity has not been loaded yet.</p>
      ) : null}
    </section>
  );
}

export function recoveryEmptyState(mutationAvailable: boolean) {
  if (mutationAvailable) {
    return {
      title: "Nothing to recover",
      detail: "No held files, verified backups or recovery journal entries are recorded for this profile. Keep mapping projects first, then return here only after a confirmed safe action creates recovery history."
    };
  }
  return {
    title: "No recovery history in this build",
    detail: "This installation is read-only, so it never creates held files, backup manifests or disk-action journals. Keep mapping projects in Discover or review scan folders; files stay untouched."
  };
}

export function formatRecoveryTimestamp(value?: string | null) {
  if (!value) return "Earlier";
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? value : formatTimestamp(timestamp);
}

export interface FinalRemovePreviewSummary {
  ready: number;
  needsArchiveV2: number;
  blocked: number;
  blockedSubtrees: number;
}

export function summarizeFinalRemovePreview(preview: FinalRemoveBatchPreview): FinalRemovePreviewSummary {
  const eligibleTopologyIds = new Set(preview.eligibleTopologyGroupIds);
  const heldMemberCounts = new Map<string, { total: number; blocked: number }>();
  for (const object of preview.objects) {
    if (object.lifecycle !== "held") continue;
    const counts = heldMemberCounts.get(object.topologyGroupId) ?? { total: 0, blocked: 0 };
    counts.total += 1;
    if (object.eligibility !== "ready" && object.eligibility !== "needsArchiveV2") counts.blocked += 1;
    heldMemberCounts.set(object.topologyGroupId, counts);
  }
  const whollyEligibleTopologyIds = new Set(Array.from(eligibleTopologyIds).filter((topologyGroupId) => {
    const counts = heldMemberCounts.get(topologyGroupId);
    return Boolean(counts && counts.total > 0 && counts.blocked === 0);
  }));
  const selectedObjects = preview.objects.filter((object) => (
    object.lifecycle === "held"
    && (object.eligibility === "ready" || object.eligibility === "needsArchiveV2")
    && whollyEligibleTopologyIds.has(object.topologyGroupId)
  ));
  const heldObjects = preview.objects.filter((object) => object.lifecycle === "held");
  return {
    ready: selectedObjects.filter((object) => object.eligibility === "ready").length,
    needsArchiveV2: selectedObjects.filter((object) => object.eligibility === "needsArchiveV2").length,
    blocked: heldObjects.length - selectedObjects.length,
    blockedSubtrees: preview.projects.reduce((total, project) => total + project.blockedSubtrees.length, 0)
  };
}

export function finalRemoveProjectActionEnabled(
  project: FinalRemoveProjectPreview,
  mutationAvailable: boolean,
  mutationBusy: boolean,
  previewExpired = false
) {
  return mutationAvailable && !mutationBusy && !previewExpired && project.totalObjects > 0;
}

export function shouldShowFinalCleanup(
  heldEntryCount: number,
  preview: FinalRemoveBatchPreview | null,
  previewLoading: boolean,
  unavailableReason: string | null,
  forceVisible = false,
  mutationAvailable = false
): boolean {
  return forceVisible
    || heldEntryCount > 0
    || Boolean(preview?.projects.length)
    || previewLoading
    || Boolean(unavailableReason && (heldEntryCount > 0 || mutationAvailable));
}

export function finalRemoveVolumeQualityLabel(quality: FinalRemoveVolumeImpact["quality"]): string {
  switch (quality) {
    case "exactObjectAllocation": return "Exact object allocation from the preview";
    case "estimated": return "Estimate only — not an exact free-space promise";
    case "observedNoisy": return "Observed volume change — other disk activity may affect it";
  }
}

export function recoveryFinalRemovePhaseLabel(phase: FinalRemoveBatchProgress["phase"]): string {
  switch (phase) {
    case "waitingForUac": return "waiting for Windows approval";
    case "verifyingArchives": return "verifying object archives";
    case "roundtrip": return "verifying the restore round-trip";
    case "parentDisposition": return "applying parent dispositions";
    case "deleting": return "deleting eligible held objects";
    case "cleaningDirs": return "cleaning eligible empty directories";
    case "stoppingAfterCurrentTopologyGroup": return "stop requested; finishing the current safe object/group boundary";
    case "finished": return "backend reports finished; terminal result is being reconciled";
    case "interrupted": return "interrupted or unknown; journal reconciliation required";
  }
}

export function finalRemoveResultMessage(result: FinalRemoveBatchResult): string {
  const unresolved = result.keptObjects + result.failedObjects;
  if (result.status === "completed") {
    return `${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted; all recovery archives were kept.`;
  }
  if (result.status === "cancelled") {
    return `Cleanup stopped after ${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted. ${unresolved} remain selected or held; all recovery archives were kept.`;
  }
  return `${result.deletedObjects} held object${result.deletedObjects === 1 ? " was" : "s were"} deleted; ${unresolved} remain held or need review. All recovery archives were kept.`;
}

export interface RecoverySummary {
  heldFiles: number;
  appListings: number;
  restorableNow: number;
  storedRecords: number;
  resolvedStoredRecords: number;
  verifiedBackups: number;
  totalBackups: number;
  diskActions: number;
  failedActions: number;
}

export function summarizeRecovery(activity: MutationActivityLog, appListingCount: number): RecoverySummary {
  const heldFiles = activity.storedEntries.filter((entry) => entry.status === "quarantined").length;
  const resolvedStoredRecords = activity.storedEntries.filter((entry) => entry.status !== "quarantined").length;
  const verifiedBackups = activity.backups.filter((backup) => backup.verified).length;
  const failedActions = activity.operations.filter((operation) => operation.status === "failed").length;
  return {
    heldFiles,
    appListings: appListingCount,
    restorableNow: heldFiles + appListingCount,
    storedRecords: activity.storedEntries.length,
    resolvedStoredRecords,
    verifiedBackups,
    totalBackups: activity.backups.length,
    diskActions: activity.operations.length,
    failedActions
  };
}

export function recoveryHasRecords(activity: MutationActivityLog | null, appListingCount: number): boolean {
  return appListingCount > 0 || Boolean(
    activity && (
      activity.operations.length > 0 ||
      activity.storedEntries.length > 0 ||
      activity.backups.length > 0
    )
  );
}

export function recoveryStatusBanner(summary: RecoverySummary): string {
  if (summary.restorableNow === 0) {
    return "Nothing is waiting to be restored. The records below are local history.";
  }
  return `${summary.restorableNow} recovery item${summary.restorableNow === 1 ? " is" : "s are"} ready to review. Every restore remains explicit; file restores refuse to overwrite occupied destinations.`;
}

export function recoveryRestorableSummaryDetail(summary: RecoverySummary): string {
  const parts = [];
  if (summary.heldFiles > 0) {
    parts.push(`${summary.heldFiles} held file${summary.heldFiles === 1 ? "" : "s"}`);
  }
  if (summary.appListings > 0) {
    parts.push(`${summary.appListings} app listing${summary.appListings === 1 ? "" : "s"}`);
  }
  return parts.length > 0 ? parts.join(" + ") : "Nothing waiting";
}

export function recoveryPreviewItems<T>(items: readonly T[], expanded: boolean, limit = RECOVERY_PREVIEW_LIMIT): readonly T[] {
  return expanded ? items : items.slice(0, limit);
}

export function recoveryOperationMeta(operation: MutationActivityOperation): string {
  const parts = [];
  if (operation.recoveredBytes != null) {
    parts.push(formatBytes(operation.recoveredBytes));
  }
  parts.push(formatRecoveryTimestamp(operation.finishedAt ?? operation.startedAt ?? operation.createdAt));
  return parts.join(" · ");
}

export function orderRecoveryStoredEntries(entries: readonly MutationStoredEntry[]): MutationStoredEntry[] {
  return [...entries].sort((left, right) => {
    const leftHeld = left.status === "quarantined" ? 0 : 1;
    const rightHeld = right.status === "quarantined" ? 0 : 1;
    return leftHeld - rightHeld || right.id - left.id;
  });
}

export function storedEntryDisclosureLabel(total: number, held: number, expanded: boolean): string {
  if (expanded) return "Hide file records";
  if (held > 0) return `Review ${held} held file${held === 1 ? "" : "s"}`;
  return `Show ${total} completed record${total === 1 ? "" : "s"}`;
}

export function storedGroupStatusLabel(total: number, held: number): string {
  if (held > 0) {
    const completed = Math.max(0, total - held);
    return `${held} ready to restore · ${completed} completed`;
  }
  return `${total} completed record${total === 1 ? "" : "s"} · nothing held`;
}

export function technicalActivityDisclosureLabel(total: number): string {
  const visible = Math.min(total, TECHNICAL_ACTIVITY_LIMIT);
  return total > TECHNICAL_ACTIVITY_LIMIT
    ? `Show technical record (${visible} of ${total})`
    : `Show technical record (${visible})`;
}

export function technicalActivityPathLabel(item: MutationActivityItem): string {
  const from = item.fromPath ? displayLocalPath(item.fromPath) : null;
  const to = item.toPath ? displayLocalPath(item.toPath) : null;
  if (from && to) return `${from} -> ${to}`;
  return from ?? to ?? "No path recorded";
}

function groupStoredEntriesByProject(entries: MutationStoredEntry[], projects: ProjectSummary[]) {
  const groups = new Map<string, {
    key: string;
    label: string;
    count: number;
    quarantined: number;
    samplePath: string;
  }>();
  for (const entry of entries) {
    const project = findProjectForPath(entry.originalPath, projects);
    const key = project ? `project:${project.id}` : `operation:${entry.operationId ?? "unknown"}`;
    const label = project?.name ?? `Operation ${entry.operationId ?? "unknown"}`;
    const current = groups.get(key) ?? {
      key,
      label,
      count: 0,
      quarantined: 0,
      samplePath: entry.originalPath
    };
    current.count += 1;
    if (entry.status === "quarantined") current.quarantined += 1;
    groups.set(key, current);
  }
  return Array.from(groups.values()).sort((left, right) => (
    right.quarantined - left.quarantined || left.label.localeCompare(right.label)
  ));
}

function findProjectForPath(path: string, projects: ProjectSummary[]) {
  const normalizedPath = normalizeLocalPath(path);
  return projects
    .filter((project) => {
      const projectPath = normalizeLocalPath(project.path);
      const projectPrefix = projectPath.endsWith("\\") ? projectPath : `${projectPath}\\`;
      return normalizedPath === projectPath || normalizedPath.startsWith(projectPrefix);
    })
    .sort((left, right) => right.path.length - left.path.length)[0] ?? null;
}

function normalizeLocalPath(path: string) {
  return path
    .replace(/^\\\\\?\\UNC\\/i, "\\\\")
    .replace(/^\\\\\?\\/i, "")
    .replace(/\//g, "\\")
    .toLowerCase();
}

function recoveryOperationKindLabel(kind: string) {
  switch (kind) {
    case "quarantine":
      return "Moved to recovery area";
    case "restore":
      return "Restored";
    case "backup":
      return "Verified backup";
    case "purge":
    case "final_remove":
    case "permanent_delete":
      return "Held copy deleted";
    default:
      return humanizeIdentifier(kind);
  }
}

function recoveryStatusLabel(status: string) {
  switch (status) {
    case "completed":
    case "done":
      return "Completed";
    case "running":
    case "in_progress":
      return "In progress";
    case "failed":
      return "Failed";
    case "pending":
      return "Pending";
    case "cancelled":
      return "Stopped";
    default:
      return humanizeIdentifier(status);
  }
}

export function recoveryStoredEntryStatusLabel(status: string) {
  switch (status) {
    case "quarantined":
      return "Restorable in recovery area";
    case "restored":
      return "Restored";
    case "restore_content_mismatch":
      return "Restore destination has different content";
    case "purged":
    case "final_removed":
    case "permanently_deleted":
      return "Held copy deleted; recovery archive kept";
    default:
      return recoveryStatusLabel(status);
  }
}

function formatRemovedAt(removedAtUnix: number) {
  if (!removedAtUnix) return "earlier";
  try {
    return new Date(removedAtUnix * 1000).toLocaleString();
  } catch {
    return "earlier";
  }
}

function humanizeIdentifier(value: string) {
  return value
    .split(/[_-]+/g)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(" ") || "Unknown";
}
