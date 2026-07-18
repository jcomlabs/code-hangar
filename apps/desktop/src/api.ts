import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { fixtureApi } from "./fixtures";
import {
  readVisualAcceptanceState,
  shouldFailVisualAcceptanceCommand,
  visualAcceptanceDelayMs
} from "./visualAcceptance";
import type {
  AdapterSummary,
  AppRemovalRecord,
  PersistedAppRemoval,
  ProjectContextSummary,
  DbMaintenanceReport,
  Comment,
  ControlledCheckRun,
  CorrectionStaticCheckReport,
  ContextFile,
  DashboardSummary,
  DocumentSearchResult,
  DuplicateCandidates,
  DuplicateConfirmation,
  DuplicateConfirmStatus,
  EditableValueSet,
  EditSnapshotComparison,
  EditSnapshotRestoreResult,
  EditSnapshotSummary,
  ExportResult,
  FileEditPreview,
  FilePreview,
  FolderExplanation,
  FolderInvestigation,
  InvestigationHandle,
  GitRepoSummary,
  GraphMap,
  LostProjectCandidates,
  MutationActivityLog,
  MutationBackupSummary,
  MutationFinalRemoveSummary,
  MutationLockInspection,
  MutationMoveSummary,
  MutationProtectedPreview,
  MutationRestoreSummary,
  MutationTokenResult,
  NavChildrenPage,
  NavItem,
  NodeRelationships,
  OperationPlan,
  PlanPreviewStatus,
  OrphanCandidates,
  OrphanStatus,
  PinnedItem,
  PreviewPolicy,
  PreviewMode,
  ProcessResourceUsage,
  InstalledApp,
  ProjectDiscoveryReport,
  ProjectReviewCheckpoint,
  ProjectCheckDefinition,
  ProjectSummary,
  QuickOpenResult,
  RecentItem,
  RecoverableSummary,
  RecoveryPending,
  RecoveryResolveResult,
  ReviewLedgerEntry,
  RiskReport,
  ScanRoot,
  ScanStatus,
  SecurityStatus,
  SessionChangeSet,
  SessionPreview,
  StartupStatus,
  SystemResourceProfile,
  WatcherStatus,
  ValueEditRequest,
  ValueEditResult,
  ProtectedZone
} from "./types";

export type PerformanceMode = "balanced" | "priority" | "max";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const hasTauri = () => typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
export const hasTauriRuntime = hasTauri;

async function prepareVisualAcceptanceFallback(command: string) {
  const state = readVisualAcceptanceState();
  const delayMs = visualAcceptanceDelayMs(state);
  if (delayMs > 0) {
    await new Promise((resolve) => window.setTimeout(resolve, delayMs));
  }
  if (shouldFailVisualAcceptanceCommand(state, command)) {
    throw new Error(`Acceptance fixture failure: ${command}`);
  }
}

export async function call<T>(command: string, args?: Record<string, unknown>, fallback?: () => Promise<T>): Promise<T> {
  if (hasTauri()) {
    return invoke<T>(command, args);
  }
  if (!fallback) {
    throw new Error(`Command ${command} is only available in the Tauri desktop runtime.`);
  }
  await prepareVisualAcceptanceFallback(command);
  return fallback();
}

function isMissingCommandError(error: unknown) {
  const message = String(error).toLowerCase();
  return message.includes("unknown command") || message.includes("command not found") || message.includes("not found");
}

export async function optionalCommand<T>(command: string, args: Record<string, unknown> | undefined, fallback: () => Promise<T>): Promise<T> {
  if (hasTauri()) {
    try {
      return await invoke<T>(command, args);
    } catch (error) {
      if (isMissingCommandError(error)) {
        return fallback();
      }
      throw error;
    }
  }
  await prepareVisualAcceptanceFallback(command);
  return fallback();
}

