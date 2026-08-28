// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type {
  SafeManageAnalysisRun,
  SafeManageDecision,
  SafeManageFirstRunPreference,
  SafeManageOverview,
  SafeManageProjectAssessment
} from "../types";
import { applySafeManageFirstRunChoice, type SafeManageFirstRunChoice } from "../safeManageFirstRun";
import {
  safeManageDisplayRun,
  safeManageIncompleteRunIsReviewOnly,
  SafeManageComparisonEvidence,
  safeManageBoundedEvidenceCount,
  safeManageEvidenceCoverageDescription,
  safeManageFilteredAssessments,
  safeManageHomogeneousGroupKey,
  safeManageLatestDecisions,
  safeManagePositiveFileKindCounts,
  safeManageRunAllowsDecisions
} from "../views/SafeManagePortfolioView";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../api.ts", import.meta.url), "utf8");
const portfolioSource = readFileSync(new URL("../views/SafeManagePortfolioView.tsx", import.meta.url), "utf8");

function assessment(overrides: Partial<SafeManageProjectAssessment> = {}): SafeManageProjectAssessment {
  return {
    analysisRunId: "run-complete",
    projectId: 7,
    projectName: "CodeHangar",
    projectPath: "C:\\Synthetic\\CodeHangarDemo",
    lifecycle: "needs_review",
    recommendation: "review",
    confidence: "medium",
    reasonCode: "git_state_unknown",
    reason: "Git evidence is incomplete, so this project needs review.",
    rulesetVersion: "safe-manage-v1",
    evidenceRevision: "evidence-7",
    evidenceStale: false,
    lastActivityMs: null,
    apps: ["Claude"],
    sessionCount: null,
    hasGit: true,
    gitHasRemote: null,
    gitUncommitted: null,
    apparentBytes: null,
    physicalBytes: null,
    footprintPartial: true,
    fileKindProfile: {
      coverage: "partial",
      inspectedFileCount: 3,
      counts: [
        { kind: "documentation", label: "Documentation and context files", fileCount: 1 },
        { kind: "source", label: "Source files", fileCount: 2 }
      ]
    },
    duplicateEvidence: {
      coverage: "partial",
      inspectedFileCount: 3,
      possibleCopyFileCount: 0,
      indexedTextFileCount: 1,
      confirmedIndexedTextCopyCount: 0
    },
    materiallySimilarProjectCount: null,
    signals: [],
    importantFiles: [],
    riskRelations: [],
    ...overrides
  };
}

function run(id: string, state: SafeManageAnalysisRun["state"]): SafeManageAnalysisRun {
  return {
    id,
    state,
    rulesetVersion: "safe-manage-v1",
    catalogRevision: `catalog-${id}`,
    createdAt: "2026-08-27T20:00:00Z",
    startedAt: "2026-08-27T20:00:01Z",
    completedAt: state === "completed" ? "2026-08-27T20:00:02Z" : null,
    processedProjects: 1,
    totalProjects: 1,
    counts: {
      total: 1,
      active: 0,
      dormant: 0,
      archiveCandidates: 0,
      cleanupCandidates: 0,
      needsReview: 1
    },
    message: state,
    error: null,
    assessments: [assessment({ analysisRunId: id })]
  };
}

