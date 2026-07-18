import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  projectContextScanAction,
  projectSidebarSummaryLabel,
  shouldShowGuideRail,
  valueEditorAvailable
} from "../views/ProjectCenterView";
import { snapshotOriginLabel } from "../views/project-center/PreviousVersions";
import {
  filterProjectSessions,
  graphFiltersForCounts,
  graphFilterLabel,
  graphIssueContextPath,
  graphSummaryItems,
  groupGraphIssuesForDisplay,
  groupGraphNodesForDisplay,
  nextProgressiveListLimit,
  ProjectConnectionsHome,
  projectAccountingState,
  projectSessionAppOptions,
  sessionCardFacts,
  sessionCardHelp
} from "../views/ProjectHomeViews";
import { readableSourcePreviewText, shouldUseReadableSourcePreview } from "../views/project-center/previewDisplay";
import { graphMapItemCounts, nextGraphMapExpansionLimit } from "../graphMapExpansion";
import type { GraphIssue, GraphMap, GraphNode, ProjectSummary, SessionDiscoveryCandidate } from "../types";

const project: ProjectSummary = {
  id: 42,
  name: "Sample3DProject",
  path: "C:\\Work\\Sample3DProject",
  source: "scan",
  contextCount: 0,
  pinned: false,
  scanState: "outdated",
  scanRootId: 7
};

describe("project context scan action", () => {
  it("shows a direct re-scan action when a stale project has a scan root", () => {
    const onRescanProject = vi.fn();
    const onOpenScanFolders = vi.fn();

    const action = projectContextScanAction({
      project,
      scanState: "outdated",
      canRescanProject: true,
      onRescanProject,
      onOpenScanFolders
    });

    expect(action).toMatchObject({
      kind: "scan",
      label: "Re-scan project",
      disabled: false
    });
    action?.onSelect();
    expect(onRescanProject).toHaveBeenCalledOnce();
    expect(onOpenScanFolders).not.toHaveBeenCalled();
  });

  it("keeps the action visible but disabled while the project is scanning", () => {
    const action = projectContextScanAction({
      project,
      scanState: "scanning",
      canRescanProject: true,
      onRescanProject: vi.fn(),
      onOpenScanFolders: vi.fn()
    });

    expect(action).toMatchObject({
      kind: "scan",
      label: "Scanning project",
      disabled: true
    });
  });

  it("falls back to Scan folders when a stale project has no resolvable root", () => {
    const onRescanProject = vi.fn();
    const onOpenScanFolders = vi.fn();

    const action = projectContextScanAction({
      project,
      scanState: "outdated",
      canRescanProject: false,
      onRescanProject,
      onOpenScanFolders
    });

    expect(action).toMatchObject({
      kind: "folders",
      label: "Open scan folders",
      disabled: false
    });
    action?.onSelect();
    expect(onOpenScanFolders).toHaveBeenCalledOnce();
    expect(onRescanProject).not.toHaveBeenCalled();
  });

  it("does not add scan chrome for projects with a fresh inventory", () => {
    expect(projectContextScanAction({
      project,
      scanState: "scanned",
      canRescanProject: true,
      onRescanProject: vi.fn(),
      onOpenScanFolders: vi.fn()
    })).toBeNull();
  });
});

describe("project footprint accounting copy", () => {
  it("does not call missing or stale totals complete and current", () => {
    expect(projectAccountingState(false, false, null)).toMatchObject({
      label: "Awaiting totals",
      tone: "attention"
    });
    expect(projectAccountingState(true, false, "outdated")).toMatchObject({
      label: "Last scan complete",
      tone: "attention"
    });
  });

  it("distinguishes a partial scan from a fresh complete scan", () => {
    expect(projectAccountingState(true, true, "scanning")).toMatchObject({
      label: "Incomplete count",
      tone: "attention"
    });
    expect(projectAccountingState(true, false, "scanned")).toMatchObject({
      label: "Complete known scan",
      tone: "ready"
    });
  });
});

