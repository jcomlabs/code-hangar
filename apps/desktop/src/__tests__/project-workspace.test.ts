import { describe, expect, it } from "vitest";
import {
  compactSidebarProjects,
  composeQuickOpenResults,
  orderSidebarProjects,
  projectMatchesSidebarQuery,
  projectSearchKeyAction,
  projectStatusBucketFromScanState,
  projectWatchLabel,
  quickOpenSearchMessage,
  resolveProjectScanState,
  shouldRenderProjectRow,
  starterQuickOpenResults,
  shouldShowDemoProjects,
  visibleProjectItems,
  visibleProjects
} from "../projectVisibility";
import type { ProjectSummary } from "../types";
import {
  initialProjectWorkspaceState,
  projectWorkspaceReducer,
  selectedProjectActivation,
  type ProjectWorkspaceAction
} from "../useProjectWorkspace";
import { INITIAL_CONTEXT_OPEN_OPTIONS, selectInitialContextFile } from "../projectAutoOpen";
import { projectInspectorContext, projectViewUsesFileInspector } from "../projectInspector";
import type { NavItem } from "../types";
import { folderClassificationLabel, folderInventoryLabel } from "../views/project-center/FolderOverviewPane";

const demoProject: ProjectSummary = {
  id: 1,
  name: "Demo",
  path: "fixture://demo",
  source: "fixture",
  contextCount: 1,
  pinned: false,
  scanState: "scanned",
  scanRootId: null
};

const realProject: ProjectSummary = {
  id: 2,
  name: "Real",
  path: "C:\\Real",
  source: "scan",
  contextCount: 2,
  pinned: false,
  scanState: "scanned",
  scanRootId: 2
};

describe("demo project visibility", () => {
  it("shows demos automatically only while there are no real projects", () => {
    expect(shouldShowDemoProjects([demoProject], null)).toBe(true);
    expect(visibleProjects([demoProject, realProject], null)).toEqual([realProject]);
  });

  it("respects an explicit local preference", () => {
    expect(visibleProjects([demoProject, realProject], true)).toEqual([demoProject, realProject]);
    expect(visibleProjects([demoProject], false)).toEqual([]);
  });

  it("hides project-owned items for hidden demo projects", () => {
    const items = [
      { nodeId: 10, projectId: demoProject.id, label: "README.md" },
      { nodeId: 20, projectId: realProject.id, label: "README.md" }
    ];

    expect(visibleProjectItems(items, [demoProject, realProject], null)).toEqual([items[1]]);
    expect(visibleProjectItems(items, [demoProject, realProject], true)).toEqual(items);
  });

  it("adds matching projects as first-class Quick Open targets", () => {
    const codeHangarProject: ProjectSummary = {
      ...realProject,
      id: 42,
      name: "CodeHangar",
      path: "C:\\Work\\SampleProject"
    };
    const fileResult = {
      nodeId: 100,
      projectId: 42,
      label: "CodeHangar_Master_Spec_v20_Final.md",
      path: "CodeHangar_Master_Spec_v20_Final.md",
      itemKind: "file",
      score: 100
    };

    const results = composeQuickOpenResults("CodeHangar", [fileResult], [codeHangarProject], null);

    expect(results[0]).toMatchObject({
      nodeId: 42,
      projectId: 42,
      label: "CodeHangar",
      itemKind: "project"
    });
    expect(results[1]).toBe(fileResult);
  });

  it("matches multi-term project queries without requiring a literal phrase", () => {
    const codeHangarProject: ProjectSummary = {
      ...realProject,
      id: 42,
      name: "CodeHangar",
      path: "C:\\AI\\Codex\\CodeHangar"
    };

    expect(composeQuickOpenResults("Code Hangar", [], [codeHangarProject], null)[0]).toMatchObject({
      projectId: 42,
      itemKind: "project"
    });
    expect(composeQuickOpenResults("README CodeHangar", [], [codeHangarProject], null)).toEqual([]);
  });

  it("does not add hidden demo projects to Quick Open", () => {
    const demoFile = {
      nodeId: 10,
      projectId: demoProject.id,
      label: "Demo README.md",
      path: "README.md",
      itemKind: "file",
      score: 100
    };

    expect(composeQuickOpenResults("Demo", [demoFile], [demoProject, realProject], null)).toEqual([]);
    expect(composeQuickOpenResults("Demo", [demoFile], [demoProject], null)[0]?.itemKind).toBe("project");
  });

  it("offers useful starter projects before typing in Quick Open", () => {
    const needsScan: ProjectSummary = {
      ...realProject,
      id: 3,
      name: "NeedsScan",
      contextCount: 0,
      scanState: "outdated"
    };
    const active: ProjectSummary = {
      ...realProject,
      id: 4,
      name: "ActiveProject",
      contextCount: 6
    };

    const results = starterQuickOpenResults([needsScan, realProject, active], active.id);

    expect(results.map((result) => result.label)).toEqual(["ActiveProject", "Real", "NeedsScan"]);
    expect(results[0]).toMatchObject({ itemKind: "project", projectId: active.id });
  });

  it("prefers scanned projects with context for empty Quick Open suggestions", () => {
    const emptyRoot: ProjectSummary = {
      ...realProject,
      id: 3,
      name: "EmptyRoot",
      contextCount: 0,
      scanState: "outdated"
    };
    const richContext: ProjectSummary = {
      ...realProject,
      id: 4,
      name: "RichContext",
      contextCount: 12,
      scanState: "scanned"
    };

    expect(starterQuickOpenResults([emptyRoot, richContext], null)[0]?.label).toBe("RichContext");
  });

  it("keeps asynchronous Quick Open feedback honest", () => {
    expect(quickOpenSearchMessage("README", 0, "loading")).toBe("Searching indexed files...");
    expect(quickOpenSearchMessage("README", 0, "idle")).toBe("No projects or files match this query.");
    expect(quickOpenSearchMessage("R", 0, "idle")).toBe("Type one more character to search indexed files.");
    expect(quickOpenSearchMessage("README", 2, "error")).toBe("Indexed file search is unavailable. Project matches are still shown.");
    expect(quickOpenSearchMessage("README", 2, "idle")).toBeNull();
  });
});

