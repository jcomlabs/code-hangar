export type RightPaneView = "inspector" | "dashboard" | "search" | "orphans" | "duplicates" | "organize" | "plan" | "activity" | "roots" | "zones";
export type PrimaryView = "project" | "overview" | "discover" | "review" | "recovery" | "settings";
export type ProjectView = "context" | "recap" | "files" | "space" | "connections" | "sessions";
export type DiscoverView = "projects" | "search" | "lost" | "assets" | "duplicates" | "organize";
export type SettingsView = "folders" | "protection" | "appearance" | "advanced";

export interface WorkspaceRoute {
  primaryView: PrimaryView;
  projectView: ProjectView;
  discoverView: DiscoverView;
  settingsView: SettingsView;
  rightPaneView: RightPaneView;
  selectedProjectId: number | null;
}

export type StartupDestination = "overview" | "last-workspace";
export type StartupPaneMode = "open" | "remember" | "collapsed";

export interface StartupPreferences {
  destination: StartupDestination;
  leftPane: StartupPaneMode;
  rightPane: StartupPaneMode;
}

export const INITIAL_WORKSPACE_ROUTE: WorkspaceRoute = {
  primaryView: "overview",
  projectView: "context",
  discoverView: "projects",
  settingsView: "folders",
  rightPaneView: "dashboard",
  selectedProjectId: null
};

export const DEFAULT_STARTUP_PREFERENCES: StartupPreferences = {
  destination: "overview",
  leftPane: "open",
  rightPane: "remember"
};

const PRIMARY_VIEWS: readonly PrimaryView[] = ["project", "overview", "discover", "review", "recovery", "settings"];
const PROJECT_VIEWS: readonly ProjectView[] = ["context", "recap", "files", "space", "connections", "sessions"];
const DISCOVER_VIEWS: readonly DiscoverView[] = ["projects", "search", "lost", "assets", "duplicates", "organize"];
const SETTINGS_VIEWS: readonly SettingsView[] = ["folders", "protection", "appearance", "advanced"];
const RIGHT_PANE_VIEWS: readonly RightPaneView[] = ["inspector", "dashboard", "search", "orphans", "duplicates", "organize", "plan", "activity", "roots", "zones"];
const STARTUP_PANE_MODES: readonly StartupPaneMode[] = ["open", "remember", "collapsed"];

function allowed<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && (values as readonly string[]).includes(value);
}

export function normalizeStartupPreferences(value: unknown): StartupPreferences {
  if (!value || typeof value !== "object") return { ...DEFAULT_STARTUP_PREFERENCES };
  const candidate = value as Partial<StartupPreferences>;
  return {
    destination: candidate.destination === "last-workspace" ? "last-workspace" : "overview",
    leftPane: allowed(STARTUP_PANE_MODES, candidate.leftPane) ? candidate.leftPane : DEFAULT_STARTUP_PREFERENCES.leftPane,
    rightPane: allowed(STARTUP_PANE_MODES, candidate.rightPane) ? candidate.rightPane : DEFAULT_STARTUP_PREFERENCES.rightPane
  };
}

export function parseWorkspaceRoute(value: unknown): WorkspaceRoute | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<WorkspaceRoute>;
  const selectedProjectId = candidate.selectedProjectId;
  if (!allowed(PRIMARY_VIEWS, candidate.primaryView)
    || !allowed(PROJECT_VIEWS, candidate.projectView)
    || !allowed(DISCOVER_VIEWS, candidate.discoverView)
    || !allowed(SETTINGS_VIEWS, candidate.settingsView)
    || !allowed(RIGHT_PANE_VIEWS, candidate.rightPaneView)
    || !(selectedProjectId === null || (Number.isInteger(selectedProjectId) && Number(selectedProjectId) > 0))) {
    return null;
  }
  return {
    primaryView: candidate.primaryView,
    projectView: candidate.projectView,
    discoverView: candidate.discoverView,
    settingsView: candidate.settingsView,
    rightPaneView: candidate.rightPaneView,
    selectedProjectId: selectedProjectId as number | null
  };
}

function startupPaneCollapsed(mode: StartupPaneMode, stored: boolean | undefined) {
  if (mode === "open") return false;
  if (mode === "collapsed") return true;
  return Boolean(stored);
}

/** Defaults keep the project sidebar visible while preserving the Inspector's
 * previous state. Settings can override either choice for the next launch. */
export function startupPaneCollapse(
  stored?: { left?: boolean; right?: boolean },
  preferences: StartupPreferences = DEFAULT_STARTUP_PREFERENCES
) {
  return {
    left: startupPaneCollapsed(preferences.leftPane, stored?.left),
    right: startupPaneCollapsed(preferences.rightPane, stored?.right)
  };
}

