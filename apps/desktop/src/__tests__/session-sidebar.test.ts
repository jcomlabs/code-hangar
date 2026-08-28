import { describe, expect, it } from "vitest";

import {
  SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT,
  SIDEBAR_SESSION_GROUP_ITEM_LIMIT,
  SIDEBAR_SESSION_SEARCH_ITEM_LIMIT,
  compactSidebarSessionGroups,
  displayedSidebarSessionGroups,
  previewSidebarSessionItems,
  sessionMatchesSidebarQuery
} from "../sessionSidebar";
import type { ProjectSummary, SessionDiscoveryCandidate } from "../types";

const project: ProjectSummary = {
  id: 42,
  name: "CodeHangar",
  path: "C:\\Work\\SampleProject",
  source: "scan",
  contextCount: 19,
  pinned: false,
  scanState: "scanned",
  scanRootId: 42,
  app: "codex",
  apps: ["codex", "claude"]
};

function session(overrides: Partial<SessionDiscoveryCandidate> = {}): SessionDiscoveryCandidate {
  return {
    path: "C:\\Users\\sample-user\\.codex\\sessions\\rollout.jsonl",
    displayName: "Fix sidebar UX",
    sourceKind: "codex",
    sourceLabel: "Codex",
    sessionKind: "codex",
    confidence: "high",
    linkedProjectPaths: ["C:\\Work\\SampleProject"],
    linkedRegisteredProjectIds: [42],
    association: "registered_project",
    modifiedMs: 2000,
    ...overrides
  };
}

