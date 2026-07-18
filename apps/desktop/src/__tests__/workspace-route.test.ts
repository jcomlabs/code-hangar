import { describe, expect, it } from "vitest";
import {
  focusedFileStatusIsRelevant,
  DEFAULT_STARTUP_PREFERENCES,
  INITIAL_WORKSPACE_ROUTE,
  normalizeStartupPreferences,
  parseWorkspaceRoute,
  projectInspectorCollapsedForLayout,
  projectInspectorHostsAiExplain,
  projectSidebarCollapsedForLayout,
  projectViewPrefersWideCanvas,
  shouldRecordWorkspaceRoute,
  startupPaneCollapse,
  startupWorkspaceRoute,
  workspaceCenterPaneIsCramped,
  workspaceRouteStatusText
} from "../workspaceRoute";

describe("initial workspace route", () => {
  it("opens on the local inventory overview instead of an empty project inspector", () => {
    expect(INITIAL_WORKSPACE_ROUTE).toMatchObject({
      primaryView: "overview",
      rightPaneView: "dashboard",
      selectedProjectId: null
    });
  });

  it("uses a visible sidebar default while preserving the right-pane choice", () => {
    expect(startupPaneCollapse()).toEqual({ left: false, right: false });
    expect(startupPaneCollapse({ left: true, right: true })).toEqual({ left: false, right: true });
    expect(startupPaneCollapse({ left: true, right: false })).toEqual({ left: false, right: false });
  });

  it("normalizes compact startup preferences and applies explicit pane choices", () => {
    expect(normalizeStartupPreferences(null)).toEqual(DEFAULT_STARTUP_PREFERENCES);
    expect(normalizeStartupPreferences({ destination: "last-workspace", leftPane: "remember", rightPane: "collapsed" })).toEqual({
      destination: "last-workspace",
      leftPane: "remember",
      rightPane: "collapsed"
    });
    expect(startupPaneCollapse(
      { left: true, right: false },
      { destination: "overview", leftPane: "remember", rightPane: "collapsed" }
    )).toEqual({ left: true, right: true });
  });

  it("restores a valid last workspace and falls back when its project disappeared", () => {
    const stored = parseWorkspaceRoute({
      ...INITIAL_WORKSPACE_ROUTE,
      primaryView: "project",
      projectView: "recap",
      rightPaneView: "inspector",
      selectedProjectId: 42
    });
    const preferences = { ...DEFAULT_STARTUP_PREFERENCES, destination: "last-workspace" as const };
    expect(startupWorkspaceRoute(preferences, stored, [42])).toMatchObject({
      primaryView: "project",
      projectView: "recap",
      selectedProjectId: 42
    });
    expect(startupWorkspaceRoute(preferences, stored, [7])).toEqual(INITIAL_WORKSPACE_ROUTE);
    expect(startupWorkspaceRoute(DEFAULT_STARTUP_PREFERENCES, stored, [42])).toEqual(INITIAL_WORKSPACE_ROUTE);
  });
});

describe("workspace route status text", () => {
  it("keeps the status bar aligned with global navigation", () => {
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "overview" })).toBe("Overview opened.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "project", projectView: "context" })).toBe("Project opened: Context.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "project", projectView: "recap" })).toBe("Project opened: What changed.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "project", projectView: "sessions" })).toBe("Project opened: Sessions.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "recovery" })).toBe("Recover opened.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "discover", discoverView: "projects" })).toBe("Discover opened: Find local projects & sessions.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "settings", settingsView: "folders" })).toBe("Settings opened: Scan folders.");
    expect(workspaceRouteStatusText({ ...INITIAL_WORKSPACE_ROUTE, primaryView: "settings", settingsView: "advanced" })).toBe("Settings opened: System & diagnostics.");
  });
});

describe("transient workspace history", () => {
  it("can record the current tab before opening a transient session preview", () => {
    const sessionsRoute = {
      ...INITIAL_WORKSPACE_ROUTE,
      primaryView: "project" as const,
      projectView: "sessions" as const,
      rightPaneView: "inspector" as const,
      selectedProjectId: 42
    };

    expect(shouldRecordWorkspaceRoute(sessionsRoute, sessionsRoute)).toBe(false);
    expect(shouldRecordWorkspaceRoute(sessionsRoute, sessionsRoute, true)).toBe(true);
  });
});