describe("project sidebar summary label", () => {
  it("keeps the plain N-of-M phrasing when the whole matching list is on screen", () => {
    expect(projectSidebarSummaryLabel({ matchCount: 40, totalCount: 40, collapsed: false, hiddenCount: 0 }))
      .toBe("40 of 40 shown");
    expect(projectSidebarSummaryLabel({ matchCount: 5, totalCount: 40, collapsed: false, hiddenCount: 0 }))
      .toBe("5 of 40 shown");
  });

  it("is honest about the visible-row count while the list is collapsed to a preview", () => {
    // The old copy said "40 of 40 shown" while only 2 rows were rendered.
    expect(projectSidebarSummaryLabel({ matchCount: 40, totalCount: 40, collapsed: true, hiddenCount: 38 }))
      .toBe("40 match · 2 shown");
  });

  it("does not switch phrasing when collapsed but nothing is actually hidden", () => {
    expect(projectSidebarSummaryLabel({ matchCount: 2, totalCount: 40, collapsed: true, hiddenCount: 0 }))
      .toBe("2 of 40 shown");
  });
});

describe("project guide rail visibility", () => {
  it("drops the guide rail (deduping the empty hero) when no project is selected", () => {
    expect(shouldShowGuideRail("context", false)).toBe(false);
    expect(shouldShowGuideRail("space", false)).toBe(false);
    expect(shouldShowGuideRail("recap", false)).toBe(false);
    expect(shouldShowGuideRail("connections", false)).toBe(false);
    expect(shouldShowGuideRail("sessions", false)).toBe(false);
  });

  it("does not duplicate the recommended context queue in a guide rail", () => {
    expect(shouldShowGuideRail("context", true)).toBe(false);
    expect(shouldShowGuideRail("space", true)).toBe(false);
    expect(shouldShowGuideRail("recap", true)).toBe(false);
    expect(shouldShowGuideRail("connections", true)).toBe(false);
    expect(shouldShowGuideRail("sessions", true)).toBe(false);
  });

  it("never shows the guide rail in the Files view (the file tree takes its place)", () => {
    expect(shouldShowGuideRail("files", true)).toBe(false);
    expect(shouldShowGuideRail("files", false)).toBe(false);
  });
});

describe("safe value editing eligibility", () => {
  const preview = {
    nodeId: 1,
    projectId: 1,
    path: "C:\\Work\\config.JSON",
    displayPath: "config.JSON",
    displayName: "config.JSON",
    mode: "source" as const,
    state: "ready" as const,
    fileKind: "text" as const,
    sizeBytes: 20,
    truncated: false,
    previewLimitBytes: 100,
    systemErrorCode: null,
    wasRevealed: false,
    source: "{}",
    renderedHtml: null,
    blockedReason: null,
    headings: [],
    links: []
  };

  it("offers Values only for editable JSON or TOML files", () => {
    expect(valueEditorAvailable(preview, true)).toBe(true);
    expect(valueEditorAvailable({ ...preview, path: "settings.toml" }, true)).toBe(true);
    expect(valueEditorAvailable({ ...preview, path: "notes.md" }, true)).toBe(false);
    expect(valueEditorAvailable(preview, false)).toBe(false);
  });

  it("uses plain-language labels for durable edit origins", () => {
    expect(snapshotOriginLabel("value")).toBe("Value edit");
    expect(snapshotOriginLabel("ai_suggestion")).toBe("AI suggestion");
    expect(snapshotOriginLabel("unknown")).toBe("Saved edit");
  });
});

function graphNode(path: string, nodeId: number): GraphNode {
  return {
    nodeId,
    projectId: project.id,
    path,
    displayName: path.split(/[\\/]/).pop() ?? path,
    itemKind: "file",
    graphKind: "workflow",
    confidence: "medium",
    details: [],
    physicalBytes: 1024,
    protectedOrSensitive: false,
    sharedProjectIds: []
  };
}

function graphIssue(nodeId: number, target: string): GraphIssue {
  return {
    nodeId,
    projectId: project.id,
    kind: "missing_model_reference",
    confidence: "medium",
    target,
    evidence: null
  };
}