describe("Safe Manage portfolio evidence", () => {
  it("keeps the last complete portfolio visible when a newer run is partial", () => {
    const complete = run("complete", "completed");
    const partial = run("partial", "partial");
    const overview: SafeManageOverview = {
      latestRun: partial,
      lastCompleteRun: complete,
      decisions: [],
      firstRun: { suggestAfterDiscovery: true, promptState: "completed", lastPromptedAt: null }
    };
    expect(safeManageDisplayRun(overview)).toBe(complete);
    expect(safeManageRunAllowsDecisions(safeManageDisplayRun(overview))).toBe(true);
    expect(safeManageIncompleteRunIsReviewOnly(overview)).toBe(false);
  });

  it("keeps every first-ever incomplete result review-only before decision IPC", () => {
    for (const state of ["partial", "cancelled", "failed"] as const) {
      const incomplete = run(`first-${state}`, state);
      const overview: SafeManageOverview = {
        latestRun: incomplete,
        lastCompleteRun: null,
        decisions: [],
        firstRun: { suggestAfterDiscovery: true, promptState: "pending", lastPromptedAt: null }
      };
      expect(safeManageDisplayRun(overview)).toBe(incomplete);
      expect(safeManageRunAllowsDecisions(incomplete)).toBe(false);
      expect(safeManageIncompleteRunIsReviewOnly(overview)).toBe(true);
    }

    expect(safeManageRunAllowsDecisions(run("complete", "completed"))).toBe(true);
    const singleDecisionHandler = portfolioSource.slice(
      portfolioSource.indexOf("const recordDecision"),
      portfolioSource.indexOf("const displayRun")
    );
    expect(singleDecisionHandler.indexOf("safeManageRunAllowsDecisions"))
      .toBeGreaterThan(-1);
    expect(singleDecisionHandler.indexOf("safeManageRunAllowsDecisions"))
      .toBeLessThan(singleDecisionHandler.indexOf("api.safeManageDecisionRecord"));
    const groupedDecisionHandler = portfolioSource.slice(
      portfolioSource.indexOf("const recordGroupDecision"),
      portfolioSource.indexOf("return (", portfolioSource.indexOf("const recordGroupDecision"))
    );
    expect(groupedDecisionHandler).toContain("if (!decisionsAllowed");
    expect(groupedDecisionHandler.indexOf("if (!decisionsAllowed"))
      .toBeLessThan(groupedDecisionHandler.indexOf("api.safeManageDecisionsRecordAtomic"));
    expect(portfolioSource).toContain("Analyze again");
    expect(portfolioSource).toContain("selection, decisions and OperationPlan continuation require one complete baseline");
    expect(portfolioSource).toContain("const latestDecision = decisionsAllowed");
    expect(portfolioSource).toContain("decisionsAllowed && targetReview");
  });

  it("filters by recommendation, path, reason and associated local application", () => {
    const keep = assessment({ projectId: 8, projectName: "Active", recommendation: "keep", apps: ["Codex"] });
    const remove = assessment({ projectId: 9, projectName: "Old copy", projectPath: "D:\\Archives\\Old", recommendation: "removal_candidate", reason: "Old residual project." });
    expect(safeManageFilteredAssessments([keep, remove], "codex", "all")).toEqual([keep]);
    expect(safeManageFilteredAssessments([keep, remove], "old residual", "removal_candidate")).toEqual([remove]);
    expect(safeManageFilteredAssessments([keep, remove], "", "review")).toEqual([]);
  });

  it("retains only the latest explicit local-user decision per project", () => {
    const decisions: SafeManageDecision[] = [
      { id: 2, projectId: 7, analysisRunId: "complete", decision: "keep", evidenceRevision: "e2", decidedBy: "local_user", decidedAt: "2026-08-27T20:02:00Z", evidenceStale: false },
      { id: 1, projectId: 7, analysisRunId: "complete", decision: "ignore", evidenceRevision: "e1", decidedBy: "local_user", decidedAt: "2026-08-27T20:01:00Z", evidenceStale: true },
      { id: 1, projectId: 8, analysisRunId: "complete", decision: "ignore", evidenceRevision: "e1", decidedBy: "local_user", decidedAt: "2026-08-27T20:01:00Z", evidenceStale: false }
    ];
    expect(safeManageLatestDecisions(decisions).get(7)?.decision).toBe("keep");
    expect(safeManageLatestDecisions(decisions).get(8)?.decidedBy).toBe("local_user");
  });

  it("groups only evidence with the same run, recommendation, confidence and completeness", () => {
    const first = assessment();
    expect(safeManageHomogeneousGroupKey(assessment({ projectId: 8 })))
      .toBe(safeManageHomogeneousGroupKey(first));
    expect(safeManageHomogeneousGroupKey(assessment({ projectId: 9, confidence: "low" })))
      .not.toBe(safeManageHomogeneousGroupKey(first));
    expect(safeManageHomogeneousGroupKey(assessment({ projectId: 10, footprintPartial: false })))
      .not.toBe(safeManageHomogeneousGroupKey(first));
    expect(safeManageHomogeneousGroupKey(assessment({
      projectId: 11,
      fileKindProfile: { ...first.fileKindProfile, coverage: "complete" }
    }))).not.toBe(safeManageHomogeneousGroupKey(first));
    expect(safeManageHomogeneousGroupKey(assessment({
      projectId: 12,
      duplicateEvidence: { ...first.duplicateEvidence, coverage: "complete" }
    }))).not.toBe(safeManageHomogeneousGroupKey(first));
    expect(safeManageHomogeneousGroupKey(assessment({
      projectId: 13,
      materiallySimilarProjectCount: 0
    }))).not.toBe(safeManageHomogeneousGroupKey(first));
  });

  it("renders bounded comparison evidence without exposing paths or treating unavailable as zero", () => {
    const unavailable = assessment({
      projectPath: "C:\\private\\must-not-appear",
      fileKindProfile: {
        coverage: "unavailable",
        inspectedFileCount: 0,
        counts: []
      },
      duplicateEvidence: {
        coverage: "unavailable",
        inspectedFileCount: 0,
        possibleCopyFileCount: 0,
        indexedTextFileCount: 0,
        confirmedIndexedTextCopyCount: 0
      },
      materiallySimilarProjectCount: null
    });
    const html = renderToStaticMarkup(createElement(SafeManageComparisonEvidence, {
      assessment: unavailable
    }));

    expect(html).toContain("Bounded portfolio comparison");
    expect(html).toContain("Unavailable");
    expect(html).toContain("no missing category or duplicate count is treated as zero");
    expect(html).toContain("no zero is inferred");
    expect(html).toContain("<dd>Unknown</dd>");
    expect(html).not.toContain("<dd>0</dd>");
    expect(html).not.toContain(unavailable.projectPath);
    expect(html).not.toContain("0 materially similar projects");
  });

  it("shows positive file kinds and distinguishes metadata hints from indexed-text identity", () => {
    const withEvidence = assessment({
      fileKindProfile: {
        coverage: "partial",
        inspectedFileCount: 12,
        counts: [
          { kind: "documentation", label: "Documentation and context files", fileCount: 3 },
          { kind: "source", label: "Source files", fileCount: 8 },
          { kind: "other", label: "Other files", fileCount: 0 }
        ]
      },
      duplicateEvidence: {
        coverage: "partial",
        inspectedFileCount: 12,
        possibleCopyFileCount: 2,
        indexedTextFileCount: 3,
        confirmedIndexedTextCopyCount: 1
      },
      materiallySimilarProjectCount: 2
    });
    const html = renderToStaticMarkup(createElement(SafeManageComparisonEvidence, {
      assessment: withEvidence
    }));

    expect(safeManagePositiveFileKindCounts(withEvidence.fileKindProfile).map((item) => item.kind))
      .toEqual(["documentation", "source"]);
    expect(safeManageEvidenceCoverageDescription("partial", 12))
      .toContain("known lower bounds");
    expect(safeManageBoundedEvidenceCount(2, "partial")).toBe("At least 2");
    expect(safeManageBoundedEvidenceCount(0, "partial")).toBe("Unknown");
    expect(html).toContain("Partial");
    expect(html).toContain("Documentation and context files");
    expect(html).not.toContain("Other files");
    expect(html).toContain("Possible metadata-copy files");
    expect(html).toContain("Low-confidence, metadata-only hints");
    expect(html).toContain("Confirmed indexed-text duplicate files");
    expect(html).toContain("existing full local BLAKE3 hashes");
    expect(html).toContain('<p class="safe-manage-material-count"><strong>2</strong>');
    expect(html).toContain("materially similar projects");
  });
});