describe("contextual workspace focus", () => {
  it("keeps project navigation visible when the current view has room for it", () => {
    expect(projectSidebarCollapsedForLayout("overview", "context", false, true, false)).toBe(false);
    expect(projectSidebarCollapsedForLayout("project", "context", false, true, false)).toBe(false);
    expect(projectSidebarCollapsedForLayout("project", "sessions", false, true, false)).toBe(false);
    expect(projectSidebarCollapsedForLayout("project", "files", false, true, false)).toBe(true);
    expect(projectSidebarCollapsedForLayout("project", "context", false, true, true)).toBe(true);
    expect(projectSidebarCollapsedForLayout("review", "context", false, true, false)).toBe(true);
    expect(projectSidebarCollapsedForLayout("project", "files", false, false, false)).toBe(false);
    expect(projectSidebarCollapsedForLayout("overview", "context", true, false, false)).toBe(true);
  });

  it("shows file-change notices only while a file-oriented project view is visible", () => {
    expect(focusedFileStatusIsRelevant("project", "context", false)).toBe(true);
    expect(focusedFileStatusIsRelevant("project", "files", false)).toBe(true);
    expect(focusedFileStatusIsRelevant("project", "sessions", false)).toBe(false);
    expect(focusedFileStatusIsRelevant("discover", "files", false)).toBe(false);
    expect(focusedFileStatusIsRelevant("project", "files", true)).toBe(false);
  });

  it("gives project tools a wide canvas until the user explicitly opens details", () => {
    expect(projectViewPrefersWideCanvas("space")).toBe(true);
    expect(projectViewPrefersWideCanvas("recap")).toBe(true);
    expect(projectViewPrefersWideCanvas("connections")).toBe(true);
    expect(projectViewPrefersWideCanvas("sessions")).toBe(true);
    expect(projectViewPrefersWideCanvas("context")).toBe(false);

    expect(projectInspectorCollapsedForLayout("project", "connections", false, false, false)).toBe(true);
    expect(projectInspectorCollapsedForLayout("project", "connections", false, true, false)).toBe(false);
    expect(projectInspectorCollapsedForLayout("project", "context", false, false, false)).toBe(false);
    expect(projectInspectorCollapsedForLayout("project", "context", false, false, true)).toBe(true);
    expect(projectInspectorCollapsedForLayout("project", "context", false, true, true)).toBe(false);
    expect(projectInspectorCollapsedForLayout("project", "space", true, true, false)).toBe(true);
    expect(projectInspectorCollapsedForLayout("review", "context", false, true, false)).toBe(true);
    expect(projectInspectorCollapsedForLayout("discover", "connections", true, false, true)).toBe(false);
  });

  it("protects the center workspace from oversized persisted panes", () => {
    expect(workspaceCenterPaneIsCramped(1282, 445, 460)).toBe(true);
    expect(workspaceCenterPaneIsCramped(1282, 286, 360)).toBe(false);
    expect(workspaceCenterPaneIsCramped(1282, 445, 44)).toBe(false);
    expect(workspaceCenterPaneIsCramped(1282, 44, 460)).toBe(false);
    expect(workspaceCenterPaneIsCramped(1282, -100, -20)).toBe(false);
  });

  it("keeps a docked AI explanation reachable when the Inspector collapses", () => {
    expect(projectInspectorHostsAiExplain(true, "inspector", false, true)).toBe(true);
    expect(projectInspectorHostsAiExplain(true, "inspector", true, true)).toBe(false);
    expect(projectInspectorHostsAiExplain(true, "dashboard", false, true)).toBe(false);
    expect(projectInspectorHostsAiExplain(false, "inspector", false, true)).toBe(false);
    expect(projectInspectorHostsAiExplain(true, "inspector", false, false)).toBe(false);
  });
});
