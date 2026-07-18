import { describe, expect, it } from "vitest";
import { projectAppMetas } from "../app-meta";
import { fixtureApi } from "../fixtures";

describe("fixture navigation", () => {
  it("shows one project tracked by ChatGPT and Claude without changing stable app ids", async () => {
    const projects = await fixtureApi.projectsList();
    const project = projects[0];
    expect(project.apps).toEqual(["codex", "claude"]);
    expect(projectAppMetas(project).map((meta) => meta.label)).toEqual(["ChatGPT", "Claude"]);

    const report = await fixtureApi.projectDiscoveryReport();
    const linked = report.sessions.filter((session) => session.linkedRegisteredProjectIds.includes(project.id));
    expect(linked.map((session) => session.sessionKind).sort()).toEqual(["ChatGPT", "Claude"]);
    expect(linked.map((session) => session.sourceKind).sort()).toEqual(["claude_code_sessions", "codex_sessions"]);
  });

  it("loads projects and context files", async () => {
    const projects = await fixtureApi.projectsList();
    const contexts = await fixtureApi.projectContextFiles(projects[0].id);

    expect(projects.length).toBeGreaterThan(0);
    expect(contexts.map((file) => file.displayName)).toContain("README.md");
    expect(contexts[0].displayName).toBe("README.md");
  });

  it("quick open finds context files", async () => {
    const results = await fixtureApi.quickOpen("agents");

    expect(results[0].label).toBe("AGENTS.md");
  });

  it("quick open combines file and project terms in any order", async () => {
    const projects = await fixtureApi.projectsList();
    const project = projects[0];

    for (const query of [`README ${project.name}`, `${project.name} README`]) {
      const results = await fixtureApi.quickOpen(query);
      expect(results.some((result) => result.projectId === project.id && result.path === "README.md")).toBe(true);
    }
  });

  it("pages tree children and exposes scan job root metadata", async () => {
    const projects = await fixtureApi.projectsList();
    const rootPage = await fixtureApi.projectNavChildren(projects[0].id, null, 20, 0);
    const docs = rootPage.items.find((item) => item.path === "docs");

    expect(rootPage.items.length).toBeGreaterThan(0);
    expect(docs?.itemKind).toBe("directory");

    const jobId = await fixtureApi.scanStart([42]);
    const status = await fixtureApi.scanStatus(jobId);

    expect(status.jobId).toBe(jobId);
    expect(status.rootIds).toEqual([]);
    expect(status.rootPaths).toEqual([]);
  });

  it("explains fixture folders without overstating ownership", async () => {
    const projects = await fixtureApi.projectsList();
    const rootPage = await fixtureApi.projectNavChildren(projects[0].id, null, 20, 0);
    const docs = rootPage.items.find((item) => item.path === "docs");

    expect(docs).toBeDefined();
    const explanation = await fixtureApi.folderExplanation(docs!.id);

    expect(explanation?.classification).toBe("documentation-context");
    expect(explanation?.confidence).toBe("medium");
    expect(explanation?.summary).toContain("fixture folder");
  });

  it("exposes local markdown relationships", async () => {
    const projects = await fixtureApi.projectsList();
    const readme = (await fixtureApi.quickOpen("README.md")).find((item) => item.projectId === projects[0].id && item.path === "README.md");
    const overview = (await fixtureApi.quickOpen("overview")).find((item) => item.projectId === projects[0].id && item.path === "docs/overview.md");

    expect(readme).toBeDefined();
    expect(overview).toBeDefined();

    const readmeRelationships = await fixtureApi.nodeRelationships(readme!.nodeId);
    const overviewRelationships = await fixtureApi.nodeRelationships(overview!.nodeId);

    expect(readmeRelationships.outgoing.some((relationship) => relationship.nodeId === overview!.nodeId)).toBe(true);
    expect(overviewRelationships.incoming.some((relationship) => relationship.nodeId === readme!.nodeId)).toBe(true);
    expect(readmeRelationships.issues.some((issue) => issue.kind === "unresolved_markdown_link" && issue.target === "missing-note.md")).toBe(true);
  });

  it("lists unreferenced asset candidates without cleanup actions", async () => {
    const orphans = await fixtureApi.graphOrphans();

    expect(orphans.candidates.some((candidate) => candidate.path === "assets/unused.png")).toBe(true);
    expect(orphans.candidates.some((candidate) => candidate.path === "docs/diagram.png")).toBe(false);
    expect(orphans.candidates.every((candidate) => candidate.reason.includes("Unreferenced"))).toBe(true);
  });

  it("supports explicit orphan search modes and filters", async () => {
    const assets = await fixtureApi.orphanAssetCandidates({ minSizeBytes: 0, assetKind: "image", minConfidence: "Medium" });
    const lost = await fixtureApi.lostProjectCandidates({ minSizeBytes: 0, stalePreset: "forgotten", signals: ["no_recent_opens"], keyword: "fixture" });
    const unused = assets.candidates.find((candidate) => candidate.path === "assets/unused.png");

    expect(assets.candidates.some((candidate) => candidate.path === "assets/unused.png")).toBe(true);
    expect(lost.candidates.length).toBeGreaterThan(0);
    expect(lost.candidates.every((candidate) => candidate.signals.includes("no_recent_opens"))).toBe(true);
    expect(lost.candidates.every((candidate) => candidate.signals.includes("keyword_match"))).toBe(true);

    expect(unused).toBeDefined();
    const status = await fixtureApi.nodeOrphanStatus(unused!.nodeId);
    expect(status.evaluated).toBe(true);
    expect(status.isCandidate).toBe(true);
  });

  it("lists duplicate candidates without cleanup actions", async () => {
    const duplicates = await fixtureApi.duplicateCandidates({ minSizeBytes: 1, fileKind: "data" });
    const group = duplicates.groups.find((candidate) => candidate.members.some((member) => member.path === "assets/copy-a.dat"));

    expect(group).toBeDefined();
    expect(group?.confidence).toBe("Medium");
    expect(group?.reason).toContain("Full hash confirmation is deferred");
    expect(group?.members.map((member) => member.path)).toEqual(["assets/copy-a.dat", "assets/copy-b.dat"]);

    const currentFile = group!.members.find((member) => member.path === "assets/copy-a.dat");
    const currentFileDuplicates = await fixtureApi.duplicateCandidates({ minSizeBytes: 1, fileKind: "data", currentFileNodeId: currentFile!.nodeId });
    expect(currentFileDuplicates.groups).toHaveLength(1);
    expect(currentFileDuplicates.groups[0].members.map((member) => member.path)).toEqual(["assets/copy-a.dat", "assets/copy-b.dat"]);
  });

  it("filters document search explicitly", async () => {
    const result = await fixtureApi.searchDocuments({ query: "code hangar", indexedKind: "context", pathFilter: "README", limit: 1, includeFixtureProjects: true });
    const unlimited = await fixtureApi.searchDocuments({ query: "code hangar", indexedKind: "all", limit: 0 });
    const hiddenFixtures = await fixtureApi.searchDocuments({ query: "code hangar", indexedKind: "all", limit: 10, includeFixtureProjects: false });

    expect(result.hits.length).toBeLessThanOrEqual(1);
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
    expect(unlimited.truncated).toBe(false);
    expect(hiddenFixtures.hits).toEqual([]);
    expect(hiddenFixtures.truncated).toBe(false);
  });

  it("keeps hidden fixture projects out of every Discover result type", async () => {
    const [orphans, lost, duplicates] = await Promise.all([
      fixtureApi.orphanAssetCandidates({ minSizeBytes: 0, includeFixtureProjects: false }),
      fixtureApi.lostProjectCandidates({ minSizeBytes: 0, includeFixtureProjects: false }),
      fixtureApi.duplicateCandidates({ minSizeBytes: 0, includeFixtureProjects: false })
    ]);

    expect(orphans.candidates).toEqual([]);
    expect(lost.candidates).toEqual([]);
    expect(duplicates.groups).toEqual([]);
  });

  it("blocks sensitive fixture preview", async () => {
    const results = await fixtureApi.quickOpen(".env");
    const preview = await fixtureApi.filePreview(results[0].nodeId, "source");

    expect(preview.state).toBe("blocked");
    expect(preview.source).toBeNull();
  });

  it("reveals non-strong sensitive fixture content only with temporary policy", async () => {
    const results = await fixtureApi.quickOpen(".env");
    const blocked = await fixtureApi.fileReveal(results[0].nodeId, "source");
    const revealed = await fixtureApi.fileReveal(results[0].nodeId, "source", { allowSensitiveReveal: true, relaxNonStrongProtectedPreview: false });

    expect(blocked.state).toBe("blocked");
    expect(revealed.state).toBe("ready");
    expect(revealed.wasRevealed).toBe(true);
    expect(revealed.source).toContain("SECRET");
  });

  it("returns passive git metadata for the git-like fixture", async () => {
    const git = await fixtureApi.projectGitStatus(3);

    expect(git.hasGit).toBe(true);
    expect(git.currentBranch).toBe("main");
    expect(git.originUrl).toContain("example.invalid");
  });

  it("summarizes fixture dashboard metrics", async () => {
    const dashboard = await fixtureApi.dashboardSummary();
    const hiddenDashboard = await fixtureApi.dashboardSummary(false);

    expect(dashboard.totalProjects).toBe(3);
    expect(dashboard.gitProjects).toBe(1);
    expect(dashboard.sensitiveFiles).toBeGreaterThan(0);
    expect(dashboard.adaptersNeedingReview).toBe(0);
    expect(dashboard.staleOrDirty).toContain("No live disk check");
    expect(hiddenDashboard).toMatchObject({
      totalProjects: 0,
      totalItems: 0,
      contextFiles: 0,
      indexedDocuments: 0,
      gitProjects: 0
    });
    expect(hiddenDashboard.largestProjects).toEqual([]);
  });

  it("exposes built-in adapters for pre-Phase 2 coverage", async () => {
    const adapters = await fixtureApi.adaptersList();

    expect(adapters.map((adapter) => adapter.name)).toContain("generic_markdown_context");
    expect(adapters.map((adapter) => adapter.name)).toContain("generic_git_project");
    expect(adapters.every((adapter) => adapter.enabled)).toBe(true);
  });
});