describe("Safe Manage first-run choice behavior", () => {
  it("only offers first-run analysis after a fresh settled real-project inventory", () => {
    const promptReference = appSource.indexOf("safeManagePromptShownRef.current");
    const promptStart = appSource.lastIndexOf("useEffect(() =>", promptReference);
    const promptEffect = appSource.slice(
      promptStart,
      appSource.indexOf("useEffect(() =>", promptReference + 1)
    );
    expect(promptEffect).toContain('sessionInventoryState !== "fresh"');
    expect(promptEffect).toContain("sessionInventoryRefreshing");
    expect(promptEffect).toContain("projectDiscoveryLoading");
    expect(promptEffect).toContain("realProjectCount === 0");
    expect(promptEffect).not.toContain("projectDiscoveryReport.totalCandidates === 0");
  });

  it("rejects a cached discovery snapshot dated in the future", () => {
    const cacheLoader = appSource.slice(
      appSource.indexOf("async function loadCachedDiscoveryReport"),
      appSource.indexOf("function normalizeProjectPath")
    );
    expect(cacheLoader).toContain("parsed.savedAt > Date.now()");
    expect(cacheLoader).toContain('typeof parsed.savedAt !== "number"');
  });

  it("first-run choices persist distinct behavior and only Analyze now starts work", async () => {
    const exercise = async (choice: SafeManageFirstRunChoice) => {
      let persisted: SafeManageFirstRunPreference = {
        suggestAfterDiscovery: true,
        promptState: "pending",
        lastPromptedAt: "2026-08-27T19:00:00Z"
      };
      const events: string[] = [];
      const outcome = await applySafeManageFirstRunChoice(choice, persisted, {
        savePreference: async (suggestAfterDiscovery, promptState, markPromptedNow) => {
          events.push(`save:${suggestAfterDiscovery}:${promptState}:${markPromptedNow}`);
          persisted = {
            suggestAfterDiscovery,
            promptState,
            lastPromptedAt: markPromptedNow ? "unexpected-new-time" : persisted.lastPromptedAt
          };
          return { ...persisted };
        },
        startAnalysis: async () => {
          events.push("start-analysis");
          return "durable-job-id";
        }
      });
      // This read models a remount/reload: the assertion observes the saved
      // backend value rather than relying only on the function's return value.
      const reloaded = { ...persisted };
      return { events, outcome, reloaded };
    };

    const analyze = await exercise("analyze_now");
    expect(analyze.events).toEqual([
      "save:true:postponed:false",
      "start-analysis"
    ]);
    expect(analyze.outcome.analysisJobId).toBe("durable-job-id");
    expect(analyze.reloaded).toMatchObject({
      suggestAfterDiscovery: true,
      promptState: "postponed",
      lastPromptedAt: "2026-08-27T19:00:00Z"
    });

    const later = await exercise("later");
    expect(later.events).toEqual(["save:true:postponed:false"]);
    expect(later.outcome.analysisJobId).toBeNull();
    expect(later.reloaded).toMatchObject({
      suggestAfterDiscovery: true,
      promptState: "postponed",
      lastPromptedAt: "2026-08-27T19:00:00Z"
    });

    const suppress = await exercise("suppress");
    expect(suppress.events).toEqual(["save:false:suppressed:false"]);
    expect(suppress.outcome.analysisJobId).toBeNull();
    expect(suppress.reloaded).toMatchObject({
      suggestAfterDiscovery: false,
      promptState: "suppressed",
      lastPromptedAt: "2026-08-27T19:00:00Z"
    });
  });

  it("keeps the postponed choice persisted when Analyze now cannot start", async () => {
    let persisted: SafeManageFirstRunPreference = {
      suggestAfterDiscovery: true,
      promptState: "pending",
      lastPromptedAt: "2026-08-27T19:00:00Z"
    };
    const events: string[] = [];
    await expect(applySafeManageFirstRunChoice("analyze_now", persisted, {
      savePreference: async (suggestAfterDiscovery, promptState) => {
        events.push("saved-before-start");
        persisted = { ...persisted, suggestAfterDiscovery, promptState };
        return { ...persisted };
      },
      startAnalysis: async () => {
        events.push("start-failed");
        throw new Error("synthetic start refusal");
      }
    })).rejects.toThrow("synthetic start refusal");
    expect(events).toEqual(["saved-before-start", "start-failed"]);
    expect(persisted.promptState).toBe("postponed");
    expect(persisted.suggestAfterDiscovery).toBe(true);
  });
});