describe("project Hangar Map display grouping", () => {
  it("reveals large lists in bounded pages without making the remainder unreachable", () => {
    expect(nextProgressiveListLimit(40, 299)).toBe(80);
    expect(nextProgressiveListLimit(280, 299)).toBe(299);
    expect(nextProgressiveListLimit(0, 12)).toBe(12);
  });

  it("labels missing workflow references without claiming the model is unused", () => {
    expect(graphFilterLabel("orphans")).toBe("Unreferenced models");
  });

  it("groups dependency-cache graph nodes without dropping them", () => {
    const cargoWorkflow = graphNode(".local/cargo/registry/src/index.crates.io-abc/pkg/.github/workflows/ci.yml", 1);
    const directWorkflow = graphNode("workflows/local-review.json", 2);
    const nodeModulesWorkflow = graphNode("node_modules/pkg/workflow.json", 3);

    const grouped = groupGraphNodesForDisplay([cargoWorkflow, directWorkflow, nodeModulesWorkflow]);

    expect(grouped.nodes).toEqual([directWorkflow]);
    expect(grouped.cacheNodes).toEqual([cargoWorkflow, nodeModulesWorkflow]);
    expect([...grouped.nodes, ...grouped.cacheNodes]).toEqual([directWorkflow, cargoWorkflow, nodeModulesWorkflow]);
  });

  it("leaves graph nodes untouched when no dependency cache paths are present", () => {
    const nodes = [
      graphNode("workflows/local-review.json", 1),
      graphNode("models/local.safetensors", 2)
    ];

    expect(groupGraphNodesForDisplay(nodes)).toEqual({ nodes, cacheNodes: [] });
  });

  it("groups references that originate from dependency-cache graph nodes", () => {
    const cargoWorkflow = graphNode(".local/cargo/registry/src/index.crates.io-abc/pkg/.github/workflows/ci.yml", 1);
    const directWorkflow = graphNode(".github/workflows/ci.yml", 2);
    const nodeById = new Map([
      [directWorkflow.nodeId, directWorkflow]
    ]);
    const cacheIssue = {
      ...graphIssue(cargoWorkflow.nodeId, "models/vendor.safetensors"),
      sourcePath: cargoWorkflow.path
    };
    const directIssue = graphIssue(directWorkflow.nodeId, "models/local.safetensors");

    const grouped = groupGraphIssuesForDisplay([cacheIssue, directIssue], nodeById);

    expect(grouped.issues).toEqual([directIssue]);
    expect(grouped.cacheIssues).toEqual([cacheIssue]);
  });

  it("keeps the issue source path available when its graph node is outside the loaded page", () => {
    const issue = {
      ...graphIssue(999, "models/missing.safetensors"),
      sourcePath: "workflows/from-truncated-page.json"
    };
    const loadedNode = graphNode("workflows/loaded.json", 999);

    expect(graphIssueContextPath(issue)).toBe("workflows/from-truncated-page.json");
    expect(graphIssueContextPath(issue, loadedNode)).toBe("workflows/loaded.json");
    expect(graphIssueContextPath(graphIssue(1000, "models/fallback.safetensors"))).toBe(
      "models/fallback.safetensors"
    );
  });

  it("keeps only meaningful filters and surfaces caches in the summary", () => {
    const counts = {
      mapped: 2,
      workflows: 0,
      cachedWorkflows: 0,
      models: 0,
      cachedModels: 0,
      caches: 2,
      modelUses: 0,
      unreferenced: 0,
      risk: 0,
      issues: 0,
      cacheIssues: 0
    };

    expect(graphFiltersForCounts(counts)).toEqual(["all", "caches", "assets"]);
    expect(graphSummaryItems(counts)).toEqual([
      { key: "mapped", count: 2, label: "mapped items" },
      { key: "caches", count: 2, label: "inside dependency caches" },
      { key: "issues", count: 0, label: "need direct review" }
    ]);
  });

  it("drops empty map categories while preserving populated risk and review views", () => {
    const counts = {
      mapped: 10,
      workflows: 0,
      cachedWorkflows: 0,
      models: 10,
      cachedModels: 0,
      caches: 0,
      modelUses: 0,
      unreferenced: 10,
      risk: 1,
      issues: 2,
      cacheIssues: 0
    };

    expect(graphFiltersForCounts(counts)).toEqual(["all", "models", "assets", "orphans", "risk", "issues"]);
    expect(graphSummaryItems(counts).map((item) => item.key)).toEqual([
      "mapped",
      "models",
      "unreferenced",
      "issues"
    ]);
  });

  it("separates direct review signals from dependency-cache observations", () => {
    const counts = {
      mapped: 300,
      workflows: 2,
      cachedWorkflows: 0,
      models: 10,
      cachedModels: 0,
      caches: 1,
      modelUses: 1,
      unreferenced: 3,
      risk: 0,
      issues: 0,
      cacheIssues: 348
    };

    expect(graphFiltersForCounts(counts)).toContain("issues");
    expect(graphSummaryItems(counts).slice(-2)).toEqual([
      { key: "issues", count: 0, label: "need direct review" },
      { key: "cacheIssues", count: 348, label: "cache observations" }
    ]);
  });

  it("keeps cached models discoverable without presenting them as direct orphan candidates", () => {
    const counts = {
      mapped: 299,
      workflows: 0,
      cachedWorkflows: 4,
      models: 0,
      cachedModels: 295,
      caches: 299,
      modelUses: 0,
      unreferenced: 0,
      risk: 0,
      issues: 0,
      cacheIssues: 348
    };

    expect(graphFiltersForCounts(counts)).toEqual(["all", "workflows", "models", "caches", "assets", "issues"]);
    expect(graphFiltersForCounts(counts)).not.toContain("orphans");
  });

  it("reports complete map totals separately from the loaded bounded page", () => {
    const map: GraphMap = {
      projectId: project.id,
      nodes: [
        { ...graphNode(project.path, project.id), graphKind: "project", itemKind: "project" },
        graphNode("workflows/local-review.json", 2)
      ],
      edges: [],
      issues: [],
      totalNodes: 301,
      totalEdges: 0,
      totalIssues: 0,
      partial: false
    };

    expect(graphMapItemCounts(map)).toEqual({ loadedItems: 1, totalItems: 300 });
    expect(nextGraphMapExpansionLimit(map)).toBe(301);

    const largeMap = {
      ...map,
      nodes: Array.from({ length: 1_200 }, (_, index) => ({
        ...graphNode(`models/model-${index}.gguf`, index + 1),
        nodeId: index + 1
      })),
      totalNodes: 10_000
    };
    expect(nextGraphMapExpansionLimit(largeMap)).toBe(2_400);
  });

  it("renders complete-map loading controls with explicit pause and resume states", () => {
    const map: GraphMap = {
      projectId: project.id,
      nodes: [
        { ...graphNode(project.path, project.id), graphKind: "project", itemKind: "project" },
        graphNode("workflows/local-review.json", 2)
      ],
      edges: [],
      issues: [],
      totalNodes: 301,
      totalEdges: 0,
      totalIssues: 0,
      partial: false
    };
    const render = (status: "idle" | "paused") => renderToStaticMarkup(createElement(ProjectConnectionsHome, {
      preview: null,
      relationships: null,
      relationshipsNodeId: null,
      relationshipsLoading: false,
      graphMap: map,
      graphMapLoading: false,
      graphMapError: null,
      graphMapExpansion: { status, loadedItems: 1, totalItems: 300, message: status === "paused" ? "Paused between batches." : null },
      onExpandGraphMap: vi.fn(),
      onPauseGraphMap: vi.fn(),
      onContinueGraphMap: vi.fn(),
      onOpen: vi.fn(),
      onContextMenu: vi.fn()
    }));

    expect(render("idle")).toContain("Load complete map");
    const paused = render("paused");
    expect(paused).toContain("Paused between batches.");
    expect(paused).toContain("Continue");
  });
});

