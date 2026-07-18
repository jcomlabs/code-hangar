import { useMemo, useState } from "react";
import { AlertTriangle, ArchiveRestore, CheckCircle2, ChevronDown, ChevronUp, History, RefreshCcw, Shield } from "lucide-react";
import { ConceptHelp } from "../BeginnerHelp";
import type { MutationActivityItem, MutationActivityLog, MutationActivityOperation, MutationStoredEntry, PersistedAppRemoval, ProjectSummary } from "../types";
import { displayLocalPath, formatBytes, formatOptionalBytes, formatTimestamp } from "../ui";
import { PreviousVersions } from "./project-center/PreviousVersions";

const RECOVERY_PREVIEW_LIMIT = 3;
const TECHNICAL_ACTIVITY_LIMIT = 30;

export function RecoveryView({
  mutationAvailable,
  finalRemoveEnabled,
  mutationMessage,
  mutationActivity,
  mutationBusy,
  advancedMode,
  projects,
  appRemovals,
  restoreAppRemoval,
  refreshMutationActivity,
  runMutationRestore,
  runMutationRestoreElsewhere,
  runMutationFinalRemove,
  onToggleFinalRemove,
  onDiscoverProjects,
  onOpenScanFolders,
  currentFile,
  onFileHistoryMutated,
  setStatusText
}: {
  mutationAvailable: boolean;
  finalRemoveEnabled: boolean;
  mutationMessage: string | null;
  mutationActivity: MutationActivityLog | null;
  mutationBusy: boolean;
  advancedMode: boolean;
  projects: ProjectSummary[];
  appRemovals: PersistedAppRemoval[];
  restoreAppRemoval: (id: string, projectName: string) => Promise<void>;
  refreshMutationActivity: () => Promise<boolean>;
  runMutationRestore: (entryId: number) => void;
  runMutationRestoreElsewhere: (entryId: number) => void;
  runMutationFinalRemove: (entryId: number) => void;
  onToggleFinalRemove: (enabled: boolean) => void;
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
  const heldGroups = useMemo(
    () => groupStoredEntriesByProject(mutationActivity?.storedEntries ?? [], projects),
    [mutationActivity?.storedEntries, projects]
  );
  const summary = useMemo(
    () => mutationActivity ? summarizeRecovery(mutationActivity, appRemovals.length) : null,
    [appRemovals.length, mutationActivity]
  );
  const orderedStoredEntries = useMemo(
    () => orderRecoveryStoredEntries(mutationActivity?.storedEntries ?? []),
    [mutationActivity?.storedEntries]
  );
  const visibleOperations = recoveryPreviewItems(mutationActivity?.operations ?? [], showAllOperations);
  const visibleBackups = recoveryPreviewItems(mutationActivity?.backups ?? [], showAllBackups);
  const visibleAppRemovals = recoveryPreviewItems(appRemovals, showAllAppRemovals);
  const hasRecoveryRecords = recoveryHasRecords(mutationActivity, appRemovals.length);
  const emptyState = recoveryEmptyState(mutationAvailable);
  const nothingToRecover = Boolean(mutationActivity && !hasRecoveryRecords);
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

  return (
    <section className="pane-section compact recovery-view">
      {summary && hasRecoveryRecords ? (
        <div className="preview-banner" data-help="This area explains verified backups, recoverable held files and the local history needed to recover interrupted disk actions.">
          {recoveryStatusBanner(summary)}
        </div>
      ) : null}
      {hasRecoveryRecords || mutationAvailable ? (
        <div className="button-row activity-toolbar">
          <button type="button" className="secondary-button activity-refresh-button" disabled={refreshState === "loading" || mutationBusy} data-help="Reload local recovery history, verified backups and stored entries." onClick={() => void refreshHistory()}>
            <RefreshCcw size={14} className={refreshState === "loading" ? "spin" : ""} />
            <span>{refreshState === "loading" ? "Checking history…" : "Refresh history"}</span>
          </button>
          {refreshState === "done" ? (
            <span className="activity-refresh-status" role="status" aria-live="polite"><CheckCircle2 size={14} /> History is current</span>
          ) : null}
        </div>
      ) : null}
      {mutationMessage ? <p className="mutation-message" data-help="Latest recovery or safe-action status message.">{mutationMessage}</p> : null}
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
              <span>VERIFIED BACKUPS</span>
              <strong>{summary.verifiedBackups}</strong>
              <small>{summary.totalBackups === 0 ? "No backup records" : `${summary.verifiedBackups} of ${summary.totalBackups} verified`}</small>
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
          <section className="activity-card" data-help="Stored-file records distinguish files currently held in recovery from completed restores and final removals. Restore refuses to overwrite an occupied destination.">
            <div className="heading-with-help">
              <h3>Held and restored files</h3>
              <ConceptHelp concept="backup" />
            </div>
            <p className="muted help-copy">
              {summary && summary.heldFiles > 0
                ? `${summary.heldFiles} file${summary.heldFiles === 1 ? " is" : "s are"} currently held and ready for review.`
                : `Nothing is currently held. ${summary?.resolvedStoredRecords ?? 0} completed record${summary?.resolvedStoredRecords === 1 ? " remains" : "s remain"} as local history.`}
            </p>
            <div className="held-group-list">
              {heldGroups.map((group) => (
                <div className="activity-row compact-row" key={group.key} data-help={`Stored-file records for ${group.label}. ${group.count} records, ${group.quarantined} currently held, ${formatBytes(group.spaceRecovered)} recorded as recovered space.`}>
                  <div>
                    <strong>{group.label}</strong>
                    <span>{storedGroupStatusLabel(group.count, group.quarantined)}</span>
                    <small>{displayLocalPath(group.samplePath)}</small>
                  </div>
                  <small>{formatBytes(group.spaceRecovered)}</small>
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
            {shouldShowFinalRemoveOptIn(finalRemoveEnabled, mutationAvailable, summary?.heldFiles ?? 0) ? (
              <label
                className={`toggle-row final-remove-optin ${finalRemoveEnabled ? "danger-card" : ""}`}
                data-help={
                  finalRemoveEnabled
                    ? "Final removal is available. It permanently removes a held copy from disk, but every removal still needs a verified backup and a fresh confirmation."
                    : "Final removal is turned off. Held files can still be restored; irreversible removal stays hidden until you enable it here."
                }
              >
                <input type="checkbox" checked={finalRemoveEnabled} disabled={mutationBusy} onChange={() => onToggleFinalRemove(!finalRemoveEnabled)} />
                <span>
                  <strong className="final-remove-label"><AlertTriangle size={15} /> Final removal (irreversible) — {finalRemoveEnabled ? "on" : "off"}</strong>
                  <small>
                    {finalRemoveEnabled
                      ? "Opted in for this installation. A verified backup and fresh confirmation are still required for every final removal."
                      : "Turned off. Held entries remain restorable, and the irreversible action stays hidden."}
                  </small>
                </span>
              </label>
            ) : null}
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
                    <small>{formatOptionalBytes(entry.size)} · recovered {formatBytes(entry.spaceRecovered)}</small>
                    {isHeld ? (
                      <>
                        <button type="button" className="secondary-button" disabled={!mutationAvailable || mutationBusy} data-help="Restore this stored entry to its original path if that path is free. This changes files on disk." onClick={() => void runMutationRestore(entry.id)}>Restore</button>
                        <button type="button" className="secondary-button" disabled={!mutationAvailable || mutationBusy} data-help="Choose a different destination folder. Code Hangar preserves the stored relative path and refuses to overwrite an existing file." onClick={() => void runMutationRestoreElsewhere(entry.id)}>Restore elsewhere...</button>
                        {finalRemoveEnabled ? (
                          <button type="button" className="secondary-button danger-outline" disabled={!finalRemoveEntryActionEnabled(finalRemoveEnabled, mutationAvailable, mutationBusy, entry.status)} data-help="Irreversibly remove the stored copy after a fresh confirmation. Always requires a verified backup that covers the file." onClick={() => void runMutationFinalRemove(entry.id)}>Final remove</button>
                        ) : null}
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

export function finalRemoveEntryActionEnabled(
  finalRemoveEnabled: boolean,
  mutationAvailable: boolean,
  mutationBusy: boolean,
  status: MutationStoredEntry["status"]
) {
  return finalRemoveEnabled && mutationAvailable && !mutationBusy && status === "quarantined";
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

export function shouldShowFinalRemoveOptIn(
  finalRemoveEnabled: boolean,
  mutationAvailable: boolean,
  heldFileCount: number
): boolean {
  return mutationAvailable && (heldFileCount > 0 || finalRemoveEnabled);
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
    spaceRecovered: number;
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
      spaceRecovered: 0,
      samplePath: entry.originalPath
    };
    current.count += 1;
    if (entry.status === "quarantined") current.quarantined += 1;
    current.spaceRecovered += entry.spaceRecovered;
    groups.set(key, current);
  }
  return Array.from(groups.values()).sort((left, right) => right.spaceRecovered - left.spaceRecovered);
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
      return "Final removal";
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
      return "Finally removed";
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