describe("project inspector context", () => {
  it("uses file details only for views that inspect a selected file", () => {
    expect(projectViewUsesFileInspector("context")).toBe(true);
    expect(projectViewUsesFileInspector("files")).toBe(true);
    expect(projectViewUsesFileInspector("connections")).toBe(true);
    expect(projectViewUsesFileInspector("space")).toBe(false);
    expect(projectViewUsesFileInspector("sessions")).toBe(false);
  });

  it("uses project copy on Sessions so stale file details do not leak into the panel", () => {
    const context = projectInspectorContext("sessions", "SampleAgent", 2);

    expect(context.sectionLabel).toBe("Project details");
    expect(context.subject).toBe("Project sessions");
    expect(context.emptyTitle).toBe("SampleAgent sessions");
    expect(context.emptyBody).toContain("2 linked sessions");
    expect(context.emptyBody).toContain("file checks stay in Context, Files and Connections");
  });
});

describe("folder overview copy", () => {
  it("turns backend classification keys into readable labels", () => {
    expect(folderClassificationLabel("project-source")).toBe("Project source");
    expect(folderClassificationLabel("dependency_cache")).toBe("Dependency cache");
    expect(folderClassificationLabel("  ")).toBe("Folder");
  });

  it("keeps partial and failed inventory states distinct", () => {
    expect(folderInventoryLabel({ fullyScanned: true, scanError: null })).toBe("Complete");
    expect(folderInventoryLabel({ fullyScanned: false, scanError: null })).toBe("Partial");
    expect(folderInventoryLabel({ fullyScanned: false, scanError: "access denied" })).toBe("Scan issue");
  });
});