describe("sidebar session filtering", () => {
  it("matches search against session metadata, linked project and app labels", () => {
    const candidate = session({
      displayName: "Review markdown renderer",
      sessionKind: "claude_code",
      sourceKind: "claude",
      sourceLabel: "Claude",
      path: "C:\\Users\\sample-user\\.claude\\projects\\sample-project\\session.jsonl"
    });

    expect(sessionMatchesSidebarQuery(candidate, "markdown claude", project)).toBe(true);
    expect(sessionMatchesSidebarQuery(candidate, "CodeHangar", project)).toBe(true);
    expect(sessionMatchesSidebarQuery(candidate, ".claude projects")).toBe(true);
    expect(sessionMatchesSidebarQuery(candidate, "antigravity")).toBe(false);
  });

  it("combines app filter, search and recent sorting without losing group counts", () => {
    const older = session({ displayName: "Older Claude pass", sourceKind: "claude", sessionKind: "claude", modifiedMs: 1000 });
    const newer = session({ displayName: "Newer Claude pass", sourceKind: "claude", sessionKind: "claude", modifiedMs: 3000 });
    const codex = session({ displayName: "Codex pass", sourceKind: "codex", sessionKind: "codex", modifiedMs: 5000 });

    const displayed = displayedSidebarSessionGroups({
      projectGroups: [{ project, sessions: [older, codex, newer] }],
      independent: [session({ displayName: "Loose Claude note", sourceKind: "claude", sessionKind: "claude", linkedProjectPaths: [], linkedRegisteredProjectIds: [], association: "loose_session", modifiedMs: 2500 })],
      hermes: []
    }, {
      sort: "recent",
      appFilter: "claude",
      query: "Claude"
    });

    expect(displayed.count).toBe(3);
    expect(displayed.projectGroups[0]?.sessions.map((item) => item.displayName)).toEqual([
      "Newer Claude pass",
      "Older Claude pass"
    ]);
    expect(displayed.independent.map((item) => item.displayName)).toEqual(["Loose Claude note"]);
    expect(displayed.hermes).toEqual([]);
  });

  it("filters directly to independent or project-linked sessions", () => {
    const independent = session({
      displayName: "Loose note",
      linkedProjectPaths: [],
      linkedRegisteredProjectIds: [],
      association: "loose_session"
    });
    const groups = {
      projectGroups: [{ project, sessions: [session({ displayName: "Project pass" })] }],
      independent: [independent],
      hermes: [session({ displayName: "Hermes run", sessionKind: "hermes" })]
    };

    const independentOnly = displayedSidebarSessionGroups(groups, {
      sort: "recent",
      appFilter: "all",
      scope: "independent"
    });
    const projectsOnly = displayedSidebarSessionGroups(groups, {
      sort: "recent",
      appFilter: "all",
      scope: "projects"
    });

    expect(independentOnly.count).toBe(1);
    expect(independentOnly.independent).toEqual([independent]);
    expect(independentOnly.projectGroups).toEqual([]);
    expect(independentOnly.hermes).toEqual([]);
    expect(projectsOnly.count).toBe(1);
    expect(projectsOnly.projectGroups).toHaveLength(1);
    expect(projectsOnly.independent).toEqual([]);
    expect(projectsOnly.hermes).toEqual([]);
  });

  it("compacts session groups without changing the matching session count", () => {
    const alpha = { ...project, id: 1, name: "Alpha" };
    const beta = { ...project, id: 2, name: "Beta" };
    const gamma = { ...project, id: 3, name: "Gamma" };
    const displayed = displayedSidebarSessionGroups({
      projectGroups: [
        { project: alpha, sessions: [session({ displayName: "Alpha pass", modifiedMs: 3000 })] },
        { project: beta, sessions: [session({ displayName: "Beta pass", modifiedMs: 2000 })] },
        { project: gamma, sessions: [session({ displayName: "Gamma pass", modifiedMs: 1000 })] }
      ],
      independent: [session({
        displayName: "Loose pass",
        linkedProjectPaths: [],
        linkedRegisteredProjectIds: [],
        association: "loose_session",
        modifiedMs: 500
      })],
      hermes: []
    }, {
      sort: "name",
      appFilter: "all",
      query: ""
    });

    const compact = compactSidebarSessionGroups(displayed, 2);

    expect(compact.compacted).toBe(true);
    expect(compact.hiddenGroupCount).toBe(1);
    expect(compact.projectGroups.map((group) => group.project.name)).toEqual(["Alpha", "Beta"]);
    expect(compact.independent.map((item) => item.displayName)).toEqual(["Loose pass"]);
    expect(compact.count).toBe(displayed.count);
  });

  it("never hides Independent behind a long project-group list", () => {
    const projectGroups = Array.from({ length: 50 }, (_, index) => ({
      project: { ...project, id: index + 1, name: `Project ${index + 1}` },
      sessions: [session({ displayName: `Project pass ${index + 1}` })]
    }));
    const loose = session({
      displayName: "Always visible",
      linkedProjectPaths: [],
      linkedRegisteredProjectIds: [],
      association: "loose_session"
    });
    const displayed = displayedSidebarSessionGroups({ projectGroups, independent: [loose], hermes: [] }, {
      sort: "name",
      appFilter: "all"
    });

    const compact = compactSidebarSessionGroups(displayed, 4);

    expect(compact.projectGroups).toHaveLength(4);
    expect(compact.hiddenGroupCount).toBe(46);
    expect(compact.independent).toEqual([loose]);
  });

  it("keeps search-opened groups shorter than manually expanded session groups", () => {
    const sessions = Array.from({ length: SIDEBAR_SESSION_GROUP_ITEM_LIMIT + 3 }, (_, index) =>
      session({ displayName: `Session ${index + 1}`, path: `C:\\sessions\\${index + 1}.jsonl` })
    );

    const manualPreview = previewSidebarSessionItems(sessions);
    const searchPreview = previewSidebarSessionItems(sessions, { searchActive: true });
    const independentPreview = previewSidebarSessionItems(sessions, {
      itemLimit: SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT
    });
    const allPreview = previewSidebarSessionItems(sessions, { searchActive: true, showAll: true });

    expect(manualPreview.visibleSessions).toHaveLength(SIDEBAR_SESSION_GROUP_ITEM_LIMIT);
    expect(manualPreview.hiddenCount).toBe(3);
    expect(searchPreview.visibleSessions).toHaveLength(SIDEBAR_SESSION_SEARCH_ITEM_LIMIT);
    expect(searchPreview.hiddenCount).toBe(sessions.length - SIDEBAR_SESSION_SEARCH_ITEM_LIMIT);
    expect(searchPreview.canToggle).toBe(true);
    expect(independentPreview.visibleSessions).toHaveLength(SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT);
    expect(independentPreview.hiddenCount).toBe(sessions.length - SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT);
    expect(allPreview.visibleSessions).toHaveLength(sessions.length);
    expect(allPreview.canToggle).toBe(true);
  });
});