function fixtureEditPreview(nodeId: number): FileEditPreview {
  return {
    nodeId,
    projectId: 1,
    beforeHash: "fixture-before",
    afterHash: "fixture-after",
    addedLines: 1,
    removedLines: 1,
    hunks: [{
      header: "@@ -1,1 +1,1 @@",
      oldStart: 1,
      newStart: 1,
      lines: [
        { kind: "removed", content: "const message = \"Before\";", oldLine: 1 },
        { kind: "added", content: "const message = \"After\";", newLine: 1 }
      ]
    }],
    diffTruncated: false,
    validation: {
      status: "passed",
      label: "Basic structure check passed",
      note: "Fixture review: this is a lightweight structure check, not a compiler result."
    },
    gitContext: {
      state: "modified",
      label: "This file already has local changes",
      note: "The review compares against the current bytes and does not stage or revert Git changes.",
      otherChangedFiles: 2
    }
  };
}

function fixtureSnapshotComparison(snapshotId: number): EditSnapshotComparison {
  const preview = fixtureEditPreview(1);
  return {
    snapshotId,
    nodeId: preview.nodeId,
    addedLines: preview.removedLines,
    removedLines: preview.addedLines,
    hunks: [{
      oldStart: 1,
      newStart: 1,
      header: "@@ -1,1 +1,1 @@",
      lines: [
        { kind: "removed", content: "const message = \"After\";", oldLine: 1 },
        { kind: "added", content: "const message = \"Before\";", newLine: 1 }
      ]
    }],
    diffTruncated: false,
    alreadyCurrent: false
  };
}