describe("sidebar project filtering", () => {
  it("treats a provably empty project as ready and labels it Empty", () => {
    expect(resolveProjectScanState("outdated", "empty", false, false)).toBe("scanned");
    expect(projectStatusBucketFromScanState(resolveProjectScanState("outdated", "empty", false, false))).toBe("ready");
    expect(projectWatchLabel("empty")).toBe("Empty");
  });

  it("keeps a non-empty unscanned project in Needs scan", () => {
    expect(resolveProjectScanState("scanned", "needs_scan", false, false)).toBe("outdated");
    expect(projectWatchLabel("needs_scan")).toBe("Needs scan");
  });

  it("matches project search against names, folder paths, aliases and app labels", () => {
    const antigravityProject: ProjectSummary = {
      ...realProject,
      id: 3,
      name: "Sample3DProject",
      path: "C:\\Work\\Sample3DProject",
      app: "antigravity",
      apps: ["antigravity", "codex"],
      antigravityName: "3D All Lab"
    };

    expect(projectMatchesSidebarQuery(antigravityProject, "3d lab")).toBe(true);
    expect(projectMatchesSidebarQuery(antigravityProject, "Sample3DProject")).toBe(true);
    expect(projectMatchesSidebarQuery(antigravityProject, "ChatGPT")).toBe(true);
    expect(projectMatchesSidebarQuery(antigravityProject, "Codex")).toBe(true);
    expect(projectMatchesSidebarQuery(antigravityProject, "missing")).toBe(false);
  });

  it("combines search, app filter, status filter and archived grouping", () => {
    const alpha: ProjectSummary = {
      ...realProject,
      id: 10,
      name: "Alpha",
      path: "C:\\Work\\Alpha",
      app: "claude",
      apps: ["claude"],
      scanState: "outdated"
    };
    const beta: ProjectSummary = {
      ...realProject,
      id: 11,
      name: "Beta",
      path: "C:\\Work\\Beta",
      app: "claude",
      apps: ["claude"],
      scanState: "outdated"
    };
    const gamma: ProjectSummary = {
      ...realProject,
      id: 12,
      name: "Gamma",
      path: "C:\\Work\\Gamma",
      app: "codex",
      apps: ["codex"],
      scanState: "scanned"
    };

    const ordered = orderSidebarProjects([beta, gamma, alpha], {
      sort: "name",
      appFilter: "claude",
      statusFilter: "needs-scan",
      query: "a",
      archivedProjectIds: new Set([beta.id]),
      getStatusBucket: (project) => projectStatusBucketFromScanState(project.scanState)
    });

    expect(ordered.active.map((project) => project.name)).toEqual(["Alpha"]);
    expect(ordered.archived.map((project) => project.name)).toEqual(["Beta"]);
    expect(ordered.all.map((project) => project.name)).toEqual(["Alpha", "Beta"]);
  });

  it("compacts the project sidebar to a short preview by default", () => {
    const projects = [1, 2, 3, 4, 5].map((id) => ({ ...realProject, id, name: `Project ${id}` }));

    const compact = compactSidebarProjects(projects, { limit: 3 });

    expect(compact).toMatchObject({ hiddenCount: 2, compacted: true });
    expect(compact.projects.map((project) => project.id)).toEqual([1, 2, 3]);
  });

  it("keeps the selected project visible when compacting a long sidebar", () => {
    const projects = [1, 2, 3, 4, 5].map((id) => ({ ...realProject, id, name: `Project ${id}` }));

    const compact = compactSidebarProjects(projects, { limit: 3, selectedProjectId: 5 });

    expect(compact.projects.map((project) => project.id)).toEqual([1, 2, 5]);
  });

  it("maps project search keyboard shortcuts to list actions", () => {
    expect(projectSearchKeyAction("Enter", "CodeHangar", 1)).toBe("open-first");
    expect(projectSearchKeyAction("Enter", "Missing", 0)).toBe("none");
    expect(projectSearchKeyAction("Escape", "CodeHangar", 1)).toBe("clear");
    expect(projectSearchKeyAction("Escape", "", 40)).toBe("none");
  });

  it("keeps the selected archived project visible while Archived is collapsed", () => {
    expect(shouldRenderProjectRow({ isArchived: true, archivedCollapsed: true, isSelected: true })).toBe(true);
    expect(shouldRenderProjectRow({ isArchived: true, archivedCollapsed: true, isSelected: false })).toBe(false);
    expect(shouldRenderProjectRow({ isArchived: true, archivedCollapsed: false, isSelected: false })).toBe(true);
    expect(shouldRenderProjectRow({ isArchived: false, archivedCollapsed: true, isSelected: false })).toBe(true);
  });
});