export function startupWorkspaceRoute(
  preferences: StartupPreferences,
  stored: WorkspaceRoute | null,
  availableProjectIds: readonly number[]
): WorkspaceRoute {
  if (preferences.destination === "overview" || !stored) {
    return { ...INITIAL_WORKSPACE_ROUTE };
  }
  const projectExists = stored.selectedProjectId === null || availableProjectIds.includes(stored.selectedProjectId);
  if (projectExists) return { ...stored };
  if (stored.primaryView === "project" || stored.primaryView === "review") {
    return { ...INITIAL_WORKSPACE_ROUTE };
  }
  return { ...stored, selectedProjectId: null };
}

export function sameWorkspaceRoute(left: WorkspaceRoute, right: WorkspaceRoute): boolean {
  return left.primaryView === right.primaryView
    && left.projectView === right.projectView
    && left.discoverView === right.discoverView
    && left.settingsView === right.settingsView
    && left.rightPaneView === right.rightPaneView
    && left.selectedProjectId === right.selectedProjectId;
}

export function shouldRecordWorkspaceRoute(
  current: WorkspaceRoute,
  planned: WorkspaceRoute,
  recordCurrent = false
): boolean {
  return recordCurrent || !sameWorkspaceRoute(current, planned);
}

export function projectSidebarCollapsedForLayout(
  primaryView: PrimaryView,
  projectView: ProjectView,
  manuallyCollapsed: boolean,
  projectFocusActive: boolean,
  compactWindow: boolean
): boolean {
  if (manuallyCollapsed) return true;
  if (!projectFocusActive) return false;
  if (primaryView === "review") return true;
  return primaryView === "project" && (compactWindow || projectView === "files");
}

export function focusedFileStatusIsRelevant(
  primaryView: PrimaryView,
  projectView: ProjectView,
  sessionPreviewOpen: boolean
): boolean {
  return primaryView === "project"
    && !sessionPreviewOpen
    && (projectView === "context" || projectView === "files");
}

export function projectViewPrefersWideCanvas(projectView: ProjectView): boolean {
  return projectView === "recap" || projectView === "space" || projectView === "connections" || projectView === "sessions";
}

export function workspaceCenterPaneIsCramped(
  windowWidth: number,
  leftPaneWidth: number,
  rightPaneWidth: number,
  minimumCenterWidth = 600
): boolean {
  const usableWindowWidth = Math.max(0, windowWidth);
  const reservedPaneWidth = Math.max(0, leftPaneWidth) + Math.max(0, rightPaneWidth);
  return usableWindowWidth - reservedPaneWidth < Math.max(0, minimumCenterWidth);
}

export function projectInspectorCollapsedForLayout(
  primaryView: PrimaryView,
  projectView: ProjectView,
  manuallyCollapsed: boolean,
  autoCollapsedInspectorExpanded: boolean,
  compactWindow: boolean
): boolean {
  if (primaryView === "review") return true;
  if (primaryView !== "project") return false;
  const autoCollapse = compactWindow || projectViewPrefersWideCanvas(projectView);
  return manuallyCollapsed || (autoCollapse && !autoCollapsedInspectorExpanded);
}

export function projectInspectorHostsAiExplain(
  isProjectLayout: boolean,
  rightPaneView: RightPaneView,
  rightPaneCollapsed: boolean,
  aiExplainDocked: boolean
): boolean {
  return aiExplainDocked
    && isProjectLayout
    && rightPaneView === "inspector"
    && !rightPaneCollapsed;
}

export function workspaceRouteStatusText(route: { primaryView: PrimaryView; projectView?: ProjectView; discoverView?: DiscoverView; settingsView?: SettingsView }) {
  switch (route.primaryView) {
    case "overview":
      return "Overview opened.";
    case "project":
      return `Project opened: ${projectViewLabel(route.projectView ?? "context")}.`;
    case "review":
      return "Safe Manage opened.";
    case "recovery":
      return "Recover opened.";
    case "discover":
      return `Discover opened: ${discoverViewStatusLabel(route.discoverView ?? "projects")}.`;
    case "settings":
      return `Settings opened: ${settingsViewStatusLabel(route.settingsView ?? "folders")}.`;
    default:
      return "Workspace opened.";
  }
}

export function projectViewLabel(view: ProjectView) {
  switch (view) {
    case "context":
      return "Context";
    case "recap":
      return "What changed";
    case "space":
      return "Space";
    case "connections":
      return "Connections";
    case "sessions":
      return "Sessions";
    case "files":
    default:
      return "Files";
  }
}

function discoverViewStatusLabel(view: DiscoverView) {
  switch (view) {
    case "projects":
      return "Find local projects & sessions";
    case "search":
      return "Document search";
    case "lost":
      return "Forgotten projects";
    case "assets":
      return "Unreferenced files";
    case "duplicates":
      return "Duplicate files";
    case "organize":
      return "Organize";
  }
}

function settingsViewStatusLabel(view: SettingsView) {
  switch (view) {
    case "folders":
      return "Scan folders";
    case "protection":
      return "Protected locations";
    case "appearance":
      return "Appearance";
    case "advanced":
      return "System & diagnostics";
  }
}
