import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { documentSearchCriteriaKey, duplicateSearchCriteriaKey, orphanSearchCriteriaKey, retainRunningDuplicateConfirmations, scopeForDiscoveryEntry, scopeForDocumentSearchEntry } from "../documentSearch";
import { canRunCurrentProjectScope, canRunDocumentSearch, canSubmitDocumentSearch, currentProjectScopeLabel, defaultSessionGroupCollapsed, discoverSessionFacts, discoverSessionHelp, DiscoverDuplicatesView, documentHitPathLabel, documentHitSnippet, duplicateConfirmationGroupKey, duplicateGroupCountLabel, groupDuplicateGroupsForDisplay, groupLostCandidatesForDisplay, hiddenDiscoveryCandidateCount, orphanCandidateFacts, orphanCandidatePathLabel, orphanResultCountLabel, projectCandidateSignalChips, searchMinPresetForMode, type DuplicateConfirmStateMap } from "../views/DiscoverView";
import type { DuplicateConfirmation, DuplicateGroup, LostProjectCandidate, SessionDiscoveryCandidate } from "../types";

function session(overrides: Partial<SessionDiscoveryCandidate> = {}): SessionDiscoveryCandidate {
  return {
    path: "C:\\Users\\sample-user\\AppData\\Roaming\\Cursor\\User\\globalStorage\\state.vscdb#cursor-ide-chat=11111111-2222-4333-8444-555555555555",
    displayName: "Portal de pedidos clinicos",
    sourceKind: "cursor",
    sourceLabel: "Cursor in-IDE conversations",
    sessionKind: "Cursor",
    confidence: "high",
    linkedProjectPaths: ["C:\\Work\\Projects\\SampleFrontend"],
    linkedRegisteredProjectIds: [7],
    association: "registered_project",
    modifiedMs: 1710000000000,
    ...overrides
  };
}

describe("Discover session cards", () => {
  it("keeps long session groups collapsed until the user opens them", () => {
    expect(defaultSessionGroupCollapsed("Cursor", 19)).toBe(true);
    expect(defaultSessionGroupCollapsed("Antigravity/Gemini", 5)).toBe(false);
    expect(defaultSessionGroupCollapsed("Hermes", 1)).toBe(true);
  });

  it("summarizes linked project paths as compact labels", () => {
    expect(discoverSessionFacts(session())).toEqual(["Project: SampleFrontend"]);
    expect(discoverSessionFacts(session({
      linkedProjectPaths: [
        "C:\\Work\\Projects\\SampleFrontend",
        "C:\\Users\\sample-user\\sample-index",
        "D:\\AI2"
      ]
    }))).toEqual(["Projects: SampleFrontend, sample-index +1"]);
  });

  it("shows a compact no-project fact when no local project path is linked", () => {
    expect(discoverSessionFacts(session({ linkedProjectPaths: [], linkedRegisteredProjectIds: [], association: "loose_session" }))).toEqual(["No project path"]);
  });

  it("keeps full technical paths in the card help text", () => {
    const candidate = session();
    const help = discoverSessionHelp(candidate);

    expect(help).toContain(candidate.path);
    expect(help).toContain(candidate.linkedProjectPaths[0]);
    expect(help).toContain("Click to open it read-only");
  });
});

function duplicateGroup(id: number, paths: string[]): DuplicateGroup {
  return {
    id,
    sizeBytes: 128,
    hashPartial: `hash-${id}`,
    confidence: "medium",
    reason: "Same apparent size and first 64 KiB hash.",
    memberCount: paths.length,
    physicalBytes: paths.length * 128,
    footprintPartial: false,
    members: paths.map((path, index) => ({
      nodeId: id * 10 + index,
      projectId: 1,
      projectName: "Fixture",
      path,
      displayName: path.split(/[\\/]/).pop() ?? path,
      physicalBytes: 128,
      footprintPartial: false
    }))
  };
}