describe("project workspace reducer", () => {
  it("never resets an already selected project to loading without starting work", () => {
    expect(selectedProjectActivation("ready")).toBe("reuse");
    expect(selectedProjectActivation("loading")).toBe("wait");
    expect(selectedProjectActivation("error")).toBe("reload");
    expect(selectedProjectActivation("idle")).toBe("reload");
  });

  const begin = (projectId: number, generation: number): ProjectWorkspaceAction => ({
    type: "begin",
    projectId,
    generation
  });

  it("clears project-scoped data immediately when switching projects", () => {
    const loaded = {
      ...initialProjectWorkspaceState,
      activeProjectId: 1,
      generation: 2,
      loadStatus: "ready" as const,
      treePages: { root: { items: [], total: 10, hasMore: false } },
      contextFiles: [{
        navId: 1,
        nodeId: 1,
        projectId: 1,
        path: "README.md",
        displayName: "README.md",
        priority: 1,
        contextRank: 0,
        contextGroup: "Project overview",
        recommendationReason: "Root README usually gives the fastest project overview.",
        recommended: true,
        isSensitive: false,
        protectedLevel: null
      }],
      gitStatus: {
        projectId: 1,
        hasGit: true,
        currentBranch: "main",
        headRef: null,
        originUrl: null,
        metadataError: null
      }
    };

    const switched = projectWorkspaceReducer(loaded, begin(2, 3));
    expect(switched.activeProjectId).toBe(2);
    expect(switched.loadStatus).toBe("loading");
    expect(switched.treePages).toEqual({});
    expect(switched.contextFiles).toEqual([]);
    expect(switched.gitStatus).toBeNull();
  });

  it("ignores a late response from the previous project", () => {
    const loadingOne = projectWorkspaceReducer(initialProjectWorkspaceState, begin(1, 1));
    const loadingTwo = projectWorkspaceReducer(loadingOne, begin(2, 2));
    const staleSuccess: ProjectWorkspaceAction = {
      type: "load-success",
      projectId: 1,
      generation: 1,
      rootPage: { items: [], total: 99, hasMore: false },
      contextFiles: [],
      gitStatus: {
        projectId: 1,
        hasGit: false,
        currentBranch: null,
        headRef: null,
        originUrl: null,
        metadataError: null
      },
      resetExpansion: true
    };

    expect(projectWorkspaceReducer(loadingTwo, staleSuccess)).toBe(loadingTwo);
  });

  it("marks the project ready as soon as root tree data arrives", () => {
    const loading = projectWorkspaceReducer(initialProjectWorkspaceState, begin(2, 4));
    const rootReady = projectWorkspaceReducer(loading, {
      type: "root-success",
      projectId: 2,
      generation: 4,
      rootPage: { items: [navItem(1, "README.md")], total: 1, hasMore: false },
      resetExpansion: true
    });

    expect(rootReady.loadStatus).toBe("ready");
    expect(rootReady.treePages.root.items).toHaveLength(1);
    expect(rootReady.contextFiles).toEqual([]);
    expect(rootReady.gitStatus).toBeNull();
  });

  it("ignores an older response when the same project has a newer load in flight", () => {
    const loading = projectWorkspaceReducer(initialProjectWorkspaceState, begin(2, 4));
    const churned = { ...loading, generation: 9 };
    const staleRoot = projectWorkspaceReducer(churned, {
      type: "root-success",
      projectId: 2,
      generation: 4,
      rootPage: { items: [navItem(1, "README.md")], total: 1, hasMore: false },
      resetExpansion: true
    });
    expect(staleRoot).toBe(churned);

    const rootReady = projectWorkspaceReducer(churned, {
      type: "root-success",
      projectId: 2,
      generation: 9,
      rootPage: { items: [navItem(2, "fresh.md")], total: 1, hasMore: false },
      resetExpansion: true
    });
    expect(rootReady.loadStatus).toBe("ready");
    expect(rootReady.treePages.root.items).toHaveLength(1);
    expect(rootReady.treePages.root.items[0]?.displayName).toBe("fresh.md");
  });

  it("ignores a stale timeout while a newer same-project load is running", () => {
    const loading = projectWorkspaceReducer(initialProjectWorkspaceState, begin(2, 4));
    const newer = projectWorkspaceReducer(loading, { type: "load-start", generation: 5 });
    const staleError = projectWorkspaceReducer(newer, {
      type: "load-error",
      projectId: 2,
      generation: 4,
      error: "old timeout"
    });
    expect(staleError).toBe(newer);
  });

  it("still ignores a churned-generation result from a project switched away from", () => {
    const loadingTwo = projectWorkspaceReducer(
      projectWorkspaceReducer(initialProjectWorkspaceState, begin(1, 1)),
      begin(2, 2)
    );
    const otherProjectRoot = projectWorkspaceReducer(loadingTwo, {
      type: "root-success",
      projectId: 1,
      generation: 1,
      rootPage: { items: [navItem(1, "README.md")], total: 1, hasMore: false },
      resetExpansion: true
    });
    expect(otherProjectRoot).toBe(loadingTwo);
  });

  it("loads context and git side data without resetting the ready tree", () => {
    const loading = projectWorkspaceReducer(initialProjectWorkspaceState, begin(2, 4));
    const rootReady = projectWorkspaceReducer(loading, {
      type: "root-success",
      projectId: 2,
      generation: 4,
      rootPage: { items: [navItem(1, "README.md")], total: 1, hasMore: false },
      resetExpansion: true
    });
    const withSideData = projectWorkspaceReducer(rootReady, {
      type: "side-data-success",
      projectId: 2,
      generation: 4,
      contextFiles: [{
        navId: 1,
        nodeId: 1,
        projectId: 2,
        path: "README.md",
        displayName: "README.md",
        priority: 1,
        contextRank: 0,
        contextGroup: "Project overview",
        recommendationReason: "Root README usually gives the fastest project overview.",
        recommended: true,
        isSensitive: false,
        protectedLevel: null
      }],
      gitStatus: {
        projectId: 2,
        hasGit: false,
        currentBranch: null,
        headRef: null,
        originUrl: null,
        metadataError: null
      }
    });

    expect(withSideData.loadStatus).toBe("ready");
    expect(withSideData.treePages.root.items).toHaveLength(1);
    expect(withSideData.contextFiles).toHaveLength(1);
    expect(withSideData.gitStatus?.projectId).toBe(2);
  });

  it("exposes a controlled error that can be retried", () => {
    const loading = projectWorkspaceReducer(initialProjectWorkspaceState, begin(2, 4));
    const failed = projectWorkspaceReducer(loading, {
      type: "load-error",
      projectId: 2,
      generation: 4,
      error: "database busy"
    });

    expect(failed.loadStatus).toBe("error");
    expect(failed.error).toBe("database busy");
  });

  it("appends paged tree results in order for Show next items", () => {
    const first = projectWorkspaceReducer(initialProjectWorkspaceState, {
      type: "tree-page",
      key: "root",
      page: { items: [navItem(1, "a.md"), navItem(2, "b.md")], total: 4, hasMore: true },
      append: false
    });
    const second = projectWorkspaceReducer(first, {
      type: "tree-page",
      key: "root",
      page: { items: [navItem(3, "c.md"), navItem(4, "d.md")], total: 4, hasMore: false },
      append: true
    });

    expect(second.treePages.root.items.map((item) => item.displayName)).toEqual(["a.md", "b.md", "c.md", "d.md"]);
    expect(second.treePages.root.hasMore).toBe(false);
  });

  it("injects and expands a reveal path without advancing the backend page offset", () => {
    const root = { ...navItem(10, "models"), projectId: 2, itemKind: "directory", childCount: 500 };
    const model = { ...navItem(99, "large.gguf"), projectId: 2, parentNavId: 10 };
    const ready = {
      ...initialProjectWorkspaceState,
      activeProjectId: 2,
      generation: 7,
      loadStatus: "ready" as const,
      treePages: {
        root: { items: [root], total: 1, hasMore: false, nextOffset: 1 },
        "10": { items: [{ ...navItem(11, "first.gguf"), projectId: 2, parentNavId: 10 }], total: 500, hasMore: true, nextOffset: 200 }
      }
    };

    const revealed = projectWorkspaceReducer(ready, {
      type: "reveal-path",
      projectId: 2,
      generation: 7,
      path: [root, model]
    });

    expect(revealed.expandedTree.has(10)).toBe(true);
    expect(revealed.treePages["10"].items.some((item) => item.nodeId === 99)).toBe(true);
    expect(revealed.treePages["10"].nextOffset).toBe(200);
  });
});

