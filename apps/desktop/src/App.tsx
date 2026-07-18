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
  Sparkles,
  Wand2,
  Eye,
  FileText,
  Folder,
  FolderOpen,
  Gauge,
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
import {
  ADD_PROJECTS_DEEP_SCAN_ACTION,
  ADD_PROJECTS_SHOW_PROGRESS_ACTION,
  deepScanSourceLabels,
  deepScanUsesIndeterminateProgress,
  partitionInstalledApps,
  type DeepScanPhase
} from "./addProjectsDialog";
import { api } from "./api";
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
import { documentSearchCriteriaKey, duplicateSearchCriteriaKey, orphanSearchCriteriaKey, retainRunningDuplicateConfirmations, scopeForDiscoveryEntry, scopeForDocumentSearchEntry } from "./documentSearch";
import { graphMapItemCounts, INITIAL_GRAPH_MAP_LIMIT, nextGraphMapExpansionLimit } from "./graphMapExpansion";
import { pinFailureMessage, pinSuccessMessage, postActionHoverHelp, scanRootToggleFailureMessage, scanRootToggleMessage } from "./interactionFeedback";
import { renderMarkdownSafe } from "./markdown";
import { FILE_INSPECTOR_CONTEXT, projectInspectorContext, projectViewUsesFileInspector } from "./projectInspector";
import { compactSidebarProjects, composeQuickOpenResults, isDemoProject, orderSidebarProjects, projectSearchKeyAction, projectWatchLabel, quickOpenSearchMessage, resolveProjectScanState, shouldRenderProjectRow, shouldShowDemoProjects, starterQuickOpenResults, visibleProjectItems, visibleProjects, type QuickOpenSearchStatus } from "./projectVisibility";
import type { ProjectSort, ProjectStatusFilter } from "./projectVisibility";
import { removeProjectActionLabel } from "./removeProjectDialog";
import { unregisterProjectConfirmationMessage, unregisterRootConfirmationMessage } from "./settingsActions";
import { INITIAL_CONTEXT_OPEN_OPTIONS, selectInitialContextFile } from "./projectAutoOpen";
import { formatScanDuration, scanProgressParts } from "./scanProgress";
import { clampSessionTranscriptPage, compactSessionToolActivity, enrichedSessionDisplayName, initialSessionTranscriptPage, nextSessionPreviewLimit, parseSessionMetadata, parseSessionTranscript, SESSION_TRANSCRIPT_PAGE_SIZE, sessionDisplayNameNeedsEnrichment, sessionSupportsProgressiveLoading, sessionTranscriptPageCount, sessionTranscriptPageSlice, type SessionMetadataSummary } from "./session-transcript";
import { SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT, compactSidebarSessionGroups, displayedSidebarSessionGroups, previewSidebarSessionItems } from "./sessionSidebar";
import type { SessionScope, SessionSort } from "./sessionSidebar";
import { focusedFileStatusIsRelevant, INITIAL_WORKSPACE_ROUTE, normalizeStartupPreferences, parseWorkspaceRoute, projectInspectorCollapsedForLayout, projectInspectorHostsAiExplain, projectSidebarCollapsedForLayout, projectViewPrefersWideCanvas, sameWorkspaceRoute, shouldRecordWorkspaceRoute, startupPaneCollapse, startupWorkspaceRoute, workspaceCenterPaneIsCramped, workspaceRouteStatusText } from "./workspaceRoute";
import type { DiscoverView, PrimaryView, ProjectView, RightPaneView, SettingsView, WorkspaceRoute } from "./workspaceRoute";
import type { StartupPreferences } from "./workspaceRoute";
import { displayAppText, projectAppMetas, sessionAppMeta, type AppMeta } from "./app-meta";
import { OverviewView } from "./views/OverviewView";
import { InspectorPane, ProjectWorkspace, Sidebar, ToolWorkspace, WorkspaceGrid } from "./WorkspaceShell";
import { selectedProjectActivation, useProjectWorkspace } from "./useProjectWorkspace";
import { loadStartupSideData } from "./startupSideData";
import { useTabDrag } from "./hooks/useTabDrag";
import { ConceptHelp, type BeginnerHelpConcept } from "./BeginnerHelp";
import { CountUp, SectionTitle, compactLocalPath, displayLocalPath, formatBytes, formatOptionalBytes, formatTimestamp, orphanReferenceStatusText, quickOpenLocationLabel, storedBooleanPreference } from "./ui";
import type { AiExplainTarget } from "./views/AiAssist";
import { ProjectCenterView, projectSidebarSummaryLabel } from "./views/ProjectCenterView";
import { ChangeAccessDialog } from "./views/project-center/ChangeAccessDialog";
import type { RewriteTarget } from "./views/RewriteDialog";
import { GuidedTour, guidedTourStepCopy, guidedTourStorageKey, TOUR_SELECTORS, type GuidedTourMode, type TourStep } from "./views/GuidedTour";
import type { DuplicateConfirmStateMap } from "./views/DiscoverView";
import type {
  AdapterSummary,
  AiRewriteProposal,
  AiSuggestionApplyResult,
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationStatus,
  DashboardSummary,
  DocumentHit,
  DuplicateCandidates,
  FilePreview,
  FolderExplanation,
  FolderInvestigation,
  GraphMap,
  GraphMapExpansionState,
  LostProjectCandidates,
  MutationActivityLog,
  MutationLockInspection,
  NavItem,
  NodeRelationships,
  OperationPlan,
  OrphanCandidates,
  OrphanStatus,
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
  SessionDiscoveryCandidate,
  SessionPreview,
  SecurityStatus,
  ProcessResourceUsage,
  ProtectedZone,
  SystemResourceProfile,
  WatcherStatus
} from "./types";

const connectorFrontendBuild = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector";
const tutorialStorageKey = guidedTourStorageKey(connectorFrontendBuild ? "connector" : "local");
const connectorApiLoader = connectorFrontendBuild
  ? () => import("./connectorApi").then((module) => module.connectorApi)
  : null;
const EmptyConnectorView = () => null;

async function requireConnectorApi() {
  if (!connectorApiLoader) {
    throw new Error("This feature is not compiled into this frontend edition.");
  }
  return connectorApiLoader();
}

const SettingsAppearanceView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsAppearanceView })));
const SettingsAutomationView = connectorFrontendBuild
  ? lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsAutomationView })))
  : EmptyConnectorView;
const SettingsConnectedAppsView = connectorFrontendBuild
  ? lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsConnectedAppsView })))
  : EmptyConnectorView;
const SettingsFoldersView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsFoldersView })));
const SettingsProtectionView = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsProtectionView })));
const SettingsDiagnosticsExportCard = lazy(() => import("./views/SettingsView").then((module) => ({ default: module.SettingsDiagnosticsExportCard })));
const RecoveryView = lazy(() => import("./views/RecoveryView").then((module) => ({ default: module.RecoveryView })));
const InspectorView = lazy(() => import("./views/InspectorView").then((module) => ({ default: module.InspectorView })));
const DiscoverSearchView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverSearchView })));
const DiscoverOrphansView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverOrphansView })));
const DiscoverDuplicatesView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverDuplicatesView })));
const DiscoverProjectDiscoveryView = lazy(() => import("./views/DiscoverView").then((module) => ({ default: module.DiscoverProjectDiscoveryView })));
const OrganizeView = lazy(() => import("./views/OrganizeView").then((module) => ({ default: module.OrganizeView })));
const AiExplainPanel = connectorFrontendBuild
  ? lazy(() => import("./views/AiAssist").then((module) => ({ default: module.AiExplainPanel })))
  : EmptyConnectorView;
const AiAssistKeyCard = connectorFrontendBuild
  ? lazy(() => import("./views/AiAssist").then((module) => ({ default: module.AiAssistKeyCard })))
  : EmptyConnectorView;
const ReviewImpactView = lazy(() => import("./views/ReviewImpactView").then((module) => ({ default: module.ReviewImpactView })));
const RewriteDialog = connectorFrontendBuild
  ? lazy(() => import("./views/RewriteDialog").then((module) => ({ default: module.RewriteDialog })))
  : EmptyConnectorView;
const RecapAiLayer = connectorFrontendBuild
  ? lazy(() => import("./views/RecapAiLayer").then((module) => ({ default: module.RecapAiLayer })))
  : undefined;
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

const PANE_WIDTH_STORAGE_KEY = "codehangar:pane-widths";
const TREE_WIDTH_STORAGE_KEY = "codehangar:tree-pane-width";
const SIDEBAR_COLLAPSE_STORAGE_KEY = "codehangar:sidebar-collapse-v2";
const LOST_PRESETS_STORAGE_KEY = "codehangar:lost-project-presets";
const PERFORMANCE_MODE_STORAGE_KEY = "codehangar:performance-mode";
const SHOW_DEMO_PROJECTS_STORAGE_KEY = "codehangar:show-demo-projects";
const THEME_MODE_STORAGE_KEY = "codehangar:theme-mode";
const ADVANCED_MODE_STORAGE_KEY = "codehangar:advanced-mode";
const SHOW_PROJECT_PATHS_STORAGE_KEY = "codehangar:show-project-paths";
const SHOW_TOPBAR_NAV_STORAGE_KEY = "codehangar:show-topbar-nav-v3";
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
  if (typeof window === "undefined") return true;
  return storedBooleanPreference(window.localStorage.getItem(SHOW_TOPBAR_NAV_STORAGE_KEY), true);
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
// CH-logo hover flyout (reachable when the sidebar is collapsed or scrolled) and
// the optional compact icon row in the top bar. `iconsOnly` drops the labels.
function PrimaryNavButtons({
  primaryView,
  iconsOnly,
  onOverview,
  onDiscover,
  onRecovery,
  onSettings
}: {
  primaryView: string;
  iconsOnly?: boolean;
  onOverview: () => void;
  onDiscover: () => void;
  onRecovery: () => void;
  onSettings: () => void;
}) {
  // Only the global sections live here. Safe Manage is project-scoped, so it sits
  // with the Projects list (above the filters) and acts on the selected project.
  return (
    <>
      <button className={primaryView === "overview" ? "active" : ""} type="button" onClick={onOverview} aria-label="Overview" data-help="Open a clear summary of the local inventory, scan health and largest project footprints.">
        <Home size={16} />{iconsOnly ? null : <span>Overview</span>}
      </button>
      <button className={primaryView === "discover" ? "active" : ""} type="button" onClick={onDiscover} aria-label="Discover" data-help="Search local content and find forgotten projects, unreferenced files or duplicate candidates. Discovery never changes files.">
        <Compass size={16} />{iconsOnly ? null : <span>Discover</span>}
      </button>
      <button className={primaryView === "recovery" ? "active" : ""} type="button" onClick={onRecovery} aria-label="Recover" data-help="Open Recover to review local recovery history, verified backups and held files recorded for this profile.">
        <ArchiveRestore size={16} />{iconsOnly ? null : <span>Recover</span>}
      </button>
      <button className={primaryView === "settings" ? "active" : ""} type="button" onClick={onSettings} aria-label="Settings" data-help="Manage scan folders, protected locations and advanced local-only details.">
        <Settings size={16} />{iconsOnly ? null : <span>Settings</span>}
      </button>
    </>
  );
}

