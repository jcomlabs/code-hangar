import {
  Activity,
  AlertTriangle,
  Archive,
  ArchiveRestore,
  ArrowLeft,
  ArrowRight,
  BarChart3,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Command,
  Compass,
  Copy,
  Database,
  Layers,
  Eye,
  FileText,
  Folder,
  FolderOpen,
  Home,
  History,
  Info,
  Keyboard,
  Loader2,
  Lock,
  ListChecks,
  MessageSquare,
  Moon,
  PanelLeft,
  Pin,
  PinOff,
  Plug,
  Radar,
  RefreshCcw,
  Search,
  FolderSearch,
  Settings,
  Shield,
  SlidersHorizontal,
  Sun,
  TerminalSquare,
  X
} from "lucide-react";
import { Fragment, lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent, MouseEvent, PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ADD_PROJECTS_DEEP_SCAN_ACTION,
  ADD_PROJECTS_SHOW_PROGRESS_ACTION,
  deepScanSourceLabels,
  deepScanUsesIndeterminateProgress,
  partitionInstalledApps,
  type DeepScanPhase
} from "./addProjectsDialog";
import { api, hasTauriRuntime } from "./api";
import type { PerformanceMode } from "./api";
import {
  globalPaletteShortcut,
  paletteFocusIndex,
  palettePointerMayMoveFocus,
  paletteShortcutsBlocked,
  projectScopedCommandState,
  scrollPaletteResultIntoView,
  type PaletteNavigationKey
} from "./commandPalette";
import { ContextMenu, contextMenuCoordinates, fileContextCapabilities } from "./ContextMenu";
import type { ContextMenuState } from "./ContextMenu";
import { DIALOG_FOCUSABLE_SELECTOR, DIALOG_INITIAL_FOCUS_SELECTOR, nextDialogFocusIndex } from "./dialogFocus";
import {
  attemptDeepScanInventoryStart,
  deepScanBuildProjectState,
  deepScanOutcomeFromScanState,
  deepScanTerminalPercent,
  deepScanTerminalPresentation,
  type DeepScanBuildProjectState,
  type DeepScanOutcome
} from "./deepScanState";
import { documentSearchCriteriaKey, duplicateSearchCriteriaKey, orphanSearchCriteriaKey, projectPickerInputStatus, resolveProjectPickerInput, retainRunningDuplicateConfirmations, scopeForDiscoveryEntry, scopeForDocumentSearchEntry } from "./documentSearch";
import type { DocumentSearchScope } from "./documentSearch";
import { graphMapItemCounts, INITIAL_GRAPH_MAP_LIMIT, nextGraphMapExpansionLimit } from "./graphMapExpansion";
import { pinFailureMessage, pinSuccessMessage, postActionHoverHelp, scanRootToggleFailureMessage, scanRootToggleMessage } from "./interactionFeedback";
import { renderMarkdownSafe } from "./markdown";
import { FILE_INSPECTOR_CONTEXT, projectInspectorContext, projectViewUsesFileInspector } from "./projectInspector";
import { compactSidebarProjects, composeQuickOpenResults, isDemoProject, orderSidebarProjects, projectSearchKeyAction, projectWatchLabel, quickOpenSearchMessage, resolveProjectScanState, shouldRenderProjectRow, shouldShowDemoProjects, starterQuickOpenResults, visibleProjectItems, visibleProjects, type QuickOpenSearchStatus } from "./projectVisibility";
import type { ProjectSort, ProjectStatusFilter } from "./projectVisibility";
import { refreshUnindexedPreview } from "./previewRefresh";
import { removeProjectActionLabel } from "./removeProjectDialog";
import { shouldDeferResidentUi } from "./residentStartup";
import { applySafeManageFirstRunChoice } from "./safeManageFirstRun";
import { unregisterProjectConfirmationMessage, unregisterRootConfirmationMessage } from "./settingsActions";
import { INITIAL_CONTEXT_OPEN_OPTIONS, selectInitialContextFile } from "./projectAutoOpen";
import { formatScanDuration, scanProgressParts, shouldCelebrateStandaloneScan } from "./scanProgress";
import { mergeScanStatusSnapshot, scanStatusAnnouncementKind } from "./scanStatusStore";
import { disposeShellViewerSafely, shellOpenImmediateMode, shellOpenPreviewReadOwnsFocus, shellOpenRequestOwnsFocus, shellPreviewMode, shellScanFailedToOpen, shellScanIsPending } from "./shellOpen";
import { clampSessionTranscriptPage, compactSessionToolActivity, connectedToolServerCount, enrichedSessionDisplayName, initialSessionTranscriptPage, nextSessionPreviewLimit, parseSessionMetadata, parseSessionTranscript, SESSION_TRANSCRIPT_PAGE_SIZE, sessionDisplayNameNeedsEnrichment, sessionSupportsProgressiveLoading, sessionTranscriptPageCount, sessionTranscriptPageSlice, type SessionMetadataSummary } from "./session-transcript";
import { SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT, compactSidebarSessionGroups, displayedSidebarSessionGroups, previewSidebarSessionItems } from "./sessionSidebar";
import type { SessionScope, SessionSort } from "./sessionSidebar";
import { appendFileHistoryEntry, fileMembershipKey, focusedFileStatusIsRelevant, INITIAL_WORKSPACE_ROUTE, normalizeStartupPreferences, parseWorkspaceRoute, projectInspectorCollapsedForLayout, projectSidebarCollapsedForLayout, projectViewPrefersWideCanvas, sameWorkspaceRoute, shouldRecordFileHistoryEntry, shouldRecordWorkspaceRoute, startupPaneCollapse, startupWorkspaceRoute, workspaceCenterPaneIsCramped, workspaceRouteStatusText } from "./workspaceRoute";
import type { DiscoverView, FileHistoryEntry, PrimaryView, ProjectView, RightPaneView, SettingsView, WorkspaceRoute } from "./workspaceRoute";
import type { StartupPreferences } from "./workspaceRoute";
import { applyWslScanPreference, isWslScanPreferenceApplyError, runWslGatedDiscovery, type WslGatedDiscoveryScope } from "./wslScanGate";
import { displayAppText, projectAppMetas, sessionAppMeta, type AppMeta } from "./app-meta";
import { InspectorPane, ProjectWorkspace, Sidebar, ToolWorkspace, WorkspaceGrid } from "./WorkspaceShell";
import { selectedProjectActivation, useProjectWorkspace } from "./useProjectWorkspace";
import { loadStartupSideData } from "./startupSideData";
import { useTabDrag } from "./hooks/useTabDrag";
import { ConceptHelp, type BeginnerHelpConcept } from "./BeginnerHelp";
import { CountUp, SectionTitle, compactLocalPath, displayLocalPath, formatBytes, formatOptionalBytes, formatTimestamp, orphanReferenceStatusText, quickOpenLocationLabel, storedBooleanPreference } from "./ui";
import { ProjectCenterView, projectSidebarSummaryLabel } from "./views/ProjectCenterView";
import type { ConnectorEditionBridge } from "./views/ConnectorEditionLayer";
import { ChangeAccessDialog } from "./views/project-center/ChangeAccessDialog";
import { GuidedTour, guidedTourStepCopy, guidedTourStorageKey, TOUR_SELECTORS, type GuidedTourMode, type TourStep } from "./views/GuidedTour";
import type { DuplicateConfirmStateMap } from "./views/DiscoverView";
import type {
  AdapterSummary,
  DashboardSummary,
  DocumentHit,
  DuplicateCandidates,
  FilePreview,
  FinalRemoveBatchPreview,
  FinalRemoveBatchProgress,
  FinalRemoveBatchResult,
  FinalRemoveBatchStatus,
  FinalRemoveObjectDecision,
  FinalRemoveScope,
  FolderExplanation,
  FolderInvestigation,
  GraphMap,
  GraphMapExpansionState,
  LostProjectCandidates,
  MutationActivityLog,
  MutationLockInspection,
  MutationMoveSummary,
  NavItem,
  NodeRelationships,
  OperationPlan,
  OrphanCandidates,
  OrphanStatus,
  OpenTargetInspection,
  OpenTargetPreparation,
  PinnedItem,
  PlanPreviewStatus,
  PreviewPolicy,
  PreviewMode,
  AppRemovalRecord,
  PersistedAppRemoval,
  ProjectDiscoveryCandidate,
  InstalledApp,
  ProjectDiscoveryReport,
  ProjectFootprintSummary,
  ProjectSummary,
  ProjectScanState,
  QuickOpenResult,
  RecentItem,
  RecoveryPending,
  RiskReport,
  ScanRoot,
  ScanStatus,
  SafeManageDecisionKind,
  SafeManageFirstRunPreference,
  SafeManageOperationPlanRequest,
  SafeManageProjectAssessment,
  SafeManageRegenerableTarget,
  SessionDiscoveryCandidate,
  SessionPreview,
  SecurityStatus,
  ShellIntegrationStatus,
  ShellOpenMode,
  ProcessResourceUsage,
  ProtectedZone,
  SystemResourceProfile,
  WatcherStatus
} from "./types";

const connectorFrontendBuild = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector";
const frontendEditionLabel = connectorFrontendBuild ? "AI Connector" : "Local";
const tutorialStorageKey = guidedTourStorageKey(connectorFrontendBuild ? "connector" : "local");
const EmptyConnectorView = () => null;

const SettingsAppearanceView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsAppearanceView })));
const OverviewView = lazy(() => import("./views/OverviewView").then((module) => ({ default: module.OverviewView })));
const EditionSettingsPanel = connectorFrontendBuild
  ? lazy(() => import("./views/ConnectorEditionLayer").then((module) => ({ default: module.ConnectorSettingsPanel })))
  : EmptyConnectorView;
const SettingsFoldersView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsFoldersView })));
const SettingsProtectionView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsProtectionView })));
const SettingsDiagnosticsExportCard = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsDiagnosticsExportCard })));
const ShellOpenModeDialog = lazy(() => import("./views/ShellOpenModeDialog"));
const RecoveryView = lazy(() => import("./views/RecoveryView").then((module) => ({ default: module.RecoveryView })));
const FinalRemoveReviewDialog = lazy(() => import("./views/FinalRemoveReviewDialog").then((module) => ({ default: module.FinalRemoveReviewDialog })));
const InspectorView = lazy(() => import("./views/InspectorView").then((module) => ({ default: module.InspectorView })));
const DiscoverSearchView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverSearchView })));
const DiscoverOrphansView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverOrphansView })));
const DiscoverDuplicatesView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverDuplicatesView })));
const DiscoverProjectDiscoveryView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverProjectDiscoveryView })));
const OrganizeView = lazy(() => import("./views/OrganizeView").then((module) => ({ default: module.OrganizeView })));
const EditionLayer = connectorFrontendBuild
  ? lazy(() => import("./views/ConnectorEditionLayer").then((module) => ({ default: module.ConnectorEditionLayer })))
  : EmptyConnectorView;
const EditionRecapDetailLayer = connectorFrontendBuild
  ? lazy(() => import("./views/ConnectorEditionLayer").then((module) => ({ default: module.ConnectorRecapDetailLayer })))
  : undefined;
const EditionSafeManageRecommendation = connectorFrontendBuild
  ? lazy(() => import("./views/ConnectorEditionLayer").then((module) => ({ default: module.ConnectorSafeManageRecommendation })))
  : undefined;
const ReviewImpactView = lazy(() => import("./views/ReviewImpactView").then((module) => ({ default: module.ReviewImpactView })));
const SafeManagePortfolioView = lazy(() => import("./views/SafeManagePortfolioView").then((module) => ({ default: module.SafeManagePortfolioView })));
const SafeManageFirstRunPrompt = lazy(() => import("./views/SafeManagePortfolioView").then((module) => ({ default: module.SafeManageFirstRunPrompt })));
const ConnectorGuidedTour = connectorFrontendBuild
  ? lazy(() => import("./views/ConnectorGuidedTour").then((module) => ({ default: module.ConnectorGuidedTour })))
  : EmptyConnectorView;

function ToolViewFallback() {
  return (
    <section className="pane-section compact">
      <p className="muted result-empty">Loading tool...</p>
    </section>
  );
}

interface OpenTab {
  nodeId: number;
  projectId: number;
  label: string;
  path: string;
}

interface OpenNodeOptions {
  replaceHistory?: boolean;
  allowProjectSwitch?: boolean;
  mode?: PreviewMode;
  recordRecent?: boolean;
  /** Known owner from a cross-project result. The backend validates this node/project pair. */
  projectId?: number | null;
  /** Refresh the preview content only (e.g. the watcher noticed a disk change):
   *  never switch primary/project/right-pane views or touch route history, so a
   *  background refresh can't yank the user out of the screen they are on. */
  refreshOnly?: boolean;
}

interface LostPreset {
  name: string;
  stalePreset: string;
  signals: string[];
  keyword: string;
  minPreset: string;
  customMiB: number;
  includePartial: boolean;
}

interface StartupProgress {
  active: boolean;
  label: string;
  detail: string;
  progress: number;
}

interface DuplicateSearchOverrides {
  scope?: "file" | "current" | "all";
  minPreset?: string;
  customMiB?: number;
  fileKind?: string;
  limit?: number;
  currentFileNodeId?: number | null;
}

type AppearanceFontSize = "compact" | "comfortable" | "large" | "xlarge";
type AppearanceDensity = "compact" | "comfortable" | "spacious";
type AppearanceContrast = "standard" | "high";
type ThemeMode = "light" | "oled";
type SessionInventoryState = "restoring" | "fresh" | "cached" | "unavailable";

const PANE_WIDTH_STORAGE_KEY = "codehangar:pane-widths";
const TREE_WIDTH_STORAGE_KEY = "codehangar:tree-pane-width";
const SIDEBAR_COLLAPSE_STORAGE_KEY = "codehangar:sidebar-collapse-v2";
const LOST_PRESETS_STORAGE_KEY = "codehangar:lost-project-presets";
const PERFORMANCE_MODE_STORAGE_KEY = "codehangar:performance-mode";
const SHOW_DEMO_PROJECTS_STORAGE_KEY = "codehangar:show-demo-projects";
const THEME_MODE_STORAGE_KEY = "codehangar:theme-mode";
const ADVANCED_MODE_STORAGE_KEY = "codehangar:advanced-mode";
const SHOW_PROJECT_PATHS_STORAGE_KEY = "codehangar:show-project-paths";
const SHOW_TOPBAR_NAV_STORAGE_KEY = "codehangar:show-topbar-nav-v4";
const PROJECT_SORT_STORAGE_KEY = "codehangar:project-sort";
const PROJECT_APP_FILTER_STORAGE_KEY = "codehangar:project-app-filter";
const PROJECT_STATUS_FILTER_STORAGE_KEY = "codehangar:project-status-filter";
const SESSION_SORT_STORAGE_KEY = "codehangar:session-sort";
const SESSION_APP_FILTER_STORAGE_KEY = "codehangar:session-app-filter";
const DISCOVERY_INCLUDE_LOOSE_STORAGE_KEY = "codehangar:discovery-include-loose";
const DISCOVERY_INCLUDE_AGENTS_STORAGE_KEY = "codehangar:discovery-include-agents";
const INVENTORY_INCLUDE_STORAGE_KEY = "codehangar:inventory-include";
const ARCHIVED_COLLAPSE_STORAGE_KEY = "codehangar:projects-archived-collapsed";
const PANE_COLLAPSE_STORAGE_KEY = "codehangar:pane-collapse";
const APPEARANCE_STORAGE_KEY = "codehangar:appearance";
const STARTUP_PREFERENCES_STORAGE_KEY = "codehangar:startup-preferences-v1";
const LAST_WORKSPACE_ROUTE_STORAGE_KEY = "codehangar:last-workspace-route-v1";
const DEPRECATED_PROJECT_CACHE_STORAGE_KEY = "codehangar:project-cache-v1";
const PROJECT_LIST_PREVIEW_LIMIT = 2;
const SESSION_GROUP_PREVIEW_LIMIT = 3;
const DEFAULT_LEFT_PANE_WIDTH = 286;
const DEFAULT_TREE_PANE_WIDTH = 388;
const DEFAULT_RIGHT_PANE_WIDTH = 318;
const COLLAPSED_PANE_WIDTH = 44;
const MIB = 1024 * 1024;
const GIB = 1024 * MIB;

type SessionPreviewLoadKind = "initial" | "more" | "full" | "reveal";

function yieldToUi() {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, 0);
  });
}

function delay(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function afterFirstPaint() {
  return new Promise<void>((resolve) => {
    let settled = false;
    const done = () => {
      if (settled) return;
      settled = true;
      resolve();
    };
    window.requestAnimationFrame(() => window.requestAnimationFrame(done));
    // WebView2 (Windows) pauses requestAnimationFrame whenever the window is occluded, minimized
    // or in the background. Without a fallback, a project clicked while the window is not in the
    // foreground would gate its load behind a paint that never happens — the spinner then hangs
    // forever with no backend call. Resolve after a short delay regardless so the load always runs.
    window.setTimeout(done, 250);
  });
}

const FINAL_REMOVE_PHASES: readonly FinalRemoveBatchProgress["phase"][] = [
  "waitingForUac",
  "verifyingArchives",
  "roundtrip",
  "parentDisposition",
  "deleting",
  "cleaningDirs",
  "stoppingAfterCurrentTopologyGroup",
  "finished",
  "interrupted"
];

const FINAL_REMOVE_RESULT_STATUSES: readonly FinalRemoveBatchResult["status"][] = [
  "completed",
  "partial",
  "cancelled",
  "failed",
  "interrupted"
];

const FINAL_REMOVE_ITEM_STATES = new Set([
  "planned",
  "archiveFinalizing",
  "archiveVerified",
  "blocked",
  "deleteIntent",
  "deleted",
  "deleteFailed",
  "kept",
  "reconciledDeleted"
]);

const FINAL_REMOVE_REASON_CODES = new Set([
  "archiveVerified",
  "legacyContentOnly",
  "archiveMissing",
  "archiveCorrupt",
  "unsupportedReparse",
  "unsupportedEfs",
  "unsupportedObjectStream",
  "externalHardlink",
  "nonNtfs",
  "cloudOrRecall",
  "locked",
  "identityChanged",
  "insufficientSpace",
  "permissionDenied",
  "helperUnsigned",
  "helperUntrusted",
  "releaseManifestMismatch",
  "uacCancelled",
  "capacityBlocked",
  "scratchCleanupPending",
  "stopRequested",
  "interrupted"
]);

function isSafeNonnegativeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function isFinalRemovePhase(value: unknown): value is FinalRemoveBatchProgress["phase"] {
  return FINAL_REMOVE_PHASES.includes(value as FinalRemoveBatchProgress["phase"]);
}

function isExplicitlyEligibleFinalRemoveObject(object: FinalRemoveObjectDecision): boolean {
  return object.eligibility === "ready" || object.eligibility === "needsArchiveV2";
}

function assertFinalRemovePreviewContract(preview: FinalRemoveBatchPreview): void {
  if (preview.archivesRetained !== true) {
    throw new Error("The final-cleanup backend returned an incompatible archive-retention contract. Held objects were not selected for deletion.");
  }
  if (!preview.previewId
    || !/^v2:[0-9a-f]{64}$/u.test(preview.previewDigest)
    || !Number.isFinite(Date.parse(preview.expiresAt))
    || !Number.isSafeInteger(preview.maxDeleteObjects)
    || preview.maxDeleteObjects < 0) {
    throw new Error("The final-cleanup backend returned an incomplete preview identity, expiry or object-capacity limit. Held objects were not selected for deletion.");
  }
  if (!Array.isArray(preview.projects)
    || !Array.isArray(preview.objects)
    || !Array.isArray(preview.volumes)
    || !Array.isArray(preview.eligibleTopologyGroupIds)) {
    throw new Error("The final-cleanup backend returned malformed project, object or volume collections. Held objects were not selected for deletion.");
  }
  const projectIds = preview.projects.map((project) => project?.groupId);
  const volumeIds = preview.volumes.map((volume) => volume?.volumeId);
  const entryIds = preview.objects.map((object) => object?.entryId);
  const projectIdSet = new Set(projectIds);
  const volumeIdSet = new Set(volumeIds);
  const projectsValid = preview.projects.every((project) => (
    project
    && typeof project.groupId === "string"
    && project.groupId.length > 0
    && typeof project.projectName === "string"
    && typeof project.originalRoot === "string"
    && isSafeNonnegativeInteger(project.totalObjects)
    && isSafeNonnegativeInteger(project.ready)
    && isSafeNonnegativeInteger(project.needsArchiveV2)
    && isSafeNonnegativeInteger(project.blocked)
    && project.ready + project.needsArchiveV2 + project.blocked === project.totalObjects
    && Array.isArray(project.blockedSubtrees)
    && project.blockedSubtrees.every((subtree) => (
      subtree
      && typeof subtree.root === "string"
      && isSafeNonnegativeInteger(subtree.count)
      && Array.isArray(subtree.reasonCodes)
      && subtree.reasonCodes.every((reasonCode) => FINAL_REMOVE_REASON_CODES.has(reasonCode))
    ))
  ));
  const volumesValid = preview.volumes.every((volume) => (
    volume
    && typeof volume.volumeId === "string"
    && volume.volumeId.length > 0
    && typeof volume.label === "string"
    && isSafeNonnegativeInteger(volume.alreadyFreedFromSourceBytes)
    && isSafeNonnegativeInteger(volume.heldAllocatedBytes)
    && isSafeNonnegativeInteger(volume.projectedReleaseBytes)
    && isSafeNonnegativeInteger(volume.archiveRetainedAllocatedBytes)
    && (volume.freeBytesBefore == null || isSafeNonnegativeInteger(volume.freeBytesBefore))
    && (volume.freeBytesAfter == null || isSafeNonnegativeInteger(volume.freeBytesAfter))
    && (volume.observedDeltaBytes == null || Number.isSafeInteger(volume.observedDeltaBytes))
    && ["exactObjectAllocation", "estimated", "observedNoisy"].includes(volume.quality)
  ));
  const objectsValid = preview.objects.every((object) => (
      !object
        ? false
        : isSafeNonnegativeInteger(object.entryId)
          && typeof object.groupId === "string"
          && projectIdSet.has(object.groupId)
          && typeof object.topologyGroupId === "string"
          && object.topologyGroupId.length > 0
          && typeof object.relativePath === "string"
          && ["file", "directory"].includes(object.kind)
          && ["held", "deleting", "deleted", "restored"].includes(object.lifecycle)
          && ["ready", "needsArchiveV2", "blocked"].includes(object.eligibility)
          && FINAL_REMOVE_REASON_CODES.has(object.reasonCode)
          && typeof object.reason === "string"
          && (object.remediation == null || typeof object.remediation === "string")
          && (object.archiveId == null || typeof object.archiveId === "string")
          && ["none", "contentOnlyLegacy", "pending", "objectCompleteVerified", "invalid"].includes(object.objectArchiveState)
          && typeof object.heldVolumeId === "string"
          && volumeIdSet.has(object.heldVolumeId)
          && typeof object.heldVolumeLabel === "string"
          && isSafeNonnegativeInteger(object.logicalBytes)
          && (object.allocatedBytes == null || isSafeNonnegativeInteger(object.allocatedBytes))
          && ["exactStreams", "logicalUpperBound", "unknown"].includes(object.measurement)
  ));
  if (!projectsValid
    || !volumesValid
    || !objectsValid
    || typeof preview.requiresElevation !== "boolean"
    || preview.requiresElevation !== preview.objects.some((object) => (
      object.lifecycle === "held" && object.eligibility === "needsArchiveV2"
    ))
    || !isSafeNonnegativeInteger(preview.blockedObjects)
    || preview.blockedObjects !== preview.objects.filter((object) => object.eligibility === "blocked").length
    || projectIdSet.size !== projectIds.length
    || volumeIdSet.size !== volumeIds.length
    || new Set(entryIds).size !== entryIds.length
    || preview.eligibleTopologyGroupIds.some((id) => typeof id !== "string" || id.length === 0)
    || new Set(preview.eligibleTopologyGroupIds).size !== preview.eligibleTopologyGroupIds.length) {
    throw new Error("The final-cleanup backend returned malformed object eligibility or topology data. Unknown eligibility is blocked; held objects were not selected for deletion.");
  }
}

function heldFinalRemoveMembersByTopology(objects: readonly FinalRemoveObjectDecision[]) {
  const membersByTopology = new Map<string, FinalRemoveObjectDecision[]>();
  for (const object of objects) {
    if (object.lifecycle !== "held") continue;
    const members = membersByTopology.get(object.topologyGroupId) ?? [];
    members.push(object);
    membersByTopology.set(object.topologyGroupId, members);
  }
  return membersByTopology;
}

function assertFinalRemoveBatchStatus(status: FinalRemoveBatchStatus, expectedBatchId: string): void {
  const progress = status.progress;
  if (!progress || !expectedBatchId || progress.batchId !== expectedBatchId) {
    throw new Error("Final-cleanup status was bound to a different batch identity.");
  }
  if (!isFinalRemovePhase(progress.phase)) {
    throw new Error("Final-cleanup status returned an unknown phase. Deletion state cannot be inferred; journal reconciliation is required.");
  }
  if (!Number.isSafeInteger(progress.total)
    || !Number.isSafeInteger(progress.completed)
    || progress.total < 0
    || progress.completed < 0
    || progress.completed > progress.total
    || (progress.currentPath != null && typeof progress.currentPath !== "string")) {
    throw new Error("Final-cleanup status returned invalid progress counts.");
  }
  if (progress.phase === "finished" && !status.result) {
    throw new Error("Final-cleanup status reported a finished phase without its required terminal result.");
  }
  if (progress.phase === "interrupted" && !status.result) {
    throw new Error("Final-cleanup status is interrupted without a terminal result and requires journal reconciliation.");
  }
  if (!status.result) return;
  const result = status.result;
  if (progress.phase !== "finished" && progress.phase !== "interrupted") {
    throw new Error("Final-cleanup status returned a terminal result during a non-terminal phase. Journal reconciliation is required.");
  }
  const counts = [result.requestedObjects, result.deletedObjects, result.keptObjects, result.failedObjects];
  const projectIds = Array.isArray(result.projects)
    ? result.projects.map((project) => project?.groupId)
    : [];
  const resultVolumeIds = Array.isArray(result.volumes)
    ? result.volumes.map((volume) => volume?.volumeId)
    : [];
  const resultEntryIds = Array.isArray(result.items)
    ? result.items.map((item) => item?.entryId)
    : [];
  const projectRowsValid = Array.isArray(result.projects) && result.projects.every((project) => (
    project
    && typeof project.groupId === "string"
    && project.groupId.length > 0
    && isSafeNonnegativeInteger(project.deleted)
    && isSafeNonnegativeInteger(project.kept)
    && isSafeNonnegativeInteger(project.failed)
  ));
  const projectTotalsMatch = !projectRowsValid || result.projects.length === 0 || (
    result.projects.reduce((total, project) => total + project.deleted, 0) === result.deletedObjects
    && result.projects.reduce((total, project) => total + project.kept, 0) === result.keptObjects
    && result.projects.reduce((total, project) => total + project.failed, 0) === result.failedObjects
  );
  const volumeRowsValid = Array.isArray(result.volumes) && result.volumes.every((volume) => (
    volume
    && typeof volume.volumeId === "string"
    && volume.volumeId.length > 0
    && typeof volume.label === "string"
    && isSafeNonnegativeInteger(volume.alreadyFreedFromSourceBytes)
    && isSafeNonnegativeInteger(volume.heldAllocatedBytes)
    && isSafeNonnegativeInteger(volume.projectedReleaseBytes)
    && isSafeNonnegativeInteger(volume.archiveRetainedAllocatedBytes)
    && (volume.freeBytesBefore == null || isSafeNonnegativeInteger(volume.freeBytesBefore))
    && (volume.freeBytesAfter == null || isSafeNonnegativeInteger(volume.freeBytesAfter))
    && (volume.observedDeltaBytes == null || Number.isSafeInteger(volume.observedDeltaBytes))
    && ["exactObjectAllocation", "estimated", "observedNoisy"].includes(volume.quality)
  ));
  if (result.batchId !== expectedBatchId
    || result.archiveRetained !== true
    || !FINAL_REMOVE_RESULT_STATUSES.includes(result.status)
    || !Array.isArray(result.projects)
    || !Array.isArray(result.volumes)
    || !Array.isArray(result.items)
    || !projectRowsValid
    || new Set(projectIds).size !== projectIds.length
    || new Set(resultVolumeIds).size !== resultVolumeIds.length
    || new Set(resultEntryIds).size !== resultEntryIds.length
    || !projectTotalsMatch
    || !volumeRowsValid
    || result.items.some((item) => (
      !item
      || !isSafeNonnegativeInteger(item.entryId)
      || !FINAL_REMOVE_ITEM_STATES.has(item.state)
      || (item.reasonCode != null && !FINAL_REMOVE_REASON_CODES.has(item.reasonCode))
      || (item.error != null && typeof item.error !== "string")
    ))
    || counts.some((count) => !Number.isSafeInteger(count) || count < 0)
    || result.deletedObjects + result.keptObjects + result.failedObjects !== result.requestedObjects
    || progress.total !== result.requestedObjects) {
    throw new Error("Final-cleanup result identity, archive proof or object counts are inconsistent.");
  }
}

function waitForResidentUiActivation(signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    let settled = false;
    let unlisten: (() => void) | undefined;

    const cleanup = () => {
      window.removeEventListener("focus", handleWindowActivation);
      document.removeEventListener("visibilitychange", handleWindowActivation);
      signal.removeEventListener("abort", finish);
      unlisten?.();
    };
    const finish = () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve();
    };
    const handleWindowActivation = () => {
      void api.residentWindowVisible()
        .then((visible) => {
          if (visible) finish();
        })
        .catch(() => undefined);
    };

    window.addEventListener("focus", handleWindowActivation);
    document.addEventListener("visibilitychange", handleWindowActivation);
    signal.addEventListener("abort", finish, { once: true });
    void listen("resident-window-shown", () => finish())
      .then((release) => {
        if (settled) release();
        else unlisten = release;
      })
      .catch(() => undefined);

    // Close the small race between the startup commands and listener registration.
    // WebView2 may report DOM visibility/focus while its native Tauri window is
    // hidden, so only the backend's actual window state can release this gate.
    if (signal.aborted) finish();
    else handleWindowActivation();
  });
}

function initialPaneWidths() {
  if (typeof window === "undefined") {
    return { left: DEFAULT_LEFT_PANE_WIDTH, right: DEFAULT_RIGHT_PANE_WIDTH };
  }
  try {
    const stored = window.localStorage.getItem(PANE_WIDTH_STORAGE_KEY);
    if (!stored) return { left: DEFAULT_LEFT_PANE_WIDTH, right: DEFAULT_RIGHT_PANE_WIDTH };
    const parsed = JSON.parse(stored) as { left?: number; right?: number };
    return {
      left: clamp(parsed.left ?? DEFAULT_LEFT_PANE_WIDTH, 176, 460),
      right: clamp(parsed.right ?? DEFAULT_RIGHT_PANE_WIDTH, 190, 560)
    };
  } catch {
    return { left: DEFAULT_LEFT_PANE_WIDTH, right: DEFAULT_RIGHT_PANE_WIDTH };
  }
}

function initialPaneCollapse() {
  if (typeof window === "undefined") return startupPaneCollapse();
  try {
    const stored = window.localStorage.getItem(PANE_COLLAPSE_STORAGE_KEY);
    const preferences = initialStartupPreferences();
    if (!stored) return startupPaneCollapse(undefined, preferences);
    const parsed = JSON.parse(stored) as { left?: boolean; right?: boolean };
    return startupPaneCollapse(parsed, preferences);
  } catch {
    return startupPaneCollapse(undefined, initialStartupPreferences());
  }
}

function initialStartupPreferences(): StartupPreferences {
  if (typeof window === "undefined") return normalizeStartupPreferences(null);
  try {
    return normalizeStartupPreferences(JSON.parse(window.localStorage.getItem(STARTUP_PREFERENCES_STORAGE_KEY) ?? "null"));
  } catch {
    return normalizeStartupPreferences(null);
  }
}

function initialStoredWorkspaceRoute(): WorkspaceRoute | null {
  if (typeof window === "undefined") return null;
  try {
    return parseWorkspaceRoute(JSON.parse(window.localStorage.getItem(LAST_WORKSPACE_ROUTE_STORAGE_KEY) ?? "null"));
  } catch {
    return null;
  }
}

function initialAppearance(): {
  fontSize: AppearanceFontSize;
  density: AppearanceDensity;
  contrast: AppearanceContrast;
  reduceMotion: boolean;
} {
  const fallback: {
    fontSize: AppearanceFontSize;
    density: AppearanceDensity;
    contrast: AppearanceContrast;
    reduceMotion: boolean;
  } = {
    fontSize: "comfortable" as AppearanceFontSize,
    density: "comfortable" as AppearanceDensity,
    contrast: "standard" as AppearanceContrast,
    reduceMotion: false
  };
  if (typeof window === "undefined") return fallback;
  try {
    const parsed = JSON.parse(window.localStorage.getItem(APPEARANCE_STORAGE_KEY) ?? "{}") as Partial<typeof fallback>;
    return {
      fontSize: isAppearanceFontSize(parsed.fontSize) ? parsed.fontSize : fallback.fontSize,
      density: isAppearanceDensity(parsed.density) ? parsed.density : fallback.density,
      contrast: parsed.contrast === "high" ? "high" : fallback.contrast,
      reduceMotion: Boolean(parsed.reduceMotion)
    };
  } catch {
    return fallback;
  }
}

function isAppearanceFontSize(value: unknown): value is AppearanceFontSize {
  return value === "compact" || value === "comfortable" || value === "large" || value === "xlarge";
}

function isAppearanceDensity(value: unknown): value is AppearanceDensity {
  return value === "compact" || value === "comfortable" || value === "spacious";
}

function initialTreePaneWidth() {
  if (typeof window === "undefined") {
    return DEFAULT_TREE_PANE_WIDTH;
  }
  // No stored value must yield the default: Number(null) is 0, which would clamp
  // to the 300px minimum and silently kill the wider default.
  const raw = window.localStorage.getItem(TREE_WIDTH_STORAGE_KEY);
  if (!raw) return DEFAULT_TREE_PANE_WIDTH;
  const stored = Number(raw);
  return Number.isFinite(stored) ? clamp(stored, 300, 720) : DEFAULT_TREE_PANE_WIDTH;
}

function initialSidebarCollapse() {
  // Only Projects is expanded by default; everything else starts collapsed so the
  // sidebar opens tidy. Once the user toggles a section the choice is persisted.
  const defaults = { projects: false, sessions: true, pinned: true, recent: true };
  if (typeof window === "undefined") {
    return defaults;
  }
  try {
    const stored = window.localStorage.getItem(SIDEBAR_COLLAPSE_STORAGE_KEY);
    if (!stored) return defaults;
    const parsed = JSON.parse(stored) as { projects?: boolean; sessions?: boolean; pinned?: boolean; recent?: boolean };
    return { projects: Boolean(parsed.projects), sessions: Boolean(parsed.sessions), pinned: Boolean(parsed.pinned), recent: Boolean(parsed.recent) };
  } catch {
    return defaults;
  }
}

function initialDemoProjectPreference() {
  if (typeof window === "undefined") return null;
  const stored = window.localStorage.getItem(SHOW_DEMO_PROJECTS_STORAGE_KEY);
  if (stored === "true") return true;
  if (stored === "false") return false;
  return null;
}

function initialPerformanceMode(): PerformanceMode {
  if (typeof window === "undefined") return "priority";
  const stored = window.localStorage.getItem(PERFORMANCE_MODE_STORAGE_KEY);
  if (stored === "boost" || stored === "priority") return "priority";
  if (stored === "max") return "max";
  if (stored === "balanced") return "balanced";
  return "priority";
}

function initialThemeMode(): ThemeMode {
  if (typeof window === "undefined") return "light";
  return window.localStorage.getItem(THEME_MODE_STORAGE_KEY) === "oled" ? "oled" : "light";
}

function initialAdvancedMode(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(ADVANCED_MODE_STORAGE_KEY) === "true";
}

function initialShowAllProjectPaths(): boolean {
  if (typeof window === "undefined") return true;
  // Paths are shown for every project by default now; only an explicit opt-out hides them.
  return window.localStorage.getItem(SHOW_PROJECT_PATHS_STORAGE_KEY) !== "false";
}

function initialShowTopbarNav(): boolean {
  if (typeof window === "undefined") return false;
  return storedBooleanPreference(window.localStorage.getItem(SHOW_TOPBAR_NAV_STORAGE_KEY), false);
}

function readStored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  if (typeof window === "undefined") return fallback;
  const value = window.localStorage.getItem(key);
  return value && (allowed as readonly string[]).includes(value) ? (value as T) : fallback;
}

function initialProjectSort(): ProjectSort {
  return readStored(PROJECT_SORT_STORAGE_KEY, ["name", "size", "recent"] as const, "name");
}

function initialProjectStatusFilter(): ProjectStatusFilter {
  return readStored(PROJECT_STATUS_FILTER_STORAGE_KEY, ["all", "ready", "scanning", "needs-scan"] as const, "all");
}

function initialSessionSort(): SessionSort {
  return readStored(SESSION_SORT_STORAGE_KEY, ["recent", "name"] as const, "recent");
}

function initialProjectAppFilter(): string {
  if (typeof window === "undefined") return "all";
  return window.localStorage.getItem(PROJECT_APP_FILTER_STORAGE_KEY) ?? "all";
}

function initialSessionAppFilter(): string {
  if (typeof window === "undefined") return "all";
  return window.localStorage.getItem(SESSION_APP_FILTER_STORAGE_KEY) ?? "all";
}

function initialDiscoveryIncludeLoose(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(DISCOVERY_INCLUDE_LOOSE_STORAGE_KEY) === "true";
}

function initialDiscoveryIncludeAgents(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(DISCOVERY_INCLUDE_AGENTS_STORAGE_KEY) === "true";
}

// The include-options (loose conversations, autonomous agents) that produced the
// CURRENT session inventory. The startup background rediscovery replays these
// instead of the Deep Scan checkbox defaults, so Hermes/Independent groups found
// by "Find Sessions" don't silently vanish on the next launch.
function readInventoryIncludeOptions(): { loose: boolean; agents: boolean } | null {
  if (typeof window === "undefined") return null;
  try {
    const stored = window.localStorage.getItem(INVENTORY_INCLUDE_STORAGE_KEY);
    if (!stored) return null;
    const parsed = JSON.parse(stored) as { loose?: boolean; agents?: boolean };
    return { loose: Boolean(parsed.loose), agents: Boolean(parsed.agents) };
  } catch {
    return null;
  }
}

function persistInventoryIncludeOptions(loose: boolean, agents: boolean) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(INVENTORY_INCLUDE_STORAGE_KEY, JSON.stringify({ loose, agents }));
}

function initialArchivedCollapsed(): boolean {
  if (typeof window === "undefined") return true;
  // Archived projects start collapsed; only honour an explicit stored "false".
  return window.localStorage.getItem(ARCHIVED_COLLAPSE_STORAGE_KEY) !== "false";
}

// The last discovery report, cached locally so the session grouping and
// Active/Archived split paint instantly on the next launch instead of waiting
// out the (cold-WSL) background rediscovery. Project ids are stable across
// restarts, so cached session→project links stay valid; a fresh discovery
// overwrites this a moment after startup, and Reset clears it.
async function loadCachedDiscoveryReport(): Promise<ProjectDiscoveryReport | null> {
  try {
    // Read the DPAPI-protected snapshot from the backend — never plaintext UI storage.
    const raw = await api.readDiscoverySnapshot();
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { savedAt?: number; report?: ProjectDiscoveryReport };
    if (
      typeof parsed.savedAt !== "number"
      || !Number.isFinite(parsed.savedAt)
      || parsed.savedAt < 0
      || parsed.savedAt > Date.now()
    ) {
      return null;
    }
    return parsed?.report ?? null;
  } catch {
    return null;
  }
}

function normalizeProjectPath(value: string) {
  return value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
}

// THE session→project matcher, shared by the sidebar session groups, the project
// workspace's Sessions tab and the open-session pane so they can never disagree:
// registered-id link first, then bidirectional slash-normalized path containment.
// The path fallback matters right after a Deep Scan, when the registered-id link
// is stale (discovery ran before the projects were registered).
function findSessionProject(
  session: SessionDiscoveryCandidate,
  projectById: Map<number, ProjectSummary>,
  projectByPath: { project: ProjectSummary; path: string }[]
): ProjectSummary | undefined {
  const linkedId = session.linkedRegisteredProjectIds.find((id) => projectById.has(id));
  if (linkedId != null) return projectById.get(linkedId);
  for (const linked of session.linkedProjectPaths) {
    const p = normalizeProjectPath(linked);
    const hit = projectByPath.find(({ path }) => p === path || p.startsWith(`${path}\\`) || path.startsWith(`${p}\\`));
    if (hit) return hit.project;
  }
  return undefined;
}

// A small colored stamp identifying the owning AI app of a project or session.
function AppBadge({ meta, suffix }: { meta: AppMeta; suffix?: string }) {
  return (
    <span className={`app-badge app-badge--${meta.slug}`} title={`${meta.label}${suffix ?? ""}`}>
      {meta.label}
    </span>
  );
}

// Every app a project belongs to, as compact chips. A folder used in several tools (e.g.
// a project worked on in both Claude and ChatGPT) shows a chip for each, so the "Claude" tag
// is always visible instead of being hidden behind whichever app the badge would otherwise
// pick. The most specific owner leads; the rest follow in stable order.
function AppBadges({ metas, suffix }: { metas: AppMeta[]; suffix?: string }) {
  if (metas.length === 0) return null;
  return (
    <span className="app-badges">
      {metas.map((meta) => (
        <span
          key={meta.slug}
          className={`app-badge app-badge--${meta.slug}`}
          title={`${meta.label}${suffix ?? ""}`}
        >
          {meta.label}
        </span>
      ))}
    </span>
  );
}

// The primary navigation (Overview → Settings), shared by the sidebar nav, the
// CH-logo flyout (reachable when the sidebar is collapsed or scrolled) and the
// optional compact icon row in the top bar. `iconsOnly` drops the labels.
function PrimaryNavButtons({
  primaryView,
  iconsOnly,
  disabled,
  onOverview,
  onDiscover,
  onSafeManage,
  onRecovery,
  onSettings
}: {
  primaryView: string;
  iconsOnly?: boolean;
  disabled?: boolean;
  onOverview: () => void;
  onDiscover: () => void;
  onSafeManage: () => void;
  onRecovery: () => void;
  onSettings: () => void;
}) {
  // These are portfolio-wide destinations. The project-scoped Safe Manage review
  // remains available only after a project decision has selected a concrete target.
  return (
    <>
      <button disabled={disabled} className={primaryView === "overview" ? "active" : ""} type="button" onClick={onOverview} aria-label="Overview" data-help={disabled ? "Exit Viewer to open the complete project overview." : "Open a clear summary of the local inventory, scan health and largest project footprints."}>
        <Home size={16} />{iconsOnly ? null : <span>Overview</span>}
      </button>
      <button disabled={disabled} className={primaryView === "discover" ? "active" : ""} type="button" onClick={onDiscover} aria-label="Discover" data-help={disabled ? "Exit Viewer to search across the complete catalog." : "Search local content and find forgotten projects, unreferenced files or duplicate candidates. Discovery never changes files."}>
        <Compass size={16} />{iconsOnly ? null : <span>Discover</span>}
      </button>
      <button disabled={disabled} className={primaryView === "safe_manage" || primaryView === "review" ? "active" : ""} type="button" onClick={onSafeManage} aria-label="Safe Manage" data-help={disabled ? "Exit Viewer to open Safe Manage." : "Analyze the whole local project portfolio, review evidence, and decide what should be kept, archived, cleaned or prepared for removal."}>
        <ListChecks size={16} />{iconsOnly ? null : <span>Safe Manage</span>}
      </button>
      <button disabled={disabled} className={primaryView === "recovery" ? "active" : ""} type="button" onClick={onRecovery} aria-label="Recovery and cleanup" data-help={disabled ? "Exit Viewer to open Recovery & cleanup." : "Review held projects, eligible final cleanup, recovery archives and local disk-action history."}>
        <ArchiveRestore size={16} />{iconsOnly ? null : <span>Recovery &amp; cleanup</span>}
      </button>
      <button disabled={disabled} className={primaryView === "settings" ? "active" : ""} type="button" onClick={onSettings} aria-label="Settings" data-help={disabled ? "Exit Viewer to change application settings." : "Manage scan folders, protected locations and advanced local-only details."}>
        <Settings size={16} />{iconsOnly ? null : <span>Settings</span>}
      </button>
    </>
  );
}

function NavigationFlyout({
  primaryView,
  disabled,
  onOverview,
  onDiscover,
  onSafeManage,
  onRecovery,
  onSettings
}: {
  primaryView: string;
  disabled?: boolean;
  onOverview: () => void;
  onDiscover: () => void;
  onSafeManage: () => void;
  onRecovery: () => void;
  onSettings: () => void;
}) {
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const close = useCallback((restoreFocus = false) => {
    setOpen(false);
    if (restoreFocus) triggerRef.current?.focus();
  }, []);

  const focusFirstDestination = useCallback(() => {
    window.requestAnimationFrame(() => {
      containerRef.current
        ?.querySelector<HTMLButtonElement>('.brand-flyout button:not(:disabled)')
        ?.focus();
    });
  }, []);

  useEffect(() => {
    if (!open) return;
    const handleOutsidePointer = (event: PointerEvent) => {
      if (event.target instanceof Node && !containerRef.current?.contains(event.target)) {
        close();
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close(true);
    };
    document.addEventListener("pointerdown", handleOutsidePointer);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("pointerdown", handleOutsidePointer);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [close, open]);

  const choose = useCallback((destination: () => void) => () => {
    close(true);
    destination();
  }, [close]);

  return (
    <div className="brand-menu" ref={containerRef}>
      <button
        ref={triggerRef}
        className="brand-mark"
        type="button"
        aria-label={open ? "Close navigation menu" : "Open navigation menu"}
        aria-expanded={open}
        aria-controls="primary-navigation-flyout"
        data-help="Open Overview, Discover, Safe Manage, Recovery & cleanup or Settings when the sidebar is unavailable."
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
            focusFirstDestination();
          } else if (event.key === "Escape" && open) {
            event.preventDefault();
            close(true);
          }
        }}
      >
        CH
      </button>
      <nav
        id="primary-navigation-flyout"
        className="brand-flyout"
        aria-label="Main navigation menu"
        hidden={!open}
      >
        <PrimaryNavButtons
          primaryView={primaryView}
          disabled={disabled}
          onOverview={choose(onOverview)}
          onDiscover={choose(onDiscover)}
          onSafeManage={choose(onSafeManage)}
          onRecovery={choose(onRecovery)}
          onSettings={choose(onSettings)}
        />
      </nav>
    </div>
  );
}

type ShellOpenChoice = Exclude<ShellOpenMode, "known"> | "cancel";

interface ShellViewerSession {
  rootId: number;
  project: ProjectSummary;
  inputPath: string;
  scanJobId?: string | null;
  temporary: boolean;
  ready: boolean;
}

interface PendingShellOpenRequest {
  sequence: number;
  path: string;
  directFileOpen: boolean;
  previewSelectionSequence: number;
  intentGeneration: number;
  fullPreviewPromise: Promise<void> | null;
}

interface ShellOpenChoiceRequest {
  sequence: number;
  resolve: (choice: ShellOpenChoice) => void;
}

export function App() {
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [projectsFromCache, setProjectsFromCache] = useState(false);
  const [inventoryReady, setInventoryReady] = useState(false);
  const [shellOpenRevision, setShellOpenRevision] = useState(0);
  const shellOpenProcessingRef = useRef(false);
  const shellOpenDrainPromiseRef = useRef<Promise<void> | null>(null);
  const shellOpenReconcileTailRef = useRef<Promise<void>>(Promise.resolve());
  const shellOpenRequestSequenceRef = useRef(0);
  const shellOpenLatestRequestSequenceRef = useRef(0);
  const shellOpenIntentGenerationRef = useRef(0);
  const shellViewerCloseGenerationRef = useRef(0);
  const shellViewerRetirementRef = useRef<ShellViewerSession | null>(null);
  const shellViewerDisposalsRef = useRef<Map<number, Promise<void>>>(new Map());
  const shellOpenRerunRef = useRef(false);
  // Windows file associations are explicit read requests. Once one arrives it
  // owns the startup route; slower catalog hydration must never cover the file
  // with the tutorial, Overview, or an older project snapshot.
  const shellOpenHasPriorityRef = useRef(false);
  const shellScanWaitersRef = useRef<Map<string, Promise<ScanStatus>>>(new Map());
  const shellDiscoveryRefreshJobsRef = useRef<Set<string>>(new Set());
  const transientShellNodeIdsRef = useRef<Map<string, number>>(new Map());
  const nextTransientShellNodeIdRef = useRef(-1);
  const shellOpenChoiceResolverRef = useRef<ShellOpenChoiceRequest | null>(null);
  const [shellOpenChoice, setShellOpenChoice] = useState<OpenTargetInspection | null>(null);
  const [shellViewer, setShellViewer] = useState<ShellViewerSession | null>(null);
  const shellViewerRef = useRef<ShellViewerSession | null>(null);
  const [shellViewerClosing, setShellViewerClosing] = useState(false);
  const [shellIntegration, setShellIntegration] = useState<ShellIntegrationStatus | null>(null);
  const [shellIntegrationBusy, setShellIntegrationBusy] = useState(false);
  const [shellIntegrationError, setShellIntegrationError] = useState<string | null>(null);
  const [selectedProjectId, setSelectedProjectId] = useState<number | null>(null);
  const selectedProjectIdRef = useRef<number | null>(null);
  const {
    state: projectWorkspace,
    beginProject,
    loadProjectData,
    loadTreeChildren: loadProjectTreeChildren,
    toggleExpanded: toggleExpandedTree,
    revealNode: revealProjectNode
  } = useProjectWorkspace();
  const { treePages, expandedTree, treeLoading, contextFiles, gitStatus } = projectWorkspace;
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [folderExplanation, setFolderExplanation] = useState<FolderExplanation | null>(null);
  const [relationships, setRelationships] = useState<NodeRelationships | null>(null);
  const [relationshipsMembership, setRelationshipsMembership] = useState<string | null>(null);
  const [relationshipsLoading, setRelationshipsLoading] = useState(false);
  const [graphMap, setGraphMap] = useState<GraphMap | null>(null);
  const [graphMapLoading, setGraphMapLoading] = useState(false);
  const [graphMapError, setGraphMapError] = useState<string | null>(null);
  const [graphMapRevision, setGraphMapRevision] = useState(0);
  const [graphMapExpansion, setGraphMapExpansion] = useState<GraphMapExpansionState>({
    status: "idle",
    loadedItems: 0,
    totalItems: 0,
    message: null
  });
  const [previewMode, setPreviewMode] = useState<PreviewMode>("rendered");
  // Edit mode (Local/Connector editions): an in-memory editable buffer for the current file.
  // `editDraft` is null when not editing; it is seeded from the file's source on entering Edit.
  // `editUndo` holds the pre-save content so a single Save can be reverted on disk.
  const [editDraft, setEditDraft] = useState<string | null>(null);
  const [editSaving, setEditSaving] = useState(false);
  const [editUndo, setEditUndo] = useState<{ nodeId: number; previous: string; applied: string } | null>(null);
  const editDraftNodeRef = useRef<number | null>(null);
  const [tabs, setTabs] = useState<OpenTab[]>([]);
  const [draggedTabNodeId, setDraggedTabNodeId] = useState<number | null>(null);
  const [tabDropTargetNodeId, setTabDropTargetNodeId] = useState<number | null>(null);
  const [backStack, setBackStack] = useState<FileHistoryEntry[]>([]);
  const [forwardStack, setForwardStack] = useState<FileHistoryEntry[]>([]);
  const [quickOpenVisible, setQuickOpenVisible] = useState(false);
  const quickOpenButtonRef = useRef<HTMLButtonElement>(null);
  const quickOpenReturnFocusRef = useRef<HTMLElement | null>(null);
  const [commandVisible, setCommandVisible] = useState(false);
  const commandButtonRef = useRef<HTMLButtonElement>(null);
  const commandReturnFocusRef = useRef<HTMLElement | null>(null);
  const [addProjectsVisible, setAddProjectsVisible] = useState(false);
  // First-run onboarding hands off to Deep Scan; a later replay is read-only and
  // returns to the exact route/history it started from.
  const [tourMode, setTourMode] = useState<GuidedTourMode | null>(null);
  const tourActive = tourMode !== null;
  const [deepScanIncludeLoose, setDeepScanIncludeLoose] = useState(initialDiscoveryIncludeLoose);
  const [deepScanIncludeAgents, setDeepScanIncludeAgents] = useState(initialDiscoveryIncludeAgents);
  // Which AI tools were detected on this PC (drives the Deep Scan UI so it lists
  // only what's actually installed), and whether the user has opted into scanning
  // WSL (off by default — the app never touches wsl.exe unless this is on).
  const [installedApps, setInstalledApps] = useState<InstalledApp[]>([]);
  const [installedAppsLoading, setInstalledAppsLoading] = useState(true);
  const [installedAppsError, setInstalledAppsError] = useState<string | null>(null);
  const [wslScanChoice, setWslScanChoice] = useState(false);
  const [wslPreferencePending, setWslPreferencePending] = useState(false);
  const [wslPreferenceError, setWslPreferenceError] = useState<string | null>(null);
  const wslConfirmedChoiceRef = useRef(false);
  const wslRequestedChoiceRef = useRef(false);
  const wslPreferenceSequenceRef = useRef(0);
  const wslPreferenceApplyRef = useRef<Promise<boolean> | null>(null);
  const [deepScanProgress, setDeepScanProgress] = useState<DeepScanProgress | null>(null);
  const [deepScanOverlayVisible, setDeepScanOverlayVisible] = useState(false);
  const [resetAllVisible, setResetAllVisible] = useState(false);
  const [removeProjectTarget, setRemoveProjectTarget] = useState<ProjectSummary | null>(null);
  const editionBridgeRef = useRef<ConnectorEditionBridge | null>(null);
  const [editionOverlayOpen, setEditionOverlayOpen] = useState(false);
  const setEditionBridge = useCallback((bridge: ConnectorEditionBridge | null) => {
    editionBridgeRef.current = bridge;
  }, []);
  const [unlockedChangeProjectId, setUnlockedChangeProjectId] = useState<number | null>(null);
  const [changeUnlockTarget, setChangeUnlockTarget] = useState<ProjectSummary | null>(null);
  // When a unified "Remove project" also asks to forget from Code Hangar AND delete the
  // folder, we defer the Code Hangar unregister until the disk move actually completes
  // (the disk flow needs the live project node to build its plan). A scan-root project
  // unregisters by root; an orphan project unregisters by node.
  const pendingPostMoveUnregister = useRef<{ rootId: number | null; projectId: number } | null>(null);
  const [previewSession, setPreviewSession] = useState<SessionDiscoveryCandidate | null>(null);
  const [sessionPreview, setSessionPreview] = useState<SessionPreview | null>(null);
  const [sessionPreviewLoading, setSessionPreviewLoading] = useState(false);
  const [sessionPreviewLoadKind, setSessionPreviewLoadKind] = useState<SessionPreviewLoadKind | null>(null);
  const [sessionPreviewError, setSessionPreviewError] = useState<string | null>(null);
  const [sessionRevealing, setSessionRevealing] = useState(false);
  const [quickQuery, setQuickQuery] = useState("");
  const [quickResults, setQuickResults] = useState<QuickOpenResult[]>([]);
  const [quickSearchStatus, setQuickSearchStatus] = useState<QuickOpenSearchStatus>("idle");
  const [documentQuery, setDocumentQuery] = useState("");
  const [documentScope, setDocumentScope] = useState<DocumentSearchScope>("current");
  const [documentProjectInput, setDocumentProjectInput] = useState("");
  const [documentKind, setDocumentKind] = useState<"context" | "markdown" | "all">("context");
  const [documentPathFilter, setDocumentPathFilter] = useState("");
  const [documentNameFilter, setDocumentNameFilter] = useState("");
  const [documentLimit, setDocumentLimit] = useState(10);
  const [documentHits, setDocumentHits] = useState<DocumentHit[]>([]);
  const [documentSearchTruncated, setDocumentSearchTruncated] = useState(false);
  const [documentSearchDuration, setDocumentSearchDuration] = useState<number | null>(null);
  const [documentSearching, setDocumentSearching] = useState(false);
  const [documentSearchRan, setDocumentSearchRan] = useState(false);
  const [documentSearchError, setDocumentSearchError] = useState<string | null>(null);
  const [documentSearchCriteria, setDocumentSearchCriteria] = useState<string | null>(null);
  const [recentItems, setRecentItems] = useState<RecentItem[]>([]);
  const [pinnedItems, setPinnedItems] = useState<PinnedItem[]>([]);
  const [roots, setRoots] = useState<ScanRoot[]>([]);
  const [zones, setZones] = useState<ProtectedZone[]>([]);
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  const [watcherStatus, setWatcherStatus] = useState<WatcherStatus | null>(null);
  const [resourceProfile, setResourceProfile] = useState<SystemResourceProfile | null>(null);
  const [resourceProfileLoading, setResourceProfileLoading] = useState(false);
  const [resourceProfileError, setResourceProfileError] = useState<string | null>(null);
  const [recoveryState, setRecoveryState] = useState<RecoveryPending | null>(null);
  const [recoveryFrozen, setRecoveryFrozen] = useState(false);
  const [recoveryResolving, setRecoveryResolving] = useState<"rollback" | null>(null);
  const [dashboard, setDashboard] = useState<DashboardSummary | null>(null);
  const [adapters, setAdapters] = useState<AdapterSummary[]>([]);
  const [dashboardLoading, setDashboardLoading] = useState(false);
  const [orphanCandidates, setOrphanCandidates] = useState<OrphanCandidates | null>(null);
  const [orphanStatusByMembership, setOrphanStatusByMembership] = useState<Record<string, OrphanStatus>>({});
  const [lostProjectCandidates, setLostProjectCandidates] = useState<LostProjectCandidates | null>(null);
  const [projectDiscoveryReport, setProjectDiscoveryReport] = useState<ProjectDiscoveryReport | null>(null);
  // The left-sidebar Sessions list is its own inventory, populated only by
  // "Find Sessions". Project discovery (Find Projects / Deep Scan) never writes
  // here, so it can't bleed into the sidebar — it only fills each project's own
  // Sessions tab (selectedProjectSessions, from the project report).
  const [sessionInventory, setSessionInventory] = useState<SessionDiscoveryCandidate[]>([]);
  const [sessionInventoryState, setSessionInventoryState] = useState<SessionInventoryState>("restoring");
  const [sessionInventoryRefreshing, setSessionInventoryRefreshing] = useState(true);
  const [sessionInventoryError, setSessionInventoryError] = useState<string | null>(null);
  const [sessionTitleOverrides, setSessionTitleOverrides] = useState<Record<string, string>>({});
  const [projectDiscoveryLoading, setProjectDiscoveryLoading] = useState(false);
  const [projectDiscoveryError, setProjectDiscoveryError] = useState<string | null>(null);
  const [safeManageFirstRunPreference, setSafeManageFirstRunPreference] = useState<SafeManageFirstRunPreference | null>(null);
  const [safeManageFirstRunOpen, setSafeManageFirstRunOpen] = useState(false);
  const safeManagePromptShownRef = useRef(false);
  const [orphanMode, setOrphanMode] = useState<"lost" | "assets">("lost");
  const [orphanLoading, setOrphanLoading] = useState(false);
  const [orphanSearchError, setOrphanSearchError] = useState<string | null>(null);
  const [lostSearchCriteria, setLostSearchCriteria] = useState<string | null>(null);
  const [assetSearchCriteria, setAssetSearchCriteria] = useState<string | null>(null);
  const [orphanActiveSearchCriteria, setOrphanActiveSearchCriteria] = useState<string | null>(null);
  const [fileOrphanLoading, setFileOrphanLoading] = useState(false);
  const [orphanScope, setOrphanScope] = useState<"current" | "all">("current");
  const [orphanAutoRunSeq, setOrphanAutoRunSeq] = useState(0);
  const [orphanMinPreset, setOrphanMinPreset] = useState("100m");
  const [orphanCustomMiB, setOrphanCustomMiB] = useState(100);
  const [orphanAssetKind, setOrphanAssetKind] = useState("all");
  const [orphanMinConfidence, setOrphanMinConfidence] = useState("Low");
  const [orphanIncludePartial, setOrphanIncludePartial] = useState(false);
  const [lostStalePreset, setLostStalePreset] = useState("any");
  const [lostSignals, setLostSignals] = useState<string[]>([]);
  const [lostKeyword, setLostKeyword] = useState("");
  const [savedLostPresets, setSavedLostPresets] = useState<LostPreset[]>(loadSavedLostPresets);
  const [lostPresetName, setLostPresetName] = useState("");
  const [duplicateCandidates, setDuplicateCandidates] = useState<DuplicateCandidates | null>(null);
  const [duplicateLoading, setDuplicateLoading] = useState(false);
  const [duplicateHasRun, setDuplicateHasRun] = useState(false);
  const [duplicateSearchError, setDuplicateSearchError] = useState<string | null>(null);
  const [duplicateSearchCriteria, setDuplicateSearchCriteria] = useState<string | null>(null);
  const [duplicateScope, setDuplicateScope] = useState<"file" | "current" | "all">("current");
  const [duplicateMinPreset, setDuplicateMinPreset] = useState("10m");
  const [duplicateCustomMiB, setDuplicateCustomMiB] = useState(10);
  const [duplicateFileKind, setDuplicateFileKind] = useState("all");
  const [duplicateLimit, setDuplicateLimit] = useState(25);
  const [duplicateConfirmState, setDuplicateConfirmState] = useState<DuplicateConfirmStateMap>({});
  const [operationPlan, setOperationPlan] = useState<OperationPlan | null>(null);
  const [riskReport, setRiskReport] = useState<RiskReport | null>(null);
  const [planLoading, setPlanLoading] = useState(false);
  const [planJobId, setPlanJobId] = useState<string | null>(null);
  const [planJobStatus, setPlanJobStatus] = useState<PlanPreviewStatus | null>(null);
  const [planTargetNode, setPlanTargetNode] = useState<{ nodeId: number; label: string; kind: string } | null>(null);
  const [preparedSafeManageDecision, setPreparedSafeManageDecision] = useState<{
    projectId: number;
    decision: Extract<SafeManageDecisionKind, "archive" | "clean_regenerables" | "prepare_removal">;
    analysisRunId: string;
    evidenceRevision: string;
    target?: SafeManageOperationPlanRequest["target"];
  } | null>(null);
  // In-app confirmation modal. We do NOT use window.confirm: in the Tauri webview it is routed
  // to plugin:dialog|confirm and blocked by the capability ACL ("not allowed by ACL"), which
  // silently broke every destructive confirmation. This promise-based modal has no ACL
  // dependency and works the same in the core and mutation builds.
  const [confirmRequest, setConfirmRequest] = useState<{
    message: string;
    confirmLabel: string;
    tone: "primary" | "danger";
    resolve: (ok: boolean) => void;
  } | null>(null);
  const requestConfirm = useCallback(
    (
      message: string,
      options: { confirmLabel?: string; tone?: "primary" | "danger" } = {}
    ) => new Promise<boolean>((resolve) => setConfirmRequest({
      message,
      confirmLabel: options.confirmLabel ?? "Confirm",
      tone: options.tone ?? "primary",
      resolve
    })),
    []
  );
  const resolveConfirm = useCallback((ok: boolean) => {
    setConfirmRequest((current) => {
      current?.resolve(ok);
      return null;
    });
  }, []);
  const [reportLoading, setReportLoading] = useState(false);
  const [mutationAvailable, setMutationAvailable] = useState(false);
  const [finalRemoveEnabled, setFinalRemoveEnabled] = useState(false);
  const [finalRemoveCapabilityLoading, setFinalRemoveCapabilityLoading] = useState(true);
  const [finalRemovePreview, setFinalRemovePreview] = useState<FinalRemoveBatchPreview | null>(null);
  const [finalRemovePreviewLoading, setFinalRemovePreviewLoading] = useState(false);
  const [finalRemoveUnavailableReason, setFinalRemoveUnavailableReason] = useState<string | null>(null);
  const [finalRemoveReview, setFinalRemoveReview] = useState<{ preview: FinalRemoveBatchPreview; scope: FinalRemoveScope } | null>(null);
  const [finalRemoveProgress, setFinalRemoveProgress] = useState<FinalRemoveBatchProgress | null>(null);
  const [finalRemoveResult, setFinalRemoveResult] = useState<FinalRemoveBatchResult | null>(null);
  const [finalRemoveError, setFinalRemoveError] = useState<string | null>(null);
  const [finalRemoveJobId, setFinalRemoveJobId] = useState<string | null>(null);
  const [finalRemoveBatchId, setFinalRemoveBatchId] = useState<string | null>(null);
  const [finalRemoveExecutionUnknown, setFinalRemoveExecutionUnknown] = useState(false);
  // The last reversible "remove from AI apps" action, so the status bar can offer Undo.
  const [appRemovalUndo, setAppRemovalUndo] = useState<{ name: string; id: string } | null>(null);
  const [appRemovals, setAppRemovals] = useState<PersistedAppRemoval[]>([]);
  const [mutationModeToken, setMutationModeToken] = useState<string | null>(null);
  const [mutationBusy, setMutationBusy] = useState(false);
  const [mutationActivity, setMutationActivity] = useState<MutationActivityLog | null>(null);
  const [mutationMessage, setMutationMessage] = useState<string | null>(null);
  const [lastMutationMove, setLastMutationMove] = useState<MutationMoveSummary | null>(null);
  const [mutationLockInspection, setMutationLockInspection] = useState<MutationLockInspection | null>(null);
  const [mutationLockLoading, setMutationLockLoading] = useState(false);
  const [mutationBackupLevel, setMutationBackupLevel] = useState<"minimal" | "standard" | "full">("standard");
  const [mutationAllowSameVolume, setMutationAllowSameVolume] = useState(false);
  // Protected content is included only after explicit disclosure because the backup may
  // contain secrets. This stage creates a content backup and then moves supported objects
  // to holding; it does not claim object-archive-v2 completeness or final disk cleanup.
  const [mutationIncludeProtected] = useState(true);
  // The verified backup that covers the CURRENT plan. A move to the recovery area is
  // only allowed once this is set (Gate 3: no move/delete without a verified backup).
  const [mutationBackupId, setMutationBackupId] = useState<number | null>(null);
  // The folder currently being investigated by path (not a registered project).
  const [investigation, setInvestigation] = useState<FolderInvestigation | null>(null);
  const [investigationBusy, setInvestigationBusy] = useState(false);
  const [scanStatuses, setScanStatuses] = useState<Record<string, ScanStatus>>({});
  const scanStatusesRef = useRef<Record<string, ScanStatus>>({});
  const scanAnnouncementSnapshotsRef = useRef<Record<string, ScanStatus>>({});
  const [scanCelebration, setScanCelebration] = useState<{ files: number; durationMs: number; nonce: number } | null>(null);
  const celebratedJobsRef = useRef<Set<string>>(new Set());
  const [startupProgress, setStartupProgress] = useState<StartupProgress>({
    active: true,
    label: "Opening local inventory",
    detail: "Preparing the navigation shell.",
    progress: 8
  });
  const [backgroundStatus, setBackgroundStatus] = useState<string | null>("Starting Code Hangar.");
  const [statusText, setStatusText] = useState("Starting local inventory.");
  const [hoverHelp, setHoverHelp] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [rightPaneView, setRightPaneView] = useState<RightPaneView>(INITIAL_WORKSPACE_ROUTE.rightPaneView);
  const [primaryView, setPrimaryView] = useState<PrimaryView>(INITIAL_WORKSPACE_ROUTE.primaryView);
  const [projectView, setProjectView] = useState<ProjectView>(INITIAL_WORKSPACE_ROUTE.projectView);
  const [discoverView, setDiscoverView] = useState<DiscoverView>(INITIAL_WORKSPACE_ROUTE.discoverView);
  const [settingsView, setSettingsView] = useState<SettingsView>(INITIAL_WORKSPACE_ROUTE.settingsView);
  const [viewBackStack, setViewBackStack] = useState<WorkspaceRoute[]>([]);
  const [viewForwardStack, setViewForwardStack] = useState<WorkspaceRoute[]>([]);
  const tourReplayOriginRef = useRef<{
    route: WorkspaceRoute;
    backStack: WorkspaceRoute[];
    forwardStack: WorkspaceRoute[];
    paneCollapsed: { left: boolean; right: boolean };
  } | null>(null);
  const [paneWidths, setPaneWidths] = useState(initialPaneWidths);
  const [paneCollapsed, setPaneCollapsed] = useState(initialPaneCollapse);
  const [startupPreferences, setStartupPreferences] = useState(initialStartupPreferences);
  const [startupRouteResolved, setStartupRouteResolved] = useState(false);
  const storedStartupRouteRef = useRef<WorkspaceRoute | null>(initialStoredWorkspaceRoute());
  const [projectSidebarFocus, setProjectSidebarFocus] = useState(true);
  const [projectInspectorExpanded, setProjectInspectorExpanded] = useState(false);
  const [workspaceWindowWidth, setWorkspaceWindowWidth] = useState(
    () => typeof window !== "undefined" ? window.innerWidth : 1280
  );
  const compactProjectWindow = workspaceWindowWidth <= 1080;
  const [treePaneWidth, setTreePaneWidth] = useState(initialTreePaneWidth);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(initialSidebarCollapse);
  // Recent shows the 5 newest by default; right-click the header to switch to all.
  const [recentShowAll, setRecentShowAll] = useState(false);
  const [archivedCollapsed, setArchivedCollapsed] = useState(initialArchivedCollapsed);
  const [showDemoProjects, setShowDemoProjects] = useState<boolean | null>(initialDemoProjectPreference);
  const [performanceMode, setPerformanceMode] = useState<PerformanceMode>(initialPerformanceMode);
  const [themeMode, setThemeMode] = useState<ThemeMode>(initialThemeMode);
  const [advancedMode, setAdvancedMode] = useState(initialAdvancedMode);
  const [showAllProjectPaths, setShowAllProjectPaths] = useState(initialShowAllProjectPaths);
  const [showTopbarNav, setShowTopbarNav] = useState(initialShowTopbarNav);
  const [sidebarScrolled, setSidebarScrolled] = useState(false);
  const [projectSort, setProjectSort] = useState<ProjectSort>(initialProjectSort);
  const [projectAppFilter, setProjectAppFilter] = useState<string>(initialProjectAppFilter);
  const [projectStatusFilter, setProjectStatusFilter] = useState<ProjectStatusFilter>(initialProjectStatusFilter);
  const [projectQuery, setProjectQuery] = useState("");
  const [projectListExpanded, setProjectListExpanded] = useState(false);
  const [sessionSort, setSessionSort] = useState<SessionSort>(initialSessionSort);
  const [sessionAppFilter, setSessionAppFilter] = useState<string>(initialSessionAppFilter);
  const [sessionScope, setSessionScope] = useState<SessionScope>("all");
  const [sessionQuery, setSessionQuery] = useState("");
  const [sessionGroupsExpanded, setSessionGroupsExpanded] = useState(false);
  const [appearance, setAppearance] = useState(initialAppearance);
  const [zoneShowProtectedMetadata, setZoneShowProtectedMetadata] = useState(false);
  const [zoneAllowSensitiveReveal, setZoneAllowSensitiveReveal] = useState(false);
  const [zoneRelaxNonStrongPreview, setZoneRelaxNonStrongPreview] = useState(false);
  const selectionSeq = useRef(0);
  const pendingTreeRevealRef = useRef<{ projectId: number; nodeId: number } | null>(null);
  const sessionPreviewSeq = useRef(0);
  const sessionTitleEnrichmentAttemptedRef = useRef<Set<string>>(new Set());
  const primaryViewRef = useRef(primaryView);
  const previewCacheRef = useRef<Map<string, FilePreview>>(new Map());
  const documentSearchSeq = useRef(0);
  const orphanSearchSeq = useRef(0);
  const duplicateSearchSeq = useRef(0);
  const fileOrphanSeq = useRef(0);
  const manualPreviewClearProjectRef = useRef<number | null>(null);
  const watcherPreviewRefreshRef = useRef<string | null>(null);
  const graphMapRef = useRef<GraphMap | null>(null);
  const graphMapExpansionRunRef = useRef(0);
  const graphMapExpansionPauseRef = useRef(false);
  const dashboardAutoLoadAttemptedRef = useRef(false);
  // While a full "Reset all" is in flight, background DB polling (the watcher)
  // must stand down: its read would otherwise contend with the reset's large
  // write transaction and, on a non-WAL/journal database, deadlock it.
  const resettingRef = useRef(false);
  const pointerTabDragRef = useRef<{ nodeId: number; startX: number; startY: number; dragging: boolean } | null>(null);
  const projectSearchInputRef = useRef<HTMLInputElement | null>(null);
  const suppressNextTabClickRef = useRef(false);
  const { startTabPointerDrag } = useTabDrag<OpenTab>({
    pointerTabDragRef,
    suppressNextTabClickRef,
    setDraggedTabNodeId,
    setTabDropTargetNodeId,
    setTabs
  });

  const invalidateShellOpenIntent = useCallback(() => {
    const nextGeneration = shellOpenIntentGenerationRef.current + 1;
    shellOpenIntentGenerationRef.current = nextGeneration;
    const pendingChoice = shellOpenChoiceResolverRef.current;
    if (pendingChoice) {
      shellOpenChoiceResolverRef.current = null;
      setShellOpenChoice(null);
      pendingChoice.resolve("cancel");
    }
    return nextGeneration;
  }, []);

  const navigationProjects = useMemo(
    () => shellViewer ? [shellViewer.project] : projects,
    [projects, shellViewer]
  );
  const selectedProject = useMemo(
    () => navigationProjects.find((project) => project.id === selectedProjectId) ?? null,
    [navigationProjects, selectedProjectId]
  );
  const changesUnlocked = shellViewer === null
    && selectedProjectId !== null
    && unlockedChangeProjectId === selectedProjectId;
  useEffect(() => {
    setUnlockedChangeProjectId(null);
    setChangeUnlockTarget(null);
  }, [selectedProjectId]);
  useEffect(() => {
    if (!changesUnlocked && (previewMode === "edit" || previewMode === "values")) {
      setPreviewMode("source");
    }
  }, [changesUnlocked, previewMode]);
  const requestChangeAccess = useCallback(() => {
    if (shellViewer) {
      setStatusText("Viewer mode is read-only. Exit Viewer and register the project before changing files.");
      return;
    }
    if (!selectedProject) {
      setStatusText("Choose a project before unlocking file changes.");
      return;
    }
    setChangeUnlockTarget(selectedProject);
  }, [selectedProject, shellViewer]);
  const selectedProjectScanRoot = useMemo(() => {
    if (!selectedProject) return null;
    if (selectedProject.scanRootId != null) {
      return roots.find((root) => root.id === selectedProject.scanRootId) ?? null;
    }
    const selectedPath = normalizeProjectRootPath(selectedProject.path);
    return roots.find((root) => normalizeProjectRootPath(root.path) === selectedPath) ?? null;
  }, [roots, selectedProject]);
  const displayedProjects = useMemo(
    () => visibleProjects(navigationProjects, showDemoProjects),
    [navigationProjects, showDemoProjects]
  );
  const displayedDocumentHits = useMemo(
    () => visibleProjectItems(documentHits, navigationProjects, showDemoProjects),
    [documentHits, navigationProjects, showDemoProjects]
  );
  const effectiveSessionInventory = useMemo(
    () => shellViewer ? [] : sessionInventory.map((session) => {
      const displayName = sessionDisplayNameNeedsEnrichment(session.displayName)
        ? sessionTitleOverrides[session.path]
        : undefined;
      return displayName ? { ...session, displayName } : session;
    }),
    [sessionInventory, sessionTitleOverrides, shellViewer]
  );
  const visibleQuickResults = useMemo(
    () => composeQuickOpenResults(quickQuery, quickResults, navigationProjects, showDemoProjects),
    [navigationProjects, quickQuery, quickResults, showDemoProjects]
  );
  const quickOpenStarterResults = useMemo(
    () => starterQuickOpenResults(displayedProjects, selectedProjectId),
    [displayedProjects, selectedProjectId]
  );
  // Active vs archived, the way the apps reflect it: a project an AI app has
  // actually held a conversation about (a session links to it) is "active"; one
  // with no linked session — only ever registry-listed, or opened once and left —
  // is "archived". The signal is the session→project linkage (the same reliable
  // match the sidebar groups sessions by), not folder mtimes (which OneDrive sync
  // inflates). Only classify once a Deep Scan has loaded sessions; before that
  // every project stays active so nothing hides behind an Archived header.
  const archivedProjectIds = useMemo(() => {
    if (!projectDiscoveryReport || sessionInventory.length === 0) return new Set<number>();
    const normalize = (value: string) => value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
    const projectByPath = projects.map((project) => ({ id: project.id, path: normalize(project.path) }));
    const activeIds = new Set<number>();
    for (const session of sessionInventory) {
      if (isHermesSessionKind(session.sessionKind)) continue;
      for (const id of session.linkedRegisteredProjectIds) activeIds.add(id);
      for (const linked of session.linkedProjectPaths) {
        const p = normalize(linked);
        const hit = projectByPath.find(({ path }) => p === path || p.startsWith(`${path}\\`) || path.startsWith(`${p}\\`));
        if (hit) activeIds.add(hit.id);
      }
    }
    const archived = new Set<number>();
    for (const project of projects) {
      if (isDemoProject(project)) continue;
      // The owning AI app still listing it as active (open/recent) overrides the
      // session-linkage heuristic — e.g. a project is current in Antigravity even
      // though its conversation didn't resolve to a linked session here.
      if (project.isCurrent) continue;
      if (!activeIds.has(project.id)) archived.add(project.id);
    }
    return archived;
  }, [projects, projectDiscoveryReport, sessionInventory]);
  // Latest linked-session timestamp per project — powers the "Recently active" sort.
  const projectRecency = useMemo(() => {
    const normalize = (value: string) => value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
    const projectByPath = projects.map((project) => ({ id: project.id, path: normalize(project.path) }));
    const latest = new Map<number, number>();
    const bump = (id: number, ms: number) => {
      const prev = latest.get(id);
      if (prev == null || ms > prev) latest.set(id, ms);
    };
    for (const session of sessionInventory) {
      const ms = session.modifiedMs;
      if (ms == null) continue;
      for (const id of session.linkedRegisteredProjectIds) bump(id, ms);
      for (const linked of session.linkedProjectPaths) {
        const p = normalize(linked);
        const hit = projectByPath.find(({ path }) => p === path || p.startsWith(`${path}\\`) || path.startsWith(`${p}\\`));
        if (hit) bump(hit.id, ms);
      }
    }
    return latest;
  }, [projects, sessionInventory]);
  // Distinct apps actually present, for the app-filter dropdowns.
  const projectAppOptions = useMemo(() => {
    const map = new Map<string, AppMeta>();
    for (const project of displayedProjects) {
      for (const meta of projectAppMetas(project)) {
        if (!map.has(meta.slug)) map.set(meta.slug, meta);
      }
    }
    return [...map.values()].sort((a, b) => a.label.localeCompare(b.label));
  }, [displayedProjects]);
  const sessionAppOptions = useMemo(() => {
    const map = new Map<string, AppMeta>();
    for (const session of sessionInventory) {
      const meta = sessionAppMeta(session);
      if (!map.has(meta.slug)) map.set(meta.slug, meta);
    }
    return [...map.values()].sort((a, b) => a.label.localeCompare(b.label));
  }, [sessionInventory]);
  // A stored app filter that no longer matches any present app degrades to "all"
  // so the list never silently empties (e.g. after a Reset).
  const effectiveProjectAppFilter = projectAppOptions.some((o) => o.slug === projectAppFilter) ? projectAppFilter : "all";
  const effectiveSessionAppFilter = sessionAppOptions.some((o) => o.slug === sessionAppFilter) ? sessionAppFilter : "all";
  const demosVisible = useMemo(() => shouldShowDemoProjects(projects, showDemoProjects), [projects, showDemoProjects]);
  const documentProjectResolution = useMemo(
    () => resolveProjectPickerInput(displayedProjects, documentProjectInput),
    [displayedProjects, documentProjectInput]
  );
  const currentDocumentProjectAvailable = selectedProjectId !== null
    && displayedProjects.some((project) => project.id === selectedProjectId);
  const documentSearchProjectId = documentScope === "current"
    ? currentDocumentProjectAvailable ? selectedProjectId : null
    : documentScope === "project" && documentProjectResolution.kind === "resolved"
      ? documentProjectResolution.project.id
      : null;
  const currentDocumentSearchCriteria = useMemo(() => documentSearchCriteriaKey({
    query: documentQuery,
    scope: documentScope,
    projectId: documentSearchProjectId,
    indexedKind: documentKind,
    pathFilter: documentPathFilter,
    nameFilter: documentNameFilter,
    limit: documentLimit,
    includeFixtureProjects: demosVisible
  }), [demosVisible, documentKind, documentLimit, documentNameFilter, documentPathFilter, documentQuery, documentScope, documentSearchProjectId]);
  const currentOrphanSearchCriteria = useMemo(() => orphanSearchCriteriaKey({
    mode: orphanMode,
    scope: orphanScope,
    projectId: selectedProjectId,
    minPreset: orphanMinPreset,
    customMiB: orphanCustomMiB,
    includePartial: orphanIncludePartial,
    stalePreset: lostStalePreset,
    signals: lostSignals,
    keyword: lostKeyword,
    assetKind: orphanAssetKind,
    minConfidence: orphanMinConfidence,
    includeFixtureProjects: demosVisible
  }), [demosVisible, lostKeyword, lostSignals, lostStalePreset, orphanAssetKind, orphanCustomMiB, orphanIncludePartial, orphanMinConfidence, orphanMinPreset, orphanMode, orphanScope, selectedProjectId]);
  const currentDuplicateSearchCriteria = useMemo(() => duplicateSearchCriteriaKey({
    scope: duplicateScope,
    projectId: selectedProjectId,
    currentFileNodeId: preview?.nodeId ?? null,
    minPreset: duplicateMinPreset,
    customMiB: duplicateCustomMiB,
    fileKind: duplicateFileKind,
    limit: duplicateLimit,
    includeFixtureProjects: demosVisible
  }), [demosVisible, duplicateCustomMiB, duplicateFileKind, duplicateLimit, duplicateMinPreset, duplicateScope, preview?.nodeId, selectedProjectId]);
  const documentSearchIsCurrent = documentSearchCriteria === currentDocumentSearchCriteria;
  const orphanSearchIsCurrent = (orphanMode === "lost" ? lostSearchCriteria : assetSearchCriteria) === currentOrphanSearchCriteria;
  const duplicateSearchIsCurrent = duplicateSearchCriteria === currentDuplicateSearchCriteria;
  const realProjectCount = useMemo(() => projects.filter((project) => !isDemoProject(project)).length, [projects]);
  const tourHasRealProjects = realProjectCount > 0;
  // True only when this edition includes the optional integration surface.
  const editionExtensionActive = connectorFrontendBuild
    && shellViewer === null
    && (security?.activeFeatures.includes("agent_automation") ?? false);
  const displayedProjectIds = useMemo(() => new Set(displayedProjects.map((project) => project.id)), [displayedProjects]);
  const displayedPinnedItems = useMemo(
    () => pinnedItems.filter((item) => {
      if (shellViewer) return item.projectId != null && displayedProjectIds.has(item.projectId);
      if (item.itemKind === "project") return displayedProjectIds.has(item.nodeId);
      if (item.projectId != null) return displayedProjectIds.has(item.projectId);
      return true;
    }),
    [displayedProjectIds, pinnedItems, shellViewer]
  );
  const displayedRecentItems = useMemo(
    () => shellViewer
      ? recentItems.filter((item) => item.projectId != null && displayedProjectIds.has(item.projectId))
      : recentItems,
    [displayedProjectIds, recentItems, shellViewer]
  );
  const previewPolicy = useMemo<PreviewPolicy>(
    () => ({
      allowSensitiveReveal: zoneAllowSensitiveReveal,
      relaxNonStrongProtectedPreview: zoneRelaxNonStrongPreview
    }),
    [zoneAllowSensitiveReveal, zoneRelaxNonStrongPreview]
  );

  const selectedPinned = useMemo(
    () => preview
      ? pinnedItems.some((item) => (
          item.nodeId === preview.nodeId
          && item.projectId === preview.projectId
          && item.itemKind === "file"
        ))
      : false,
    [pinnedItems, preview]
  );
  const selectedFootprint = useMemo(
    () => dashboard?.largestProjects.find((project) => project.projectId === selectedProjectId) ?? null,
    [dashboard, selectedProjectId]
  );
  const reviewTargetNodeId = planTargetNode?.nodeId ?? selectedProjectId ?? null;
  const activeOperationPlan = useMemo(() => {
    if (!operationPlan || reviewTargetNodeId === null) {
      return null;
    }
    return operationPlan.target.nodeId === reviewTargetNodeId ? operationPlan : null;
  }, [operationPlan, reviewTargetNodeId]);
  const activeRiskReport = useMemo(() => {
    if (!activeOperationPlan || !riskReport) {
      return null;
    }
    return riskReport.target.nodeId === activeOperationPlan.target.nodeId ? riskReport : null;
  }, [activeOperationPlan, riskReport]);
  useEffect(() => {
    setMutationBackupId(null);
  }, [activeOperationPlan?.targetFingerprint, mutationIncludeProtected]);
  const selectedProjectOverlapWarning = useMemo(
    () => selectedProject ? projectOverlapWarning(selectedProject, displayedProjects) : null,
    [displayedProjects, selectedProject]
  );
  const previewOrphanStatus = preview ? orphanStatusByMembership[fileMembershipKey(preview.projectId, preview.nodeId)] ?? null : null;
  const rootTreeItems = useMemo(() => treePages.root?.items ?? [], [treePages.root]);
  const selectedProjectSessions = useMemo(() => {
    if (!selectedProject) return [];
    // Same source and matcher as the sidebar's per-project session groups, so the
    // project's Sessions tab can never disagree with the sidebar (a projects-only
    // rescan replaces the report but not the inventory). The last discovery report
    // is only a fallback for before any session inventory has loaded.
    const source = effectiveSessionInventory.length > 0 ? effectiveSessionInventory : projectDiscoveryReport?.sessions ?? [];
    const projectById = new Map(projects.map((project) => [project.id, project]));
    const projectByPath = projects.map((project) => ({ project, path: normalizeProjectPath(project.path) }));
    return source.filter((session) =>
      !isHermesSessionKind(session.sessionKind)
      && findSessionProject(session, projectById, projectByPath)?.id === selectedProject.id
    );
  }, [effectiveSessionInventory, projectDiscoveryReport, projects, selectedProject]);
  // The registered project the open session belongs to (same matcher again) —
  // drives the session pane's Back target and its "Open project" action.
  const previewSessionProject = useMemo(() => {
    if (!previewSession) return null;
    const projectById = new Map(projects.map((project) => [project.id, project]));
    const projectByPath = projects.map((project) => ({ project, path: normalizeProjectPath(project.path) }));
    return findSessionProject(previewSession, projectById, projectByPath) ?? null;
  }, [previewSession, projects]);
  // Left-sidebar Sessions, organised the way the user expects: sessions that
  // belong to a registered project grouped under that project, then independent
  // sessions (no project), then Hermes (high-volume — kept separate). Built only
  // from the session inventory, so project discovery never changes it.
  const sidebarSessionGroups = useMemo(() => {
    const projectById = new Map(projects.map((project) => [project.id, project]));
    const projectByPath = projects.map((project) => ({ project, path: normalizeProjectPath(project.path) }));
    const byProject = new Map<number, { project: ProjectSummary; sessions: SessionDiscoveryCandidate[] }>();
    const independent: SessionDiscoveryCandidate[] = [];
    const hermes: SessionDiscoveryCandidate[] = [];
    for (const session of effectiveSessionInventory) {
      if (isHermesSessionKind(session.sessionKind)) {
        hermes.push(session);
        continue;
      }
      const project = findSessionProject(session, projectById, projectByPath);
      if (project) {
        const group = byProject.get(project.id) ?? { project, sessions: [] };
        group.sessions.push(session);
        byProject.set(project.id, group);
      } else {
        independent.push(session);
      }
    }
    const projectGroups = [...byProject.values()].sort((a, b) => a.project.name.localeCompare(b.project.name));
    return { projectGroups, independent, hermes };
  }, [effectiveSessionInventory, projects]);
  const reviewProjectGroups = useMemo(
    () => sidebarSessionGroups.projectGroups.filter(({ project }) => realProjectCount === 0 || !isDemoProject(project)),
    [realProjectCount, sidebarSessionGroups.projectGroups]
  );
  // Apply the session sort + app filter to the grouped sessions for rendering.
  const displayedSessionGroups = useMemo(() => {
    return displayedSidebarSessionGroups(sidebarSessionGroups, {
      sort: sessionSort,
      appFilter: effectiveSessionAppFilter,
      query: sessionQuery,
      scope: sessionScope
    });
  }, [sidebarSessionGroups, sessionSort, effectiveSessionAppFilter, sessionQuery, sessionScope]);
  const sessionContentFiltersActive = sessionQuery.trim().length > 0 || effectiveSessionAppFilter !== "all";
  const sessionListFiltersActive = sessionContentFiltersActive || sessionScope !== "all";
  const sessionListCountLabel = sessionListFiltersActive
    ? `${displayedSessionGroups.count}/${sessionInventory.length}`
    : String(displayedSessionGroups.count);
  const sessionSearchActive = sessionQuery.trim().length > 0;
  const renderedSessionGroups = useMemo(() => {
    if (sessionContentFiltersActive || sessionGroupsExpanded) {
      return { ...displayedSessionGroups, hiddenGroupCount: 0, compacted: false };
    }
    return compactSidebarSessionGroups(displayedSessionGroups, SESSION_GROUP_PREVIEW_LIMIT);
  }, [displayedSessionGroups, sessionContentFiltersActive, sessionGroupsExpanded]);
  const clearSessionListFilters = useCallback(() => {
    setSessionQuery("");
    setSessionAppFilter("all");
    setSessionScope("all");
  }, []);
  useEffect(() => {
    if (sessionInventory.length === 0) {
      sessionTitleEnrichmentAttemptedRef.current.clear();
      setSessionTitleOverrides((current) => Object.keys(current).length === 0 ? current : {});
      return;
    }
    const attempted = sessionTitleEnrichmentAttemptedRef.current;
    const selectedSessionPaths = new Set(selectedProjectSessions.map((session) => session.path));
    const candidates = sessionInventory
      .filter((session) => !sessionTitleOverrides[session.path] && sessionDisplayNameNeedsEnrichment(session.displayName) && !attempted.has(session.path))
      .sort((left, right) => {
        const leftSelected = selectedSessionPaths.has(left.path);
        const rightSelected = selectedSessionPaths.has(right.path);
        return Number(rightSelected) - Number(leftSelected);
      })
      .slice(0, 8);
    if (candidates.length === 0) return;
    candidates.forEach((session) => attempted.add(session.path));
    void Promise.all(candidates.map(async (session) => {
      try {
        const preview = await api.sessionPreview(session.path, false);
        const displayName = enrichedSessionDisplayName(session.displayName, preview.text);
        return displayName === session.displayName ? null : ([session.path, displayName] as const);
      } catch {
        return null;
      }
    })).then((updates) => {
      const titlesByPath = new Map(updates.filter((update): update is readonly [string, string] => update !== null));
      if (titlesByPath.size === 0) return;
      setSessionTitleOverrides((current) => {
        const next = { ...current };
        for (const [path, displayName] of titlesByPath) next[path] = displayName;
        return next;
      });
    });
  }, [selectedProjectSessions, sessionInventory, sessionTitleOverrides]);
  const selectedFootprintForView = useMemo(
    () => selectedFootprint ?? deriveFootprintFromRootItems(selectedProject, rootTreeItems),
    [rootTreeItems, selectedFootprint, selectedProject]
  );
  const scanStatusList = useMemo(() => Object.values(scanStatuses), [scanStatuses]);
  const runningScanStatuses = useMemo(
    () => scanStatusList.filter((status) => ["running", "cancelling"].includes(status.state)),
    [scanStatusList]
  );
  const primaryRunningScan = runningScanStatuses[0] ?? null;
  const primaryRunningScanProgress = useMemo(
    () => (primaryRunningScan ? scanProgressParts(primaryRunningScan) : null),
    [primaryRunningScan]
  );
  const latestScanStatus = scanStatusList.at(-1) ?? null;
  const runningJobKey = runningScanStatuses.map((status) => status.jobId).sort().join("|");
  const backgroundStatusText = useMemo(() => {
    if (backgroundStatus) return backgroundStatus;
    if (
      watcherStatus?.focused?.state === "dirty"
      && focusedFileStatusIsRelevant(primaryView, projectView, previewSession !== null)
    ) return watcherStatus.focused.message;
    if (watcherStatus && watcherStatus.staleProjects > 0) return watcherStatus.message;
    return null;
  }, [backgroundStatus, previewSession, primaryView, projectView, watcherStatus]);
  const rootIsScanning = useCallback(
    (rootId: number) => runningScanStatuses.some((status) => status.rootIds.includes(rootId)),
    [runningScanStatuses]
  );
  const watcherProjectsByProjectId = useMemo(() => {
    const map = new Map<number, WatcherStatus["projects"][number]>();
    for (const status of watcherStatus?.projects ?? []) {
      if (typeof status.projectId === "number") map.set(status.projectId, status);
    }
    return map;
  }, [watcherStatus]);
  const watcherProjectsByRootId = useMemo(() => {
    const map = new Map<number, WatcherStatus["projects"][number]>();
    for (const status of watcherStatus?.projects ?? []) {
      map.set(status.scanRootId, status);
    }
    return map;
  }, [watcherStatus]);
  const projectWatchStatus = useCallback(
    (project: ProjectSummary) => {
      const byProject = watcherProjectsByProjectId.get(project.id);
      if (byProject) return byProject;
      const rootId = project.scanRootId ?? roots.find((root) => root.path === project.path)?.id ?? null;
      return rootId === null ? null : watcherProjectsByRootId.get(rootId) ?? null;
    },
    [roots, watcherProjectsByProjectId, watcherProjectsByRootId]
  );
  const projectScanState = useCallback(
    (project: ProjectSummary): ProjectScanState => {
      const rootId = project.scanRootId ?? roots.find((root) => root.path === project.path)?.id ?? null;
      const watchState = projectWatchStatus(project)?.state;
      return resolveProjectScanState(
        project.scanState,
        watchState,
        rootId !== null && rootIsScanning(rootId),
        projectsFromCache && project.source === "scan" && project.scanState === "scanned"
      );
    },
    [projectWatchStatus, projectsFromCache, rootIsScanning, roots]
  );
  // Coarse status bucket for the project status filter: ready | scanning | needs-scan.
  const projectStatusBucket = useCallback(
    (project: ProjectSummary): Exclude<ProjectStatusFilter, "all"> => {
      const state = projectScanState(project);
      if (state === "scanning") return "scanning";
      if (state === "outdated") return "needs-scan";
      return "ready";
    },
    [projectScanState]
  );
  const orderedDisplayedProjects = useMemo(() => {
    return orderSidebarProjects(displayedProjects, {
      sort: projectSort,
      appFilter: effectiveProjectAppFilter,
      statusFilter: projectStatusFilter,
      query: projectQuery,
      archivedProjectIds,
      recencyByProjectId: projectRecency,
      getStatusBucket: projectStatusBucket
    });
  }, [displayedProjects, archivedProjectIds, projectSort, effectiveProjectAppFilter, projectStatusFilter, projectQuery, projectRecency, projectStatusBucket]);
  const projectListFiltersActive = projectQuery.trim().length > 0 || effectiveProjectAppFilter !== "all" || projectStatusFilter !== "all";
  const projectListCountLabel = projectListFiltersActive
    ? `${orderedDisplayedProjects.all.length}/${displayedProjects.length}`
    : String(orderedDisplayedProjects.all.length);
  const projectListHasOverflow = !projectListFiltersActive && orderedDisplayedProjects.all.length > PROJECT_LIST_PREVIEW_LIMIT;
  const displayedSidebarProjects = useMemo(() => {
    if (!projectListHasOverflow || projectListExpanded) {
      return { projects: orderedDisplayedProjects.all, hiddenCount: 0, compacted: false };
    }
    return compactSidebarProjects(orderedDisplayedProjects.all, {
      limit: PROJECT_LIST_PREVIEW_LIMIT,
      selectedProjectId
    });
  }, [orderedDisplayedProjects.all, projectListExpanded, projectListHasOverflow, selectedProjectId]);
  const firstRenderedArchivedProjectId = displayedSidebarProjects.projects.find((project) => archivedProjectIds.has(project.id))?.id ?? null;
  const clearProjectListFilters = useCallback(() => {
    setProjectQuery("");
    setProjectAppFilter("all");
    setProjectStatusFilter("all");
  }, []);
  // Live status + per-project state for the Deep Scan "building inventory" panel.
  const buildScanStatus = deepScanProgress?.scanJobId
    ? scanStatuses[deepScanProgress.scanJobId] ?? null
    : null;
  const buildProjects = useMemo<BuildProject[]>(() => {
    if (!buildScanStatus) return [];
    // The scan walks roots sequentially in rootPaths order, so the project that
    // contains currentPath is being visited now. Earlier roots are only labelled
    // "processed" until the whole job completes; a later failure must never make
    // them look like a proven, ready inventory.
    const normalize = (value: string) => value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
    const order = buildScanStatus.rootPaths.map(normalize);
    const current = buildScanStatus.currentPath ? normalize(buildScanStatus.currentPath) : null;
    let currentIndex = -1;
    if (current) {
      currentIndex = order.findIndex((path) => current === path || current.startsWith(`${path}\\`));
    }
    const terminal = deepScanOutcomeFromScanState(buildScanStatus.state) != null;
    // The scan is actively working even when currentPath can't be pinned to a root
    // (the estimate/persist/finalize phases). In that case show the not-yet-done
    // projects as "indexing", not "queued", so the panel never looks stuck.
    const working = !terminal && buildScanStatus.scannedFiles > 0;
    const rootIds = new Set(buildScanStatus.rootIds);
    return projects
      .filter((project) => project.scanRootId != null && rootIds.has(project.scanRootId))
      .map((project) => ({ project, index: order.indexOf(normalize(project.path)) }))
      .sort((a, b) => (a.index < 0 ? order.length : a.index) - (b.index < 0 ? order.length : b.index))
      .map(({ project, index }) => {
        const state = deepScanBuildProjectState(buildScanStatus.state, index, currentIndex, working);
        return {
          id: project.id,
          name: project.name,
          state,
          done: state === "indexed",
          current: state === "indexing"
        };
      });
  }, [buildScanStatus, projects]);
  const buildDoneRef = useRef(false);
  useEffect(() => {
    if (deepScanProgress?.phase !== "building") {
      buildDoneRef.current = false;
      return;
    }
    if (!buildScanStatus || ["running", "cancelling", "queued"].includes(buildScanStatus.state)) return;
    if (buildDoneRef.current) return;
    buildDoneRef.current = true;
    const outcome = deepScanOutcomeFromScanState(buildScanStatus.state);
    if (!outcome) return;
    const note = outcome === "completed"
      ? `Mapped ${deepScanProgress.addedCount} project${deepScanProgress.addedCount === 1 ? "" : "s"} — files and context are ready.`
      : outcome === "partial"
        ? `Inventory is incomplete after ${buildScanStatus.scannedFiles.toLocaleString()} items. Resume to finish the retained local index.`
        : outcome === "cancelled"
          ? `Scan stopped after ${buildScanStatus.scannedFiles.toLocaleString()} items. The partial inventory was kept.`
          : `Inventory scan failed after ${buildScanStatus.scannedFiles.toLocaleString()} items: ${buildScanStatus.error ?? buildScanStatus.message}`;
    setDeepScanProgress((current) =>
      current && current.phase === "building"
        ? {
            ...current,
            phase: "done",
            outcome,
            retryRootIds: buildScanStatus.rootIds,
            note
          }
        : current
    );
  }, [deepScanProgress?.addedCount, deepScanProgress?.phase, buildScanStatus]);
  useEffect(() => {
    if (!deepScanProgress || !deepScanTerminalPresentation(deepScanProgress.phase, deepScanProgress.outcome).autoDismiss) return;
    const timer = window.setTimeout(() => {
      setDeepScanProgress(null);
      setDeepScanOverlayVisible(false);
    }, 2400);
    return () => window.clearTimeout(timer);
  }, [deepScanProgress]);
  const projectRootPath = useCallback(
    (project: ProjectSummary) =>
      (project.scanRootId != null
        ? roots.find((root) => root.id === project.scanRootId)?.path
        : roots.find((root) => root.path === project.path)?.path) ?? project.path,
    [roots]
  );
  const isProjectLayout = primaryView === "project" || primaryView === "review";
  const reviewFocusedLayout = primaryView === "review";
  const leftPaneCollapsedForLayout = projectSidebarCollapsedForLayout(
    primaryView,
    projectView,
    paneCollapsed.left,
    projectSidebarFocus,
    compactProjectWindow
  );
  const inspectorLayoutCompact = compactProjectWindow || workspaceCenterPaneIsCramped(
    workspaceWindowWidth,
    leftPaneCollapsedForLayout ? COLLAPSED_PANE_WIDTH : paneWidths.left,
    paneCollapsed.right ? COLLAPSED_PANE_WIDTH : paneWidths.right
  );
  const projectInspectorAutoCollapse = primaryView === "project"
    && (inspectorLayoutCompact || projectViewPrefersWideCanvas(projectView));
  const rightPaneCollapsedForLayout = projectInspectorCollapsedForLayout(
    primaryView,
    projectView,
    paneCollapsed.right,
    projectInspectorExpanded,
    inspectorLayoutCompact
  );
  const projectViewHasFileInspector = primaryView !== "project" || projectViewUsesFileInspector(projectView);
  const inspectorPreview = projectViewHasFileInspector ? preview : null;
  const inspectorFolderExplanation = projectViewHasFileInspector ? folderExplanation : null;
  const inspectorContext = projectViewHasFileInspector
    ? FILE_INSPECTOR_CONTEXT
    : projectInspectorContext(projectView, selectedProject?.name, selectedProjectSessions.length);
  const detailsPaneSubject = previewSession?.displayName
    ?? (projectViewHasFileInspector
      ? preview?.displayName ?? folderExplanation?.displayName ?? FILE_INSPECTOR_CONTEXT.subject
      : inspectorContext.subject);
  const workspaceStyle = useMemo(
    () => ({
      "--left-pane-width": `${leftPaneCollapsedForLayout ? COLLAPSED_PANE_WIDTH : paneWidths.left}px`,
      "--right-pane-width": `${rightPaneCollapsedForLayout ? COLLAPSED_PANE_WIDTH : paneWidths.right}px`
    }) as CSSProperties,
    [leftPaneCollapsedForLayout, paneWidths.left, paneWidths.right, rightPaneCollapsedForLayout]
  );
  useEffect(() => {
    setProjectInspectorExpanded(false);
  }, [inspectorLayoutCompact, projectView, selectedProjectId]);
  useEffect(() => {
    const updateWindowWidth = () => setWorkspaceWindowWidth(window.innerWidth);
    window.addEventListener("resize", updateWindowWidth);
    return () => window.removeEventListener("resize", updateWindowWidth);
  }, []);
  const contentGridStyle = useMemo(
    () => ({ "--tree-pane-width": `${treePaneWidth}px` }) as CSSProperties,
    [treePaneWidth]
  );

  const setScanStatus = useCallback((status: ScanStatus) => {
    const next = mergeScanStatusSnapshot(scanStatusesRef.current, status);
    if (next === scanStatusesRef.current) return;
    scanStatusesRef.current = next;
    setScanStatuses(next);
  }, []);

  useEffect(() => {
    window.localStorage.removeItem(DEPRECATED_PROJECT_CACHE_STORAGE_KEY);
  }, []);

  useEffect(() => {
    window.localStorage.setItem(PANE_WIDTH_STORAGE_KEY, JSON.stringify(paneWidths));
  }, [paneWidths]);

  useEffect(() => {
    window.localStorage.setItem(PANE_COLLAPSE_STORAGE_KEY, JSON.stringify(paneCollapsed));
  }, [paneCollapsed]);

  useEffect(() => {
    window.localStorage.setItem(STARTUP_PREFERENCES_STORAGE_KEY, JSON.stringify(startupPreferences));
  }, [startupPreferences]);

  useEffect(() => {
    window.localStorage.setItem(TREE_WIDTH_STORAGE_KEY, String(treePaneWidth));
  }, [treePaneWidth]);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_COLLAPSE_STORAGE_KEY, JSON.stringify(sidebarCollapsed));
  }, [sidebarCollapsed]);

  useEffect(() => {
    window.localStorage.setItem(ARCHIVED_COLLAPSE_STORAGE_KEY, String(archivedCollapsed));
  }, [archivedCollapsed]);

  useEffect(() => {
    window.localStorage.setItem(LOST_PRESETS_STORAGE_KEY, JSON.stringify(savedLostPresets));
  }, [savedLostPresets]);

  const refreshInstalledApps = useCallback(async () => {
    setInstalledAppsLoading(true);
    setInstalledAppsError(null);
    try {
      setInstalledApps(await api.detectInstalledApps());
    } catch (error) {
      setInstalledAppsError(error instanceof Error ? error.message : String(error));
    } finally {
      setInstalledAppsLoading(false);
    }
  }, []);

  const refreshWslScanPreference = useCallback(async () => {
    const sequence = wslPreferenceSequenceRef.current + 1;
    wslPreferenceSequenceRef.current = sequence;
    setWslPreferencePending(true);
    setWslPreferenceError(null);
    try {
      const enabled = await api.wslScanEnabled();
      if (sequence !== wslPreferenceSequenceRef.current) return;
      wslConfirmedChoiceRef.current = enabled;
      wslRequestedChoiceRef.current = enabled;
      setWslScanChoice(enabled);
    } catch (error) {
      if (sequence !== wslPreferenceSequenceRef.current) return;
      const message = error instanceof Error ? error.message : String(error);
      wslRequestedChoiceRef.current = wslConfirmedChoiceRef.current;
      setWslScanChoice(wslConfirmedChoiceRef.current);
      setWslPreferenceError(`Could not read the saved WSL choice: ${message}`);
    } finally {
      if (sequence === wslPreferenceSequenceRef.current) setWslPreferencePending(false);
    }
  }, []);

  const updateWslScanPreference = useCallback((enabled: boolean) => {
    const sequence = wslPreferenceSequenceRef.current + 1;
    wslPreferenceSequenceRef.current = sequence;
    // Record intent synchronously so even an immediate programmatic scan uses
    // the latest click, while the checkbox itself stays on the confirmed value.
    wslRequestedChoiceRef.current = enabled;
    setWslPreferencePending(true);
    setWslPreferenceError(null);
    const operation = applyWslScanPreference(enabled, {
      setEnabled: api.setWslScanEnabled,
      readEnabled: api.wslScanEnabled
    });
    wslPreferenceApplyRef.current = operation;
    void operation
      .then((appliedEnabled) => {
        if (sequence !== wslPreferenceSequenceRef.current) return;
        wslConfirmedChoiceRef.current = appliedEnabled;
        wslRequestedChoiceRef.current = appliedEnabled;
        setWslScanChoice(appliedEnabled);
      })
      .catch((error) => {
        if (sequence !== wslPreferenceSequenceRef.current) return;
        const message = error instanceof Error ? error.message : String(error);
        wslRequestedChoiceRef.current = wslConfirmedChoiceRef.current;
        setWslScanChoice(wslConfirmedChoiceRef.current);
        setWslPreferenceError(`WSL preference was not changed: ${message}`);
      })
      .finally(() => {
        if (wslPreferenceApplyRef.current === operation) wslPreferenceApplyRef.current = null;
        if (sequence === wslPreferenceSequenceRef.current) setWslPreferencePending(false);
      });
  }, []);

  const startWslGatedProjectDiscovery = useCallback(async <T,>(
    scope: WslGatedDiscoveryScope,
    start: (appliedEnabled: boolean) => Promise<T>
  ) => {
    const pendingPreference = wslPreferenceApplyRef.current;
    const sequence = wslPreferenceSequenceRef.current + 1;
    wslPreferenceSequenceRef.current = sequence;
    setWslPreferencePending(true);
    setWslPreferenceError(null);
    const requestedEnabled = wslRequestedChoiceRef.current;
    try {
      // A scan triggered immediately after a toggle observes that exact toggle.
      // If its persistence failed, this scan aborts instead of silently retrying
      // with an ambiguous scope.
      if (pendingPreference) await pendingPreference;
      const gated = await runWslGatedDiscovery({
        scope,
        requestedEnabled,
        port: {
          setEnabled: api.setWslScanEnabled,
          readEnabled: api.wslScanEnabled
        },
        start
      });
      wslConfirmedChoiceRef.current = gated.appliedEnabled;
      wslRequestedChoiceRef.current = gated.appliedEnabled;
      setWslScanChoice(gated.appliedEnabled);
      return gated.result;
    } catch (error) {
      if (isWslScanPreferenceApplyError(error)) {
        wslRequestedChoiceRef.current = wslConfirmedChoiceRef.current;
        setWslScanChoice(wslConfirmedChoiceRef.current);
        setWslPreferenceError(`WSL preference could not be applied: ${error.message}`);
      } else {
        // The sole gate only invokes `start` after verification. A downstream
        // discovery error therefore does not turn into a false preference error.
        wslConfirmedChoiceRef.current = requestedEnabled;
        wslRequestedChoiceRef.current = requestedEnabled;
        setWslScanChoice(requestedEnabled);
      }
      throw error;
    } finally {
      if (sequence === wslPreferenceSequenceRef.current) setWslPreferencePending(false);
    }
  }, []);

  // Probe which AI tools are installed on this PC (and the saved WSL-scan choice)
  // so the Deep Scan dialog lists only what's present. Pure existence checks — no
  // wsl.exe, no scan — safe at startup and refreshed whenever the dialog opens.
  useEffect(() => {
    void refreshInstalledApps();
    void refreshWslScanPreference();
  }, [refreshInstalledApps, refreshWslScanPreference]);

  useEffect(() => {
    if (!addProjectsVisible) return;
    void refreshInstalledApps();
    // Re-read on every open; the dialog never assumes its startup snapshot is
    // still the backend's effective WSL gate.
    void refreshWslScanPreference();
  }, [addProjectsVisible, refreshInstalledApps, refreshWslScanPreference]);

  useEffect(() => {
    window.localStorage.setItem(PERFORMANCE_MODE_STORAGE_KEY, performanceMode);
    void api.performanceSetMode(performanceMode).catch((error) => {
      setStatusText(`Performance mode update failed: ${error instanceof Error ? error.message : String(error)}`);
    });
  }, [performanceMode]);

  useLayoutEffect(() => {
    window.localStorage.setItem(THEME_MODE_STORAGE_KEY, themeMode);
    // Mirror the theme onto <html> so large surfaces behind the app shell
    // (body/root background) follow OLED dark instead of bleeding light.
    document.documentElement.setAttribute("data-theme", themeMode);
  }, [themeMode]);

  useEffect(() => {
    window.localStorage.setItem(ADVANCED_MODE_STORAGE_KEY, String(advancedMode));
  }, [advancedMode]);

  useEffect(() => {
    window.localStorage.setItem(SHOW_PROJECT_PATHS_STORAGE_KEY, String(showAllProjectPaths));
  }, [showAllProjectPaths]);

  useEffect(() => {
    primaryViewRef.current = primaryView;
  }, [primaryView]);
  useEffect(() => {
    selectedProjectIdRef.current = selectedProjectId;
  }, [selectedProjectId]);

  useEffect(() => {
    window.localStorage.setItem(SHOW_TOPBAR_NAV_STORAGE_KEY, String(showTopbarNav));
  }, [showTopbarNav]);

  useEffect(() => {
    window.localStorage.setItem(PROJECT_SORT_STORAGE_KEY, projectSort);
    window.localStorage.setItem(PROJECT_APP_FILTER_STORAGE_KEY, projectAppFilter);
    window.localStorage.setItem(PROJECT_STATUS_FILTER_STORAGE_KEY, projectStatusFilter);
    window.localStorage.setItem(SESSION_SORT_STORAGE_KEY, sessionSort);
    window.localStorage.setItem(SESSION_APP_FILTER_STORAGE_KEY, sessionAppFilter);
  }, [projectSort, projectAppFilter, projectStatusFilter, sessionSort, sessionAppFilter]);

  useEffect(() => {
    window.localStorage.setItem(DISCOVERY_INCLUDE_LOOSE_STORAGE_KEY, String(deepScanIncludeLoose));
  }, [deepScanIncludeLoose]);

  useEffect(() => {
    window.localStorage.setItem(DISCOVERY_INCLUDE_AGENTS_STORAGE_KEY, String(deepScanIncludeAgents));
  }, [deepScanIncludeAgents]);

  useEffect(() => {
    // Cache the latest discovery report so the next launch can hydrate the session
    // grouping + Active/Archived split instantly. The snapshot is inventory data
    // (project names, absolute paths, session-transcript paths), so it goes to the
    // DPAPI-protected backend store — NEVER plaintext localStorage
    // (SECURITY_INVARIANTS.md:42). Size-guarded so a huge inventory can't bloat it.
    if (!projectDiscoveryReport) return;
    try {
      const payload = JSON.stringify({ savedAt: Date.now(), report: projectDiscoveryReport });
      if (payload.length <= 3_500_000) {
        void api.cacheDiscoverySnapshot(payload);
      }
    } catch {
      // Serialization failure is non-fatal: the background rediscovery still rebuilds
      // the inventory on the next launch.
    }
  }, [projectDiscoveryReport]);

  useEffect(() => {
    if (
      !projectDiscoveryReport
      || projectDiscoveryLoading
      || sessionInventoryState !== "fresh"
      || sessionInventoryRefreshing
      || safeManagePromptShownRef.current
      || realProjectCount === 0
    ) {
      return;
    }

    let disposed = false;
    void api.safeManageFirstRunGet()
      .then(async (preference) => {
        if (disposed) return;
        setSafeManageFirstRunPreference(preference);
        if (
          !preference.suggestAfterDiscovery
          || preference.promptState === "completed"
          || preference.promptState === "suppressed"
        ) {
          safeManagePromptShownRef.current = true;
          return;
        }
        safeManagePromptShownRef.current = true;
        setSafeManageFirstRunOpen(true);
        const marked = await api.safeManageFirstRunSet(
          preference.suggestAfterDiscovery,
          preference.promptState,
          true
        );
        if (!disposed) setSafeManageFirstRunPreference(marked);
      })
      .catch((error) => {
        if (!disposed) {
          setStatusText(`Safe Manage first-run preference could not be loaded: ${error instanceof Error ? error.message : String(error)}`);
        }
      });

    return () => {
      disposed = true;
    };
  }, [
    projectDiscoveryLoading,
    projectDiscoveryReport,
    realProjectCount,
    sessionInventoryRefreshing,
    sessionInventoryState
  ]);

  useEffect(() => {
    // Preview content depends on the reveal/protected policy, so drop the cache
    // whenever it changes to avoid serving stale blocked/revealed text.
    previewCacheRef.current.clear();
  }, [previewPolicy]);

  useEffect(() => {
    window.localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(appearance));
  }, [appearance]);

  useEffect(() => {
    if (showDemoProjects === null) {
      window.localStorage.removeItem(SHOW_DEMO_PROJECTS_STORAGE_KEY);
      return;
    }
    window.localStorage.setItem(SHOW_DEMO_PROJECTS_STORAGE_KEY, String(showDemoProjects));
  }, [showDemoProjects]);

  useEffect(() => {
    // Dashboard totals and footprints follow the same demo-project visibility as
    // the sidebar. Drop the previous visibility snapshot so the next dashboard
    // load cannot briefly contradict the project list.
    dashboardAutoLoadAttemptedRef.current = false;
    setDashboard(null);
  }, [demosVisible]);

  const choosePerformanceMode = useCallback((mode: PerformanceMode) => {
    setPerformanceMode(mode);
    setStatusText(performanceStatusText(mode));
    setHoverHelp(performanceHelpText(mode));
  }, []);

  const loadSystemResourceProfile = useCallback(async () => {
    setResourceProfileLoading(true);
    setResourceProfileError(null);
    try {
      const profile = await api.systemResourceProfile();
      setResourceProfile(profile);
      setStatusText(`Resource profile loaded: ${profile.logicalCpuCount} logical CPU threads detected.`);
      setHoverHelp("This profile is local-only. It explains how Code Hangar maps Balanced, Priority and Max CPU to this PC.");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setResourceProfileError(message);
      setStatusText(`Resource profile failed: ${message}`);
    } finally {
      setResourceProfileLoading(false);
    }
  }, []);

  useEffect(() => {
    if (primaryView !== "settings" || settingsView !== "advanced" || resourceProfile || resourceProfileLoading) {
      return;
    }
    let cancelled = false;
    void afterFirstPaint().then(() => {
      if (!cancelled) void loadSystemResourceProfile();
    });
    return () => {
      cancelled = true;
    };
  }, [loadSystemResourceProfile, primaryView, resourceProfile, resourceProfileLoading, settingsView]);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void afterFirstPaint().then(() => {
        if (cancelled) return;
        api.recoveryPending()
          .then((pending) => {
            if (cancelled) return;
            setRecoveryState(pending);
            if (pending.pending) {
              setStatusText("Recovery required before any disk action can continue.");
              setBackgroundStatus("Interrupted operation journal detected.");
            }
          })
          .catch((error) => {
            if (cancelled) return;
            setStatusText(`Recovery check failed: ${error instanceof Error ? error.message : String(error)}`);
          });
      });
    }, 600);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, []);

  const updateHoverHelp = useCallback((event: { target: EventTarget | null }) => {
    if (!(event.target instanceof Element)) return;
    setHoverHelp(event.target.closest<HTMLElement>("[data-help]")?.dataset.help ?? null);
  }, []);

  const refreshHoverHelpAfterAction = useCallback((event: MouseEvent<HTMLElement>) => {
    const pointerInitiated = event.detail > 0;
    const { clientX, clientY } = event;
    window.requestAnimationFrame(() => {
      const element = pointerInitiated ? document.elementFromPoint(clientX, clientY) : null;
      const underlyingHelp = element?.closest<HTMLElement>("[data-help]")?.dataset.help;
      setHoverHelp(postActionHoverHelp(pointerInitiated, underlyingHelp));
    });
  }, []);

  const loadDashboardData = useCallback(async (quiet = false) => {
    if (dashboardLoading) return;
    setDashboardLoading(true);
    if (!quiet) {
      setBackgroundStatus("Loading dashboard, adapters and footprint summaries.");
    }
    try {
      const [dashboardSummary, adapterSummaries] = await Promise.all([
        api.dashboardSummary(demosVisible),
        api.adaptersList()
      ]);
      setDashboard(dashboardSummary);
      setAdapters(adapterSummaries);
    } catch (error) {
      setStatusText(`Dashboard refresh failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setDashboardLoading(false);
      if (!quiet) {
        setBackgroundStatus(null);
      }
    }
  }, [dashboardLoading, demosVisible]);

  const refreshSideData = useCallback(async () => {
    setBackgroundStatus("Refreshing sidebar metadata.");
    try {
      const [recent, pinned, scanRoots, protectedZones, securityStatus] = await Promise.all([
        api.recentItemsList(),
        api.pinnedItemsList(),
        api.rootsList(),
        api.zonesList(),
        api.securityStatus()
      ]);
      setRecentItems(recent);
      setPinnedItems(pinned);
      setRoots(scanRoots);
      setZones(protectedZones);
      setSecurity(securityStatus);
    } finally {
      setBackgroundStatus(null);
    }
  }, []);

  const refreshFinalRemovePreview = useCallback(async () => {
    if (!mutationAvailable) {
      setFinalRemovePreview(null);
      setFinalRemoveUnavailableReason(
        "This read-only installation can show recovery history but cannot run project or batch final cleanup."
      );
      return "journalBlocked" as const;
    }
    setFinalRemovePreviewLoading(true);
    setFinalRemoveUnavailableReason(null);
    let dashboardConfirmedIdle = false;
    try {
      const dashboard = await api.mutationRecoveryDashboard();
      if (!dashboard.available) {
        setFinalRemovePreview(null);
        setFinalRemoveUnavailableReason(
          dashboard.message || "The mutation recovery dashboard is unavailable, so Code Hangar cannot prove that no cleanup batch is active."
        );
        return "journalBlocked" as const;
      }
      const recoveryState = dashboard.finalRemove?.state;
      if (!(["idle", "active", "interrupted", "unknown"] as const).includes(recoveryState)) {
        throw new Error("The mutation recovery dashboard returned an invalid final-cleanup state. Another cleanup batch is blocked.");
      }
      const dashboardPhase = dashboard.finalRemove?.phase;
      if (dashboardPhase != null && !isFinalRemovePhase(dashboardPhase)) {
        throw new Error("The mutation recovery dashboard returned an unknown final-cleanup phase. Deletion state cannot be inferred, so another batch is blocked.");
      }
      if (recoveryState === "idle" && (
        dashboard.finalRemove.batchId != null
        || dashboard.finalRemove.jobId != null
        || dashboardPhase != null
      )) {
        throw new Error("The mutation recovery dashboard reported idle while retaining a batch, job or phase identity. The contradictory journal state blocks another cleanup batch.");
      }
      if (recoveryState !== "idle") {
        const identity = [dashboard.finalRemove.batchId, dashboard.finalRemove.jobId]
          .filter((value): value is string => Boolean(value))
          .join(" / ");
        const reason = dashboard.finalRemove.message
          || `The recovery dashboard reports final cleanup as ${recoveryState}.`;
        let reconciliationDetail = "";
        setFinalRemovePreview(null);
        setFinalRemoveExecutionUnknown(true);
        setMutationBusy(true);
        setFinalRemoveJobId(dashboard.finalRemove.jobId ?? null);
        setFinalRemoveBatchId(dashboard.finalRemove.batchId ?? null);
        setFinalRemoveProgress((current) => current
          && dashboard.finalRemove.batchId === current.batchId
          && dashboardPhase
          ? {
              ...current,
              phase: dashboardPhase
            }
          : null);
        if (dashboard.finalRemove.jobId && dashboard.finalRemove.batchId) {
          try {
            const status = await api.mutationFinalRemoveBatchStatus(dashboard.finalRemove.jobId);
            assertFinalRemoveBatchStatus(status, dashboard.finalRemove.batchId);
            setFinalRemoveProgress(status.progress);
            if (status.result) {
              setFinalRemoveResult(status.result);
              reconciliationDetail = " A terminal result is recorded, but cleanup remains blocked until the recovery dashboard itself reports idle.";
            }
          } catch (error) {
            reconciliationDetail = ` The persisted job could not be reconciled: ${error instanceof Error ? error.message : String(error)}`;
          }
        }
        setFinalRemoveUnavailableReason(
          `${reason}${identity ? ` Batch identity: ${identity}.` : ""}${reconciliationDetail} Another cleanup batch cannot start until the journal reports idle.`
        );
        return "journalBlocked" as const;
      }
      dashboardConfirmedIdle = true;
      if (finalRemoveExecutionUnknown) {
        setFinalRemoveExecutionUnknown(false);
        setMutationBusy(false);
        setFinalRemoveJobId(null);
        setFinalRemoveBatchId(null);
        setFinalRemoveProgress(null);
        setFinalRemoveError(null);
        setMutationMessage("The mutation recovery dashboard now reports no active or interrupted final-cleanup batch. Eligibility was refreshed before re-enabling cleanup.");
      }
      if (!finalRemoveEnabled) {
        setFinalRemovePreview(null);
        setFinalRemoveUnavailableReason(
          "Permanent removal is off. Held projects remain restorable until you explicitly enable the irreversible workflow above."
        );
        return "disabled" as const;
      }
      const preview = await api.mutationFinalRemovePreview({ kind: "allEligible" });
      assertFinalRemovePreviewContract(preview);
      setFinalRemovePreview(preview);
      return "ready" as const;
    } catch (error) {
      setFinalRemovePreview(null);
      const message = error instanceof Error ? error.message : String(error);
      if (!dashboardConfirmedIdle) {
        setFinalRemoveExecutionUnknown(true);
        setMutationBusy(true);
        setFinalRemoveUnavailableReason(
          `Code Hangar could not prove that the final-cleanup journal is idle: ${message} Another cleanup batch is blocked.`
        );
      } else {
        setFinalRemoveUnavailableReason(message);
      }
      return dashboardConfirmedIdle ? "journalIdlePreviewUnavailable" as const : "journalBlocked" as const;
    } finally {
      setFinalRemovePreviewLoading(false);
    }
  }, [finalRemoveEnabled, finalRemoveExecutionUnknown, mutationAvailable]);

  const refreshMutationActivity = useCallback(async () => {
    try {
      const [log, removals] = await Promise.all([
        api.mutationActivityLog(80),
        api.appRemovalsList()
      ]);
      setMutationActivity(log);
      setAppRemovals(removals);
      return true;
    } catch (error) {
      setMutationMessage(`Activity log failed: ${error instanceof Error ? error.message : String(error)}`);
      return false;
    }
  }, []);

  const refreshRecoveryData = useCallback(async () => {
    const [historyCurrent, cleanupCurrent] = await Promise.all([
      refreshMutationActivity(),
      refreshFinalRemovePreview()
    ]);
    return historyCurrent || cleanupCurrent === "ready" || cleanupCurrent === "disabled";
  }, [refreshFinalRemovePreview, refreshMutationActivity]);

  // Recover a persisted "remove from AI apps" from the Recover view (survives restarts,
  // unlike the in-session Undo). Restores the registry files, then refreshes.
  const restoreAppRemoval = useCallback(async (id: string, projectName: string) => {
    try {
      await api.appRemovalRestore(id);
      setStatusText(`Restored ${projectName} to its AI apps. Reopen the app to see it.`);
      // Only clear the in-session Undo banner if IT is the removal we just restored — restoring
      // an older entry from Recover must not silently drop the one-click Undo for a different,
      // still-removed project.
      setAppRemovalUndo((current) => (current?.id === id ? null : current));
      const removals = await api.appRemovalsList();
      setAppRemovals(removals);
    } catch (error) {
      setStatusText(`Could not restore: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void afterFirstPaint().then(() => {
        if (cancelled) return;
        Promise.all([
          api.mutationModeStatus(),
          api.mutationFinalRemoveEnabled()
        ])
          .then(([available, enabled]) => {
            if (cancelled) return;
            setMutationAvailable(available);
            setFinalRemoveEnabled(available && enabled);
            setFinalRemoveCapabilityLoading(false);
            if (available) {
              void refreshMutationActivity();
            }
          })
          .catch(() => {
            if (!cancelled) {
              setMutationAvailable(false);
              setFinalRemoveEnabled(false);
              setFinalRemoveCapabilityLoading(false);
            }
          });
      });
    }, 700);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [refreshMutationActivity]);

  useEffect(() => {
    void refreshFinalRemovePreview();
  }, [refreshFinalRemovePreview]);

  const setFinalRemoveCapability = useCallback(async (
    enabled: boolean,
    acknowledgement?: string | null
  ) => {
    if (!mutationAvailable) {
      setMutationMessage("Permanent removal is unavailable in this read-only installation.");
      return false;
    }
    if (finalRemoveExecutionUnknown) {
      setMutationMessage("Reconcile the existing final-cleanup journal before changing this capability.");
      return false;
    }
    setFinalRemoveCapabilityLoading(true);
    try {
      await api.mutationSetFinalRemoveEnabled(enabled, acknowledgement ?? null);
      const current = await api.mutationFinalRemoveEnabled();
      if (current !== enabled) {
        throw new Error("The backend did not persist the requested permanent-removal state.");
      }
      setFinalRemoveEnabled(current);
      setFinalRemoveReview(null);
      setFinalRemovePreview(null);
      setFinalRemoveError(null);
      setFinalRemoveUnavailableReason(current
        ? "Checking held objects against the current journal and verified recovery archives…"
        : "Permanent removal is off. Held projects remain restorable until you explicitly enable the irreversible workflow above.");
      setMutationMessage(current
        ? "Permanent removal is enabled for explicit reviews. Nothing was deleted; every operation still needs a fresh preview and confirmation."
        : "Permanent removal is off. Held projects and recovery archives remain available.");
      return true;
    } catch (error) {
      setMutationMessage(`Permanent-removal setting was not changed: ${error instanceof Error ? error.message : String(error)}`);
      return false;
    } finally {
      setFinalRemoveCapabilityLoading(false);
    }
  }, [finalRemoveExecutionUnknown, mutationAvailable]);

  const enterMutationMode = useCallback(async () => {
    setMutationBusy(true);
    setMutationMessage("Unlocking one short-lived disk action…");
    try {
      const result = await api.mutationTokenIssue("enter_mutation_mode");
      setMutationModeToken(result.token);
      setMutationMessage("One disk action is unlocked for this review. Choose either verified backup or move to the recovery holding area; the token is used once.");
      setStatusText("One safe disk action is ready for confirmation.");
    } catch (error) {
      setMutationMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setMutationBusy(false);
    }
  }, []);

  const runMutationBackup = useCallback(async () => {
    if (!activeOperationPlan || !mutationModeToken) return;
    // Before copying secrets into the recovery backup, disclose protected files and links.
    // Links may later block the move/final-cleanup preview; this backup step never promises
    // that the source folder will become empty.
    if (mutationIncludeProtected) {
      try {
        const preview = await api.mutationPreviewProtected(activeOperationPlan);
        if (preview.protected.length === 0 && preview.reparse.length === 0) {
          setMutationMessage("This project has no sensitive/protected files or links to include; the standard backup already covers it.");
        } else {
          const sample = (list: string[]) => list.slice(0, 8).join("\n  ") + (list.length > 8 ? `\n  …and ${list.length - 8} more` : "");
          const parts: string[] = [];
          if (preview.protected.length > 0) {
            parts.push(`${preview.protected.length} SENSITIVE/PROTECTED file(s) — these will be COPIED into the backup folder (secrets included). A later holding-area preview decides which objects can move:\n  ${sample(preview.protected)}`);
          }
          if (preview.reparse.length > 0) {
            parts.push(`${preview.reparse.length} junction/symlink LINK(s) — these are listed for review and may block their subtree. Link targets are never followed or removed:\n  ${sample(preview.reparse)}`);
          }
          if (!(await requestConfirm(
            `Include protected content in this recovery backup?\n\n${parts.join("\n\n")}\n\nYour chosen backup folder will contain these secrets. This step does not delete or move source files. Continue?`,
            { confirmLabel: "Continue to backup", tone: "danger" }
          ))) {
            setMutationMessage("Backup cancelled. No source files were moved or deleted.");
            return;
          }
        }
      } catch (error) {
        setMutationMessage(`Could not preview the sensitive/protected files: ${error instanceof Error ? error.message : String(error)}`);
        return;
      }
    }
    const destination = await api.pickFolder("Choose verified backup destination");
    if (!destination) {
      setMutationMessage("Choose a backup destination folder before running backup.");
      return;
    }
    setMutationBusy(true);
    setMutationMessage("Creating and verifying the backup. Source files remain untouched…");
    try {
      const result = await api.mutationBackupStart(activeOperationPlan, destination, mutationBackupLevel, mutationAllowSameVolume, mutationIncludeProtected, mutationModeToken);
      setMutationModeToken(null);
      // Remember the verified content backup so the move to the recovery area is allowed
      // (Gate 3). Object-complete final-cleanup eligibility is proved separately by v2.
      setMutationBackupId(result.verified ? result.backupId : null);
      setMutationMessage(`Verified content backup ${result.backupId} wrote ${formatBytes(result.totalBytes)} across ${result.itemCount} item${result.itemCount === 1 ? "" : "s"}. Final cleanup still requires separate object-archive-v2 proof.`);
      setStatusText(`Verified content-backup manifest written: ${result.manifestPath}`);
      await refreshMutationActivity();
    } catch (error) {
      setMutationMessage(`Backup failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setMutationBusy(false);
    }
  }, [activeOperationPlan, mutationAllowSameVolume, mutationBackupLevel, mutationIncludeProtected, mutationModeToken, refreshMutationActivity, requestConfirm]);

  const runMutationMove = useCallback(async () => {
    if (!activeOperationPlan || !mutationModeToken) return;
    // Gate 3: never move to the recovery area without a verified backup of this plan.
    if (mutationBackupId == null) {
      setMutationMessage("Create a verified backup of this plan before moving files to the recovery area.");
      return;
    }
    const destination = await api.pickFolder("Choose recovery holding area");
    if (!destination) {
      setMutationMessage("Choose a holding folder before moving files.");
      return;
    }
    if (!(await requestConfirm(
      "This will move recoverable files out of the selected project into Code Hangar's recovery holding area. It is journaled and restorable, but it changes files on disk. Continue?",
      { confirmLabel: "Move to holding area", tone: "danger" }
    ))) {
      setMutationMessage("Move cancelled. No files were changed.");
      return;
    }
    setMutationBusy(true);
    setLastMutationMove(null);
    setMutationMessage("Moving verified items into the recovery holding area and writing restore records…");
    try {
      const result = await api.mutationMoveStart(activeOperationPlan, destination, mutationBackupId, mutationIncludeProtected, mutationModeToken);
      setMutationModeToken(null);
      // A fresh backup is required before any subsequent move.
      setMutationBackupId(null);
      setLastMutationMove(result);
      setMutationMessage(`${result.moved} item${result.moved === 1 ? " was" : "s were"} moved to the recovery holding area; ${result.skipped} skipped and ${result.failed} failed. Source-volume effects recorded by the legacy journal are not a promise of current reclaimable space.`);
      setStatusText(`Move operation ${result.operationId} recorded in the journal.`);
      await Promise.all([refreshMutationActivity(), refreshSideData()]);
      // If this move came from a unified "Remove project" that also asked to forget the
      // project from Code Hangar, do it now that the folder has actually left the disk — but
      // ONLY when the move that just completed is for that exact project (the plan target),
      // so a stale deferral from an abandoned remove can never fire on an unrelated move.
      const pending = pendingPostMoveUnregister.current;
      if (pending != null && activeOperationPlan?.target.nodeId === pending.projectId) {
        pendingPostMoveUnregister.current = null;
        try {
          if (pending.rootId != null) await api.rootsUnregister(pending.rootId);
          else await api.projectsUnregister(pending.projectId);
          const loaded = await api.projectsList();
          setProjects(loaded);
          setProjectsFromCache(false);
          setStatusText("Supported project files were moved to holding and the project was forgotten from Code Hangar. Review Recovery & cleanup for anything blocked or still held.");
        } catch {
          // The folder move succeeded; forgetting from Code Hangar can be retried manually.
        }
      }
    } catch (error) {
      setMutationMessage(`Move failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setMutationBusy(false);
    }
  }, [activeOperationPlan, mutationBackupId, mutationIncludeProtected, mutationModeToken, refreshMutationActivity, refreshSideData, requestConfirm]);

  const runMutationRestore = useCallback(async (entryId: number) => {
    if (!(await requestConfirm(
      "Restore moves the stored file back to its original path if that path is free. Continue?",
      { confirmLabel: "Restore file" }
    ))) return;
    setMutationBusy(true);
    try {
      const token = (await api.mutationTokenIssue("enter_mutation_mode")).token;
      const result = await api.mutationRestoreStart(entryId, token);
      if (result.outcome === "conflict") {
        setMutationMessage(`Restore conflict: ${result.conflictPath ?? result.originalPath}. Choose Restore elsewhere or free the original path and retry.`);
      } else {
        setMutationMessage(`Restore ${result.outcome}: ${result.restoredPath ?? result.originalPath}`);
      }
      await Promise.all([refreshMutationActivity(), refreshSideData()]);
    } catch (error) {
      setMutationMessage(`Restore failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setMutationBusy(false);
    }
  }, [refreshMutationActivity, refreshSideData, requestConfirm]);

  const runMutationRestoreElsewhere = useCallback(async (entryId: number) => {
    const destination = await api.pickFolder("Choose a destination folder for restore");
    if (!destination) {
      setMutationMessage("Restore elsewhere cancelled.");
      return;
    }
    if (!(await requestConfirm(
      "Restore elsewhere moves the stored file under the folder you chose, preserving its stored relative path. It never overwrites an existing file. Continue?",
      { confirmLabel: "Restore elsewhere" }
    ))) return;
    setMutationBusy(true);
    try {
      const token = (await api.mutationTokenIssue("enter_mutation_mode")).token;
      const result = await api.mutationRestoreToFolderStart(entryId, destination, token);
      if (result.outcome === "conflict") {
        setMutationMessage(`Restore elsewhere conflict: ${result.conflictPath ?? destination}. Choose another destination folder.`);
      } else {
        setMutationMessage(`Restore elsewhere ${result.outcome}: ${result.restoredPath ?? destination}`);
      }
      await Promise.all([refreshMutationActivity(), refreshSideData()]);
    } catch (error) {
      setMutationMessage(`Restore elsewhere failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setMutationBusy(false);
    }
  }, [refreshMutationActivity, refreshSideData, requestConfirm]);

  const reviewFinalRemove = useCallback(async (scope: FinalRemoveScope) => {
    if (!finalRemoveEnabled) {
      setMutationMessage("Permanent removal is off. Explicitly enable it in Recovery before opening an irreversible cleanup review.");
      return;
    }
    if (finalRemoveExecutionUnknown) {
      setMutationMessage("A previous final-cleanup batch still needs reconciliation. No second batch can start.");
      return;
    }
    if (!finalRemovePreview) {
      setMutationMessage(finalRemoveUnavailableReason ?? "Final cleanup needs a current object-archive-v2 preview.");
      return;
    }
    setMutationMessage("Re-proving the cleanup journal is idle and loading a fresh preview for this exact selection…");
    if (await refreshFinalRemovePreview() !== "ready") return;
    setFinalRemovePreviewLoading(true);
    try {
      const scopedPreview = await api.mutationFinalRemovePreview(scope);
      assertFinalRemovePreviewContract(scopedPreview);
      if (Date.parse(scopedPreview.expiresAt) <= Date.now()) {
        throw new Error("The selection-specific final-cleanup preview is already expired. Refresh and try again; no deletion was submitted.");
      }
      const allowedGroupIds = scope.kind === "project"
        ? new Set([scope.groupId])
        : scope.kind === "groups"
          ? new Set(scope.groupIds)
          : null;
      if (allowedGroupIds && (
        scopedPreview.projects.some((project) => !allowedGroupIds.has(project.groupId))
        || scopedPreview.objects.some((object) => !allowedGroupIds.has(object.groupId))
      )) {
        throw new Error("The backend returned objects outside the requested final-cleanup scope. Nothing was selected for deletion.");
      }
      const heldObjects = scopedPreview.objects.filter((object) => object.lifecycle === "held");
      if (heldObjects.length === 0) {
        setMutationMessage("This fresh preview contains no held objects to review.");
        return;
      }
      const selectedGroupIds = allowedGroupIds
        ?? new Set(scopedPreview.projects.map((project) => project.groupId));
      const heldMembersByTopology = heldFinalRemoveMembersByTopology(scopedPreview.objects);
      const advertisedEligibleTopologyIds = new Set(scopedPreview.eligibleTopologyGroupIds);
      const advertisedEligibilityIsAtomic = scopedPreview.eligibleTopologyGroupIds.every((topologyGroupId) => {
        const heldMembers = heldMembersByTopology.get(topologyGroupId) ?? [];
        return heldMembers.length > 0
          && heldMembers.every((object) => selectedGroupIds.has(object.groupId) && isExplicitlyEligibleFinalRemoveObject(object));
      });
      const everyNonBlockedObjectIsAdvertised = heldObjects.every((object) => (
        !isExplicitlyEligibleFinalRemoveObject(object) || advertisedEligibleTopologyIds.has(object.topologyGroupId)
      ));
      if (!advertisedEligibilityIsAtomic || !everyNonBlockedObjectIsAdvertised) {
        throw new Error("The selection-specific preview has an inconsistent topology eligibility set. All held objects stay on disk.");
      }
      const candidateTopologyIds = new Set(heldObjects
        .filter((object) => selectedGroupIds.has(object.groupId) && isExplicitlyEligibleFinalRemoveObject(object))
        .map((object) => object.topologyGroupId));
      const safeTopologyIds = scopedPreview.eligibleTopologyGroupIds.filter((topologyGroupId) => {
        if (!candidateTopologyIds.has(topologyGroupId)) return false;
        const heldMembers = heldMembersByTopology.get(topologyGroupId) ?? [];
        return heldMembers.length > 0
          && heldMembers.every((object) => selectedGroupIds.has(object.groupId) && isExplicitlyEligibleFinalRemoveObject(object));
      });
      setMutationMessage(safeTopologyIds.length === 0
        ? "This selection is review-only: no topology group is eligible, so every held object remains on disk."
        : "Fresh selection-specific cleanup preview loaded. Review the exact objects, volume impact and retained archives before confirming.");
      setFinalRemoveReview({ preview: scopedPreview, scope });
      setFinalRemoveProgress(null);
      setFinalRemoveResult(null);
      setFinalRemoveError(null);
      setFinalRemoveJobId(null);
      setFinalRemoveBatchId(null);
    } catch (error) {
      setMutationMessage(`Final-cleanup review could not open: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setFinalRemovePreviewLoading(false);
    }
  }, [finalRemoveEnabled, finalRemoveExecutionUnknown, finalRemovePreview, finalRemoveUnavailableReason, refreshFinalRemovePreview]);

  const recordFinalRemoveResult = useCallback(async (result: FinalRemoveBatchResult) => {
    if (result.archiveRetained !== true) {
      throw new Error("The final-cleanup backend returned an incompatible result that does not prove recovery-archive retention. Review the journal before another disk action.");
    }
    setFinalRemoveResult(result);
    const unresolved = result.keptObjects + result.failedObjects;
    setMutationMessage(result.status === "completed"
      ? `Final cleanup deleted ${result.deletedObjects} held object${result.deletedObjects === 1 ? "" : "s"}. All recovery archives were kept.`
      : `Final cleanup is ${result.status}: ${result.deletedObjects} held object${result.deletedObjects === 1 ? "" : "s"} deleted and ${unresolved} remain held or need review. All recovery archives were kept.`);
    setStatusText(`Final-cleanup batch ${result.batchId}: ${result.status}.`);
    await Promise.all([refreshMutationActivity(), refreshSideData()]);
  }, [refreshMutationActivity, refreshSideData]);

  const runFinalRemoveBatch = useCallback(async (selectedTopologyGroupIds: string[]) => {
    const review = finalRemoveReview;
    if (!review || selectedTopologyGroupIds.length === 0 || finalRemoveExecutionUnknown) return;
    if (!finalRemoveEnabled) {
      setFinalRemoveError("Permanent removal was turned off after this review opened. Nothing was submitted for deletion.");
      return;
    }
    const selectedGroupIds = review.scope.kind === "project"
      ? new Set([review.scope.groupId])
      : review.scope.kind === "groups"
        ? new Set(review.scope.groupIds)
        : new Set(review.preview.projects.map((project) => project.groupId));
    const uniqueTopologyGroupIds = Array.from(new Set(selectedTopologyGroupIds));
    const eligibleTopologyGroupIds = new Set(review.preview.eligibleTopologyGroupIds);
    const heldMembersByTopology = heldFinalRemoveMembersByTopology(review.preview.objects);
    const selectionIsSafe = uniqueTopologyGroupIds.length === selectedTopologyGroupIds.length
      && uniqueTopologyGroupIds.every((topologyGroupId) => {
        if (!eligibleTopologyGroupIds.has(topologyGroupId)) return false;
        const heldMembers = heldMembersByTopology.get(topologyGroupId) ?? [];
        return heldMembers.length > 0
          && heldMembers.every((object) => selectedGroupIds.has(object.groupId) && isExplicitlyEligibleFinalRemoveObject(object));
      });
    if (!selectionIsSafe) {
      setFinalRemoveError("The selected topology groups no longer match the reviewed eligible objects. Refresh the preview; nothing was submitted for deletion.");
      return;
    }
    const reviewedPreviewExpiry = Date.parse(review.preview.expiresAt);
    if (!Number.isFinite(reviewedPreviewExpiry) || reviewedPreviewExpiry <= Date.now()) {
      setFinalRemoveError("The reviewed final-cleanup preview expired. Refresh eligibility and review the selection again; nothing was submitted for deletion.");
      return;
    }
    const selectedTopologyIdSet = new Set(uniqueTopologyGroupIds);
    const selectedObjectCount = review.preview.objects.filter((object) => (
      object.lifecycle === "held"
      && isExplicitlyEligibleFinalRemoveObject(object)
      && selectedTopologyIdSet.has(object.topologyGroupId)
    )).length;
    if (!Number.isSafeInteger(review.preview.maxDeleteObjects)
      || review.preview.maxDeleteObjects < 0
      || selectedObjectCount > review.preview.maxDeleteObjects) {
      setFinalRemoveError(
        Number.isSafeInteger(review.preview.maxDeleteObjects) && review.preview.maxDeleteObjects >= 0
          ? `Capacity blocked: this review selects ${selectedObjectCount} held objects, but the verified transport permits at most ${review.preview.maxDeleteObjects}. Review a smaller project or refresh for a narrower preview; nothing was submitted for deletion.`
          : "Capacity blocked: the preview has no valid maximum object count. Nothing was submitted for deletion."
      );
      return;
    }
    let batchStartInvoked = false;
    let preserveExecutionLock = false;
    setMutationBusy(true);
    setFinalRemoveError(null);
    setFinalRemoveProgress({
      batchId: "pending",
      phase: review.preview.requiresElevation ? "waitingForUac" : "verifyingArchives",
      total: selectedObjectCount,
      completed: 0,
      currentPath: null
    });
    setMutationMessage("Confirming this immutable final-cleanup preview. No legacy single-file deletion fallback will be used…");
    try {
      const confirmation = await api.mutationFinalRemoveConfirm(
        review.preview.previewId,
        review.preview.previewDigest,
        selectedTopologyGroupIds
      );
      if (confirmation.previewId !== review.preview.previewId
        || confirmation.previewDigest !== review.preview.previewDigest
        || !/^[0-9a-f]{64}$/u.test(confirmation.token)
        || !Number.isFinite(Date.parse(confirmation.expiresAt))
        || Date.parse(confirmation.expiresAt) <= Date.now()) {
        throw new Error("The confirmation capability is expired or bound to a different final-cleanup preview. Nothing was submitted for deletion.");
      }
      batchStartInvoked = true;
      const started = await api.mutationFinalRemoveBatchStart({
        previewId: review.preview.previewId,
        previewDigest: review.preview.previewDigest,
        selectedTopologyGroupIds,
        confirmationToken: confirmation.token
      });
      if (!started.jobId || !started.batchId) {
        throw new Error("The submitted final-cleanup batch did not return a complete immutable identity.");
      }
      setFinalRemoveJobId(started.jobId);
      setFinalRemoveBatchId(started.batchId);
      setFinalRemoveProgress((current) => current ? { ...current, batchId: started.batchId } : current);
      setMutationMessage(`Final-cleanup batch ${started.batchId} is running. Recovery archives remain in place.`);

      let completedResult: FinalRemoveBatchResult | null = null;
      for (let attempt = 0; attempt < 600; attempt += 1) {
        const status = await api.mutationFinalRemoveBatchStatus(started.jobId);
        assertFinalRemoveBatchStatus(status, started.batchId);
        setFinalRemoveProgress(status.progress);
        if (status.result) {
          completedResult = status.result;
          break;
        }
        await new Promise<void>((resolve) => window.setTimeout(resolve, 400));
      }
      if (!completedResult) {
        preserveExecutionLock = true;
        setFinalRemoveExecutionUnknown(true);
        // Keep the last backend-reported phase. A polling timeout proves only
        // that this renderer lost certainty; it does not prove that the durable
        // batch itself entered the backend's `interrupted` state.
        const message = `Final-cleanup batch ${started.batchId} did not report a terminal state. It may still be running; another cleanup batch is blocked until this job is reconciled.`;
        setFinalRemoveError(message);
        setMutationMessage(message);
        await refreshMutationActivity();
        return;
      }
      await recordFinalRemoveResult(completedResult);
      const refreshOutcome = await refreshFinalRemovePreview();
      preserveExecutionLock = refreshOutcome === "journalBlocked";
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (batchStartInvoked) {
        preserveExecutionLock = true;
        setFinalRemoveExecutionUnknown(true);
        setFinalRemoveError(`Batch status is unknown after submission: ${message}`);
        setMutationMessage("A submitted final-cleanup batch could not be reconciled. Another cleanup batch is blocked; recovery history remains available.");
        await refreshMutationActivity();
      } else {
        setFinalRemoveError(message);
        setMutationMessage(`Final cleanup did not start: ${message}`);
      }
    } finally {
      if (!preserveExecutionLock) {
        setFinalRemoveExecutionUnknown(false);
        setMutationBusy(false);
        setFinalRemoveJobId(null);
        setFinalRemoveBatchId(null);
      }
    }
  }, [finalRemoveEnabled, finalRemoveExecutionUnknown, finalRemoveReview, recordFinalRemoveResult, refreshFinalRemovePreview, refreshMutationActivity]);

  const stopFinalRemoveBatch = useCallback(async () => {
    if (!finalRemoveJobId || !finalRemoveBatchId) {
      setMutationMessage("Cancellation cannot be sent until the backend returns the immutable batch and job identity. No second cleanup batch can start meanwhile.");
      return;
    }
    try {
      await api.mutationFinalRemoveBatchStop(finalRemoveJobId);
      let status = await api.mutationFinalRemoveBatchStatus(finalRemoveJobId);
      assertFinalRemoveBatchStatus(status, finalRemoveBatchId);
      setFinalRemoveProgress(status.progress);
      if (!finalRemoveExecutionUnknown) {
        // The submitter loop owns terminal reconciliation for an in-session
        // batch. Returning here prevents duplicate history/preview refreshes if
        // Stop happened to race with the terminal status update.
        setMutationMessage(status.result
          ? "Stop reached a terminal boundary. Code Hangar is reconciling the exact batch result and retained archives."
          : "Stop accepted. Code Hangar will finish only the current object or inseparable topology group, then preserve every remaining held object and all recovery archives.");
        return;
      }
      // A job recovered from the dashboard no longer has the submitter's polling
      // loop. Keep reconciling that exact immutable identity after accepting Stop.
      for (let attempt = 0; !status.result && attempt < 600; attempt += 1) {
        await new Promise<void>((resolve) => window.setTimeout(resolve, 400));
        status = await api.mutationFinalRemoveBatchStatus(finalRemoveJobId);
        assertFinalRemoveBatchStatus(status, finalRemoveBatchId);
        setFinalRemoveProgress(status.progress);
      }
      if (status.result) {
        await recordFinalRemoveResult(status.result);
        const refreshOutcome = await refreshFinalRemovePreview();
        if (refreshOutcome !== "journalBlocked") {
          setFinalRemoveExecutionUnknown(false);
          setMutationBusy(false);
          setFinalRemoveJobId(null);
          setFinalRemoveBatchId(null);
        }
      } else {
        setFinalRemoveExecutionUnknown(true);
        setMutationMessage("Stop is still latched, but this recovered job has not yet reported a terminal journal state. Its last truthful phase remains visible and another cleanup batch stays blocked.");
      }
    } catch (error) {
      setFinalRemoveExecutionUnknown(true);
      setFinalRemoveError(`Could not reconcile the cancellation request: ${error instanceof Error ? error.message : String(error)}`);
      setMutationMessage("The final-cleanup job status remains unknown. Another cleanup batch is blocked until recovery reconciliation succeeds.");
    }
  }, [finalRemoveBatchId, finalRemoveExecutionUnknown, finalRemoveJobId, recordFinalRemoveResult, refreshFinalRemovePreview]);

  const closeFinalRemoveReview = useCallback(() => {
    if (mutationBusy && finalRemoveJobId && !finalRemoveExecutionUnknown && !finalRemoveResult) return;
    setFinalRemoveReview(null);
    if (finalRemoveExecutionUnknown || mutationBusy) return;
    setFinalRemoveProgress(null);
    setFinalRemoveError(null);
  }, [finalRemoveExecutionUnknown, finalRemoveJobId, finalRemoveResult, mutationBusy]);

  useEffect(() => {
    setMutationLockInspection(null);
  }, [preview?.path]);

  const inspectCurrentFileLock = useCallback(async () => {
    if (!preview?.path) return;
    setMutationLockLoading(true);
    try {
      const inspection = await api.mutationLockInspectPath(preview.path);
      setMutationLockInspection(inspection);
      setStatusText(`Lock inspector: ${inspection.state} for ${preview.displayName}.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setMutationLockInspection({ path: preview.path, state: "unavailable" });
      setStatusText(`Lock inspector unavailable: ${message}`);
    } finally {
      setMutationLockLoading(false);
    }
  }, [preview]);

  const resolveRecovery = useCallback(async (decision: "rollback") => {
    setRecoveryResolving(decision);
    try {
      const result = await api.recoveryResolve(decision);
      const refreshed = await api.recoveryPending();
      setRecoveryState(refreshed);
      setRecoveryFrozen(false);
      setStatusText(`${result.message} (${result.recoveredOperations} operation${result.recoveredOperations === 1 ? "" : "s"}, ${result.rolledBackItems} item${result.rolledBackItems === 1 ? "" : "s"} rolled back.)`);
      setBackgroundStatus(refreshed.pending ? "Recovery still has pending journal entries." : null);
      void refreshSideData();
      void refreshMutationActivity();
    } catch (error) {
      setStatusText(`Recovery failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setRecoveryResolving(null);
    }
  }, [refreshMutationActivity, refreshSideData]);

  const freezeRecovery = useCallback(() => {
    setRecoveryFrozen(true);
    setStatusText("Recovery frozen for this session. Read-only navigation remains available; disk actions stay blocked.");
    setBackgroundStatus("Recovery frozen. Resolve it before disk actions.");
  }, []);

  useEffect(() => {
    setDocumentSearchError(null);
  }, [documentKind, documentLimit, documentNameFilter, documentPathFilter, documentProjectInput, documentQuery, documentScope, selectedProjectId]);

  useEffect(() => {
    setOrphanSearchError(null);
  }, [lostKeyword, lostSignals, lostStalePreset, orphanAssetKind, orphanCustomMiB, orphanIncludePartial, orphanMinConfidence, orphanMinPreset, orphanMode, orphanScope, selectedProjectId]);

  useEffect(() => {
    setDuplicateSearchError(null);
  }, [duplicateCustomMiB, duplicateFileKind, duplicateLimit, duplicateMinPreset, duplicateScope, preview?.nodeId, selectedProjectId]);

  const runDocumentSearch = useCallback(async (overrides?: {
    scope?: DocumentSearchScope;
    kind?: "context" | "markdown" | "all";
    clearAdvanced?: boolean;
  }) => {
    const searchSeq = documentSearchSeq.current + 1;
    documentSearchSeq.current = searchSeq;
    const effectiveScope = overrides?.scope ?? documentScope;
    const effectiveKind = overrides?.kind ?? documentKind;
    const effectivePathFilter = overrides?.clearAdvanced ? "" : documentPathFilter;
    const effectiveNameFilter = overrides?.clearAdvanced ? "" : documentNameFilter;
    const effectiveProjectId = effectiveScope === "current"
      ? currentDocumentProjectAvailable ? selectedProjectId : null
      : effectiveScope === "project" && documentProjectResolution.kind === "resolved"
        ? documentProjectResolution.project.id
        : null;
    const effectiveCriteria = documentSearchCriteriaKey({
      query: documentQuery,
      scope: effectiveScope,
      projectId: effectiveProjectId,
      indexedKind: effectiveKind,
      pathFilter: effectivePathFilter,
      nameFilter: effectiveNameFilter,
      limit: documentLimit,
      includeFixtureProjects: demosVisible
    });
    if (documentQuery.trim().length < 2) {
      setDocumentSearching(false);
      setDocumentHits([]);
      setDocumentSearchRan(false);
      setDocumentSearchCriteria(null);
      setDocumentSearchTruncated(false);
      setDocumentSearchDuration(0);
      setDocumentSearchError("Enter at least two characters before searching indexed documents.");
      setStatusText("Enter at least two characters before searching indexed documents.");
      return;
    }
    if (effectiveScope === "current" && !currentDocumentProjectAvailable) {
      setDocumentSearching(false);
      setDocumentHits([]);
      setDocumentSearchRan(false);
      setDocumentSearchCriteria(null);
      setDocumentSearchTruncated(false);
      setDocumentSearchDuration(null);
      setDocumentSearchError("Choose an available project before searching Current project, or switch Scope to All projects.");
      setStatusText("Choose an available project before searching the current project, or switch scope to All projects.");
      return;
    }
    if (effectiveScope === "project" && documentProjectResolution.kind !== "resolved") {
      const reason = projectPickerInputStatus(documentProjectResolution)
        ?? "Choose one exact project name or path before searching.";
      setDocumentSearching(false);
      setDocumentHits([]);
      setDocumentSearchRan(false);
      setDocumentSearchCriteria(null);
      setDocumentSearchTruncated(false);
      setDocumentSearchDuration(null);
      setDocumentSearchError(reason);
      setStatusText(reason);
      return;
    }
    setDocumentHits([]);
    setDocumentSearchTruncated(false);
    setDocumentSearchDuration(null);
    setDocumentSearchError(null);
    setDocumentSearching(true);
    setDocumentSearchRan(true);
    setDocumentSearchCriteria(effectiveCriteria);
    await yieldToUi();
    try {
      const result = await api.searchDocuments({
        query: documentQuery,
        projectId: effectiveProjectId,
        indexedKind: effectiveKind,
        pathFilter: effectivePathFilter,
        nameFilter: effectiveNameFilter,
        limit: documentLimit,
        includeFixtureProjects: demosVisible,
        performanceMode
      });
      if (searchSeq !== documentSearchSeq.current) return;
      setDocumentHits(result.hits);
      setDocumentSearchTruncated(result.truncated);
      setDocumentSearchDuration(result.durationMs);
      setStatusText(`Document search returned ${result.hits.length}${result.truncated ? "+" : ""} hits${documentLimit === 0 ? " with unlimited result mode" : ""}.`);
    } catch (error) {
      if (searchSeq !== documentSearchSeq.current) return;
      const message = error instanceof Error ? error.message : String(error);
      setDocumentSearchRan(false);
      setDocumentSearchError(`Document search could not finish: ${message}`);
      setStatusText(`Document search failed: ${message}`);
    } finally {
      if (searchSeq === documentSearchSeq.current) setDocumentSearching(false);
    }
  }, [currentDocumentProjectAvailable, demosVisible, documentKind, documentLimit, documentNameFilter, documentPathFilter, documentProjectResolution, documentQuery, documentScope, performanceMode, selectedProjectId]);

  const startInventoryForRoots = useCallback(async (rootIds: number[]) => {
    if (rootIds.length === 0) return null;
    const jobId = await api.scanStart(rootIds, performanceMode);
    const status = await api.scanStatus(jobId);
    setScanStatus(status);
    return status;
  }, [performanceMode, setScanStatus]);

  const markSessionInventoryNeedsRefresh = useCallback(() => {
    setSessionInventoryState((current) => current === "restoring" || current === "unavailable" ? "unavailable" : "cached");
    setSessionInventoryError(null);
  }, []);

  const markDiscoveryCandidatesRegistered = useCallback((paths: ReadonlySet<string>, loadedProjects: ProjectSummary[] = []) => {
    setProjectDiscoveryReport((current) => current ? {
      ...current,
      candidates: current.candidates.map((item) => paths.has(item.path) ? {
        ...item,
        alreadyRegistered: true,
        existingProjectId: loadedProjects.find((project) => project.path === item.path)?.id ?? item.existingProjectId ?? null,
        sourceKinds: Array.from(new Set([...item.sourceKinds, "code_hangar_registered"])),
        signals: [
          ...item.signals.filter((signal) => signal.kind !== "already_registered"),
          {
            kind: "already_registered",
            label: "Already registered in Code Hangar",
            detail: null,
            confidence: "High"
          }
        ]
      } : item)
    } : current);
  }, []);

  const runProjectDiscovery = useCallback(async (
    limit = 100,
    kind: "projects" | "sessions" = "projects",
    includeTechnicalCandidates = false
  ) => {
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
    if (kind === "sessions") {
      setSessionInventoryRefreshing(true);
      setSessionInventoryError(null);
      setSessionInventoryState((current) => current === "unavailable" ? "restoring" : current);
    }
    setStatusText(kind === "sessions" ? "Finding local sessions… searching known folders and app/session metadata." : "Finding local projects… searching known folders and app/session metadata.");
    await yieldToUi();
    try {
      // The dedicated Sessions action is the complete conversation inventory:
      // include loose conversations and autonomous-agent chats (Hermes,
      // OpenClaw, NemoClaw) instead of silently applying the narrower project
      // discovery defaults. A zero limit keeps every bounded local result.
      const result = await api.projectDiscoveryReport(
        kind === "sessions" ? 0 : limit,
        kind === "sessions",
        kind === "sessions",
        includeTechnicalCandidates
      );
      setProjectDiscoveryReport(result);
      // Only "Find Sessions" refreshes the sidebar's session inventory.
      if (kind === "sessions") {
        setSessionInventory(result.sessions);
        setSessionInventoryState("fresh");
        setSessionInventoryError(null);
        persistInventoryIncludeOptions(true, true);
      }
      setStatusText(kind === "sessions"
        ? `Session discovery found ${result.totalSessions} local conversation${result.totalSessions === 1 ? "" : "s"}, including project-linked, standalone and agent sessions.`
        : `Project discovery found ${result.totalCandidates} project candidate${result.totalCandidates === 1 ? "" : "s"} and ${result.totalSessions} linked local session${result.totalSessions === 1 ? "" : "s"}.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setProjectDiscoveryError(message);
      if (kind === "sessions") {
        setSessionInventoryState((current) => current === "fresh" || current === "cached" ? "cached" : "unavailable");
        setSessionInventoryError(message);
      }
      setStatusText(`${kind === "sessions" ? "Session" : "Project"} discovery failed: ${message}`);
    } finally {
      if (kind === "sessions") setSessionInventoryRefreshing(false);
      setProjectDiscoveryLoading(false);
    }
  }, []);

  const addDiscoveryCandidateAsRoot = useCallback(async (candidate: ProjectDiscoveryCandidate) => {
    if (candidate.alreadyRegistered) {
      setStatusText(`${candidate.displayName} is already registered.`);
      return;
    }
    if (candidate.overlapKind !== "none") {
      setStatusText(`${candidate.displayName} overlaps an existing scan root. Resolve overlapping roots before adding it.`);
      return;
    }
    try {
      const root = await api.rootsAdd(candidate.path);
      markSessionInventoryNeedsRefresh();
      const addedPaths = new Set([candidate.path]);
      markDiscoveryCandidatesRegistered(addedPaths);
      setRoots((current) => current.some((item) => item.id === root.id) ? current : [...current, root]);
      let inventoryMessage = "Inventory scan started.";
      try {
        const status = await startInventoryForRoots([root.id]);
        inventoryMessage = status?.message ?? inventoryMessage;
      } catch (error) {
        inventoryMessage = `Inventory scan could not start: ${error instanceof Error ? error.message : String(error)}`;
      }
      let loadedProjects: ProjectSummary[];
      try {
        loadedProjects = await api.projectsListLite();
      } catch (error) {
        setStatusText(`${candidate.displayName} was added. ${inventoryMessage} Project navigation could not refresh yet: ${error instanceof Error ? error.message : String(error)}`);
        void refreshSideData();
        return;
      }
      setProjects(loadedProjects);
      setProjectsFromCache(false);
      markDiscoveryCandidatesRegistered(addedPaths, loadedProjects);
      setStatusText(`${candidate.displayName} added to Projects. ${inventoryMessage}`);
      void refreshSideData();
    } catch (error) {
      setStatusText(`Could not add discovered project: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [markDiscoveryCandidatesRegistered, markSessionInventoryNeedsRefresh, refreshSideData, startInventoryForRoots]);

  const addVisibleDiscoveryCandidatesAsRoots = useCallback(async (candidates: ProjectDiscoveryCandidate[]) => {
    const addable = candidates.filter((candidate) => !candidate.alreadyRegistered && candidate.overlapKind === "none");
    if (addable.length === 0) {
      setStatusText("No visible project candidates can be added. Registered and overlapping roots are skipped.");
      return;
    }
    const addedPaths = new Set<string>();
    const addedRoots: typeof roots = [];
    const failedCandidates: string[] = [];
    try {
      for (const candidate of addable) {
        try {
          const root = await api.rootsAdd(candidate.path);
          addedPaths.add(candidate.path);
          addedRoots.push(root);
        } catch {
          failedCandidates.push(candidate.displayName);
        }
      }
      if (addedRoots.length === 0) {
        setStatusText(`Could not add any visible project candidate${addable.length === 1 ? "" : "s"}. Review overlaps and local folder access, then retry.`);
        return;
      }
      markSessionInventoryNeedsRefresh();
      markDiscoveryCandidatesRegistered(addedPaths);
      setRoots((current) => {
        const known = new Set(current.map((root) => root.id));
        const next = [...current];
        for (const root of addedRoots) {
          if (!known.has(root.id)) next.push(root);
        }
        return next;
      });
      let inventoryMessage = "Inventory scan started.";
      try {
        const status = await startInventoryForRoots(addedRoots.map((root) => root.id));
        inventoryMessage = status?.message ?? inventoryMessage;
      } catch (error) {
        inventoryMessage = `Inventory scan could not start: ${error instanceof Error ? error.message : String(error)}`;
      }
      let loadedProjects: ProjectSummary[];
      try {
        loadedProjects = await api.projectsListLite();
      } catch (error) {
        setStatusText(`Added ${addedPaths.size} project candidate${addedPaths.size === 1 ? "" : "s"}. ${inventoryMessage} Project navigation could not refresh yet: ${error instanceof Error ? error.message : String(error)}`);
        void refreshSideData();
        return;
      }
      setProjects(loadedProjects);
      setProjectsFromCache(false);
      markDiscoveryCandidatesRegistered(addedPaths, loadedProjects);
      const failureNote = failedCandidates.length > 0
        ? ` ${failedCandidates.length} candidate${failedCandidates.length === 1 ? "" : "s"} could not be registered.`
        : "";
      setStatusText(`Added ${addedPaths.size} visible project candidate${addedPaths.size === 1 ? "" : "s"} to Projects. ${inventoryMessage}${failureNote}`);
      void refreshSideData();
    } catch (error) {
      setStatusText(`Could not add all visible projects: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [markDiscoveryCandidatesRegistered, markSessionInventoryNeedsRefresh, refreshSideData, startInventoryForRoots]);

  const runOrphanSearch = useCallback(async () => {
    if (orphanScope === "current" && selectedProjectId == null) {
      setOrphanSearchError("Choose a project before using Current project, or switch Scope to All projects.");
      setStatusText("Choose a project before running a Current project discovery search, or switch Scope to All projects.");
      return;
    }
    const searchSeq = orphanSearchSeq.current + 1;
    orphanSearchSeq.current = searchSeq;
    setOrphanSearchError(null);
    if (orphanMode === "lost") {
      setLostProjectCandidates(null);
      setLostSearchCriteria(currentOrphanSearchCriteria);
    } else {
      setOrphanCandidates(null);
      setAssetSearchCriteria(currentOrphanSearchCriteria);
    }
    setOrphanActiveSearchCriteria(currentOrphanSearchCriteria);
    setOrphanLoading(true);
    await yieldToUi();
    try {
      const projectId = orphanScope === "current" ? selectedProjectId : null;
      const minSizeBytes = sizePresetToBytes(orphanMinPreset, orphanCustomMiB);
      if (orphanMode === "lost") {
        const result = await api.lostProjectCandidates({
          minSizeBytes,
          projectId,
          stalePreset: lostStalePreset,
          signals: lostSignals,
          keyword: lostKeyword,
          includePartial: orphanIncludePartial,
          limit: 50,
          includeFixtureProjects: demosVisible,
          performanceMode
        });
        if (searchSeq !== orphanSearchSeq.current) return;
        setLostProjectCandidates(result);
        setStatusText(result.total > result.candidates.length
          ? `Forgotten Projects found ${result.total} review signals; showing the first ${result.candidates.length}.`
          : `Forgotten Projects found ${result.total} review signal${result.total === 1 ? "" : "s"}.`);
      } else {
        const result = await api.orphanAssetCandidates({
          minSizeBytes,
          projectId,
          assetKind: orphanAssetKind,
          minConfidence: orphanMinConfidence,
          includePartial: orphanIncludePartial,
          limit: 50,
          includeFixtureProjects: demosVisible,
          performanceMode
        });
        if (searchSeq !== orphanSearchSeq.current) return;
        setOrphanCandidates(result);
        setStatusText(result.total > result.candidates.length
          ? `Unreferenced Files found ${result.total} review signals; showing the first ${result.candidates.length}.`
          : `Unreferenced Files found ${result.total} review signal${result.total === 1 ? "" : "s"}.`);
      }
    } catch (error) {
      if (searchSeq !== orphanSearchSeq.current) return;
      const message = error instanceof Error ? error.message : String(error);
      setOrphanSearchError(`Search could not finish: ${message}`);
      setStatusText(`Orphan search failed: ${message}`);
    } finally {
      if (searchSeq === orphanSearchSeq.current) setOrphanLoading(false);
    }
  }, [currentOrphanSearchCriteria, demosVisible, lostKeyword, lostSignals, lostStalePreset, orphanAssetKind, orphanCustomMiB, orphanIncludePartial, orphanMinConfidence, orphanMinPreset, orphanMode, orphanScope, performanceMode, selectedProjectId]);

  // Run the orphan search once after a programmatic jump (e.g. "View orphans for
  // this project") has updated scope/mode/project in the same render batch.
  useEffect(() => {
    if (orphanAutoRunSeq > 0) {
      void runOrphanSearch();
    }
    // Intentionally keyed only on the trigger; runOrphanSearch captures fresh filters.
  }, [orphanAutoRunSeq]); // eslint-disable-line react-hooks/exhaustive-deps

  const toggleLostSignal = useCallback((signal: string) => {
    setLostSignals((current) => current.includes(signal)
      ? current.filter((item) => item !== signal)
      : [...current, signal]);
  }, []);

  const saveLostPreset = useCallback(() => {
    const name = lostPresetName.trim();
    if (!name) {
      setStatusText("Name the Lost Projects preset before saving it.");
      return;
    }
    const preset: LostPreset = {
      name,
      stalePreset: lostStalePreset,
      signals: lostSignals,
      keyword: lostKeyword,
      minPreset: orphanMinPreset,
      customMiB: orphanCustomMiB,
      includePartial: orphanIncludePartial
    };
    setSavedLostPresets((current) => [preset, ...current.filter((item) => item.name !== name)].slice(0, 12));
    setLostPresetName("");
    setStatusText(`Saved Lost Projects preset "${name}" locally.`);
  }, [lostKeyword, lostPresetName, lostSignals, lostStalePreset, orphanCustomMiB, orphanIncludePartial, orphanMinPreset]);

  const applyLostPreset = useCallback((name: string) => {
    const preset = savedLostPresets.find((item) => item.name === name);
    if (!preset) return;
    setLostStalePreset(preset.stalePreset);
    setLostSignals(preset.signals);
    setLostKeyword(preset.keyword);
    setOrphanMinPreset(preset.minPreset);
    setOrphanCustomMiB(preset.customMiB);
    setOrphanIncludePartial(preset.includePartial);
    setStatusText(`Applied Lost Projects preset "${name}".`);
  }, [savedLostPresets]);

  const loadDuplicateCandidates = useCallback(async (overrides: DuplicateSearchOverrides = {}) => {
    const scope = overrides.scope ?? duplicateScope;
    const currentFileNodeId = overrides.currentFileNodeId ?? preview?.nodeId ?? null;
    const minPreset = overrides.minPreset ?? duplicateMinPreset;
    const customMiB = overrides.customMiB ?? duplicateCustomMiB;
    const fileKind = overrides.fileKind ?? duplicateFileKind;
    const limit = overrides.limit ?? duplicateLimit;
    if (scope === "current" && selectedProjectId == null) {
      setDuplicateSearchError("Choose a project before using Current project, or switch Scope to All projects.");
      setStatusText("Choose a project before searching duplicate files for Current project, or switch Scope to All projects.");
      return;
    }
    if (scope === "file" && !currentFileNodeId) {
      setDuplicateSearchError("Open a file before searching duplicates for Current file.");
      setStatusText("Open a file before searching duplicates for the current file.");
      return;
    }
    const searchSeq = duplicateSearchSeq.current + 1;
    duplicateSearchSeq.current = searchSeq;
    const searchCriteria = duplicateSearchCriteriaKey({
      scope,
      projectId: selectedProjectId,
      currentFileNodeId,
      minPreset,
      customMiB,
      fileKind,
      limit,
      includeFixtureProjects: demosVisible
    });
    setDuplicateSearchError(null);
    setDuplicateCandidates(null);
    setDuplicateLoading(true);
    setDuplicateHasRun(true);
    setDuplicateSearchCriteria(searchCriteria);
    setDuplicateConfirmState((current) => retainRunningDuplicateConfirmations(current));
    await yieldToUi();
    try {
      const result = await api.duplicateCandidates({
        minSizeBytes: sizePresetToBytes(minPreset, customMiB),
        projectId: scope === "current" ? selectedProjectId : null,
        fileKind,
        currentFileNodeId: scope === "file" ? currentFileNodeId : null,
        limit,
        includeFixtureProjects: demosVisible,
        performanceMode
      });
      if (searchSeq !== duplicateSearchSeq.current) return;
      setDuplicateCandidates(result);
      const shown = result.groups.length;
      setStatusText(result.total > shown
        ? `Duplicate search found ${result.total} groups; showing the first ${shown}.`
        : `Duplicate search found ${shown} group${shown === 1 ? "" : "s"}.`);
    } catch (error) {
      if (searchSeq !== duplicateSearchSeq.current) return;
      const message = error instanceof Error ? error.message : String(error);
      setDuplicateSearchError(`Duplicate search could not finish: ${message}`);
      setStatusText(`Duplicate analysis failed: ${message}`);
    } finally {
      if (searchSeq === duplicateSearchSeq.current) setDuplicateLoading(false);
    }
  }, [demosVisible, duplicateCustomMiB, duplicateFileKind, duplicateLimit, duplicateMinPreset, duplicateScope, performanceMode, preview?.nodeId, selectedProjectId]);

  const evaluateCurrentFileOrphan = useCallback(async () => {
    if (!preview || preview.nodeId <= 0) {
      setStatusText(preview
        ? "Orphan analysis becomes available after the background index has registered this file."
        : "Open a file before evaluating orphan status.");
      return;
    }
    const searchSeq = fileOrphanSeq.current + 1;
    fileOrphanSeq.current = searchSeq;
    const expectedSelectionSeq = selectionSeq.current;
    const nodeId = preview.nodeId;
    const displayName = preview.displayName;
    setFileOrphanLoading(true);
    await yieldToUi();
    try {
      const status = await api.nodeOrphanStatus(preview.projectId, nodeId);
      if (searchSeq !== fileOrphanSeq.current || expectedSelectionSeq !== selectionSeq.current) return;
      setOrphanStatusByMembership((current) => ({
        ...current,
        [fileMembershipKey(preview.projectId, nodeId)]: status
      }));
      setStatusText(orphanReferenceStatusText(displayName, status));
    } catch (error) {
      if (searchSeq !== fileOrphanSeq.current || expectedSelectionSeq !== selectionSeq.current) return;
      setStatusText(`Orphan status failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      if (searchSeq === fileOrphanSeq.current && expectedSelectionSeq === selectionSeq.current) setFileOrphanLoading(false);
    }
  }, [preview]);

  // Declared before the first navigation callback that records history
  // (inspectCurrentFileDuplicates below) — everything else lives further down.
  const currentWorkspaceRoute = useCallback((): WorkspaceRoute => ({
    primaryView,
    projectView,
    discoverView,
    settingsView,
    rightPaneView,
    selectedProjectId: selectedProjectId != null && selectedProjectId > 0
      ? selectedProjectId
      : null
  }), [discoverView, primaryView, projectView, rightPaneView, selectedProjectId, settingsView]);

  const pushWorkspaceRoute = useCallback((
    next: Partial<WorkspaceRoute>,
    options?: { recordCurrent?: boolean }
  ) => {
    const current = currentWorkspaceRoute();
    const planned = { ...current, ...next };
    if (!shouldRecordWorkspaceRoute(current, planned, options?.recordCurrent)) return;
    // Composite flows (e.g. selectProject then showReview in one tick) push the same
    // origin route twice; collapsing consecutive duplicates keeps Back a single press.
    setViewBackStack((stack) => {
      const top = stack.at(-1);
      if (top && sameWorkspaceRoute(top, current)) return stack;
      return [...stack.slice(-39), current];
    });
    setViewForwardStack([]);
  }, [currentWorkspaceRoute]);

  useEffect(() => {
    if (!startupRouteResolved) return;
    window.localStorage.setItem(LAST_WORKSPACE_ROUTE_STORAGE_KEY, JSON.stringify(currentWorkspaceRoute()));
  }, [currentWorkspaceRoute, startupRouteResolved]);

  const inspectCurrentFileDuplicates = useCallback(async () => {
    invalidateShellOpenIntent();
    if (!preview || preview.nodeId <= 0) {
      setStatusText(preview
        ? "Duplicate analysis becomes available after the background index has registered this file."
        : "Open a file before searching duplicates for it.");
      return;
    }
    setDuplicateScope("file");
    setDuplicateMinPreset("0");
    setDuplicateFileKind("all");
    setDuplicateLimit(25);
    // Record the origin screen so Back returns to it.
    pushWorkspaceRoute({ primaryView: "discover", discoverView: "duplicates", rightPaneView: "duplicates" });
    setPrimaryView("discover");
    setDiscoverView("duplicates");
    setRightPaneView("duplicates");
    await loadDuplicateCandidates({
      scope: "file",
      minPreset: "0",
      fileKind: "all",
      limit: 25,
      currentFileNodeId: preview.nodeId
    });
  }, [invalidateShellOpenIntent, loadDuplicateCandidates, preview, pushWorkspaceRoute]);

  const buildPreviewPlan = useCallback(async (explicitNodeId?: number) => {
    if (planLoading) {
      setStatusText("A Safe Manage review is already loading. Stop it before starting another one.");
      return;
    }
    const targetNodeId = explicitNodeId ?? planTargetNode?.nodeId ?? selectedProjectId;
    if (!targetNodeId) {
      setStatusText("Select a project before loading a Safe Manage review.");
      return;
    }
    if (preparedSafeManageDecision) {
      const preparedTargetNodeId = preparedSafeManageDecision.target?.nodeId
        ?? preparedSafeManageDecision.projectId;
      if (preparedSafeManageDecision.decision === "clean_regenerables" && !preparedSafeManageDecision.target) {
        setStatusText("Regenerable cleanup never targets the whole project. Choose an exact build, cache or dependency folder from Safe Manage, then load its OperationPlan.");
        return;
      }
      if (preparedTargetNodeId !== targetNodeId) {
        setStatusText("The selected Safe Manage target changed. Return to Safe Manage and choose the current exact project or regenerable folder again.");
        return;
      }
    }
    setPlanLoading(true);
    setPlanJobStatus(null);
    setOperationPlan(null);
    setRiskReport(null);
    setLastMutationMove(null);
    try {
      const jobId = preparedSafeManageDecision
        ? await api.safeManageOperationPlanStart({
            projectId: preparedSafeManageDecision.projectId,
            analysisRunId: preparedSafeManageDecision.analysisRunId,
            evidenceRevision: preparedSafeManageDecision.evidenceRevision,
            decision: preparedSafeManageDecision.decision,
            target: preparedSafeManageDecision.target ?? null
          }, performanceMode)
        : await api.operationPlanStart(targetNodeId, "Read-only local review", performanceMode);
      setPlanJobId(jobId);
      setStatusText("Safe Manage review started. You can keep using the UI or stop the load.");
    } catch (error) {
      setStatusText(`Safe Manage review failed: ${error instanceof Error ? error.message : String(error)}`);
      setPlanLoading(false);
    }
  }, [performanceMode, planLoading, planTargetNode, preparedSafeManageDecision, selectedProjectId]);

  const cancelPreviewPlan = useCallback(async () => {
    if (!planJobId) return;
    try {
      await api.operationPlanCancel(planJobId);
      setStatusText("Stopping Safe Manage review.");
      setPlanJobStatus((current) => current ? { ...current, state: "cancelling", message: "Stopping review load." } : current);
    } catch (error) {
      setStatusText(`Could not stop Safe Manage review: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [planJobId]);

  useEffect(() => {
    if (!planJobId) return;
    let stopped = false;
    let timer: number | undefined;

    const poll = async () => {
      try {
        const status = await api.operationPlanStatus(planJobId);
        if (stopped) return;
        setPlanJobStatus(status);
        if (status.state === "completed") {
          setOperationPlan(status.plan ?? null);
          setRiskReport(status.report ?? null);
          setPlanLoading(false);
          setPlanJobId(null);
          setStatusText(status.plan
            ? `Safe Manage review loaded for ${status.plan.target.displayName}. No project files were changed.`
            : "Safe Manage review completed.");
          return;
        }
        if (status.state === "cancelled") {
          setPlanLoading(false);
          setPlanJobId(null);
          setStatusText("Safe Manage review stopped.");
          return;
        }
        if (status.state === "failed") {
          setPlanLoading(false);
          setPlanJobId(null);
          setStatusText(`Safe Manage review failed: ${status.error ?? status.message}`);
          return;
        }
        timer = window.setTimeout(poll, document.hidden ? 2_000 : 500);
      } catch (error) {
        if (stopped) return;
        setPlanLoading(false);
        setPlanJobId(null);
        setStatusText(`Safe Manage review status failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    };

    void poll();
    return () => {
      stopped = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [planJobId]);

  const exportRiskReport = useCallback(async () => {
    setReportLoading(true);
    try {
      const report = activeRiskReport ?? (activeOperationPlan ? await api.riskReportBuild(activeOperationPlan, performanceMode) : null);
      if (!report) {
        setStatusText("Load a Safe Manage review before exporting its JSON report.");
        return;
      }
      const path = await api.pickReportPath();
      if (!path) {
        setStatusText("Risk report export cancelled.");
        return;
      }
      const result = await api.riskReportExport(report, path);
      setRiskReport(report);
      setStatusText(`Risk report exported to ${result.path} (${formatBytes(result.bytesWritten)}).`);
    } catch (error) {
      setStatusText(`Risk report export failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setReportLoading(false);
    }
  }, [activeOperationPlan, activeRiskReport, performanceMode]);

  const loadProjects = useCallback(async () => {
    const loaded = await api.projectsList();
    setProjects(loaded);
    setProjectsFromCache(false);
    if (selectedProjectId === null) {
      const nextProjectId = visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
      beginProject(nextProjectId);
      setSelectedProjectId(nextProjectId);
    }
  }, [beginProject, selectedProjectId, showDemoProjects]);

  const loadProjectsLite = useCallback(async () => {
    const loaded = await api.projectsListLite();
    setProjects(loaded);
    setProjectsFromCache(false);
    if (selectedProjectId === null) {
      const nextProjectId = visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
      beginProject(nextProjectId);
      setSelectedProjectId(nextProjectId);
    }
    return loaded;
  }, [beginProject, selectedProjectId, showDemoProjects]);

  const refreshAfterScanFinish = useCallback(async () => {
    setBackgroundStatus("Refreshing scan state.");
    previewCacheRef.current.clear();
    setGraphMapRevision((current) => current + 1);
    try {
      await loadProjects();
      await yieldToUi();
      await refreshSideData();
      await yieldToUi();
      window.setTimeout(() => {
        void (async () => {
          try {
            if (primaryView === "overview" || rightPaneView === "dashboard") {
              dashboardAutoLoadAttemptedRef.current = false;
              await loadDashboardData(true);
            }
            if (selectedProjectId) {
              await loadProjectData(selectedProjectId, false);
            }
          } catch (error) {
            setStatusText(`Background scan refresh failed: ${error instanceof Error ? error.message : String(error)}`);
          } finally {
            setBackgroundStatus(null);
          }
        })();
      }, 250);
    } catch (error) {
      setBackgroundStatus(null);
      setStatusText(`Scan refresh failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [loadDashboardData, loadProjectData, loadProjects, primaryView, refreshSideData, rightPaneView, selectedProjectId]);

  const prepareDiscoverScope = useCallback((view: DiscoverView, projectId: number | null) => {
    if (view === "search") {
      setDocumentScope((current) => scopeForDocumentSearchEntry(current, projectId));
      return;
    }
    if (view === "lost" || view === "assets") {
      setOrphanScope((current) => scopeForDiscoveryEntry(current, projectId));
      return;
    }
    if (view === "duplicates") {
      setDuplicateScope((current) => scopeForDiscoveryEntry(current, projectId));
    }
  }, []);

  const applyWorkspaceRoute = useCallback((route: WorkspaceRoute) => {
    invalidateShellOpenIntent();
    const safeRoute = route.selectedProjectId != null && route.selectedProjectId > 0
      ? route
      : { ...route, selectedProjectId: null };
    // Routes describe the workspace beneath any open session pane; navigating
    // history must dismiss the session or the center pane would keep showing it.
    setPreviewSession(null);
    if (safeRoute.selectedProjectId !== selectedProjectId) {
      if (planJobId) void api.operationPlanCancel(planJobId);
      selectionSeq.current += 1;
      beginProject(safeRoute.selectedProjectId);
      manualPreviewClearProjectRef.current = null;
      setFolderExplanation(null);
      setRelationships(null);
      setRelationshipsMembership(null);
      setRelationshipsLoading(false);
      setFileOrphanLoading(false);
      setPlanTargetNode(null);
      setOperationPlan(null);
      setRiskReport(null);
      setPlanJobId(null);
      setPlanJobStatus(null);
      setPlanLoading(false);
      setPreview(null);
      setTabs([]);
      setBackStack([]);
      setForwardStack([]);
      setSelectedProjectId(safeRoute.selectedProjectId);
    }
    setPrimaryView(safeRoute.primaryView);
    setProjectSidebarFocus(safeRoute.primaryView === "project" || safeRoute.primaryView === "review");
    setProjectView(safeRoute.projectView);
    setDiscoverView(safeRoute.discoverView);
    if (safeRoute.primaryView === "discover") {
      prepareDiscoverScope(safeRoute.discoverView, safeRoute.selectedProjectId);
    }
    if (safeRoute.discoverView === "lost" || safeRoute.discoverView === "assets") {
      setOrphanMode(safeRoute.discoverView);
    }
    setSettingsView(safeRoute.settingsView);
    setRightPaneView(safeRoute.rightPaneView);
    setStatusText(workspaceRouteStatusText(safeRoute));
  }, [beginProject, invalidateShellOpenIntent, planJobId, prepareDiscoverScope, selectedProjectId]);

  const selectProject = useCallback((
    projectId: number,
    options?: { preserveShellIntent?: boolean }
  ) => {
    if (!options?.preserveShellIntent) invalidateShellOpenIntent();
    setPreparedSafeManageDecision(null);
    setPreviewSession(null);
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({
      primaryView: "project",
      projectView: "context",
      rightPaneView: "inspector",
      selectedProjectId: projectId > 0 ? projectId : null
    });
    if (projectId === selectedProjectId) {
      setPrimaryView("project");
      setProjectView("context");
      setRightPaneView("inspector");
      const activation = selectedProjectActivation(projectWorkspace.loadStatus);
      if (activation === "reuse") {
        setStatusText("Returned to this project's context. No reload was needed.");
      } else if (activation === "wait") {
        setStatusText("Returned to this project's context. The project is still loading.");
      } else {
        setStatusText("Returned to this project's context. Retrying the local project load.");
        void loadProjectData(projectId);
      }
      return;
    }
    if (planJobId) void api.operationPlanCancel(planJobId);
    selectionSeq.current += 1;
    beginProject(projectId);
    manualPreviewClearProjectRef.current = null;
    setFolderExplanation(null);
    setRelationships(null);
    setRelationshipsMembership(null);
    setRelationshipsLoading(false);
    setFileOrphanLoading(false);
    setPlanTargetNode(null);
    setOperationPlan(null);
    setRiskReport(null);
    setPlanJobId(null);
    setPlanJobStatus(null);
    setPlanLoading(false);
    setPreview(null);
    setTabs([]);
    setBackStack([]);
    setForwardStack([]);
    setSelectedProjectId(projectId);
    setPrimaryView("project");
    setProjectView("context");
    setRightPaneView("inspector");
    setStatusText("Project selected. Loading project context.");
  }, [beginProject, invalidateShellOpenIntent, loadProjectData, planJobId, projectWorkspace.loadStatus, pushWorkspaceRoute, selectedProjectId]);

  const handleProjectSearchKeyDown = useCallback((event: ReactKeyboardEvent<HTMLInputElement>) => {
    const action = projectSearchKeyAction(event.key, projectQuery, orderedDisplayedProjects.all.length);
    if (action === "none") return;
    event.preventDefault();
    if (action === "clear") {
      setProjectQuery("");
      setStatusText("Project search cleared.");
      return;
    }
    const target = orderedDisplayedProjects.all[0];
    if (!target) return;
    selectProject(target.id);
    setStatusText(`Opened ${target.name} from project search.`);
  }, [orderedDisplayedProjects.all, projectQuery, selectProject]);

  const showOverview = useCallback((options?: { preserveShellIntent?: boolean }) => {
    if (!options?.preserveShellIntent) invalidateShellOpenIntent();
    dashboardAutoLoadAttemptedRef.current = false;
    pushWorkspaceRoute({ primaryView: "overview", rightPaneView: "dashboard" });
    setPrimaryView("overview");
    setRightPaneView("dashboard");
    setStatusText(workspaceRouteStatusText({ primaryView: "overview" }));
  }, [invalidateShellOpenIntent, pushWorkspaceRoute]);

  const showProjectWorkspace = useCallback((view: ProjectView = "context") => {
    invalidateShellOpenIntent();
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({ primaryView: "project", projectView: view, rightPaneView: "inspector" });
    setPrimaryView("project");
    setProjectView(view);
    setRightPaneView("inspector");
    setStatusText(workspaceRouteStatusText({ primaryView: "project", projectView: view }));
  }, [invalidateShellOpenIntent, pushWorkspaceRoute]);

  const openProjectRecap = useCallback((projectId: number) => {
    selectProject(projectId);
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({
      primaryView: "project",
      projectView: "recap",
      rightPaneView: "inspector",
      selectedProjectId: projectId
    });
    setPrimaryView("project");
    setProjectView("recap");
    setRightPaneView("inspector");
    setStatusText(workspaceRouteStatusText({ primaryView: "project", projectView: "recap" }));
  }, [pushWorkspaceRoute, selectProject]);

  const showDiscover = useCallback((view: DiscoverView) => {
    invalidateShellOpenIntent();
    const rightPaneView: RightPaneView = view === "search" || view === "projects" ? "search" : view === "duplicates" ? "duplicates" : view === "organize" ? "organize" : "orphans";
    prepareDiscoverScope(view, selectedProjectId);
    pushWorkspaceRoute({
      primaryView: "discover",
      discoverView: view,
      rightPaneView
    });
    setPrimaryView("discover");
    setDiscoverView(view);
    setStatusText(workspaceRouteStatusText({ primaryView: "discover", discoverView: view }));
    if (view === "search" || view === "projects") {
      setRightPaneView("search");
      return;
    }
    if (view === "duplicates") {
      setRightPaneView("duplicates");
      return;
    }
    if (view === "organize") {
      setRightPaneView("organize");
      return;
    }
    setOrphanMode(view);
    setRightPaneView("orphans");
  }, [invalidateShellOpenIntent, prepareDiscoverScope, pushWorkspaceRoute, selectedProjectId]);

  const showSafeManage = useCallback(() => {
    invalidateShellOpenIntent();
    pushWorkspaceRoute({ primaryView: "safe_manage", rightPaneView: "plan" });
    setPrimaryView("safe_manage");
    setProjectSidebarFocus(false);
    setRightPaneView("plan");
    setStatusText(workspaceRouteStatusText({ primaryView: "safe_manage" }));
  }, [invalidateShellOpenIntent, pushWorkspaceRoute]);

  const persistSafeManageFirstRunPreference = useCallback(async (
    suggestAfterDiscovery: boolean,
    promptState: SafeManageFirstRunPreference["promptState"],
    markPromptedNow: boolean
  ) => {
    const preference = await api.safeManageFirstRunSet(
      suggestAfterDiscovery,
      promptState,
      markPromptedNow
    );
    // Reflect the durable choice before Analyze now starts. A later start
    // failure must not make the already-saved preference look unsaved.
    setSafeManageFirstRunPreference(preference);
    return preference;
  }, []);

  const postponeSafeManageFirstRun = useCallback(async () => {
    setSafeManageFirstRunOpen(false);
    try {
      const outcome = await applySafeManageFirstRunChoice("later", safeManageFirstRunPreference, {
        savePreference: persistSafeManageFirstRunPreference,
        startAnalysis: api.safeManageAnalysisStart
      });
      setSafeManageFirstRunPreference(outcome.preference);
      setStatusText("Safe Manage analysis postponed. You can run it from the main menu at any time.");
    } catch (error) {
      setStatusText(`Safe Manage preference could not be saved: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [persistSafeManageFirstRunPreference, safeManageFirstRunPreference]);

  const suppressSafeManageFirstRun = useCallback(async () => {
    setSafeManageFirstRunOpen(false);
    try {
      const outcome = await applySafeManageFirstRunChoice("suppress", safeManageFirstRunPreference, {
        savePreference: persistSafeManageFirstRunPreference,
        startAnalysis: api.safeManageAnalysisStart
      });
      setSafeManageFirstRunPreference(outcome.preference);
      setStatusText("Automatic Safe Manage suggestions are off. Manual analysis remains available at any time.");
    } catch (error) {
      setStatusText(`Safe Manage preference could not be saved: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [persistSafeManageFirstRunPreference, safeManageFirstRunPreference]);

  const analyzeSafeManageFirstRun = useCallback(async () => {
    setSafeManageFirstRunOpen(false);
    try {
      // Start persists the queued run before it resolves. Navigate only after
      // that point so SafeManagePortfolioView cannot mount, observe no job and
      // miss the run id that it needs to poll.
      const outcome = await applySafeManageFirstRunChoice("analyze_now", safeManageFirstRunPreference, {
        savePreference: persistSafeManageFirstRunPreference,
        startAnalysis: api.safeManageAnalysisStart
      });
      setSafeManageFirstRunPreference(outcome.preference);
      showSafeManage();
      setStatusText("Safe Manage is analyzing the current local project catalog. You can continue using the application.");
    } catch (error) {
      showSafeManage();
      setStatusText(`Safe Manage analysis could not start: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [persistSafeManageFirstRunPreference, safeManageFirstRunPreference, showSafeManage]);

  const focusProjectPicker = useCallback(() => {
    if (displayedProjects.length === 0) {
      showDiscover("projects");
      setStatusText("Find local projects first, then choose one to understand.");
      return;
    }
    setProjectSidebarFocus(false);
    setPaneCollapsed((current) => ({ ...current, left: false }));
    setSidebarCollapsed((current) => ({ ...current, projects: false }));
    setProjectQuery("");
    setStatusText("Choose a project from the sidebar, or type to filter the list.");
    const focusSearch = () => {
      const input = projectSearchInputRef.current;
      if (!input) return;
      input.scrollIntoView({ block: "center", inline: "nearest" });
      input.focus();
      input.select();
    };
    if (typeof window === "undefined" || typeof window.requestAnimationFrame !== "function") {
      focusSearch();
      return;
    }
    window.requestAnimationFrame(() => window.requestAnimationFrame(focusSearch));
  }, [displayedProjects.length, showDiscover]);

  const openReviewWorkspace = useCallback(() => {
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({ primaryView: "review", rightPaneView: "inspector" });
    setPrimaryView("review");
    setRightPaneView("inspector");
    setStatusText(workspaceRouteStatusText({ primaryView: "review" }));
  }, [pushWorkspaceRoute]);

  const showReview = useCallback(() => {
    invalidateShellOpenIntent();
    setPreparedSafeManageDecision(null);
    openReviewWorkspace();
  }, [invalidateShellOpenIntent, openReviewWorkspace]);

  const prepareSafeManageDecision = useCallback((
    assessment: SafeManageProjectAssessment,
    decision: Exclude<SafeManageDecisionKind, "keep" | "ignore" | "request_deeper_review">,
    target?: SafeManageRegenerableTarget
  ) => {
    invalidateShellOpenIntent();
    selectProject(assessment.projectId);
    setPlanTargetNode(target ? {
      nodeId: target.nodeId,
      label: target.path,
      kind: target.kind
    } : null);
    setPreparedSafeManageDecision({
      projectId: assessment.projectId,
      decision,
      analysisRunId: assessment.analysisRunId,
      evidenceRevision: assessment.evidenceRevision,
      target: target ? {
        navId: target.navId,
        nodeId: target.nodeId,
        path: target.path
      } : null
    });
    openReviewWorkspace();
    const label = decision === "archive"
      ? "archive"
      : decision === "clean_regenerables"
        ? "regenerable cleanup"
        : "removal preparation";
    setStatusText(target
      ? `${assessment.projectName}: exact ${target.path} ${label} selected. Load the current OperationPlan and Risk Report before any action can continue.`
      : `${assessment.projectName}: ${label} selected. Load the current OperationPlan and Risk Report before any action can continue.`);
  }, [invalidateShellOpenIntent, openReviewWorkspace, selectProject]);

  // --- First-run guided tour ---------------------------------------------------
  // Use the current project when possible, then the first real project, and only
  // fall back to a built-in example when the inventory is empty.
  const selectTourExample = useCallback(() => {
    const current = selectedProjectId == null
      ? undefined
      : projects.find((project) => project.id === selectedProjectId);
    const example = current ?? projects.find((project) => !isDemoProject(project)) ?? projects[0];
    if (example) {
      selectProject(example.id);
    }
  }, [projects, selectProject, selectedProjectId]);

  const startTour = useCallback(() => {
    tourReplayOriginRef.current = {
      route: currentWorkspaceRoute(),
      backStack: [...viewBackStack],
      forwardStack: [...viewForwardStack],
      paneCollapsed: { ...paneCollapsed }
    };
    setAddProjectsVisible(false);
    setPaneCollapsed((current) => ({ ...current, left: false }));
    setTourMode("replay");
  }, [currentWorkspaceRoute, paneCollapsed, viewBackStack, viewForwardStack]);

  const closeReplayTour = useCallback((status: string) => {
    const origin = tourReplayOriginRef.current;
    tourReplayOriginRef.current = null;
    setTourMode(null);
    if (origin) {
      applyWorkspaceRoute(origin.route);
      setViewBackStack(origin.backStack);
      setViewForwardStack(origin.forwardStack);
      setPaneCollapsed(origin.paneCollapsed);
    }
    setStatusText(status);
  }, [applyWorkspaceRoute]);

  const finishTour = useCallback(() => {
    window.localStorage.setItem(tutorialStorageKey, "1");
    if (tourMode === "replay") {
      closeReplayTour("Guided tour complete. Your project, filters and preferences were left unchanged.");
      return;
    }
    setTourMode(null);
    showOverview();
    if (!tourHasRealProjects) {
      setShowDemoProjects(false);
      setAddProjectsVisible(true);
    } else {
      setStatusText("Tutorial complete. Choose a project and start with What changed.");
    }
  }, [closeReplayTour, showOverview, tourHasRealProjects, tourMode]);

  const skipTour = useCallback(() => {
    window.localStorage.setItem(tutorialStorageKey, "1");
    if (tourMode === "replay") {
      closeReplayTour("Guided tour closed. Your project, filters and preferences were left unchanged.");
      return;
    }
    setTourMode(null);
    showOverview();
    if (!tourHasRealProjects) {
      setShowDemoProjects(false);
      setAddProjectsVisible(true);
    }
  }, [closeReplayTour, showOverview, tourHasRealProjects, tourMode]);

  // Connector copy lives in a Connector-only chunk. These base steps therefore
  // remain the only tutorial text physically present in the Local bundle.
  const tourSteps: TourStep[] = guidedTourStepCopy(tourMode ?? "first-run", tourHasRealProjects).map((copy) =>
    copy.selector === TOUR_SELECTORS.workspace || copy.selector === TOUR_SELECTORS.safeManage
      ? { ...copy, before: selectTourExample }
      : { ...copy }
  );

  const showRecovery = useCallback(() => {
    invalidateShellOpenIntent();
    pushWorkspaceRoute({ primaryView: "recovery", rightPaneView: "activity" });
    setPrimaryView("recovery");
    setRightPaneView("activity");
    setStatusText(workspaceRouteStatusText({ primaryView: "recovery" }));
    void refreshMutationActivity();
  }, [invalidateShellOpenIntent, pushWorkspaceRoute, refreshMutationActivity]);

  const showSettings = useCallback((view: SettingsView) => {
    invalidateShellOpenIntent();
    const nextRightPane = view === "protection" ? "zones" : "inspector";
    pushWorkspaceRoute({
      primaryView: "settings",
      settingsView: view,
      rightPaneView: nextRightPane
    });
    setPrimaryView("settings");
    setSettingsView(view);
    setRightPaneView(nextRightPane);
    setStatusText(workspaceRouteStatusText({ primaryView: "settings", settingsView: view }));
  }, [invalidateShellOpenIntent, pushWorkspaceRoute]);

  const startPaneResize = useCallback(
    (pane: "left" | "right") => (event: MouseEvent<HTMLDivElement>) => {
      event.preventDefault();
      const startX = event.clientX;
      const startLeft = paneWidths.left;
      const startRight = paneWidths.right;
      document.body.classList.add("is-resizing-pane");

      const onMove = (moveEvent: globalThis.MouseEvent) => {
        const delta = moveEvent.clientX - startX;
        setPaneWidths({
          left: pane === "left" ? clamp(startLeft + delta, 176, 460) : startLeft,
          right: pane === "right" ? clamp(startRight - delta, 190, 560) : startRight
        });
      };

      const onUp = () => {
        document.body.classList.remove("is-resizing-pane");
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [paneWidths.left, paneWidths.right]
  );

  const startTreeResize = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      event.preventDefault();
      const startX = event.clientX;
      const startWidth = treePaneWidth;
      document.body.classList.add("is-resizing-pane");

      const onMove = (moveEvent: globalThis.MouseEvent) => {
        setTreePaneWidth(clamp(startWidth + moveEvent.clientX - startX, 300, 720));
      };

      const onUp = () => {
        document.body.classList.remove("is-resizing-pane");
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [treePaneWidth]
  );

  const loadTreeChildren = useCallback(
    async (parentNavId: number | null, options?: { append?: boolean; offset?: number }) => {
      if (!selectedProjectId) return;
      const error = await loadProjectTreeChildren(selectedProjectId, parentNavId, options);
      if (error) setStatusText(error);
    },
    [loadProjectTreeChildren, selectedProjectId]
  );

  const explainFolder = useCallback(async (item: NavItem) => {
    if (item.itemKind !== "directory") return;
    const folderSelectionSeq = selectionSeq.current + 1;
    selectionSeq.current = folderSelectionSeq;
    setRelationships(null);
    setRelationshipsMembership(null);
    setRelationshipsLoading(false);
    setFileOrphanLoading(false);
    try {
      const explanation = await api.folderExplanation(item.id);
      if (folderSelectionSeq !== selectionSeq.current) return;
      setFolderExplanation(explanation);
      setPreview(null);
      setPrimaryView("project");
      setProjectView("files");
      setRightPaneView("inspector");
      setStatusText(explanation ? `Inspecting folder ${explanation.displayName}` : `No local folder details for ${item.displayName}`);
    } catch (error) {
      if (folderSelectionSeq !== selectionSeq.current) return;
      setStatusText(error instanceof Error ? error.message : "Could not inspect folder details.");
    }
  }, []);

  const loadRelationshipsInBackground = useCallback((projectId: number, nodeId: number, expectedSeq: number, label: string) => {
    setRelationshipsLoading(true);
    void api.nodeRelationships(projectId, nodeId)
      .then((nextRelationships) => {
        if (expectedSeq !== selectionSeq.current) return;
        setRelationships(nextRelationships);
        setRelationshipsMembership(fileMembershipKey(projectId, nodeId));
      })
      .catch((error) => {
        if (expectedSeq !== selectionSeq.current) return;
        setRelationships(null);
        setRelationshipsMembership(null);
        setStatusText(`Could not load connections for ${label}: ${error instanceof Error ? error.message : String(error)}`);
      })
      .finally(() => {
        if (expectedSeq === selectionSeq.current) setRelationshipsLoading(false);
      });
  }, []);

  const openNode = useCallback(
    async (nodeId: number, options?: OpenNodeOptions) => {
      const openSelectionSeq = selectionSeq.current + 1;
      selectionSeq.current = openSelectionSeq;
      const refreshOnly = options?.refreshOnly ?? false;
      if (!refreshOnly) setPreviewSession(null);
      const requestedMode = options?.mode ?? previewMode;
      const recordRecent = options?.recordRecent ?? true;
      const cacheKey = `${options?.projectId ?? "any"}:${nodeId}:${requestedMode}`;
      const cached = previewCacheRef.current.get(cacheKey);
      setRelationships(null);
      setRelationshipsMembership(null);
      setRelationshipsLoading(false);
      setFileOrphanLoading(false);
      setStatusText(cached ? `Opening ${cached.displayName}…` : "Opening file preview.");
      await yieldToUi();
      if (openSelectionSeq !== selectionSeq.current) return false;
      let nextPreview: FilePreview;
      if (cached) {
        // Serve the cached preview instantly so switching between recently
        // opened files feels snappy; a background revalidation follows below.
        nextPreview = cached;
      } else {
        try {
          nextPreview = await api.filePreview(nodeId, requestedMode === "edit" ? "source" : requestedMode, recordRecent, previewPolicy, options?.projectId);
        } catch (error) {
          if (openSelectionSeq === selectionSeq.current) {
            setRelationshipsLoading(false);
            setStatusText(`Open failed: ${error instanceof Error ? error.message : String(error)}`);
          }
          return false;
        }
        if (openSelectionSeq !== selectionSeq.current) return false;
        if (options?.projectId != null && nextPreview.projectId !== options.projectId) {
          setRelationshipsLoading(false);
          setStatusText("Open failed: the document no longer belongs to the project returned by search.");
          return false;
        }
        previewCacheRef.current.set(cacheKey, nextPreview);
      }
      manualPreviewClearProjectRef.current = null;
      setFolderExplanation(null);
      const allowProjectSwitch = options?.allowProjectSwitch ?? true;
      if (!refreshOnly) {
        // Record the origin screen (Overview, Discover, Settings…) so Back returns to it.
        pushWorkspaceRoute({
          primaryView: "project",
          projectView: "files",
          rightPaneView: "inspector",
          selectedProjectId: allowProjectSwitch && nextPreview.projectId ? nextPreview.projectId : selectedProjectId
        });
        setPrimaryView("project");
        setProjectView("files");
        setRightPaneView("inspector");
      }
      if (requestedMode !== previewMode) {
        setPreviewMode(requestedMode);
      }
      if (!refreshOnly && allowProjectSwitch && nextPreview.projectId && nextPreview.projectId !== selectedProjectId) {
        manualPreviewClearProjectRef.current = null;
        beginProject(nextPreview.projectId);
        setSelectedProjectId(nextPreview.projectId);
      }
      setPreview((current) => {
        if (!options?.replaceHistory && shouldRecordFileHistoryEntry(current, nextPreview)) {
          setBackStack((stack) => appendFileHistoryEntry(stack, current, nextPreview));
          setForwardStack([]);
        }
        return nextPreview;
      });
      setTabs((current) => {
        const nextTab = {
          nodeId,
          projectId: nextPreview.projectId,
          label: nextPreview.displayName,
          path: nextPreview.displayPath || nextPreview.path
        };
        const existingIndex = current.findIndex((tab) => tab.nodeId === nodeId);
        if (existingIndex < 0) return [...current, nextTab].slice(-8);
        const existing = current[existingIndex];
        if (
          existing.projectId === nextTab.projectId
          && existing.label === nextTab.label
          && existing.path === nextTab.path
        ) return current;
        // The tab strip identifies tabs by node id. If another project reuses
        // that id, move this tab to the validated owner instead of rendering
        // two visually indistinguishable/ambiguous tabs.
        return current.map((tab, index) => index === existingIndex ? nextTab : tab);
      });
      setStatusText(nextPreview.state === "blocked" ? "Preview blocked by policy" : `Opened ${nextPreview.displayName}`);
      if ((options?.recordRecent ?? true) && nextPreview.state === "ready") {
        const openedAt = new Date().toISOString();
        setRecentItems((current) => [
          {
            nodeId,
            projectId: nextPreview.projectId,
            itemKind: "file",
            path: nextPreview.displayPath || nextPreview.path,
            openedAt
          },
          ...current.filter((item) => item.nodeId !== nodeId || item.projectId !== nextPreview.projectId)
        ].slice(0, 20));
        window.setTimeout(() => {
          void api.recentItemsList().then(setRecentItems).catch((error) => {
            setStatusText(`Recent items refresh failed: ${error instanceof Error ? error.message : String(error)}`);
          });
        }, 350);
      }
      if (cached) {
        // Revalidate the cached preview against disk and record recent on the
        // backend; update the view only if this is still the active selection.
        void api
          .filePreview(nodeId, requestedMode, recordRecent, previewPolicy, options?.projectId)
          .then((fresh) => {
            if (options?.projectId != null && fresh.projectId !== options.projectId) return;
            previewCacheRef.current.set(cacheKey, fresh);
            if (openSelectionSeq === selectionSeq.current) {
              setPreview((current) => (
                current
                && current.nodeId === fresh.nodeId
                && current.projectId === fresh.projectId
                  ? fresh
                  : current
              ));
            }
          })
          .catch(() => {
            /* keep the cached preview if revalidation fails */
          });
      }
      return true;
    },
    [beginProject, previewMode, previewPolicy, pushWorkspaceRoute, selectedProjectId]
  );

  const revealNodeInLoadedTree = useCallback(async (projectId: number, nodeId: number) => {
    try {
      const revealed = await revealProjectNode(projectId, nodeId);
      if (!revealed) {
        if (pendingTreeRevealRef.current?.projectId === projectId && pendingTreeRevealRef.current.nodeId === nodeId) {
          pendingTreeRevealRef.current = null;
        }
        return false;
      }
      if (pendingTreeRevealRef.current?.projectId === projectId && pendingTreeRevealRef.current.nodeId === nodeId) {
        pendingTreeRevealRef.current = null;
      }
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => {
          document.querySelector<HTMLElement>(`[data-tree-node-id="${nodeId}"]`)
            ?.scrollIntoView({ block: "nearest", inline: "nearest" });
        });
      });
      setStatusText("Opened the item and revealed its folder in Files.");
      return true;
    } catch (error) {
      setStatusText(`The file opened, but its folder could not be revealed: ${error instanceof Error ? error.message : String(error)}`);
      return false;
    }
  }, [revealProjectNode]);

  const openDocumentHit = useCallback(async (nodeId: number, projectId: number) => {
    const targetProject = displayedProjects.find((project) => project.id === projectId);
    if (!targetProject) {
      setStatusText("That document belongs to a project that is no longer available in the current catalog.");
      return;
    }
    await openNode(nodeId, { projectId, allowProjectSwitch: true });
  }, [displayedProjects, openNode]);

  const openNodeInTree = useCallback(async (
    nodeId: number,
    options?: OpenNodeOptions & { projectId?: number | null }
  ) => {
    const targetProjectId = options?.projectId ?? selectedProjectId;
    if (targetProjectId) pendingTreeRevealRef.current = { projectId: targetProjectId, nodeId };
    const opened = await openNode(nodeId, options);
    if (!opened) {
      if (pendingTreeRevealRef.current?.projectId === targetProjectId && pendingTreeRevealRef.current.nodeId === nodeId) {
        pendingTreeRevealRef.current = null;
      }
      return;
    }
    if (
      targetProjectId
      && targetProjectId === selectedProjectId
      && projectWorkspace.loadStatus === "ready"
    ) {
      await revealNodeInLoadedTree(targetProjectId, nodeId);
    }
  }, [openNode, projectWorkspace.loadStatus, revealNodeInLoadedTree, selectedProjectId]);

  const openShellFileImmediately = useCallback(async (
    target: OpenTargetPreparation,
    preparedPreview?: FilePreview,
    provisional = false,
    foreground = true
  ) => {
    const requestedMode = shellPreviewMode(target.inputPath);
    let nextPreview = preparedPreview ?? await api.openTargetPreview(
        target.projectId,
        target.inputPath,
        requestedMode,
        previewPolicy
      );
    const transientKey = `${target.projectId}:${normalizeProjectRootPath(target.inputPath)}`;
    if (nextPreview.nodeId <= 0) {
      let transientNodeId = transientShellNodeIdsRef.current.get(transientKey);
      if (transientNodeId == null) {
        transientNodeId = nextTransientShellNodeIdRef.current;
        nextTransientShellNodeIdRef.current -= 1;
        transientShellNodeIdsRef.current.set(transientKey, transientNodeId);
      }
      nextPreview = { ...nextPreview, nodeId: transientNodeId };
    } else {
      transientShellNodeIdsRef.current.delete(transientKey);
    }
    if (!provisional) {
      transientShellNodeIdsRef.current.delete(`-1:${normalizeProjectRootPath(target.inputPath)}`);
    }
    if (foreground) {
      const openSelectionSeq = selectionSeq.current + 1;
      selectionSeq.current = openSelectionSeq;
      setPreviewSession(null);
      setFolderExplanation(null);
      setRelationships(null);
      setRelationshipsMembership(null);
      setRelationshipsLoading(false);
      setFileOrphanLoading(false);
      manualPreviewClearProjectRef.current = null;
      if (!provisional) {
        pushWorkspaceRoute({
          primaryView: "project",
          projectView: "files",
          rightPaneView: "inspector",
          selectedProjectId: target.projectId
        });
      }
      setPrimaryView("project");
      setProjectView("files");
      setRightPaneView("inspector");
      setPreviewMode(requestedMode);
      if (target.projectId !== selectedProjectId) {
        beginProject(target.projectId);
        setSelectedProjectId(target.projectId);
      }
      setPreview((current) => {
        if (current?.projectId !== -1 && shouldRecordFileHistoryEntry(current, nextPreview)) {
          setBackStack((stack) => appendFileHistoryEntry(stack, current, nextPreview));
          setForwardStack([]);
        }
        return nextPreview;
      });
    }
    setTabs((current) => {
      // A newer Explorer destination replaces the older provisional tab;
      // indexed tabs from the normal workspace remain available.
      const base = provisional ? current.filter((tab) => tab.projectId !== -1) : current;
      const samePathIndex = base.findIndex((tab) =>
        (tab.projectId === nextPreview.projectId || (!provisional && tab.projectId === -1))
        && normalizeProjectRootPath(tab.path) === normalizeProjectRootPath(nextPreview.path)
      );
      const nextTab = {
        nodeId: nextPreview.nodeId,
        projectId: nextPreview.projectId,
        label: nextPreview.displayName,
        path: nextPreview.path
      };
      if (samePathIndex >= 0) {
        return base.map((tab, index) => index === samePathIndex ? nextTab : tab);
      }
      return [...base, nextTab].slice(-8);
    });
    if (nextPreview.nodeId > 0) {
      previewCacheRef.current.set(`${nextPreview.projectId}:${nextPreview.nodeId}:${requestedMode}`, nextPreview);
    }
    if (foreground) {
      setStatusText(nextPreview.state === "blocked"
        ? "Preview blocked by local protection policy."
        : `Opened ${displayLocalPath(target.inputPath)} immediately; its project is refreshing in the background.`);
    }
    return nextPreview;
  }, [beginProject, previewPolicy, pushWorkspaceRoute, selectedProjectId]);

  const openWorkspaceTab = useCallback(async (
    nodeId: number,
    options?: OpenNodeOptions,
    tabHint?: OpenTab
  ) => {
    const tab = tabHint ?? tabs.find((candidate) => candidate.nodeId === nodeId);
    if (nodeId > 0) {
      await openNode(nodeId, {
        ...options,
        projectId: options?.projectId ?? tab?.projectId
      });
      return;
    }
    if (!tab) return;
    const expectedSeq = selectionSeq.current + 1;
    selectionSeq.current = expectedSeq;
    const requestedMode = shellPreviewMode(tab.path);
    try {
      const direct = await api.openTargetPreview(tab.projectId, tab.path, requestedMode, previewPolicy);
      if (expectedSeq !== selectionSeq.current) return;
      const nextPreview = direct.nodeId <= 0 ? { ...direct, nodeId: tab.nodeId } : direct;
      if (nextPreview.nodeId > 0) {
        transientShellNodeIdsRef.current.delete(`${tab.projectId}:${normalizeProjectRootPath(tab.path)}`);
      }
      pushWorkspaceRoute({
        primaryView: "project",
        projectView: "files",
        rightPaneView: "inspector",
        selectedProjectId: tab.projectId
      });
      setPrimaryView("project");
      setProjectView("files");
      setRightPaneView("inspector");
      setPreviewMode(requestedMode);
      if (tab.projectId !== selectedProjectId) {
        beginProject(tab.projectId);
        setSelectedProjectId(tab.projectId);
      }
      setPreview(nextPreview);
      setTabs((current) => current.map((candidate) => candidate.nodeId === tab.nodeId
        ? {
            nodeId: nextPreview.nodeId,
            projectId: nextPreview.projectId,
            label: nextPreview.displayName,
            path: nextPreview.path
          }
        : candidate));
      setStatusText(`Opened ${nextPreview.displayName}${nextPreview.nodeId > 0 ? "" : " while its index refreshes"}.`);
    } catch (error) {
      if (expectedSeq === selectionSeq.current) {
        setStatusText(`Could not reopen ${tab.label}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }, [beginProject, openNode, previewPolicy, pushWorkspaceRoute, selectedProjectId, tabs]);

  const waitForShellOpenScan = useCallback((jobId: string) => {
    const existing = shellScanWaitersRef.current.get(jobId);
    if (existing) return existing;
    const waiter = (async () => {
      try {
        while (true) {
          const status = await api.scanStatus(jobId);
          setScanStatus(status);
          if (!shellScanIsPending(status)) return status;
          await delay(document.hidden ? 1_200 : 350);
        }
      } finally {
        shellScanWaitersRef.current.delete(jobId);
      }
    })();
    shellScanWaitersRef.current.set(jobId, waiter);
    return waiter;
  }, [setScanStatus]);

  const refreshShellProjectDiscovery = useCallback(async () => {
    setSessionInventoryRefreshing(true);
    setSessionInventoryError(null);
    try {
      const report = await api.projectDiscoveryReport(500, true, true, false, 0);
      setProjectDiscoveryReport(report);
      setSessionInventory(report.sessions);
      setSessionInventoryState("fresh");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSessionInventoryState((current) => current === "fresh" || current === "cached" ? "cached" : "unavailable");
      setSessionInventoryError(message);
      throw error;
    } finally {
      setSessionInventoryRefreshing(false);
    }
  }, []);

  const requestShellOpenChoice = useCallback((
    inspection: OpenTargetInspection,
    sequence: number
  ) => (
    new Promise<ShellOpenChoice>((resolve) => {
      shellOpenChoiceResolverRef.current?.resolve("cancel");
      shellOpenChoiceResolverRef.current = { sequence, resolve };
      setShellOpenChoice(inspection);
    })
  ), []);

  const finishShellOpenChoice = useCallback((choice: ShellOpenChoice) => {
    const pending = shellOpenChoiceResolverRef.current;
    shellOpenChoiceResolverRef.current = null;
    setShellOpenChoice(null);
    pending?.resolve(choice);
  }, []);

  const disposeShellViewerResources = useCallback(async (session: ShellViewerSession) => {
    await disposeShellViewerSafely(session, {
      scanStatus: api.scanStatus,
      scanCancel: api.scanCancel,
      waitForScan: waitForShellOpenScan,
      discardInvestigation: api.discardInvestigation
    });
  }, [waitForShellOpenScan]);

  const queueShellViewerDisposal = useCallback((session: ShellViewerSession) => {
    const existing = shellViewerDisposalsRef.current.get(session.rootId);
    if (existing) return existing;
    const pending = disposeShellViewerResources(session).finally(() => {
      if (shellViewerDisposalsRef.current.get(session.rootId) === pending) {
        shellViewerDisposalsRef.current.delete(session.rootId);
      }
    });
    shellViewerDisposalsRef.current.set(session.rootId, pending);
    return pending;
  }, [disposeShellViewerResources]);

  const closeShellViewerSession = useCallback(async (
    session: ShellViewerSession,
    announce: boolean
  ) => {
    if (!session.ready || shellViewerClosing) return false;
    setShellViewerClosing(true);
    const closeGeneration = shellViewerCloseGenerationRef.current + 1;
    shellViewerCloseGenerationRef.current = closeGeneration;
    const closeIntentGeneration = invalidateShellOpenIntent();
    selectionSeq.current += 1;
    const retiredViewer = shellViewerRetirementRef.current
      && shellViewerRetirementRef.current.rootId !== session.rootId
        ? shellViewerRetirementRef.current
        : null;
    if (shellViewerRetirementRef.current) {
      shellViewerRetirementRef.current = null;
    }
    if (shellViewerRef.current?.rootId === session.rootId) {
      shellViewerRef.current = null;
      setShellViewer(null);
    }
    // Viewer dismissal is a complete local UI transition before cancellation,
    // discard or catalog refresh can block/fail. A later shell request therefore
    // cannot be covered by this close operation's eventual DB response.
    setProjects((current) => current.filter((project) => (
      project.id !== session.project.id && project.id !== -1
    )));
    setTabs((current) => current.filter((tab) => (
      tab.projectId !== session.project.id && tab.projectId !== -1
    )));
    setPreview((current) => current
      && (current.projectId === session.project.id || current.projectId === -1)
        ? null
        : current);
    transientShellNodeIdsRef.current.clear();
    beginProject(null);
    setSelectedProjectId(null);
    setBackStack([]);
    setForwardStack([]);
    showOverview({ preserveShellIntent: true });

    const closeStillOwnsUi = () => (
      shellViewerCloseGenerationRef.current === closeGeneration
      && shellOpenIntentGenerationRef.current === closeIntentGeneration
      && shellViewerRef.current === null
    );
    try {
      await queueShellViewerDisposal(session);
      if (retiredViewer) await queueShellViewerDisposal(retiredViewer);
      await Promise.all([...shellViewerDisposalsRef.current.values()]);
      const loaded = await api.projectsListLite().catch(() => (
        projects.filter((project) => project.id !== session.project.id && project.id !== -1)
      ));
      if (closeStillOwnsUi()) {
        setProjects(loaded);
        setProjectsFromCache(false);
      }
      if (announce && closeStillOwnsUi()) {
        setStatusText(session.rootId <= 0
          ? "Viewer closed. The requested file on disk was not changed."
          : session.temporary
          ? "Viewer closed. Its temporary index was discarded; files on disk were not changed."
          : "Viewer closed. Returned to the project catalog.");
      }
      return true;
    } catch (error) {
      if (announce && closeStillOwnsUi()) {
        setStatusText(`Could not close Viewer cleanly: ${error instanceof Error ? error.message : String(error)}`);
      }
      return false;
    } finally {
      setShellViewerClosing(false);
    }
  }, [beginProject, invalidateShellOpenIntent, projects, queueShellViewerDisposal, shellViewerClosing, showOverview]);

  const waitForShellInventory = useCallback(async () => {
    while (true) {
      const status = await api.startupStatus();
      if (status.state === "ready") return;
      if (status.state === "failed") throw new Error(status.message);
      await delay(80);
    }
  }, []);

  const previewShellOpenPath = useCallback(async (
    sequence: number,
    path: string
  ): Promise<PendingShellOpenRequest> => {
    const activeViewer = shellViewerRef.current;
    if (activeViewer && activeViewer.rootId > 0) {
      shellViewerRetirementRef.current = activeViewer;
    }
    const intentGeneration = shellOpenIntentGenerationRef.current;
    shellOpenHasPriorityRef.current = true;
    setQuickOpenVisible(false);
    setCommandVisible(false);
    setAddProjectsVisible(false);
    setTourMode(null);
    setBackgroundStatus(`Opening ${displayLocalPath(path)} from Windows.`);
    let directFileOpen = false;
    let fullPreviewPromise: Promise<void> | null = null;
    const initialSelectionSequence = selectionSeq.current;
    const previewRequestOwnsFocus = () => shellOpenPreviewReadOwnsFocus(
      sequence,
      shellOpenLatestRequestSequenceRef.current,
      intentGeneration,
      shellOpenIntentGenerationRef.current,
      initialSelectionSequence,
      selectionSeq.current
    );
    try {
      // Cold-start path: reading the one explicitly requested file does not
      // require SQLCipher, migrations, project registration or an inventory
      // scan. Publish this isolated preview first, then reconcile it with the
      // catalog once the local database becomes ready.
      const direct = previewRequestOwnsFocus()
        ? await api.openLocalFilePreview(path, shellPreviewMode(path), previewPolicy)
        : null;
      if (!previewRequestOwnsFocus()) {
        return {
          sequence,
          path,
          directFileOpen,
          previewSelectionSequence: selectionSeq.current,
          intentGeneration,
          fullPreviewPromise
        };
      }
      if (direct) {
        const provisionalTarget: OpenTargetPreparation = {
          inputPath: direct.inputPath,
          targetKind: "file",
          projectRoot: direct.viewerRoot,
          projectId: -1,
          rootId: -1,
          nodeId: null,
          scanJobId: null,
          scanAlreadyRunning: false,
          openMode: "viewer",
          temporary: true
        };
        await openShellFileImmediately(provisionalTarget, direct.preview, true);
        directFileOpen = true;
        const provisionalProject: ProjectSummary = {
          id: -1,
          name: displayLocalPath(direct.viewerRoot).split(/[\\/]/).at(-1) || "Viewer",
          path: direct.viewerRoot,
          source: "viewer",
          contextCount: 0,
          pinned: false,
          scanState: "outdated",
          scanRootId: null,
          isCurrent: false,
          app: null,
          apps: []
        };
        const provisionalSession: ShellViewerSession = {
          rootId: -1,
          project: provisionalProject,
          inputPath: direct.inputPath,
          scanJobId: null,
          temporary: true,
          ready: true
        };
        shellViewerRef.current = provisionalSession;
        setShellViewer(provisionalSession);
        setProjects((current) => [...current.filter((project) => project.id !== -1), provisionalProject]);
        setStartupProgress({
          active: false,
          label: "File open",
          detail: "Local inventory will attach in the background.",
          progress: 100
        });
        const previewSelectionSequence = selectionSeq.current;
        // The normal-size expansion is another DB-independent lane. Start it
        // immediately after yielding the first frame; never place it behind an
        // older request waiting for encrypted inventory.
        fullPreviewPromise = afterFirstPaint().then(async () => {
          try {
            const ownsFocusBeforeRead = shellOpenRequestOwnsFocus(
              sequence,
              shellOpenLatestRequestSequenceRef.current,
              previewSelectionSequence,
              selectionSeq.current,
              intentGeneration,
              shellOpenIntentGenerationRef.current
            );
            if (!ownsFocusBeforeRead) return;
            const full = await api.openLocalFilePreviewFull(path, shellPreviewMode(path), previewPolicy);
            const ownsFocus = shellOpenRequestOwnsFocus(
              sequence,
              shellOpenLatestRequestSequenceRef.current,
              previewSelectionSequence,
              selectionSeq.current,
              intentGeneration,
              shellOpenIntentGenerationRef.current
            );
            if (!full || !ownsFocus) return;
            const currentTransientId = transientShellNodeIdsRef.current.get(
              `-1:${normalizeProjectRootPath(full.inputPath)}`
            );
            const fullPreview = {
              ...full.preview,
              nodeId: currentTransientId ?? full.preview.nodeId
            };
            setPreview((current) => current
              && current.projectId === -1
              && normalizeProjectRootPath(current.path) === normalizeProjectRootPath(full.inputPath)
                ? { ...fullPreview, nodeId: currentTransientId ?? current.nodeId }
                : current);
            setTabs((current) => current.map((tab) => tab.projectId === -1
              && normalizeProjectRootPath(tab.path) === normalizeProjectRootPath(full.inputPath)
                ? {
                    nodeId: currentTransientId ?? tab.nodeId,
                    projectId: -1,
                    label: fullPreview.displayName,
                    path: fullPreview.path
                  }
                : tab));
          } catch {
            // Keep the already rendered first frame. Inventory attachment has
            // its own error/status path and remains independent.
          }
        });
      }
    } catch (error) {
      if (sequence === shellOpenLatestRequestSequenceRef.current) {
        setStatusText(`Could not preview the Windows item immediately; its local inventory will retry: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    return {
      sequence,
      path,
      directFileOpen,
      previewSelectionSequence: selectionSeq.current,
      intentGeneration,
      fullPreviewPromise
    };
  }, [openShellFileImmediately, previewPolicy]);

  const reconcileShellOpenPath = useCallback(async (request: PendingShellOpenRequest) => {
    const { sequence, path, directFileOpen, previewSelectionSequence, intentGeneration, fullPreviewPromise } = request;
    const requestOwnsFocus = () => shellOpenRequestOwnsFocus(
      sequence,
      shellOpenLatestRequestSequenceRef.current,
      previewSelectionSequence,
      selectionSeq.current,
      intentGeneration,
      shellOpenIntentGenerationRef.current
    );
    let viewerRootId: number | null = null;
    let claimedForeground = false;
    try {
      if (directFileOpen) void fullPreviewPromise;

      await waitForShellInventory();
      // A newer Explorer request or an explicit in-app navigation supersedes
      // this single-destination request. Do not register/scan an orphan ad-hoc
      // root for a file the user has already left.
      if (!requestOwnsFocus()) {
        const requestedPath = normalizeProjectRootPath(path);
        const latestViewerOwnsPath = shellViewerRef.current?.rootId === -1
          && normalizeProjectRootPath(shellViewerRef.current.inputPath) === requestedPath;
        if (sequence !== shellOpenLatestRequestSequenceRef.current && !latestViewerOwnsPath) {
          setTabs((current) => current.filter((tab) => !(
            tab.projectId === -1
            && normalizeProjectRootPath(tab.path) === requestedPath
          )));
          transientShellNodeIdsRef.current.delete(`-1:${requestedPath}`);
        }
        return;
      }
      const inspection = await api.inspectOpenTarget(path);
      if (!requestOwnsFocus()) return;
      let openMode = shellOpenImmediateMode(inspection);
      if (!openMode) {
        const choice = await requestShellOpenChoice(inspection, sequence);
        if (!requestOwnsFocus()) return;
        if (choice === "cancel") {
          if (sequence === shellOpenLatestRequestSequenceRef.current) {
            setStatusText("Windows open cancelled. No project was registered or scanned.");
          }
          return;
        }
        openMode = choice;
      }
      let manualRoot: string | null = null;
      if (openMode === "manual") {
        manualRoot = await api.pickFolder("Choose the project root that contains this item");
        if (!requestOwnsFocus()) return;
        if (!manualRoot) {
          if (sequence === shellOpenLatestRequestSequenceRef.current) {
            setStatusText("Manual root selection cancelled. No project was registered or scanned.");
          }
          return;
        }
      }

      if (!requestOwnsFocus()) return;
      const target = await api.prepareOpenTarget(path, openMode, manualRoot, performanceMode);
      let staleTargetDiscarded = false;
      const discardStaleTarget = async () => {
        if (staleTargetDiscarded || requestOwnsFocus()) return false;
        staleTargetDiscarded = true;
        if (target.temporary && shellViewerRef.current?.rootId !== target.rootId) {
          await api.discardInvestigation(target.rootId).catch(() => undefined);
        }
        setTabs((current) => current.filter((tab) => !(
          tab.projectId === target.projectId
          && normalizeProjectRootPath(tab.path) === normalizeProjectRootPath(target.inputPath)
        )));
        return true;
      };
      if (await discardStaleTarget()) return;
      let openedPreview: FilePreview | null = null;
      if (target.targetKind === "file") {
        const requestedMode = shellPreviewMode(target.inputPath);
        const attachedPreview = await api.openTargetPreview(
          target.projectId,
          target.inputPath,
          requestedMode,
          previewPolicy
        );
        if (await discardStaleTarget()) return;
        claimedForeground = requestOwnsFocus();
        openedPreview = await openShellFileImmediately(
          target,
          attachedPreview,
          false,
          claimedForeground
        );
        if (claimedForeground && target.openMode === "viewer") {
          // Publish the isolated project identity in the same render as the
          // direct preview. Waiting for projectGet would otherwise leave the
          // workspace with no selected project and briefly hide the document.
          const immediateViewerProject: ProjectSummary = {
            id: target.projectId,
            name: displayLocalPath(target.projectRoot).split(/[\\/]/).at(-1) || "Viewer",
            path: target.projectRoot,
            source: "viewer",
            contextCount: 0,
            pinned: false,
            scanState: "outdated",
            scanRootId: target.rootId,
            isCurrent: false,
            app: null,
            apps: []
          };
          const immediateSession: ShellViewerSession = {
            rootId: target.rootId,
            project: immediateViewerProject,
            inputPath: target.inputPath,
            scanJobId: target.scanJobId,
            temporary: target.temporary,
            ready: false
          };
          viewerRootId = target.rootId;
          shellViewerRef.current = immediateSession;
          setShellViewer(immediateSession);
          setProjects((current) => current.some((project) => project.id === immediateViewerProject.id)
            ? current.map((project) => project.id === immediateViewerProject.id ? immediateViewerProject : project)
            : [...current, immediateViewerProject]);
        } else if (claimedForeground) {
          // Move the navigation catalog off provisional -1 in the same commit
          // as the real selected project. Otherwise the catalog-integrity
          // effect clears the just-opened document on the next paint.
          const immediateProject: ProjectSummary = {
            id: target.projectId,
            name: displayLocalPath(target.projectRoot).split(/[\\/]/).at(-1) || "Project",
            path: target.projectRoot,
            source: "shell",
            contextCount: 0,
            pinned: false,
            scanState: "outdated",
            scanRootId: target.rootId,
            isCurrent: false,
            app: null,
            apps: []
          };
          shellViewerRef.current = null;
          setShellViewer(null);
          setProjects((current) => {
            const withoutProvisional = current.filter((project) => project.id !== -1);
            return withoutProvisional.some((project) => project.id === target.projectId)
              ? withoutProvisional
              : [...withoutProvisional, immediateProject];
          });
        }
        // The document is the requested destination, not a startup detail.
        // Remove the catalog progress cover before any project/session work so
        // the rendered Markdown is actually visible on the next paint.
        if (claimedForeground) {
          setStartupProgress({
            active: false,
            label: "File open",
            detail: "Project inventory is refreshing in the background.",
            progress: 100
          });
          // Commit the React state to a real WebView frame before allowing any
          // secondary read, recent-history write or scan worker to start.
          await afterFirstPaint();
        }
        if (openedPreview.nodeId > 0) {
          // Recent history is secondary to reading. It may revalidate the
          // indexed file, but only after the requested document has painted.
          void api.filePreview(
            openedPreview.nodeId,
            requestedMode,
            true,
            previewPolicy,
            openedPreview.projectId
          ).catch(() => undefined);
        }
      }
      // Folder preparation has no direct preview call to establish ownership.
      // Decide it here, before any catalog/session promotion can touch -1 state.
      if (target.targetKind === "folder") claimedForeground = requestOwnsFocus();
      // The requested file is already visible. Resolve only this one cheap DB
      // row before returning control to the UI; project catalogs, roots,
      // discovery and the full tree are deliberately background work.
      const detail = await api.projectGet(target.projectId);
      if (await discardStaleTarget()) return;
      if (!detail) throw new Error("The local viewer/project was created but could not be loaded.");
      const targetProject: ProjectSummary = target.openMode === "viewer"
        ? {
            ...detail,
            source: "viewer",
            contextCount: detail.contextCount ?? 0,
            pinned: false,
            scanState: detail.scanState ?? "outdated",
            scanRootId: target.rootId,
            isCurrent: false,
            app: null,
            apps: []
          }
        : detail;
      // Revalidate after projectGet: a newer request may have published a new
      // provisional Viewer while this metadata row was resolving.
      claimedForeground = requestOwnsFocus();
      setProjects((current) => {
        // The global -1 row always belongs to the latest immediate preview.
        // An older attachment may add its real project to the catalog, but it
        // must not remove or promote that newer provisional Viewer.
        const base = claimedForeground
          ? current.filter((project) => project.id !== -1)
          : current;
        const existingIndex = base.findIndex((project) => project.id === targetProject.id);
        if (existingIndex < 0) return [...base, targetProject];
        return base.map((project, index) => index === existingIndex ? targetProject : project);
      });
      setProjectsFromCache(false);

      if (claimedForeground) {
        if (target.openMode === "viewer") {
          const session: ShellViewerSession = {
            rootId: target.rootId,
            project: targetProject,
            inputPath: target.inputPath,
            scanJobId: target.scanJobId,
            temporary: target.temporary,
            ready: false
          };
          viewerRootId = target.rootId;
          shellViewerRef.current = session;
          setShellViewer(session);
        } else {
          shellViewerRef.current = null;
          setShellViewer(null);
        }
      }
      const projectName = targetProject.name
        || displayLocalPath(target.projectRoot).split(/[\\/]/).at(-1)
        || "project";

      // Root-list reconciliation is metadata-only, but it is not needed to read
      // the file. Keep it off the latency path as well.
      void api.rootsList().then(setRoots).catch(() => undefined);

      if (target.targetKind === "folder" && claimedForeground) {
        selectProject(target.projectId, { preserveShellIntent: true });
        setProjectView("files");
        setStatusText(target.openMode === "viewer"
          ? `Opened ${projectName} in Viewer. Its temporary read-only index is refreshing.`
          : `Opened ${projectName} from File Explorer. Its local inventory is refreshing.`);
      }

      // A real Viewer that existed before an A/B shell-open batch belongs to
      // the latest surviving request, never to whichever provisional preview
      // happened to capture it. Retire it only after replacement promotion.
      const viewerToRetire = shellViewerRetirementRef.current;
      if (viewerToRetire) {
        shellViewerRetirementRef.current = null;
        if (viewerToRetire.rootId !== target.rootId) {
          void queueShellViewerDisposal(viewerToRetire).catch(() => undefined);
        }
      }

      // Preparation may already have completed, but a superseded request must
      // never start scan work or later compete with the active request.
      if (!requestOwnsFocus()) return;

      let shellScanJobId = target.scanJobId ?? null;
      let scanStartedHere = false;
      let scanStartWarning: string | null = null;
      if (!shellScanJobId) {
        try {
          // For files, `openShellFileImmediately` has rendered and yielded
          // before this command can create any scan worker.
          const scanStart = await api.startOpenTargetScan(target.rootId, performanceMode);
          shellScanJobId = scanStart.jobId;
          scanStartedHere = scanStart.startedHere;
        } catch (error) {
          scanStartWarning = error instanceof Error ? error.message : String(error);
        }
      }
      if (!requestOwnsFocus()) {
        if (scanStartedHere && shellScanJobId) {
          try {
            const staleStatus = await api.scanStatus(shellScanJobId);
            if (shellScanIsPending(staleStatus)) {
              await api.scanCancel(shellScanJobId);
              await waitForShellOpenScan(shellScanJobId);
            }
          } catch {
            // Best effort only for a job atomically admitted for this request.
          }
        }
        return;
      }
      if (target.openMode === "viewer" && shellViewerRef.current?.rootId === target.rootId) {
        const viewerWithScan = { ...shellViewerRef.current, scanJobId: shellScanJobId };
        shellViewerRef.current = viewerWithScan;
        setShellViewer(viewerWithScan);
      }

      if (shellScanJobId) {
        const jobId = shellScanJobId;
        // Crucial latency boundary: nothing below is awaited by shell-open. The
        // requested file is already visible; inventory, tree and app/session
        // correlation converge in the background.
        void (async () => {
          let refreshWarning: string | null = null;
          try {
            const finalStatus = await waitForShellOpenScan(jobId);
            if (!requestOwnsFocus()) return;
            if (shellScanFailedToOpen(finalStatus) || finalStatus.state === "partial") {
              refreshWarning = finalStatus.error ?? finalStatus.message;
            }
            const [refreshedProjects, refreshedRoots] = await Promise.all([
              api.projectsListLite(),
              api.rootsList()
            ]);
            if (!requestOwnsFocus()) return;
            setProjects((current) => {
              const activeProvisional = current.find((project) => project.id === -1);
              return activeProvisional && !refreshedProjects.some((project) => project.id === -1)
                ? [...refreshedProjects, activeProvisional]
                : refreshedProjects;
            });
            setProjectsFromCache(false);
            setRoots(refreshedRoots);

            if (target.targetKind === "file" && !shellScanFailedToOpen(finalStatus)) {
              const nodeId = await api.resolveOpenTarget(target.projectId, target.inputPath);
              if (!requestOwnsFocus()) return;
              if (nodeId != null) {
                const requestedMode = shellPreviewMode(target.inputPath);
                const fresh = await api.filePreview(nodeId, requestedMode, true, previewPolicy, target.projectId);
                if (!requestOwnsFocus()) return;
                transientShellNodeIdsRef.current.delete(`${target.projectId}:${normalizeProjectRootPath(target.inputPath)}`);
                previewCacheRef.current.set(`${target.projectId}:${nodeId}:${requestedMode}`, fresh);
                setPreview((current) => current
                  && current.projectId === target.projectId
                  && normalizeProjectRootPath(current.path) === normalizeProjectRootPath(target.inputPath)
                  ? fresh
                  : current);
                setTabs((current) => current.map((tab) =>
                  tab.projectId === target.projectId
                  && normalizeProjectRootPath(tab.path) === normalizeProjectRootPath(target.inputPath)
                    ? { nodeId, projectId: target.projectId, label: fresh.displayName, path: fresh.path }
                    : tab
                ));
              } else {
                refreshWarning = "The file stayed readable in Viewer, but it was excluded from the project index.";
              }
            }

            if (!requestOwnsFocus()) return;
            if (selectedProjectIdRef.current === target.projectId) {
              await loadProjectData(target.projectId, false);
            }

            if (target.openMode !== "viewer" && !shellDiscoveryRefreshJobsRef.current.has(jobId)) {
              shellDiscoveryRefreshJobsRef.current.add(jobId);
              try {
                await refreshShellProjectDiscovery();
              } catch (error) {
                refreshWarning = `AI-app discovery could not refresh: ${error instanceof Error ? error.message : String(error)}`;
              } finally {
                shellDiscoveryRefreshJobsRef.current.delete(jobId);
              }
            }

            const stillRelevant = target.openMode === "viewer"
              ? shellViewerRef.current?.rootId === target.rootId
              : selectedProjectIdRef.current === target.projectId;
            if (stillRelevant) {
              setStatusText(refreshWarning
                ? `${target.targetKind === "file" ? "File is open" : `Opened ${projectName}`}; background refresh warning: ${refreshWarning}`
                : target.openMode === "viewer"
                  ? `${target.targetKind === "file" ? displayLocalPath(target.inputPath) : projectName} is open in isolated Viewer mode and up to date.`
                  : `${target.targetKind === "file" ? displayLocalPath(target.inputPath) : projectName} is open; local project and app/session correlation is up to date.`);
            }
          } catch (error) {
            const stillRelevant = target.openMode === "viewer"
              ? shellViewerRef.current?.rootId === target.rootId
              : selectedProjectIdRef.current === target.projectId;
            if (stillRelevant) {
              setStatusText(`${target.targetKind === "file" ? "The file remains open" : `Opened ${projectName}`}, but its background refresh failed: ${error instanceof Error ? error.message : String(error)}`);
            }
          }
        })();
      } else if (scanStartWarning && claimedForeground) {
        setStatusText(target.targetKind === "file"
          ? `The file is open, but its background project refresh could not start: ${scanStartWarning}`
          : `Opened ${projectName}, but its background refresh could not start: ${scanStartWarning}`);
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (claimedForeground || requestOwnsFocus()) {
        setStatusText(directFileOpen
          ? `The file is open, but Code Hangar could not attach its project inventory: ${message}`
          : `Could not open the Windows item: ${message}`);
      }
    } finally {
      if (viewerRootId != null && shellViewerRef.current?.rootId === viewerRootId) {
        const readyViewer = { ...shellViewerRef.current, ready: true };
        shellViewerRef.current = readyViewer;
        setShellViewer(readyViewer);
      }
      if (sequence === shellOpenLatestRequestSequenceRef.current) setBackgroundStatus(null);
    }
  }, [loadProjectData, openShellFileImmediately, performanceMode, previewPolicy, queueShellViewerDisposal, refreshShellProjectDiscovery, requestShellOpenChoice, selectProject, waitForShellInventory, waitForShellOpenScan]);

  const drainShellOpenRequests = useCallback(async () => {
    if (!hasTauriRuntime()) return;
    if (shellOpenDrainPromiseRef.current) return shellOpenDrainPromiseRef.current;
    const drain = (async () => {
      shellOpenProcessingRef.current = true;
      const requests: PendingShellOpenRequest[] = [];
      try {
        while (true) {
          const paths = await api.shellOpenTakePending();
          if (paths.length === 0) break;
          const sequencedPaths = paths.map((path) => {
            const sequence = shellOpenRequestSequenceRef.current + 1;
            shellOpenRequestSequenceRef.current = sequence;
            shellOpenLatestRequestSequenceRef.current = sequence;
            return { sequence, path };
          });
          for (const request of sequencedPaths) {
            requests.push(await previewShellOpenPath(request.sequence, request.path));
          }
        }
        // Inventory attachment is deliberately a separate serial lane. A cold
        // DB or a folder-choice dialog can delay reconciliation, but it can no
        // longer hold up DB-independent previews from later Explorer events.
        for (const request of requests) {
          shellOpenReconcileTailRef.current = shellOpenReconcileTailRef.current
            .catch(() => undefined)
            .then(() => reconcileShellOpenPath(request));
        }
      } finally {
        shellOpenProcessingRef.current = false;
        if (shellOpenRerunRef.current) {
          shellOpenRerunRef.current = false;
          setShellOpenRevision((revision) => revision + 1);
        }
      }
    })();
    shellOpenDrainPromiseRef.current = drain;
    try {
      await drain;
    } finally {
      if (shellOpenDrainPromiseRef.current === drain) shellOpenDrainPromiseRef.current = null;
    }
  }, [previewShellOpenPath, reconcileShellOpenPath]);

  const updateShellIntegration = useCallback(async (markdown: boolean, contextMenu: boolean) => {
    setShellIntegrationBusy(true);
    setShellIntegrationError(null);
    try {
      const next = await api.shellIntegrationSet(markdown, contextMenu);
      setShellIntegration(next);
      setStatusText("Windows integration updated. Project files were not changed.");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setShellIntegrationError(message);
      setStatusText(`Windows integration could not be updated: ${message}`);
    } finally {
      setShellIntegrationBusy(false);
    }
  }, []);

  const updateBackgroundStartup = useCallback(async (enabled: boolean) => {
    setShellIntegrationBusy(true);
    setShellIntegrationError(null);
    try {
      const next = await api.backgroundStartupSet(enabled);
      setShellIntegration(next);
      setStatusText(enabled
        ? "Code Hangar will start quietly with Windows and keep local projects refreshed."
        : "Start with Windows disabled. Closing the window still keeps this running in the tray for the current session.");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setShellIntegrationError(message);
      setStatusText(`Background startup could not be updated: ${message}`);
    } finally {
      setShellIntegrationBusy(false);
    }
  }, []);

  const refreshProjectsInBackground = useCallback(async () => {
    setShellIntegrationBusy(true);
    setShellIntegrationError(null);
    try {
      const jobId = await api.backgroundRefreshNow();
      if (jobId) {
        const status = await api.scanStatus(jobId);
        setScanStatus(status);
        setStatusText("Refreshing registered projects in the background. You can keep using Code Hangar.");
      } else {
        setStatusText("Everything is already refreshing or no registered project currently needs a scan.");
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setShellIntegrationError(message);
      setStatusText(`Background refresh could not start: ${message}`);
    } finally {
      setShellIntegrationBusy(false);
    }
  }, [setScanStatus]);

  const openWindowsDefaultApps = useCallback(async () => {
    setShellIntegrationBusy(true);
    setShellIntegrationError(null);
    try {
      await api.shellOpenDefaultApps();
      const next = await api.shellDefaultGuideDismiss();
      setShellIntegration(next);
      setStatusText("Windows Default Apps opened. Choose Code Hangar for Markdown there if you want it as the default.");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setShellIntegrationError(message);
      setStatusText(`Could not open Windows Default Apps: ${message}`);
    } finally {
      setShellIntegrationBusy(false);
    }
  }, []);

  const dismissShellDefaultGuide = useCallback(async () => {
    try {
      setShellIntegration(await api.shellDefaultGuideDismiss());
    } catch (error) {
      setShellIntegrationError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    if (!hasTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("shell-open-available", () => {
      // Invalidate a pending attachment at notification time, before the next
      // drain has even read the newer path from the native inbox.
      shellOpenLatestRequestSequenceRef.current = Math.max(
        shellOpenLatestRequestSequenceRef.current,
        shellOpenRequestSequenceRef.current + 1
      );
      invalidateShellOpenIntent();
      if (shellOpenProcessingRef.current) shellOpenRerunRef.current = true;
      setShellOpenRevision((revision) => revision + 1);
    }).then((release) => {
      if (disposed) release();
      else unlisten = release;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [invalidateShellOpenIntent]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<string>("background-scan-started", (event) => {
      void api.scanStatus(event.payload)
        .then((status) => {
          if (!disposed) setScanStatus(status);
        })
        .catch(() => undefined);
    }).then((release) => {
      if (disposed) release();
      else unlisten = release;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [setScanStatus]);

  useEffect(() => {
    // The first read of a shell-supplied file is DB-independent. Drain as soon
    // as the WebView is mounted; project attachment waits internally for the
    // encrypted inventory without delaying the document.
    void drainShellOpenRequests();
  }, [drainShellOpenRequests, shellOpenRevision]);

  useEffect(() => {
    if (!startupRouteResolved || !hasTauriRuntime()) return;
    void api.shellIntegrationStatus()
      .then(setShellIntegration)
      .catch((error) => setShellIntegrationError(error instanceof Error ? error.message : String(error)));
  }, [startupRouteResolved]);

  const loadSessionPreviewContent = useCallback(async (
    session: SessionDiscoveryCandidate,
    reveal: boolean,
    options: {
      maxBytes?: number;
      loadFull?: boolean;
      kind?: SessionPreviewLoadKind;
      preserveCurrent?: boolean;
    } = {}
  ) => {
    const requestSeq = sessionPreviewSeq.current + 1;
    sessionPreviewSeq.current = requestSeq;
    const loadKind = options.kind ?? (reveal ? "reveal" : "initial");
    setSessionPreviewLoading(true);
    setSessionPreviewLoadKind(loadKind);
    setSessionPreviewError(null);
    if (reveal) {
      setSessionRevealing(true);
    } else if (!options.preserveCurrent) {
      setSessionPreview(null);
    }
    try {
      const result = await api.sessionPreview(session.path, reveal, {
        maxBytes: options.maxBytes,
        loadFull: options.loadFull
      });
      if (requestSeq !== sessionPreviewSeq.current) return;
      const displayName = enrichedSessionDisplayName(session.displayName, result.text);
      if (displayName !== session.displayName) {
        setPreviewSession((current) => current?.path === session.path ? { ...current, displayName } : current);
        setSessionTitleOverrides((current) => current[session.path] === displayName
          ? current
          : { ...current, [session.path]: displayName });
      }
      setSessionPreview(result);
      if (primaryViewRef.current === "project") {
        const message = loadKind === "more"
          ? `Loaded more of ${displayName}.`
          : loadKind === "full"
            ? `Opened the full session ${displayName}.`
            : reveal
              ? `Revealed masked content transiently for ${displayName}.`
              : `Opened session ${displayName}.`;
        setStatusText(message);
      }
    } catch (error) {
      if (requestSeq !== sessionPreviewSeq.current) return;
      setSessionPreviewError(error instanceof Error ? error.message : String(error));
    } finally {
      if (requestSeq === sessionPreviewSeq.current) {
        setSessionPreviewLoading(false);
        setSessionPreviewLoadKind(null);
        setSessionRevealing(false);
      }
    }
  }, []);

  const openSession = useCallback((session: SessionDiscoveryCandidate) => {
    invalidateShellOpenIntent();
    // Record the origin screen so Back returns to it (the route itself does not
    // encode the open session — applying a route dismisses it).
    pushWorkspaceRoute({}, { recordCurrent: true });
    setProjectSidebarFocus(true);
    setPreviewSession(session);
    setPrimaryView("project");
    setRightPaneView("inspector");
    setStatusText(`Opening session ${session.displayName}.`);
  }, [invalidateShellOpenIntent, pushWorkspaceRoute]);

  const revealSessionTokens = useCallback(async () => {
    if (!previewSession || !sessionPreview || !previewPolicy.allowSensitiveReveal) return;
    const confirmed = await requestConfirm(
      `Reveal the masked tokens in ${previewSession.displayName} transiently in this session? They are not indexed or persisted.`,
      { confirmLabel: "Reveal tokens" }
    );
    if (!confirmed) return;
    void loadSessionPreviewContent(previewSession, true, {
      maxBytes: sessionPreview.previewLimitBytes,
      loadFull: !sessionPreview.truncated,
      kind: "reveal",
      preserveCurrent: true
    });
  }, [previewSession, sessionPreview, previewPolicy, loadSessionPreviewContent, requestConfirm]);

  const loadMoreSessionPreview = useCallback(() => {
    if (!previewSession || !sessionPreview?.truncated || sessionPreviewLoading) return;
    const maxBytes = nextSessionPreviewLimit(sessionPreview.previewLimitBytes, sessionPreview.sizeBytes);
    void loadSessionPreviewContent(previewSession, sessionPreview.revealed, {
      maxBytes,
      kind: "more",
      preserveCurrent: true
    });
  }, [loadSessionPreviewContent, previewSession, sessionPreview, sessionPreviewLoading]);

  const loadFullSessionPreview = useCallback(() => {
    if (!previewSession || !sessionPreview?.truncated || sessionPreviewLoading) return;
    void loadSessionPreviewContent(previewSession, sessionPreview.revealed, {
      maxBytes: sessionPreview.previewLimitBytes,
      loadFull: true,
      kind: "full",
      preserveCurrent: true
    });
  }, [loadSessionPreviewContent, previewSession, sessionPreview, sessionPreviewLoading]);

  useEffect(() => {
    if (!previewSession) {
      sessionPreviewSeq.current += 1;
      setSessionPreview(null);
      setSessionPreviewError(null);
      setSessionPreviewLoading(false);
      setSessionPreviewLoadKind(null);
      setSessionRevealing(false);
      return;
    }
    void loadSessionPreviewContent(previewSession, false, { kind: "initial" });
  }, [previewSession, loadSessionPreviewContent]);

  const refreshWatcherStatus = useCallback(async () => {
    if (resettingRef.current) return;
    const next = await api.watcherStatus(
      preview?.projectId ?? selectedProjectId,
      preview && preview.nodeId > 0 ? preview.nodeId : null
    );
    setWatcherStatus(next);
    const currentNode = next.focused?.currentNode ?? null;
    if (
      preview
      && currentNode
      && currentNode.nodeId === preview.nodeId
      && currentNode.state === "changed"
      && (currentNode.isMarkdown || currentNode.isContext)
    ) {
      const refreshKey = `${preview.projectId}:${currentNode.nodeId}:${currentNode.liveMtime ?? ""}:${currentNode.liveSize ?? ""}`;
      if (watcherPreviewRefreshRef.current !== refreshKey) {
        watcherPreviewRefreshRef.current = refreshKey;
        setBackgroundStatus(`Refreshing preview because ${currentNode.displayName} changed on disk.`);
        // refreshOnly: a background poll must never switch views or routes — it
        // would yank the user out of Overview/Discover/Settings mid-read.
        await openNode(currentNode.nodeId, {
          projectId: preview.projectId,
          recordRecent: false,
          replaceHistory: true,
          refreshOnly: true
        });
        setBackgroundStatus(null);
      }
    } else if (currentNode?.state === "clean") {
      watcherPreviewRefreshRef.current = null;
    }
  }, [openNode, preview, selectedProjectId]);

  useEffect(() => {
    let cancelled = false;
    let timerId: number | null = null;
    const schedule = (delay: number) => {
      if (cancelled) return;
      if (timerId !== null) window.clearTimeout(timerId);
      timerId = window.setTimeout(run, delay);
    };
    const run = () => {
      if (cancelled) return;
      if (document.hidden) {
        schedule(120_000);
        return;
      }
      void refreshWatcherStatus().catch((error) => {
        if (!cancelled) {
          setStatusText(`Watcher refresh failed: ${error instanceof Error ? error.message : String(error)}`);
        }
      }).finally(() => {
        const focusedWorkspace = primaryView === "project" && selectedProjectId !== null;
        schedule(focusedWorkspace ? watcherStatus?.pollIntervalMs ?? 30_000 : 60_000);
      });
    };
    const onVisibilityChange = () => {
      if (!document.hidden) schedule(250);
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    schedule(2_000);
    return () => {
      cancelled = true;
      document.removeEventListener("visibilitychange", onVisibilityChange);
      if (timerId !== null) window.clearTimeout(timerId);
    };
  }, [primaryView, refreshWatcherStatus, selectedProjectId, watcherStatus?.pollIntervalMs]);

  const revealPreview = useCallback(async () => {
    if (!preview) return;
    if (preview.nodeId <= 0) {
      setStatusText("This file is already open. Protected-content reveal becomes available after its background index entry is ready.");
      return;
    }
    const confirmed = await requestConfirm(
      `Reveal ${preview.displayName} transiently in this session? The content will not be indexed or persisted.`,
      { confirmLabel: "Reveal file" }
    );
    if (!confirmed) return;
    const revealSelectionSeq = selectionSeq.current;
    setRelationships(null);
    setRelationshipsMembership(null);
    setRelationshipsLoading(false);
    setStatusText("Revealing preview for this session.");
    await yieldToUi();
    if (revealSelectionSeq !== selectionSeq.current) return;
    let nextPreview: FilePreview;
    try {
      nextPreview = await api.fileReveal(
        preview.nodeId,
        previewMode === "edit" ? "source" : previewMode,
        previewPolicy,
        preview.projectId
      );
    } catch (error) {
      if (revealSelectionSeq === selectionSeq.current) {
        setRelationshipsLoading(false);
        setStatusText(`Reveal failed: ${error instanceof Error ? error.message : String(error)}`);
      }
      return;
    }
    if (revealSelectionSeq !== selectionSeq.current) return;
    if (nextPreview.projectId !== preview.projectId) {
      setStatusText("Reveal failed: the file no longer belongs to the project that opened it.");
      return;
    }
    setPreview(nextPreview);
    setTabs((current) => {
      const nextTab = {
        nodeId: nextPreview.nodeId,
        projectId: nextPreview.projectId,
        label: nextPreview.displayName,
        path: nextPreview.displayPath || nextPreview.path
      };
      const existingIndex = current.findIndex((tab) => tab.nodeId === nextPreview.nodeId);
      if (existingIndex < 0) return [...current, nextTab].slice(-8);
      const existing = current[existingIndex];
      if (
        existing.projectId === nextTab.projectId
        && existing.label === nextTab.label
        && existing.path === nextTab.path
      ) return current;
      return current.map((tab, index) => index === existingIndex ? nextTab : tab);
    });
    setStatusText(nextPreview.state === "ready" ? `Revealed ${nextPreview.displayName}` : nextPreview.blockedReason ?? "Reveal unavailable");
  }, [preview, previewMode, previewPolicy, requestConfirm]);

  const dismissProvisionalShellDestination = useCallback(() => {
    invalidateShellOpenIntent();
    selectionSeq.current += 1;
    if (shellViewerRef.current?.rootId === -1) {
      shellViewerRef.current = null;
      setShellViewer(null);
    }
    const retiredViewer = shellViewerRetirementRef.current;
    shellViewerRetirementRef.current = null;
    if (retiredViewer) {
      void queueShellViewerDisposal(retiredViewer).catch(() => undefined);
    }
    setProjects((current) => current.filter((project) => (
      project.id !== -1 && project.id !== retiredViewer?.project.id
    )));
    if (retiredViewer) {
      setRoots((current) => current.filter((root) => root.id !== retiredViewer.rootId));
    }
    for (const key of transientShellNodeIdsRef.current.keys()) {
      if (key.startsWith("-1:")) transientShellNodeIdsRef.current.delete(key);
    }
  }, [invalidateShellOpenIntent, queueShellViewerDisposal]);

  const activateAfterTabRemoval = useCallback(
    async (
      nextTabs: OpenTab[],
      replacementIndex: number,
      removedActiveTab: boolean,
      removedProvisionalDestination: boolean
    ) => {
      if (removedProvisionalDestination) dismissProvisionalShellDestination();
      setTabs(nextTabs);
      if (!removedActiveTab) {
        if (removedProvisionalDestination && nextTabs.length === 0) {
          beginProject(null);
          setSelectedProjectId(null);
          showOverview({ preserveShellIntent: true });
        }
        return;
      }
      const replacement = nextTabs[Math.min(Math.max(replacementIndex, 0), nextTabs.length - 1)];
      if (replacement) {
        await openWorkspaceTab(replacement.nodeId, { replaceHistory: true }, replacement);
      } else {
        manualPreviewClearProjectRef.current = selectedProjectId;
        setPreview(null);
        setRelationships(null);
        setRelationshipsMembership(null);
        setRelationshipsLoading(false);
        setFileOrphanLoading(false);
        if (removedProvisionalDestination) {
          beginProject(null);
          setSelectedProjectId(null);
          showOverview({ preserveShellIntent: true });
        }
      }
    },
    [beginProject, dismissProvisionalShellDestination, openWorkspaceTab, selectedProjectId, showOverview]
  );

  const closeTab = useCallback(
    async (nodeId: number) => {
      const index = tabs.findIndex((tab) => tab.nodeId === nodeId);
      if (index === -1) return;
      const nextTabs = tabs.filter((tab) => tab.nodeId !== nodeId);
      await activateAfterTabRemoval(
        nextTabs,
        index,
        preview?.nodeId === nodeId,
        tabs.some((tab) => tab.projectId === -1) && !nextTabs.some((tab) => tab.projectId === -1)
      );
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeOtherTabs = useCallback(
    async (nodeId: number) => {
      const tab = tabs.find((candidate) => candidate.nodeId === nodeId);
      if (!tab) return;
      await activateAfterTabRemoval(
        [tab],
        0,
        preview ? preview.nodeId !== nodeId : false,
        tabs.some((candidate) => candidate.projectId === -1) && tab.projectId !== -1
      );
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeAllTabs = useCallback(() => {
    void activateAfterTabRemoval([], 0, preview !== null, tabs.some((tab) => tab.projectId === -1));
  }, [activateAfterTabRemoval, preview, tabs]);

  const closeTabsToSide = useCallback(
    async (nodeId: number, side: "left" | "right") => {
      const index = tabs.findIndex((tab) => tab.nodeId === nodeId);
      if (index === -1) return;
      const nextTabs = tabs.filter((_, tabIndex) => (side === "left" ? tabIndex >= index : tabIndex <= index));
      const removedActiveTab = preview ? !nextTabs.some((tab) => tab.nodeId === preview.nodeId) : false;
      await activateAfterTabRemoval(
        nextTabs,
        side === "left" ? 0 : index,
        removedActiveTab,
        tabs.some((tab) => tab.projectId === -1) && !nextTabs.some((tab) => tab.projectId === -1)
      );
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeTabsOutsideProject = useCallback(
    async (projectId: number, preferredNodeId: number) => {
      const nextTabs = tabs.filter((tab) => tab.projectId === projectId);
      const preferredIndex = Math.max(nextTabs.findIndex((tab) => tab.nodeId === preferredNodeId), 0);
      const removedActiveTab = preview ? !nextTabs.some((tab) => tab.nodeId === preview.nodeId) : false;
      await activateAfterTabRemoval(
        nextTabs,
        preferredIndex,
        removedActiveTab,
        tabs.some((tab) => tab.projectId === -1) && !nextTabs.some((tab) => tab.projectId === -1)
      );
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const copyPath = useCallback(async (path: string) => {
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("Clipboard unavailable");
      }
      await navigator.clipboard.writeText(path);
      setStatusText("Path copied to clipboard.");
    } catch {
      setStatusText("Clipboard is unavailable in this runtime.");
    }
  }, []);

  const invalidatePreviewCache = useCallback((nodeId: number) => {
    for (const key of previewCacheRef.current.keys()) {
      const [, cachedNodeId] = key.split(":", 3);
      if (cachedNodeId === String(nodeId)) previewCacheRef.current.delete(key);
    }
    for (const mode of ["rendered", "source", "edit", "values"]) {
      previewCacheRef.current.delete(`${nodeId}:${mode}`);
    }
  }, []);

  const refreshEditionNode = useCallback(async (nodeId: number, projectId: number) => {
    invalidatePreviewCache(nodeId);
    await openNode(nodeId, {
      projectId,
      mode: "source",
      recordRecent: false,
      refreshOnly: true,
      replaceHistory: true,
      allowProjectSwitch: false
    });
  }, [invalidatePreviewCache, openNode]);

  const showSelectedTextMenu = useCallback((selectedText: string, event: MouseEvent<HTMLElement>) => {
    if (!selectedText) return;
    event.preventDefault();
    const anchor = contextMenuCoordinates(
      event.clientX,
      event.clientY,
      event.currentTarget.getBoundingClientRect()
    );
    const snippet = selectedText.trim().slice(0, 16_000);
    const safeToOffer = preview?.state === "ready"
      && preview.nodeId > 0
      && !preview.wasRevealed
      && !preview.truncated
      && snippet.length > 0;
    const editionItems = preview
      ? editionBridgeRef.current?.selectedTextItems({
        nodeId: preview.nodeId,
        projectId: preview.projectId,
        path: preview.path,
        snippet,
        safeToOffer
      }) ?? []
      : [];
    setContextMenu({
      x: anchor.x,
      y: anchor.y,
      label: "Selected text",
      items: [{
        id: "copy-selection",
        label: "Copy selected text",
        section: "Clipboard",
        help: "Copy the selected text locally without sending or changing anything.",
        icon: <Copy size={15} />,
        onSelect: async () => {
          try {
            if (!navigator.clipboard?.writeText) throw new Error("Clipboard unavailable");
            await navigator.clipboard.writeText(selectedText);
            setStatusText("Selected text copied to clipboard.");
          } catch {
            setStatusText("Clipboard is unavailable in this runtime.");
          }
        }
      }, ...editionItems]
    });
  }, [preview]);

  const copyNodePath = useCallback(async (nodeId: number | null | undefined, fallbackPath: string) => {
    if (!nodeId || nodeId <= 0) {
      await copyPath(fallbackPath);
      return;
    }
    try {
      const fullPath = await api.nodeFullPath(nodeId, fallbackPath);
      await copyPath(fullPath);
    } catch (error) {
      setStatusText(`Copy path failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [copyPath]);

  const openNodeWithSystem = useCallback(async (nodeId: number | null | undefined) => {
    if (!nodeId || nodeId <= 0) {
      setStatusText("Windows actions become available when the background index has registered this file.");
      return;
    }
    try {
      await api.openNodeExternal(nodeId);
      setStatusText("Opening path with Windows.");
    } catch (error) {
      setStatusText(`Open failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  const revealNodeWithSystem = useCallback(async (nodeId: number | null | undefined) => {
    if (!nodeId || nodeId <= 0) {
      setStatusText("Show in File Explorer becomes available when the background index has registered this file.");
      return;
    }
    try {
      await api.revealNodeExternal(nodeId);
      setStatusText("Showing the item in File Explorer.");
    } catch (error) {
      setStatusText(`Show in File Explorer failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  const revealProjectWithSystem = useCallback(async (projectId: number) => {
    try {
      await api.revealProjectExternal(projectId);
      setStatusText("Showing the project in File Explorer.");
    } catch (error) {
      setStatusText(`Show project in File Explorer failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  const revealSessionWithSystem = useCallback(async (path: string) => {
    try {
      await api.revealSessionExternal(path);
      setStatusText("Showing the conversation record in File Explorer.");
    } catch (error) {
      setStatusText(`Show conversation in File Explorer failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const startupAbort = new AbortController();
    const finishStartup = () => {
      window.setTimeout(() => {
        if (!cancelled) {
          setStartupProgress((current) => ({ ...current, active: false }));
        }
      }, 450);
    };

    const runStartup = async () => {
      const [startedHidden, backendWindowVisible] = await Promise.all([
        api.residentStartedHidden().catch(() => false),
        api.residentWindowVisible().catch(() => true)
      ]);
      if (cancelled) return;
      if (shouldDeferResidentUi(startedHidden, backendWindowVisible)) {
        setStartupProgress({
          active: false,
          label: "Running quietly in the background",
          detail: "Project freshness continues locally. Open Code Hangar when you need the interface.",
          progress: 0
        });
        setBackgroundStatus(null);
        setStatusText("Code Hangar is keeping projects ready in the background.");
        await waitForResidentUiActivation(startupAbort.signal);
        if (cancelled) return;
      }
      setStartupProgress({
        active: true,
        label: "Opening local inventory",
        detail: "Showing the app first. Local inventory opens in the background.",
        progress: 8
      });
      setBackgroundStatus("Opening local inventory.");
      try {
        await afterFirstPaint();
        if (cancelled) return;
        const cachedProjects = await api.projectsCachedSnapshot();
        if (cancelled) return;
        if (cachedProjects.length > 0) {
          if (shellOpenHasPriorityRef.current) {
            // The eager shell-open effect may already have published a
            // DB-independent Viewer while this cache read was in flight. Keep
            // that newer destination instead of replacing it with the older
            // snapshot and briefly clearing the requested document.
            setProjects((current) => {
              const cachedIds = new Set(cachedProjects.map((project) => project.id));
              return [...cachedProjects, ...current.filter((project) => !cachedIds.has(project.id))];
            });
          } else {
            setProjects(cachedProjects);
          }
          setProjectsFromCache(true);
          setStatusText(`Showing ${cachedProjects.length} cached projects while local inventory opens.`);
        }
        await yieldToUi();
        if (cancelled) return;
        let backendStatus = await api.startupStatus();
        while (!cancelled && backendStatus.state === "starting") {
          setStartupProgress({
            active: true,
            label: "Opening local inventory",
            detail: backendStatus.message,
            progress: Math.min(42, 10 + Math.floor(backendStatus.elapsedMs / 220))
          });
          setBackgroundStatus(backendStatus.message);
          await delay(140);
          if (cancelled) return;
          backendStatus = await api.startupStatus();
        }
        if (cancelled) return;
        if (backendStatus.state === "failed") {
          throw new Error(backendStatus.message);
        }
        setInventoryReady(true);
        setStartupProgress({
          active: true,
          label: "Inventory connection ready",
          detail: backendStatus.dbOpenMs != null
            ? `Local inventory opened in ${backendStatus.dbOpenMs} ms. Loading projects now.`
            : "Local inventory opened. Loading projects now.",
          progress: 46
        });
        // Drain Explorer/file-association requests before starting the first
        // expensive catalog enrichment. This is the hard latency boundary: a
        // requested Markdown body wins over catalog/session discovery.
        await drainShellOpenRequests();
        if (cancelled) return;
        await yieldToUi();
        if (cancelled) return;
        const loadedProjects = await api.projectsList();
        if (cancelled) return;
        if (shellOpenHasPriorityRef.current) {
          // `projectsList` may have taken its DB snapshot just before the shell
          // path registered a temporary/new root. Preserve that newer target;
          // the scan-completion refresh will reconcile the complete catalog.
          setProjects((current) => {
            const loadedIds = new Set(loadedProjects.map((project) => project.id));
            return [...loadedProjects, ...current.filter((project) => !loadedIds.has(project.id))];
          });
        } else {
          setProjects(loadedProjects);
        }
        setProjectsFromCache(false);
        if (!shellOpenHasPriorityRef.current) {
          setStartupProgress({
            active: true,
            label: "Navigation ready",
            detail: "Projects are visible. Choose a project when you want to load its files.",
            progress: 62
          });
        }
        if (!shellOpenHasPriorityRef.current) {
          setStatusText(`Loaded ${loadedProjects.length} projects. Choose one to load its files.`);
        }
        finishStartup();
        // This tutorial revision runs once per installed edition. Empty inventories
        // continue to hand off to Add Projects after the walkthrough.
        const inventoryHasRealProjects = !loadedProjects.every((project) => isDemoProject(project));
        if (!inventoryHasRealProjects) {
          setSessionInventoryState("fresh");
          setSessionInventoryRefreshing(false);
          setSessionInventoryError(null);
        }
        const tutorialSeen = window.localStorage.getItem(tutorialStorageKey) === "1";
        let hydratedFromCache = false;
        if (shellOpenHasPriorityRef.current) {
          // The explicitly opened file already chose the route. Never cover it
          // with first-run or startup-route UI.
        } else if (!tutorialSeen) {
          // The v2 walkthrough is edition-specific and runs once even when an
          // existing encrypted catalog already contains real projects.
          tourReplayOriginRef.current = null;
          setPaneCollapsed((current) => ({ ...current, left: false }));
          setTourMode("first-run");
        } else if (!inventoryHasRealProjects) {
          setAddProjectsVisible(true);
        } else {
          applyWorkspaceRoute(startupWorkspaceRoute(
            startupPreferences,
            storedStartupRouteRef.current,
            loadedProjects.map((project) => project.id)
          ));
        }
        setStartupRouteResolved(true);
        if (inventoryHasRealProjects) {
          // Paint the session grouping + Active/Archived split immediately from
          // the last cached discovery (the background rediscovery below replaces
          // it with fresh data a moment later). Project ids persist across
          // restarts, so the cached session→project links are still valid.
          const cachedReport = await loadCachedDiscoveryReport();
          if (cachedReport && !cancelled) {
            setProjectDiscoveryReport(cachedReport);
            setSessionInventory(cachedReport.sessions);
            setSessionInventoryState("cached");
            setSessionInventoryError(null);
            hydratedFromCache = true;
          }
        }

        await yieldToUi();
        if (cancelled) return;
        setBackgroundStatus("Loading recent items, roots and security state in the background.");

        const sideData = await loadStartupSideData({
          recentItems: api.recentItemsList,
          pinnedItems: api.pinnedItemsList,
          roots: api.rootsList,
          zones: api.zonesList,
          security: api.securityStatus
        });
        if (cancelled) return;
        if (sideData.data.recentItems) setRecentItems(sideData.data.recentItems);
        if (sideData.data.pinnedItems) setPinnedItems(sideData.data.pinnedItems);
        if (sideData.data.roots) setRoots(sideData.data.roots);
        if (sideData.data.zones) setZones(sideData.data.zones);
        if (sideData.data.security) setSecurity(sideData.data.security);

        const sideDataWarning = sideData.failures.length > 0
          ? `${sideData.failures.length} local metadata source${sideData.failures.length === 1 ? "" : "s"} could not be loaded: ${sideData.failures.map(({ key, message }) => `${key}: ${message}`).join("; ")}`
          : null;

        setStartupProgress({
          active: false,
          label: sideDataWarning ? "Local inventory ready with warnings" : "Local inventory ready",
          detail: sideDataWarning
            ?? "Projects and sidebar metadata are ready. Heavy summaries load only when opened.",
          progress: 100
        });
        setBackgroundStatus(null);
        if (!shellOpenHasPriorityRef.current) {
          setStatusText(sideDataWarning ? `Local inventory ready. ${sideDataWarning}` : "Local inventory ready.");
        }

        // Refresh the session grouping and Active/Archived split from a fresh
        // discovery on every launch. The cache hydrate above paints these
        // instantly; this rediscovery (registries + session metadata only — the
        // fast "mapped" path, not the heavy inventory walk) replaces the cached
        // view with current data and re-seeds the cache. Backgrounded so it never
        // delays the ready state; skipped when only demo fixtures exist. When no
        // cache was available it doubles as the first-time restore, so it
        // surfaces a status only in that case (otherwise it freshens silently).
        if (inventoryHasRealProjects) {
          window.setTimeout(() => {
            void (async () => {
              setSessionInventoryRefreshing(true);
              if (!hydratedFromCache) {
                setBackgroundStatus("Restoring sessions and project grouping.");
              }
              try {
                // Replay the include-options that produced the current inventory
                // ("Find Sessions" uses loose+agents), not the Deep Scan checkbox
                // defaults — otherwise Hermes/Independent session groups would
                // silently vanish from the sidebar on every restart.
                const include = readInventoryIncludeOptions();
                const restored = await api.projectDiscoveryReport(
                  500,
                  include?.loose ?? deepScanIncludeLoose,
                  include?.agents ?? deepScanIncludeAgents,
                  false,
                  0
                );
                if (cancelled) return;
                setProjectDiscoveryReport(restored);
                setSessionInventory(restored.sessions);
                setSessionInventoryState("fresh");
                setSessionInventoryError(null);
                const grouped = restored.sessions.length;
                if (!hydratedFromCache && grouped > 0) {
                  setStatusText(`Restored ${grouped} session${grouped === 1 ? "" : "s"} and project grouping.`);
                }
              } catch (error) {
                const message = error instanceof Error ? error.message : String(error);
                setSessionInventoryState(hydratedFromCache ? "cached" : "unavailable");
                setSessionInventoryError(message);
                setStatusText(hydratedFromCache
                  ? `The saved session index is still available, but its local refresh failed: ${message}`
                  : `Local sessions could not be restored: ${message}`);
              } finally {
                if (!cancelled) setSessionInventoryRefreshing(false);
                if (!cancelled && !hydratedFromCache) setBackgroundStatus(null);
              }
            })();
          }, 1200);
        }
      } catch (error) {
        if (cancelled) return;
        const message = error instanceof Error ? error.message : String(error);
        setSessionInventoryState("unavailable");
        setSessionInventoryRefreshing(false);
        setSessionInventoryError(message);
        setInventoryReady(false);
        setStartupProgress({
          active: false,
          label: "Startup failed",
          detail: message,
          progress: 100
        });
        setBackgroundStatus(null);
        setStatusText(`Startup failed: ${message}`);
        setStartupRouteResolved(true);
      }
    };

    void runStartup();
    return () => {
      cancelled = true;
      startupAbort.abort();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- startup runs once on mount; it intentionally captures the initial deepScanInclude* toggle values and must NOT re-run when they change
  }, []);

  useEffect(() => {
    if (!inventoryReady) return;
    if (dashboard && adapters.length > 0) return;
    if (primaryView !== "overview" && rightPaneView !== "dashboard") return;
    if (dashboardAutoLoadAttemptedRef.current) return;

    let cancelled = false;
    const timer = window.setTimeout(() => {
      void afterFirstPaint().then(() => {
        if (!cancelled) {
          dashboardAutoLoadAttemptedRef.current = true;
          void loadDashboardData();
        }
      });
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [adapters.length, dashboard, inventoryReady, loadDashboardData, primaryView, rightPaneView]);

  useEffect(() => {
    if (!inventoryReady || !selectedProjectId || projectView !== "space") return;
    if (selectedFootprint || dashboardLoading) return;
    void loadDashboardData(true);
  }, [dashboardLoading, inventoryReady, loadDashboardData, projectView, selectedFootprint, selectedProjectId]);

  useEffect(() => {
    if (!selectedProjectId || selectedProjectId <= 0) return;
    if (!inventoryReady) {
      setStatusText("Local inventory is still opening. The selected project will load automatically when ready.");
      return;
    }
    // Load directly. The previous afterFirstPaint() + timer + cancelled dance gated the load behind a
    // requestAnimationFrame that WebView2 pauses whenever the window is occluded/backgrounded, leaving
    // the spinner stuck with no backend call. loadProjectData is async and self-guards stale loads, so
    // running it immediately on selection is both simpler and robust to window-occlusion state.
    void loadProjectData(selectedProjectId as number);
  }, [inventoryReady, loadProjectData, selectedProjectId]);

  useEffect(() => {
    const pending = pendingTreeRevealRef.current;
    if (!pending || projectWorkspace.loadStatus !== "ready" || selectedProjectId !== pending.projectId) return;
    void revealNodeInLoadedTree(pending.projectId, pending.nodeId);
  }, [projectWorkspace.loadStatus, revealNodeInLoadedTree, selectedProjectId]);

  useEffect(() => {
    if (selectedProjectId === null) return;
    if (displayedProjects.some((project) => project.id === selectedProjectId)) {
      return;
    }
    selectionSeq.current += 1;
    beginProject(null);
    setSelectedProjectId(null);
    setPreview(null);
    setFolderExplanation(null);
    setRelationships(null);
    setRelationshipsMembership(null);
    setRelationshipsLoading(false);
    setFileOrphanLoading(false);
    setTabs([]);
    setBackStack([]);
    setForwardStack([]);
  }, [beginProject, displayedProjects, selectedProjectId]);

  useEffect(() => {
    if (projectWorkspace.loadStatus === "error" && projectWorkspace.error) {
      setStatusText(`Project load failed: ${projectWorkspace.error}`);
    }
  }, [projectWorkspace.error, projectWorkspace.loadStatus]);

  useEffect(() => {
    if (!selectedProjectId) return;
    if (projectView !== "context") return;
    if (projectWorkspace.loadStatus !== "ready") return;
    if (preview || folderExplanation) return;
    if (manualPreviewClearProjectRef.current === selectedProjectId) return;
    const initialContext = selectInitialContextFile(contextFiles);
    if (!initialContext) {
      setStatusText("Project loaded. No priority context file is available yet.");
      return;
    }
    void openNode(initialContext.nodeId, {
      ...INITIAL_CONTEXT_OPEN_OPTIONS,
      projectId: initialContext.projectId
    });
  }, [contextFiles, folderExplanation, openNode, preview, projectView, projectWorkspace.loadStatus, selectedProjectId]);

  useEffect(() => {
    if (!preview) return;
    const expectedNodeId = preview.nodeId;
    const expectedProjectId = preview.projectId;
    const expectedSeq = selectionSeq.current;
    // "edit" is a frontend-only view; the backend still serves the file's source for it.
    const backendMode = previewMode === "edit" || previewMode === "values" ? "source" : previewMode;
    const next = preview.nodeId <= 0
      ? refreshUnindexedPreview(
          preview.projectId,
          preview.path,
          async (path) => {
            const direct = await api.openLocalFilePreviewFull(path, backendMode, previewPolicy);
            if (!direct) {
              throw new Error("The provisional local file is no longer available.");
            }
            return direct.preview;
          },
          (projectId, path) => api.openTargetPreview(projectId, path, backendMode, previewPolicy)
        )
      : preview.wasRevealed && previewPolicy.allowSensitiveReveal
        ? api.fileReveal(preview.nodeId, backendMode, previewPolicy, preview.projectId)
        : api.filePreview(preview.nodeId, backendMode, false, previewPolicy, preview.projectId);
    void next
      .then((nextPreview) => {
        if (expectedSeq !== selectionSeq.current) return;
        if (expectedNodeId > 0 && nextPreview.nodeId !== expectedNodeId) return;
        if (nextPreview.projectId !== expectedProjectId) return;
        const resolvedPreview = nextPreview.nodeId <= 0 && expectedNodeId < 0
          ? { ...nextPreview, nodeId: expectedNodeId }
          : nextPreview;
        if (expectedNodeId < 0 && resolvedPreview.nodeId > 0) {
          transientShellNodeIdsRef.current.delete(`${resolvedPreview.projectId}:${normalizeProjectRootPath(resolvedPreview.path)}`);
          setTabs((current) => current.map((tab) => (
            tab.nodeId === expectedNodeId && tab.projectId === expectedProjectId
            ? {
                nodeId: resolvedPreview.nodeId,
                projectId: resolvedPreview.projectId,
                label: resolvedPreview.displayName,
                path: resolvedPreview.path
              }
            : tab
          )));
        }
        setPreview(resolvedPreview);
      })
      .catch((error) => {
        if (expectedSeq !== selectionSeq.current) return;
        setStatusText(`Preview refresh failed: ${error instanceof Error ? error.message : String(error)}`);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional: re-runs only when preview mode/policy change, re-fetching whatever preview is current; depending on `preview` would re-fetch on every selection
  }, [previewMode, previewPolicy]);

  // Seed the edit buffer from the file's source the first time Edit opens for a file; clear it when
  // leaving Edit. Keyed by node so a save (which only updates source) does not clobber the draft.
  useEffect(() => {
    if (previewMode !== "edit") {
      if (editDraft !== null) setEditDraft(null);
      editDraftNodeRef.current = null;
      return;
    }
    // Seed ONLY once the file's source has actually loaded. Entering Edit from Rendered mode
    // triggers a source refetch; seeding from the still-source-less preview would leave the editor
    // empty AND marked dirty — and a Save then would write empty content over the file. While the
    // source is still loading editDraft stays null (Save disabled, never dirty).
    if (preview?.state === "ready" && preview.source != null && editDraftNodeRef.current !== preview.nodeId) {
      setEditDraft(preview.source);
      editDraftNodeRef.current = preview.nodeId;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional: re-seeds only on entering edit or switching file
  }, [previewMode, preview?.nodeId, preview?.state, preview?.source]);

  const saveEditedFile = useCallback(async (reviewedAfterHash: string): Promise<boolean> => {
    if (!changesUnlocked) {
      setStatusText("Changes are locked. Unlock this project before applying a reviewed file change.");
      return false;
    }
    if (!preview || editDraft === null) return false;
    const node = preview.nodeId;
    const previousContent = preview.source ?? "";
    if (editDraft === previousContent) {
      setStatusText("No changes to save.");
      return false;
    }
    setEditSaving(true);
    try {
      // The command returns the EXACT prior bytes it read on disk — use those for Undo, not the
      // possibly size-capped preview, so Undo can never truncate the file.
      const previousOnDisk = await api.writeFileContent(node, editDraft, "manual", previousContent, reviewedAfterHash);
      setEditUndo({ nodeId: node, previous: previousOnDisk, applied: editDraft });
      // Optimistically update the in-memory baseline so the editor is no longer "dirty"; drop the
      // cached renders so a later Rendered/Source view re-fetches the new bytes.
      setPreview((current) => (current && current.nodeId === node ? { ...current, source: editDraft } : current));
      invalidatePreviewCache(node);
      setStatusText(`Applied ${preview.displayName}. Previous version created.`);
      return true;
    } catch (error) {
      setStatusText(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setEditSaving(false);
    }
  }, [changesUnlocked, preview, editDraft, invalidatePreviewCache]);

  const revertEditedFile = useCallback(() => {
    setEditDraft(preview?.source ?? "");
  }, [preview?.source]);

  const undoEditedFile = useCallback(async () => {
    if (!editUndo) return;
    if (!changesUnlocked) {
      setStatusText("Changes are locked. Unlock this project before restoring a previous file version.");
      return;
    }
    if (!(await requestConfirm(
      "Undo changes the real file back to its verified previous content. The current bytes are checked first and the action refuses a stale file. Continue?",
      { confirmLabel: "Undo this file change", tone: "danger" }
    ))) return;
    const { nodeId, previous, applied } = editUndo;
    setEditSaving(true);
    try {
      await api.writeFileContent(nodeId, previous, "restore", applied);
      setEditUndo(null);
      setPreview((current) => (current && current.nodeId === nodeId ? { ...current, source: previous } : current));
      if (editDraftNodeRef.current === nodeId) setEditDraft(previous);
      invalidatePreviewCache(nodeId);
      setStatusText("Reverted to the previous saved version.");
    } catch (error) {
      setStatusText(`Undo failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setEditSaving(false);
    }
  }, [changesUnlocked, editUndo, invalidatePreviewCache, requestConfirm]);

  useEffect(() => {
    if (projectView !== "connections" || !preview || preview.nodeId <= 0) return;
    const membership = fileMembershipKey(preview.projectId, preview.nodeId);
    if (relationshipsLoading || relationshipsMembership === membership) return;
    loadRelationshipsInBackground(preview.projectId, preview.nodeId, selectionSeq.current, preview.displayName);
  }, [loadRelationshipsInBackground, preview, projectView, relationshipsLoading, relationshipsMembership]);

  useEffect(() => {
    if (projectView !== "connections" || !selectedProjectId) {
      graphMapExpansionPauseRef.current = true;
      graphMapExpansionRunRef.current += 1;
      return;
    }
    let cancelled = false;
    graphMapExpansionPauseRef.current = false;
    graphMapExpansionRunRef.current += 1;
    setGraphMap(null);
    graphMapRef.current = null;
    setGraphMapError(null);
    setGraphMapLoading(true);
    setGraphMapExpansion({ status: "idle", loadedItems: 0, totalItems: 0, message: null });
    void api.projectGraphMap(selectedProjectId, INITIAL_GRAPH_MAP_LIMIT)
      .then((nextMap) => {
        if (!cancelled) {
          graphMapRef.current = nextMap;
          setGraphMap(nextMap);
          setGraphMapExpansion({ status: "idle", ...graphMapItemCounts(nextMap), message: null });
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setGraphMapError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (!cancelled) setGraphMapLoading(false);
      });
    return () => {
      cancelled = true;
      graphMapExpansionPauseRef.current = true;
      graphMapExpansionRunRef.current += 1;
    };
  }, [graphMapRevision, projectView, selectedProjectId]);

  const runGraphMapExpansion = useCallback(async (askForConfirmation: boolean) => {
    const projectId = selectedProjectId;
    let currentMap = graphMapRef.current;
    if (!projectId || !currentMap || currentMap.projectId !== projectId) return;
    const currentCounts = graphMapItemCounts(currentMap);
    if (currentCounts.loadedItems >= currentCounts.totalItems) {
      setGraphMapExpansion({ status: "complete", ...currentCounts, message: "The complete local map is loaded." });
      return;
    }
    if (askForConfirmation) {
      const remaining = currentCounts.totalItems - currentCounts.loadedItems;
      const confirmed = await requestConfirm(
        `Load the complete Hangar Map?\n\n${remaining} more mapped item${remaining === 1 ? "" : "s"} will be read from Code Hangar's local inventory in batches. This can temporarily increase CPU and memory use, especially when model headers are inspected. No project file will be changed.`,
        { confirmLabel: "Load complete map" }
      );
      if (!confirmed) return;
    }

    const runId = graphMapExpansionRunRef.current + 1;
    graphMapExpansionRunRef.current = runId;
    graphMapExpansionPauseRef.current = false;
    setGraphMapExpansion({ status: "loading", ...currentCounts, message: "Loading the next local batch..." });

    try {
      while (currentMap.nodes.length < currentMap.totalNodes) {
        if (graphMapExpansionRunRef.current !== runId) return;
        if (graphMapExpansionPauseRef.current) {
          setGraphMapExpansion({ status: "paused", ...graphMapItemCounts(currentMap), message: "Paused between batches." });
          return;
        }

        const nextLimit = nextGraphMapExpansionLimit(currentMap);
        if (nextLimit === null) break;
        const nextMap = await api.projectGraphMap(projectId, nextLimit);
        if (graphMapExpansionRunRef.current !== runId) return;
        if (nextMap.nodes.length <= currentMap.nodes.length && nextMap.totalNodes > nextMap.nodes.length) {
          throw new Error("The complete map exceeds the in-app safety limit. The loaded portion remains available.");
        }

        currentMap = nextMap;
        graphMapRef.current = nextMap;
        setGraphMap(nextMap);
        const counts = graphMapItemCounts(nextMap);
        setGraphMapExpansion({
          status: graphMapExpansionPauseRef.current ? "pausing" : "loading",
          ...counts,
          message: graphMapExpansionPauseRef.current ? "Finishing the current batch before pausing..." : "Loading the next local batch..."
        });
        if (graphMapExpansionPauseRef.current) {
          setGraphMapExpansion({ status: "paused", ...counts, message: "Paused between batches." });
          return;
        }
        await new Promise<void>((resolve) => window.setTimeout(resolve, 50));
      }

      const finalCounts = graphMapItemCounts(currentMap);
      setGraphMapExpansion({ status: "complete", ...finalCounts, message: "The complete local map is loaded." });
    } catch (error) {
      const counts = graphMapItemCounts(currentMap);
      setGraphMapExpansion({
        status: "error",
        ...counts,
        message: error instanceof Error ? error.message : String(error)
      });
    }
  }, [requestConfirm, selectedProjectId]);

  const pauseGraphMapExpansion = useCallback(() => {
    graphMapExpansionPauseRef.current = true;
    setGraphMapExpansion((current) => current.status === "loading"
      ? { ...current, status: "pausing", message: "Finishing the current batch before pausing..." }
      : current);
  }, []);

  useEffect(() => {
    // Start empty and only search once there are a couple of characters. The
    // backend combines terms across file names, paths and owning projects.
    const trimmed = quickQuery.trim();
    if (trimmed.length < 2) {
      setQuickResults([]);
      setQuickSearchStatus("idle");
      return;
    }
    let cancelled = false;
    setQuickResults([]);
    setQuickSearchStatus("loading");
    const timer = window.setTimeout(() => {
      void api.quickOpen(trimmed)
        .then((results) => {
          if (cancelled) return;
          setQuickResults(results);
          setQuickSearchStatus("idle");
        })
        .catch(() => {
          if (cancelled) return;
          setQuickResults([]);
          setQuickSearchStatus("error");
        });
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [quickQuery]);

  const goBack = useCallback(async () => {
    if (shellViewerRef.current) return;
    const route = viewBackStack.at(-1);
    if (route) {
      const current = currentWorkspaceRoute();
      setViewBackStack((stack) => stack.slice(0, -1));
      setViewForwardStack((stack) => [...stack, current]);
      applyWorkspaceRoute(route);
      return;
    }
    const target = backStack.at(-1);
    if (!target) return;
    setBackStack((stack) => stack.slice(0, -1));
    // With no open preview (e.g. after closing all tabs) there is nothing to put on
    // the forward stack — still reopen the last file instead of a dead Back button.
    if (preview) setForwardStack((stack) => appendFileHistoryEntry(stack, preview, target));
    await openNode(target.nodeId, { projectId: target.projectId, replaceHistory: true });
  }, [applyWorkspaceRoute, backStack, currentWorkspaceRoute, openNode, preview, viewBackStack]);

  const goForward = useCallback(async () => {
    if (shellViewerRef.current) return;
    const route = viewForwardStack.at(-1);
    if (route) {
      const current = currentWorkspaceRoute();
      setViewForwardStack((stack) => stack.slice(0, -1));
      setViewBackStack((stack) => [...stack, current]);
      applyWorkspaceRoute(route);
      return;
    }
    const target = forwardStack.at(-1);
    if (!target) return;
    setForwardStack((stack) => stack.slice(0, -1));
    if (preview) setBackStack((stack) => appendFileHistoryEntry(stack, preview, target));
    await openNode(target.nodeId, { projectId: target.projectId, replaceHistory: true });
  }, [applyWorkspaceRoute, currentWorkspaceRoute, forwardStack, openNode, preview, viewForwardStack]);

  const openQuickOpen = useCallback((returnFocus?: HTMLElement | null) => {
    const activeElement = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const candidate = returnFocus ?? activeElement;
    quickOpenReturnFocusRef.current = candidate?.closest('[role="dialog"]')
      ? quickOpenButtonRef.current
      : candidate ?? quickOpenButtonRef.current;
    setContextMenu(null);
    setQuickQuery("");
    setQuickResults([]);
    setQuickSearchStatus("idle");
    setQuickOpenVisible(true);
  }, []);

  const openCommandPalette = useCallback((returnFocus?: HTMLElement | null) => {
    const activeElement = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    commandReturnFocusRef.current = returnFocus ?? activeElement ?? commandButtonRef.current;
    setContextMenu(null);
    setCommandVisible(true);
  }, []);

  const paletteShortcutBlocked = paletteShortcutsBlocked({
    quickOpen: quickOpenVisible,
    commands: commandVisible,
    addProjects: addProjectsVisible,
    tour: tourActive,
    deepScan: deepScanOverlayVisible,
    resetAll: resetAllVisible,
    removeProject: removeProjectTarget !== null,
    extensionOverlay: editionOverlayOpen,
    confirmation: confirmRequest !== null,
    recovery: Boolean(recoveryState?.pending && !recoveryFrozen)
  });

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const paletteShortcut = globalPaletteShortcut(
        event.key,
        event.ctrlKey || event.metaKey
      );
      if (paletteShortcut && paletteShortcutBlocked) {
        event.preventDefault();
        return;
      }
      if (paletteShortcut === "quick-open") {
        event.preventDefault();
        openQuickOpen();
        return;
      }
      if (paletteShortcut === "commands") {
        event.preventDefault();
        openCommandPalette();
        return;
      }
      if (paletteShortcutBlocked || contextMenu || isEditableTarget(event.target)) {
        return;
      }
      if (event.altKey && event.key === "ArrowLeft") {
        event.preventDefault();
        void goBack();
      }
      if (event.altKey && event.key === "ArrowRight") {
        event.preventDefault();
        void goForward();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [contextMenu, goBack, goForward, openCommandPalette, openQuickOpen, paletteShortcutBlocked]);

  // Safety net: surface any stray rejected invoke (e.g. a floating `void handler()`
  // whose backend call failed) to the status bar instead of dropping it silently.
  useEffect(() => {
    const onRejection = (event: PromiseRejectionEvent) => {
      const reason = event.reason;
      const message = reason instanceof Error ? reason.message : String(reason);
      console.error("Unhandled promise rejection:", reason);
      setStatusText(`Unexpected background error: ${message}`);
    };
    const onError = (event: ErrorEvent) => {
      console.error("Unhandled error:", event.error ?? event.message);
    };
    window.addEventListener("unhandledrejection", onRejection);
    window.addEventListener("error", onError);
    return () => {
      window.removeEventListener("unhandledrejection", onRejection);
      window.removeEventListener("error", onError);
    };
  }, [setStatusText]);

  const updateFilePin = useCallback(async (
    nodeId: number,
    projectId: number,
    label: string,
    currentlyPinned: boolean
  ) => {
    const nextPinned = !currentlyPinned;
    try {
      if (currentlyPinned) await api.unpinItem(nodeId, "file", projectId);
      else await api.pinItem(nodeId, "file", projectId);
    } catch (error) {
      setStatusText(pinFailureMessage(label, nextPinned, error));
      return;
    }

    const successMessage = pinSuccessMessage(label, nextPinned);
    try {
      await refreshSideData();
      setStatusText(successMessage);
    } catch (error) {
      setStatusText(`${successMessage} Could not refresh the sidebar: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [refreshSideData]);

  const updateProjectPin = useCallback(async (project: ProjectSummary) => {
    const nextPinned = !project.pinned;
    try {
      if (project.pinned) await api.unpinItem(project.id, "project");
      else await api.pinItem(project.id, "project");
    } catch (error) {
      setStatusText(pinFailureMessage(project.name, nextPinned, error));
      return;
    }

    const successMessage = pinSuccessMessage(project.name, nextPinned);
    try {
      await Promise.all([loadProjects(), refreshSideData()]);
      setStatusText(successMessage);
    } catch (error) {
      setStatusText(`${successMessage} Could not refresh project navigation: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [loadProjects, refreshSideData]);

  const togglePin = useCallback(async () => {
    if (!preview || preview.nodeId <= 0) {
      setStatusText("Pinning becomes available as soon as the background index has registered this file.");
      return;
    }
    await updateFilePin(preview.nodeId, preview.projectId, preview.displayName, selectedPinned);
  }, [preview, selectedPinned, updateFilePin]);

  // Auto-register the strong, deliberate candidates from a discovery report —
  // projects an AI app already lists in its registry, or folders a local session
  // has worked in — that aren't already registered and don't overlap a root,
  // then start scanning them. Returns how many were added. Weaker candidates are
  // left in the report for manual review. Shared by the one-click global Deep
  // Scan and the folder-scoped search.
  const autoRegisterStrongCandidates = useCallback(
    async (result: ProjectDiscoveryReport): Promise<{
      addedCount: number;
      jobId: string | null;
      inventoryMessage: string | null;
      retryRootIds: number[];
      inventoryStartError: string | null;
    }> => {
      const strong = result.candidates.filter((candidate) =>
        !candidate.alreadyRegistered
        && candidate.overlapKind === "none"
        // Never auto-add technical candidates (ComfyUI custom_nodes, agent skill
        // folders, dependency dirs an AI session happened to run in). They are
        // still listed below for manual review — just not registered for you.
        && candidate.projectKind !== "technical_candidate"
        // Auto-add real projects: a folder an AI app catalogues as a project, or
        // one a local session actually worked in. Bundled tool examples (pinokio
        // demos, etc.) are filtered at the discovery source, so this stays
        // session-inclusive — which is what lets a project's sessions group under
        // it instead of showing as independent.
        && candidate.signals.some((signal) =>
          signal.kind === "session_path" || signal.kind === "app_project_registry"
        )
      );
      // Auto-add only the top-most of any nested set: when one strong candidate
      // sits inside another (a test fixture under its parent project, a
      // sub-package under a monorepo root), the parent already covers it, so
      // registering the child would just duplicate inventory.
      const normalizePath = (value: string) =>
        value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
      const isNestedUnder = (childPath: string, parentPath: string) => {
        const child = normalizePath(childPath);
        const parent = normalizePath(parentPath);
        return child !== parent && child.startsWith(`${parent}\\`);
      };
      const autoAdd = strong.filter((candidate) =>
        !strong.some((other) => other !== candidate && isNestedUnder(candidate.path, other.path))
      );
      if (autoAdd.length === 0) {
        return {
          addedCount: 0,
          jobId: null,
          inventoryMessage: null,
          retryRootIds: [] as number[],
          inventoryStartError: null as string | null
        };
      }
      const addedRoots: typeof roots = [];
      const addedPaths = new Set<string>();
      let registrationFailures = 0;
      for (const candidate of autoAdd) {
        try {
          const root = await api.rootsAdd(candidate.path);
          addedRoots.push(root);
          addedPaths.add(candidate.path);
        } catch {
          registrationFailures += 1;
        }
      }
      if (addedRoots.length === 0) {
        return {
          addedCount: 0,
          jobId: null,
          inventoryMessage: registrationFailures > 0 ? "No strong candidate could be registered." : null,
          retryRootIds: [] as number[],
          inventoryStartError: null as string | null
        };
      }
      setRoots((current) => {
        const known = new Set(current.map((root) => root.id));
        return [...current, ...addedRoots.filter((root) => !known.has(root.id))];
      });
      let loaded: ProjectSummary[] | null = null;
      let navigationWarning: string | null = null;
      try {
        loaded = await api.projectsListLite();
        setProjects(loaded);
        setProjectsFromCache(false);
      } catch (error) {
        navigationWarning = `Project navigation could not refresh yet: ${error instanceof Error ? error.message : String(error)}`;
      }
      setProjectDiscoveryReport((current) => current ? {
        ...current,
        candidates: current.candidates.map((item) => addedPaths.has(item.path) ? {
          ...item,
          alreadyRegistered: true,
          existingProjectId: loaded?.find((project) => project.path === item.path)?.id ?? item.existingProjectId ?? null,
          sourceKinds: Array.from(new Set([...item.sourceKinds, "code_hangar_registered"]))
        } : item)
      } : current);
      let jobId: string | null = null;
      const retryRootIds = addedRoots.map((root) => root.id);
      const startAttempt = await attemptDeepScanInventoryStart(retryRootIds, startInventoryForRoots);
      let inventoryStartError: string | null = null;
      let inventoryMessage: string;
      if (startAttempt.kind === "started") {
        jobId = startAttempt.status.jobId;
        inventoryMessage = startAttempt.status.message || "Inventory scan started.";
      } else {
        inventoryStartError = startAttempt.error;
        inventoryMessage = `Inventory scan could not start: ${startAttempt.error}`;
      }
      if (registrationFailures > 0) {
        inventoryMessage += ` ${registrationFailures} strong candidate${registrationFailures === 1 ? "" : "s"} could not be registered.`;
      }
      if (navigationWarning) inventoryMessage += ` ${navigationWarning}`;
      void refreshSideData();
      return {
        addedCount: addedRoots.length,
        jobId,
        inventoryMessage,
        retryRootIds,
        inventoryStartError
      };
    },
    [refreshSideData, startInventoryForRoots]
  );

  // The one-click Deep Scan: read every local AI app's project registry across
  // Windows and WSL — no folder to pick — and surface a rewarding progress panel
  // while it maps everything. Honours the two opt-in toggles (loose sessions,
  // agents). Strong matches are auto-added; the rest are listed for review.
  const runGlobalDeepScan = useCallback(async () => {
    if (projectDiscoveryLoading || (deepScanProgress && deepScanProgress.phase !== "done")) {
      setAddProjectsVisible(false);
      if (deepScanProgress) setDeepScanOverlayVisible(true);
      setStatusText("A Deep Scan is already running.");
      return;
    }
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
    setStatusText("Applying the confirmed WSL scope before Deep Scan…");
    let sessionsDiscovered = false;
    try {
      const result = await startWslGatedProjectDiscovery("global", async (appliedWslScan) => {
        // Nothing in this callback can run until set + read-back verification
        // succeeds. A preference failure therefore leaves the dialog in place.
        setAddProjectsVisible(false);
        setDeepScanOverlayVisible(true);
        showDiscover("projects");
        setProjectDiscoveryReport(null);
        setSessionInventoryRefreshing(true);
        setSessionInventoryError(null);
        setDeepScanProgress({
          stages: initialDeepScanStages(installedApps, appliedWslScan),
          phase: "scanning",
          outcome: null,
          projectsFound: 0,
          sessionsFound: 0,
          addedCount: 0,
          note: appliedWslScan
            ? "Reading local AI app registries across Windows and WSL…"
            : "Reading local AI app registries on this PC…"
        });
        return api.projectDiscoveryReport(500, deepScanIncludeLoose, deepScanIncludeAgents, false, 0);
      });
      setProjectDiscoveryReport(result);
      // Populate the sidebar's session inventory too, so a Deep Scan surfaces the
      // sessions (grouped under their projects) — not just the project list.
      setSessionInventory(result.sessions);
      setSessionInventoryState("fresh");
      setSessionInventoryError(null);
      sessionsDiscovered = true;
      persistInventoryIncludeOptions(deepScanIncludeLoose, deepScanIncludeAgents);
      setDeepScanProgress((current) => current ? {
        ...current,
        phase: "registering",
        projectsFound: result.totalCandidates,
        sessionsFound: result.totalSessions,
        note: "Adding the projects your AI apps already know…"
      } : current);
      const {
        addedCount,
        jobId,
        inventoryMessage,
        retryRootIds,
        inventoryStartError
      } = await autoRegisterStrongCandidates(result);
      // The first discovery ran before these projects were registered, so its
      // sessions only knew them as "not added yet" (no registered-id link). Now
      // that they're registered, re-read discovery in the background so every
      // session links to its project — grouping the sidebar sessions under their
      // projects and driving the Active/Archived split. Cheap relative to the
      // inventory scan already running, and it never blocks the progress panel.
      if (addedCount > 0) {
        void api
          .projectDiscoveryReport(500, deepScanIncludeLoose, deepScanIncludeAgents, false, 0)
          .then((linked) => {
            setProjectDiscoveryReport(linked);
            setSessionInventory(linked.sessions);
          })
          .catch(() => {
            /* keep the pre-registration view if the refresh fails */
          });
      }
      const reviewable = result.totalCandidates - addedCount;
      const inventorySummary = jobId
        ? inventoryMessage ?? "Inventory scan started."
        : addedCount > 0
          ? `Projects were added, but their inventory did not start: ${inventoryStartError ?? "no scan job was returned"}.`
          : inventoryMessage;
      setStatusText(
        addedCount > 0
          ? `Deep Scan added ${addedCount} project${addedCount === 1 ? "" : "s"} automatically. ${inventorySummary}${reviewable > 0 ? ` ${reviewable} more candidate${reviewable === 1 ? "" : "s"} listed for review.` : ""}`
          : `Deep Scan mapped ${result.totalCandidates} project candidate${result.totalCandidates === 1 ? "" : "s"}.${inventorySummary ? ` ${inventorySummary}` : ""} Review before adding.`
      );
      if (jobId) {
        // Carry the loved overlay straight into a rewarding "building inventory"
        // phase. It shows live scan progress and stays until the scan finishes (an
        // effect dismisses it) — or the user hides it to keep working meanwhile.
        setDeepScanProgress((current) => current ? {
          ...current,
          phase: "building",
          outcome: null,
          addedCount,
          scanJobId: jobId,
          retryRootIds,
          note: addedCount === 1
            ? "Indexing 1 project so its files and context are ready."
            : `Indexing ${addedCount} projects so their files and context are ready.`
        } : current);
      } else {
        setDeepScanProgress((current) => current ? {
          ...current,
          phase: "done",
          outcome: addedCount > 0 ? "inventory-not-started" : "mapped",
          addedCount,
          scanJobId: null,
          retryRootIds,
          note: addedCount > 0
            ? `${addedCount} project${addedCount === 1 ? " was" : "s were"} added, but inventory did not start: ${inventoryStartError ?? "no scan job was returned"}.${reviewable > 0 ? ` ${reviewable} more to review.` : ""}`
            : "Review the candidates below and add the ones you want."
        } : current);
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (isWslScanPreferenceApplyError(error)) {
        setDeepScanProgress(null);
        setDeepScanOverlayVisible(false);
        setAddProjectsVisible(true);
        setProjectDiscoveryError(message);
        setStatusText(`Deep Scan did not start because its WSL scope could not be applied safely: ${message}`);
        return;
      }
      setDeepScanProgress((current) => current ? {
        ...current,
        phase: "done",
        outcome: "failed",
        scanJobId: null,
        retryRootIds: [],
        note: `Deep Scan could not finish: ${message}`
      } : {
        stages: initialDeepScanStages(installedApps, wslConfirmedChoiceRef.current),
        phase: "done",
        outcome: "failed",
        projectsFound: 0,
        sessionsFound: 0,
        addedCount: 0,
        note: `Deep Scan could not finish: ${message}`,
        scanJobId: null,
        retryRootIds: []
      });
      setDeepScanOverlayVisible(true);
      setProjectDiscoveryError(message);
      if (!sessionsDiscovered) {
        setSessionInventoryState((current) => current === "fresh" || current === "cached" ? "cached" : "unavailable");
        setSessionInventoryError(message);
      }
      setStatusText(`Deep Scan failed: ${message}`);
    } finally {
      setSessionInventoryRefreshing(false);
      setProjectDiscoveryLoading(false);
    }
  }, [autoRegisterStrongCandidates, deepScanIncludeAgents, deepScanIncludeLoose, deepScanProgress, installedApps, projectDiscoveryLoading, showDiscover, startWslGatedProjectDiscovery]);

  const recoverDeepScan = useCallback(async () => {
    if (!deepScanProgress || deepScanProgress.phase !== "done") return;
    const presentation = deepScanTerminalPresentation(deepScanProgress.phase, deepScanProgress.outcome);
    if (!presentation.action) return;
    const retryRootIds = deepScanProgress.retryRootIds?.length
      ? deepScanProgress.retryRootIds
      : buildScanStatus?.rootIds ?? [];
    if (retryRootIds.length === 0) {
      // A discovery-stage failure has no inventory roots to resume. Retry the
      // same guarded Deep Scan flow instead of inventing a backend resume API.
      setDeepScanProgress(null);
      await runGlobalDeepScan();
      return;
    }
    const continuing = presentation.action === "resume";
    setDeepScanOverlayVisible(true);
    setDeepScanProgress((current) => current ? {
      ...current,
      phase: "building",
      outcome: null,
      scanJobId: null,
      retryRootIds,
      note: continuing
        ? "Resuming the retained local inventory for the affected projects…"
        : "Retrying inventory for the projects that were already added…"
    } : current);
    const attempt = await attemptDeepScanInventoryStart(retryRootIds, startInventoryForRoots);
    if (attempt.kind === "started") {
      setDeepScanProgress((current) => current ? {
        ...current,
        phase: "building",
        outcome: null,
        scanJobId: attempt.status.jobId,
        retryRootIds,
        note: continuing
          ? "Inventory scan resumed. Existing local inventory is retained while remaining work is indexed."
          : "Inventory retry started for the projects already added."
      } : current);
      setStatusText(attempt.status.message || (continuing ? "Inventory scan resumed." : "Inventory retry started."));
      return;
    }
    setDeepScanProgress((current) => current ? {
      ...current,
      phase: "done",
      outcome: "inventory-not-started",
      scanJobId: null,
      retryRootIds,
      note: `The projects remain added, but inventory still did not start: ${attempt.error}`
    } : current);
    setStatusText(`Inventory did not start: ${attempt.error}`);
  }, [buildScanStatus?.rootIds, deepScanProgress, runGlobalDeepScan, startInventoryForRoots]);

  // Folder-scoped search (the Add Project ▸ root-folder path): pick a folder or
  // drive and scan it for projects, auto-adding the strong matches.
  const chooseDeepDiscoveryRoot = useCallback(async () => {
    let folder: string | null;
    try {
      folder = await api.pickFolder("Choose a folder or drive to search for projects");
    } catch (error) {
      setStatusText(`Could not open the folder picker: ${error instanceof Error ? error.message : String(error)}`);
      return;
    }
    if (!folder) {
      setStatusText("Folder search cancelled.");
      return;
    }
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
    setStatusText("Applying the confirmed WSL scope before folder discovery…");
    try {
      const result = await startWslGatedProjectDiscovery("folder", async () => {
        setAddProjectsVisible(false);
        setProjectDiscoveryReport(null);
        showDiscover("projects");
        setStatusText(`Searching ${folder} for projects. Strong matches are added automatically.`);
        await yieldToUi();
        return api.projectDiscoveryDeepScan(folder, 500, deepScanIncludeLoose, deepScanIncludeAgents, false, 0);
      });
      setProjectDiscoveryReport(result);
      const { addedCount, inventoryMessage } = await autoRegisterStrongCandidates(result);
      if (addedCount > 0) markSessionInventoryNeedsRefresh();
      const reviewable = result.totalCandidates - addedCount;
      setStatusText(
        addedCount > 0
          ? `Added ${addedCount} project${addedCount === 1 ? "" : "s"} automatically under ${folder}. ${inventoryMessage ?? "Inventory is ready."}${reviewable > 0 ? ` ${reviewable} more candidate${reviewable === 1 ? "" : "s"} below need review.` : ""}`
          : `Found ${result.totalCandidates} candidate${result.totalCandidates === 1 ? "" : "s"} under ${folder}.${inventoryMessage ? ` ${inventoryMessage}` : ""} Review before adding.`
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (isWslScanPreferenceApplyError(error)) {
        setAddProjectsVisible(true);
        setProjectDiscoveryError(message);
        setStatusText(`Folder search did not start because its WSL scope could not be applied safely: ${message}`);
        return;
      }
      setProjectDiscoveryError(message);
      setStatusText(`Search failed: ${message}`);
    } finally {
      setProjectDiscoveryLoading(false);
    }
  }, [autoRegisterStrongCandidates, deepScanIncludeAgents, deepScanIncludeLoose, markSessionInventoryNeedsRefresh, showDiscover, startWslGatedProjectDiscovery]);

  const startRootScan = useCallback(
    async (rootId: number) => {
      if (rootIsScanning(rootId)) {
        setStatusText("A scan is already running for this root.");
        return;
      }
      try {
        const jobId = await api.scanStart([rootId], performanceMode);
        const status = await api.scanStatus(jobId);
        setScanStatus(status);
        setStatusText(status.message);
        await loadProjectsLite();
      } catch (error) {
        setStatusText(`Scan failed to start: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    [loadProjectsLite, performanceMode, rootIsScanning, setScanStatus]
  );

  const [compactBusy, setCompactBusy] = useState(false);

  // Re-scan every enabled root in one job (scan_start with no ids → all roots), applying the
  // current scan rules so build/dependency folders indexed by an older version are dropped.
  const rescanAllRoots = useCallback(async () => {
    if (roots.some((root) => rootIsScanning(root.id))) {
      setStatusText("A scan is already running. Wait for it to finish before re-scanning all roots.");
      return;
    }
    try {
      setStatusText("Re-scanning all roots with the current rules…");
      const jobId = await api.scanStart(undefined, performanceMode);
      const status = await api.scanStatus(jobId);
      setScanStatus(status);
      setStatusText(status.message);
      await loadProjectsLite();
    } catch (error) {
      setStatusText(`Re-scan all failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [loadProjectsLite, performanceMode, rootIsScanning, roots, setScanStatus]);

  // Compact (VACUUM) the local database to return the space a re-scan freed back to disk.
  const compactDatabase = useCallback(async () => {
    if (roots.some((root) => rootIsScanning(root.id))) {
      setStatusText("Cancel the active scan before compacting the database.");
      return;
    }
    setCompactBusy(true);
    setStatusText("Compacting the local database… this can take a minute on a large inventory.");
    try {
      const report = await api.compactDatabase();
      const mb = (bytes: number) => `${Math.round(bytes / 1048576).toLocaleString()} MB`;
      setStatusText(
        report.freedBytes > 0
          ? `Database compacted: reclaimed ${mb(report.freedBytes)} (now ${mb(report.afterBytes)}).`
          : "Database compacted. It was already compact — nothing to reclaim."
      );
    } catch (error) {
      setStatusText(`Compact failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setCompactBusy(false);
    }
  }, [roots, rootIsScanning]);

  const continueSubtreeScan = useCallback(
    async (navId: number) => {
      try {
        const jobId = await api.scanResumeSubtree(navId, performanceMode);
        const status = await api.scanStatus(jobId);
        setScanStatus(status);
        setStatusText(status.message);
      } catch (error) {
        setStatusText(`Could not continue the scan: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    [performanceMode, setScanStatus]
  );

  // Investigate an arbitrary folder by path: index it as an ad-hoc (unregistered) root,
  // wait for the scan, then show the report and open the same Gate-3 review on it so it
  // can be backed up / moved / deleted like a project — without joining your projects.
  const runInvestigate = useCallback(async () => {
    let path: string | null;
    try {
      path = await api.pickFolder("Choose a folder to investigate");
    } catch (error) {
      setStatusText(`Could not open the folder picker: ${error instanceof Error ? error.message : String(error)}`);
      return;
    }
    if (!path) {
      setStatusText("Investigation cancelled.");
      return;
    }
    setAddProjectsVisible(false);
    setInvestigation(null);
    setInvestigationBusy(true);
    try {
      setStatusText(`Investigating ${path}…`);
      const handle = await api.investigateFolder(path, performanceMode);
      // Poll the existing scan job to completion.
      let state = "running";
      for (let attempt = 0; attempt < 1200 && (state === "running" || state === "cancelling"); attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        const status = await api.scanStatus(handle.jobId);
        setScanStatus(status);
        state = status.state;
      }
      const report = await api.investigationReport(handle.rootId);
      setInvestigation(report);
      setStatusText(
        report.isOrphan
          ? `${report.path}: orphan folder — no registered project owns it.`
          : `${report.path}: relates to ${report.owners.length} registered project${report.owners.length === 1 ? "" : "s"}.`
      );
      if (report.rootNodeId != null) {
        // Make the ad-hoc folder the active plan target so the Safe Manage review and optional
        // disk actions resolve a target (they fall
        // back to planTargetNode/selectedProjectId, which are otherwise unset for a folder that
        // is deliberately kept out of the projects list), then kick off the first preview.
        setSelectedProjectId(report.rootNodeId);
        setPlanTargetNode({ nodeId: report.rootNodeId, label: report.path, kind: "project" });
        showReview();
        void buildPreviewPlan(report.rootNodeId);
      }
    } catch (error) {
      setStatusText(`Investigation failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setInvestigationBusy(false);
    }
  }, [buildPreviewPlan, performanceMode, setScanStatus, showReview]);

  const discardCurrentInvestigation = useCallback(async () => {
    if (!investigation) return;
    if (!(await requestConfirm(
      "Discard this investigation? It removes Code Hangar's temporary index of this folder — your files on disk are not touched.",
      { confirmLabel: "Discard investigation", tone: "danger" }
    ))) {
      return;
    }
    try {
      await api.discardInvestigation(investigation.rootId);
      setInvestigation(null);
      // Reset the plan context the investigate flow established: it pointed selectedProjectId /
      // planTargetNode at the now-deleted ad-hoc node and may hold a verified-backup id from it,
      // so a later Safe Manage action could otherwise resolve a dangling target or reuse a stale
      // backup. Clearing it leaves the review pane in its harmless "choose a project" state.
      setSelectedProjectId(null);
      setPlanTargetNode(null);
      setOperationPlan(null);
      setRiskReport(null);
      setMutationBackupId(null);
      setStatusText("Investigation discarded.");
      await loadProjectsLite();
    } catch (error) {
      setStatusText(`Could not discard the investigation: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [investigation, loadProjectsLite, requestConfirm]);

  const toggleRoot = useCallback(
    async (root: ScanRoot) => {
      if (root.enabled && rootIsScanning(root.id)) {
        setStatusText("Cancel the active scan before disabling this root.");
        return;
      }
      const nextEnabled = !root.enabled;
      try {
        await api.rootsSetEnabled(root.id, nextEnabled);
      } catch (error) {
        setStatusText(scanRootToggleFailureMessage(root.path, nextEnabled, error));
        return;
      }

      const successMessage = scanRootToggleMessage(root.path, nextEnabled);
      try {
        await refreshSideData();
        setStatusText(successMessage);
      } catch (error) {
        setStatusText(`${successMessage} Could not refresh the sidebar: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    [refreshSideData, rootIsScanning]
  );

  const unregisterRoot = useCallback(
    async (rootId: number, options: { alreadyConfirmed?: boolean } = {}) => {
      if (rootIsScanning(rootId)) {
        setStatusText("Cancel the active scan before unregistering this root.");
        return;
      }
      const root = roots.find((candidate) => candidate.id === rootId) ?? null;
      if (!options.alreadyConfirmed) {
        const confirmed = await requestConfirm(
          unregisterRootConfirmationMessage(root?.path),
          { confirmLabel: "Unregister folder", tone: "danger" }
        );
        if (!confirmed) {
          setStatusText("Unregister cancelled. No local inventory changed.");
          return;
        }
        if (rootIsScanning(rootId)) {
          setStatusText("Cancel the active scan before unregistering this root.");
          return;
        }
      }
      const projectBeingRemoved = projects.find((project) => (
        project.scanRootId === rootId || (root && normalizeProjectRootPath(project.path) === normalizeProjectRootPath(root.path))
      )) ?? null;
      const wasSelected = Boolean(projectBeingRemoved && projectBeingRemoved.id === selectedProjectId);
      setStatusText(`Unregistering ${root?.path ?? "scan root"} from Code Hangar metadata.`);
      setRoots((current) => current.filter((candidate) => candidate.id !== rootId));
      if (projectBeingRemoved) {
        setProjects((current) => current.filter((project) => project.id !== projectBeingRemoved.id));
      }
      if (wasSelected) {
        if (planJobId) void api.operationPlanCancel(planJobId);
        selectionSeq.current += 1;
        beginProject(null);
        manualPreviewClearProjectRef.current = null;
        setFolderExplanation(null);
        setRelationships(null);
        setRelationshipsMembership(null);
        setRelationshipsLoading(false);
        setFileOrphanLoading(false);
        setPlanTargetNode(null);
        setOperationPlan(null);
        setRiskReport(null);
        setPlanJobId(null);
        setPlanJobStatus(null);
        setPlanLoading(false);
        setPreview(null);
        setTabs([]);
        setBackStack([]);
        setForwardStack([]);
        setSelectedProjectId(null);
        setPrimaryView("overview");
        setRightPaneView("dashboard");
      }
      await yieldToUi();
      try {
        await api.rootsUnregister(rootId);
        const loaded = await api.projectsList();
        setProjects(loaded);
        setProjectsFromCache(false);
        if (wasSelected) {
          const nextProjectId = visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
          beginProject(nextProjectId);
          setSelectedProjectId(nextProjectId);
        }
        await refreshSideData();
        setStatusText(`Unregistered ${root?.path ?? "scan root"} from Code Hangar. Files on disk were not touched.`);
      } catch (error) {
        setStatusText(`Unregister failed: ${error instanceof Error ? error.message : String(error)}`);
        const loaded = await api.projectsList();
        setProjects(loaded);
        setProjectsFromCache(false);
        if (wasSelected) {
          const originalProjectStillExists = projectBeingRemoved
            ? loaded.find((project) => project.id === projectBeingRemoved.id)?.id ?? null
            : null;
          const nextProjectId = originalProjectStillExists ?? visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
          beginProject(nextProjectId);
          setSelectedProjectId(nextProjectId);
        }
        await refreshSideData();
      }
    },
    [beginProject, planJobId, projects, refreshSideData, requestConfirm, rootIsScanning, roots, selectedProjectId, showDemoProjects]
  );

  const resetAllProjects = useCallback(async () => {
    setStatusText("Resetting Code Hangar's local index and reclaiming its disk space…");
    if (planJobId) void api.operationPlanCancel(planJobId);
    // Stand the watcher poll down for the duration so it doesn't contend with
    // the reset's bulk delete + VACUUM; we resume it once the index reloads.
    resettingRef.current = true;
    // Bump the selection sequence so any in-flight project/preview load is
    // ignored, then tear the workspace down to a clean Overview *before* the
    // data is removed. This guarantees nothing renders against a project,
    // file, tab or session that is about to stop existing.
    selectionSeq.current += 1;
    beginProject(null);
    previewCacheRef.current.clear();
    manualPreviewClearProjectRef.current = null;
    setSelectedProjectId(null);
    setPreview(null);
    setPreviewSession(null);
    setSessionPreview(null);
    setSessionPreviewError(null);
    setTabs([]);
    setBackStack([]);
    setForwardStack([]);
    setFolderExplanation(null);
    setRelationships(null);
    setProjectDiscoveryReport(null);
    setSessionInventory([]);
    setSessionInventoryState("fresh");
    setSessionInventoryRefreshing(false);
    setSessionInventoryError(null);
    // Drop the cached discovery so a wiped index can't rehydrate stale grouping
    // on the next launch (an empty snapshot clears the DPAPI-protected store).
    void api.cacheDiscoverySnapshot("");
    setPlanTargetNode(null);
    setOperationPlan(null);
    setRiskReport(null);
    setRoots([]);
    setProjects([]);
    setProjectsFromCache(false);
    try {
      // The backend schedules a full wipe of the database file, then we restart
      // so the wipe runs at startup before any connection opens — this actually
      // reclaims the disk (deleting the large encrypted index in place would
      // crawl, and Windows locks the file while it is open). Project files on
      // disk are never touched; the demo projects return fresh.
      const removed = await api.resetAllProjects();
      setStatusText(
        `Reset done: unregistered ${removed} project${removed === 1 ? "" : "s"} and every scan root. ` +
          "Restarting Code Hangar to reclaim the disk space…"
      );
      await api.restartApp();
    } catch (error) {
      setStatusText(`Reset failed: ${error instanceof Error ? error.message : String(error)}`);
      resettingRef.current = false;
      void loadProjectsLite();
    }
  }, [beginProject, loadProjectsLite, planJobId]);

  const cancelScan = useCallback(async (jobId: string) => {
    try {
      await api.scanCancel(jobId);
      const status = await api.scanStatus(jobId);
      setScanStatus(status);
      setStatusText(status.message);
      if (status.rootIds.length > 0) {
        const affectedRoots = new Set(status.rootIds);
        setProjects((current) => current.map((project) => (
          project.scanRootId != null && affectedRoots.has(project.scanRootId)
            ? { ...project, scanState: "outdated" }
            : project
        )));
        window.setTimeout(() => {
          void loadProjectsLite();
        }, 250);
      }
    } catch (error) {
      setStatusText(`Could not cancel the scan: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [loadProjectsLite, setScanStatus]);

  const removeOrphanProject = useCallback(
    async (project: ProjectSummary, options: { alreadyConfirmed?: boolean } = {}) => {
      if (!options.alreadyConfirmed) {
        const confirmed = await requestConfirm(
          unregisterProjectConfirmationMessage(project.name),
          { confirmLabel: "Remove from Code Hangar", tone: "danger" }
        );
        if (!confirmed) {
          setStatusText("Removal cancelled. No local inventory changed.");
          return;
        }
      }
      const wasSelected = project.id === selectedProjectId;
      setStatusText(`Removing ${project.name} from Code Hangar. Files on disk are not touched.`);
      setProjects((current) => current.filter((candidate) => candidate.id !== project.id));
      if (wasSelected) {
        if (planJobId) void api.operationPlanCancel(planJobId);
        selectionSeq.current += 1;
        beginProject(null);
        manualPreviewClearProjectRef.current = null;
        setFolderExplanation(null);
        setRelationships(null);
        setRelationshipsMembership(null);
        setRelationshipsLoading(false);
        setFileOrphanLoading(false);
        setPlanTargetNode(null);
        setOperationPlan(null);
        setRiskReport(null);
        setPlanJobId(null);
        setPlanJobStatus(null);
        setPlanLoading(false);
        setSelectedProjectId(null);
        setPreview(null);
        setTabs([]);
        setBackStack([]);
        setForwardStack([]);
        setPrimaryView("overview");
        setRightPaneView("dashboard");
      }
      await yieldToUi();
      try {
        await api.projectsUnregister(project.id);
        const loaded = await api.projectsList();
        setProjects(loaded);
        setProjectsFromCache(false);
        if (wasSelected) {
          const nextProjectId = visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
          beginProject(nextProjectId);
          setSelectedProjectId(nextProjectId);
        }
        await refreshSideData();
        setStatusText(`Removed ${project.name} from Code Hangar. Files on disk were not touched.`);
      } catch (error) {
        setStatusText(`Remove failed: ${error instanceof Error ? error.message : String(error)}`);
        const loaded = await api.projectsList();
        setProjects(loaded);
        setProjectsFromCache(false);
        if (wasSelected) {
          const originalProjectStillExists = loaded.find((candidate) => candidate.id === project.id)?.id ?? null;
          const nextProjectId = originalProjectStillExists ?? visibleProjects(loaded, showDemoProjects)[0]?.id ?? null;
          beginProject(nextProjectId);
          setSelectedProjectId(nextProjectId);
        }
        await refreshSideData();
      }
    },
    [beginProject, planJobId, refreshSideData, requestConfirm, selectedProjectId, showDemoProjects]
  );

  // De-register a project from the AI apps (Antigravity now): each app's registry entry
  // is backed up, then deleted, so the project stops appearing in that app. Callers pass
  // `armUndo`: when the project's folder is NOT also being deleted, we offer the one-click
  // status-bar Undo. When the folder IS being deleted, restoring just the app entry would
  // point the app at a now-missing folder, so we do NOT arm that misleading Undo — recovery
  // goes through Recover (the folder) instead. The registry backup file is still kept.
  const removeProjectFromApps = useCallback(
    async (project: ProjectSummary, armUndo = true) => {
      try {
        const removal = await api.removeProjectFromApps(project.id);
        if (!removal || removal.records.length === 0) {
          setStatusText(`${project.name} was not registered in any supported AI app (Antigravity, Cursor, ChatGPT, Claude, Hermes).`);
          setAppRemovalUndo(null);
          return;
        }
        const apps = [...new Set(removal.records.map((record) => record.app))].join(", ");
        // The durable Undo restores by id (same path as Recover) — it survives navigation,
        // rebuilds and restarts, unlike the old in-memory records round-trip.
        if (armUndo) {
          setStatusText(`Removed ${project.name} from ${apps}. Backed up — use Undo or Recovery & cleanup to restore.`);
          setAppRemovalUndo({ name: project.name, id: removal.id });
        } else {
          setStatusText(`Removed ${project.name} from ${apps}. A backup copy is kept; restore the project from Recovery & cleanup.`);
          setAppRemovalUndo(null);
        }
        const refreshed = await api.appRemovalsList();
        setAppRemovals(refreshed);
      } catch (error) {
        setStatusText(`Could not remove from AI apps: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    []
  );

  // Unified "Remove project": run the chosen removals. AI-app de-registration and the
  // Code Hangar unregister are instant + reversible and run here; deleting the folder
  // from disk routes into the proven Safe Manage backup→remove review (where the user
  // picks a safe backup location), since that step needs a location and the live node.
  const confirmRemoveProject = useCallback(
    async (opts: { fromApps: boolean; fromHangar: boolean; fromDisk: boolean }) => {
      const project = removeProjectTarget;
      if (!project) return;
      setRemoveProjectTarget(null);
      // A new remove operation supersedes any deferred unregister left by an abandoned one.
      pendingPostMoveUnregister.current = null;
      // Resolve the scan root the way the rest of the file does — by id, then by normalized
      // path (a registered/scanned root often yields scanRootId === null, matchable only by
      // path). Without the fallback, root-backed projects get the wrong unregister path.
      const root =
        roots.find((candidate) => candidate.id === project.scanRootId) ??
        roots.find((candidate) => normalizeProjectRootPath(candidate.path) === normalizeProjectRootPath(project.path));
      if (opts.fromApps) {
        // Only arm the one-click Undo when the folder is NOT also being deleted; otherwise
        // recovery is through Recover (restoring just the app entry would dangle).
        await removeProjectFromApps(project, !opts.fromDisk);
      }
      if (opts.fromDisk) {
        // The disk flow needs the project node, so do NOT unregister first — defer it to
        // run after the move completes if the user also asked to forget from Code Hangar.
        pendingPostMoveUnregister.current = opts.fromHangar
          ? { rootId: root?.id ?? null, projectId: project.id }
          : null;
        selectProject(project.id);
        showReview();
        setStatusText(
          `Safe Manage: back up and remove ${project.name}'s folder. Your files are backed up to a location you choose before anything leaves the disk${opts.fromHangar ? "; it is then forgotten from Code Hangar" : ""}.`
        );
        return;
      }
      if (opts.fromHangar) {
        if (root) await unregisterRoot(root.id, { alreadyConfirmed: true });
        else await removeOrphanProject(project, { alreadyConfirmed: true });
      }
    },
    [removeProjectTarget, roots, removeProjectFromApps, removeOrphanProject, unregisterRoot, selectProject, showReview]
  );

  const undoAppRemoval = useCallback(async () => {
    if (!appRemovalUndo) return;
    const { name, id } = appRemovalUndo;
    try {
      await api.appRemovalRestore(id);
      setStatusText(`Restored ${name} to its AI apps. Reopen the app to see it.`);
      setAppRemovalUndo(null);
      const refreshed = await api.appRemovalsList();
      setAppRemovals(refreshed);
    } catch (error) {
      setStatusText(`Could not undo: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [appRemovalUndo]);

  const showProjectMenu = useCallback(
    (project: ProjectSummary, event: MouseEvent<HTMLElement>) => {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const root = project.scanRootId != null
        ? roots.find((candidate) => candidate.id === project.scanRootId)
        : roots.find((candidate) => candidate.path === project.path);
      const isScanning = root ? rootIsScanning(root.id) : false;
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: project.name,
        items: [
          {
            id: "open",
            label: "Open project",
            section: "Open and organize",
            help: `Open ${project.name}. If it is already active, Code Hangar returns to its Context without reloading.`,
            icon: <FolderOpen size={15} />,
            onSelect: () => selectProject(project.id)
          },
          ...(!isDemoProject(project) ? [{
            id: "show-in-explorer",
            label: "Show in File Explorer",
            help: `Open ${project.name}'s registered local folder in File Explorer.`,
            icon: <FolderOpen size={15} />,
            onSelect: () => void revealProjectWithSystem(project.id)
          }] : []),
          {
            id: "pin",
            label: project.pinned ? "Unpin" : "Pin",
            help: project.pinned ? `Remove ${project.name} from Pinned. No project files are changed.` : `Keep ${project.name} in Pinned for quick access. No project files are changed.`,
            icon: project.pinned ? <PinOff size={15} /> : <Pin size={15} />,
            onSelect: () => updateProjectPin(project)
          },
          ...(isDemoProject(project)
            ? [{
                id: "hide-demos",
                label: "Hide demo projects",
                section: "Display",
                help: "Hide built-in demo projects from the sidebar. This does not unregister or delete anything.",
                icon: <Eye size={15} />,
                onSelect: () => setShowDemoProjects(false)
              }]
            : root
              ? [
                {
                  id: "rescan",
                  label: "Re-scan",
                  section: "Local inventory",
                  help: `Refresh Code Hangar metadata for ${project.name}. The scan reads local files but does not modify them.`,
                  icon: <RefreshCcw size={15} />,
                  disabled: !root || isScanning,
                  onSelect: () => root ? void startRootScan(root.id) : undefined
                },
                {
                  id: "toggle-root",
                  label: root?.enabled ? "Disable" : "Enable",
                  help: root?.enabled ? `Disable future scans for ${project.name}. Existing inventory remains local.` : `Enable future scans for ${project.name}.`,
                  icon: <Database size={15} />,
                  disabled: !root || (root.enabled && isScanning),
                  onSelect: () => root ? void toggleRoot(root) : undefined
                },
                {
                  id: "unregister",
                  label: "Unregister from Code Hangar…",
                  section: "Removal",
                  help: `Open a confirmation to remove ${project.name} from Code Hangar's local inventory. Real files on disk are not deleted.`,
                  icon: <X size={15} />,
                  disabled: !root || isScanning,
                  danger: true,
                  onSelect: () => root ? void unregisterRoot(root.id) : undefined
                }
                ]
              : [
                  {
                    id: "remove-orphan",
                    label: "Remove from Code Hangar",
                    section: "Removal",
                    help: `Open a confirmation to remove ${project.name} from Code Hangar. This project has no scan root left, so only local Code Hangar metadata is cleared; your files on disk are never touched.`,
                    icon: <X size={15} />,
                    danger: true,
                    onSelect: () => void removeOrphanProject(project)
                  }
                ]),
          ...(mutationAvailable
            ? [{
                id: "remove-project",
                label: "More removal options…",
                section: "Removal",
                help: `Choose whether to remove ${project.name} from AI apps, Code Hangar metadata or, only through Safe Manage, from disk.`,
                icon: <Plug size={15} />,
                danger: true,
                onSelect: () => setRemoveProjectTarget(project)
              }]
            : []),
          {
            id: "copy-path",
            label: "Copy path",
            section: "Copy",
            help: `Copy the full local path for ${project.name} to the clipboard.`,
            icon: <Copy size={15} />,
            onSelect: () => void copyPath(projectRootPath(project))
          }
        ]
      });
    },
    [copyPath, mutationAvailable, projectRootPath, revealProjectWithSystem, rootIsScanning, roots, removeOrphanProject, selectProject, startRootScan, toggleRoot, unregisterRoot, updateProjectPin]
  );

  // Stable identities for the sidebar project rows. selectProject/showProjectMenu
  // depend on navigation/route state (via pushWorkspaceRoute → currentWorkspaceRoute)
  // and so change on most App renders, which would defeat ProjectRow's memo. These
  // thin wrappers always call the latest handler through a ref, so the row's
  // onSelect/onContextMenu props stay referentially stable and the project list
  // skips re-rendering when unrelated App state changes.
  const selectProjectRef = useRef(selectProject);
  selectProjectRef.current = selectProject;
  const showProjectMenuRef = useRef(showProjectMenu);
  showProjectMenuRef.current = showProjectMenu;
  const rowSelectProject = useCallback((projectId: number) => selectProjectRef.current(projectId), []);
  const rowShowProjectMenu = useCallback(
    (project: ProjectSummary, event: MouseEvent<HTMLElement>) => showProjectMenuRef.current(project, event),
    []
  );
  // Same stable-identity treatment for the memoized sidebar session groups: their
  // openSession/onOpenProject props otherwise change on every navigation (route
  // state), re-rendering all groups whenever an unrelated view changes.
  const openSessionRef = useRef(openSession);
  openSessionRef.current = openSession;
  const rowOpenSession = useCallback((session: SessionDiscoveryCandidate) => openSessionRef.current(session), []);

  const showSessionMenu = useCallback((session: SessionDiscoveryCandidate, event: MouseEvent<HTMLElement>) => {
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    const linkedProjectId = session.linkedRegisteredProjectIds[0] ?? null;
    const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
    setContextMenu({
      x: anchor.x,
      y: anchor.y,
      label: session.displayName,
      items: [
        {
          id: "open-session",
          label: "Open conversation",
          section: "Open and inspect",
          help: "Open this local conversation in Code Hangar with its progressive reader.",
          icon: <MessageSquare size={15} />,
          onSelect: () => rowOpenSession(session)
        },
        ...(linkedProjectId ? [{
          id: "open-session-project",
          label: "Open linked project",
          help: "Open the registered project Code Hangar linked to this conversation.",
          icon: <FolderOpen size={15} />,
          onSelect: () => selectProject(linkedProjectId)
        }] : []),
        {
          id: "show-session-in-explorer",
          label: "Show record in File Explorer",
          help: "Open the known local conversation store and select this record. This does not open or parse the transcript.",
          icon: <FolderOpen size={15} />,
          onSelect: () => void revealSessionWithSystem(session.path)
        },
        {
          id: "copy-session-path",
          label: "Copy path",
          section: "Copy",
          help: "Copy the full local path of this conversation record.",
          icon: <Copy size={15} />,
          onSelect: () => void copyPath(session.path)
        }
      ]
    });
  }, [copyPath, revealSessionWithSystem, rowOpenSession, selectProject]);

  const showTabMenu = useCallback(
    (tab: OpenTab, event: MouseEvent<HTMLElement>) => {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const tabIndex = tabs.findIndex((candidate) => candidate.nodeId === tab.nodeId);
      const hasTabsLeft = tabIndex > 0;
      const hasTabsRight = tabIndex >= 0 && tabIndex < tabs.length - 1;
      const hasOtherProjectTabs = tabs.some((candidate) => candidate.projectId !== tab.projectId);
      const hasOtherTabs = tabs.length > 1;
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: tab.label,
        items: [
          {
            id: "show-tab-in-explorer",
            label: "Show in File Explorer",
            section: "File",
            help: `Open the containing folder and select ${tab.label}.`,
            icon: <FolderOpen size={15} />,
            onSelect: () => void revealNodeWithSystem(tab.nodeId)
          },
          {
            id: "copy-tab-path",
            label: "Copy path",
            help: `Copy the full local path for ${tab.label}.`,
            icon: <Copy size={15} />,
            onSelect: () => void copyNodePath(tab.nodeId, tab.path)
          },
          { id: "close", label: "Close tab", section: "Tabs", icon: <X size={15} />, onSelect: () => void closeTab(tab.nodeId) },
          {
            id: "close-left",
            label: "Close tabs to the left",
            icon: <ArrowLeft size={15} />,
            disabled: !hasTabsLeft,
            onSelect: () => void closeTabsToSide(tab.nodeId, "left")
          },
          {
            id: "close-right",
            label: "Close tabs to the right",
            icon: <ArrowRight size={15} />,
            disabled: !hasTabsRight,
            onSelect: () => void closeTabsToSide(tab.nodeId, "right")
          },
          {
            id: "close-other-projects",
            label: "Close tabs from other projects",
            icon: <FolderOpen size={15} />,
            disabled: !hasOtherProjectTabs,
            onSelect: () => void closeTabsOutsideProject(tab.projectId, tab.nodeId)
          },
          { id: "close-others", label: "Close all other tabs", icon: <PanelLeft size={15} />, disabled: !hasOtherTabs, onSelect: () => void closeOtherTabs(tab.nodeId) },
          { id: "close-all", label: "Close all tabs", icon: <X size={15} />, onSelect: closeAllTabs }
        ]
      });
    },
    [closeAllTabs, closeOtherTabs, closeTab, closeTabsOutsideProject, closeTabsToSide, copyNodePath, revealNodeWithSystem, tabs]
  );

  const showFileMenu = useCallback(
    (
      item: { nodeId: number; projectId?: number | null; path: string; label: string; itemKind?: string },
      event: MouseEvent<HTMLElement>
    ) => {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const isPinned = item.projectId != null && pinnedItems.some((pinned) => (
        pinned.nodeId === item.nodeId
        && pinned.projectId === item.projectId
        && pinned.itemKind === "file"
      ));
      const capabilities = fileContextCapabilities(item.path, item.itemKind);
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: item.label,
        items: [
          {
            id: "open",
            label: "View in Code Hangar",
            section: "Open and inspect",
            help: `View ${item.label} in Code Hangar without opening another app.`,
            icon: capabilities.isDirectory ? <FolderOpen size={15} /> : <FileText size={15} />,
            onSelect: () => void openNodeInTree(item.nodeId, { projectId: item.projectId })
          },
          {
            id: "show-in-explorer",
            label: "Show in File Explorer",
            help: item.itemKind === "directory"
              ? `Open ${item.label} in File Explorer.`
              : `Open the containing folder and select ${item.label}.`,
            icon: <FolderOpen size={15} />,
            onSelect: () => void revealNodeWithSystem(item.nodeId)
          },
          ...(capabilities.canOpenWithDefaultApp ? [{
            id: "open-system",
            label: "Open with default app",
            help: `Open ${item.label} with its Windows default application.`,
            icon: <FileText size={15} />,
            onSelect: () => void openNodeWithSystem(item.nodeId)
          }] : []),
          ...(capabilities.canViewSource ? [{
            id: "open-source",
            label: "View source",
            help: `View the local source text for ${item.label} in Code Hangar.`,
            icon: <TerminalSquare size={15} />,
            onSelect: () => void openNodeInTree(item.nodeId, { mode: "source", projectId: item.projectId })
          }] : []),
          ...(!shellViewer && item.projectId != null && !capabilities.isDirectory && !capabilities.isLink ? [{
            id: "pin",
            label: isPinned ? "Unpin" : "Pin",
            section: "More tools",
            help: isPinned ? `Remove ${item.label} from Pinned.` : `Keep ${item.label} in Pinned for quick access.`,
            icon: isPinned ? <PinOff size={15} /> : <Pin size={15} />,
            onSelect: () => updateFilePin(item.nodeId, item.projectId as number, item.label, isPinned)
          }] : []),
          ...(!shellViewer ? [{
            id: "review-impact",
            label: "Safe Manage",
            section: "More tools",
            help: `Review references and reversible actions for ${item.label}.`,
            icon: <ListChecks size={15} />,
            onSelect: () => {
              if (item.projectId) selectProject(item.projectId);
              setPlanTargetNode({ nodeId: item.nodeId, label: item.label, kind: item.itemKind ?? "file" });
              showReview();
              void buildPreviewPlan(item.nodeId);
            }
          }] : []),
          {
            id: "copy-path",
            label: "Copy path",
            section: "Copy",
            help: `Copy the full local path for ${item.label} to the clipboard.`,
            icon: <Copy size={15} />,
            onSelect: () => void copyNodePath(item.nodeId, item.path)
          }
        ]
      });
    },
    [buildPreviewPlan, copyNodePath, openNodeInTree, openNodeWithSystem, pinnedItems, revealNodeWithSystem, selectProject, shellViewer, showReview, updateFilePin]
  );

  const showForgottenProjectMenu = useCallback(
    (candidate: LostProjectCandidates["candidates"][number], event: MouseEvent<HTMLElement>) => {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: candidate.displayName,
        items: [
          {
            id: "open-project",
            label: "Open project",
            section: "Open and inspect",
            icon: <FolderOpen size={15} />,
            onSelect: () => selectProject(candidate.projectId)
          },
          {
            id: "show-in-explorer",
            label: "Show in File Explorer",
            icon: <FolderOpen size={15} />,
            onSelect: () => candidate.nodeId != null
              ? void revealNodeWithSystem(candidate.nodeId)
              : void revealProjectWithSystem(candidate.projectId)
          },
          {
            id: "review-impact",
            label: "Safe Manage",
            section: "More tools",
            icon: <ListChecks size={15} />,
            onSelect: () => {
              selectProject(candidate.projectId);
              if (candidate.candidateKind === "folder" && candidate.nodeId != null) {
                setPlanTargetNode({ nodeId: candidate.nodeId, label: candidate.displayName, kind: "directory" });
                showReview();
                void buildPreviewPlan(candidate.nodeId);
              } else {
                setPlanTargetNode(null);
                showReview();
                void buildPreviewPlan(candidate.projectId);
              }
            }
          },
          {
            id: "copy-path",
            label: "Copy path",
            section: "Copy",
            icon: <Copy size={15} />,
            onSelect: () => void copyPath(candidate.path)
          }
        ]
      });
    },
    [buildPreviewPlan, copyPath, revealNodeWithSystem, revealProjectWithSystem, selectProject, showReview]
  );

  const showTreeMenu = useCallback(
    (item: NavItem, event: MouseEvent<HTMLElement>) => {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const path = item.displayPath || item.path;
      const isPinned = Boolean(item.nodeId && pinnedItems.some((pinned) => (
        pinned.nodeId === item.nodeId
        && pinned.projectId === item.projectId
        && pinned.itemKind === "file"
      )));
      const capabilities = fileContextCapabilities(path, item.itemKind);
      const folderExpanded = capabilities.isDirectory && expandedTree.has(item.id);
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: item.displayName,
        items: [
          ...(capabilities.isDirectory
            ? [
                {
                  id: "inspect-folder-details",
                  label: "Inspect folder details",
                  section: "Open and inspect",
                  icon: <Eye size={15} />,
                  help: `Show what ${item.displayName} appears to contain and why it may matter. This is read-only.`,
                  onSelect: () => explainFolder(item)
                },
                {
                  id: "toggle-folder",
                  label: folderExpanded ? "Collapse folder" : "Expand folder",
                  icon: folderExpanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />,
                  help: `${folderExpanded ? "Collapse" : "Expand"} ${item.displayName} in the file tree.`,
                  onSelect: () => {
                    toggleExpandedTree(item.id);
                    if (!folderExpanded && !treePages[String(item.id)]) void loadTreeChildren(item.id);
                  }
                }
              ]
            : [{
                id: "view",
                label: "View in Code Hangar",
                section: "Open and inspect",
                icon: <Eye size={15} />,
                disabled: !item.nodeId,
                help: `View ${item.displayName} inside Code Hangar's preview pane.`,
                onSelect: () => item.nodeId
                  ? void openNodeInTree(item.nodeId, { projectId: item.projectId })
                  : undefined
              }]),
          {
            id: "show-in-explorer",
            label: "Show in File Explorer",
            icon: <FolderOpen size={15} />,
            disabled: !item.nodeId,
            help: item.itemKind === "directory"
              ? `Open folder ${item.displayName} in File Explorer.`
              : `Open the containing folder and select ${item.displayName}.`,
            onSelect: () => void revealNodeWithSystem(item.nodeId)
          },
          ...(capabilities.canOpenWithDefaultApp ? [{
            id: "open-system",
            label: "Open with default app",
            icon: <FileText size={15} />,
            disabled: !item.nodeId,
            help: `Open file ${item.displayName} with the Windows default app.`,
            onSelect: () => void openNodeWithSystem(item.nodeId)
          }] : []),
          ...(capabilities.canViewSource ? [{
            id: "open-source",
            label: "View source",
            help: `View the local source text for ${item.displayName} in Code Hangar.`,
            icon: <TerminalSquare size={15} />,
            disabled: !item.nodeId,
            onSelect: () => item.nodeId
              ? void openNodeInTree(item.nodeId, { mode: "source", projectId: item.projectId })
              : undefined
          }] : []),
          ...(!shellViewer && !capabilities.isDirectory && !capabilities.isLink ? [{
            id: "pin",
            label: isPinned ? "Unpin" : "Pin",
            section: "More tools",
            help: isPinned ? `Remove ${item.displayName} from Pinned.` : `Keep ${item.displayName} in Pinned for quick access.`,
            icon: isPinned ? <PinOff size={15} /> : <Pin size={15} />,
            disabled: !item.nodeId,
            onSelect: () => item.nodeId
              ? updateFilePin(item.nodeId, item.projectId, item.displayName, isPinned)
              : undefined
          }] : []),
          ...(!shellViewer ? [{
            id: "review-impact",
            label: "Safe Manage",
            section: "More tools",
            help: `Review references and reversible actions for ${item.displayName}.`,
            icon: <ListChecks size={15} />,
            disabled: !item.nodeId,
            onSelect: () => {
              if (!item.nodeId) return;
              setPlanTargetNode({ nodeId: item.nodeId, label: item.displayName, kind: item.itemKind });
              showReview();
              void buildPreviewPlan(item.nodeId);
            }
          }] : []),
          {
            id: "copy-path",
            label: "Copy path",
            section: "Copy",
            icon: <Copy size={15} />,
            help: `Copy the full absolute path for ${item.displayName}, suitable for File Explorer.`,
            onSelect: () => void copyNodePath(item.nodeId, path)
          }
        ]
      });
    },
    [buildPreviewPlan, copyNodePath, expandedTree, explainFolder, loadTreeChildren, openNodeInTree, openNodeWithSystem, pinnedItems, revealNodeWithSystem, shellViewer, showReview, toggleExpandedTree, treePages, updateFilePin]
  );

  useEffect(() => {
    if (!runningJobKey) return;
    const runningJobIds = runningJobKey.split("|");
    let cancelled = false;
    let timer: number | null = null;
    const schedule = (delay: number) => {
      if (!cancelled) timer = window.setTimeout(() => void poll(), delay);
    };
    const poll = async () => {
      try {
        const statuses = await Promise.all(runningJobIds.map((jobId) => api.scanStatus(jobId)));
        if (cancelled) return;
        let finished = false;
        for (const status of statuses) {
          const previousAnnouncement = scanAnnouncementSnapshotsRef.current[status.jobId];
          const announcementKind = scanStatusAnnouncementKind(previousAnnouncement, status);
          scanAnnouncementSnapshotsRef.current = mergeScanStatusSnapshot(
            scanAnnouncementSnapshotsRef.current,
            status
          );
          setScanStatus(status);
          const progress = scanProgressParts(status);
          if (announcementKind) {
            const message = status.error && announcementKind === "error" && status.error !== status.message
              ? `${status.message.replace(/[.:]\s*$/, "")}: ${status.error}`
              : status.message;
            setStatusText(
              announcementKind === "terminal"
                ? `${message.replace(/[.:]\s*$/, "")}: ${progress.countText}`
                : message
            );
          }
          if (!["running", "cancelling"].includes(status.state)) {
            finished = true;
          }
          if (status.state === "completed"
            && shouldCelebrateStandaloneScan(status.jobId, deepScanProgress?.scanJobId)
            && !celebratedJobsRef.current.has(status.jobId)) {
            celebratedJobsRef.current.add(status.jobId);
            setScanCelebration({
              files: status.scannedFiles,
              durationMs: Math.max(0, status.updatedAtMs - status.startedAtMs),
              nonce: Date.now()
            });
          }
        }
        if (finished) {
          await refreshAfterScanFinish();
          // Re-poll the watcher immediately so a just-scanned project drops its
          // "changed/needs scan" badge now instead of waiting for the next watcher interval.
          void refreshWatcherStatus();
          return;
        }
        schedule(document.hidden ? 2_000 : 500);
      } catch (error) {
        if (cancelled) return;
        setStatusText(`Scan progress refresh failed: ${error instanceof Error ? error.message : String(error)}`);
        schedule(document.hidden ? 4_000 : 1_500);
      }
    };
    schedule(0);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [deepScanProgress?.scanJobId, refreshAfterScanFinish, refreshWatcherStatus, runningJobKey, setScanStatus]);

  useEffect(() => {
    if (!scanCelebration) return;
    const timer = window.setTimeout(
      () => setScanCelebration(null),
      appearance.reduceMotion ? 1400 : 2200
    );
    return () => window.clearTimeout(timer);
  }, [appearance.reduceMotion, scanCelebration]);

  const RightWorkspace = isProjectLayout ? InspectorPane : ToolWorkspace;
  const rightWorkspaceScrollKey = isProjectLayout
    ? undefined
    : primaryView === "discover"
      ? `discover:${discoverView}`
      : primaryView === "settings"
        ? `settings:${settingsView}`
        : primaryView;

  return (
    <div
      className="app-shell"
      data-theme={themeMode}
      data-font-size={appearance.fontSize}
      data-density={appearance.density}
      data-contrast={appearance.contrast}
      data-reduce-motion={appearance.reduceMotion ? "true" : "false"}
      onMouseOver={updateHoverHelp}
      onFocus={updateHoverHelp}
      onClick={refreshHoverHelpAfterAction}
      onKeyDown={() => setHoverHelp(null)}
      onMouseLeave={() => setHoverHelp(null)}
    >
      {scanCelebration ? (
        <div
          className="scan-celebration"
          role="status"
          key={scanCelebration.nonce}
          data-help="A scan just finished. This summary clears on its own."
        >
          <span className="scan-celebration-spark"><CheckCircle2 size={16} /></span>
          <span>
            Scan complete · mapped <strong><CountUp value={scanCelebration.files} reduceMotion={appearance.reduceMotion} /></strong> items
            <span className="scan-celebration-sub"> in {formatScanDuration(scanCelebration.durationMs)}</span>
          </span>
        </div>
      ) : null}
      <header className="topbar">
        <div className={`brand ${leftPaneCollapsedForLayout || sidebarScrolled ? "brand-nav-hint" : ""}`}>
          <NavigationFlyout
            primaryView={primaryView}
            disabled={shellViewer !== null}
            onOverview={showOverview}
            onDiscover={() => showDiscover(discoverView)}
            onSafeManage={showSafeManage}
            onRecovery={showRecovery}
            onSettings={() => showSettings(settingsView)}
          />
          <div className="brand-text">
            <strong>Code Hangar</strong>
            <span>Understand local projects safely</span>
          </div>
          <small
            className={`brand-edition brand-edition--${connectorFrontendBuild ? "connector" : "local"}`}
            aria-label={`Code Hangar edition: ${frontendEditionLabel}`}
          >
            {frontendEditionLabel}
          </small>
          {showTopbarNav ? (
            <nav className="topbar-nav" aria-label="Quick navigation">
              <PrimaryNavButtons
                primaryView={primaryView}
                iconsOnly
                disabled={shellViewer !== null}
                onOverview={showOverview}
                onDiscover={() => showDiscover(discoverView)}
                onSafeManage={showSafeManage}
                onRecovery={showRecovery}
                onSettings={() => showSettings(settingsView)}
              />
            </nav>
          ) : null}
        </div>
        <div className="topbar-actions">
          <button className="icon-button" type="button" onClick={goBack} disabled={shellViewer !== null || (viewBackStack.length === 0 && backStack.length === 0)} aria-label="Back" data-help={shellViewer ? "Exit Viewer before navigating workspace history." : "Go back to the previous screen. If no screen history exists, go back to the previous opened file."}>
            <ArrowLeft size={17} />
          </button>
          <button className="icon-button" type="button" onClick={goForward} disabled={shellViewer !== null || (viewForwardStack.length === 0 && forwardStack.length === 0)} aria-label="Forward" data-help={shellViewer ? "Exit Viewer before navigating workspace history." : "Go forward to the next screen. If no screen history exists, go forward to the next file."}>
            <ArrowRight size={17} />
          </button>
          <button ref={quickOpenButtonRef} className="toolbar-button" type="button" data-tour="tour-quick-open" onClick={(event) => openQuickOpen(event.currentTarget)} aria-label="Quick Open" data-help="Open Quick Open to jump to projects or indexed files by name and path.">
            <Search size={16} />
            <span className="tb-label">Quick Open</span>
            <kbd>Ctrl+P</kbd>
          </button>
          <button ref={commandButtonRef} className="toolbar-button" type="button" disabled={shellViewer !== null} onClick={(event) => openCommandPalette(event.currentTarget)} aria-label="Commands" data-help={shellViewer ? "Exit Viewer to use catalog-wide commands." : "Show available commands and keyboard shortcuts."}>
            <Command size={16} />
            <span className="tb-label">Commands</span>
            <kbd>Ctrl+K</kbd>
          </button>
          {primaryRunningScan ? <ResourceMeter /> : null}
          <button
            className="toolbar-button mode-toggle"
            type="button"
            aria-label={advancedMode ? "Switch to Simple mode" : "Switch to Advanced mode"}
            aria-pressed={advancedMode}
            onClick={() => {
              setAdvancedMode((current) => {
                const next = !current;
                setStatusText(next ? "Advanced mode enabled. Technical details and heavy filters are visible." : "Simple mode enabled. The main workflow stays visible and technical detail is tucked away.");
                setHoverHelp(next ? "Advanced mode shows technical details, raw activity records, deeper filters and detailed impact breakdowns." : "Simple mode keeps the main workflow visible and hides technical detail until you need it.");
                return next;
              });
            }}
            data-help={advancedMode ? "Switch to Simple mode. Main workflows stay visible while technical sections are hidden." : "Switch to Advanced mode to show technical details, deeper filters and raw activity records."}
          >
            <SlidersHorizontal size={15} aria-hidden="true" />
            <span className="tb-label">{advancedMode ? "Advanced" : "Simple"}</span>
            <span className={`tb-switch ${advancedMode ? "on" : ""}`} aria-hidden="true" />
          </button>
          <button
            className="toolbar-button theme-toggle"
            type="button"
            aria-label={themeMode === "oled" ? "Switch to Light mode" : "Switch to OLED dark mode"}
            aria-pressed={themeMode === "oled"}
            onClick={() => {
              setThemeMode((current) => {
                const next = current === "oled" ? "light" : "oled";
                setStatusText(next === "oled" ? "OLED dark mode enabled: large surfaces use true black." : "Light mode enabled.");
                setHoverHelp(next === "oled" ? "OLED dark mode is on. Main surfaces use true black to keep OLED pixels off where possible." : "Light mode is on. Click to return to OLED dark mode.");
                return next;
              });
            }}
            data-help={themeMode === "oled" ? "Dark mode is on (true-black OLED surfaces). Click to switch to light mode." : "Light mode is on. Click to switch to OLED dark mode."}
          >
            {themeMode === "oled" ? <Moon size={15} aria-hidden="true" /> : <Sun size={15} aria-hidden="true" />}
            <span className="tb-label">{themeMode === "oled" ? "Dark" : "Light"}</span>
            <span className={`tb-switch ${themeMode === "oled" ? "on" : ""}`} aria-hidden="true" />
          </button>
          <button className="toolbar-button primary" type="button" disabled={shellViewer !== null} onClick={() => setAddProjectsVisible(true)} aria-label="Add Projects" data-help={shellViewer ? "Exit Viewer before adding or discovering projects." : "Add projects manually or run passive discovery under a selected folder or drive."}>
            <FolderOpen size={16} />
            <span className="tb-label">Add Projects</span>
          </button>
        </div>
      </header>

      {startupProgress.active && !shellOpenHasPriorityRef.current ? (
        <div className="startup-progress-card" role="status" aria-live="polite">
          <div className="startup-progress-copy">
            <strong>{startupProgress.label}</strong>
            <span>{startupProgress.detail}</span>
          </div>
          <div className="startup-progress-track" aria-hidden="true">
            <span style={{ width: `${startupProgress.progress}%` }} />
          </div>
        </div>
      ) : null}

      {recoveryFrozen && recoveryState?.pending ? (
        <div className="recovery-banner" role="status" data-help="Recovery is frozen for this session. Read-only navigation is allowed, but future disk actions should stay blocked until recovery is resolved.">
          <AlertTriangle size={16} />
          <strong>Recovery frozen</strong>
          <span>{recoveryState.operations.length} interrupted operation{recoveryState.operations.length === 1 ? "" : "s"} remain in the journal.</span>
          <button type="button" className="secondary-button compact" data-help="Review the interrupted-operation journal and choose whether to roll it back safely or leave it frozen. Code Hangar never treats rollback as a resumed operation." onClick={() => setRecoveryFrozen(false)}>
            Review recovery
          </button>
        </div>
      ) : null}

      {!tourActive && !shellViewer && safeManageFirstRunOpen ? (
        <Suspense fallback={null}>
          <SafeManageFirstRunPrompt
            open
            onAnalyzeNow={() => void analyzeSafeManageFirstRun()}
            onLater={() => void postponeSafeManageFirstRun()}
            onSuppress={() => void suppressSafeManageFirstRun()}
          />
        </Suspense>
      ) : null}

      <WorkspaceGrid mode={isProjectLayout ? "project" : "tool"} style={workspaceStyle} leftCollapsed={leftPaneCollapsedForLayout} rightCollapsed={rightPaneCollapsedForLayout} className={reviewFocusedLayout ? "review-focused" : ""}>
        <Sidebar collapsed={leftPaneCollapsedForLayout} onScrolledChange={setSidebarScrolled}>
          <button
            className="pane-collapse-button left"
            type="button"
            aria-label={leftPaneCollapsedForLayout ? "Expand projects sidebar" : "Collapse projects sidebar"}
            data-help={leftPaneCollapsedForLayout ? "Expand the projects sidebar." : "Collapse the projects sidebar to give more room to the project workspace."}
            onClick={() => {
              if (leftPaneCollapsedForLayout) {
                setProjectSidebarFocus(false);
                setPaneCollapsed((current) => ({ ...current, left: false }));
                return;
              }
              setPaneCollapsed((current) => ({ ...current, left: true }));
            }}
          >
            {leftPaneCollapsedForLayout ? <ChevronRight size={16} /> : <ChevronLeft size={16} />}
          </button>
          {!leftPaneCollapsedForLayout ? (
          <>
          <nav className="primary-nav" aria-label="Main navigation">
            <PrimaryNavButtons
              primaryView={primaryView}
              disabled={shellViewer !== null}
              onOverview={showOverview}
              onDiscover={() => showDiscover(discoverView)}
              onSafeManage={showSafeManage}
              onRecovery={showRecovery}
              onSettings={() => showSettings(settingsView)}
            />
          </nav>

          <section className="pane-section" data-tour="tour-projects">
            <button
              className="section-title section-toggle"
              type="button"
              aria-expanded={!sidebarCollapsed.projects}
              data-help="Show or hide registered local projects."
              onClick={() => setSidebarCollapsed((current) => ({ ...current, projects: !current.projects }))}
            >
              {sidebarCollapsed.projects ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
              <PanelLeft size={15} />
              <span>Projects</span>
              <small>{projectListCountLabel}</small>
            </button>
            {!sidebarCollapsed.projects ? (
              <div className="project-list">
                {selectedProject && !shellViewer ? (
                  <button
                    type="button"
                    className={`project-safe-manage ${primaryView === "review" ? "active" : ""}`}
                    onClick={showReview}
                    data-help={`Open the project-specific OperationPlan and Risk Report review for ${selectedProject.name}. Portfolio analysis and recommendations live in the Safe Manage main menu.`}
                  >
                    <ListChecks size={15} />
                    <span>Review selected project</span>
                  </button>
                ) : null}
                {displayedProjects.length > 1 ? (
                  <div className="project-list-toolbar" role="group" aria-label="Find and filter projects">
                    <div className="project-search" data-help="Filter only this project catalog by name or local metadata such as path, AI app, alias, status, pinned, archived or protected. This does not search inside files; use Document Search for file contents.">
                      <Search size={14} aria-hidden="true" />
                      <input
                        ref={projectSearchInputRef}
                        className="project-search-input"
                        type="search"
                        value={projectQuery}
                        onChange={(event) => setProjectQuery(event.target.value)}
                        onKeyDown={handleProjectSearchKeyDown}
                        placeholder="Project name or keyword…"
                        aria-label="Filter projects by name or metadata keyword"
                        aria-describedby="project-search-scope"
                      />
                      {projectQuery ? (
                        <button
                          className="project-search-clear"
                          type="button"
                          onClick={() => setProjectQuery("")}
                          aria-label="Clear project search"
                          data-help="Clear the project search text."
                        >
                          <X size={13} />
                        </button>
                      ) : null}
                    </div>
                    <small id="project-search-scope" className="project-search-scope">
                      Project metadata only · name, path, app or status — not file contents.
                    </small>
                    {advancedMode ? <div className="list-controls" role="group" aria-label="Sort and filter projects">
                      <select
                        className="list-control"
                        value={projectSort}
                        onChange={(event) => setProjectSort(event.target.value as ProjectSort)}
                        aria-label="Sort projects"
                        data-help="Order projects by name, by indexed size, or by most recent session activity."
                      >
                        <option value="name">Name A–Z</option>
                        <option value="size">Size</option>
                        <option value="recent">Recently active</option>
                      </select>
                      <select
                        className="list-control"
                        value={effectiveProjectAppFilter}
                        onChange={(event) => setProjectAppFilter(event.target.value)}
                        aria-label="Filter projects by app"
                        data-help="Show only projects belonging to a given AI app."
                      >
                        <option value="all">All apps</option>
                        {projectAppOptions.map((option) => (
                          <option key={option.slug} value={option.slug}>{option.label}</option>
                        ))}
                      </select>
                      <select
                        className="list-control"
                        value={projectStatusFilter}
                        onChange={(event) => setProjectStatusFilter(event.target.value as ProjectStatusFilter)}
                        aria-label="Filter projects by status"
                        data-help="Show only projects that are ready, currently scanning, or need a scan."
                      >
                        <option value="all">Any status</option>
                        <option value="ready">Ready</option>
                        <option value="scanning">Scanning</option>
                        <option value="needs-scan">Needs scan</option>
                      </select>
                    </div> : null}
                    <div className="project-filter-summary" aria-live="polite">
                      <span>{projectSidebarSummaryLabel({
                        matchCount: orderedDisplayedProjects.all.length,
                        totalCount: displayedProjects.length,
                        collapsed: projectListHasOverflow && !projectListExpanded,
                        hiddenCount: displayedSidebarProjects.hiddenCount
                      })}</span>
                      {projectListFiltersActive ? (
                        <button
                          type="button"
                          onClick={clearProjectListFilters}
                          data-help="Reset project search, app filter and status filter."
                        >
                          Clear
                        </button>
                      ) : null}
                    </div>
                  </div>
                ) : null}
                {projectsFromCache ? (
                  <p className="context-list-note cached-project-note" data-help="These projects are the last known local list. Code Hangar is opening the encrypted inventory and will refresh counts and scan states automatically.">
                    Showing cached projects while local inventory opens.
                  </p>
                ) : null}
                {displayedProjects.length === 0 ? (
                  <div className="project-list-empty">
                    <p>No projects are currently shown.</p>
                    <button type="button" onClick={() => showDiscover("projects")} data-help="Open Discover on Find projects so you can search local folders and sessions before adding anything.">
                      <FolderOpen size={14} />
                      Find Projects
                    </button>
                  </div>
                ) : null}
                {displayedProjects.length > 0 && orderedDisplayedProjects.all.length === 0 ? (
                  <div className="project-list-empty">
                    <p>No project names or metadata match the current filters. File contents are searched separately in Document Search.</p>
                    <button type="button" onClick={clearProjectListFilters} data-help="Reset project search, app filter and status filter.">
                      <X size={14} />
                      Clear filters
                    </button>
                  </div>
                ) : null}
                {projectListHasOverflow && projectListExpanded ? (
                  <button
                    type="button"
                    className="project-list-more"
                    onClick={() => setProjectListExpanded(false)}
                    data-help={`Collapse Projects back to the ${PROJECT_LIST_PREVIEW_LIMIT} most useful visible entries so Sessions, Pinned and Recent stay close.`}
                  >
                    Show fewer projects
                  </button>
                ) : null}
                {displayedSidebarProjects.projects.map((project) => {
                  const isFirstArchived = firstRenderedArchivedProjectId === project.id;
                  const isArchived = archivedProjectIds.has(project.id);
                  const isSelectedProject = project.id === selectedProjectId;
                  const keepSelectedArchivedVisible = isArchived && archivedCollapsed && isSelectedProject;
                  const state = projectScanState(project);
                  const watch = projectWatchStatus(project);
                  const watchBadge = watch && watch.state !== "clean" && watch.state !== "disabled" ? watch.state : null;
                  const sidebarPath = projectRootPath(project);
                  const pathShown = showAllProjectPaths;
                  const renderProjectRow = shouldRenderProjectRow({
                    isArchived,
                    archivedCollapsed,
                    isSelected: isSelectedProject
                  });
                  return (
                    <Fragment key={project.id}>
                    {isFirstArchived ? (
                      <button
                        type="button"
                        className="project-archived-divider"
                        aria-expanded={!archivedCollapsed}
                        onClick={() => setArchivedCollapsed((current) => !current)}
                        data-help="Projects an AI app catalogued but hasn't touched recently — no recent local or session activity. They stay listed so you can still open them. Click to expand or collapse."
                      >
                        {archivedCollapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                        <Archive size={12} /> Archived <small>{orderedDisplayedProjects.archived.length}</small>
                      </button>
                    ) : null}
                    {renderProjectRow ? (
                      <ProjectRow
                        project={project}
                        state={state}
                        watchBadge={watchBadge}
                        watchReason={watch ? watch.reason : ""}
                        isSelected={isSelectedProject}
                        keepSelectedArchivedVisible={keepSelectedArchivedVisible}
                        showPath={pathShown}
                        sidebarPath={sidebarPath}
                        onSelect={rowSelectProject}
                        onContextMenu={shellViewer ? undefined : rowShowProjectMenu}
                      />
                    ) : null}
                    </Fragment>
                  );
                })}
                {projectListHasOverflow && !projectListExpanded ? (
                  <button
                    type="button"
                    className="project-list-more"
                    onClick={() => setProjectListExpanded(true)}
                    data-help="Show every visible project. You can also use Find project to jump directly without expanding the whole list."
                  >
                    Show all projects ({displayedSidebarProjects.hiddenCount} more)
                  </button>
                ) : null}
              </div>
            ) : null}
          </section>

          <section className="pane-section">
            <div className="sidebar-section-heading-row">
              <button
                className="section-title section-toggle"
                type="button"
                aria-expanded={!sidebarCollapsed.sessions}
                data-help="Show local AI conversations discovered from ChatGPT, Claude, Cursor, Antigravity/Gemini, Hermes/NemoClaw, OpenClaw and similar tools. Run Discover > Find local sessions to refresh the complete list."
                onClick={() => setSidebarCollapsed((current) => ({ ...current, sessions: !current.sessions }))}
              >
                {sidebarCollapsed.sessions ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
                <MessageSquare size={15} />
                <span>Sessions</span>
                <small>{sessionListCountLabel}</small>
              </button>
              <ConceptHelp concept="sessions" />
            </div>
            {!sidebarCollapsed.sessions ? (
              sessionInventory.length === 0 ? (
                <div className="context-list">
                  <div className="project-list-empty">
                    <p>No local sessions loaded yet.</p>
                    <button type="button" onClick={() => void runProjectDiscovery(500, "sessions")} data-help="Find local AI conversations across coding tools and autonomous agents, including standalone sessions, then group linked conversations under their projects.">
                      <Search size={14} />
                      Find Sessions
                    </button>
                  </div>
                </div>
              ) : (
                <>
                  {sessionInventory.length > 1 ? (
                    <div className="project-list-toolbar session-list-toolbar" role="group" aria-label="Find and filter sessions">
                      <div className="project-search session-search" data-help="Filter local AI sessions by title, path, linked project or AI app. Matching groups open automatically with their top matches while you search.">
                        <Search size={14} aria-hidden="true" />
                        <input
                          className="project-search-input"
                          type="search"
                          value={sessionQuery}
                          onChange={(event) => setSessionQuery(event.target.value)}
                          placeholder="Find session..."
                          aria-label="Find session"
                        />
                        {sessionQuery ? (
                          <button
                            className="project-search-clear"
                            type="button"
                            onClick={() => setSessionQuery("")}
                            aria-label="Clear session search"
                            data-help="Clear the session search text."
                          >
                            <X size={13} />
                          </button>
                          ) : null}
                      </div>
                      <div className="session-scope-control" role="group" aria-label="Session scope">
                        <button
                          className={`segmented session-scope-button ${sessionScope === "all" ? "active" : ""}`}
                          type="button"
                          aria-pressed={sessionScope === "all"}
                          onClick={() => setSessionScope("all")}
                        >
                          All
                        </button>
                        <button
                          className={`segmented session-scope-button ${sessionScope === "independent" ? "active" : ""}`}
                          type="button"
                          aria-pressed={sessionScope === "independent"}
                          onClick={() => setSessionScope("independent")}
                        >
                          Independent
                        </button>
                        <button
                          className={`segmented session-scope-button ${sessionScope === "projects" ? "active" : ""}`}
                          type="button"
                          aria-pressed={sessionScope === "projects"}
                          onClick={() => setSessionScope("projects")}
                        >
                          Projects
                        </button>
                      </div>
                      {advancedMode ? <div className="list-controls" role="group" aria-label="Sort and filter sessions">
                        <select
                          className="list-control"
                          value={sessionSort}
                          onChange={(event) => setSessionSort(event.target.value as SessionSort)}
                          aria-label="Sort sessions"
                          data-help="Order sessions by most recent activity or by name."
                        >
                          <option value="recent">Most recent</option>
                          <option value="name">Name A–Z</option>
                        </select>
                        <select
                          className="list-control"
                          value={effectiveSessionAppFilter}
                          onChange={(event) => setSessionAppFilter(event.target.value)}
                          aria-label="Filter sessions by app"
                          data-help="Show only sessions from a given AI app."
                        >
                          <option value="all">All apps</option>
                          {sessionAppOptions.map((option) => (
                            <option key={option.slug} value={option.slug}>{option.label}</option>
                          ))}
                        </select>
                      </div> : null}
                      <div className="project-filter-summary session-filter-summary" aria-live="polite">
                        <span>{displayedSessionGroups.count} of {sessionInventory.length} shown</span>
                        {sessionListFiltersActive ? (
                          <button
                            type="button"
                            onClick={clearSessionListFilters}
                            data-help="Reset session scope, search and app filter."
                          >
                            Clear
                          </button>
                        ) : null}
                      </div>
                    </div>
                  ) : null}
                  {displayedSessionGroups.count === 0 ? (
                    <div className="project-list-empty session-filter-empty">
                      <p>No sessions match the current filters.</p>
                      <button type="button" onClick={clearSessionListFilters} data-help="Reset session scope, search and app filter.">
                        <X size={14} />
                        Clear filters
                      </button>
                    </div>
                  ) : (
                    <div className="session-sidebar-groups">
                      {renderedSessionGroups.independent.length > 0 ? (
                        <SidebarSessionGroup
                          title="Independent · no project linked"
                          kind="independent"
                          sessions={renderedSessionGroups.independent}
                          defaultCollapsed={false}
                          previewLimit={SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT}
                          openSession={rowOpenSession}
                          showSessionMenu={showSessionMenu}
                          forceExpanded={sessionSearchActive}
                        />
                      ) : null}
                      {renderedSessionGroups.projectGroups.length > 0 ? (
                        <div className="session-sidebar-section project-sessions">
                          <div className="session-sidebar-section-title">
                            <Folder size={12} aria-hidden="true" />
                            <span>By project</span>
                            <small>{displayedSessionGroups.projectGroups.length}</small>
                          </div>
                          {sessionGroupsExpanded && !sessionContentFiltersActive ? (
                            <button
                              type="button"
                              className="project-list-more session-list-more"
                              onClick={() => setSessionGroupsExpanded(false)}
                              data-help={`Collapse the project section back to the ${SESSION_GROUP_PREVIEW_LIMIT} most recent groups.`}
                            >
                              Show fewer project groups
                            </button>
                          ) : null}
                          {renderedSessionGroups.projectGroups.map((group) => (
                            <SidebarSessionGroup
                              key={`project-${group.project.id}`}
                              title={group.project.name}
                              kind="project"
                              sessions={group.sessions}
                              openSession={rowOpenSession}
                              showSessionMenu={showSessionMenu}
                              projectId={group.project.id}
                              onOpenProject={rowSelectProject}
                              forceExpanded={sessionSearchActive}
                            />
                          ))}
                          {renderedSessionGroups.compacted ? (
                            <button
                              type="button"
                              className="project-list-more session-list-more"
                              onClick={() => setSessionGroupsExpanded(true)}
                              data-help="Show every matching project session group. Use Find session to jump directly without expanding the whole list."
                            >
                              Show all project groups ({renderedSessionGroups.hiddenGroupCount} more)
                            </button>
                          ) : null}
                        </div>
                      ) : null}
                      {renderedSessionGroups.hermes.length > 0 ? (
                        <div className="session-sidebar-section autonomous-sessions">
                          <div className="session-sidebar-section-title">
                            <Bot size={12} aria-hidden="true" />
                            <span>Autonomous agents</span>
                          </div>
                          <SidebarSessionGroup title="Hermes" kind="hermes" sessions={renderedSessionGroups.hermes} defaultCollapsed openSession={rowOpenSession} showSessionMenu={showSessionMenu} forceExpanded={sessionSearchActive} />
                        </div>
                      ) : null}
                    </div>
                  )}
                </>
              )
            ) : null}
          </section>

          {displayedPinnedItems.length > 0 ? (
            <section className="pane-section compact">
              <button
                className="section-title section-toggle"
                type="button"
                aria-expanded={!sidebarCollapsed.pinned}
                data-help="Show or hide pinned projects and files."
                onClick={() => setSidebarCollapsed((current) => ({ ...current, pinned: !current.pinned }))}
              >
                {sidebarCollapsed.pinned ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
                <Pin size={15} />
                <span>Pinned</span>
                <small>{displayedPinnedItems.length}</small>
              </button>
              {!sidebarCollapsed.pinned ? (
                <>
                  {displayedPinnedItems.slice(0, 8).map((item) => {
                    const pinnedProject = item.itemKind === "project"
                      ? projects.find((project) => project.id === item.nodeId) ?? null
                      : null;
                    return (
                      <button
                        className="small-row"
                        key={`${item.itemKind}-${item.projectId ?? "none"}-${item.nodeId}`}
                        type="button"
                        data-help={`Open pinned ${item.itemKind} ${item.path}. Right-click for safe actions.`}
                        onClick={() => pinnedProject ? selectProject(pinnedProject.id) : void openNode(item.nodeId, { projectId: item.projectId })}
                        onContextMenu={(event) => pinnedProject
                          ? showProjectMenu(pinnedProject, event)
                          : showFileMenu({ nodeId: item.nodeId, projectId: item.projectId, path: item.path, label: item.path }, event)}
                      >
                        {pinnedProject?.name ?? item.path}
                      </button>
                    );
                  })}
                </>
              ) : null}
            </section>
          ) : null}

          <section className="pane-section compact">
            <button
              className="section-title section-toggle"
              type="button"
              aria-expanded={!sidebarCollapsed.recent}
              data-help="Show or hide recently opened files. Opening from here keeps the current order."
              onClick={() => setSidebarCollapsed((current) => ({ ...current, recent: !current.recent }))}
            >
              {sidebarCollapsed.recent ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
              <History size={15} />
              <span>Recent</span>
              <small>{displayedRecentItems.length}</small>
            </button>
            {!sidebarCollapsed.recent ? (
              <>
                {displayedRecentItems.length === 0 ? <p className="muted">No recent files in this view.</p> : null}
                {displayedRecentItems.slice(0, recentShowAll ? displayedRecentItems.length : 5).map((item) => (
                  <button
                    className="small-row"
                    key={`${item.projectId ?? "none"}-${item.nodeId}-${item.openedAt}`}
                    type="button"
                    data-help={`Open recent file ${item.path} without moving it to the top. Right-click for safe actions.`}
                    onClick={() => openNode(item.nodeId, { recordRecent: false, projectId: item.projectId })}
                    onContextMenu={(event) => showFileMenu({ nodeId: item.nodeId, projectId: item.projectId, path: item.path, label: item.path }, event)}
                  >
                    {item.path}
                  </button>
                ))}
                {displayedRecentItems.length > 5 ? (
                  <button
                    type="button"
                    className="session-group-more"
                    onClick={() => setRecentShowAll((value) => !value)}
                    data-help={recentShowAll ? "Return to the 5 most recently opened files." : "Show every recently opened file Code Hangar is tracking."}
                  >
                    {recentShowAll ? "Show top 5" : `Show all recent files (${displayedRecentItems.length})`}
                  </button>
                ) : null}
              </>
            ) : null}
          </section>
          </>
          ) : null}
        </Sidebar>

        <div
          className={`pane-resizer left-resizer ${leftPaneCollapsedForLayout ? "collapsed-resizer" : ""}`}
          role="separator"
          aria-label="Resize projects pane"
          aria-orientation="vertical"
          data-help={leftPaneCollapsedForLayout ? "Projects sidebar is collapsed. Expand it before resizing." : "Drag to resize the projects sidebar."}
          onMouseDown={leftPaneCollapsedForLayout ? undefined : startPaneResize("left")}
        />

        {isProjectLayout ? (
          <>
        <ProjectWorkspace>
          {primaryView === "review" ? (
            <div className="project-review-workspace">
              <button type="button" className="tool-back-button" onClick={() => showProjectWorkspace("context")} data-help="Return to the selected project's normal Context, Files, Space, Connections and Sessions workspace.">
                <ArrowLeft size={15} />
                Back to {selectedProject?.name ?? "project"}
              </button>
              <Suspense fallback={<ToolViewFallback />}>
              <ReviewImpactView
                planTargetNode={planTargetNode}
                setPlanTargetNode={setPlanTargetNode}
                selectedProject={selectedProject}
                selectedProjectId={selectedProjectId}
                preview={preview}
                planJobId={planJobId}
                planJobStatus={planJobStatus}
                planLoading={planLoading}
                operationPlan={activeOperationPlan}
                setOperationPlan={setOperationPlan}
                riskReport={activeRiskReport}
                setRiskReport={setRiskReport}
                reportLoading={reportLoading}
                advancedMode={advancedMode}
                setAdvancedMode={setAdvancedMode}
                mutationAvailable={mutationAvailable}
                mutationBackupLevel={mutationBackupLevel}
                setMutationBackupLevel={setMutationBackupLevel}
                mutationAllowSameVolume={mutationAllowSameVolume}
                setMutationAllowSameVolume={setMutationAllowSameVolume}
                mutationModeToken={mutationModeToken}
                mutationBusy={mutationBusy}
                mutationMessage={mutationMessage}
                lastMutationMove={lastMutationMove}
                setPlanJobId={setPlanJobId}
                setPlanJobStatus={setPlanJobStatus}
                setPlanLoading={setPlanLoading}
                setStatusText={setStatusText}
                buildPreviewPlan={buildPreviewPlan}
                cancelPreviewPlan={cancelPreviewPlan}
                exportRiskReport={exportRiskReport}
                enterMutationMode={enterMutationMode}
                runMutationBackup={runMutationBackup}
                runMutationMove={runMutationMove}
                selectProject={selectProject}
                showDiscover={showDiscover}
                showRecovery={showRecovery}
                setOrphanScope={setOrphanScope}
                setOrphanMode={setOrphanMode}
                setOrphanAutoRunSeq={setOrphanAutoRunSeq}
              />
              </Suspense>
            </div>
          ) : previewSession ? (
            <SessionCenterView
              session={previewSession}
              preview={sessionPreview}
              loading={sessionPreviewLoading}
              loadKind={sessionPreviewLoadKind}
              error={sessionPreviewError}
              onLoadMore={loadMoreSessionPreview}
              onLoadFull={loadFullSessionPreview}
              onBack={() => {
                // Prefer the actual origin screen (for example a filtered project
                // session list). With no history, fall back to the linked project.
                if (viewBackStack.length > 0) {
                  void goBack();
                } else if (previewSessionProject) {
                  selectProject(previewSessionProject.id);
                } else {
                  invalidateShellOpenIntent();
                  setPreviewSession(null);
                }
              }}
              backLabel={previewSessionProject?.name ?? null}
            />
          ) : (
          <ProjectCenterView
            projectView={projectView}
            setProjectView={showProjectWorkspace}
            selectedProject={selectedProject}
            selectedProjectId={selectedProjectId}
            preview={preview}
            folderExplanation={folderExplanation}
            previewMode={previewMode}
            setPreviewMode={setPreviewMode}
            editor={{
              available: mutationAvailable
                && preview?.state === "ready"
                && preview.nodeId > 0
                && !preview.truncated
                && !preview.wasRevealed
                && (preview.fileKind === "text" || preview.fileKind === "markdown"),
              draft: editDraft,
              saving: editSaving,
              dirty: editDraft !== null && editDraft !== (preview?.source ?? ""),
              canUndo: !!editUndo && editUndo.nodeId === preview?.nodeId,
              onChange: setEditDraft,
              onSave: saveEditedFile,
              onRevert: revertEditedFile,
              onUndo: () => void undoEditedFile()
            }}
            togglePin={togglePin}
            selectedPinned={selectedPinned}
            tabs={tabs}
            draggedTabNodeId={draggedTabNodeId}
            tabDropTargetNodeId={tabDropTargetNodeId}
            showTabMenu={showTabMenu}
            suppressNextTabClickRef={suppressNextTabClickRef}
            openNode={(nodeId) => void openWorkspaceTab(nodeId)}
            openNodeInTree={(nodeId, projectId) => void openNodeInTree(nodeId, { projectId })}
            startTabPointerDrag={startTabPointerDrag}
            closeTab={closeTab}
            loadStatus={projectWorkspace.loadStatus}
            loadError={projectWorkspace.error}
            loadProjectData={loadProjectData}
            contentGridStyle={contentGridStyle}
            rootTreeItems={rootTreeItems}
            expandedTree={expandedTree}
            treePages={treePages}
            treeLoading={treeLoading}
            toggleExpandedTree={toggleExpandedTree}
            loadTreeChildren={loadTreeChildren}
            continueSubtreeScan={continueSubtreeScan}
            explainFolder={explainFolder}
            showTreeMenu={showTreeMenu}
            showFileMenu={showFileMenu}
            showSessionMenu={showSessionMenu}
            showSelectedTextMenu={showSelectedTextMenu}
            zoneShowProtectedMetadata={zoneShowProtectedMetadata}
            startTreeResize={startTreeResize}
              contextFiles={contextFiles}
              projectOverlapWarning={selectedProjectOverlapWarning}
            selectedFootprint={selectedFootprintForView}
            projectScanState={projectScanState}
            projectStateLabel={projectStateLabel}
            canRescanProject={Boolean(selectedProjectScanRoot)}
            onRescanProject={() => {
              if (selectedProjectScanRoot) void startRootScan(selectedProjectScanRoot.id);
            }}
            onOpenScanFolders={() => showSettings("folders")}
            projectSessions={selectedProjectSessions}
            onOpenSession={openSession}
            onFindSessions={() => void runProjectDiscovery(500, "sessions")}
            relationships={relationships}
            relationshipsMembership={relationshipsMembership}
            relationshipsLoading={relationshipsLoading}
            graphMap={graphMap}
            graphMapLoading={graphMapLoading}
            graphMapError={graphMapError}
            graphMapExpansion={graphMapExpansion}
            onExpandGraphMap={() => void runGraphMapExpansion(true)}
            onPauseGraphMap={pauseGraphMapExpansion}
            onContinueGraphMap={() => void runGraphMapExpansion(false)}
            revealPreview={revealPreview}
            zoneAllowSensitiveReveal={zoneAllowSensitiveReveal}
            openProtectedSettings={() => showSettings("protection")}
            setStatusText={setStatusText}
            onFileMutated={async (nodeId) => {
              const projectId = preview?.nodeId === nodeId ? preview.projectId : selectedProjectId;
              if (!projectId) {
                setStatusText("The project owning this file is no longer selected, so its preview was not refreshed.");
                return;
              }
              invalidatePreviewCache(nodeId);
              if (editDraftNodeRef.current === nodeId) {
                editDraftNodeRef.current = null;
                setEditUndo(null);
              }
              await openNode(nodeId, {
                projectId,
                mode: previewMode,
                recordRecent: false,
                refreshOnly: true,
                replaceHistory: true,
                allowProjectSwitch: false
              });
            }}
            changesUnlocked={changesUnlocked}
            onRequestChangeAccess={requestChangeAccess}
            onRelockChanges={() => {
              setUnlockedChangeProjectId(null);
              setStatusText("Project file changes are locked again.");
            }}
            onOverview={showOverview}
            onAddProjects={() => setAddProjectsVisible(true)}
            editionExtensionActive={editionExtensionActive}
            recapDetailLayer={editionExtensionActive ? EditionRecapDetailLayer : undefined}
          />
          )}
        </ProjectWorkspace>

        <div
          className={`pane-resizer right-resizer ${rightPaneCollapsedForLayout ? "collapsed-resizer" : ""}`}
          role="separator"
          aria-label="Resize inspector pane"
          aria-orientation="vertical"
          data-help={rightPaneCollapsedForLayout ? "Details pane is collapsed. Expand it before resizing." : "Drag to resize the details pane."}
          onMouseDown={rightPaneCollapsedForLayout ? undefined : startPaneResize("right")}
        />
          </>
        ) : null}

        <RightWorkspace collapsed={rightPaneCollapsedForLayout} scrollResetKey={rightWorkspaceScrollKey}>
          {isProjectLayout ? (
            <>
            <button
              className="pane-collapse-button right"
              type="button"
              aria-label={rightPaneCollapsedForLayout ? "Expand details pane" : "Collapse details pane"}
              data-help={rightPaneCollapsedForLayout ? "Expand the details pane." : "Collapse the details pane to give more room to the project workspace."}
              onClick={() => {
                if (rightPaneCollapsedForLayout) {
                  setPaneCollapsed((current) => ({ ...current, right: false }));
                  setProjectInspectorExpanded(true);
                  return;
                }
                if (projectInspectorAutoCollapse) {
                  setProjectInspectorExpanded(false);
                  return;
                }
                setPaneCollapsed((current) => ({ ...current, right: true }));
              }}
            >
              {rightPaneCollapsedForLayout ? <ChevronLeft size={16} /> : <ChevronRight size={16} />}
            </button>
            {!rightPaneCollapsedForLayout ? (
            <div className="details-pane-heading" data-help="Details change with the file or folder you are inspecting. Technical fields live under Advanced details.">
              <Info size={16} />
              <div>
                <strong>Details</strong>
                <span>{detailsPaneSubject}</span>
              </div>
            </div>
            ) : null}
            </>
          ) : (
            <header className="tool-workspace-header">
              <div className="tool-workspace-copy">
                <span>{workspaceEyebrow(primaryView)}</span>
                <div className="heading-with-help">
                  <h1>{workspaceTitle(primaryView, discoverView, settingsView)}</h1>
                  <WorkspaceConceptHelp view={primaryView} discoverView={discoverView} settingsView={settingsView} />
                </div>
                <p>{workspaceDescription(primaryView, discoverView, settingsView)}</p>
              </div>
              {primaryView === "discover" ? (
                <nav className="tool-subnav" aria-label="Discover tools">
                  <button className={discoverView === "projects" ? "active" : ""} type="button" onClick={() => showDiscover("projects")} data-help="Find local project folders and local AI sessions from known folders and app metadata. Nothing is changed until you add a candidate.">
                    <FolderOpen size={15} /> Projects & sessions
                  </button>
                  <button className={discoverView === "search" ? "active" : ""} type="button" onClick={() => showDiscover("search")} data-help="Search the content of locally indexed, non-sensitive documents.">
                    <Search size={15} /> Document search
                  </button>
                  <button className={discoverView === "lost" ? "active" : ""} type="button" onClick={() => showDiscover("lost")} data-help="Find projects or folders that may have been forgotten, using passive local signals.">
                    <Compass size={15} /> Forgotten projects
                  </button>
                  <button className={discoverView === "assets" ? "active" : ""} type="button" onClick={() => showDiscover("assets")} data-help="Find local files with no known references. Results are candidates for review, not delete recommendations.">
                    <AlertTriangle size={15} /> Unreferenced files
                  </button>
                  <button className={discoverView === "duplicates" ? "active" : ""} type="button" onClick={() => showDiscover("duplicates")} data-help="Find possible duplicate files using bounded local comparisons. Nothing is removed.">
                    <Copy size={15} /> Duplicate files
                  </button>
                  <button className={discoverView === "organize" ? "active" : ""} type="button" onClick={() => showDiscover("organize")} data-help="See where your AI models and projects are scattered across disk, grouped by location, with duplicates and idle projects flagged. Read-only — tidy through Safe Manage.">
                    <Layers size={15} /> Organize
                  </button>
                </nav>
              ) : null}
              {primaryView === "settings" ? (
                <nav className="tool-subnav" aria-label="Settings sections">
                  <button className={settingsView === "folders" ? "active" : ""} type="button" onClick={() => showSettings("folders")} data-help="Manage the local folders Code Hangar inventories. Unregistering never deletes real files.">
                    <Database size={15} /> Scan folders
                  </button>
                  <button className={settingsView === "protection" ? "active" : ""} type="button" onClick={() => showSettings("protection")} data-help="Understand and temporarily adjust local preview protection for this session.">
                    <Shield size={15} /> Protected locations
                  </button>
                  <button className={settingsView === "appearance" ? "active" : ""} type="button" onClick={() => showSettings("appearance")} data-help="Adjust text size, layout density, contrast and motion for this local UI.">
                    <SlidersHorizontal size={15} /> Appearance
                  </button>
                  <button className={settingsView === "advanced" ? "active" : ""} type="button" onClick={() => showSettings("advanced")} data-help="Inspect build capability, resource plans and local safety state without changing any safety rule.">
                    <Settings size={15} /> System
                  </button>
                </nav>
              ) : null}
            </header>
          )}

          <Suspense fallback={<ToolViewFallback />}>
          {primaryView === "safe_manage" && rightPaneView === "plan" ? (
            <SafeManagePortfolioView
              active
              extraRecommendation={EditionSafeManageRecommendation
                ? (assessment) => (
                    <Suspense fallback={null}>
                      <EditionSafeManageRecommendation assessment={assessment} />
                    </Suspense>
                  )
                : undefined}
              onInspectProject={selectProject}
              onPrepareDecision={prepareSafeManageDecision}
              onStatus={setStatusText}
            />
          ) : null}
          {primaryView === "settings" && settingsView === "advanced" ? (
            <>
            <section className="pane-section tool-content-section settings-system-view">
              <div className="dashboard-grid wide settings-system-grid">
                <div className="dashboard-card">
                  <h3>Build and safety</h3>
                  <dl className="inspector-list small">
                    <dt>Disk actions</dt>
                    <dd>{mutationAvailable ? "Available after review and confirmation" : "Read-only build"}</dd>
                    <dt>Protection</dt>
                    <dd>Always enforced</dd>
                  </dl>
                </div>
                <div className="dashboard-card" data-help="Optional per-user Windows integration. These switches register Code Hangar locally with File Explorer; they do not change project files or force a Windows default app.">
                  <h3>Windows Explorer</h3>
                  <p className="muted help-copy">Open projects and text files directly in Code Hangar. Windows keeps final control of default-app choices.</p>
                  {shellIntegration?.available ? (
                    <>
                      <label className="toggle-row" data-help="Adds Code Hangar to Open with for .md, .markdown and .mdx files. Windows requires you to choose the final default in Default Apps.">
                        <input
                          type="checkbox"
                          checked={shellIntegration.markdownRegistered}
                          disabled={shellIntegrationBusy}
                          onChange={(event) => void updateShellIntegration(event.target.checked, shellIntegration.contextMenuRegistered)}
                        />
                        <span>Register as a Markdown app<small>Adds Code Hangar to Windows Open with; it does not force the default.</small></span>
                      </label>
                      <label className="toggle-row" data-help="Adds Open in Code Hangar for text files and Open folder in Code Hangar for folders and folder backgrounds.">
                        <input
                          type="checkbox"
                          checked={shellIntegration.contextMenuRegistered}
                          disabled={shellIntegrationBusy}
                          onChange={(event) => void updateShellIntegration(shellIntegration.markdownRegistered, event.target.checked)}
                        />
                        <span>File Explorer context menu<small>Text files and folders; registration is per-user and reversible.</small></span>
                      </label>
                      <label className="toggle-row" data-help="Starts Code Hangar hidden in the notification area when you sign in and performs bounded, read-only refreshes of registered local projects.">
                        <input
                          type="checkbox"
                          checked={shellIntegration.runAtLogin}
                          disabled={shellIntegrationBusy || (shellIntegration.runAtLogin && !shellIntegration.runAtLoginOwnedByCurrentEdition)}
                          onChange={(event) => void updateBackgroundStartup(event.target.checked)}
                        />
                        <span>Start quietly with Windows<small>Runs in the tray and keeps registered local projects ready.</small></span>
                      </label>
                      <div className="button-row compact">
                        <button type="button" className="secondary-button" disabled={shellIntegrationBusy || !shellIntegration.markdownRegistered} onClick={() => void openWindowsDefaultApps()}>
                          Open Windows Default Apps
                        </button>
                        <button type="button" className="secondary-button" disabled={shellIntegrationBusy} onClick={() => void refreshProjectsInBackground()}>
                          Refresh projects now
                        </button>
                      </div>
                      <p className="muted help-copy">Closing the window hides Code Hangar in the tray. Use <strong>Exit Code Hangar</strong> from the tray menu to stop it completely.</p>
                      {shellIntegration.runAtLogin && !shellIntegration.runAtLoginOwnedByCurrentEdition ? (
                        <p className="muted help-copy">Windows starts another installed edition{shellIntegration.runAtLoginOwnerExecutable ? ` (${shellIntegration.runAtLoginOwnerExecutable})` : ""}. Change this setting from that edition.</p>
                      ) : null}
                      {!shellIntegration.ownedByCurrentEdition && shellIntegration.ownerExecutable ? (
                        <p className="muted help-copy">Another installed Code Hangar edition currently owns these entries. Changing a switch here transfers them to this executable.</p>
                      ) : null}
                    </>
                  ) : (
                    <p className="muted help-copy">Available in the installed Windows desktop app.</p>
                  )}
                  {shellIntegrationError ? <p className="scan-error">{shellIntegrationError}</p> : null}
                </div>
                <div className="dashboard-card" data-help="Analyze this PC locally and show the CPU/RAM budget Code Hangar applies to Balanced, Priority and Max CPU.">
                  <div className="card-title-row">
                    <h3>Resource profile</h3>
                    <button type="button" className="secondary-button" disabled={resourceProfileLoading} onClick={() => void loadSystemResourceProfile()} data-help="Re-read local CPU and memory information and refresh the recommended per-mode resource plan.">
                      {resourceProfileLoading ? "Analyzing..." : "Analyze this PC"}
                    </button>
                  </div>
                  {resourceProfileError ? <p className="scan-error">{resourceProfileError}</p> : null}
                  {resourceProfile ? (
                    <dl className="inspector-list small">
                      <dt>Logical CPU threads</dt>
                      <dd>{resourceProfile.logicalCpuCount}</dd>
                      <dt>Total RAM</dt>
                      <dd>{resourceProfile.totalMemoryBytes != null ? formatBytes(resourceProfile.totalMemoryBytes) : "Not available"}</dd>
                      <dt>Available RAM now</dt>
                      <dd>{resourceProfile.availableMemoryBytes != null ? formatBytes(resourceProfile.availableMemoryBytes) : "Not available"}</dd>
                      <dt>GPU / VRAM</dt>
                      <dd>{resourceProfile.gpuAcceleration}</dd>
                    </dl>
                  ) : (
                    <p className="muted help-copy">Open this panel or click Analyze to map Code Hangar's modes to this PC. The scan is local-only and reads CPU/RAM capability, not project contents.</p>
                  )}
                </div>
                <div className="dashboard-card resource-plan-card" data-help="These are the actual knobs used for newly started heavy tasks: scan workers, scan batch size, memory budget and Windows process priority.">
                  <h3>Mode resource plan</h3>
                  <p className="muted help-copy">Mode changes persist until you choose another mode. Running jobs keep their worker count, but Windows process priority follows the current mode.</p>
                  <div className="resource-plan-list">
                    {(resourceProfile?.plans ?? []).map((plan) => {
                      const mode = asPerformanceMode(plan.mode);
                      const active = mode === performanceMode;
                      return (
                        <div className={`resource-plan-row ${active ? "active" : ""}`} key={plan.mode} data-help={performancePlanHelp(plan.label, plan.cpuThreads, plan.processPriority, plan.scanBatchSize)}>
                          <div>
                            <strong>{plan.label}</strong>
                            <small>{plan.cpuThreads} CPU threads · batch {plan.scanBatchSize.toLocaleString()} · {plan.processPriority}</small>
                            <small>RAM budget: {plan.memoryBudgetBytes != null ? formatBytes(plan.memoryBudgetBytes) : "not available"}</small>
                          </div>
                          <button type="button" disabled={!mode || active} onClick={() => mode ? choosePerformanceMode(mode) : undefined} data-help={mode ? `Switch to ${plan.label}. This persists until changed and affects newly started heavy tasks.` : "This mode is not recognized by this build."}>
                            {active ? "Active" : "Use"}
                          </button>
                        </div>
                      );
                    })}
                    {!resourceProfile && !resourceProfileLoading ? <p className="muted">Analyze this PC to show the exact per-mode plan.</p> : null}
                  </div>
                </div>
              </div>
              <SettingsDiagnosticsExportCard />
              <div className="dashboard-card">
                <h3>Technical vocabulary</h3>
                <dl className="inspector-list">
                  <dt>Inventory</dt>
                  <dd>Local metadata about files and folders that Code Hangar has scanned.</dd>
                  <dt>Safe Manage review</dt>
                  <dd>A read-only local review of ownership, references, protection and scan gaps.</dd>
                  <dt>Activity record</dt>
                  <dd>The local history used to explain and recover disk actions in supported builds.</dd>
                  <dt>Holding area</dt>
                  <dd>A local, recoverable location used before any final removal is considered.</dd>
                </dl>
              </div>
            </section>
            {connectorFrontendBuild ? (
              <Suspense fallback={null}>
                <EditionSettingsPanel
                  projects={projects.filter((project) => !isDemoProject(project))}
                  currentFile={preview && preview.nodeId > 0 ? { nodeId: preview.nodeId, displayName: preview.displayName } : null}
                  confirm={requestConfirm}
                  onCopy={(value) => void copyPath(value)}
                  onStatus={setStatusText}
                />
              </Suspense>
            ) : null}
            </>
          ) : null}
          {primaryView === "settings" && settingsView === "folders" ? (
            <SettingsFoldersView
              roots={roots}
              rootIsScanning={rootIsScanning}
              startRootScan={startRootScan}
              toggleRoot={toggleRoot}
              unregisterRoot={unregisterRoot}
              latestScanStatus={latestScanStatus}
              scanStatusList={scanStatusList}
              cancelScan={cancelScan}
              onRescanAll={() => void rescanAllRoots()}
              onCompactDatabase={() => void compactDatabase()}
              compactBusy={compactBusy}
              onResetAll={() => setResetAllVisible(true)}
            />
          ) : null}
          {primaryView === "settings" && settingsView === "appearance" ? (
            <SettingsAppearanceView
              fontSize={appearance.fontSize}
              setFontSize={(fontSize) => setAppearance((current) => ({ ...current, fontSize }))}
              density={appearance.density}
              setDensity={(density) => setAppearance((current) => ({ ...current, density }))}
              contrast={appearance.contrast}
              setContrast={(contrast) => setAppearance((current) => ({ ...current, contrast }))}
              reduceMotion={appearance.reduceMotion}
              setReduceMotion={(reduceMotion) => setAppearance((current) => ({ ...current, reduceMotion }))}
              showTopbarNav={showTopbarNav}
              setShowTopbarNav={setShowTopbarNav}
              showAllProjectPaths={showAllProjectPaths}
              setShowAllProjectPaths={setShowAllProjectPaths}
              demosVisible={demosVisible}
              demoVisibilityAutomatic={showDemoProjects === null}
              setDemosVisible={setShowDemoProjects}
              startupPreferences={startupPreferences}
              setStartupPreferences={setStartupPreferences}
              replayTour={startTour}
              resetLayout={() => {
                setPaneWidths({ left: DEFAULT_LEFT_PANE_WIDTH, right: DEFAULT_RIGHT_PANE_WIDTH });
                setTreePaneWidth(DEFAULT_TREE_PANE_WIDTH);
                setPaneCollapsed({ left: false, right: false });
                setStatusText("Pane widths reset.");
              }}
            />
          ) : null}

          {isProjectLayout && previewSession ? (
            <SessionDetailsPanel
              session={previewSession}
              preview={sessionPreview}
              linkedProject={previewSessionProject}
              canReveal={previewPolicy.allowSensitiveReveal}
              revealing={sessionRevealing}
              onReveal={revealSessionTokens}
              onOpenProject={selectProject}
              onOpenProtectedSettings={() => showSettings("protection")}
              onCopyPath={copyPath}
            />
          ) : isProjectLayout && rightPaneView === "inspector" ? (
            <InspectorView
              preview={inspectorPreview}
              folderExplanation={inspectorFolderExplanation}
              context={inspectorContext}
              gitStatus={gitStatus}
              relationships={relationships}
              relationshipsLoading={relationshipsLoading}
              previewOrphanStatus={previewOrphanStatus}
              advancedMode={advancedMode}
              selectedProject={selectedProject}
              mutationAvailable={mutationAvailable}
              mutationLockLoading={mutationLockLoading}
              mutationLockInspection={mutationLockInspection}
              duplicateLoading={duplicateLoading}
              fileOrphanLoading={fileOrphanLoading}
              inspectCurrentFileDuplicates={inspectCurrentFileDuplicates}
              evaluateCurrentFileOrphan={evaluateCurrentFileOrphan}
              inspectCurrentFileLock={inspectCurrentFileLock}
              setPlanTargetNode={setPlanTargetNode}
              setOperationPlan={setOperationPlan}
              setRiskReport={setRiskReport}
              showReview={showReview}
              setStatusText={setStatusText}
              openNode={(nodeId, projectId) => void openNodeInTree(nodeId, { projectId })}
              editionExtensionActive={editionExtensionActive}
            />
          ) : null}

          {rightPaneView === "dashboard" ? (
            <section className="pane-section">
              <SectionTitle icon={<BarChart3 size={15} />} label="Local inventory overview" />
              <OverviewView
                showFlow={primaryView === "overview"}
                selectedProjectId={selectedProjectId}
                realProjectCount={realProjectCount}
                mutationAvailable={mutationAvailable}
                dashboard={dashboard}
                watcherStatus={watcherStatus}
                dashboardLoading={dashboardLoading}
                gitStatus={gitStatus}
                adapters={adapters}
                demosVisible={demosVisible}
                demoPreference={showDemoProjects}
                reduceMotion={appearance.reduceMotion}
                formatBytes={formatBytes}
                formatOptionalBytes={formatOptionalBytes}
                onOpenProject={selectProject}
                onAddProjects={() => setAddProjectsVisible(true)}
                onSetShowDemoProjects={setShowDemoProjects}
                onOpenScanFolders={() => showSettings("folders")}
                onUnderstandProject={() => selectedProjectId ? showProjectWorkspace("context") : focusProjectPicker()}
                onOpenFiles={() => selectedProjectId ? showProjectWorkspace("files") : focusProjectPicker()}
                reviewProjectGroups={reviewProjectGroups}
                reviewInventoryState={realProjectCount === 0 ? "fresh" : sessionInventoryState}
                reviewInventoryRefreshing={sessionInventoryRefreshing}
                reviewInventoryError={sessionInventoryError}
                onRefreshReviewInventory={() => void runProjectDiscovery(0, "sessions")}
                onOpenRecap={() => selectedProjectId ? showProjectWorkspace("recap") : focusProjectPicker()}
                onOpenProjectRecap={openProjectRecap}
              />
            </section>
          ) : null}

          {rightPaneView === "search" && discoverView === "projects" ? (
            <DiscoverProjectDiscoveryView
              loading={projectDiscoveryLoading}
              error={projectDiscoveryError}
              report={projectDiscoveryReport}
              runProjectDiscovery={runProjectDiscovery}
              addCandidateAsRoot={addDiscoveryCandidateAsRoot}
              addVisibleCandidatesAsRoots={addVisibleDiscoveryCandidatesAsRoots}
              onOpenSession={openSession}
            />
          ) : null}

          {rightPaneView === "search" && discoverView === "search" ? (
            <DiscoverSearchView
              documentQuery={documentQuery}
              setDocumentQuery={setDocumentQuery}
              documentScope={documentScope}
              setDocumentScope={setDocumentScope}
              documentProjectInput={documentProjectInput}
              setDocumentProjectInput={setDocumentProjectInput}
              documentProjectResolution={documentProjectResolution}
              documentKind={documentKind}
              setDocumentKind={setDocumentKind}
              documentPathFilter={documentPathFilter}
              setDocumentPathFilter={setDocumentPathFilter}
              documentNameFilter={documentNameFilter}
              setDocumentNameFilter={setDocumentNameFilter}
              documentLimit={documentLimit}
              setDocumentLimit={setDocumentLimit}
              documentSearching={documentSearching && documentSearchIsCurrent}
              runDocumentSearch={runDocumentSearch}
              documentSearchRan={documentSearchRan && documentSearchIsCurrent}
              documentHits={documentSearchIsCurrent ? displayedDocumentHits : []}
              documentSearchTruncated={documentSearchIsCurrent && documentSearchTruncated}
              documentSearchDuration={documentSearchIsCurrent ? documentSearchDuration : null}
              documentSearchError={documentSearchError}
              projects={displayedProjects}
              openDocumentHit={openDocumentHit}
              showFileMenu={showFileMenu}
              selectedProjectId={selectedProjectId}
              showReview={showReview}
            />
          ) : null}

          {rightPaneView === "orphans" ? (
            <DiscoverOrphansView
              orphanMode={orphanMode}
              orphanScope={orphanScope}
              setOrphanScope={setOrphanScope}
              orphanMinPreset={orphanMinPreset}
              setOrphanMinPreset={setOrphanMinPreset}
              orphanCustomMiB={orphanCustomMiB}
              setOrphanCustomMiB={setOrphanCustomMiB}
              lostStalePreset={lostStalePreset}
              setLostStalePreset={setLostStalePreset}
              lostKeyword={lostKeyword}
              setLostKeyword={setLostKeyword}
              savedLostPresets={savedLostPresets}
              applyLostPreset={applyLostPreset}
              orphanAssetKind={orphanAssetKind}
              setOrphanAssetKind={setOrphanAssetKind}
              orphanMinConfidence={orphanMinConfidence}
              setOrphanMinConfidence={setOrphanMinConfidence}
              advancedMode={advancedMode}
              lostSignals={lostSignals}
              toggleLostSignal={toggleLostSignal}
              lostPresetName={lostPresetName}
              setLostPresetName={setLostPresetName}
              saveLostPreset={saveLostPreset}
              orphanIncludePartial={orphanIncludePartial}
              setOrphanIncludePartial={setOrphanIncludePartial}
              orphanLoading={orphanLoading && orphanSearchIsCurrent && orphanActiveSearchCriteria === currentOrphanSearchCriteria}
              orphanSearchError={orphanSearchError}
              runOrphanSearch={runOrphanSearch}
              lostProjectCandidates={orphanMode === "lost" && orphanSearchIsCurrent ? lostProjectCandidates : null}
              showForgottenProjectMenu={showForgottenProjectMenu}
              selectProject={selectProject}
              showReview={showReview}
              setPlanTargetNode={setPlanTargetNode}
              buildPreviewPlan={buildPreviewPlan}
              orphanCandidates={orphanMode === "assets" && orphanSearchIsCurrent ? orphanCandidates : null}
              openNode={(nodeId, projectId) => void openNode(nodeId, { projectId })}
              showFileMenu={showFileMenu}
              projects={displayedProjects}
              selectedProjectId={selectedProjectId}
            />
          ) : null}

          {rightPaneView === "duplicates" ? (
            <DiscoverDuplicatesView
              duplicateScope={duplicateScope}
              setDuplicateScope={setDuplicateScope}
              preview={preview}
              duplicateMinPreset={duplicateMinPreset}
              setDuplicateMinPreset={setDuplicateMinPreset}
              duplicateCustomMiB={duplicateCustomMiB}
              setDuplicateCustomMiB={setDuplicateCustomMiB}
              duplicateFileKind={duplicateFileKind}
              setDuplicateFileKind={setDuplicateFileKind}
              duplicateLimit={duplicateLimit}
              setDuplicateLimit={setDuplicateLimit}
              duplicateLoading={duplicateLoading && duplicateSearchIsCurrent}
              duplicateSearchError={duplicateSearchError}
              loadDuplicateCandidates={loadDuplicateCandidates}
              duplicateHasRun={duplicateHasRun && duplicateSearchIsCurrent}
              duplicateCandidates={duplicateSearchIsCurrent ? duplicateCandidates : null}
              advancedMode={advancedMode}
              openNode={(nodeId, projectId) => void openNode(nodeId, { projectId })}
              showFileMenu={showFileMenu}
              projects={displayedProjects}
              selectedProjectId={selectedProjectId}
              confirmState={duplicateConfirmState}
              setConfirmState={setDuplicateConfirmState}
            />
          ) : null}

          {rightPaneView === "organize" ? (
            <OrganizeView
              active={primaryView === "discover" && discoverView === "organize"}
              projects={displayedProjects}
              onOpenNode={(nodeId, projectId) => void openNode(nodeId, { projectId })}
              onSafeManageProject={(projectId) => {
                selectProject(projectId);
                showReview();
              }}
            />
          ) : null}

          {rightPaneView === "activity" ? (
            <RecoveryView
              mutationAvailable={mutationAvailable}
              mutationMessage={mutationMessage}
              mutationActivity={mutationActivity}
              mutationBusy={mutationBusy}
              finalRemoveExecutionUnknown={finalRemoveExecutionUnknown}
              finalRemoveProgress={finalRemoveProgress}
              finalRemoveJobId={finalRemoveJobId}
              finalRemoveBatchId={finalRemoveBatchId}
              finalRemovePreview={finalRemovePreview}
              finalRemovePreviewLoading={finalRemovePreviewLoading}
              finalRemoveUnavailableReason={finalRemoveUnavailableReason}
              finalRemoveResult={finalRemoveResult}
              finalRemoveEnabled={finalRemoveEnabled}
              finalRemoveCapabilityLoading={finalRemoveCapabilityLoading}
              advancedMode={advancedMode}
              projects={projects}
              appRemovals={appRemovals}
              restoreAppRemoval={restoreAppRemoval}
              refreshMutationActivity={refreshRecoveryData}
              runMutationRestore={runMutationRestore}
              runMutationRestoreElsewhere={runMutationRestoreElsewhere}
              onReviewFinalRemove={reviewFinalRemove}
              onSetFinalRemoveEnabled={setFinalRemoveCapability}
              onStopFinalRemoveBatch={stopFinalRemoveBatch}
              onDiscoverProjects={() => showDiscover("projects")}
              onOpenScanFolders={() => showSettings("folders")}
              currentFile={preview && preview.nodeId > 0 ? { nodeId: preview.nodeId, displayName: preview.displayName } : null}
              onFileHistoryMutated={(nodeId) => invalidatePreviewCache(nodeId)}
              setStatusText={setStatusText}
            />
          ) : null}

          {rightPaneView === "zones" && !(primaryView === "settings" && settingsView === "advanced") ? (
            <SettingsProtectionView
              zones={zones}
              zoneAllowSensitiveReveal={zoneAllowSensitiveReveal}
              setZoneAllowSensitiveReveal={setZoneAllowSensitiveReveal}
              zoneRelaxNonStrongPreview={zoneRelaxNonStrongPreview}
              setZoneRelaxNonStrongPreview={setZoneRelaxNonStrongPreview}
              zoneShowProtectedMetadata={zoneShowProtectedMetadata}
              setZoneShowProtectedMetadata={setZoneShowProtectedMetadata}
            />
          ) : null}
          </Suspense>
        </RightWorkspace>
      </WorkspaceGrid>

      <footer
        className={[
          "statusbar",
          primaryRunningScan ? "has-scan" : "is-idle",
          backgroundStatusText ? "has-background-work" : "",
          hoverHelp ? "has-hover-help" : ""
        ].filter(Boolean).join(" ")}
      >
        <span className="statusbar-message" role="status" aria-live="polite" aria-atomic="true">{statusText}</span>
        {primaryRunningScan && primaryRunningScanProgress ? (
          <span className="statusbar-scan" aria-live="off" data-help="Live scan progress. Code Hangar reuses previous inventory estimates when available; new roots are counted before indexing metadata.">
            <span className="statusbar-scan-track" aria-hidden="true">
              <span
                className={primaryRunningScanProgress.percent == null ? "indeterminate" : ""}
                style={primaryRunningScanProgress.percent == null ? undefined : { width: `${primaryRunningScanProgress.percent}%` }}
              />
            </span>
            <span className="statusbar-scan-copy">
              Scan {primaryRunningScanProgress.progressText ? `${primaryRunningScanProgress.progressText} · ` : ""}
              {primaryRunningScanProgress.countText} · {primaryRunningScanProgress.rateText}
              {primaryRunningScan.workerCount ? ` · ${primaryRunningScan.workerCount} thread${primaryRunningScan.workerCount === 1 ? "" : "s"}` : ""}
              {" · "}{primaryRunningScanProgress.bottleneckText}
              {" · "}{primaryRunningScanProgress.timeText}
            </span>
          </span>
        ) : null}
        <span className="statusbar-action-slot">
          {appRemovalUndo ? (
            <button type="button" onClick={() => void undoAppRemoval()} data-help={`Restore ${appRemovalUndo.name} to its AI apps from the backup just made.`}>
              Undo remove
            </button>
          ) : null}
          {primaryRunningScan ? (
            <button type="button" onClick={() => void cancelScan(primaryRunningScan.jobId)} disabled={primaryRunningScan.state === "cancelling"} data-help="Cancel this scan at the next safe checkpoint. Partial inventory remains marked incomplete.">
              {primaryRunningScan.state === "cancelling" ? "Stopping" : "Stop"}
            </button>
          ) : null}
        </span>
        <span className="background-work">{backgroundStatusText ?? ""}</span>
        <span className="hover-help">{hoverHelp ?? ""}</span>
      </footer>

      {quickOpenVisible ? (
        <QuickOpenDialog
          query={quickQuery}
          results={visibleQuickResults}
          starterResults={quickOpenStarterResults}
          projects={displayedProjects}
          searchStatus={quickSearchStatus}
          returnFocus={quickOpenReturnFocusRef.current}
          onQuery={setQuickQuery}
          onClose={() => setQuickOpenVisible(false)}
          onOpen={(result) => {
            setQuickOpenVisible(false);
            if (result.itemKind === "project") {
              selectProject(result.projectId);
              return;
            }
            void openNode(result.nodeId, { projectId: result.projectId });
          }}
        />
      ) : null}

      {contextMenu ? <ContextMenu menu={contextMenu} onClose={() => setContextMenu(null)} /> : null}

      {commandVisible ? (
        <CommandDialog
          selectedProjectName={selectedProject?.name ?? null}
          returnFocus={commandReturnFocusRef.current}
          onClose={() => setCommandVisible(false)}
          onQuickOpen={() => {
            setCommandVisible(false);
            openQuickOpen();
          }}
          onAddProjects={() => {
            setCommandVisible(false);
            setAddProjectsVisible(true);
          }}
          onOverview={() => {
            setCommandVisible(false);
            showOverview();
          }}
          onProject={() => {
            setCommandVisible(false);
            showProjectWorkspace("context");
          }}
          onDiscover={() => {
            setCommandVisible(false);
            showDiscover("search");
          }}
          onReview={() => {
            setCommandVisible(false);
            showSafeManage();
          }}
          onRecovery={() => {
            setCommandVisible(false);
            showRecovery();
          }}
          onSettings={() => {
            setCommandVisible(false);
            showSettings("folders");
          }}
        />
      ) : null}

      {shellOpenChoice ? (
        <Suspense fallback={null}>
          <ShellOpenModeDialog
            inspection={shellOpenChoice}
            onChoose={finishShellOpenChoice}
          />
        </Suspense>
      ) : null}

      {shellIntegration?.defaultGuidePending && !tourActive && !addProjectsVisible && !shellOpenChoice ? (
        <ShellDefaultGuideDialog
          busy={shellIntegrationBusy}
          error={shellIntegrationError}
          onOpenSettings={() => void openWindowsDefaultApps()}
          onDismiss={() => void dismissShellDefaultGuide()}
        />
      ) : null}

      {shellViewer ? (
        <div className="investigation-banner viewer-mode-banner" role="status">
          <Eye size={18} aria-hidden="true" />
          <div className="investigation-banner-body">
            <strong>Viewer mode: {shellViewer.project.name}</strong>
            <span className="muted">Only {shellViewer.project.path} is open. It is not part of the project catalog and AI-app/session discovery is paused for this view.</span>
          </div>
          <button
            type="button"
            className="secondary-button"
            onClick={() => void closeShellViewerSession(shellViewer, true)}
            disabled={!shellViewer.ready || shellViewerClosing}
          >
            {shellViewerClosing ? "Closing…" : shellViewer.ready ? "Exit Viewer" : "Preparing Viewer…"}
          </button>
        </div>
      ) : null}

      {investigation ? (
        <div className="investigation-banner" role="status">
          <div className="investigation-banner-body">
            <strong>Investigating: {investigation.path}</strong>
            <span className="muted">
              {investigation.isOrphan
                ? "Orphan folder — no registered project owns it."
                : `Related to ${investigation.owners.length} registered project${investigation.owners.length === 1 ? "" : "s"}: ${investigation.owners.map((owner) => `${owner.name} (${owner.relation})`).join(", ")}.`}
              {" "}{investigation.fileCount} file{investigation.fileCount === 1 ? "" : "s"} · {formatBytes(investigation.totalBytes)}{investigation.hasGit ? " · git repo" : ""}{investigation.explanation ? ` · ${investigation.explanation.classification}` : ""}.
            </span>
            <span className="muted">Not added to your projects. Review the local evidence below, then discard the investigation when you are done.</span>
          </div>
          <button type="button" className="secondary-button" onClick={() => void discardCurrentInvestigation()} disabled={investigationBusy}>
            Discard investigation
          </button>
        </div>
      ) : null}

      {editionExtensionActive ? (
        <Suspense fallback={null}>
          <EditionLayer
            selectedProjectId={selectedProjectId}
            changesUnlocked={changesUnlocked}
            onRequestChangeAccess={requestChangeAccess}
            onStatus={setStatusText}
            onRefreshNode={refreshEditionNode}
            confirm={requestConfirm}
            onBridge={setEditionBridge}
            onOverlayChange={setEditionOverlayOpen}
          />
        </Suspense>
      ) : null}

      {addProjectsVisible ? (
        <AddProjectsDialog
          onClose={() => setAddProjectsVisible(false)}
          onDeepScan={() => {
            if (deepScanProgress && deepScanProgress.phase !== "done") {
              setAddProjectsVisible(false);
              setDeepScanOverlayVisible(true);
              setStatusText("Deep Scan progress opened.");
            } else {
              void runGlobalDeepScan();
            }
          }}
          onSearchFolder={() => {
            void chooseDeepDiscoveryRoot();
          }}
          onInvestigate={() => {
            void runInvestigate();
          }}
          deepScanRunning={Boolean(deepScanProgress && deepScanProgress.phase !== "done")}
          actionsBusy={projectDiscoveryLoading || investigationBusy}
          includeLoose={deepScanIncludeLoose}
          onToggleLoose={setDeepScanIncludeLoose}
          includeAgents={deepScanIncludeAgents}
          onToggleAgents={setDeepScanIncludeAgents}
          installedApps={installedApps}
          installedAppsLoading={installedAppsLoading}
          installedAppsError={installedAppsError}
          wslScan={wslScanChoice}
          wslPreferencePending={wslPreferencePending}
          wslPreferenceError={wslPreferenceError}
          onToggleWsl={updateWslScanPreference}
        />
      ) : null}

      {tourActive ? connectorFrontendBuild ? (
        <Suspense fallback={null}>
          <ConnectorGuidedTour
            mode={tourMode ?? "first-run"}
            hasRealProjects={tourHasRealProjects}
            selectExample={selectTourExample}
            onFinish={finishTour}
            onSkip={skipTour}
          />
        </Suspense>
      ) : (
        <GuidedTour steps={tourSteps} mode={tourMode ?? "first-run"} productName="Code Hangar" onFinish={finishTour} onSkip={skipTour} />
      ) : null}

      {deepScanProgress && deepScanOverlayVisible ? (
        <DeepScanProgressOverlay
          progress={deepScanProgress}
          scanStatus={buildScanStatus}
          buildProjects={buildProjects}
          onHide={() => {
            setDeepScanOverlayVisible(false);
            if (deepScanProgress.phase === "done") {
              setDeepScanProgress(null);
            } else {
              setStatusText(
                deepScanProgress.phase === "building"
                  ? "Inventory indexing continues in the background."
                  : "Deep Scan continues in the background."
              );
            }
          }}
          onRetry={() => void recoverDeepScan()}
          onStop={() => {
            if (deepScanProgress.scanJobId) void cancelScan(deepScanProgress.scanJobId);
          }}
        />
      ) : null}

      {resetAllVisible ? (
        <ResetAllDialog
          projectCount={projects.filter((project) => !isDemoProject(project)).length}
          rootCount={roots.length}
          editionConsequence={editionBridgeRef.current?.resetConsequence()}
          onCancel={() => setResetAllVisible(false)}
          onConfirm={() => {
            setResetAllVisible(false);
            void resetAllProjects();
          }}
        />
      ) : null}

      {removeProjectTarget ? (
        <RemoveProjectDialog
          project={removeProjectTarget}
          hasApp={(removeProjectTarget.apps?.length ?? 0) > 0 || !!removeProjectTarget.app}
          onCancel={() => setRemoveProjectTarget(null)}
          onConfirm={(opts) => void confirmRemoveProject(opts)}
        />
      ) : null}

      {changeUnlockTarget ? (
        <ChangeAccessDialog
          projectName={changeUnlockTarget.name}
          onCancel={() => setChangeUnlockTarget(null)}
          onUnlock={() => {
            setUnlockedChangeProjectId(changeUnlockTarget.id);
            setChangeUnlockTarget(null);
            setStatusText(`File changes unlocked for ${changeUnlockTarget.name}. Every apply still requires a separate review.`);
          }}
        />
      ) : null}

      {finalRemoveReview ? (
        <Suspense
          fallback={(
            <div className="dialog-backdrop" role="status" aria-live="polite">
              <div className="command-dialog final-remove-review-dialog"><p className="muted">Opening final-cleanup review…</p></div>
            </div>
          )}
        >
          <FinalRemoveReviewDialog
            preview={finalRemoveReview.preview}
            scope={finalRemoveReview.scope}
            busy={mutationBusy}
            canStop={Boolean(finalRemoveJobId && finalRemoveBatchId)}
            progress={finalRemoveProgress}
            result={finalRemoveResult}
            error={finalRemoveError}
            onCancel={closeFinalRemoveReview}
            onConfirm={runFinalRemoveBatch}
            onStop={stopFinalRemoveBatch}
          />
        </Suspense>
      ) : null}

      {confirmRequest ? (
        <ConfirmActionDialog
          message={confirmRequest.message}
          confirmLabel={confirmRequest.confirmLabel}
          tone={confirmRequest.tone}
          onCancel={() => resolveConfirm(false)}
          onConfirm={() => resolveConfirm(true)}
        />
      ) : null}

      {recoveryState?.pending && !recoveryFrozen ? (
        <RecoveryRequiredDialog
          state={recoveryState}
          resolving={recoveryResolving}
          onRollback={() => void resolveRecovery("rollback")}
          onFreeze={freezeRecovery}
        />
      ) : null}
    </div>
  );
}

function isHermesSessionKind(kind: string) {
  const lower = kind.toLocaleLowerCase();
  return lower.includes("hermes") || lower.includes("nemoclaw");
}

interface DeepScanStage {
  label: string;
}
interface DeepScanProgress {
  stages: DeepScanStage[];
  phase: DeepScanPhase;
  outcome?: DeepScanOutcome | null;
  projectsFound: number;
  sessionsFound: number;
  addedCount: number;
  note: string;
  // During "building", the inventory scan job whose live progress the panel shows.
  scanJobId?: string | null;
  // Roots retained for a deliberate retry/resume when the inventory did not complete.
  retryRootIds?: number[];
}

interface BuildProject {
  id: number;
  name: string;
  state: DeepScanBuildProjectState;
  done: boolean;
  current: boolean;
}

function initialDeepScanStages(installedApps: InstalledApp[], includeWsl: boolean): DeepScanStage[] {
  return deepScanSourceLabels(installedApps, includeWsl).map((label) => ({ label }));
}

// A full-screen Deep Scan progress panel. Discovery exposes no per-source
// completion events, so sources remain an honest static "included" list while
// the aggregate bar is indeterminate. Inventory building then uses measured
// backend progress when available.
function DeepScanProgressOverlay({
  progress,
  scanStatus,
  buildProjects,
  onHide,
  onRetry,
  onStop
}: {
  progress: DeepScanProgress;
  scanStatus: ScanStatus | null;
  buildProjects: BuildProject[];
  onHide: () => void;
  onRetry: () => void;
  onStop: () => void;
}) {
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onHide);
  const presentation = deepScanTerminalPresentation(progress.phase, progress.outcome);
  const finished = presentation.terminal;
  const inventoryReady = presentation.inventoryReady;
  const building = progress.phase === "building" || (finished && progress.scanJobId != null);
  // One unified panel for the whole Deep Scan: the per-app checklist and the
  // running totals stay visible the whole time, and once the inventory scan
  // starts its live readout (items/GiB/rate/threads) and per-project ticker fill
  // in below — so it reads as a single pleasing menu instead of two that swap.
  const parts = building && scanStatus ? scanProgressParts(scanStatus) : null;
  const terminalPercent = finished && scanStatus
    ? deepScanTerminalPercent(progress.outcome, scanStatus.scannedFiles, scanStatus.estimatedTotalFiles)
    : null;
  const indeterminate = !finished && deepScanUsesIndeterminateProgress(progress.phase, parts?.percent);
  const percent = finished
    ? terminalPercent
    : building
      ? parts?.percent ?? 0
      : 0;
  const showProgressBar = !finished || percent != null;
  const processedCount = buildProjects.filter((project) => project.state === "processed" || project.state === "indexed").length;
  const title = presentation.title
    ?? (building
      ? "Building your inventory"
      : progress.phase === "registering"
        ? "Adding confirmed projects"
        : "Mapping your AI projects");
  const projectStateLabels: Record<DeepScanBuildProjectState, string> = {
    queued: "queued",
    indexing: "indexing…",
    processed: "processed",
    indexed: "indexed",
    partial: "incomplete",
    stopped: "stopped",
    failed: "failed"
  };
  const footText = building
    ? inventoryReady
      ? buildProjects.length > 0
        ? `${buildProjects.length} of ${buildProjects.length} ready`
        : "Inventory complete."
      : finished
        ? progress.outcome === "partial"
          ? "Partial local inventory retained. Resume to finish it."
          : progress.outcome === "cancelled"
            ? "Stopped safely. Partial local inventory was retained."
            : progress.outcome === "failed"
              ? "Inventory is not complete. Retry when ready."
              : "No completed inventory is available."
        : buildProjects.length > 0
          ? `${processedCount} of ${buildProjects.length} processed so far`
          : "Inventory scan in progress."
    : finished
      ? progress.outcome === "mapped"
        ? "Mapping complete. Review candidates before adding them."
        : progress.outcome === "inventory-not-started"
          ? "No inventory job started. The registered projects remain available for retry."
          : progress.outcome === "failed"
            ? "No inventory completion was recorded."
            : "Deep Scan finished."
      : progress.phase === "registering"
        ? "Sources checked. Adding strong matches…"
        : "Checking all included sources…";
  return (
    <div className="dialog-backdrop deep-scan-backdrop" role="presentation">
      <div
        ref={dialogRef}
        className="deep-scan-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="deep-scan-progress-title"
        onKeyDown={onDialogKeyDown}
      >
        <header className="deep-scan-head">
          <div className="deep-scan-spinner" data-phase={progress.phase} data-outcome={progress.outcome ?? undefined} aria-hidden="true">
            {inventoryReady ? <CheckCircle2 size={28} /> : finished ? <AlertTriangle size={28} /> : <Radar size={28} />}
          </div>
          <div className="deep-scan-head-copy">
            <strong id="deep-scan-progress-title">{title}</strong>
            <span className="muted" aria-live="polite">{progress.note}</span>
          </div>
          <button
            data-dialog-initial-focus
            type="button"
            className="icon-button deep-scan-close"
            onClick={onHide}
            aria-label={finished ? "Close Deep Scan progress" : "Hide Deep Scan progress"}
            data-help={finished ? "Close this progress summary." : "Hide this panel while the scan continues in the background."}
          >
            <X size={16} />
          </button>
        </header>
        {showProgressBar ? (
          <div className="deep-scan-bar" aria-hidden="true">
            <div
              className={`deep-scan-bar-fill ${indeterminate ? "indeterminate" : ""}`}
              style={indeterminate ? undefined : { width: `${percent ?? 0}%` }}
            />
          </div>
        ) : null}
        {building ? (
          <>
            <div className="deep-scan-build-readout">
              <strong>{presentation.readoutLabel ?? parts?.progressText ?? "Scanning…"}</strong>
              <span className="muted">{parts?.countText ?? "Preparing the inventory…"}</span>
            </div>
            <div className="deep-scan-build-stats">
              {parts?.rateText ? <span>{parts.rateText}</span> : null}
              {parts?.timeText ? <span>{parts.timeText}</span> : null}
              {scanStatus?.workerCount ? <span>{scanStatus.workerCount} threads</span> : null}
            </div>
          </>
        ) : null}
        <ul className="deep-scan-stages">
          {progress.stages.map((stage) => (
            <li key={stage.label} data-done="false">
              <span className="deep-scan-stage-icon" aria-hidden="true">
                <span className="deep-scan-dot" />
              </span>
              <span className="deep-scan-stage-label">{stage.label}</span>
              <span className="deep-scan-stage-state">included</span>
            </li>
          ))}
        </ul>
        <div className="deep-scan-totals">
          <div>
            <strong>{progress.projectsFound}</strong>
            <span className="muted">project{progress.projectsFound === 1 ? "" : "s"} found</span>
          </div>
          <div>
            <strong>{progress.addedCount}</strong>
            <span className="muted">added automatically</span>
          </div>
          <div>
            <strong>{progress.sessionsFound}</strong>
            <span className="muted">session{progress.sessionsFound === 1 ? "" : "s"}</span>
          </div>
        </div>
        {building && buildProjects.length > 0 ? (
          <ul className="deep-scan-build-projects">
            {buildProjects.map((project) => (
              <li
                key={project.id}
                data-done={project.done ? "true" : "false"}
                data-current={project.current ? "true" : "false"}
                data-state={project.state}
              >
                <span className="deep-scan-stage-icon" aria-hidden="true">
                  {project.done ? (
                    <CheckCircle2 size={14} />
                  ) : project.current ? (
                    <Loader2 size={14} className="spin" />
                  ) : project.state === "failed" ? (
                    <AlertTriangle size={14} />
                  ) : (
                    <span className="deep-scan-dot" />
                  )}
                </span>
                <span className="deep-scan-stage-label">{project.name}</span>
                <span className="deep-scan-stage-state">
                  {projectStateLabels[project.state]}
                </span>
              </li>
            ))}
          </ul>
        ) : null}
        <div className="deep-scan-build-foot">
          <span className="muted">{footText}</span>
          <div className="deep-scan-build-actions">
            <button type="button" className="deep-scan-ghost" onClick={onHide} data-help={finished ? "Close this progress summary." : "Keep the scan running in the background and return to the app. Open Add Projects to show progress again."}>
              {finished ? "Close" : "Hide and keep working"}
            </button>
            {finished && presentation.actionLabel ? (
              <button
                type="button"
                className="deep-scan-ghost"
                onClick={onRetry}
                data-help={presentation.action === "resume"
                  ? "Continue by starting a new local scan over the same roots. The retained partial inventory stays available until replacement data is finalized."
                  : progress.retryRootIds?.length
                    ? "Retry only the unfinished local inventory scan; registered projects remain registered."
                    : "Run the same local project discovery again. No remote source is contacted."}
              >
                {presentation.actionLabel}
              </button>
            ) : null}
            {building && !finished && progress.scanJobId ? (
              <button type="button" className="deep-scan-ghost danger" onClick={onStop} data-help="Stop the inventory scan at the next safe checkpoint. Partial inventory is kept.">
                Stop indexing
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

const SidebarSessionGroup = memo(function SidebarSessionGroup({
  title,
  kind,
  sessions,
  defaultCollapsed,
  previewLimit,
  openSession,
  showSessionMenu,
  projectId,
  onOpenProject,
  forceExpanded
}: {
  title: string;
  kind: "project" | "independent" | "hermes";
  sessions: SessionDiscoveryCandidate[];
  defaultCollapsed?: boolean;
  previewLimit?: number;
  openSession: (session: SessionDiscoveryCandidate) => void;
  showSessionMenu: (session: SessionDiscoveryCandidate, event: MouseEvent<HTMLElement>) => void;
  /** For project groups: jump straight to the project this group belongs to. */
  projectId?: number;
  onOpenProject?: (projectId: number) => void;
  forceExpanded?: boolean;
}) {
  // Project groups start collapsed. Independent opts out so its recent sessions stay visible.
  const [collapsed, setCollapsed] = useState(defaultCollapsed ?? true);
  const [showAll, setShowAll] = useState(false);
  if (sessions.length === 0) return null;
  const expanded = Boolean(forceExpanded) || !collapsed;
  const preview = previewSidebarSessionItems(sessions, {
    searchActive: Boolean(forceExpanded),
    showAll,
    itemLimit: previewLimit
  });
  return (
    <div className={`session-sidebar-group ${kind}`}>
      <div className="session-sidebar-group-head">
      <button
        type="button"
        className="session-sidebar-group-header"
        aria-expanded={expanded}
        onClick={() => setCollapsed((value) => !value)}
        data-help={
          kind === "project"
            ? `Local AI sessions linked to ${title}.`
            : kind === "hermes"
              ? "Hermes sessions are high-volume, so they are kept separate and collapsed by default."
              : "Local sessions with no linked project."
        }
      >
        {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span>{title}</span>
        <small>{sessions.length}</small>
      </button>
      {projectId != null && onOpenProject ? (
        <button
          type="button"
          className="session-group-open-project"
          onClick={() => onOpenProject(projectId)}
          aria-label={`Open project ${title}`}
          data-help={`Open the ${title} project workspace.`}
        >
          <FolderOpen size={13} />
        </button>
      ) : null}
      </div>
      {expanded ? (
        <div className="context-list compact">
          {preview.visibleSessions.map((session) => (
            <button
              className="context-row"
              key={`${session.sourceKind}-${session.path}`}
              type="button"
              data-help={`${displayAppText(session.sessionKind)} session. ${sessionAssociationHelpText(session)} Click to open it read-only in the workspace, with secrets masked and details on the right.`}
              onClick={() => openSession(session)}
              onContextMenu={(event) => showSessionMenu(session, event)}
            >
              <span className="row-main">
                <span className="row-title">
                  <strong>{session.displayName}</strong>
                  <AppBadge meta={sessionAppMeta(session)} suffix=" session" />
                </span>
                <small>{sessionAssociationLabelText(session.association)}</small>
              </span>
            </button>
          ))}
          {preview.canToggle ? (
            <button
              type="button"
              className="session-group-more"
              onClick={() => setShowAll((value) => !value)}
              data-help={forceExpanded ? "Keep search matches compact, or show every matching session in this group." : "Show every session in this group, or collapse back to the most recent ones."}
            >
              {showAll
                ? (forceExpanded ? "Show fewer matches" : "Show fewer")
                : (forceExpanded ? `Show all matches (${sessions.length})` : `Show all sessions (${sessions.length})`)}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
});

function sessionAssociationLabelText(association: string) {
  if (association === "registered_project") return "linked to project";
  if (association === "unregistered_project_reference") return "project not added yet";
  if (association === "loose_session") return "loose session";
  return "session";
}

function sessionAssociationHelpText(session: SessionDiscoveryCandidate) {
  if (session.association === "registered_project") {
    return "Code Hangar found a project path in this session that matches a registered project.";
  }
  if (session.association === "unregistered_project_reference") {
    return "This session mentions a local project folder that has not been added to Projects.";
  }
  if (session.association === "loose_session") {
    return "This session was found locally, but no project path was linked from its metadata.";
  }
  return `${displayAppText(session.sourceLabel)} local session metadata discovered on this machine.`;
}

function workspaceEyebrow(view: PrimaryView) {
  switch (view) {
    case "safe_manage":
    case "review":
      return "Safe Manage";
    case "settings":
      return "Local preferences";
    case "recovery":
      return "Recovery & cleanup";
    default:
      return "Local inventory";
  }
}

function workspaceTitle(view: PrimaryView, discoverView: DiscoverView, settingsView: SettingsView) {
  if (view === "overview") return "Overview";
  if (view === "safe_manage") return "Safe Manage";
  if (view === "review") return "Safe Manage";
  if (view === "recovery") return "Recovery & cleanup";
  if (view === "discover") {
    if (discoverView === "projects") return "Find local projects & sessions";
    if (discoverView === "lost") return "Forgotten projects";
    if (discoverView === "assets") return "Unreferenced files";
    if (discoverView === "duplicates") return "Duplicate files";
    if (discoverView === "organize") return "Organize";
    return "Document search";
  }
  if (settingsView === "protection") return "Protected locations";
  if (settingsView === "appearance") return "Appearance";
  if (settingsView === "advanced") return "System & diagnostics";
  return "Scan folders";
}

function workspaceHelpConcept(
  view: PrimaryView,
  discoverView: DiscoverView,
  settingsView: SettingsView
): BeginnerHelpConcept | null {
  if (view === "overview") return "inventory";
  if (view === "safe_manage" || view === "review") return "safeManage";
  if (view === "recovery") return "recover";
  if (view === "discover") {
    if (discoverView === "projects") return "sessions";
    if (discoverView === "lost" || discoverView === "assets") return "unreferenced";
    if (discoverView === "duplicates") return "duplicates";
    if (discoverView === "organize") return "inventory";
    return "context";
  }
  if (view === "settings") {
    if (settingsView === "protection") return "protected";
    if (settingsView === "folders") return "scan";
    if (settingsView === "advanced") return "inventory";
  }
  return null;
}

function WorkspaceConceptHelp({ view, discoverView, settingsView }: {
  view: PrimaryView;
  discoverView: DiscoverView;
  settingsView: SettingsView;
}) {
  const concept = workspaceHelpConcept(view, discoverView, settingsView);
  return concept ? <ConceptHelp concept={concept} /> : null;
}

function workspaceDescription(view: PrimaryView, discoverView: DiscoverView, settingsView: SettingsView) {
  if (view === "overview") return "See what Code Hangar knows, what is still scanning, and where local disk space is concentrated.";
  if (view === "safe_manage") return "Analyze every known project, compare objective local evidence, and make explicit decisions before any OperationPlan or Risk Report is prepared.";
  if (view === "review") return "Review the selected target's exact OperationPlan, Risk Report, protection and recovery gates before any disk action can continue.";
  if (view === "recovery") return "Review held projects, object-archive eligibility, recovery archives and interrupted local actions recorded for this profile.";
  if (view === "discover") {
    if (discoverView === "projects") return "Find project folders and local AI sessions from known local locations and app metadata, then choose what to add or inspect.";
    if (discoverView === "lost") return "Use passive local signals to rediscover projects or folders you may have lost track of. Results are review signals, never delete recommendations.";
    if (discoverView === "assets") return "Find files with no known local references. Results need human review and are never delete recommendations.";
    if (discoverView === "duplicates") return "Compare local files using bounded checks. Candidate groups are not removed or treated as confirmed cleanup.";
    if (discoverView === "organize") return "See where your models and projects are scattered across disk, grouped by location, with duplicates and idle projects flagged. Read-only.";
    return "Search the content of locally indexed, non-sensitive documents. Large searches run only when you press Search.";
  }
  if (settingsView === "protection") return "Understand which local paths receive stronger preview and indexing protection.";
  if (settingsView === "appearance") return "Adjust text size, density, navigation and motion for this local interface.";
  if (settingsView === "advanced") return "Inspect this build, local resource plans and safety state. These controls never relax protection.";
  return "Choose which local folders Code Hangar inventories. These controls never delete files from disk.";
}

function asPerformanceMode(mode: string): PerformanceMode | null {
  if (mode === "balanced" || mode === "priority" || mode === "max") return mode;
  return null;
}

function performanceStatusText(mode: PerformanceMode) {
  switch (mode) {
    case "max":
      return "Max CPU selected. New heavy tasks can use all logical CPU threads; process priority rises only while the task runs.";
    case "priority":
      return "Priority selected. New heavy tasks use about three quarters of local CPU threads and return to normal priority when finished.";
    default:
      return "Background mode enabled. Heavy tasks use normal priority and conservative workers.";
  }
}

function performancePlanHelp(label: string, cpuThreads: number, processPriority: string, batchSize: number) {
  return `${label}: newly started scans use ${cpuThreads} local metadata worker${cpuThreads === 1 ? "" : "s"}, batch size ${batchSize.toLocaleString()}, and ${processPriority} process priority.`;
}

function performanceHelpText(mode: PerformanceMode) {
  switch (mode) {
    case "max":
      return "Max CPU: newly started heavy tasks use all available logical CPU threads and above-normal priority only while they run. Idle Code Hangar stays at normal priority.";
    case "priority":
      return "Priority: newly started heavy tasks use larger batches, about three quarters of local CPU threads and above-normal priority only while they run.";
    default:
      return "Background: heavy local tasks use normal priority and conservative scan workers. Use it when you want Code Hangar to stay quiet while you work.";
  }
}

function RecoveryRequiredDialog({
  state,
  resolving,
  onRollback,
  onFreeze
}: {
  state: RecoveryPending;
  resolving: "rollback" | null;
  onRollback: () => void;
  onFreeze: () => void;
}) {
  const totals = state.operations.reduce(
    (acc, operation) => ({
      done: acc.done + operation.doneItems,
      pending: acc.pending + operation.pendingItems,
      failed: acc.failed + operation.failedItems,
      total: acc.total + operation.totalItems
    }),
    { done: 0, pending: 0, failed: 0, total: 0 }
  );
  return (
    <div className="dialog-backdrop recovery-backdrop" role="presentation">
      <div className="recovery-dialog" role="dialog" aria-modal="true" aria-labelledby="recovery-required-title">
        <div className="recovery-dialog-heading">
          <AlertTriangle size={20} />
          <div>
            <h2 id="recovery-required-title">Recovery required</h2>
            <p>An earlier disk operation was interrupted. Code Hangar found journal entries and needs your decision before any future disk action continues.</p>
          </div>
        </div>
        <div className="recovery-summary-grid">
          <div>
            <span>Operations</span>
            <strong>{state.operations.length}</strong>
          </div>
          <div>
            <span>Total items</span>
            <strong>{totals.total}</strong>
          </div>
          <div>
            <span>Done items</span>
            <strong>{totals.done}</strong>
          </div>
          <div>
            <span>Pending items</span>
            <strong>{totals.pending}</strong>
          </div>
          <div>
            <span>Failed items</span>
            <strong>{totals.failed}</strong>
          </div>
        </div>
        <div className="recovery-operation-list">
          {state.operations.map((operation) => (
            <div className="recovery-operation-row" key={operation.id}>
              <div>
                <strong>{operation.kind}</strong>
                <span>{operation.status} · operation #{operation.id}</span>
              </div>
              <small>{operation.doneItems}/{operation.totalItems} done{operation.targetNodeId != null ? ` · node ${operation.targetNodeId}` : ""}</small>
            </div>
          ))}
        </div>
        <div className="recovery-choice-grid">
          <button type="button" className="action-button" disabled={Boolean(resolving)} onClick={onRollback} data-help="Reverse completed journaled moves where possible. Code Hangar never overwrites occupied original paths. This is rollback, never a disguised resume.">
            {resolving === "rollback" ? "Rolling back..." : "Roll back safely"}
          </button>
          <button type="button" className="secondary-button" disabled={Boolean(resolving)} onClick={onFreeze} data-help="Do not touch files now. The pending journal remains and this prompt will return next launch.">
            Freeze for now
          </button>
        </div>
        <p className="recovery-footnote">
          Interrupted disk work is never resumed automatically. Freeze is a pause, not a fix: read-only navigation remains available, but later backup, move, restore and cleanup workflows stay blocked until a safe rollback succeeds.
        </p>
      </div>
    </div>
  );
}

function ResourceMeter() {
  const [usage, setUsage] = useState<ProcessResourceUsage | null>(null);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (!active) return;
      if (typeof document !== "undefined" && document.hidden) {
        timer = setTimeout(() => void tick(), 30_000);
        return;
      }
      try {
        const next = await api.processResourceUsage();
        if (active) setUsage(next);
      } catch {
        // Best-effort live meter; ignore transient sampling errors.
      }
      if (active) timer = setTimeout(() => void tick(), 10_000);
    };
    void tick();
    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, []);

  if (!usage || !usage.sampled) return null;
  const cpu = Math.round(usage.cpuPercent);
  const ram = usage.memoryWorkingSetBytes != null ? formatBytes(usage.memoryWorkingSetBytes) : "—";
  const systemUsed =
    usage.totalMemoryBytes != null && usage.availableMemoryBytes != null
      ? `${formatBytes(usage.totalMemoryBytes - usage.availableMemoryBytes)} of ${formatBytes(usage.totalMemoryBytes)} system RAM in use`
      : null;
  const help =
    [
      `Code Hangar is using about ${cpu}% of total CPU capacity across ${usage.logicalCpuCount} logical threads`,
      usage.memoryWorkingSetBytes != null ? `working-set RAM ${formatBytes(usage.memoryWorkingSetBytes)}` : null,
      usage.memoryPrivateBytes != null ? `private RAM ${formatBytes(usage.memoryPrivateBytes)}` : null,
      systemUsed,
      usage.gpuSummary
    ]
      .filter(Boolean)
      .join(". ") + ". Sampled locally about every 10 seconds while the window is visible.";
  return (
    <div className="resource-meter" data-help={help} aria-label={`Resource usage: CPU ${cpu} percent, RAM ${ram}`}>
      <Activity size={14} aria-hidden="true" />
      <span className="resource-meter-metric"><b>{cpu}%</b> CPU</span>
      <span className="resource-meter-sep" aria-hidden="true">·</span>
      <span className="resource-meter-metric"><b>{ram}</b> RAM</span>
    </div>
  );
}

// Memoized sidebar project row. Extracted so it re-renders only when its own
// props change — not on every App re-render (session select, theme toggle,
// status-text ticks, tab switches). The parent still owns the archived divider
// and the render/skip decision; this component is just the row button. All props
// are primitives or references that stay stable while the project list is
// unchanged (selectProject/showProjectMenu keep the same identity unless the
// selected project, plan job or workspace load-status changes).
const ProjectRow = memo(function ProjectRow({
  project,
  state,
  watchBadge,
  watchReason,
  isSelected,
  keepSelectedArchivedVisible,
  showPath,
  sidebarPath,
  onSelect,
  onContextMenu
}: {
  project: ProjectSummary;
  state: ProjectScanState;
  watchBadge: string | null;
  watchReason: string;
  isSelected: boolean;
  keepSelectedArchivedVisible: boolean;
  showPath: boolean;
  sidebarPath: string;
  onSelect: (projectId: number) => void;
  onContextMenu?: (project: ProjectSummary, event: MouseEvent<HTMLElement>) => void;
}) {
  return (
    <button
      className={`project-row project-${state} ${isSelected ? "selected" : ""} ${keepSelectedArchivedVisible ? "selected-archived" : ""}`}
      type="button"
      data-help={`${keepSelectedArchivedVisible ? "Current project kept visible while Archived is collapsed. " : ""}Open project ${project.name}. ${projectStateHelp(state)} ${watchReason}${onContextMenu ? " Right-click for project actions." : " Viewer mode is read-only."}`}
      onClick={() => onSelect(project.id)}
      onContextMenu={onContextMenu ? (event) => onContextMenu(project, event) : undefined}
    >
      <span className="row-icon"><Folder size={16} /></span>
      <span className="row-main">
        <span className="row-title">
          <strong>{project.name}</strong>
        </span>
        <AppBadges metas={projectAppMetas(project)} suffix=" project" />
        <small>{projectStateSummary(state === "outdated" && watchBadge ? "scanned" : state, project.contextCount)}</small>
        {project.antigravityName ? <small className="project-aka" title={`Named "${project.antigravityName}" in the Antigravity (Gemini) IDE — the folder is ${project.name}.`}>named <strong>{project.antigravityName}</strong> in Antigravity</small> : null}
        {showPath ? <small className="project-path" title={sidebarPath}>{compactLocalPath(sidebarPath)}</small> : null}
      </span>
      <span className="project-status-slot">
        {isDemoProject(project) ? <span className="project-status-pill demo">Demo</span> : null}
        {/* Exactly one status pill, by priority: an active scan
            wins (so a scanning project never also shows "Needs
            scan"), then the live watcher state, then a partial
            inventory. "Scanned" shows nothing. */}
        {state === "scanning" ? (
          <span className="project-status-pill scanning">{projectStateLabel("scanning")}</span>
        ) : watchBadge ? (
          <span className={`project-status-pill watch-${watchBadge}`}>{projectWatchLabel(watchBadge)}</span>
        ) : state === "outdated" ? (
          <span className="project-status-pill outdated">{projectStateLabel("outdated")}</span>
        ) : null}
        {project.pinned ? <Pin size={14} /> : null}
      </span>
    </button>
  );
});

function projectStateLabel(state: ProjectScanState) {
  switch (state) {
    case "scanning":
      return "Scanning";
    case "outdated":
      return "Needs scan";
    case "scanned":
    default:
      return "Scanned";
  }
}

function projectStateSummary(state: ProjectScanState, contextCount: number) {
  const contextText = `${contextCount} context ${contextCount === 1 ? "file" : "files"}`;
  switch (state) {
    case "scanning":
      return `Scanning now · at least ${contextText}`;
    case "outdated":
      return `Needs scan · at least ${contextText}`;
    case "scanned":
    default:
      return contextText;
  }
}

function projectStateHelp(state: ProjectScanState) {
  switch (state) {
    case "scanning":
      return "This root is currently being inventoried, so its file list may still be incomplete.";
    case "outdated":
      return "This root is registered but needs a scan, or the last inventory is partial and should be continued.";
    case "scanned":
    default:
      return "The latest known scan finished without a partial-inventory marker.";
  }
}

function projectOverlapWarning(project: ProjectSummary, projects: ProjectSummary[]) {
  if (isDemoProject(project)) return null;
  const projectPath = normalizeProjectRootPath(project.path);
  if (!projectPath) return null;
  const parents: ProjectSummary[] = [];
  const children: ProjectSummary[] = [];
  for (const candidate of projects) {
    if (candidate.id === project.id || isDemoProject(candidate)) continue;
    const candidatePath = normalizeProjectRootPath(candidate.path);
    if (!candidatePath || candidatePath === projectPath) continue;
    if (projectPath.startsWith(`${candidatePath}/`)) {
      parents.push(candidate);
    } else if (candidatePath.startsWith(`${projectPath}/`)) {
      children.push(candidate);
    }
  }
  parents.sort((left, right) => right.path.length - left.path.length);
  children.sort((left, right) => left.name.localeCompare(right.name));
  const parts: string[] = [];
  const nearestParent = parents[0];
  if (nearestParent) {
    parts.push(`This project is inside the broader project "${nearestParent.name}", so its files may also appear there.`);
  }
  if (children.length > 0) {
    const childNames = children.slice(0, 3).map((child) => `"${child.name}"`).join(", ");
    const extra = children.length > 3 ? ` and ${children.length - 3} more` : "";
    parts.push(`It also contains separately registered project${children.length === 1 ? "" : "s"} ${childNames}${extra}.`);
  }
  if (parts.length === 0) return null;
  return `${parts.join(" ")} Keep one root if you want a single inventory; unregistering a root only removes Code Hangar metadata and never deletes files.`;
}

function normalizeProjectRootPath(path: string) {
  return path
    .replace(/\\/g, "/")
    .replace(/^\/\/\?\/UNC\//i, "//")
    .replace(/^\/\/\?\//i, "")
    .replace(/\/+$/g, "")
    .toLocaleLowerCase();
}

function sizePresetToBytes(preset: string, customMiB: number) {
  switch (preset) {
    case "10m":
      return 10 * MIB;
    case "100m":
      return 100 * MIB;
    case "1g":
      return GIB;
    case "custom":
      return Math.max(0, customMiB) * MIB;
    default:
      return 0;
  }
}

function deriveFootprintFromRootItems(project: ProjectSummary | null, items: NavItem[]): ProjectFootprintSummary | null {
  if (!project || items.length === 0) {
    return null;
  }
  const totals = items.reduce(
    (acc, item) => {
      acc.apparent += item.aggregateApparentBytes ?? 0;
      if (item.aggregateAllocatedBytes != null) {
        acc.allocated += item.aggregateAllocatedBytes;
        acc.hasAllocated = true;
      }
      if (item.aggregatePhysicalBytes != null) {
        acc.physical += item.aggregatePhysicalBytes;
        acc.hasPhysical = true;
      }
      acc.partial = acc.partial || item.aggregateBytesPartial || !item.fullyScanned || item.scanError != null;
      return acc;
    },
    { apparent: 0, allocated: 0, physical: 0, hasAllocated: false, hasPhysical: false, partial: false }
  );

  return {
    projectId: project.id,
    name: project.name,
    path: project.path,
    apparentBytes: totals.apparent,
    allocatedBytes: totals.hasAllocated ? totals.allocated : null,
    physicalBytes: totals.hasPhysical ? totals.physical : null,
    footprintPartial: totals.partial
  };
}

function loadSavedLostPresets(): LostPreset[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(LOST_PRESETS_STORAGE_KEY) ?? "[]") as LostPreset[];
    return Array.isArray(parsed)
      ? parsed.filter((preset) => typeof preset.name === "string" && preset.name.trim()).slice(0, 12)
      : [];
  } catch {
    return [];
  }
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  const tagName = target.tagName.toLowerCase();
  return tagName === "input" || tagName === "textarea" || target.isContentEditable;
}

function SessionCenterView({
  session,
  preview,
  loading,
  loadKind,
  error,
  onLoadMore,
  onLoadFull,
  onBack,
  backLabel
}: {
  session: SessionDiscoveryCandidate;
  preview: SessionPreview | null;
  loading: boolean;
  loadKind: SessionPreviewLoadKind | null;
  error: string | null;
  onLoadMore: () => void;
  onLoadFull: () => void;
  onBack: () => void;
  /** The session's own project name, or null when it has no registered project. */
  backLabel: string | null;
}) {
  const [mode, setMode] = useState<"rendered" | "source">("rendered");
  const [transcriptPage, setTranscriptPage] = useState(0);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const readableText = preview?.renderedText ?? preview?.text ?? "";
  const turns = useMemo(() => (preview ? parseSessionTranscript(readableText) : []), [preview, readableText]);
  const displayTurns = useMemo(() => compactSessionToolActivity(turns), [turns]);
  const metadata = useMemo(() => (preview ? parseSessionMetadata(preview.text) : null), [preview]);
  const transcriptPageCount = sessionTranscriptPageCount(displayTurns.length);
  const safeTranscriptPage = clampSessionTranscriptPage(transcriptPage, displayTurns.length);
  const pagedTurns = useMemo(
    () => sessionTranscriptPageSlice(displayTurns, safeTranscriptPage),
    [displayTurns, safeTranscriptPage]
  );
  const transcriptIsPaged = displayTurns.length > SESSION_TRANSCRIPT_PAGE_SIZE;
  const transcriptRangeStart = safeTranscriptPage * SESSION_TRANSCRIPT_PAGE_SIZE + 1;
  const transcriptRangeEnd = Math.min(displayTurns.length, transcriptRangeStart + pagedTurns.length - 1);

  useEffect(() => {
    setTranscriptPage(initialSessionTranscriptPage(displayTurns.length, Boolean(preview?.truncated)));
  }, [displayTurns.length, preview?.previewLimitBytes, preview?.truncated, session.path]);

  useEffect(() => {
    transcriptRef.current?.scrollTo({ top: 0 });
  }, [safeTranscriptPage, session.path]);

  const selectTranscriptPage = (nextPage: number) => {
    setTranscriptPage(clampSessionTranscriptPage(nextPage, displayTurns.length));
  };

  return (
    <div className="session-open">
      <button
        type="button"
        className="tool-back-button"
        onClick={onBack}
        data-help={backLabel ? `Return to the screen that opened this session. With no navigation history, open the ${backLabel} project workspace.` : "Return to the previous screen."}
      >
        <ArrowLeft size={15} />
        {backLabel ? `Back to ${backLabel}` : "Back"}
      </button>
      <header className="session-open-header">
        <MessageSquare size={16} />
        <div className="session-open-heading">
          <strong>{metadata?.title ?? session.displayName}</strong>
          <span
            className="session-open-breadcrumb"
            title={session.path}
            data-help="The full local transcript path and copy action are available in the Details pane."
          >
            {displayAppText(session.sessionKind)} session{session.modifiedMs != null ? ` · ${formatTimestamp(session.modifiedMs)}` : ""}
          </span>
        </div>
        {preview ? (
          <div className="session-view-toggle">
            <button className={`segmented ${mode === "rendered" ? "active" : ""}`} type="button" aria-pressed={mode === "rendered"} onClick={() => setMode("rendered")} data-help="Show the conversation in a readable, turn-by-turn layout.">
              Rendered
            </button>
            <button className={`segmented ${mode === "source" ? "active" : ""}`} type="button" aria-pressed={mode === "source"} onClick={() => setMode("source")} data-help="Show the raw session text exactly as stored on disk.">
              Source
            </button>
          </div>
        ) : null}
      </header>
      <div className="session-open-content">
        {sessionSupportsProgressiveLoading(session.association, preview) && preview ? (
          <div className="session-load-controls" role="status" aria-live="polite">
            <div className="session-load-copy">
              <strong>More conversation available</strong>
              <span>{formatBytes(preview.previewLimitBytes)} window loaded</span>
            </div>
            <div className="session-load-actions">
              <button
                className="secondary-button"
                type="button"
                disabled={loading}
                onClick={onLoadMore}
                data-help="Load the next larger cumulative window while keeping the current conversation visible."
              >
                {loading && loadKind === "more" ? <Loader2 className="spin" size={14} /> : <ChevronDown size={14} />}
                {loading && loadKind === "more" ? "Loading more..." : "Load more"}
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={loading}
                onClick={onLoadFull}
                data-help={`Read the complete ${formatBytes(preview.sizeBytes)} local session now. This is the only action that requests the full file.`}
              >
                {loading && loadKind === "full" ? <Loader2 className="spin" size={14} /> : <Layers size={14} />}
                {loading && loadKind === "full" ? "Opening full..." : "Open full session"}
              </button>
            </div>
          </div>
        ) : null}
        {loading && !preview ? (
          <div className="session-loading-state" role="status" aria-live="polite">
            <p>Loading session...</p>
            <div className="session-loading-turns" aria-hidden="true">
              {Array.from({ length: 4 }).map((_, index) => (
                <div className="session-loading-turn" key={index}>
                  <span className="skeleton skeleton-line session-loading-role" />
                  <span className="skeleton skeleton-line session-loading-copy" />
                  <span className="skeleton skeleton-line session-loading-copy short" />
                </div>
              ))}
            </div>
          </div>
        ) : null}
        {error ? <p className="scan-error">Could not open session: {error}</p> : null}
        {preview && mode === "source" ? <pre className="session-preview-text">{preview.text}</pre> : null}
        {preview && mode === "rendered" && transcriptIsPaged ? (
          <nav className="session-page-controls" aria-label="Transcript pages">
            <div className="session-page-summary">
              <strong>{!preview.truncated && preview.sourceTruncated ? "Full session" : "Loaded conversation"}</strong>
              <span>{transcriptRangeStart.toLocaleString()}–{transcriptRangeEnd.toLocaleString()} of {displayTurns.length.toLocaleString()} turns</span>
            </div>
            <div className="session-page-actions">
              <button
                type="button"
                aria-label="Previous transcript page"
                disabled={safeTranscriptPage === 0}
                onClick={() => selectTranscriptPage(safeTranscriptPage - 1)}
                data-help="Show the previous page of the loaded conversation."
              >
                <ChevronLeft size={15} />
              </button>
              <label>
                <span>Page</span>
                <input
                  type="number"
                  min={1}
                  max={transcriptPageCount}
                  value={safeTranscriptPage + 1}
                  aria-label="Transcript page"
                  onChange={(event) => selectTranscriptPage(Number(event.target.value) - 1)}
                />
                <span>of {transcriptPageCount.toLocaleString()}</span>
              </label>
              <button
                type="button"
                aria-label="Next transcript page"
                disabled={safeTranscriptPage >= transcriptPageCount - 1}
                onClick={() => selectTranscriptPage(safeTranscriptPage + 1)}
                data-help="Show the next page of the loaded conversation."
              >
                <ChevronRight size={15} />
              </button>
            </div>
          </nav>
        ) : null}
        {preview && mode === "rendered" ? (
          displayTurns.length > 0 ? (
            <div className="session-transcript" ref={transcriptRef}>
              {pagedTurns.map((turn, index) => (
                <div className={`session-turn${turn.role ? ` role-${turn.role}` : ""}`} key={safeTranscriptPage * SESSION_TRANSCRIPT_PAGE_SIZE + index}>
                  {turn.label ? <span className="session-turn-role">{turn.label}</span> : null}
                  <SessionTurnBody content={turn.content} />
                </div>
              ))}
            </div>
          ) : metadata ? (
            <SessionMetadataPanel metadata={metadata} />
          ) : (
            <div className="session-readable-empty" role="status">
              <MessageSquare size={20} />
              <strong>No readable conversation turns in this preview window</strong>
              <span>This part of the session contains only local tool output or internal bookkeeping. The original text for the loaded section remains available under Source.</span>
            </div>
          )
        ) : null}
      </div>
    </div>
  );
}

function SessionTurnBody({ content }: { content: string }) {
  const html = useMemo(() => renderMarkdownSafe(content, { localLinks: "inert" }).html, [content]);
  return <div className="session-turn-body markdown-preview session-turn-markdown" dangerouslySetInnerHTML={{ __html: html }} />;
}

function SessionMetadataPanel({ metadata }: { metadata: SessionMetadataSummary }) {
  const rows = [
    metadata.projectPath ? ["Project folder", metadata.projectPath] : null,
    metadata.model ? ["Model", metadata.model] : null,
    metadata.createdMs ? ["Created", formatTimestamp(metadata.createdMs)] : null,
    metadata.lastActivityMs ? ["Last activity", formatTimestamp(metadata.lastActivityMs)] : null,
    metadata.permissionMode ? ["Permission mode", metadata.permissionMode] : null,
    metadata.archived != null ? ["Archived", metadata.archived ? "Yes" : "No"] : null,
    metadata.enabledToolCount != null ? ["Enabled tools", String(metadata.enabledToolCount)] : null,
    connectedToolServerCount(metadata) != null ? ["Connected tool servers", String(connectedToolServerCount(metadata))] : null
  ].filter((row): row is [string, string] => row !== null);

  return (
    <section className="session-metadata-summary">
      <div>
        <span>Local session record</span>
        <h3>{metadata.title ?? "Session metadata"}</h3>
      </div>
      {metadata.initialMessage ? (
        <div className="session-metadata-intro">
          <span>First request</span>
          <p>{metadata.initialMessage}</p>
        </div>
      ) : null}
      <dl>
        {rows.map(([label, value]) => (
          <Fragment key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </Fragment>
        ))}
      </dl>
    </section>
  );
}

function SessionDetailsPanel({
  session,
  preview,
  linkedProject,
  canReveal,
  revealing,
  onReveal,
  onOpenProject,
  onOpenProtectedSettings,
  onCopyPath
}: {
  session: SessionDiscoveryCandidate;
  preview: SessionPreview | null;
  linkedProject: ProjectSummary | null;
  canReveal: boolean;
  revealing: boolean;
  onReveal: () => void;
  onOpenProject: (projectId: number) => void;
  onOpenProtectedSettings: () => void;
  onCopyPath: (path: string) => void;
}) {
  const hasMasked = Boolean(preview && preview.redactedCount > 0);
  return (
    <section className="pane-section">
      <SectionTitle icon={<Info size={15} />} label="File details" />
      <dl className="inspector-list">
        <dt>Status</dt>
        <dd data-help="Sessions open read-only. Secrets are masked until you reveal them.">{preview?.revealed ? "Revealed (transient)" : "Read-only · secrets masked"}</dd>
        <dt>Path</dt>
        <dd className="session-detail-path" data-help="Full local path to this session file.">{session.path}</dd>
        <dt>File type</dt>
        <dd>{displayAppText(session.sessionKind)} session</dd>
        <dt>Size on disk</dt>
        <dd>{preview ? formatBytes(preview.sizeBytes) : "—"}{preview?.truncated ? " · more available in app" : ""}</dd>
        {preview?.createdMs ? (<><dt>Created</dt><dd data-help="When this session file was first written on disk.">{formatTimestamp(preview.createdMs)}</dd></>) : null}
        {preview?.modifiedMs ? (<><dt>Modified</dt><dd data-help="When this session file was last changed on disk.">{formatTimestamp(preview.modifiedMs)}</dd></>) : null}
        <dt>Association</dt>
        <dd data-help={sessionAssociationHelpText(session)}>{sessionAssociationLabelText(session.association)}</dd>
        <dt>Linked project</dt>
        <dd>{linkedProject ? linkedProject.name : session.linkedProjectPaths.length > 0 ? session.linkedProjectPaths.join("; ") : "None linked in local metadata"}</dd>
        <dt>Secrets</dt>
        <dd>{hasMasked ? (preview?.revealed ? `${preview.redactedCount} shown` : `${preview?.redactedCount} masked`) : "None detected"}</dd>
      </dl>
      <div className="session-reader-actions">
        {linkedProject ? (
          <button className="secondary-button" type="button" onClick={() => onOpenProject(linkedProject.id)} data-help={`Open ${linkedProject.name}, the registered project this session is linked to.`}>
            <FolderOpen size={14} /> Open project
          </button>
        ) : null}
        <button className="secondary-button" type="button" onClick={() => onCopyPath(session.path)} data-help="Copy the full session file path to the clipboard.">
          <Copy size={14} /> Copy path
        </button>
        {hasMasked && !preview?.revealed ? (
          canReveal ? (
            <button className="secondary-button" type="button" disabled={revealing} onClick={onReveal} data-help="Reveal the masked tokens transiently for this session only. Allowed because sensitive reveal is enabled under Protected locations. Nothing is written to the index.">
              <Eye size={14} /> {revealing ? "Revealing…" : `Reveal ${preview?.redactedCount} hidden`}
            </button>
          ) : (
            <button className="secondary-button" type="button" onClick={onOpenProtectedSettings} data-help="Revealing masked tokens is currently off. Open Protected locations to allow sensitive reveal.">
              <Lock size={14} /> Allow reveal in Protected locations
            </button>
          )
        ) : null}
      </div>
      <p className="muted help-copy">Transient read-only view. Nothing here is written to SQLite, the search index, or logs.</p>
    </section>
  );
}

function QuickOpenDialog({
  query,
  results,
  starterResults,
  projects,
  searchStatus,
  returnFocus,
  onQuery,
  onClose,
  onOpen
}: {
  query: string;
  results: QuickOpenResult[];
  starterResults: QuickOpenResult[];
  projects: ProjectSummary[];
  searchStatus: QuickOpenSearchStatus;
  returnFocus: HTMLElement | null;
  onQuery: (query: string) => void;
  onClose: () => void;
  onOpen: (result: QuickOpenResult) => void;
}) {
  const { dialogRef, onDialogKeyDown: onFocusTrapKeyDown } = useDialogFocusTrap(onClose, returnFocus);
  const resultRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const projectNamesById = useMemo(() => new Map(projects.map((project) => [project.id, project.name])), [projects]);
  const hasQuery = query.trim().length > 0;
  const displayResults = hasQuery ? results : starterResults;
  const resultCount = displayResults.length;
  const searchMessage = quickOpenSearchMessage(query, results.length, searchStatus);

  useEffect(() => {
    setActiveIndex(0);
  }, [query, displayResults]);

  useLayoutEffect(() => {
    resultRefs.current.length = resultCount;
    scrollPaletteResultIntoView(resultRefs.current, activeIndex);
  }, [activeIndex, resultCount]);

  const onDialogKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key) && displayResults.length > 0) {
      event.preventDefault();
      setActiveIndex((index) => paletteFocusIndex(index, displayResults.length, event.key as PaletteNavigationKey));
      return;
    }
    if (event.key === "Enter" && displayResults[activeIndex]) {
      event.preventDefault();
      onOpen(displayResults[activeIndex]);
      return;
    }
    onFocusTrapKeyDown(event);
  };

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <div ref={dialogRef} className="command-dialog" role="dialog" aria-modal="true" aria-label="Quick Open" onMouseDown={(event) => event.stopPropagation()} onKeyDown={onDialogKeyDown}>
        <div className="dialog-input">
          <Search size={18} />
          <input data-dialog-initial-focus value={query} onChange={(event) => onQuery(event.target.value)} placeholder="Open project or file" aria-describedby={searchMessage ? "quick-open-search-status" : undefined} data-help="Type part of a project, file name or path to open it without browsing the tree." />
        </div>
        <div className="dialog-results" aria-busy={searchStatus === "loading"}>
          {!hasQuery && starterResults.length ? <div className="quick-open-section-label">Continue</div> : null}
          {searchMessage ? (
            <p id="quick-open-search-status" className={`quick-open-status${searchStatus === "error" ? " error" : ""}`} role="status" aria-live="polite">
              {searchStatus === "loading" ? <Loader2 className="spin" size={15} /> : null}
              <span>{searchMessage}</span>
            </p>
          ) : null}
          {!hasQuery && starterResults.length === 0 ? <p className="muted result-empty">No projects loaded.</p> : null}
          {displayResults.map((result, index) => (
            (() => {
              const projectName = projectNamesById.get(result.projectId);
              const isProjectResult = result.itemKind === "project";
              const resultPath = displayLocalPath(result.path);
              const fullDetailLabel = isProjectResult
                ? `Project root · ${resultPath}`
                : quickOpenLocationLabel(result.path, projectName);
              const compactDetailLabel = isProjectResult
                ? `Project root · ${quickOpenLocationLabel(result.path, null, { compactLocalPaths: true })}`
                : quickOpenLocationLabel(result.path, projectName, { compactLocalPaths: true });
              return (
                <button
                  ref={(element) => {
                    resultRefs.current[index] = element;
                  }}
                  className={index === activeIndex ? "active" : undefined}
                  key={`${result.itemKind}-${result.projectId}-${result.nodeId}`}
                  type="button"
                  aria-current={index === activeIndex ? "true" : undefined}
                  data-help={isProjectResult ? `Open project ${result.label}. Path: ${resultPath}.` : `Open ${result.label}${projectName ? ` in ${projectName}` : ""}. Path: ${resultPath}.`}
                  onMouseMove={() => setActiveIndex(index)}
                  onClick={() => onOpen(result)}
                >
                  <span className="quick-result-main">
                    <strong>{result.label}</strong>
                    {isProjectResult ? <span className="quick-result-project">Project</span> : projectName ? <span className="quick-result-project">{projectName}</span> : null}
                  </span>
                  <small className="quick-result-path" title={fullDetailLabel}>{compactDetailLabel}</small>
                </button>
              );
            })()
          ))}
        </div>
      </div>
    </div>
  );
}

function useDialogFocusTrap(onClose: () => void, returnFocus?: HTMLElement | null) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<Element | null>(null);
  const requestedReturnFocusRef = useRef(returnFocus);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    const requestedReturnFocus = requestedReturnFocusRef.current;
    previousFocusRef.current = requestedReturnFocus?.isConnected ? requestedReturnFocus : document.activeElement;
    const dialog = dialogRef.current;
    const initialControl = dialog?.querySelector<HTMLElement>(DIALOG_INITIAL_FOCUS_SELECTOR)
      ?? dialog?.querySelector<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR);
    initialControl?.focus({ preventScroll: true });
    return () => {
      if (previousFocusRef.current instanceof HTMLElement && previousFocusRef.current.isConnected) {
        previousFocusRef.current.focus({ preventScroll: true });
      }
    };
  }, []);

  const onDialogKeyDown = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onCloseRef.current();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR) ?? []);
    const nextIndex = nextDialogFocusIndex(
      focusable.length,
      focusable.indexOf(document.activeElement as HTMLElement),
      event.shiftKey
    );
    event.preventDefault();
    if (nextIndex >= 0) focusable[nextIndex]?.focus();
  }, []);

  return { dialogRef, onDialogKeyDown };
}

function ShellDefaultGuideDialog({
  busy,
  error,
  onOpenSettings,
  onDismiss
}: {
  busy: boolean;
  error: string | null;
  onOpenSettings: () => void;
  onDismiss: () => void;
}) {
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onDismiss);
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onDismiss}>
      <div
        ref={dialogRef}
        className="command-dialog small"
        role="dialog"
        aria-modal="true"
        aria-labelledby="markdown-default-guide-title"
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onDialogKeyDown}
      >
        <header className="dialog-header">
          <div>
            <strong id="markdown-default-guide-title">Choose your Markdown default</strong>
            <span>Code Hangar is now available in Windows Open with.</span>
          </div>
        </header>
        <div className="confirm-action-message primary">
          <Info size={19} />
          <p>Windows does not allow an installer to force this choice. Open Default Apps and choose Code Hangar for <strong>.md</strong>, <strong>.markdown</strong> and <strong>.mdx</strong> if you want double-click to open them here.</p>
        </div>
        {error ? <p className="scan-error">{error}</p> : null}
        <div className="confirm-action-actions">
          <button type="button" className="secondary-button" disabled={busy} onClick={onDismiss}>Not now</button>
          <button data-dialog-initial-focus type="button" className="primary-button" disabled={busy} onClick={onOpenSettings}>
            {busy ? "Opening…" : "Open Windows Default Apps"}
          </button>
        </div>
      </div>
    </div>
  );
}

function ConfirmActionDialog({
  message,
  confirmLabel,
  tone,
  onCancel,
  onConfirm
}: {
  message: string;
  confirmLabel: string;
  tone: "primary" | "danger";
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onCancel);

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onCancel}>
      <div
        ref={dialogRef}
        className="command-dialog small"
        role="dialog"
        aria-modal="true"
        aria-label="Confirm action"
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onDialogKeyDown}
      >
        <div className={`confirm-action-message ${tone}`}>
          {tone === "danger" ? <AlertTriangle size={19} /> : <Info size={19} />}
          <p>{message}</p>
        </div>
        {tone === "danger" ? (
          <label className="confirm-action-acknowledge">
            <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
            <span>I understand this changes local data and I have checked the target above.</span>
          </label>
        ) : null}
        <div className="confirm-action-actions">
          <button
            data-dialog-initial-focus
            type="button"
            className="secondary-button"
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className={tone === "danger" ? "danger-button" : "primary-button"}
            onClick={onConfirm}
            disabled={tone === "danger" && !acknowledged}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

function ResetAllDialog({
  projectCount,
  rootCount,
  editionConsequence,
  onCancel,
  onConfirm
}: {
  projectCount: number;
  rootCount: number;
  editionConsequence?: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onCancel);

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onCancel}>
      <div ref={dialogRef} className="command-dialog reset-all-dialog" role="dialog" aria-modal="true" aria-label="Reset all projects" onMouseDown={(event) => event.stopPropagation()} onKeyDown={onDialogKeyDown}>
        <header className="dialog-header">
          <div>
            <strong>Reset all — unregister every project</strong>
            <span>Clears Code Hangar's local inventory only. Your files on disk are never deleted or modified.</span>
          </div>
          <button className="icon-button" type="button" onClick={onCancel} aria-label="Cancel reset" data-help="Close without changing anything.">
            <X size={16} />
          </button>
        </header>
        <div className="reset-all-body">
          <p className="reset-all-warning">
            <AlertTriangle size={18} />
            <span>{rootCount > 0
              ? <>Code Hangar will clear <strong>all {rootCount} scan root{rootCount === 1 ? "" : "s"}</strong>{projectCount !== rootCount ? ` and ${projectCount} project${projectCount === 1 ? "" : "s"}` : ""} from its local index, then restart.</>
              : <>Code Hangar will <strong>clear its local index</strong>, reclaim that space and restart.</>}</span>
          </p>
          <ul className="reset-all-points">
            <li><strong>Files stay:</strong> project folders on disk are not touched.</li>
            <li><strong>Rebuild when ready:</strong> run Add Projects &gt; Deep Scan after restart.</li>
            {editionConsequence ? <li><strong>Installed-edition consequence:</strong> {editionConsequence}</li> : null}
            <li><strong>Local reset:</strong> this cannot be undone inside Code Hangar.</li>
          </ul>
          <label className="toggle-row" data-help="Confirm you understand this unregisters every project and that a new Deep Scan will be needed afterwards.">
            <input data-dialog-initial-focus type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
            <span>
              I understand this unregisters everything and I will run a new Deep Scan afterwards.
              {editionConsequence ? " I also understand the installed-edition consequence listed above." : ""}
            </span>
          </label>
        </div>
        <div className="reset-all-actions">
          <button type="button" className="secondary-button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className="danger-button"
            disabled={!acknowledged || busy}
            onClick={() => {
              setBusy(true);
              onConfirm();
            }}
            data-help="Unregister every scan root and project now. Files on disk are not deleted; a new Deep Scan rebuilds the list."
          >
            {busy ? "Resetting…" : rootCount > 0 ? `Unregister all ${rootCount} root${rootCount === 1 ? "" : "s"}` : "Reset & reclaim space"}
          </button>
        </div>
      </div>
    </div>
  );
}

function RemoveProjectDialog({
  project,
  hasApp,
  onCancel,
  onConfirm
}: {
  project: ProjectSummary;
  hasApp: boolean;
  onCancel: () => void;
  onConfirm: (opts: { fromApps: boolean; fromHangar: boolean; fromDisk: boolean }) => void;
}) {
  // Safe default: forget locally, and de-register from AI apps only when one lists
  // the project. Disk removal is a separate explicit opt-in.
  const [fromApps, setFromApps] = useState(hasApp);
  const [fromHangar, setFromHangar] = useState(true);
  const [fromDisk, setFromDisk] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [typedProjectName, setTypedProjectName] = useState("");
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onCancel);

  const nothingSelected = !fromApps && !fromHangar && !fromDisk;

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onCancel}>
      <div ref={dialogRef} className="command-dialog reset-all-dialog" role="dialog" aria-modal="true" aria-label={`Remove ${project.name}`} onMouseDown={(event) => event.stopPropagation()} onKeyDown={onDialogKeyDown}>
        <header className="dialog-header">
          <div>
            <strong>Remove {project.name}</strong>
            <span>Choose where to remove it from. Disk removal is off until you explicitly opt in.</span>
          </div>
          <button className="icon-button" type="button" onClick={onCancel} aria-label="Cancel" data-help="Close without changing anything.">
            <X size={16} />
          </button>
        </header>
        <div className="reset-all-body">
          {fromDisk ? (
            <p className="reset-all-warning">
              <AlertTriangle size={18} />
              <span>This choice opens a <strong>separate Safe Manage review</strong>. That workflow creates a verified content backup and moves supported objects to a recovery holding area. It does not permanently delete them or promise that unsupported subtrees will move.</span>
            </p>
          ) : null}
          <label className="toggle-row" data-help={hasApp ? "Stop the project appearing in the AI apps that list it (e.g. Antigravity). Each app's record is backed up first." : "This project is not registered in any supported AI app."}>
            <input type="checkbox" checked={fromApps} disabled={!hasApp} onChange={(event) => setFromApps(event.target.checked)} />
            <span>
              <strong>Remove from AI apps</strong>
              <small>{!hasApp ? "Not registered in any supported AI app." : fromDisk ? "AI-app listings are backed up before removal and can be restored from Recover." : "Stops this project appearing in supported AI apps. Registrations are backed up first and can be restored from Recover."}</small>
            </span>
          </label>
          <label className="toggle-row" data-help="Forget this project inside Code Hangar. Your files on disk are not touched; re-add it later with a scan.">
            <input type="checkbox" checked={fromHangar} onChange={(event) => setFromHangar(event.target.checked)} />
            <span>
              <strong>Remove from Code Hangar</strong>
              <small>Your files on disk stay. Re-add later with a scan.</small>
            </span>
          </label>
          <label className="toggle-row" data-help="Open Safe Manage to back up and move supported project objects into a holding area. Permanent cleanup is a later, separate review in Recovery & cleanup.">
            <input type="checkbox" checked={fromDisk} onChange={(event) => setFromDisk(event.target.checked)} />
            <span>
              <strong>Move supported files to recovery holding</strong>
              <small>Selected only when you choose it. Safe Manage chooses a content-backup destination and reviews the move. Final cleanup happens separately.</small>
            </span>
          </label>
          <label className="toggle-row remove-project-acknowledge">
            <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
            <span>
              <strong>I checked the locations selected above</strong>
              <small>I understand that removing from AI apps changes their local registries and removing from Code Hangar clears local inventory records.</small>
            </span>
          </label>
          {fromDisk ? (
            <label className="change-access-name remove-project-name-check">
              Type <strong>{project.name}</strong> before continuing to the separate backup-and-holding review
              <input value={typedProjectName} onChange={(event) => setTypedProjectName(event.target.value)} autoComplete="off" spellCheck={false} />
            </label>
          ) : null}
        </div>
        <div className="reset-all-actions">
          <button data-dialog-initial-focus type="button" className="secondary-button" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className={fromDisk ? "danger-button" : "primary-button"}
            disabled={nothingSelected || !acknowledged || (fromDisk && typedProjectName.trim() !== project.name.trim())}
            onClick={() => onConfirm({ fromApps, fromHangar, fromDisk })}
            data-help={fromDisk ? "Open Safe Manage for a content backup and supported-object move to holding. This does not run final cleanup." : "Run the selected metadata removals. Files on disk stay where they are."}
          >
            {removeProjectActionLabel({ fromApps, fromHangar, fromDisk })}
          </button>
        </div>
      </div>
    </div>
  );
}

function AddProjectsDialog({
  onClose,
  onDeepScan,
  onSearchFolder,
  onInvestigate,
  includeLoose,
  onToggleLoose,
  includeAgents,
  onToggleAgents,
  installedApps,
  installedAppsLoading,
  installedAppsError,
  wslScan,
  wslPreferencePending,
  wslPreferenceError,
  onToggleWsl,
  deepScanRunning,
  actionsBusy
}: {
  onClose: () => void;
  onDeepScan: () => void;
  onSearchFolder: () => void;
  onInvestigate: () => void;
  includeLoose: boolean;
  onToggleLoose: (value: boolean) => void;
  includeAgents: boolean;
  onToggleAgents: (value: boolean) => void;
  installedApps: InstalledApp[];
  installedAppsLoading: boolean;
  installedAppsError: string | null;
  wslScan: boolean;
  wslPreferencePending: boolean;
  wslPreferenceError: string | null;
  onToggleWsl: (value: boolean) => void;
  deepScanRunning: boolean;
  actionsBusy: boolean;
}) {
  // Split the raw detect_installed_apps result: real host apps become chips, while
  // the backend's reserved WSL rows drive the WSL offer (the `wsl` summary) and the
  // per-app WSL confirmations (`wsl:<app>`, only present once the gate is on) —
  // never bogus host-app chips.
  const { hostApps, wslOffer, wslApps } = partitionInstalledApps(installedApps);
  // The backend folds the offer's name and its call-to-action detail into one label
  // ("WSL detected: N distro(s) (…). Enable WSL scanning to…"); split on the first
  // ". " so the card can show the name bold and the detail muted beneath it.
  const wslOfferSummary = wslOffer?.label ?? "";
  const wslOfferDotIndex = wslOfferSummary.indexOf(". ");
  const wslOfferTitle = wslOfferDotIndex >= 0 ? wslOfferSummary.slice(0, wslOfferDotIndex) : wslOfferSummary;
  const wslOfferDetail = wslOfferDotIndex >= 0 ? wslOfferSummary.slice(wslOfferDotIndex + 2) : "";
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onClose);
  const secondaryActionsBusy = actionsBusy || deepScanRunning || wslPreferencePending;
  const deepScanScope = wslScan ? "Windows and enabled WSL distros" : "Windows";
  const deepScanTitle = deepScanRunning
    ? "Deep Scan is running"
    : wslPreferencePending
      ? "Applying WSL scope"
    : actionsBusy
      ? "Another local scan is running"
      : "Deep Scan — map known projects";
  const deepScanDescription = deepScanRunning
    ? "Return to the live source and inventory progress."
    : wslPreferencePending
      ? "The saved choice is being verified before any discovery can start."
    : actionsBusy
      ? "Finish the current scan before starting another discovery."
      : "Reads local project lists. Existing inventories update by delta; strong new matches are added automatically.";
  const deepScanAction = deepScanRunning
    ? ADD_PROJECTS_SHOW_PROGRESS_ACTION
    : wslPreferencePending
      ? "Applying…"
    : actionsBusy
      ? "In progress"
      : ADD_PROJECTS_DEEP_SCAN_ACTION;

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <div ref={dialogRef} className="command-dialog add-projects-dialog" role="dialog" aria-modal="true" aria-label="Add Projects" onMouseDown={(event) => event.stopPropagation()} onKeyDown={onDialogKeyDown}>
        <header className="dialog-header">
          <div>
            <strong>Add Projects</strong>
            <span>Map known projects, search one location, or inspect a folder temporarily.</span>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label="Close Add Projects" data-help="Close this dialog without adding or scanning anything.">
            <X size={16} />
          </button>
        </header>
        <button
          data-dialog-initial-focus
          type="button"
          className="deep-scan-cta"
          onClick={onDeepScan}
          disabled={(actionsBusy || wslPreferencePending) && !deepScanRunning}
          aria-busy={(actionsBusy || wslPreferencePending) && !deepScanRunning}
          data-help={deepScanRunning
            ? "Show the Deep Scan progress panel again. The scan continued while the panel was hidden."
            : `Read detected AI tools' local project lists on ${deepScanScope}. Existing projects keep their current inventory and only new, changed or removed items are reconciled; strong new matches are added automatically and the rest are listed for review.`}
        >
          <Radar size={22} />
          <span className="deep-scan-cta-text">
            <strong>{deepScanTitle}</strong>
            <span>{deepScanDescription}</span>
          </span>
          <span className="deep-scan-cta-action" aria-hidden="true">{deepScanAction}</span>
        </button>
        <p className="detected-apps" data-help="The AI tools Code Hangar detected on this PC (their config folders exist). Deep Scan only looks at these — it won't parade tools you don't have.">
          {hostApps.length > 0 || wslApps.length > 0 ? (
            <>
              <span>Detected on this PC</span>
              <span className="detected-app-chips" aria-label={`Detected AI tools: ${[...hostApps.map((appItem) => appItem.label), ...wslApps.map((wslApp) => wslApp.badge)].join(", ")}`}>
                {hostApps.map((appItem) => (
                  <span className="detected-app-chip" key={appItem.id}>{appItem.label}</span>
                ))}
                {wslApps.map((wslApp) => (
                  <span className="detected-app-chip detected-app-chip-wsl" key={wslApp.id} title={wslApp.label}>{wslApp.badge}</span>
                ))}
              </span>
            </>
          ) : installedAppsLoading ? (
            <span className="muted">Checking installed AI tools…</span>
          ) : installedAppsError ? (
            <span className="muted" role="status">Could not refresh installed AI tools. Deep Scan can still inspect known local stores.</span>
          ) : (
            <span className="muted">No AI tools detected on this PC yet — you can still search a folder or drive below.</span>
          )}
        </p>
        {wslOffer ? (
          <label className="wsl-offer" data-help="Code Hangar found WSL (Linux) distros on this PC. Enable WSL scanning to include AI tools installed inside them during the scan. Left off it never runs wsl.exe — so it can't surface a WSL error on a PC where WSL isn't fully set up.">
            <input type="checkbox" checked={wslScan} disabled={wslPreferencePending} aria-busy={wslPreferencePending} onChange={(event) => onToggleWsl(event.target.checked)} />
            <span className="wsl-offer-text">
              <strong>{wslOfferTitle}</strong>
              {wslOfferDetail ? <span className="muted">{wslOfferDetail}</span> : null}
            </span>
          </label>
        ) : null}
        {wslPreferencePending ? (
          <p className="deep-scan-options-note wsl-preference-status" role="status" aria-live="polite">
            <Loader2 size={13} className="spin" aria-hidden="true" /> Applying and verifying the WSL choice…
          </p>
        ) : wslPreferenceError ? (
          <p className="scan-error" role="alert">{wslPreferenceError} No project discovery was started.</p>
        ) : null}
        <fieldset className="deep-scan-options">
          <legend>Optional sources</legend>
          <p className="deep-scan-options-note">These apply to Deep Scan and folder search.</p>
          <label data-help="Include conversations that aren't tied to a project — ChatGPT date-named scratch runs and transcripts with no resolvable folder. Applies to both Deep Scan and Search a folder or drive.">
            <input type="checkbox" checked={includeLoose} onChange={(event) => onToggleLoose(event.target.checked)} />
            <span>Sessions without a project <span className="muted">(loose conversations)</span></span>
          </label>
          <label data-help="Include autonomous agent sessions — Hermes / NemoClaw / OpenClaw chat agents that run independently of a project. Applies to both Deep Scan and Search a folder or drive.">
            <input type="checkbox" checked={includeAgents} onChange={(event) => onToggleAgents(event.target.checked)} />
            <span>Agent sessions <span className="muted">(Hermes & similar)</span></span>
          </label>
          {/* When distros were actually detected the prominent WSL offer above owns
              this control, so we don't render (or duplicate) the generic question. */}
          {wslOffer ? null : (
            <label className="deep-scan-wsl-question" data-help="Tick this ONLY if you run AI tools (Claude Code, ChatGPT, Hermes…) inside a WSL (Linux) distro. Code Hangar then enumerates your WSL distros during the scan. Left unticked it never runs wsl.exe — so it can't surface a WSL error on a PC where WSL isn't fully set up.">
              <input type="checkbox" checked={wslScan} disabled={wslPreferencePending} aria-busy={wslPreferencePending} onChange={(event) => onToggleWsl(event.target.checked)} />
              <span>I run AI tools inside <strong>WSL</strong> <span className="muted">(Linux — scan there too)</span></span>
            </label>
          )}
        </fieldset>
        <div className="add-project-secondary">
          <button type="button" className="add-project-search" onClick={onSearchFolder} disabled={secondaryActionsBusy} data-help="Pick a folder or drive (including C:). Code Hangar runs project discovery only under that location, honouring the options above. Strong matches are added automatically; weaker candidates are listed for review.">
            <Search size={18} />
            <span>
              <strong>Find projects in a folder or drive</strong>
              <span className="muted">Scans only the location you choose. Strong matches are added; others wait for review.</span>
            </span>
          </button>
          <button type="button" className="add-project-search" onClick={onInvestigate} disabled={secondaryActionsBusy} data-help="Inspect a folder without adding it to Projects. Code Hangar reports what it is, whether a registered project owns it, and its local size.">
            <FolderSearch size={18} />
            <span>
              <strong>Inspect a folder temporarily</strong>
              <span className="muted">Builds a local report without adding it to Projects. Discard it when done.</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}

function CommandDialog({
  selectedProjectName,
  returnFocus,
  onClose,
  onQuickOpen,
  onAddProjects,
  onOverview,
  onProject,
  onDiscover,
  onReview,
  onRecovery,
  onSettings
}: {
  selectedProjectName?: string | null;
  returnFocus: HTMLElement | null;
  onClose: () => void;
  onQuickOpen: () => void;
  onAddProjects: () => void;
  onOverview: () => void;
  onProject: () => void;
  onDiscover: () => void;
  onReview: () => void;
  onRecovery: () => void;
  onSettings: () => void;
}) {
  const { dialogRef, onDialogKeyDown: onFocusTrapKeyDown } = useDialogFocusTrap(onClose, returnFocus);
  const pointerFocusReadyRef = useRef(false);
  const projectCommandState = projectScopedCommandState(selectedProjectName);

  useEffect(() => {
    const pointerFocusTimer = window.setTimeout(() => {
      pointerFocusReadyRef.current = true;
    }, 200);
    return () => {
      window.clearTimeout(pointerFocusTimer);
    };
  }, []);

  const onDialogKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
    if (focusable.length === 0) return;
    const currentIndex = focusable.indexOf(document.activeElement as HTMLElement);
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const nextIndex = paletteFocusIndex(currentIndex, focusable.length, event.key as PaletteNavigationKey);
      focusable[nextIndex]?.focus();
      return;
    }
    onFocusTrapKeyDown(event);
  };

  const onDialogMouseMove = (event: MouseEvent<HTMLDivElement>) => {
    if (!palettePointerMayMoveFocus(pointerFocusReadyRef.current, event.movementX, event.movementY)) return;
    const command = (event.target as HTMLElement).closest<HTMLButtonElement>("button:not(:disabled)");
    if (command && command !== document.activeElement && dialogRef.current?.contains(command)) {
      command.focus();
    }
  };

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <div ref={dialogRef} className="command-dialog small" role="dialog" aria-modal="true" aria-label="Command Palette" onMouseDown={(event) => event.stopPropagation()} onMouseMove={onDialogMouseMove} onKeyDown={onDialogKeyDown}>
        <button type="button" data-help="Open the project and file jump dialog." onClick={onQuickOpen}><Search size={16} /><span>Quick open</span><kbd>Ctrl+P</kbd></button>
        <button type="button" data-help="Open the local inventory overview." onClick={onOverview}><Home size={16} /><span>Overview</span></button>
        <button type="button" disabled={!projectCommandState.enabled} data-help={projectCommandState.projectHelp} onClick={onProject}><FolderOpen size={16} /><span>Selected project</span><small className="command-context-pill">{projectCommandState.contextLabel}</small></button>
        <button type="button" data-help="Search local content and find candidates for review." onClick={onDiscover}><Compass size={16} /><span>Discover</span></button>
        <button type="button" data-help="Open portfolio-wide Safe Manage analysis and recommendations. No project needs to be selected." onClick={onReview}><ListChecks size={16} /><span>Safe Manage</span></button>
        <button type="button" data-help="Open Recovery & cleanup to review held projects, final-cleanup eligibility and recovery archives." onClick={onRecovery}><ArchiveRestore size={16} /><span>Recovery &amp; cleanup</span></button>
        <button type="button" data-help="Manage scanned folders and protected locations." onClick={onSettings}><Settings size={16} /><span>Settings</span></button>
        <button type="button" data-help="Open Add Projects to add one folder directly or run passive discovery under a selected folder or drive." onClick={onAddProjects}><FolderOpen size={16} /><span>Add projects</span></button>
        <div className="shortcut-panel" aria-label="Keyboard Shortcuts">
          <div className="shortcut-heading"><Keyboard size={15} />Keyboard Shortcuts</div>
          <div><span>Quick Open</span><kbd>Ctrl+P</kbd></div>
          <div><span>Commands</span><kbd>Ctrl+K</kbd></div>
          <div><span>Back / Forward</span><kbd>Alt+Left</kbd><kbd>Alt+Right</kbd></div>
        </div>
      </div>
    </div>
  );
}