describe("project auto-open context choice", () => {
  const contextFile = (overrides: Partial<ReturnType<typeof contextFileBase>> = {}) => ({
    ...contextFileBase(),
    ...overrides
  });

  it("opens the highest-ranked recommended safe context first", () => {
    const chosen = selectInitialContextFile([
      contextFile({ nodeId: 3, displayName: "notes.md", recommended: false, contextRank: 0, priority: 1 }),
      contextFile({ nodeId: 2, displayName: "README.md", recommended: true, contextRank: 2, priority: 1 }),
      contextFile({ nodeId: 1, displayName: "package.json", recommended: true, contextRank: 0, priority: 1 })
    ]);

    expect(chosen?.nodeId).toBe(1);
  });

  it("prefers a non-sensitive context file when the top-ranked one is protected", () => {
    const chosen = selectInitialContextFile([
      contextFile({ nodeId: 1, displayName: ".env.example", recommended: true, contextRank: 0, isSensitive: true }),
      contextFile({ nodeId: 2, displayName: "README.md", recommended: true, contextRank: 1 })
    ]);

    expect(chosen?.nodeId).toBe(2);
  });

  it("returns null when a project has no loaded context", () => {
    expect(selectInitialContextFile([])).toBeNull();
  });

  it("previews the first context file without switching the workspace to Files", () => {
    expect(INITIAL_CONTEXT_OPEN_OPTIONS).toMatchObject({
      allowProjectSwitch: false,
      recordRecent: false,
      replaceHistory: true,
      refreshOnly: true
    });
  });
});

function contextFileBase() {
  return {
    navId: 1,
    nodeId: 1,
    projectId: 2,
    path: "README.md",
    displayName: "README.md",
    priority: 1,
    contextRank: 0,
    contextGroup: "Project overview",
    recommendationReason: "Root README usually gives the fastest project overview.",
    recommended: true,
    isSensitive: false,
    protectedLevel: null
  };
}

function navItem(id: number, displayName: string): NavItem {
  return {
    id,
    projectId: 1,
    nodeId: id,
    parentNavId: null,
    path: displayName,
    displayPath: displayName,
    displayName,
    itemKind: "file",
    priority: id,
    isContext: false,
    isMarkdown: displayName.endsWith(".md"),
    isSensitive: false,
    protectedLevel: null,
    childCount: 0,
    fullyScanned: true,
    collapseDefault: false,
    scanError: null,
    aggregateApparentBytes: null,
    aggregateAllocatedBytes: null,
    aggregatePhysicalBytes: null,
    aggregateBytesPartial: false,
    children: []
  };
}
