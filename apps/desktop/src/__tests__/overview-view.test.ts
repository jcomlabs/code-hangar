import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { OverviewView, buildReviewInboxItems, overviewFlowSafetyCopy, overviewInventoryHealth, overviewRecapQueueSessions, overviewRecapStep, overviewSafeManageStep, overviewTweakStep, overviewUnderstandProjectStep, reviewInboxTitle } from "../views/OverviewView";
import type { DashboardSummary, ProjectReviewCheckpoint, ProjectSummary, SessionDiscoveryCandidate, WatcherStatus } from "../types";

describe("overview recommended flow", () => {
  it("turns the first retrospective step into a project picker", () => {
    expect(overviewRecapStep(null)).toMatchObject({
      label: "Choose a project",
      detail: "Pick one from the sidebar to inspect its recent work."
    });
  });

  it("opens project context only after a project is selected", () => {
    expect(overviewUnderstandProjectStep(null).enabled).toBe(false);
    expect(overviewUnderstandProjectStep(42)).toMatchObject({
      enabled: true,
      label: "Understand the code",
      detail: "Read its context, then explain the files that matter."
    });
  });

  it("keeps the final journey step honest for mutable and read-only editions", () => {
    expect(overviewTweakStep(42, true)).toMatchObject({ enabled: true, label: "Make one safe tweak" });
    expect(overviewTweakStep(42, false)).toMatchObject({ enabled: true, label: "Inspect one file" });
    expect(overviewTweakStep(null, true).enabled).toBe(false);
  });

  it("explains why Safe Manage is disabled when no project is selected", () => {
    expect(overviewSafeManageStep(null)).toMatchObject({
      enabled: false,
      detail: "Choose a project first to load a review."
    });
  });

  it("keeps Safe Manage active once a project is selected", () => {
    expect(overviewSafeManageStep(42)).toMatchObject({
      enabled: true,
      detail: "Review ownership, protected paths and shared files."
    });
  });

  it("describes the actual build capability in the recommended flow", () => {
    expect(overviewFlowSafetyCopy(false)).toContain("read-only");
    expect(overviewFlowSafetyCopy(true)).toContain("one file, one value or selection");
    expect(overviewFlowSafetyCopy(true)).toContain("durable undo");
    expect(overviewFlowSafetyCopy(true)).not.toContain("read-only");
  });

  it("keeps the recap queue to the three newest linked sessions", () => {
    const sessions = [1, 4, 2, 3].map((modifiedMs): SessionDiscoveryCandidate => ({
      path: `fixture://${modifiedMs}.jsonl`,
      displayName: `Session ${modifiedMs}`,
      sourceKind: "codex",
      sourceLabel: "Codex",
      sessionKind: "Codex session",
      confidence: "high",
      linkedProjectPaths: [],
      linkedRegisteredProjectIds: [42],
      association: "registered_project",
      modifiedMs
    }));

    expect(overviewRecapQueueSessions(sessions).map((session) => session.modifiedMs)).toEqual([4, 3, 2]);
  });

  it("builds a cross-project inbox from the saved review cutoffs", () => {
    const project = { id: 42, name: "Inbox project", path: "fixture://inbox", source: "fixture", contextCount: 1, pinned: false, scanState: "scanned" } satisfies ProjectSummary;
    const sessions = [100, 250, null].map((modifiedMs, index): SessionDiscoveryCandidate => ({
      path: `fixture://session-${index}.jsonl`,
      displayName: `Session ${index}`,
      sourceKind: "codex",
      sourceLabel: "Codex",
      sessionKind: "Codex session",
      confidence: "high",
      linkedProjectPaths: [project.path],
      linkedRegisteredProjectIds: [project.id],
      association: "registered_project",
      modifiedMs
    }));
    const checkpoint = { projectId: 42, reviewedAt: "2026-07-14T09:00:00Z", sessionCutoffMs: 200 } satisfies ProjectReviewCheckpoint;

    expect(buildReviewInboxItems([{ project, sessions }], [checkpoint])).toMatchObject([{
      project: { id: 42 },
      unreviewedCount: 1,
      unknownTimestampCount: 1,
      latestModifiedMs: 250,
      hasCheckpoint: true
    }]);
    expect(reviewInboxTitle(true, false, null, 1, 1)).toBe("1 new session record needs review · 1 undated");
  });
});

const dashboard: DashboardSummary = {
  totalProjects: 43,
  totalItems: 260728,
  contextFiles: 556,
  indexedDocuments: 480,
  nonIndexedItems: 224639,
  partialItems: 0,
  gitProjects: 18,
  sensitiveFiles: 937,
  protectedFiles: 963,
  scanRoots: 40,
  largestProjects: [],
  staleOrDirty: "current",
  adaptersNeedingReview: 0
};

const watcher: WatcherStatus = {
  generatedAtMs: 0,
  pollIntervalMs: 15000,
  debounceMs: 1000,
  staleProjects: 6,
  changedProjects: 0,
  projects: [],
  message: "6 known project root(s) need attention or a focused rescan."
};

describe("overview information density", () => {
  it("keeps technical Git and detector details collapsed on first render", () => {
    const html = renderToStaticMarkup(createElement(OverviewView, {
      showFlow: false,
      selectedProjectId: 42,
      realProjectCount: dashboard.totalProjects,
      mutationAvailable: false,
      dashboard,
      watcherStatus: watcher,
      dashboardLoading: false,
      gitStatus: null,
      adapters: [{
        id: 1,
        name: "generic_git_project",
        version: "1",
        adapterType: "builtin",
        source: "local",
        enabled: true,
        description: "Local Git detector"
      }],
      demosVisible: false,
      demoPreference: false,
      reduceMotion: true,
      formatBytes: (value: number) => `${value} B`,
      formatOptionalBytes: (value?: number | null) => `${value ?? 0} B`,
      onOpenProject: () => undefined,
      onAddProjects: () => undefined,
      onSetShowDemoProjects: () => undefined,
      onOpenScanFolders: () => undefined,
      onUnderstandProject: () => undefined,
      onOpenFiles: () => undefined,
      reviewProjectGroups: [],
      reviewInventoryReady: true,
      onOpenRecap: () => undefined,
      onOpenProjectRecap: () => undefined,
      onDiscover: () => undefined,
      onReview: () => undefined,
      onRecovery: () => undefined
    }));

    expect(html).toContain("Show details");
    expect(html).not.toContain("Local Git Signals");
    expect(html).not.toContain("Local Detectors");
  });
});

describe("overview inventory health", () => {
  it("summarizes stale roots as the primary overview state", () => {
    const health = overviewInventoryHealth(dashboard, watcher);

    expect(health).toMatchObject({
      tone: "attention",
      title: "6 scan roots need attention",
      progress: 85
    });
    expect(health.facts.map((fact) => fact.label)).toEqual(["Projects", "Roots", "Context"]);
  });

  it("surfaces partial scans when roots are otherwise fresh", () => {
    const health = overviewInventoryHealth({ ...dashboard, partialItems: 12 }, { ...watcher, staleProjects: 0 });

    expect(health).toMatchObject({
      tone: "attention",
      title: "Inventory mapped with partial scans",
      progress: 82
    });
  });

  it("shows a ready state when the inventory has no reported scan attention", () => {
    const health = overviewInventoryHealth(dashboard, { ...watcher, staleProjects: 0 });

    expect(health).toMatchObject({
      tone: "ready",
      title: "Inventory looks current",
      progress: 100
    });
  });
});