function projectSession(overrides: Partial<SessionDiscoveryCandidate> = {}): SessionDiscoveryCandidate {
  return {
    sourceKind: "claude",
    sourceLabel: "Claude Code projects",
    sessionKind: "Claude session",
    confidence: "high",
    displayName: "Review CodeHangar UX",
    path: "C:\\Users\\sample-user\\.claude\\projects\\C--Work-SampleProject\\local_12345678-1234-1234-1234-123456789abc.jsonl",
    modifiedMs: 1710000000000,
    linkedProjectPaths: ["C:\\Work\\SampleProject"],
    linkedRegisteredProjectIds: [project.id],
    association: "registered_project",
    ...overrides
  };
}

describe("project session cards", () => {
  it("keeps session card facts compact instead of showing raw local paths", () => {
    expect(sessionCardFacts(projectSession())).toEqual(["Project-linked"]);
    expect(sessionCardFacts(projectSession({ linkedProjectPaths: [] }))).toEqual(["No project path"]);
  });

  it("keeps the technical session path available in card help", () => {
    const session = projectSession();
    const help = sessionCardHelp(session);

    expect(help).toContain(session.path);
    expect(help).toContain(session.linkedProjectPaths[0]);
    expect(help).toContain("Open Review CodeHangar UX");
  });

  it("filters project sessions by title and application while keeping newest first", () => {
    const claude = projectSession({ displayName: "Review CodeHangar UX", modifiedMs: 300 });
    const codex = projectSession({
      sourceKind: "codex",
      sourceLabel: "Codex sessions",
      sessionKind: "Codex session",
      displayName: "Implement navigation fix",
      modifiedMs: 200
    });
    const olderClaude = projectSession({ displayName: "Earlier review", modifiedMs: 100 });

    expect(filterProjectSessions([olderClaude, codex, claude], "review", "all"))
      .toEqual([claude, olderClaude]);
    expect(filterProjectSessions([olderClaude, codex, claude], "", "codex"))
      .toEqual([codex]);
    expect(filterProjectSessions([olderClaude, codex, claude], "", "all"))
      .toEqual([claude, codex, olderClaude]);
  });

  it("builds stable app filter options with counts", () => {
    const options = projectSessionAppOptions([
      projectSession(),
      projectSession({ displayName: "Second Claude session" }),
      projectSession({ sourceKind: "codex", sessionKind: "Codex session" })
    ]);

    expect(options).toEqual([
      { slug: "codex", label: "ChatGPT", count: 1 },
      { slug: "claude", label: "Claude", count: 2 }
    ]);
  });
});