describe("Safe Manage product entry", () => {
  it("is a global destination and does not require a selected project", () => {
    expect(appSource).toContain('onSafeManage={showSafeManage}');
    expect(appSource).toContain('primaryView: "safe_manage", rightPaneView: "plan"');
    expect(appSource).toContain("No project needs to be selected.");
  });

  it("keeps first-run analysis optional and permanent removal behind later review", () => {
    for (const copy of [
      "Analyze now",
      "Continue — do it later",
      "Do not suggest automatically again",
      "Prepare removal",
      "never authorize or execute a disk action"
    ]) {
      expect(portfolioSource).toContain(copy);
    }
    expect(portfolioSource).not.toContain("permanently remove now");
  });

  it("persists the first-run analysis job before mounting the polling view", () => {
    const handler = appSource.slice(
      appSource.indexOf("const analyzeSafeManageFirstRun"),
      appSource.indexOf("const focusProjectPicker")
    );
    expect(handler.indexOf('await applySafeManageFirstRunChoice("analyze_now"'))
      .toBeGreaterThan(-1);
    expect(handler.indexOf("showSafeManage()"))
      .toBeGreaterThan(handler.indexOf('await applySafeManageFirstRunChoice("analyze_now"'));
  });

  it("never converts a regenerable recommendation into a whole-project plan", () => {
    expect(appSource).toContain('preparedSafeManageDecision.decision === "clean_regenerables"');
    expect(appSource).toContain("Regenerable cleanup never targets the whole project");
    expect(appSource).toContain("Choose an exact build, cache or dependency folder");
    expect(portfolioSource).toContain("safeManageRegenerableTargets");
    expect(portfolioSource).toContain("safeManageRegenerableScanStart");
    expect(portfolioSource).toContain("Prepare this exact folder");
  });

  it("binds prepared actions to the recorded run, evidence revision, decision and exact target", () => {
    expect(appSource).toContain("api.safeManageOperationPlanStart");
    expect(appSource).toContain("analysisRunId: preparedSafeManageDecision.analysisRunId");
    expect(appSource).toContain("evidenceRevision: preparedSafeManageDecision.evidenceRevision");
    expect(appSource).toContain("decision: preparedSafeManageDecision.decision");
    expect(appSource).toContain("target: preparedSafeManageDecision.target ?? null");
    expect(appSource).toContain("The selected Safe Manage target changed");
    expect(apiSource).toContain('"safe_manage_operation_plan_start"');
  });

  it("records grouped decisions atomically and lists their exact members", () => {
    expect(portfolioSource).toContain("safeManageDecisionsRecordAtomic");
    expect(portfolioSource).toContain("Review the exact included projects");
    expect(portfolioSource).toContain("No disk action was prepared");
  });

  it("keeps the fixture contract on the objective-v2 comparison profile", () => {
    expect(apiSource).toContain('rulesetVersion: "safe-manage-objective-v2"');
    expect(apiSource).not.toContain('rulesetVersion: "safe-manage-objective-v1"');
    expect(apiSource).toContain("fileKindProfile:");
    expect(apiSource).toContain("duplicateEvidence:");
    expect(apiSource).toContain("materiallySimilarProjectCount:");
  });
});