export const api = {
  startupStatus: () => call<StartupStatus>("startup_status", undefined, fixtureApi.startupStatus),
  projectsCachedSnapshot: () => optionalCommand<ProjectSummary[]>("projects_cached_snapshot", undefined, async () => []),
  cacheDiscoverySnapshot: (snapshot: string) => optionalCommand<void>("cache_discovery_snapshot", { snapshot }, () => fixtureApi.cacheDiscoverySnapshot(snapshot)),
  readDiscoverySnapshot: () => optionalCommand<string | null>("read_discovery_snapshot", undefined, () => fixtureApi.readDiscoverySnapshot()),
  projectsList: () => call<ProjectSummary[]>("projects_list", undefined, fixtureApi.projectsList),
  projectsListLite: () => optionalCommand<ProjectSummary[]>("projects_list_lite", undefined, fixtureApi.projectsList),
  projectGet: (projectId: number) => call<ProjectSummary | null>("project_get", { projectId }, () => fixtureApi.projectGet(projectId)),
  projectNavTree: (projectId: number) => call<NavItem[]>("project_nav_tree", { projectId }, () => fixtureApi.projectNavTree(projectId)),
  projectNavChildren: (projectId: number, parentNavId: number | null = null, limit = 200, offset = 0) =>
    call<NavChildrenPage>("project_nav_children", { projectId, parentNavId, limit, offset }, () => fixtureApi.projectNavChildren(projectId, parentNavId, limit, offset)),
  projectNavPath: (projectId: number, nodeId: number) =>
    call<NavItem[]>("project_nav_path", { projectId, nodeId }, () => fixtureApi.projectNavPath(projectId, nodeId)),
  projectGitStatus: (projectId: number) => call<GitRepoSummary>("project_git_status", { projectId }, () => fixtureApi.projectGitStatus(projectId)),
  folderExplanation: (navId: number) => call<FolderExplanation | null>("folder_explanation", { navId }, () => fixtureApi.folderExplanation(navId)),
  investigateFolder: (path: string, performanceMode?: string) => call<InvestigationHandle>("investigate_folder", { path, performanceMode }, () => fixtureApi.investigateFolder(path)),
  investigationReport: (rootId: number) => call<FolderInvestigation>("investigation_report", { rootId }, () => fixtureApi.investigationReport(rootId)),
  discardInvestigation: (rootId: number) => call<void>("discard_investigation", { rootId }, async () => {}),
  nodeFullPath: (nodeId: number, fallbackPath: string) => call<string>("node_full_path", { nodeId }, async () => fallbackPath),
  openNodeExternal: (nodeId: number) => call<void>("open_node_external", { nodeId }, async () => {
    throw new Error("Opening files with the operating system is only available in the Tauri desktop runtime.");
  }),
  revealNodeExternal: (nodeId: number) => call<void>("reveal_node_external", { nodeId }, async () => {
    throw new Error("Showing files in the operating system is only available in the Tauri desktop runtime.");
  }),
  revealProjectExternal: (projectId: number) => call<void>("reveal_project_external", { projectId }, async () => {
    throw new Error("Showing projects in the operating system is only available in the Tauri desktop runtime.");
  }),
  revealSessionExternal: (path: string) => call<void>("reveal_session_external", { path }, async () => {
    throw new Error("Showing conversation records in the operating system is only available in the Tauri desktop runtime.");
  }),
  dashboardSummary: (includeFixtureProjects = true) => call<DashboardSummary>(
    "dashboard_summary",
    { includeFixtureProjects },
    () => fixtureApi.dashboardSummary(includeFixtureProjects)
  ),
  adaptersList: () => call<AdapterSummary[]>("adapters_list", undefined, fixtureApi.adaptersList),
  projectContextFiles: (projectId: number) => call<ContextFile[]>("project_context_files", { projectId }, () => fixtureApi.projectContextFiles(projectId)),
  // "edit" is a frontend-only view; the backend preview understands only "rendered"/"source", so
  // map it here at the boundary — the editor is fed the file's source.
  filePreview: (nodeId: number, mode: PreviewMode, recordRecent = true, policy?: PreviewPolicy) => {
    const backendMode: PreviewMode = mode === "edit" || mode === "values" ? "source" : mode;
    return call<FilePreview>("file_preview", { nodeId, mode: backendMode, recordRecent, policy }, () => fixtureApi.filePreview(nodeId, backendMode, recordRecent, policy));
  },
  fileReveal: (nodeId: number, mode: PreviewMode, policy?: PreviewPolicy) => {
    const backendMode: PreviewMode = mode === "edit" || mode === "values" ? "source" : mode;
    return call<FilePreview>("file_reveal", { nodeId, mode: backendMode, policy }, () => fixtureApi.fileReveal(nodeId, backendMode, policy));
  },
  quickOpen: (query: string, limit = 20) => call<QuickOpenResult[]>("quick_open", { query, limit }, () => fixtureApi.quickOpen(query, limit)),
  performanceSetMode: (mode: PerformanceMode) => call<void>("performance_set_mode", { mode }, async () => undefined),
  systemResourceProfile: () => call<SystemResourceProfile>("system_resource_profile", undefined, fixtureApi.systemResourceProfile),
  processResourceUsage: () => optionalCommand<ProcessResourceUsage>("process_resource_usage", undefined, fixtureApi.processResourceUsage),
  watcherStatus: (focusedProjectId?: number | null, currentNodeId?: number | null) =>
    call<WatcherStatus>("watcher_status", { focusedProjectId, currentNodeId }, () => fixtureApi.watcherStatus(focusedProjectId, currentNodeId)),
  searchDocuments: (filters: { query: string; projectId?: number | null; indexedKind?: string; pathFilter?: string; nameFilter?: string; limit?: number; includeFixtureProjects?: boolean; performanceMode?: PerformanceMode }) =>
    call<DocumentSearchResult>("search_documents", { filters }, () => fixtureApi.searchDocuments(filters)),
  resolveLocalLink: (projectId: number, fromNodeId: number, target: string) =>
    call<number | null>("resolve_local_link", { projectId, fromNodeId, target }, () => fixtureApi.resolveLocalLink(projectId, fromNodeId, target)),
  nodeRelationships: (nodeId: number) => call<NodeRelationships>("node_relationships", { nodeId }, () => fixtureApi.nodeRelationships(nodeId)),
  projectGraphMap: (projectId: number, limit = 300) => call<GraphMap>("project_graph_map", { projectId, limit }, () => fixtureApi.projectGraphMap(projectId, limit)),
  graphOrphans: (limit = 50) => call<OrphanCandidates>("graph_orphans", { limit }, () => fixtureApi.graphOrphans(limit)),
  orphanAssetCandidates: (filters: { minSizeBytes?: number; projectId?: number | null; assetKind?: string; minConfidence?: string; includePartial?: boolean; limit?: number; includeFixtureProjects?: boolean; performanceMode?: PerformanceMode }) =>
    call<OrphanCandidates>("orphan_asset_candidates", { filters }, () => fixtureApi.orphanAssetCandidates(filters)),
  nodeOrphanStatus: (nodeId: number) =>
    call<OrphanStatus>("node_orphan_status", { nodeId }, () => fixtureApi.nodeOrphanStatus(nodeId)),
  lostProjectCandidates: (filters: { minSizeBytes?: number; projectId?: number | null; stalePreset?: string; signals?: string[]; keyword?: string; includePartial?: boolean; limit?: number; includeFixtureProjects?: boolean; performanceMode?: PerformanceMode }) =>
    call<LostProjectCandidates>("lost_project_candidates", { filters }, () => fixtureApi.lostProjectCandidates(filters)),
  projectDiscoveryReport: (limit = 100, includeLooseSessions = false, includeAgents = false, includeTechnicalCandidates = false) =>
    call<ProjectDiscoveryReport>("project_discovery_report", { limit, includeLooseSessions, includeAgents, includeTechnicalCandidates }, () => fixtureApi.projectDiscoveryReport(limit)),
  projectDiscoveryDeepScan: (rootPath: string, limit = 250, includeLooseSessions = false, includeAgents = false, includeTechnicalCandidates = false) =>
    call<ProjectDiscoveryReport>("project_discovery_deep_scan", { rootPath, limit, includeLooseSessions, includeAgents, includeTechnicalCandidates }, () => fixtureApi.projectDiscoveryDeepScan(rootPath, limit)),
  detectInstalledApps: () => call<InstalledApp[]>("detect_installed_apps", {}, () => fixtureApi.detectInstalledApps()),
  wslScanEnabled: () => call<boolean>("wsl_scan_enabled", {}, () => fixtureApi.wslScanEnabled()),
  setWslScanEnabled: (enabled: boolean) => call<void>("set_wsl_scan_enabled", { enabled }, () => fixtureApi.setWslScanEnabled(enabled)),
  sessionPreview: (path: string, reveal = false, options?: { maxBytes?: number; loadFull?: boolean }) =>
    call<SessionPreview>(
      "session_preview",
      { path, reveal, maxBytes: options?.maxBytes, loadFull: options?.loadFull ?? false },
      () => fixtureApi.sessionPreview(path, reveal, options)
    ),
  sessionChangeSet: (path: string) =>
    call<SessionChangeSet>("session_change_set", { path }, () => fixtureApi.sessionChangeSet(path)),
  projectSessionChangeSet: (projectId: number, path: string) =>
    call<SessionChangeSet>("project_session_change_set", { projectId, path }, () => fixtureApi.sessionChangeSet(path)),
  projectGitChangeSet: (projectId: number) =>
    call<SessionChangeSet>("project_git_change_set", { projectId }, () => fixtureApi.projectGitChangeSet(projectId)),
  projectReviewCheckpoint: (projectId: number) =>
    call<ProjectReviewCheckpoint | null>("project_review_checkpoint", { projectId }, async () => null),
  projectReviewCheckpoints: () =>
    call<ProjectReviewCheckpoint[]>("project_review_checkpoints", undefined, async () => []),
  markProjectReviewed: (projectId: number, sessionCutoffMs: number) =>
    call<ProjectReviewCheckpoint>("mark_project_reviewed", { projectId, sessionCutoffMs }, async () => ({
      projectId,
      reviewedAt: new Date(sessionCutoffMs).toISOString(),
      sessionCutoffMs,
      gitFingerprint: null,
      gitHead: null
    })),
  projectReviewLedger: (projectId: number, limit = 100) =>
    call<ReviewLedgerEntry[]>("project_review_ledger", { projectId, limit }, async () => []),
  projectRecap: (projectId: number, sessionPaths: string[]) =>
    call<SessionChangeSet>("project_recap", { projectId, sessionPaths }, async () => {
      const sources = await Promise.all(sessionPaths.slice(0, 30).map((path) => fixtureApi.sessionChangeSet(path)));
      return sources[0] ?? fixtureApi.projectGitChangeSet(projectId);
    }),
  projectReviewReceiptExport: (projectId: number, sessionPaths: string[], scope: "since" | "all", path: string) =>
    call<ExportResult>("project_review_receipt_export", { projectId, sessionPaths, scope, path }, async () => ({
      path,
      bytesWritten: 0
    })),
  duplicateCandidates: (filters: { minSizeBytes?: number; projectId?: number | null; fileKind?: string; currentFileNodeId?: number | null; limit?: number; includeFixtureProjects?: boolean; performanceMode?: PerformanceMode } = {}) =>
    call<DuplicateCandidates>("duplicate_candidates", { filters }, () => fixtureApi.duplicateCandidates(filters)),
  confirmDuplicateGroup: (nodeId: number): Promise<DuplicateConfirmation> =>
    call<DuplicateConfirmation>("confirm_duplicate_group", { nodeId }, () => fixtureApi.confirmDuplicateGroup(nodeId)),
  // On-demand full-hash confirmation as a background job (streams file bytes; cancelable, with
  // progress). Mirrors operationPlanStart/Status/Cancel. Read-only; only runs on explicit request.
  confirmDuplicateGroupStart: (nodeId: number): Promise<string> =>
    call<string>("confirm_duplicate_group_start", { nodeId }, async () => `dup-${nodeId}`),
  confirmDuplicateGroupStatus: (jobId: string): Promise<DuplicateConfirmStatus> =>
    call<DuplicateConfirmStatus>("confirm_duplicate_group_status", { jobId }, async () => {
      const nodeId = Number(jobId.replace("dup-", "")) || 1;
      const result = await fixtureApi.confirmDuplicateGroup(nodeId);
      return {
        jobId,
        state: "completed",
        targetNodeId: nodeId,
        message: "Duplicate verification complete.",
        error: null,
        progress: { checkedFiles: result.checkedFiles, totalFiles: result.checkedFiles, bytesHashed: result.bytesHashed, totalBytes: result.bytesHashed },
        result
      };
    }),
  confirmDuplicateGroupCancel: (jobId: string): Promise<void> =>
    call<void>("confirm_duplicate_group_cancel", { jobId }, async () => undefined),
  projectRecoverableSummary: (projectId: number) =>
    call<RecoverableSummary>("project_recoverable_summary", { projectId }, () => fixtureApi.projectRecoverableSummary(projectId)),
  nodeRecoverableSummary: (nodeId: number) =>
    call<RecoverableSummary>("node_recoverable_summary", { nodeId }, () => fixtureApi.nodeRecoverableSummary(nodeId)),
  operationPlanBuild: (targetNodeId: number, actionLabel: string, performanceMode?: PerformanceMode) =>
    call<OperationPlan>("operation_plan_build", { targetNodeId, actionLabel, performanceMode }, () => fixtureApi.operationPlanBuild(targetNodeId, actionLabel)),
  operationPlanStart: (targetNodeId: number, actionLabel: string, performanceMode?: PerformanceMode) =>
    call<string>("operation_plan_start", { targetNodeId, actionLabel, performanceMode }, async () => `fixture-plan-${targetNodeId}`),
  operationPlanStatus: (jobId: string) =>
    call<PlanPreviewStatus>("operation_plan_status", { jobId }, async () => {
      const targetNodeId = Number(jobId.replace("fixture-plan-", "")) || 1;
      const plan = await fixtureApi.operationPlanBuild(targetNodeId, "Future backup, move, or delete review");
      const report = await fixtureApi.riskReportBuild(plan);
      return {
        jobId,
        state: "completed",
        targetNodeId,
        actionLabel: plan.actionLabel,
        message: "Preview plan calculated.",
        error: null,
        plan,
        report
      };
    }),
  operationPlanCancel: (jobId: string) =>
    call<void>("operation_plan_cancel", { jobId }, async () => undefined),
  riskReportBuild: (plan: OperationPlan, performanceMode?: PerformanceMode) =>
    call<RiskReport>("risk_report_build", { plan, performanceMode }, () => fixtureApi.riskReportBuild(plan)),
  riskReportBuildForTarget: (targetNodeId: number, actionLabel: string, performanceMode?: PerformanceMode) =>
    call<RiskReport>("risk_report_build_for_target", { targetNodeId, actionLabel, performanceMode }, () => fixtureApi.riskReportBuildForTarget(targetNodeId, actionLabel)),
  riskReportExport: (report: RiskReport, path: string) =>
    call<ExportResult>("risk_report_export", { report, path }, () => fixtureApi.riskReportExport(report, path)),
  diagnosticsExport: (path: string) =>
    call<ExportResult>("diagnostics_export", { path }, async () => ({ path, bytesWritten: 0 })),
  recentItemsList: (limit = 20) => call<RecentItem[]>("recent_items_list", { limit }, () => fixtureApi.recentItemsList(limit)),
  pinnedItemsList: () => call<PinnedItem[]>("pinned_items_list", undefined, fixtureApi.pinnedItemsList),
  pinItem: (nodeId: number, itemKind: string) => call<void>("pin_item", { nodeId, itemKind }, () => fixtureApi.pinItem(nodeId, itemKind)),
  unpinItem: (nodeId: number, itemKind: string) => call<void>("unpin_item", { nodeId, itemKind }, () => fixtureApi.unpinItem(nodeId, itemKind)),
  commentsForNode: (nodeId: number) => call<Comment[]>("comments_for_node", { nodeId }, () => fixtureApi.commentsForNode(nodeId)),
  commentsCountForNode: (nodeId: number) => call<number>("comments_count_for_node", { nodeId }, () => fixtureApi.commentsCountForNode(nodeId)),
  commentAdd: (nodeId: number, body: string) =>
    call<Comment>("comment_add", { nodeId, body }, () => fixtureApi.commentAdd(nodeId, body)),
  commentEdit: (commentId: number, body: string) => call<Comment>("comment_edit", { commentId, body }, () => fixtureApi.commentEdit(commentId, body)),
  commentDelete: (commentId: number) => call<void>("comment_delete", { commentId }, () => fixtureApi.commentDelete(commentId)),
  rootsList: () => call<ScanRoot[]>("roots_list", undefined, fixtureApi.rootsList),
  rootsAdd: (path: string) => call<ScanRoot>("roots_add", { path }, () => fixtureApi.rootsAdd(path)),
  rootsSetEnabled: (rootId: number, enabled: boolean) => call<ScanRoot>("roots_set_enabled", { rootId, enabled }, () => fixtureApi.rootsSetEnabled(rootId, enabled)),
  rootsUnregister: (rootId: number) => call<void>("roots_unregister", { rootId }, () => fixtureApi.rootsUnregister(rootId)),
  projectsUnregister: (projectId: number) => call<void>("projects_unregister", { projectId }, () => fixtureApi.projectsUnregister(projectId)),
  resetAllProjects: () => call<number>("reset_all_projects", undefined, fixtureApi.resetAllProjects),
  compactDatabase: () => call<DbMaintenanceReport>("compact_database", undefined, fixtureApi.compactDatabase),
  restartApp: () => call<void>("restart_app", undefined, fixtureApi.restartApp),
  scanStart: (rootIds?: number[], performanceMode?: PerformanceMode) => call<string>("scan_start", { rootIds, performanceMode }, () => fixtureApi.scanStart()),
  scanResumeSubtree: (navId: number, performanceMode?: PerformanceMode) => call<string>("scan_resume_subtree", { navId, performanceMode }, () => fixtureApi.scanResumeSubtree(navId)),
  scanCancel: (jobId: string) => call<void>("scan_cancel", { jobId }, () => fixtureApi.scanCancel(jobId)),
  scanStatus: (jobId: string) => call<ScanStatus>("scan_status", { jobId }, () => fixtureApi.scanStatus(jobId)),
  zonesList: () => call<ProtectedZone[]>("zones_list", undefined, fixtureApi.zonesList),
  securityStatus: () => call<SecurityStatus>("security_status", undefined, fixtureApi.securityStatus),
  mutationModeStatus: () => optionalCommand<boolean>("mutation_mode_status", undefined, fixtureApi.mutationModeStatus),
  mutationFinalRemoveEnabled: () => optionalCommand<boolean>("mutation_final_remove_enabled", undefined, fixtureApi.mutationFinalRemoveEnabled),
  mutationSetFinalRemoveEnabled: (enabled: boolean) => optionalCommand<void>("mutation_set_final_remove_enabled", { enabled }, async () => undefined),
  recoveryPending: () => optionalCommand<RecoveryPending>("recovery_pending", undefined, fixtureApi.recoveryPending),
  recoveryResolve: (decision: "rollback") =>
    call<RecoveryResolveResult>("recovery_resolve", { decision }, () => fixtureApi.recoveryResolve(decision)),
  mutationTokenIssue: (action: "enter_mutation_mode" | "final_remove") =>
    call<MutationTokenResult>("mutation_token_issue", { action }, () => fixtureApi.mutationTokenIssue(action)),
  mutationBackupStart: (plan: OperationPlan, destinationRoot: string, level: "minimal" | "standard" | "full", allowSameVolume: boolean, includeProtected: boolean, token: string) =>
    call<MutationBackupSummary>("mutation_backup_start", { plan, destinationRoot, level, allowSameVolume, includeProtected, token }, () => fixtureApi.mutationBackupStart(plan, destinationRoot, level, allowSameVolume, includeProtected, token)),
  mutationMoveStart: (plan: OperationPlan, holdingRoot: string, verifiedBackupId: number, includeProtected: boolean, token: string) =>
    call<MutationMoveSummary>("mutation_move_start", { plan, holdingRoot, verifiedBackupId, includeProtected, token }, () => fixtureApi.mutationMoveStart(plan, holdingRoot, verifiedBackupId, includeProtected, token)),
  mutationPreviewProtected: (plan: OperationPlan) =>
    call<MutationProtectedPreview>("mutation_preview_protected", { plan }, () => fixtureApi.mutationPreviewProtected(plan)),
  mutationRestoreStart: (entryId: number, token: string) =>
    call<MutationRestoreSummary>("mutation_restore_start", { entryId, token }, () => fixtureApi.mutationRestoreStart(entryId, token)),
  mutationRestoreToFolderStart: (entryId: number, destinationRoot: string, token: string) =>
    call<MutationRestoreSummary>("mutation_restore_to_folder_start", { entryId, destinationRoot, token }, () => fixtureApi.mutationRestoreToFolderStart(entryId, destinationRoot, token)),
  mutationFinalRemoveStart: (entryId: number, token: string) =>
    call<MutationFinalRemoveSummary>("mutation_final_remove_start", { entryId, token }, () => fixtureApi.mutationFinalRemoveStart(entryId, token)),
  mutationActivityLog: (limit = 50) =>
    optionalCommand<MutationActivityLog>("mutation_activity_log", { limit }, () => fixtureApi.mutationActivityLog(limit)),
  mutationLockInspectPath: (path: string) =>
    call<MutationLockInspection>("mutation_lock_inspect_path", { path }, () => fixtureApi.mutationLockInspectPath(path)),
  appRemovalsList: () =>
    optionalCommand<PersistedAppRemoval[]>("app_removals_list", {}, async () => []),
  appRemovalRestore: (id: string) =>
    optionalCommand<void>("app_removal_restore", { id }, async () => undefined),
  // Local/Connector editions only: write manually edited text back to an inventoried file.
  // AI-selected suggestions use an opaque backend proposal command instead.
  // Returns the EXACT prior file content (read server-side) so Undo restores the true original,
  // never a size-capped UI snapshot.
  fileEditPreview: (nodeId: number, content: string, expectedContent?: string) =>
    optionalCommand<FileEditPreview>("file_edit_preview", { nodeId, content, expectedContent }, async () => fixtureEditPreview(nodeId)),
  writeFileContent: (nodeId: number, content: string, origin: "manual" | "restore" = "manual", expectedContent?: string, reviewedAfterHash?: string) =>
    optionalCommand<string>("write_file_content", { nodeId, content, origin, expectedContent, reviewedAfterHash }, async () => { throw new Error("Editing is not available in this edition."); }),
  editSnapshotsForNode: (nodeId: number, limit = 20) =>
    optionalCommand<EditSnapshotSummary[]>("edit_snapshots_for_node", { nodeId, limit }, async () => []),
  editSnapshotRestore: (snapshotId: number) =>
    optionalCommand<EditSnapshotRestoreResult>("edit_snapshot_restore", { snapshotId }, async () => { throw new Error("File history is not available in this edition."); }),
  editSnapshotCompare: (snapshotId: number) =>
    optionalCommand<EditSnapshotComparison>("edit_snapshot_compare", { snapshotId }, async () => fixtureSnapshotComparison(snapshotId)),
  editableValues: (nodeId: number) =>
    optionalCommand<EditableValueSet>("editable_values", { nodeId }, async () => { throw new Error("Values are not available in this edition."); }),
  previewValueEdit: (nodeId: number, request: ValueEditRequest) =>
    optionalCommand<FileEditPreview>("preview_value_edit", { nodeId, request }, async () => fixtureEditPreview(nodeId)),
  applyValueEdit: (nodeId: number, request: ValueEditRequest, reviewedAfterHash: string) =>
    optionalCommand<ValueEditResult>("apply_value_edit", { nodeId, request, reviewedAfterHash }, async () => { throw new Error("Values are not available in this edition."); }),
  staticCorrectionCheck: (nodeId: number) =>
    optionalCommand<CorrectionStaticCheckReport>("static_correction_check", { nodeId }, async () => ({
      nodeId,
      projectId: 0,
      path: "",
      status: "passed",
      checks: [{ id: "fixture", label: "Static fixture", status: "not_applicable", detail: "Static checks require the desktop runtime." }],
      checkedAt: new Date().toISOString(),
      executedProjectCode: false
    })),
  projectChecksDetect: (projectId: number) =>
    optionalCommand<ProjectCheckDefinition[]>("project_checks_detect", { projectId }, async () => []),
  projectCheckApprove: (projectId: number, checkId: string, fingerprint: string) =>
    optionalCommand<ProjectCheckDefinition>("project_check_approve", { projectId, checkId, fingerprint }, async () => { throw new Error("Controlled checks require the desktop runtime."); }),
  projectCheckRevoke: (projectId: number, checkId: string) =>
    optionalCommand<boolean>("project_check_revoke", { projectId, checkId }, async () => false),
  projectCheckRun: (projectId: number, nodeId: number, checkId: string, fingerprint: string) =>
    optionalCommand<ControlledCheckRun>("project_check_run", { projectId, nodeId, checkId, fingerprint }, async () => { throw new Error("Controlled checks require the desktop runtime."); }),
  removeProjectFromApps: (projectId: number) =>
    optionalCommand<PersistedAppRemoval | null>("remove_project_from_apps", { projectId }, async () => null),
  projectContextSummary: (projectId: number) =>
    call<ProjectContextSummary>("project_context_summary", { projectId }, async () => ({
      kinds: [],
      readmeTitle: null,
      readmeExcerpt: null,
      runCommands: [],
      manifestFiles: [],
      markdownFiles: [],
    })),
  async pickFolder(title = "Choose a read-only scan root"): Promise<string | null> {
    if (!hasTauri()) return null;
    const result = await open({ directory: true, multiple: false, title });
    return typeof result === "string" ? result : null;
  },
  async pickReportPath(): Promise<string | null> {
    if (!hasTauri()) return null;
    const result = await save({
      title: "Export risk report",
      defaultPath: "codehangar-risk-report.json",
      filters: [{ name: "JSON", extensions: ["json"] }]
    });
    return typeof result === "string" ? result : null;
  },
  async pickReviewReceiptPath(): Promise<string | null> {
    if (!hasTauri()) return null;
    const result = await save({
      title: "Export private-safe review receipt",
      defaultPath: "code-hangar-review-receipt.html",
      filters: [{ name: "HTML", extensions: ["html"] }]
    });
    return typeof result === "string" ? result : null;
  },
  async pickDiagnosticsPath(): Promise<string | null> {
    if (!hasTauri()) return null;
    const result = await save({
      title: "Export redacted diagnostic bundle",
      defaultPath: "code-hangar-diagnostics.json",
      filters: [{ name: "JSON", extensions: ["json"] }]
    });
    return typeof result === "string" ? result : null;
  }
};