describe("Discover duplicate groups", () => {
  it("collapses groups that live entirely inside dependency caches", () => {
    const direct = duplicateGroup(1, ["src/model.bin", ".venv/Lib/site-packages/pkg/model.bin"]);
    const cache = duplicateGroup(2, [
      "C:\\AI\\App\\.venv\\Lib\\site-packages\\llvmlite\\binding\\llvmlite.dll",
      "C:\\AI\\Other\\.venv\\Lib\\site-packages\\llvmlite\\binding\\llvmlite.dll"
    ]);

    expect(groupDuplicateGroupsForDisplay([direct, cache])).toEqual({
      directGroups: [direct],
      cacheGroups: [cache]
    });
  });

  it("distinguishes returned groups from all matching groups", () => {
    const shown = duplicateGroup(1, ["src/a.bin", "src/b.bin"]);

    expect(duplicateGroupCountLabel({ groups: [shown], total: 43 }, true, false)).toBe("1 of 43 shown");
    expect(duplicateGroupCountLabel({ groups: [shown], total: 1 }, true, false)).toBe("1");
    expect(duplicateGroupCountLabel(null, true, true)).toBe("searching");
    expect(duplicateGroupCountLabel(null, false, false)).toBe("not run");
  });

  it("drops completed confirmations on a fresh search but keeps active jobs reachable", () => {
    expect(retainRunningDuplicateConfirmations({
      completed: { loading: false, result: "stale" },
      running: { loading: true, jobId: "job-1" }
    })).toEqual({
      running: { loading: true, jobId: "job-1" }
    });
  });

  it("renders a completed full-hash confirmation again after the view remounts", () => {
    const group = duplicateGroup(3, ["models/a.gguf", "models/b.gguf"]);
    const confirmation: DuplicateConfirmation = {
      targetNodeId: group.members[0].nodeId,
      confirmedGroups: [{
        fullHash: "full-hash",
        sizeBytes: group.sizeBytes,
        memberCount: group.memberCount,
        reclaimableBytes: group.sizeBytes,
        confidence: "high",
        members: group.members
      }],
      checkedFiles: group.memberCount,
      bytesHashed: group.sizeBytes * group.memberCount,
      reclaimableBytes: group.sizeBytes,
      partial: false
    };
    const confirmState: DuplicateConfirmStateMap = {
      [duplicateConfirmationGroupKey(group)]: { loading: false, result: confirmation }
    };
    const renderView = (state: DuplicateConfirmStateMap = confirmState) => renderToStaticMarkup(createElement(DiscoverDuplicatesView, {
      duplicateScope: "all",
      setDuplicateScope: () => undefined,
      preview: null,
      duplicateMinPreset: "10m",
      setDuplicateMinPreset: () => undefined,
      duplicateCustomMiB: 10,
      setDuplicateCustomMiB: () => undefined,
      duplicateFileKind: "all",
      setDuplicateFileKind: () => undefined,
      duplicateLimit: 25,
      setDuplicateLimit: () => undefined,
      duplicateLoading: false,
      duplicateSearchError: null,
      loadDuplicateCandidates: () => undefined,
      duplicateHasRun: true,
      duplicateCandidates: { groups: [group], total: 1 },
      advancedMode: true,
      openNode: () => undefined,
      showFileMenu: () => undefined,
      projects: [],
      selectedProjectId: null,
      confirmState: state,
      setConfirmState: () => undefined
    }));

    expect(renderView()).toContain("Checked");
    expect(renderView()).toContain("2 confirmed identical");
    expect(renderView({
      [duplicateConfirmationGroupKey(group)]: { loading: true, jobId: "job-1" }
    })).toContain("Finish or cancel comparison");
  });
});

describe("Discover project candidates", () => {
  it("deduplicates repeated signal labels before applying the chip limit", () => {
    const chips = projectCandidateSignalChips([
      { kind: "session_path", label: "AI session working directory", detail: "Codex session A", confidence: "high" },
      { kind: "session_path", label: "AI session working directory", detail: "Claude session B", confidence: "high" },
      { kind: "recent_activity", label: "Recent local activity", detail: "README.md", confidence: "medium" },
      { kind: "recent_activity", label: "Recent local activity", detail: "package.json", confidence: "medium" },
      { kind: "manifest", label: "Project manifest", detail: "package.json", confidence: "high" },
      { kind: "readme", label: "README project context", detail: "README.md", confidence: "high" }
    ], 3);

    expect(chips.visible.map((chip) => chip.label)).toEqual([
      "AI sessions (2)",
      "Recent activity (2)",
      "Project manifest"
    ]);
    expect(chips.visible[0].help).toContain("2 matching signals");
    expect(chips.visible[0].help).toContain("Codex session A");
    expect(chips.hiddenCount).toBe(1);
  });

  it("counts each hidden candidate once even when several filters apply", () => {
    expect(hiddenDiscoveryCandidateCount(54, 0)).toBe(54);
    expect(hiddenDiscoveryCandidateCount(3, 5)).toBe(0);
  });
});