export function App() {
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [projectsFromCache, setProjectsFromCache] = useState(false);
  const [inventoryReady, setInventoryReady] = useState(false);
  const [selectedProjectId, setSelectedProjectId] = useState<number | null>(null);
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
  const [relationshipsNodeId, setRelationshipsNodeId] = useState<number | null>(null);
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
  const [backStack, setBackStack] = useState<number[]>([]);
  const [forwardStack, setForwardStack] = useState<number[]>([]);
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
  const [wslScanChoice, setWslScanChoice] = useState(false);
  const [deepScanProgress, setDeepScanProgress] = useState<DeepScanProgress | null>(null);
  const [deepScanOverlayVisible, setDeepScanOverlayVisible] = useState(false);
  const [resetAllVisible, setResetAllVisible] = useState(false);
  const [removeProjectTarget, setRemoveProjectTarget] = useState<ProjectSummary | null>(null);
  // AI Assist "Explain this" target; authorization is always resolved from nodeId (file) or gated
  // on the exact bytes (text selection) in Rust. The panel is non-blocking: it lives docked in the
  // right column or popped out into a floating window, and the result persists in the aiTasks store.
  const [aiExplainTarget, setAiExplainTarget] = useState<AiExplainTarget | null>(null);
  const acceptanceAiPanelOpened = useRef(false);
  const [aiExplainDocked, setAiExplainDocked] = useState(true);
  // Edge card collapsed to a thin strip (tool views only); reset open on a new target.
  const [aiExplainCollapsed, setAiExplainCollapsed] = useState(false);
  const [aiExplainPos, setAiExplainPos] = useState<{ x: number; y: number }>({ x: 140, y: 96 });
  const openExplain = useCallback((target: AiExplainTarget) => {
    setAiExplainTarget(target);
    setAiExplainDocked(true);
  }, []);
  // AI-assisted correction is restricted to one explicit selection. Rust owns the proposal,
  // unique anchor, whole-file CAS, verified snapshot and durable edit-session id.
  const [rewriteTarget, setRewriteTarget] = useState<RewriteTarget | null>(null);
  const [rewriteFileName, setRewriteFileName] = useState("");
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
  const [documentScope, setDocumentScope] = useState<"current" | "all">("current");
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
  const [orphanStatusByNode, setOrphanStatusByNode] = useState<Record<number, OrphanStatus>>({});
  const [lostProjectCandidates, setLostProjectCandidates] = useState<LostProjectCandidates | null>(null);
  const [projectDiscoveryReport, setProjectDiscoveryReport] = useState<ProjectDiscoveryReport | null>(null);
  // The left-sidebar Sessions list is its own inventory, populated only by
  // "Find Sessions". Project discovery (Find Projects / Deep Scan) never writes
  // here, so it can't bleed into the sidebar — it only fills each project's own
  // Sessions tab (selectedProjectSessions, from the project report).
  const [sessionInventory, setSessionInventory] = useState<SessionDiscoveryCandidate[]>([]);
  const [sessionTitleOverrides, setSessionTitleOverrides] = useState<Record<string, string>>({});
  const [projectDiscoveryLoading, setProjectDiscoveryLoading] = useState(false);
  const [projectDiscoveryError, setProjectDiscoveryError] = useState<string | null>(null);
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
  // The irreversible "Final remove" control: OFF by default. The backend setting is the real
  // enforcement; starting false prevents the UI from briefly offering it before the setting loads.
  const [finalRemoveEnabled, setFinalRemoveEnabled] = useState(false);
  const [automationStatus, setAutomationStatus] = useState<AutomationStatus | null>(null);
  const [automationAgents, setAutomationAgents] = useState<AutomationAgentSummary[]>([]);
  const [automationActivity, setAutomationActivity] = useState<AutomationActivityEntry[]>([]);
  const [automationCredential, setAutomationCredential] = useState<AutomationCredential | null>(null);
  const [automationBusy, setAutomationBusy] = useState(false);
  const [automationError, setAutomationError] = useState<string | null>(null);
  // The last reversible "remove from AI apps" action, so the status bar can offer Undo.
  const [appRemovalUndo, setAppRemovalUndo] = useState<{ name: string; id: string } | null>(null);
  const [appRemovals, setAppRemovals] = useState<PersistedAppRemoval[]>([]);
  const [mutationModeToken, setMutationModeToken] = useState<string | null>(null);
  const [mutationBusy, setMutationBusy] = useState(false);
  const [mutationActivity, setMutationActivity] = useState<MutationActivityLog | null>(null);
  const [mutationMessage, setMutationMessage] = useState<string | null>(null);
  const [mutationLockInspection, setMutationLockInspection] = useState<MutationLockInspection | null>(null);
  const [mutationLockLoading, setMutationLockLoading] = useState(false);
  const [mutationBackupLevel, setMutationBackupLevel] = useState<"minimal" | "standard" | "full">("standard");
  const [mutationAllowSameVolume, setMutationAllowSameVolume] = useState(false);
  // Removing a folder always empties it 100%: sensitive/protected files are backed up
  // first and then moved, and junction/symlink links are removed (targets untouched), so
  // nothing is left behind. There is no partial mode — the verified backup makes it
  // reversible. A strong per-project confirmation still discloses that secrets are copied
  // into the backup before anything runs; backup and move use the SAME value.
  const [mutationIncludeProtected] = useState(true);
  // The verified backup that covers the CURRENT plan. A move to the recovery area is
  // only allowed once this is set (Gate 3: no move/delete without a verified backup).
  const [mutationBackupId, setMutationBackupId] = useState<number | null>(null);
  // The folder currently being investigated by path (not a registered project).
  const [investigation, setInvestigation] = useState<FolderInvestigation | null>(null);
  const [investigationBusy, setInvestigationBusy] = useState(false);
  const [scanStatuses, setScanStatuses] = useState<Record<string, ScanStatus>>({});
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

  const selectedProject = useMemo(
    () => projects.find((project) => project.id === selectedProjectId) ?? null,
    [projects, selectedProjectId]
  );
  const changesUnlocked = selectedProjectId !== null && unlockedChangeProjectId === selectedProjectId;
  useEffect(() => {
    setUnlockedChangeProjectId(null);
    setChangeUnlockTarget(null);
    setRewriteTarget(null);
  }, [selectedProjectId]);
  useEffect(() => {
    if (!changesUnlocked && (previewMode === "edit" || previewMode === "values")) {
      setPreviewMode("source");
    }
  }, [changesUnlocked, previewMode]);
  const requestChangeAccess = useCallback(() => {
    if (!selectedProject) {
      setStatusText("Choose a project before unlocking file changes.");
      return;
    }
    setChangeUnlockTarget(selectedProject);
  }, [selectedProject]);
  const selectedProjectScanRoot = useMemo(() => {
    if (!selectedProject) return null;
    if (selectedProject.scanRootId != null) {
      return roots.find((root) => root.id === selectedProject.scanRootId) ?? null;
    }
    const selectedPath = normalizeProjectRootPath(selectedProject.path);
    return roots.find((root) => normalizeProjectRootPath(root.path) === selectedPath) ?? null;
  }, [roots, selectedProject]);
  const displayedProjects = useMemo(() => visibleProjects(projects, showDemoProjects), [projects, showDemoProjects]);
  const displayedDocumentHits = useMemo(
    () => visibleProjectItems(documentHits, projects, showDemoProjects),
    [documentHits, projects, showDemoProjects]
  );
  const effectiveSessionInventory = useMemo(
    () => sessionInventory.map((session) => {
      const displayName = sessionDisplayNameNeedsEnrichment(session.displayName)
        ? sessionTitleOverrides[session.path]
        : undefined;
      return displayName ? { ...session, displayName } : session;
    }),
    [sessionInventory, sessionTitleOverrides]
  );
  const visibleQuickResults = useMemo(
    () => composeQuickOpenResults(quickQuery, quickResults, projects, showDemoProjects),
    [projects, quickQuery, quickResults, showDemoProjects]
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
  const currentDocumentSearchCriteria = useMemo(() => documentSearchCriteriaKey({
    query: documentQuery,
    scope: documentScope,
    projectId: selectedProjectId,
    indexedKind: documentKind,
    pathFilter: documentPathFilter,
    nameFilter: documentNameFilter,
    limit: documentLimit,
    includeFixtureProjects: demosVisible
  }), [demosVisible, documentKind, documentLimit, documentNameFilter, documentPathFilter, documentQuery, documentScope, selectedProjectId]);
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
  // True only in the connector edition (built with agent_automation). The base/local
  // edition links none of the connector commands, so its AI panels would be dead UI.
  const connectorBuild = connectorFrontendBuild && (security?.activeFeatures.includes("agent_automation") ?? false);
  const displayedProjectIds = useMemo(() => new Set(displayedProjects.map((project) => project.id)), [displayedProjects]);
  const displayedPinnedItems = useMemo(
    () => pinnedItems.filter((item) => {
      if (item.itemKind === "project") return displayedProjectIds.has(item.nodeId);
      if (item.projectId != null) return displayedProjectIds.has(item.projectId);
      return true;
    }),
    [displayedProjectIds, pinnedItems]
  );
  const previewPolicy = useMemo<PreviewPolicy>(
    () => ({
      allowSensitiveReveal: zoneAllowSensitiveReveal,
      relaxNonStrongProtectedPreview: zoneRelaxNonStrongPreview
    }),
    [zoneAllowSensitiveReveal, zoneRelaxNonStrongPreview]
  );

  const selectedPinned = useMemo(
    () => preview ? pinnedItems.some((item) => item.nodeId === preview.nodeId && item.itemKind === "file") : false,
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
  const previewOrphanStatus = preview ? orphanStatusByNode[preview.nodeId] ?? null : null;
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
    // contains currentPath is being indexed now, and earlier ones are done. That
    // lets the panel tick projects off live — the same dopamine as the mapping
    // phase — without any backend change.
    const normalize = (value: string) => value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
    const order = buildScanStatus.rootPaths.map(normalize);
    const current = buildScanStatus.currentPath ? normalize(buildScanStatus.currentPath) : null;
    let currentIndex = -1;
    if (current) {
      currentIndex = order.findIndex((path) => current === path || current.startsWith(`${path}\\`));
    }
    const terminal = ["completed", "partial", "cancelled", "failed"].includes(buildScanStatus.state);
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
        const done = terminal || (currentIndex >= 0 && index >= 0 && index < currentIndex);
        const current =
          !done && ((currentIndex >= 0 && index === currentIndex) || (currentIndex < 0 && working));
        return { id: project.id, name: project.name, done, current };
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
    const state = buildScanStatus.state;
    setDeepScanProgress((current) =>
      current && current.phase === "building"
        ? {
            ...current,
            phase: "done",
            note:
              state === "completed"
                ? `Mapped ${current.addedCount} project${current.addedCount === 1 ? "" : "s"} — files and context are ready.`
                : state === "cancelled"
                  ? "Scan stopped. Partial inventory kept — resume any project anytime."
                  : "Inventory scan finished."
          }
        : current
    );
  }, [deepScanProgress?.phase, buildScanStatus]);
  useEffect(() => {
    if (deepScanProgress?.phase !== "done") return;
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
  const aiExplainHostedInInspector = projectInspectorHostsAiExplain(
    isProjectLayout,
    rightPaneView,
    rightPaneCollapsedForLayout,
    aiExplainDocked
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
  // Width the edge-docked "Explain this" card reserves on the right in tool views:
  // a small card (≈ the floating window) when open, a thin strip when collapsed.
  // Set on the app shell so both the card and the content-reflow reserve read it.
  const appShellStyle = useMemo(
    () => ({ "--ai-edge-width": `${aiExplainCollapsed ? 44 : 360}px` }) as CSSProperties,
    [aiExplainCollapsed]
  );
  // A freshly opened Explain target shows expanded (never inherit a stale strip).
  useEffect(() => {
    if (aiExplainTarget) setAiExplainCollapsed(false);
  }, [aiExplainTarget]);
  useEffect(() => {
    if (!import.meta.env.DEV || !connectorBuild || acceptanceAiPanelOpened.current) return;
    if (new URLSearchParams(window.location.search).get("acceptanceAiPanel") !== "file") return;
    if (preview?.state !== "ready") return;
    acceptanceAiPanelOpened.current = true;
    openExplain({ kind: "file", nodeId: preview.nodeId, path: preview.path });
  }, [connectorBuild, openExplain, preview]);
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
    setScanStatuses((current) => ({ ...current, [status.jobId]: status }));
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

  // Probe which AI tools are installed on this PC (and the saved WSL-scan choice)
  // so the Deep Scan dialog lists only what's present. Pure existence checks — no
  // wsl.exe, no scan — safe to run once at startup.
  useEffect(() => {
    void api.detectInstalledApps().then(setInstalledApps).catch(() => undefined);
    void api.wslScanEnabled().then(setWslScanChoice).catch(() => undefined);
  }, []);

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
        api.mutationModeStatus()
          .then((available) => {
            if (cancelled) return;
            setMutationAvailable(available);
            if (available) {
              void refreshMutationActivity();
              void api.mutationFinalRemoveEnabled()
                .then((enabled) => { if (!cancelled) setFinalRemoveEnabled(enabled); })
                .catch(() => { if (!cancelled) setFinalRemoveEnabled(false); });
            }
          })
          .catch(() => {
            if (!cancelled) setMutationAvailable(false);
          });
      });
    }, 700);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [refreshMutationActivity]);

  const refreshAutomation = useCallback(async () => {
    setAutomationBusy(true);
    setAutomationError(null);
    try {
      const connectorApi = await requireConnectorApi();
      const status = await connectorApi.automationStatus();
      setAutomationStatus(status);
      if (status.enabled) {
        const [agents, activity] = await Promise.all([
          connectorApi.automationAgents(),
          connectorApi.automationActivity(100)
        ]);
        setAutomationAgents(agents);
        setAutomationActivity(activity);
      } else {
        setAutomationAgents([]);
        setAutomationActivity([]);
      }
    } catch (error) {
      setAutomationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationBusy(false);
    }
  }, []);

  useEffect(() => {
    // Only the AI Connector edition links automation commands; never call them in
    // the Local/core editions (the command is not registered there).
    if (primaryView === "settings" && settingsView === "advanced" && connectorBuild) {
      void refreshAutomation();
    }
  }, [primaryView, refreshAutomation, settingsView, connectorBuild]);

  const registerAutomation = useCallback(async (name: string, scopes: string[], projectIds: number[]) => {
    setAutomationBusy(true);
    setAutomationError(null);
    try {
      const connectorApi = await requireConnectorApi();
      const credential = await connectorApi.automationRegister(name, scopes, projectIds);
      setAutomationCredential(credential);
      setStatusText(`Registered local tool ${credential.agent.name}. Store its token now; it is shown once.`);
      const [agents, activity] = await Promise.all([connectorApi.automationAgents(), connectorApi.automationActivity(100)]);
      setAutomationAgents(agents);
      setAutomationActivity(activity);
    } catch (error) {
      setAutomationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationBusy(false);
    }
  }, []);

  const revokeAutomation = useCallback(async (agentId: number) => {
    if (!(await requestConfirm(
      "Revoke this local credential and all of its temporary file grants?",
      { confirmLabel: "Revoke credential", tone: "danger" }
    ))) return;
    setAutomationBusy(true);
    setAutomationError(null);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.automationRevoke(agentId);
      setAutomationCredential((current) => current?.agent.id === agentId ? null : current);
      setStatusText("Local credential revoked.");
      await refreshAutomation();
    } catch (error) {
      setAutomationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationBusy(false);
    }
  }, [refreshAutomation, requestConfirm]);

  const forgetRevokedAutomation = useCallback(async (agentId: number) => {
    setAutomationBusy(true);
    setAutomationError(null);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.automationForgetRevoked(agentId);
      setStatusText("Revoked local credential entry removed. Activity records remain.");
      await refreshAutomation();
    } catch (error) {
      setAutomationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationBusy(false);
    }
  }, [refreshAutomation]);

  const grantAutomationRead = useCallback(async (agentId: number, nodeId: number) => {
    setAutomationBusy(true);
    setAutomationError(null);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.automationGrantRead(agentId, nodeId, 10);
      setStatusText("Temporary file access granted for 10 minutes. Protected policy still applies.");
      setAutomationActivity(await connectorApi.automationActivity(100));
    } catch (error) {
      setAutomationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationBusy(false);
    }
  }, []);

  const enterMutationMode = useCallback(async () => {
    setMutationBusy(true);
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
    // Empty-completely opt-in: before copying secrets into the backup, show exactly which
    // sensitive/protected files and junction/symlink links will be included, and require
    // an explicit per-project confirmation.
    if (mutationIncludeProtected) {
      try {
        const preview = await api.mutationPreviewProtected(activeOperationPlan);
        if (preview.protected.length === 0 && preview.reparse.length === 0) {
          setMutationMessage("This project has no sensitive/protected files or links to include; the standard backup already covers it.");
        } else {
          const sample = (list: string[]) => list.slice(0, 8).join("\n  ") + (list.length > 8 ? `\n  …and ${list.length - 8} more` : "");
          const parts: string[] = [];
          if (preview.protected.length > 0) {
            parts.push(`${preview.protected.length} SENSITIVE/PROTECTED file(s) — these will be COPIED into the backup folder (secrets included) and then removed from the project:\n  ${sample(preview.protected)}`);
          }
          if (preview.reparse.length > 0) {
            parts.push(`${preview.reparse.length} junction/symlink LINK(s) — the links (not their targets) will be removed:\n  ${sample(preview.reparse)}`);
          }
          if (!(await requestConfirm(
            `Empty this project completely?\n\n${parts.join("\n\n")}\n\nYour backup folder will then contain these secrets. Continue?`,
            { confirmLabel: "Continue to backup", tone: "danger" }
          ))) {
            setMutationMessage("Removal cancelled. Removing a folder backs up everything (secrets included) and empties it fully; nothing ran.");
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
    try {
      const result = await api.mutationBackupStart(activeOperationPlan, destination, mutationBackupLevel, mutationAllowSameVolume, mutationIncludeProtected, mutationModeToken);
      setMutationModeToken(null);
      // Remember the verified backup so the move to the recovery area is allowed
      // (Gate 3) and the held copies become permanently deletable through it.
      setMutationBackupId(result.verified ? result.backupId : null);
      setMutationMessage(`Verified backup ${result.backupId} wrote ${formatBytes(result.totalBytes)} across ${result.itemCount} item${result.itemCount === 1 ? "" : "s"}.`);
      setStatusText(`Verified backup manifest written: ${result.manifestPath}`);
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
      return;
    }
    setMutationBusy(true);
    try {
      const result = await api.mutationMoveStart(activeOperationPlan, destination, mutationBackupId, mutationIncludeProtected, mutationModeToken);
      setMutationModeToken(null);
      // A fresh backup is required before any subsequent move.
      setMutationBackupId(null);
      setMutationMessage(`Move operation ${result.operationId} finished: ${result.moved} moved, ${result.skipped} skipped, ${result.failed} failed${result.removedDirs > 0 ? `, ${result.removedDirs} folder(s) removed` : ""}${result.removedLinks > 0 ? `, ${result.removedLinks} link(s) removed` : ""}.`);
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
          setStatusText("Folder removed and the project was forgotten from Code Hangar.");
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

  const runMutationFinalRemove = useCallback(async (entryId: number) => {
    // Final removal is OFF by default; it is available only after the user opts in through Recover.
    // The backend enforces the same gate and still requires a verified backup that covers the file;
    // this is just the matching UI backstop.
    if (!finalRemoveEnabled) {
      setMutationMessage("Final removal is turned off. Enable it in Recover to use it.");
      return;
    }
    if (!(await requestConfirm(
      "Final remove is irreversible and removes the stored copy from disk. Continue only after backup/review.",
      { confirmLabel: "Remove permanently", tone: "danger" }
    ))) return;
    setMutationBusy(true);
    try {
      const token = (await api.mutationTokenIssue("final_remove")).token;
      const result = await api.mutationFinalRemoveStart(entryId, token);
      setMutationMessage(`Final remove recorded ${formatBytes(result.freedBytes)} as freed from stored entry ${entryId}.`);
      await refreshMutationActivity();
    } catch (error) {
      setMutationMessage(`Final remove failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setMutationBusy(false);
    }
  }, [refreshMutationActivity, finalRemoveEnabled, requestConfirm]);

  const toggleFinalRemove = useCallback(async (enabled: boolean) => {
    // Final removal is OFF by default. Enabling it arms the irreversible action, so it asks for a
    // confirmation; turning it OFF is always allowed without one. Enabling never deletes anything
    // by itself — every final removal still requires a verified backup that covers the file and a
    // fresh confirmation. The backend setting is the enforcement.
    if (enabled) {
      if (!(await requestConfirm(
        "Enable final removal?\n\nThis makes Code Hangar's irreversible Final-remove action available. Every removal still requires a verified backup that covers the file and a fresh confirmation. Continue?",
        { confirmLabel: "Enable final removal", tone: "danger" }
      ))) {
        return;
      }
    }
    try {
      await api.mutationSetFinalRemoveEnabled(enabled);
      setFinalRemoveEnabled(enabled);
      setMutationMessage(enabled
        ? "Final removal is on. Each removal still needs a verified backup and a fresh confirmation."
        : "Final removal turned off.");
    } catch (error) {
      setMutationMessage(`Could not change the final-remove setting: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [requestConfirm]);

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
  }, [documentKind, documentLimit, documentNameFilter, documentPathFilter, documentQuery, documentScope, selectedProjectId]);

  useEffect(() => {
    setOrphanSearchError(null);
  }, [lostKeyword, lostSignals, lostStalePreset, orphanAssetKind, orphanCustomMiB, orphanIncludePartial, orphanMinConfidence, orphanMinPreset, orphanMode, orphanScope, selectedProjectId]);

  useEffect(() => {
    setDuplicateSearchError(null);
  }, [duplicateCustomMiB, duplicateFileKind, duplicateLimit, duplicateMinPreset, duplicateScope, preview?.nodeId, selectedProjectId]);

  const runDocumentSearch = useCallback(async () => {
    const searchSeq = documentSearchSeq.current + 1;
    documentSearchSeq.current = searchSeq;
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
    if (documentScope === "current" && selectedProjectId == null) {
      setDocumentSearching(false);
      setDocumentHits([]);
      setDocumentSearchRan(false);
      setDocumentSearchCriteria(null);
      setDocumentSearchTruncated(false);
      setDocumentSearchDuration(null);
      setDocumentSearchError("Choose a project before searching Current project, or switch Scope to All projects.");
      setStatusText("Choose a project before searching the current project, or switch scope to All projects.");
      return;
    }
    setDocumentHits([]);
    setDocumentSearchTruncated(false);
    setDocumentSearchDuration(null);
    setDocumentSearchError(null);
    setDocumentSearching(true);
    setDocumentSearchRan(true);
    setDocumentSearchCriteria(currentDocumentSearchCriteria);
    await yieldToUi();
    try {
      const result = await api.searchDocuments({
        query: documentQuery,
        projectId: documentScope === "current" ? selectedProjectId : null,
        indexedKind: documentKind,
        pathFilter: documentPathFilter,
        nameFilter: documentNameFilter,
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
  }, [currentDocumentSearchCriteria, demosVisible, documentKind, documentLimit, documentNameFilter, documentPathFilter, documentQuery, documentScope, performanceMode, selectedProjectId]);

  const runProjectDiscovery = useCallback(async (
    limit = 100,
    kind: "projects" | "sessions" = "projects",
    includeTechnicalCandidates = false
  ) => {
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
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
        persistInventoryIncludeOptions(true, true);
      }
      setStatusText(kind === "sessions"
        ? `Session discovery found ${result.totalSessions} local conversation${result.totalSessions === 1 ? "" : "s"}, including project-linked, standalone and agent sessions.`
        : `Project discovery found ${result.totalCandidates} project candidate${result.totalCandidates === 1 ? "" : "s"} and ${result.totalSessions} linked local session${result.totalSessions === 1 ? "" : "s"}.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setProjectDiscoveryError(message);
      setStatusText(`Project discovery failed: ${message}`);
    } finally {
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
      setRoots((current) => current.some((item) => item.id === root.id) ? current : [...current, root]);
      const loadedProjects = await api.projectsListLite();
      setProjects(loadedProjects);
      setProjectsFromCache(false);
      setProjectDiscoveryReport((current) => current ? {
        ...current,
        candidates: current.candidates.map((item) => item.path === candidate.path ? {
          ...item,
          alreadyRegistered: true,
          existingProjectId: loadedProjects.find((project) => project.scanRootId === root.id || project.path === root.path)?.id ?? item.existingProjectId ?? null,
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
      setStatusText(`${candidate.displayName} added to Projects. Run scan when you want to inventory it.`);
      void refreshSideData();
    } catch (error) {
      setStatusText(`Could not add discovered project: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [refreshSideData]);

  const addVisibleDiscoveryCandidatesAsRoots = useCallback(async (candidates: ProjectDiscoveryCandidate[]) => {
    const addable = candidates.filter((candidate) => !candidate.alreadyRegistered && candidate.overlapKind === "none");
    if (addable.length === 0) {
      setStatusText("No visible project candidates can be added. Registered and overlapping roots are skipped.");
      return;
    }
    const addedPaths = new Set<string>();
    const addedRoots: typeof roots = [];
    try {
      for (const candidate of addable) {
        const root = await api.rootsAdd(candidate.path);
        addedPaths.add(candidate.path);
        addedRoots.push(root);
      }
      setRoots((current) => {
        const known = new Set(current.map((root) => root.id));
        const next = [...current];
        for (const root of addedRoots) {
          if (!known.has(root.id)) next.push(root);
        }
        return next;
      });
      const loadedProjects = await api.projectsListLite();
      setProjects(loadedProjects);
      setProjectsFromCache(false);
      setProjectDiscoveryReport((current) => current ? {
        ...current,
        candidates: current.candidates.map((item) => addedPaths.has(item.path) ? {
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
      setStatusText(`Added ${addedPaths.size} visible project candidate${addedPaths.size === 1 ? "" : "s"} to Projects. Run scan when you want to inventory them.`);
      void refreshSideData();
    } catch (error) {
      setStatusText(`Could not add all visible projects: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [refreshSideData]);

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
    if (!preview) {
      setStatusText("Open a file before evaluating orphan status.");
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
      const status = await api.nodeOrphanStatus(nodeId);
      if (searchSeq !== fileOrphanSeq.current || expectedSelectionSeq !== selectionSeq.current) return;
      setOrphanStatusByNode((current) => ({ ...current, [nodeId]: status }));
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
    selectedProjectId
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
    if (!preview) {
      setStatusText("Open a file before searching duplicates for it.");
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
  }, [loadDuplicateCandidates, preview, pushWorkspaceRoute]);

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
    setPlanLoading(true);
    setPlanJobStatus(null);
    setOperationPlan(null);
    setRiskReport(null);
    try {
      const jobId = await api.operationPlanStart(targetNodeId, "Read-only local review", performanceMode);
      setPlanJobId(jobId);
      setStatusText("Safe Manage review started. You can keep using the UI or stop the load.");
    } catch (error) {
      setStatusText(`Safe Manage review failed: ${error instanceof Error ? error.message : String(error)}`);
      setPlanLoading(false);
    }
  }, [performanceMode, planLoading, planTargetNode, selectedProjectId]);

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
      await loadProjectsLite();
      await yieldToUi();
      await refreshSideData();
      await yieldToUi();
      window.setTimeout(() => {
        void (async () => {
          try {
            const fullProjects = await api.projectsList();
            setProjects(fullProjects);
            setProjectsFromCache(false);
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
  }, [loadDashboardData, loadProjectData, loadProjectsLite, primaryView, refreshSideData, rightPaneView, selectedProjectId]);

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
    // Routes describe the workspace beneath any open session pane; navigating
    // history must dismiss the session or the center pane would keep showing it.
    setPreviewSession(null);
    if (route.selectedProjectId !== selectedProjectId) {
      if (planJobId) void api.operationPlanCancel(planJobId);
      selectionSeq.current += 1;
      beginProject(route.selectedProjectId);
      manualPreviewClearProjectRef.current = null;
      setFolderExplanation(null);
      setRelationships(null);
      setRelationshipsNodeId(null);
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
      setSelectedProjectId(route.selectedProjectId);
    }
    setPrimaryView(route.primaryView);
    setProjectSidebarFocus(route.primaryView === "project" || route.primaryView === "review");
    setProjectView(route.projectView);
    setDiscoverView(route.discoverView);
    if (route.primaryView === "discover") {
      prepareDiscoverScope(route.discoverView, route.selectedProjectId);
    }
    if (route.discoverView === "lost" || route.discoverView === "assets") {
      setOrphanMode(route.discoverView);
    }
    setSettingsView(route.settingsView);
    setRightPaneView(route.rightPaneView);
    setStatusText(workspaceRouteStatusText(route));
  }, [beginProject, planJobId, prepareDiscoverScope, selectedProjectId]);

  const selectProject = useCallback((projectId: number) => {
    setPreviewSession(null);
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({
      primaryView: "project",
      projectView: "context",
      rightPaneView: "inspector",
      selectedProjectId: projectId
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
    setRelationshipsNodeId(null);
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
  }, [beginProject, loadProjectData, planJobId, projectWorkspace.loadStatus, pushWorkspaceRoute, selectedProjectId]);

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

  const showOverview = useCallback(() => {
    dashboardAutoLoadAttemptedRef.current = false;
    pushWorkspaceRoute({ primaryView: "overview", rightPaneView: "dashboard" });
    setPrimaryView("overview");
    setRightPaneView("dashboard");
    setStatusText(workspaceRouteStatusText({ primaryView: "overview" }));
  }, [pushWorkspaceRoute]);

  const showProjectWorkspace = useCallback((view: ProjectView = "context") => {
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({ primaryView: "project", projectView: view, rightPaneView: "inspector" });
    setPrimaryView("project");
    setProjectView(view);
    setRightPaneView("inspector");
    setStatusText(workspaceRouteStatusText({ primaryView: "project", projectView: view }));
  }, [pushWorkspaceRoute]);

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
  }, [prepareDiscoverScope, pushWorkspaceRoute, selectedProjectId]);

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

  const showReview = useCallback(() => {
    setProjectSidebarFocus(true);
    pushWorkspaceRoute({ primaryView: "review", rightPaneView: "inspector" });
    setPrimaryView("review");
    setRightPaneView("inspector");
    setStatusText(workspaceRouteStatusText({ primaryView: "review" }));
  }, [pushWorkspaceRoute]);

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
    pushWorkspaceRoute({ primaryView: "recovery", rightPaneView: "activity" });
    setPrimaryView("recovery");
    setRightPaneView("activity");
    setStatusText(workspaceRouteStatusText({ primaryView: "recovery" }));
    void refreshMutationActivity();
  }, [pushWorkspaceRoute, refreshMutationActivity]);

  const showSettings = useCallback((view: SettingsView) => {
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
  }, [pushWorkspaceRoute]);

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
    setRelationshipsNodeId(null);
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
      setStatusText(explanation ? `Explaining folder ${explanation.displayName}` : `No folder explanation for ${item.displayName}`);
    } catch (error) {
      if (folderSelectionSeq !== selectionSeq.current) return;
      setStatusText(error instanceof Error ? error.message : "Could not explain folder.");
    }
  }, []);

  const loadRelationshipsInBackground = useCallback((nodeId: number, expectedSeq: number, label: string) => {
    setRelationshipsLoading(true);
    void api.nodeRelationships(nodeId)
      .then((nextRelationships) => {
        if (expectedSeq !== selectionSeq.current) return;
        setRelationships(nextRelationships);
        setRelationshipsNodeId(nodeId);
      })
      .catch((error) => {
        if (expectedSeq !== selectionSeq.current) return;
        setRelationships(null);
        setRelationshipsNodeId(null);
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
      const cacheKey = `${nodeId}:${requestedMode}`;
      const cached = previewCacheRef.current.get(cacheKey);
      setRelationships(null);
      setRelationshipsNodeId(null);
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
          nextPreview = await api.filePreview(nodeId, requestedMode === "edit" ? "source" : requestedMode, recordRecent, previewPolicy);
        } catch (error) {
          if (openSelectionSeq === selectionSeq.current) {
            setRelationshipsLoading(false);
            setStatusText(`Open failed: ${error instanceof Error ? error.message : String(error)}`);
          }
          return false;
        }
        if (openSelectionSeq !== selectionSeq.current) return false;
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
        if (current && current.nodeId !== nodeId && !options?.replaceHistory) {
          setBackStack((stack) => [...stack, current.nodeId].slice(-50));
          setForwardStack([]);
        }
        return nextPreview;
      });
      setTabs((current) => {
        if (current.some((tab) => tab.nodeId === nodeId)) return current;
        return [
          ...current,
          { nodeId, projectId: nextPreview.projectId, label: nextPreview.displayName, path: nextPreview.displayPath || nextPreview.path }
        ].slice(-8);
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
          ...current.filter((item) => item.nodeId !== nodeId)
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
          .filePreview(nodeId, requestedMode, recordRecent, previewPolicy)
          .then((fresh) => {
            previewCacheRef.current.set(cacheKey, fresh);
            if (openSelectionSeq === selectionSeq.current) {
              setPreview((current) => (current && current.nodeId === fresh.nodeId ? fresh : current));
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

  const openNodeInTree = useCallback(async (
    nodeId: number,
    options?: OpenNodeOptions & { projectId?: number | null }
  ) => {
    const targetProjectId = options?.projectId ?? selectedProjectId;
    if (targetProjectId) pendingTreeRevealRef.current = { projectId: targetProjectId, nodeId };
    const { projectId: _projectId, ...openOptions } = options ?? {};
    const opened = await openNode(nodeId, openOptions);
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
    // Record the origin screen so Back returns to it (the route itself does not
    // encode the open session — applying a route dismisses it).
    pushWorkspaceRoute({}, { recordCurrent: true });
    setProjectSidebarFocus(true);
    setPreviewSession(session);
    setPrimaryView("project");
    setRightPaneView("inspector");
    setStatusText(`Opening session ${session.displayName}.`);
  }, [pushWorkspaceRoute]);

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
    const next = await api.watcherStatus(selectedProjectId, preview?.nodeId ?? null);
    setWatcherStatus(next);
    const currentNode = next.focused?.currentNode ?? null;
    if (
      preview
      && currentNode
      && currentNode.nodeId === preview.nodeId
      && currentNode.state === "changed"
      && (currentNode.isMarkdown || currentNode.isContext)
    ) {
      const refreshKey = `${currentNode.nodeId}:${currentNode.liveMtime ?? ""}:${currentNode.liveSize ?? ""}`;
      if (watcherPreviewRefreshRef.current !== refreshKey) {
        watcherPreviewRefreshRef.current = refreshKey;
        setBackgroundStatus(`Refreshing preview because ${currentNode.displayName} changed on disk.`);
        // refreshOnly: a background poll must never switch views or routes — it
        // would yank the user out of Overview/Discover/Settings mid-read.
        await openNode(currentNode.nodeId, { recordRecent: false, replaceHistory: true, refreshOnly: true });
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
    const confirmed = await requestConfirm(
      `Reveal ${preview.displayName} transiently in this session? The content will not be indexed or persisted.`,
      { confirmLabel: "Reveal file" }
    );
    if (!confirmed) return;
    const revealSelectionSeq = selectionSeq.current;
    setRelationships(null);
    setRelationshipsNodeId(null);
    setRelationshipsLoading(false);
    setStatusText("Revealing preview for this session.");
    await yieldToUi();
    if (revealSelectionSeq !== selectionSeq.current) return;
    let nextPreview: FilePreview;
    try {
      nextPreview = await api.fileReveal(preview.nodeId, previewMode === "edit" ? "source" : previewMode, previewPolicy);
    } catch (error) {
      if (revealSelectionSeq === selectionSeq.current) {
        setRelationshipsLoading(false);
        setStatusText(`Reveal failed: ${error instanceof Error ? error.message : String(error)}`);
      }
      return;
    }
    if (revealSelectionSeq !== selectionSeq.current) return;
    setPreview(nextPreview);
    setTabs((current) => {
      if (current.some((tab) => tab.nodeId === nextPreview.nodeId)) return current;
      return [
        ...current,
        {
          nodeId: nextPreview.nodeId,
          projectId: nextPreview.projectId,
          label: nextPreview.displayName,
          path: nextPreview.displayPath || nextPreview.path
        }
      ].slice(-8);
    });
    setStatusText(nextPreview.state === "ready" ? `Revealed ${nextPreview.displayName}` : nextPreview.blockedReason ?? "Reveal unavailable");
  }, [preview, previewMode, previewPolicy, requestConfirm]);

  const activateAfterTabRemoval = useCallback(
    async (nextTabs: OpenTab[], replacementIndex: number, removedActiveTab: boolean) => {
      setTabs(nextTabs);
      if (!removedActiveTab) return;
      const replacement = nextTabs[Math.min(Math.max(replacementIndex, 0), nextTabs.length - 1)];
      if (replacement) {
        await openNode(replacement.nodeId, { replaceHistory: true });
      } else {
        manualPreviewClearProjectRef.current = selectedProjectId;
        setPreview(null);
        setRelationships(null);
        setRelationshipsNodeId(null);
        setRelationshipsLoading(false);
        setFileOrphanLoading(false);
      }
    },
    [openNode, selectedProjectId]
  );

  const closeTab = useCallback(
    async (nodeId: number) => {
      const index = tabs.findIndex((tab) => tab.nodeId === nodeId);
      if (index === -1) return;
      const nextTabs = tabs.filter((tab) => tab.nodeId !== nodeId);
      await activateAfterTabRemoval(nextTabs, index, preview?.nodeId === nodeId);
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeOtherTabs = useCallback(
    async (nodeId: number) => {
      const tab = tabs.find((candidate) => candidate.nodeId === nodeId);
      if (!tab) return;
      await activateAfterTabRemoval([tab], 0, preview ? preview.nodeId !== nodeId : false);
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeAllTabs = useCallback(() => {
    manualPreviewClearProjectRef.current = selectedProjectId;
    setTabs([]);
    setPreview(null);
    setRelationships(null);
    setRelationshipsNodeId(null);
    setRelationshipsLoading(false);
    setFileOrphanLoading(false);
  }, [selectedProjectId]);

  const closeTabsToSide = useCallback(
    async (nodeId: number, side: "left" | "right") => {
      const index = tabs.findIndex((tab) => tab.nodeId === nodeId);
      if (index === -1) return;
      const nextTabs = tabs.filter((_, tabIndex) => (side === "left" ? tabIndex >= index : tabIndex <= index));
      const removedActiveTab = preview ? !nextTabs.some((tab) => tab.nodeId === preview.nodeId) : false;
      await activateAfterTabRemoval(nextTabs, side === "left" ? 0 : index, removedActiveTab);
    },
    [activateAfterTabRemoval, preview, tabs]
  );

  const closeTabsOutsideProject = useCallback(
    async (projectId: number, preferredNodeId: number) => {
      const nextTabs = tabs.filter((tab) => tab.projectId === projectId);
      const preferredIndex = Math.max(nextTabs.findIndex((tab) => tab.nodeId === preferredNodeId), 0);
      const removedActiveTab = preview ? !nextTabs.some((tab) => tab.nodeId === preview.nodeId) : false;
      await activateAfterTabRemoval(nextTabs, preferredIndex, removedActiveTab);
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

  const copyNodePath = useCallback(async (nodeId: number | null | undefined, fallbackPath: string) => {
    if (!nodeId) {
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
    if (!nodeId) return;
    try {
      await api.openNodeExternal(nodeId);
      setStatusText("Opening path with Windows.");
    } catch (error) {
      setStatusText(`Open failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, []);

  const revealNodeWithSystem = useCallback(async (nodeId: number | null | undefined) => {
    if (!nodeId) return;
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
    const finishStartup = () => {
      window.setTimeout(() => {
        if (!cancelled) {
          setStartupProgress((current) => ({ ...current, active: false }));
        }
      }, 450);
    };

    const runStartup = async () => {
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
          setProjects(cachedProjects);
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
        await yieldToUi();
        if (cancelled) return;
        const loadedProjects = await api.projectsListLite();
        if (cancelled) return;
        setProjects(loadedProjects);
        setProjectsFromCache(false);
        setStartupProgress({
          active: true,
          label: "Navigation ready",
          detail: "Projects are visible. Choose a project when you want to load its files.",
          progress: 62
        });
        setStatusText(`Loaded ${loadedProjects.length} projects. Choose one to load its files.`);
        finishStartup();
        // This tutorial revision runs once per installed edition. Empty inventories
        // continue to hand off to Add Projects after the walkthrough.
        const inventoryHasRealProjects = !loadedProjects.every((project) => isDemoProject(project));
        const tutorialSeen = window.localStorage.getItem(tutorialStorageKey) === "1";
        let hydratedFromCache = false;
        if (!tutorialSeen) {
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
        setStatusText(sideDataWarning ? `Local inventory ready. ${sideDataWarning}` : "Local inventory ready.");

        window.setTimeout(() => {
          void (async () => {
            try {
              const completeProjects = await api.projectsList();
              if (cancelled) return;
              setProjects(completeProjects);
              setProjectsFromCache(false);
            } catch (error) {
              if (!cancelled) {
                setStatusText(`Project metadata refresh failed: ${error instanceof Error ? error.message : String(error)}`);
              }
            }
          })();
        }, 900);

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
                  include?.agents ?? deepScanIncludeAgents
                );
                if (cancelled) return;
                setProjectDiscoveryReport(restored);
                setSessionInventory(restored.sessions);
                const grouped = restored.sessions.length;
                if (!hydratedFromCache && grouped > 0) {
                  setStatusText(`Restored ${grouped} session${grouped === 1 ? "" : "s"} and project grouping.`);
                }
              } catch {
                // A failed refresh is non-fatal: the cached (or empty) grouping
                // stays, and a Deep Scan can always rebuild it. Stay quiet.
              } finally {
                if (!cancelled && !hydratedFromCache) setBackgroundStatus(null);
              }
            })();
          }, 1200);
        }
      } catch (error) {
        if (cancelled) return;
        const message = error instanceof Error ? error.message : String(error);
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
    if (!selectedProjectId) return;
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
    setRelationshipsNodeId(null);
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
    void openNode(initialContext.nodeId, INITIAL_CONTEXT_OPEN_OPTIONS);
  }, [contextFiles, folderExplanation, openNode, preview, projectView, projectWorkspace.loadStatus, selectedProjectId]);

  useEffect(() => {
    if (!preview) return;
    const expectedNodeId = preview.nodeId;
    const expectedSeq = selectionSeq.current;
    // "edit" is a frontend-only view; the backend still serves the file's source for it.
    const backendMode = previewMode === "edit" || previewMode === "values" ? "source" : previewMode;
    const next = preview.wasRevealed && previewPolicy.allowSensitiveReveal
      ? api.fileReveal(preview.nodeId, backendMode, previewPolicy)
      : api.filePreview(preview.nodeId, backendMode, false, previewPolicy);
    void next
      .then((nextPreview) => {
        if (expectedSeq !== selectionSeq.current || nextPreview.nodeId !== expectedNodeId) return;
        setPreview(nextPreview);
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

  const invalidatePreviewCache = useCallback((nodeId: number) => {
    for (const mode of ["rendered", "source", "edit", "values"]) {
      previewCacheRef.current.delete(`${nodeId}:${mode}`);
    }
  }, []);

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

  const openRewrite = useCallback((target: RewriteTarget, name: string) => {
    if (!changesUnlocked) {
      requestChangeAccess();
      setStatusText("Project file changes are locked. Unlock them before preparing an AI rewrite.");
      return;
    }
    setRewriteTarget(target);
    setRewriteFileName(name);
  }, [changesUnlocked, requestChangeAccess]);

  // Run the rewrite. The configured provider model is used (empty override); the dialog supplies
  // the optional instruction + level. Returns the rewritten text — it is NOT written here.
  const runRewrite = useCallback(async (instruction: string, level: string): Promise<AiRewriteProposal> => {
    if (!rewriteTarget) throw new Error("No selected passage is staged.");
    const connectorApi = await requireConnectorApi();
    return connectorApi.aiRewriteText(rewriteTarget.nodeId, rewriteTarget.snippet, instruction, level, "");
  }, [rewriteTarget]);

  const applyRewriteSuggestion = useCallback(async (proposalId: string): Promise<AiSuggestionApplyResult> => {
    if (!rewriteTarget) throw new Error("No selected passage is staged.");
    if (!changesUnlocked) throw new Error("Project file changes are locked. Unlock this project and review the suggestion again.");
    const connectorApi = await requireConnectorApi();
    const result = await connectorApi.applyAiSuggestion(proposalId);
    invalidatePreviewCache(result.nodeId);
    if (editDraftNodeRef.current === result.nodeId) {
      editDraftNodeRef.current = null;
      setEditDraft(null);
      setEditUndo(null);
    }
    await openNode(result.nodeId, { mode: "source", recordRecent: false, refreshOnly: true, replaceHistory: true, allowProjectSwitch: false });
    setStatusText(result.message);
    return result;
  }, [changesUnlocked, rewriteTarget, invalidatePreviewCache, openNode]);

  const undoRewriteSession = useCallback(async (nodeId: number, sessionId: string) => {
    if (!changesUnlocked) throw new Error("Project file changes are locked. Unlock this project before restoring an AI edit session.");
    if (!(await requestConfirm(
      "Undo this AI edit session? This restores the verified file version from before that session's first edit. The current file is checked before anything changes.",
      { confirmLabel: "Undo AI edit session", tone: "danger" }
    ))) throw new Error("Undo cancelled. No file changed.");
    const connectorApi = await requireConnectorApi();
    const result = await connectorApi.undoAiEditSession(nodeId, sessionId);
    invalidatePreviewCache(result.nodeId);
    await openNode(result.nodeId, { mode: "source", recordRecent: false, refreshOnly: true, replaceHistory: true, allowProjectSwitch: false });
    setStatusText(result.message);
    return result;
  }, [changesUnlocked, invalidatePreviewCache, openNode, requestConfirm]);

  useEffect(() => {
    if (projectView !== "connections" || !preview) return;
    if (relationshipsLoading || relationshipsNodeId === preview.nodeId) return;
    loadRelationshipsInBackground(preview.nodeId, selectionSeq.current, preview.displayName);
  }, [loadRelationshipsInBackground, preview, projectView, relationshipsLoading, relationshipsNodeId]);

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
    const route = viewBackStack.at(-1);
    if (route) {
      const current = currentWorkspaceRoute();
      setViewBackStack((stack) => stack.slice(0, -1));
      setViewForwardStack((stack) => [...stack, current]);
      applyWorkspaceRoute(route);
      return;
    }
    const nodeId = backStack.at(-1);
    if (!nodeId) return;
    setBackStack((stack) => stack.slice(0, -1));
    // With no open preview (e.g. after closing all tabs) there is nothing to put on
    // the forward stack — still reopen the last file instead of a dead Back button.
    if (preview) setForwardStack((stack) => [...stack, preview.nodeId]);
    await openNode(nodeId, { replaceHistory: true });
  }, [applyWorkspaceRoute, backStack, currentWorkspaceRoute, openNode, preview, viewBackStack]);

  const goForward = useCallback(async () => {
    const route = viewForwardStack.at(-1);
    if (route) {
      const current = currentWorkspaceRoute();
      setViewForwardStack((stack) => stack.slice(0, -1));
      setViewBackStack((stack) => [...stack, current]);
      applyWorkspaceRoute(route);
      return;
    }
    const nodeId = forwardStack.at(-1);
    if (!nodeId) return;
    setForwardStack((stack) => stack.slice(0, -1));
    if (preview) setBackStack((stack) => [...stack, preview.nodeId]);
    await openNode(nodeId, { replaceHistory: true });
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
    rewrite: rewriteTarget !== null,
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

  const updateFilePin = useCallback(async (nodeId: number, label: string, currentlyPinned: boolean) => {
    const nextPinned = !currentlyPinned;
    try {
      if (currentlyPinned) await api.unpinItem(nodeId, "file");
      else await api.pinItem(nodeId, "file");
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
    if (!preview) return;
    await updateFilePin(preview.nodeId, preview.displayName, selectedPinned);
  }, [preview, selectedPinned, updateFilePin]);

  // Auto-register the strong, deliberate candidates from a discovery report —
  // projects an AI app already lists in its registry, or folders a local session
  // has worked in — that aren't already registered and don't overlap a root,
  // then start scanning them. Returns how many were added. Weaker candidates are
  // left in the report for manual review. Shared by the one-click global Deep
  // Scan and the folder-scoped search.
  const autoRegisterStrongCandidates = useCallback(
    async (result: ProjectDiscoveryReport): Promise<{ addedCount: number; jobId: string | null }> => {
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
      if (autoAdd.length === 0) return { addedCount: 0, jobId: null };
      const addedRoots: typeof roots = [];
      for (const candidate of autoAdd) {
        try {
          addedRoots.push(await api.rootsAdd(candidate.path));
        } catch {
          /* skip candidates that fail to register; keep going */
        }
      }
      if (addedRoots.length === 0) return { addedCount: 0, jobId: null };
      const addedPaths = new Set(autoAdd.map((candidate) => candidate.path));
      setRoots((current) => {
        const known = new Set(current.map((root) => root.id));
        return [...current, ...addedRoots.filter((root) => !known.has(root.id))];
      });
      const loaded = await api.projectsListLite();
      setProjects(loaded);
      setProjectsFromCache(false);
      setProjectDiscoveryReport((current) => current ? {
        ...current,
        candidates: current.candidates.map((item) => addedPaths.has(item.path) ? {
          ...item,
          alreadyRegistered: true,
          existingProjectId: loaded.find((project) => project.path === item.path)?.id ?? item.existingProjectId ?? null,
          sourceKinds: Array.from(new Set([...item.sourceKinds, "code_hangar_registered"]))
        } : item)
      } : current);
      const jobId = await api.scanStart(addedRoots.map((root) => root.id), performanceMode);
      const status = await api.scanStatus(jobId);
      setScanStatus(status);
      void refreshSideData();
      return { addedCount: addedRoots.length, jobId };
    },
    [performanceMode, refreshSideData, setScanStatus]
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
    setAddProjectsVisible(false);
    setDeepScanOverlayVisible(true);
    showDiscover("projects");
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
    setProjectDiscoveryReport(null);
    // Apply the WSL opt-in before scanning: this both persists the choice and flips
    // the discovery runtime gate, so WSL is only ever touched when the user asked.
    try {
      await api.setWslScanEnabled(wslScanChoice);
    } catch {
      // Non-fatal: a persistence hiccup shouldn't block the Windows-side scan.
    }
    setDeepScanProgress({
      stages: initialDeepScanStages(installedApps, wslScanChoice),
      phase: "scanning",
      projectsFound: 0,
      sessionsFound: 0,
      addedCount: 0,
      note: wslScanChoice
        ? "Reading local AI app registries across Windows and WSL…"
        : "Reading local AI app registries on this PC…"
    });
    try {
      const result = await api.projectDiscoveryReport(500, deepScanIncludeLoose, deepScanIncludeAgents);
      setProjectDiscoveryReport(result);
      // Populate the sidebar's session inventory too, so a Deep Scan surfaces the
      // sessions (grouped under their projects) — not just the project list.
      setSessionInventory(result.sessions);
      persistInventoryIncludeOptions(deepScanIncludeLoose, deepScanIncludeAgents);
      setDeepScanProgress((current) => current ? {
        ...current,
        stages: current.stages.map((stage) => ({ ...stage, done: true })),
        phase: "registering",
        projectsFound: result.totalCandidates,
        sessionsFound: result.totalSessions,
        note: "Adding the projects your AI apps already know…"
      } : current);
      const { addedCount, jobId } = await autoRegisterStrongCandidates(result);
      // The first discovery ran before these projects were registered, so its
      // sessions only knew them as "not added yet" (no registered-id link). Now
      // that they're registered, re-read discovery in the background so every
      // session links to its project — grouping the sidebar sessions under their
      // projects and driving the Active/Archived split. Cheap relative to the
      // inventory scan already running, and it never blocks the progress panel.
      if (addedCount > 0) {
        void api
          .projectDiscoveryReport(500, deepScanIncludeLoose, deepScanIncludeAgents)
          .then((linked) => {
            setProjectDiscoveryReport(linked);
            setSessionInventory(linked.sessions);
          })
          .catch(() => {
            /* keep the pre-registration view if the refresh fails */
          });
      }
      const reviewable = result.totalCandidates - addedCount;
      setStatusText(
        addedCount > 0
          ? `Deep Scan added ${addedCount} project${addedCount === 1 ? "" : "s"} automatically and is scanning them.${reviewable > 0 ? ` ${reviewable} more candidate${reviewable === 1 ? "" : "s"} listed for review.` : ""}`
          : `Deep Scan mapped ${result.totalCandidates} project candidate${result.totalCandidates === 1 ? "" : "s"}. Review before adding.`
      );
      if (jobId) {
        // Carry the loved overlay straight into a rewarding "building inventory"
        // phase. It shows live scan progress and stays until the scan finishes (an
        // effect dismisses it) — or the user hides it to keep working meanwhile.
        setDeepScanProgress((current) => current ? {
          ...current,
          phase: "building",
          addedCount,
          scanJobId: jobId,
          note: addedCount === 1
            ? "Indexing 1 project so its files and context are ready."
            : `Indexing ${addedCount} projects so their files and context are ready.`
        } : current);
      } else {
        setDeepScanProgress((current) => current ? {
          ...current,
          phase: "done",
          addedCount,
          note: addedCount > 0
            ? `Added ${addedCount} automatically${reviewable > 0 ? `, ${reviewable} more to review` : ""}.`
            : "Review the candidates below and add the ones you want."
        } : current);
      }
    } catch (error) {
      setDeepScanProgress(null);
      setDeepScanOverlayVisible(false);
      const message = error instanceof Error ? error.message : String(error);
      setProjectDiscoveryError(message);
      setStatusText(`Deep Scan failed: ${message}`);
    } finally {
      setProjectDiscoveryLoading(false);
    }
  }, [autoRegisterStrongCandidates, deepScanIncludeAgents, deepScanIncludeLoose, deepScanProgress, installedApps, projectDiscoveryLoading, wslScanChoice, showDiscover]);

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
    setAddProjectsVisible(false);
    setProjectDiscoveryLoading(true);
    setProjectDiscoveryError(null);
    setProjectDiscoveryReport(null);
    showDiscover("projects");
    setStatusText(`Searching ${folder} for projects. Strong matches are added automatically.`);
    await yieldToUi();
    try {
      const result = await api.projectDiscoveryDeepScan(folder, 500, deepScanIncludeLoose, deepScanIncludeAgents);
      setProjectDiscoveryReport(result);
      const { addedCount } = await autoRegisterStrongCandidates(result);
      const reviewable = result.totalCandidates - addedCount;
      setStatusText(
        addedCount > 0
          ? `Added ${addedCount} project${addedCount === 1 ? "" : "s"} automatically under ${folder} and is scanning them.${reviewable > 0 ? ` ${reviewable} more candidate${reviewable === 1 ? "" : "s"} below need review.` : ""}`
          : `Found ${result.totalCandidates} candidate${result.totalCandidates === 1 ? "" : "s"} under ${folder}. Review before adding.`
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setProjectDiscoveryError(message);
      setStatusText(`Search failed: ${message}`);
    } finally {
      setProjectDiscoveryLoading(false);
    }
  }, [autoRegisterStrongCandidates, deepScanIncludeAgents, deepScanIncludeLoose, showDiscover]);

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
        setRelationshipsNodeId(null);
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
        setRelationshipsNodeId(null);
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
          setStatusText(`Removed ${project.name} from ${apps}. Backed up — use Undo or Recover to restore.`);
          setAppRemovalUndo({ name: project.name, id: removal.id });
        } else {
          setStatusText(`Removed ${project.name} from ${apps}. A backup copy is kept; recover the project from Recover.`);
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
      const isPinned = pinnedItems.some((pinned) => pinned.nodeId === item.nodeId && pinned.itemKind === "file");
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
          ...(connectorBuild && capabilities.canUseAi
            ? [
                {
                  id: "explain-code",
                  label: "Explain file with AI",
                  section: "AI tools",
                  help: "Open a plain-language explanation of this file. Code Hangar checks the file for sensitive content and secrets before anything can be sent.",
                  icon: <Sparkles size={15} />,
                  onSelect: () => openExplain({ kind: "file", nodeId: item.nodeId, path: item.path, initialLens: "explain" as const })
                },
                {
                  id: "review-code",
                  label: "Check file for risks with AI",
                  help: "Ask your configured AI to build a read-only checklist of possible mistakes and questions. This cannot edit the file or run the project.",
                  icon: <ListChecks size={15} />,
                  onSelect: () => openExplain({ kind: "file", nodeId: item.nodeId, path: item.path, initialLens: "review" as const })
                }
              ]
            : []),
          ...(!capabilities.isDirectory && !capabilities.isLink ? [{
            id: "pin",
            label: isPinned ? "Unpin" : "Pin",
            section: "More tools",
            help: isPinned ? `Remove ${item.label} from Pinned.` : `Keep ${item.label} in Pinned for quick access.`,
            icon: isPinned ? <PinOff size={15} /> : <Pin size={15} />,
            onSelect: () => updateFilePin(item.nodeId, item.label, isPinned)
          }] : []),
          {
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
          },
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
    [buildPreviewPlan, connectorBuild, copyNodePath, openExplain, openNodeInTree, openNodeWithSystem, pinnedItems, revealNodeWithSystem, selectProject, showReview, updateFilePin]
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
      const isPinned = Boolean(item.nodeId && pinnedItems.some((pinned) => pinned.nodeId === item.nodeId && pinned.itemKind === "file"));
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
                  id: "explain-folder",
                  label: "Explain this folder",
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
                onSelect: () => item.nodeId ? void openNode(item.nodeId) : undefined
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
            onSelect: () => item.nodeId ? void openNode(item.nodeId, { mode: "source" }) : undefined
          }] : []),
          ...(connectorBuild && item.nodeId && capabilities.canUseAi
            ? [
                {
                  id: "explain-code",
                  label: "Explain file with AI",
                  section: "AI tools",
                  help: "Open a plain-language explanation of this file. Code Hangar checks the file for sensitive content and secrets before anything can be sent.",
                  icon: <Sparkles size={15} />,
                  onSelect: () => {
                    if (item.nodeId) openExplain({ kind: "file", nodeId: item.nodeId, path, initialLens: "explain" as const });
                  }
                },
                {
                  id: "review-code",
                  label: "Check file for risks with AI",
                  help: "Ask your configured AI to build a read-only checklist of possible mistakes and questions. This cannot edit the file or run the project.",
                  icon: <ListChecks size={15} />,
                  onSelect: () => {
                    if (item.nodeId) openExplain({ kind: "file", nodeId: item.nodeId, path, initialLens: "review" as const });
                  }
                }
              ]
            : []),
          ...(!capabilities.isDirectory && !capabilities.isLink ? [{
            id: "pin",
            label: isPinned ? "Unpin" : "Pin",
            section: "More tools",
            help: isPinned ? `Remove ${item.displayName} from Pinned.` : `Keep ${item.displayName} in Pinned for quick access.`,
            icon: isPinned ? <PinOff size={15} /> : <Pin size={15} />,
            disabled: !item.nodeId,
            onSelect: () => item.nodeId ? updateFilePin(item.nodeId, item.displayName, isPinned) : undefined
          }] : []),
          {
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
          },
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
    [buildPreviewPlan, connectorBuild, copyNodePath, expandedTree, explainFolder, loadTreeChildren, openExplain, openNode, openNodeWithSystem, pinnedItems, revealNodeWithSystem, showReview, toggleExpandedTree, treePages, updateFilePin]
  );

  // Right-click on a text selection inside the preview -> explicit AI reading and change tools.
  // With no selection (or in the Local edition) the default menu is left alone. The selection is
  // capped and gated server-side on the exact bytes (secrets refused) before anything is sent.
  const showPreviewSelectionMenu = useCallback(
    (event: MouseEvent<HTMLElement>) => {
      if (!connectorBuild) return;
      // No node, transiently-revealed sensitive content, or a size-capped preview: don't offer to
      // send/rewrite the selection (a truncated buffer would truncate the file on a later Save).
      // The backend re-resolves and re-gates the origin path too, so this is belt-and-suspenders.
      if (!preview?.nodeId || preview.wasRevealed || preview.truncated) return;
      const selected = (window.getSelection()?.toString() ?? "").trim();
      if (!selected) return;
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      const nodeId = preview.nodeId;
      const snippet = selected.slice(0, 16000);
      const anchor = contextMenuCoordinates(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());
      setContextMenu({
        x: anchor.x,
        y: anchor.y,
        label: "AI tools for selected text",
        items: [
          {
            id: "explain-selection",
            label: "Explain selected text with AI",
            section: "Understand with AI",
            help: "Explain the selected text in plain language using your configured AI provider. The selection is checked for secrets before anything is sent.",
            icon: <Sparkles size={15} />,
            onSelect: () => openExplain({ kind: "text", nodeId, snippet, label: "Selected text", initialLens: "explain" })
          },
          {
            id: "review-selection",
            label: "Check selected text for risks",
            help: "Ask AI for a read-only checklist of possible mistakes and useful questions about the selected text. Nothing is edited or run.",
            icon: <ListChecks size={15} />,
            onSelect: () => openExplain({ kind: "text", nodeId, snippet, label: "Selected text", initialLens: "review" })
          },
          {
            id: "rewrite-selection",
            label: changesUnlocked ? "Suggest a change" : "Suggest a change (locked)",
            section: "Change this text",
            help: changesUnlocked
              ? "Ask your configured AI provider for one replacement. Secrets are blocked and the result stays a proposal until you review the exact before/after selection and explicitly apply it."
              : "Project file changes are locked. Open the safety warning and unlock this project before preparing an AI rewrite.",
            icon: <Wand2 size={15} />,
            danger: true,
            onSelect: () => openRewrite({ nodeId, snippet, label: "Selected text" }, preview?.displayName ?? "selection")
          },
          {
            id: "copy-selection",
            label: "Copy selected text",
            section: "Other",
            help: "Copy the selected text to the clipboard without sending or changing anything.",
            icon: <Copy size={15} />,
            onSelect: async () => {
              try {
                if (!navigator.clipboard?.writeText) throw new Error("Clipboard unavailable");
                await navigator.clipboard.writeText(selected);
                setStatusText("Selected text copied to clipboard.");
              } catch {
                setStatusText("Clipboard is unavailable in this runtime.");
              }
            }
          }
        ]
      });
    },
    [changesUnlocked, connectorBuild, openExplain, openRewrite, preview, setContextMenu]
  );

  useEffect(() => {
    if (!runningJobKey) return;
    let cancelled = false;
    let timer: number | null = null;
    const schedule = (delay: number) => {
      if (!cancelled) timer = window.setTimeout(() => void poll(), delay);
    };
    const poll = async () => {
      try {
        const statuses = await Promise.all(runningScanStatuses.map((status) => api.scanStatus(status.jobId)));
        if (cancelled) return;
        let finished = false;
        for (const status of statuses) {
          setScanStatus(status);
          const progress = scanProgressParts(status);
          if (["partial", "cancelled", "completed"].includes(status.state)) {
            setStatusText(`${status.message.replace(/[.:]\s*$/, "")}: ${progress.countText}`);
          } else {
            setStatusText(status.currentPath ? `${status.message} ${status.currentPath}` : status.message);
          }
          if (!["running", "cancelling"].includes(status.state)) {
            finished = true;
          }
          if (status.state === "completed" && !celebratedJobsRef.current.has(status.jobId)) {
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
  }, [refreshAfterScanFinish, refreshWatcherStatus, runningJobKey, runningScanStatuses, setScanStatus]);

  useEffect(() => {
    if (!scanCelebration) return;
    const timer = window.setTimeout(() => setScanCelebration(null), 4500);
    return () => window.clearTimeout(timer);
  }, [scanCelebration]);

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
      style={appShellStyle}
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
          <div
            className="brand-mark"
            tabIndex={0}
            role="button"
            aria-haspopup="menu"
            aria-label="Navigation menu"
            data-help="Overview, Discover, Recover and Settings — hover here to navigate when the sidebar is collapsed or scrolled."
          >
            CH
            <div
              className="brand-flyout"
              role="menu"
              onClick={(event) => {
                const active = event.currentTarget.ownerDocument.activeElement;
                if (active instanceof HTMLElement && event.currentTarget.contains(active)) {
                  active.blur();
                }
              }}
            >
              <PrimaryNavButtons
                primaryView={primaryView}
                onOverview={showOverview}
                onDiscover={() => showDiscover(discoverView)}
                onRecovery={showRecovery}
                onSettings={() => showSettings(settingsView)}
              />
            </div>
          </div>
          <div className="brand-text">
            <strong>Code Hangar</strong>
            <span>Understand local projects safely</span>
          </div>
          {showTopbarNav ? (
            <nav className="topbar-nav" aria-label="Quick navigation">
              <PrimaryNavButtons
                primaryView={primaryView}
                iconsOnly
                onOverview={showOverview}
                onDiscover={() => showDiscover(discoverView)}
                onRecovery={showRecovery}
                onSettings={() => showSettings(settingsView)}
              />
            </nav>
          ) : null}
        </div>
        <div className="topbar-actions">
          <button className="icon-button" type="button" onClick={goBack} disabled={viewBackStack.length === 0 && backStack.length === 0} aria-label="Back" data-help="Go back to the previous screen. If no screen history exists, go back to the previous opened file.">
            <ArrowLeft size={17} />
          </button>
          <button className="icon-button" type="button" onClick={goForward} disabled={viewForwardStack.length === 0 && forwardStack.length === 0} aria-label="Forward" data-help="Go forward to the next screen. If no screen history exists, go forward to the next file.">
            <ArrowRight size={17} />
          </button>
          <button ref={quickOpenButtonRef} className="toolbar-button" type="button" data-tour="tour-quick-open" onClick={(event) => openQuickOpen(event.currentTarget)} aria-label="Quick Open" data-help="Open Quick Open to jump to projects or indexed files by name and path.">
            <Search size={16} />
            <span className="tb-label">Quick Open</span>
            <kbd>Ctrl+P</kbd>
          </button>
          <button ref={commandButtonRef} className="toolbar-button" type="button" onClick={(event) => openCommandPalette(event.currentTarget)} aria-label="Commands" data-help="Show available commands and keyboard shortcuts.">
            <Command size={16} />
            <span className="tb-label">Commands</span>
            <kbd>Ctrl+K</kbd>
          </button>
          <div className="performance-toggle" role="radiogroup" aria-label="Performance mode" data-help={performanceHelpText(performanceMode)}>
            <Gauge size={16} />
            {(["balanced", "priority", "max"] as const).map((mode) => (
              <button
                key={mode}
                className={`performance-choice ${performanceMode === mode ? "active" : ""} ${mode === "max" ? "max" : ""}`}
                type="button"
                aria-pressed={performanceMode === mode}
                onClick={() => choosePerformanceMode(mode)}
                data-help={performanceHelpText(mode)}
              >
                {performanceLabel(mode)}
              </button>
            ))}
          </div>
          <ResourceMeter />
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
          <button className="toolbar-button primary" type="button" onClick={() => setAddProjectsVisible(true)} aria-label="Add Projects" data-help="Add projects manually or run passive discovery under a selected folder or drive.">
            <FolderOpen size={16} />
            <span className="tb-label">Add Projects</span>
          </button>
        </div>
      </header>

      {startupProgress.active ? (
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

      <WorkspaceGrid mode={isProjectLayout ? "project" : "tool"} style={workspaceStyle} leftCollapsed={leftPaneCollapsedForLayout} rightCollapsed={rightPaneCollapsedForLayout} className={`${connectorBuild && aiExplainTarget && aiExplainDocked && !isProjectLayout ? "is-ai-edge-docked" : ""} ${reviewFocusedLayout ? "review-focused" : ""}`}>
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
              onOverview={showOverview}
              onDiscover={() => showDiscover(discoverView)}
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
                {selectedProject ? (
                  <button
                    type="button"
                    className={`project-safe-manage ${primaryView === "review" ? "active" : ""}`}
                    onClick={showReview}
                    data-help={`Open Safe Manage for ${selectedProject.name} — review ownership, protected paths, shared references and local impact before deciding what matters.`}
                  >
                    <ListChecks size={15} />
                    <span>Safe Manage</span>
                  </button>
                ) : null}
                {displayedProjects.length > 1 ? (
                  <div className="project-list-toolbar" role="group" aria-label="Find and filter projects">
                    <div className="project-search" data-help="Filter the project list by project name, folder path, AI app, or alternate app name without opening Quick Open.">
                      <Search size={14} aria-hidden="true" />
                      <input
                        ref={projectSearchInputRef}
                        className="project-search-input"
                        type="search"
                        value={projectQuery}
                        onChange={(event) => setProjectQuery(event.target.value)}
                        onKeyDown={handleProjectSearchKeyDown}
                        placeholder="Find project..."
                        aria-label="Find project"
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
                    <div className="list-controls" role="group" aria-label="Sort and filter projects">
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
                    </div>
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
                    <p>No projects match the current filters.</p>
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
                        onContextMenu={rowShowProjectMenu}
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
                      <div className="list-controls" role="group" aria-label="Sort and filter sessions">
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
                      </div>
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
                        key={`${item.itemKind}-${item.nodeId}`}
                        type="button"
                        data-help={`Open pinned ${item.itemKind} ${item.path}. Right-click for safe actions.`}
                        onClick={() => pinnedProject ? selectProject(pinnedProject.id) : void openNode(item.nodeId)}
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
              <small>{recentItems.length}</small>
            </button>
            {!sidebarCollapsed.recent ? (
              <>
                {recentItems.length === 0 ? <p className="muted">No recent files yet.</p> : null}
                {recentItems.slice(0, recentShowAll ? recentItems.length : 5).map((item) => (
                  <button
                    className="small-row"
                    key={`${item.nodeId}-${item.openedAt}`}
                    type="button"
                    data-help={`Open recent file ${item.path} without moving it to the top. Right-click for safe actions.`}
                    onClick={() => openNode(item.nodeId, { recordRecent: false })}
                    onContextMenu={(event) => showFileMenu({ nodeId: item.nodeId, projectId: item.projectId, path: item.path, label: item.path }, event)}
                  >
                    {item.path}
                  </button>
                ))}
                {recentItems.length > 5 ? (
                  <button
                    type="button"
                    className="session-group-more"
                    onClick={() => setRecentShowAll((value) => !value)}
                    data-help={recentShowAll ? "Return to the 5 most recently opened files." : "Show every recently opened file Code Hangar is tracking."}
                  >
                    {recentShowAll ? "Show top 5" : `Show all recent files (${recentItems.length})`}
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
            openNode={openNode}
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
            zoneShowProtectedMetadata={zoneShowProtectedMetadata}
            startTreeResize={startTreeResize}
              contextFiles={contextFiles}
              projectOverlapWarning={selectedProjectOverlapWarning}
              showReview={showReview}
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
            relationships={relationships}
            relationshipsNodeId={relationshipsNodeId}
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
            onExplainSelection={showPreviewSelectionMenu}
            onFileMutated={async (nodeId) => {
              invalidatePreviewCache(nodeId);
              if (editDraftNodeRef.current === nodeId) {
                editDraftNodeRef.current = null;
                setEditUndo(null);
              }
              await openNode(nodeId, { mode: previewMode, recordRecent: false, refreshOnly: true, replaceHistory: true, allowProjectSwitch: false });
            }}
            changesUnlocked={changesUnlocked}
            onRequestChangeAccess={requestChangeAccess}
            onRelockChanges={() => {
              setUnlockedChangeProjectId(null);
              setRewriteTarget(null);
              setStatusText("Project file changes are locked again.");
            }}
            connectorBuild={connectorBuild}
            recapDetailLayer={connectorBuild ? RecapAiLayer : undefined}
            onUndoAiSession={connectorBuild ? async (nodeId, sessionId) => { await undoRewriteSession(nodeId, sessionId); } : undefined}
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
          {primaryView === "settings" && settingsView === "advanced" ? (
            <>
            <section className="pane-section tool-content-section settings-system-view">
              <div className="dashboard-grid wide settings-system-grid">
                <div className="dashboard-card">
                  <h3>Build and safety</h3>
                  <dl className="inspector-list small">
                    <dt>Disk actions</dt>
                    <dd>{mutationAvailable ? "Available after review and confirmation" : "Read-only build"}</dd>
                    <dt>Network</dt>
                    <dd>{security?.outboundNetwork ?? "Disabled"}</dd>
                    <dt>Agent access</dt>
                    <dd>{security?.agentIpc ?? "Not compiled"}</dd>
                    <dt>Protection</dt>
                    <dd>Always enforced</dd>
                  </dl>
                </div>
                <div className="dashboard-card" data-help="Analyze this PC locally and show the CPU/RAM budget Code Hangar applies to Balanced, Priority and Max CPU. This never sends telemetry.">
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
            {connectorBuild ? (
            <>
            <SettingsAutomationView
              status={automationStatus}
              agents={automationAgents}
              activity={automationActivity}
              credential={automationCredential}
              projects={projects.filter((project) => !isDemoProject(project))}
              currentFile={preview ? { nodeId: preview.nodeId, displayName: preview.displayName } : null}
              busy={automationBusy}
              error={automationError}
              onRefresh={() => void refreshAutomation()}
              onRegister={(name, scopes, projectIds) => void registerAutomation(name, scopes, projectIds)}
              onRevoke={(agentId) => void revokeAutomation(agentId)}
              onForget={(agentId) => void forgetRevokedAutomation(agentId)}
              onGrantRead={(agentId, nodeId) => void grantAutomationRead(agentId, nodeId)}
              onCopy={(value) => void copyPath(value)}
              onClearCredential={() => setAutomationCredential(null)}
            />
            <SettingsConnectedAppsView
              confirm={requestConfirm}
              projects={projects.filter((project) => !isDemoProject(project))}
            />
            <section className="pane-section compact">
              <AiAssistKeyCard />
            </section>
            </>
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
              openNode={openNode}
              connectorBuild={connectorBuild}
              explainPanel={connectorBuild && aiExplainTarget && aiExplainHostedInInspector ? (
                <AiExplainPanel
                  target={aiExplainTarget}
                  docked
                  pos={aiExplainPos}
                  onToggleDock={() => setAiExplainDocked(false)}
                  onClose={() => setAiExplainTarget(null)}
                  onPosChange={setAiExplainPos}
                />
              ) : null}
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
                reviewInventoryReady={projectDiscoveryReport !== null}
                onOpenRecap={() => selectedProjectId ? showProjectWorkspace("recap") : focusProjectPicker()}
                onOpenProjectRecap={openProjectRecap}
                onDiscover={() => showDiscover("lost")}
                onReview={showReview}
                onRecovery={showRecovery}
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
              openNode={openNode}
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
              openNode={openNode}
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
              openNode={openNode}
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
              onOpenNode={(nodeId) => void openNode(nodeId)}
              onSafeManageProject={(projectId) => {
                selectProject(projectId);
                showReview();
              }}
            />
          ) : null}

          {rightPaneView === "activity" ? (
            <RecoveryView
              mutationAvailable={mutationAvailable}
              finalRemoveEnabled={finalRemoveEnabled}
              onToggleFinalRemove={toggleFinalRemove}
              mutationMessage={mutationMessage}
              mutationActivity={mutationActivity}
              mutationBusy={mutationBusy}
              advancedMode={advancedMode}
              projects={projects}
              appRemovals={appRemovals}
              restoreAppRemoval={restoreAppRemoval}
              refreshMutationActivity={refreshMutationActivity}
              runMutationRestore={runMutationRestore}
              runMutationRestoreElsewhere={runMutationRestoreElsewhere}
              runMutationFinalRemove={runMutationFinalRemove}
              onDiscoverProjects={() => showDiscover("projects")}
              onOpenScanFolders={() => showSettings("folders")}
              currentFile={preview ? { nodeId: preview.nodeId, displayName: preview.displayName } : null}
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
          <span className="statusbar-scan" data-help="Live scan progress. Code Hangar reuses previous inventory estimates when available; new roots are counted before indexing metadata.">
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
            void openNode(result.nodeId);
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
            showReview();
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
          wslScan={wslScanChoice}
          onToggleWsl={(value) => {
            setWslScanChoice(value);
            void api.setWslScanEnabled(value).catch(() => undefined);
          }}
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
            if (deepScanProgress.phase !== "done") {
              setStatusText(
                deepScanProgress.phase === "building"
                  ? "Inventory indexing continues in the background."
                  : "Deep Scan continues in the background."
              );
            }
          }}
          onStop={() => {
            if (deepScanProgress.scanJobId) void cancelScan(deepScanProgress.scanJobId);
          }}
        />
      ) : null}

      {resetAllVisible ? (
        <ResetAllDialog
          projectCount={projects.filter((project) => !isDemoProject(project)).length}
          rootCount={roots.length}
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

      {/* When the Inspector cannot host the panel, including while it is auto-collapsed on a narrow
          window, "dock" pins Explain this to the window edge. "Pop out" returns it to a draggable
          float, so the panel remains reachable on every screen and at every supported width. */}
      {connectorBuild && aiExplainTarget && !aiExplainHostedInInspector ? (
        <Suspense fallback={null}>
        <AiExplainPanel
          target={aiExplainTarget}
          docked={false}
          edge={aiExplainDocked}
          collapsed={aiExplainDocked && aiExplainCollapsed}
          onToggleCollapse={() => setAiExplainCollapsed((value) => !value)}
          pos={aiExplainPos}
          onToggleDock={() => setAiExplainDocked((value) => !value)}
          onClose={() => setAiExplainTarget(null)}
          onPosChange={setAiExplainPos}
        />
        </Suspense>
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

      {connectorBuild && rewriteTarget ? (
        <Suspense
          fallback={(
            <div className="modal-overlay" role="status" aria-live="polite">
              <div className="modal rewrite-dialog">
                <p className="muted">Opening selected change...</p>
              </div>
            </div>
          )}
        >
          <RewriteDialog
            target={rewriteTarget}
            fileName={rewriteFileName}
            onClose={() => setRewriteTarget(null)}
            onRun={runRewrite}
            onApply={applyRewriteSuggestion}
            onUndo={async (nodeId, sessionId) => { await undoRewriteSession(nodeId, sessionId); }}
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
  done: boolean;
}
interface DeepScanProgress {
  stages: DeepScanStage[];
  phase: DeepScanPhase;
  projectsFound: number;
  sessionsFound: number;
  addedCount: number;
  note: string;
  // During "building", the inventory scan job whose live progress the panel shows.
  scanJobId?: string | null;
}

interface BuildProject {
  id: number;
  name: string;
  done: boolean;
  current: boolean;
}

function initialDeepScanStages(installedApps: InstalledApp[], includeWsl: boolean): DeepScanStage[] {
  return deepScanSourceLabels(installedApps, includeWsl).map((label) => ({ label, done: false }));
}

// A rewarding, full-screen Deep Scan progress panel. First it maps each AI app
// (each lights up as it's read); then it carries straight into "Building your
// inventory" with live scan progress, so the unavoidable thorough scan is a clear,
// dopamine-rich wait instead of a cramped status-bar line.
function DeepScanProgressOverlay({
  progress,
  scanStatus,
  buildProjects,
  onHide,
  onStop
}: {
  progress: DeepScanProgress;
  scanStatus: ScanStatus | null;
  buildProjects: BuildProject[];
  onHide: () => void;
  onStop: () => void;
}) {
  const { dialogRef, onDialogKeyDown } = useDialogFocusTrap(onHide);
  const finished = progress.phase === "done";
  const building = progress.phase === "building" || (finished && progress.scanJobId != null);
  // One unified panel for the whole Deep Scan: the per-app checklist and the
  // running totals stay visible the whole time, and once the inventory scan
  // starts its live readout (items/GiB/rate/threads) and per-project ticker fill
  // in below — so it reads as a single pleasing menu instead of two that swap.
  const parts = building && scanStatus ? scanProgressParts(scanStatus) : null;
  const indeterminate = deepScanUsesIndeterminateProgress(progress.phase, parts?.percent);
  const stagesDone = progress.stages.filter((stage) => stage.done).length;
  const percent = finished
    ? 100
    : building
      ? parts?.percent ?? 0
      : Math.round((stagesDone / Math.max(progress.stages.length, 1)) * 100);
  const builtCount = buildProjects.filter((project) => project.done).length;
  const title = finished
    ? "Inventory ready"
    : building
      ? "Building your inventory"
      : progress.phase === "registering"
        ? "Adding confirmed projects"
        : "Mapping your AI projects";
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
          <div className="deep-scan-spinner" data-phase={progress.phase} aria-hidden="true">
            {finished ? <CheckCircle2 size={28} /> : <Radar size={28} />}
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
        <div className="deep-scan-bar" aria-hidden="true">
          <div
            className={`deep-scan-bar-fill ${indeterminate ? "indeterminate" : ""}`}
            style={indeterminate ? undefined : { width: `${percent}%` }}
          />
        </div>
        {building ? (
          <>
            <div className="deep-scan-build-readout">
              <strong>{finished ? "Done" : parts?.progressText ?? "Scanning…"}</strong>
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
            <li key={stage.label} data-done={stage.done ? "true" : "false"}>
              <span className="deep-scan-stage-icon" aria-hidden="true">
                {stage.done ? <CheckCircle2 size={16} /> : <span className="deep-scan-dot" />}
              </span>
              <span className="deep-scan-stage-label">{stage.label}</span>
              <span className="deep-scan-stage-state">{stage.done ? "checked" : "included"}</span>
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
              >
                <span className="deep-scan-stage-icon" aria-hidden="true">
                  {project.done ? (
                    <CheckCircle2 size={14} />
                  ) : project.current ? (
                    <Loader2 size={14} className="spin" />
                  ) : (
                    <span className="deep-scan-dot" />
                  )}
                </span>
                <span className="deep-scan-stage-label">{project.name}</span>
                <span className="deep-scan-stage-state">
                  {project.done ? "indexed" : project.current ? "indexing…" : "queued"}
                </span>
              </li>
            ))}
          </ul>
        ) : null}
        <div className="deep-scan-build-foot">
          <span className="muted">
            {building
              ? buildProjects.length > 0
                ? `${builtCount} of ${buildProjects.length} ready`
                : "Inventory scan in progress."
              : finished
                ? "Mapping complete."
                : progress.phase === "registering"
                  ? "Sources checked. Adding strong matches…"
                  : "Checking all included sources…"}
          </span>
          <div className="deep-scan-build-actions">
            <button type="button" className="deep-scan-ghost" onClick={onHide} data-help={finished ? "Close this progress summary." : "Keep the scan running in the background and return to the app. Open Add Projects to show progress again."}>
              {finished ? "Close" : "Hide and keep working"}
            </button>
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
    case "review":
      return "Safe Manage";
    case "settings":
      return "Local preferences";
    case "recovery":
      return "Recover";
    default:
      return "Local inventory";
  }
}

function workspaceTitle(view: PrimaryView, discoverView: DiscoverView, settingsView: SettingsView) {
  if (view === "overview") return "Overview";
  if (view === "review") return "Safe Manage";
  if (view === "recovery") return "Recover";
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
  if (view === "review") return "Review ownership, protected paths, shared references and local impact. The base build is read-only.";
  if (view === "recovery") return "Review verified backups, held files and interrupted local actions recorded for this profile.";
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

function performanceLabel(mode: PerformanceMode) {
  switch (mode) {
    case "max":
      return "Max CPU";
    case "priority":
      return "Priority";
    default:
      return "Background";
  }
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
      .join(". ") + ". Sampled locally about every 10 seconds while the window is visible; no telemetry leaves this machine.";
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
  onContextMenu: (project: ProjectSummary, event: MouseEvent<HTMLElement>) => void;
}) {
  return (
    <button
      className={`project-row project-${state} ${isSelected ? "selected" : ""} ${keepSelectedArchivedVisible ? "selected-archived" : ""}`}
      type="button"
      data-help={`${keepSelectedArchivedVisible ? "Current project kept visible while Archived is collapsed. " : ""}Open project ${project.name}. ${projectStateHelp(state)} ${watchReason} Right-click for project actions.`}
      onClick={() => onSelect(project.id)}
      onContextMenu={(event) => onContextMenu(project, event)}
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
    metadata.mcpServerCount != null ? ["Connected tool servers", String(metadata.mcpServerCount)] : null
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
                  key={`${result.itemKind}-${result.nodeId}`}
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
  onCancel,
  onConfirm
}: {
  projectCount: number;
  rootCount: number;
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
            <li><strong>Local reset:</strong> this cannot be undone inside Code Hangar.</li>
          </ul>
          <label className="toggle-row" data-help="Confirm you understand this unregisters every project and that a new Deep Scan will be needed afterwards.">
            <input data-dialog-initial-focus type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
            <span>I understand this unregisters everything and I will run a new Deep Scan afterwards.</span>
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
              <span>Deleting the folder is a <strong>full removal</strong>. You bring the project back by restoring its folder from <strong>Recover</strong> — this is not a one-click Undo.</span>
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
          <label className="toggle-row" data-help="Delete the project folder from disk. It is fully backed up to a location you choose first, then removed — restorable from Recover.">
            <input type="checkbox" checked={fromDisk} onChange={(event) => setFromDisk(event.target.checked)} />
            <span>
              <strong>Delete the folder from disk</strong>
              <small>Off by default. Continue to Safe Manage to choose and verify a backup before anything moves.</small>
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
              Type <strong>{project.name}</strong> before continuing to the separate backup-and-remove review
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
            data-help={fromDisk ? "Open Safe Manage for the selected disk removal. Code Hangar will require backup and confirmation before files move." : "Run the selected metadata removals. Files on disk stay where they are."}
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
  wslScan,
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
  wslScan: boolean;
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
  const secondaryActionsBusy = actionsBusy || deepScanRunning;
  const deepScanScope = wslScan ? "Windows and enabled WSL distros" : "Windows";
  const deepScanTitle = deepScanRunning
    ? "Deep Scan is running"
    : actionsBusy
      ? "Another local scan is running"
      : "Deep Scan — map known projects";
  const deepScanDescription = deepScanRunning
    ? "Return to the live source and inventory progress."
    : actionsBusy
      ? "Finish the current scan before starting another discovery."
      : "Reads project lists from detected AI tools. Strong matches are added automatically.";
  const deepScanAction = deepScanRunning
    ? ADD_PROJECTS_SHOW_PROGRESS_ACTION
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
          disabled={actionsBusy && !deepScanRunning}
          aria-busy={actionsBusy && !deepScanRunning}
          data-help={deepScanRunning
            ? "Show the Deep Scan progress panel again. The scan continued while the panel was hidden."
            : `Read detected AI tools' local project lists on ${deepScanScope}. Strong matches are added automatically; the rest are listed for review.`}
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
          ) : (
            <span className="muted">No AI tools detected on this PC yet — you can still search a folder or drive below.</span>
          )}
        </p>
        {wslOffer ? (
          <label className="wsl-offer" data-help="Code Hangar found WSL (Linux) distros on this PC. Enable WSL scanning to include AI tools installed inside them during the scan. Left off it never runs wsl.exe — so it can't surface a WSL error on a PC where WSL isn't fully set up.">
            <input type="checkbox" checked={wslScan} onChange={(event) => onToggleWsl(event.target.checked)} />
            <span className="wsl-offer-text">
              <strong>{wslOfferTitle}</strong>
              {wslOfferDetail ? <span className="muted">{wslOfferDetail}</span> : null}
            </span>
          </label>
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
              <input type="checkbox" checked={wslScan} onChange={(event) => onToggleWsl(event.target.checked)} />
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
        <button type="button" disabled={!projectCommandState.enabled} data-help={projectCommandState.reviewHelp} onClick={onReview}><ListChecks size={16} /><span>Safe Manage</span><small className="command-context-pill">{projectCommandState.contextLabel}</small></button>
        <button type="button" data-help="Open Recover to review verified backups and recoverable held files." onClick={onRecovery}><ArchiveRestore size={16} /><span>Recover</span></button>
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