describe("project file preview mode", () => {
  it("keeps rendered mode readable for technical text files", () => {
    expect(shouldUseReadableSourcePreview({
      mode: "rendered",
      fileKind: "text",
      source: '{ "name": "app" }',
      renderedHtml: null
    })).toBe(true);
  });

  it("also preserves text when rendered mode only returns escaped HTML", () => {
    expect(shouldUseReadableSourcePreview({
      mode: "rendered",
      fileKind: "text",
      source: null,
      renderedHtml: "{ &quot;name&quot;: &quot;app&quot; }"
    })).toBe(true);
  });

  it("uses the file name as a fallback for structured text previews", () => {
    expect(shouldUseReadableSourcePreview({
      mode: "rendered",
      fileKind: "unsupported",
      displayName: "package.json",
      path: "package.json",
      source: null,
      renderedHtml: "{ &quot;name&quot;: &quot;app&quot; }"
    })).toBe(true);
  });

  it("turns escaped rendered JSON into an indented readable block", () => {
    expect(readableSourcePreviewText({
      displayName: "package.json",
      source: null,
      renderedHtml: "{&quot;name&quot;:&quot;app&quot;,&quot;scripts&quot;:{&quot;dev&quot;:&quot;vite&quot;}}"
    })).toBe([
      "{",
      "  \"name\": \"app\",",
      "  \"scripts\": {",
      "    \"dev\": \"vite\"",
      "  }",
      "}"
    ].join("\n"));
  });

  it("leaves Markdown in the rendered HTML path", () => {
    expect(shouldUseReadableSourcePreview({
      mode: "rendered",
      fileKind: "markdown",
      source: "# README",
      renderedHtml: "<h1>README</h1>"
    })).toBe(false);
  });

  it("does not replace explicit Source mode or empty previews", () => {
    expect(shouldUseReadableSourcePreview({
      mode: "source",
      fileKind: "text",
      source: '{ "name": "app" }',
      renderedHtml: null
    })).toBe(false);
    expect(shouldUseReadableSourcePreview({
      mode: "rendered",
      fileKind: "text",
      source: "   ",
      renderedHtml: null
    })).toBe(false);
  });
});