describe("Discover document search hits", () => {
  it("opens on All projects when no current project exists", () => {
    expect(scopeForDocumentSearchEntry("current", null)).toBe("all");
    expect(scopeForDocumentSearchEntry("current", 7)).toBe("current");
    expect(scopeForDocumentSearchEntry("all", null)).toBe("all");
  });

  it("keeps every project-scoped Discover tool usable when no project is selected", () => {
    expect(scopeForDiscoveryEntry("current", null)).toBe("all");
    expect(scopeForDiscoveryEntry("current", 7)).toBe("current");
    expect(scopeForDiscoveryEntry("all", null)).toBe("all");
    expect(scopeForDiscoveryEntry("file", null)).toBe("file");
  });

  it("uses the indexed path as the distinguishing result label", () => {
    expect(documentHitPathLabel({ path: "data/README.md" })).toBe("data/README.md");
    expect(documentHitPathLabel({ path: "\\\\?\\C:\\AI\\Project\\README.md" })).toBe("C:\\AI\\Project\\README.md");
    expect(documentHitPathLabel({ title: "README.md", path: "README.md" }, "Sample3DProject")).toBe("Sample3DProject / Project root");
  });

  it("hides snippets that only repeat the title or path", () => {
    expect(documentHitSnippet({ title: "README.md", path: "README.md", snippet: "README.md" })).toBeNull();
    expect(documentHitSnippet({ title: "README.md", path: "docs/README.md", snippet: "docs/README.md" })).toBeNull();
    expect(documentHitSnippet({ title: "README.md", path: "docs/README.md", snippet: "Install steps and project notes" })).toBe("Install steps and project notes");
  });

  it("removes Markdown source markers from readable result snippets", () => {
    expect(documentHitSnippet({
      title: "PACKAGING.md",
      path: "docs/PACKAGING.md",
      snippet: "# Packaging & release\n\nShips as **two editions**."
    })).toBe("Packaging & release Ships as two editions.");
  });

  it("does not allow Current project search without a selected project", () => {
    expect(canRunDocumentSearch("current", null)).toBe(false);
    expect(canRunDocumentSearch("current", 7)).toBe(true);
    expect(canRunDocumentSearch("all", null)).toBe(true);
  });

  it("requires a useful query before submitting document search", () => {
    expect(canSubmitDocumentSearch("r", false, false)).toBe(false);
    expect(canSubmitDocumentSearch("README", true, false)).toBe(false);
    expect(canSubmitDocumentSearch("README", false, true)).toBe(false);
    expect(canSubmitDocumentSearch("README", false, false)).toBe(true);
  });

  it("labels current-project scopes with the selected project", () => {
    const projects = [
      { id: 1, name: "Sample3DProject" },
      { id: 7, name: "contosdencantarfable" }
    ];

    expect(currentProjectScopeLabel(projects, 7)).toBe("Current project: contosdencantarfable");
    expect(currentProjectScopeLabel(projects, null)).toBe("Current project (choose one)");
    expect(canRunCurrentProjectScope("current", null)).toBe(false);
    expect(canRunCurrentProjectScope("current", 7)).toBe(true);
    expect(canRunCurrentProjectScope("all", null)).toBe(true);
    expect(canRunCurrentProjectScope("file", null)).toBe(true);
  });
});

describe("Discover search criteria", () => {
  it("keeps exhaustive size presets in Advanced mode", () => {
    expect(searchMinPresetForMode("0", false, "100m")).toBe("100m");
    expect(searchMinPresetForMode("custom", false, "10m")).toBe("10m");
    expect(searchMinPresetForMode("0", true, "100m")).toBe("0");
  });

  it("keeps an all-project document search current when only the selected project changes", () => {
    const base = {
      query: " README ",
      scope: "all" as const,
      projectId: 1,
      indexedKind: "context",
      pathFilter: " Docs ",
      nameFilter: "",
      limit: 10,
      includeFixtureProjects: false
    };

    expect(documentSearchCriteriaKey(base)).toBe(documentSearchCriteriaKey({ ...base, projectId: 7 }));
    expect(documentSearchCriteriaKey(base)).not.toBe(documentSearchCriteriaKey({ ...base, limit: 0 }));
  });

  it("normalizes signal order but tracks the active orphan filters", () => {
    const base = {
      mode: "lost" as const,
      scope: "all" as const,
      projectId: null,
      minPreset: "100m",
      customMiB: 999,
      includePartial: false,
      stalePreset: "any",
      signals: ["no_context", "git_absent"],
      keyword: " Old ",
      assetKind: "all",
      minConfidence: "Low",
      includeFixtureProjects: false
    };

    expect(orphanSearchCriteriaKey(base)).toBe(orphanSearchCriteriaKey({
      ...base,
      customMiB: 123,
      signals: ["git_absent", "no_context"]
    }));
    expect(orphanSearchCriteriaKey(base)).not.toBe(orphanSearchCriteriaKey({ ...base, keyword: "archive" }));
  });

  it("tracks the file target only for file-scoped duplicate searches", () => {
    const base = {
      scope: "file" as const,
      projectId: 7,
      currentFileNodeId: 101,
      minPreset: "10m",
      customMiB: 10,
      fileKind: "all",
      limit: 25,
      includeFixtureProjects: false
    };

    expect(duplicateSearchCriteriaKey(base)).not.toBe(duplicateSearchCriteriaKey({ ...base, currentFileNodeId: 102 }));
    expect(duplicateSearchCriteriaKey({ ...base, scope: "all" })).toBe(duplicateSearchCriteriaKey({
      ...base,
      scope: "all",
      projectId: 99,
      currentFileNodeId: 999
    }));
  });
});

describe("Discover orphan candidate rows", () => {
  it("keeps same-name assets distinguishable by showing their path", () => {
    expect(orphanCandidatePathLabel("contosdencantarfable\\o-sopro-magico-da-cerejeira_preview.mp4")).toBe("contosdencantarfable\\o-sopro-magico-da-cerejeira_preview.mp4");
    expect(orphanCandidatePathLabel("models")).toBe("models");
    expect(orphanCandidatePathLabel(".")).toBe("Project root");
  });

  it("separates ownership, signal and footprint from the candidate reason", () => {
    expect(orphanCandidateFacts({
      projectName: "contosdencantarfable",
      confidence: "Low",
      physicalBytes: 480.4 * 1024 * 1024,
      footprintPartial: true
    })).toEqual(["Project: contosdencantarfable", "Weak local signal", "480.4 MiB+"]);

    expect(orphanCandidateFacts({
      candidateKind: "folder",
      confidence: "High",
      physicalBytes: null,
      footprintPartial: false
    })).toEqual(["folder", "Strong local signal", "Unknown"]);
  });

  it("distinguishes returned candidates from all matching candidates", () => {
    expect(orphanResultCountLabel({ candidates: [{}, {}], total: 81 }, false)).toBe("2 of 81 shown");
    expect(orphanResultCountLabel({ candidates: [{}], total: 1 }, false)).toBe("1");
    expect(orphanResultCountLabel(null, true)).toBe("searching");
    expect(orphanResultCountLabel(null, false)).toBe("not run");
  });

  it("keeps whole projects visible and groups nested folders by owner", () => {
    const candidate = (overrides: Partial<LostProjectCandidate>): LostProjectCandidate => ({
      projectId: 7,
      nodeId: 7,
      navId: null,
      candidateKind: "project",
      displayName: "SampleWorkstation",
      path: "C:\\Work\\Projects\\SampleWorkstation",
      confidence: "High",
      reason: "Passive project-review signal.",
      signals: ["no_recent_opens"],
      apparentBytes: 100,
      physicalBytes: 100,
      footprintPartial: false,
      ...overrides
    });
    const project = candidate({});
    const models = candidate({ nodeId: 70, navId: 700, candidateKind: "folder", displayName: "models", path: "models" });
    const outputs = candidate({ projectId: 9, nodeId: 90, navId: 900, candidateKind: "folder", displayName: "outputs", path: "outputs" });

    expect(groupLostCandidatesForDisplay([project, models, outputs], [
      { id: 7, name: "SampleWorkstation" },
      { id: 9, name: "Tanibella" }
    ])).toEqual({
      projectCandidates: [project],
      folderGroups: [
        { projectId: 7, projectName: "SampleWorkstation", candidates: [models] },
        { projectId: 9, projectName: "Tanibella", candidates: [outputs] }
      ]
    });
  });
});
