import {
  AlertTriangle,
  Archive,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Clock3,
  FolderOpen,
  HardDrive,
  ListChecks,
  Loader2,
  PauseCircle,
  RefreshCw,
  Search,
  ShieldCheck,
  Sparkles,
  Trash2
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode
} from "react";
import { api } from "../api";
import type {
  SafeManageAnalysisRun,
  SafeManageDecision,
  SafeManageDecisionKind,
  SafeManageEvidenceCoverage,
  SafeManageFileKindProfile,
  SafeManageOverview,
  SafeManageProjectAssessment,
  SafeManageRecommendation,
  SafeManageRegenerableTarget
} from "../types";
import { displayLocalPath, formatOptionalBytes, formatTimestamp } from "../ui";
import "./safe-manage-portfolio.css";

type PortfolioFilter = "all" | SafeManageRecommendation;

const TERMINAL_ANALYSIS_STATES = new Set([
  "completed",
  "partial",
  "cancelled",
  "failed"
]);

const RECOMMENDATION_COPY: Record<SafeManageRecommendation, { label: string; tone: string }> = {
  keep: { label: "Keep", tone: "keep" },
  review: { label: "Review", tone: "review" },
  archive: { label: "Archive", tone: "archive" },
  clean_regenerables: { label: "Clean regenerables", tone: "cleanup" },
  removal_candidate: { label: "Removal candidate", tone: "removal" },
  do_not_touch: { label: "Do not touch", tone: "protected" }
};

const DECISION_COPY: Record<SafeManageDecisionKind, string> = {
  keep: "Kept",
  ignore: "Ignored",
  request_deeper_review: "Deeper review requested",
  archive: "Archive selected",
  clean_regenerables: "Regenerable cleanup selected",
  prepare_removal: "Removal review selected"
};

const EVIDENCE_COVERAGE_COPY: Record<SafeManageEvidenceCoverage, string> = {
  complete: "Complete",
  partial: "Partial",
  unavailable: "Unavailable"
};

export function safeManageEvidenceCoverageDescription(
  coverage: SafeManageEvidenceCoverage,
  inspectedFileCount: number
) {
  switch (coverage) {
    case "complete":
      return `Complete — ${inspectedFileCount} comparable file${inspectedFileCount === 1 ? "" : "s"} inspected in the declared local scope.`;
    case "partial":
      return `Partial — ${inspectedFileCount} comparable file${inspectedFileCount === 1 ? "" : "s"} inspected. Counts are known lower bounds; omitted evidence remains unknown.`;
    default:
      return "Unavailable — no missing category or duplicate count is treated as zero.";
  }
}

export function safeManagePositiveFileKindCounts(profile: SafeManageFileKindProfile) {
  return profile.counts.filter((item) => item.fileCount > 0);
}

export function safeManageBoundedEvidenceCount(
  value: number,
  coverage: SafeManageEvidenceCoverage
) {
  if (coverage === "complete") return String(value);
  return value > 0 ? `At least ${value}` : "Unknown";
}

export function safeManageDisplayRun(overview: SafeManageOverview | null) {
  if (!overview) return null;
  const latest = overview.latestRun;
  if (latest?.state === "completed") return latest;
  return overview.lastCompleteRun ?? latest;
}

export function safeManageRunAllowsDecisions(run: SafeManageAnalysisRun | null | undefined) {
  return run?.state === "completed";
}

export function safeManageIncompleteRunIsReviewOnly(overview: SafeManageOverview | null) {
  if (!overview || overview.lastCompleteRun) return false;
  const state = overview.latestRun?.state;
  return state === "partial" || state === "cancelled" || state === "failed";
}

export function safeManageLatestDecisions(decisions: SafeManageDecision[]) {
  const latest = new Map<number, SafeManageDecision>();
  for (const decision of decisions) {
    const current = latest.get(decision.projectId);
    const currentTime = current ? Date.parse(current.decidedAt) : Number.NEGATIVE_INFINITY;
    const candidateTime = Date.parse(decision.decidedAt);
    if (
      !current
      || candidateTime > currentTime
      || (candidateTime === currentTime && decision.id > current.id)
    ) {
      latest.set(decision.projectId, decision);
    }
  }
  return latest;
}

export function safeManageFilteredAssessments(
  assessments: SafeManageProjectAssessment[],
  query: string,
  filter: PortfolioFilter
) {
  const normalized = query.trim().toLocaleLowerCase();
  return assessments.filter((assessment) => {
    if (filter !== "all" && assessment.recommendation !== filter) return false;
    if (!normalized) return true;
    return [
      assessment.projectName,
      assessment.projectPath,
      assessment.reason,
      ...assessment.apps
    ].some((value) => value.toLocaleLowerCase().includes(normalized));
  });
}

export function safeManageHomogeneousGroupKey(assessment: SafeManageProjectAssessment) {
  return [
    assessment.analysisRunId,
    assessment.recommendation,
    assessment.confidence,
    assessment.reasonCode,
    assessment.evidenceStale ? "stale" : "current",
    assessment.footprintPartial ? "partial" : "complete",
    assessment.fileKindProfile.coverage,
    assessment.duplicateEvidence.coverage,
    assessment.materiallySimilarProjectCount == null ? "similarity-incomplete" : "similarity-complete"
  ].join("\u001f");
}

interface RegenerableReviewState {
  projectId: number;
  analysisRunId: string;
  evidenceRevision: string;
  targets: SafeManageRegenerableTarget[];
  loading: boolean;
  scanJobId: string | null;
  error: string | null;
}

export function SafeManagePortfolioView({
  active,
  extraRecommendation,
  onInspectProject,
  onPrepareDecision,
  onStatus
}: {
  active: boolean;
  extraRecommendation?: (assessment: SafeManageProjectAssessment) => ReactNode;
  onInspectProject: (projectId: number) => void;
  onPrepareDecision: (
    assessment: SafeManageProjectAssessment,
    decision: Exclude<SafeManageDecisionKind, "keep" | "ignore" | "request_deeper_review">,
    target?: SafeManageRegenerableTarget
  ) => void;
  onStatus: (message: string) => void;
}) {
  const [overview, setOverview] = useState<SafeManageOverview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<PortfolioFilter>("all");
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set());
  const [decisionBusy, setDecisionBusy] = useState<Set<number>>(() => new Set());
  const [selectedProjects, setSelectedProjects] = useState<Set<number>>(() => new Set());
  const [groupDecision, setGroupDecision] = useState<SafeManageDecisionKind>("keep");
  const [groupBusy, setGroupBusy] = useState(false);
  const [regenerableReview, setRegenerableReview] = useState<RegenerableReviewState | null>(null);

  const loadOverview = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await api.safeManageOverview();
      setOverview(next);
      const latest = next.latestRun;
      setActiveRunId(
        latest && !TERMINAL_ANALYSIS_STATES.has(latest.state) ? latest.id : null
      );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadOverview();
  }, [active, loadOverview]);

  useEffect(() => {
    if (!active || !activeRunId) return;
    let disposed = false;
    let timer: number | null = null;
    const poll = async () => {
      try {
        const run = await api.safeManageAnalysisStatus(activeRunId);
        if (disposed) return;
        setOverview((current) => current ? { ...current, latestRun: run } : current);
        if (TERMINAL_ANALYSIS_STATES.has(run.state)) {
          setActiveRunId(null);
          await loadOverview();
          onStatus(run.message);
          return;
        }
        timer = window.setTimeout(poll, document.hidden ? 1_500 : 500);
      } catch (reason) {
        if (disposed) return;
        setError(reason instanceof Error ? reason.message : String(reason));
        timer = window.setTimeout(poll, 2_000);
      }
    };
    void poll();
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [active, activeRunId, loadOverview, onStatus]);

  const beginAnalysis = useCallback(async () => {
    setError(null);
    try {
      const runId = await api.safeManageAnalysisStart();
      setActiveRunId(runId);
      onStatus("Safe Manage is analyzing the current local project catalog.");
      await loadOverview();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [loadOverview, onStatus]);

  const cancelAnalysis = useCallback(async () => {
    if (!activeRunId) return;
    setError(null);
    try {
      const run = await api.safeManageAnalysisCancel(activeRunId);
      setOverview((current) => current ? { ...current, latestRun: run } : current);
      onStatus(run.message);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [activeRunId, onStatus]);

  const openRegenerableTargets = useCallback(async (
    assessment: SafeManageProjectAssessment
  ) => {
    if (!safeManageRunAllowsDecisions(safeManageDisplayRun(overview))) {
      onStatus("Run a complete Safe Manage analysis before reviewing cleanup targets.");
      return;
    }
    const binding = {
      projectId: assessment.projectId,
      analysisRunId: assessment.analysisRunId,
      evidenceRevision: assessment.evidenceRevision
    };
    setExpanded((current) => new Set(current).add(assessment.projectId));
    setRegenerableReview({
      ...binding,
      targets: [],
      loading: true,
      scanJobId: null,
      error: null
    });
    try {
      const targets = await api.safeManageRegenerableTargets(
        binding.projectId,
        binding.analysisRunId,
        binding.evidenceRevision
      );
      setRegenerableReview((current) => current?.projectId === binding.projectId
        && current.analysisRunId === binding.analysisRunId
        && current.evidenceRevision === binding.evidenceRevision
        ? { ...current, targets, loading: false, error: null }
        : current);
    } catch (reason) {
      setRegenerableReview((current) => current?.projectId === binding.projectId
        && current.analysisRunId === binding.analysisRunId
        && current.evidenceRevision === binding.evidenceRevision
        ? {
            ...current,
            loading: false,
            error: reason instanceof Error ? reason.message : String(reason)
          }
        : current);
    }
  }, [onStatus, overview]);

  const startRegenerableScan = useCallback(async (target: SafeManageRegenerableTarget) => {
    setRegenerableReview((current) => current?.projectId === target.projectId
      ? { ...current, loading: true, error: null }
      : current);
    try {
      const scanJobId = await api.safeManageRegenerableScanStart({
        projectId: target.projectId,
        analysisRunId: target.analysisRunId,
        evidenceRevision: target.evidenceRevision,
        navId: target.navId,
        nodeId: target.nodeId,
        path: target.path
      });
      setRegenerableReview((current) => current?.projectId === target.projectId
        ? { ...current, loading: false, scanJobId, error: null }
        : current);
      onStatus(`Reading exact metadata inside ${target.path}. No file bodies or project files are changed.`);
    } catch (reason) {
      setRegenerableReview((current) => current?.projectId === target.projectId
        ? {
            ...current,
            loading: false,
            scanJobId: null,
            error: reason instanceof Error ? reason.message : String(reason)
          }
        : current);
    }
  }, [onStatus]);

  const cancelRegenerableScan = useCallback(async () => {
    const jobId = regenerableReview?.scanJobId;
    if (!jobId) return;
    try {
      await api.scanCancel(jobId);
      onStatus("Stopping the exact regenerable metadata scan after its current boundary.");
    } catch (reason) {
      setRegenerableReview((current) => current ? {
        ...current,
        error: reason instanceof Error ? reason.message : String(reason)
      } : current);
    }
  }, [onStatus, regenerableReview?.scanJobId]);

  useEffect(() => {
    if (!active || !regenerableReview?.scanJobId) return;
    const binding = {
      projectId: regenerableReview.projectId,
      analysisRunId: regenerableReview.analysisRunId,
      evidenceRevision: regenerableReview.evidenceRevision
    };
    const jobId = regenerableReview.scanJobId;
    let disposed = false;
    let timer: number | null = null;
    const poll = async () => {
      try {
        const status = await api.scanStatus(jobId);
        if (disposed) return;
        if (["completed", "partial", "cancelled", "failed"].includes(status.state)) {
          const targets = await api.safeManageRegenerableTargets(
            binding.projectId,
            binding.analysisRunId,
            binding.evidenceRevision
          );
          if (disposed) return;
          setRegenerableReview((current) => current?.scanJobId === jobId ? {
            ...current,
            targets,
            loading: false,
            scanJobId: null,
            error: status.state === "completed" ? null : (status.error ?? status.message)
          } : current);
          onStatus(status.state === "completed"
            ? "Exact regenerable inventory is complete. Review the concrete folder before preparing an OperationPlan."
            : `Regenerable inventory ended ${status.state}; partial evidence cannot enter an OperationPlan.`);
          return;
        }
        setRegenerableReview((current) => current?.scanJobId === jobId
          ? { ...current, loading: true }
          : current);
        timer = window.setTimeout(poll, document.hidden ? 1_500 : 500);
      } catch (reason) {
        if (disposed) return;
        setRegenerableReview((current) => current?.scanJobId === jobId ? {
          ...current,
          loading: false,
          scanJobId: null,
          error: reason instanceof Error ? reason.message : String(reason)
        } : current);
      }
    };
    void poll();
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [active, onStatus, regenerableReview?.analysisRunId, regenerableReview?.evidenceRevision, regenerableReview?.projectId, regenerableReview?.scanJobId]);

  const recordDecision = useCallback(async (
    assessment: SafeManageProjectAssessment,
    decision: SafeManageDecisionKind
  ) => {
    if (!safeManageRunAllowsDecisions(safeManageDisplayRun(overview))) {
      onStatus("This incomplete analysis is review-only. Analyze again before recording decisions.");
      return;
    }
    setDecisionBusy((current) => new Set(current).add(assessment.projectId));
    setError(null);
    try {
      const result = await api.safeManageDecisionRecord(
        assessment.projectId,
        assessment.analysisRunId,
        decision,
        assessment.evidenceRevision
      );
      setOverview((current) => current ? {
        ...current,
        decisions: [
          result,
          ...current.decisions.filter((item) => item.projectId !== result.projectId)
        ]
      } : current);
      onStatus(`${assessment.projectName}: ${DECISION_COPY[decision]}. No files were changed.`);
      if (decision === "clean_regenerables") {
        await openRegenerableTargets(assessment);
      } else if (decision === "archive" || decision === "prepare_removal") {
        onPrepareDecision(assessment, decision);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setDecisionBusy((current) => {
        const next = new Set(current);
        next.delete(assessment.projectId);
        return next;
      });
    }
  }, [onPrepareDecision, onStatus, openRegenerableTargets, overview]);

  const displayRun = safeManageDisplayRun(overview);
  const latestRun = overview?.latestRun ?? null;
  const decisionsAllowed = safeManageRunAllowsDecisions(displayRun);
  const incompleteRunIsReviewOnly = safeManageIncompleteRunIsReviewOnly(overview);
  const decisions = useMemo(
    () => safeManageLatestDecisions(overview?.decisions ?? []),
    [overview?.decisions]
  );
  const assessments = useMemo(
    () => safeManageFilteredAssessments(displayRun?.assessments ?? [], query, filter),
    [displayRun?.assessments, filter, query]
  );
  const selectedAssessments = useMemo(() => {
    if (!displayRun || !decisionsAllowed) return [];
    return displayRun.assessments.filter((assessment) => selectedProjects.has(assessment.projectId));
  }, [decisionsAllowed, displayRun, selectedProjects]);
  const selectedGroupKey = selectedAssessments.length > 0
    ? safeManageHomogeneousGroupKey(selectedAssessments[0])
    : null;
  const analysisActive = Boolean(activeRunId);
  const showingPriorComplete = Boolean(
    latestRun && latestRun.id !== displayRun?.id && latestRun.state !== "completed"
  );

  useEffect(() => {
    setSelectedProjects(new Set());
    setRegenerableReview(null);
  }, [decisionsAllowed, displayRun?.id]);

  const toggleGroupProject = useCallback((assessment: SafeManageProjectAssessment) => {
    if (!decisionsAllowed) return;
    setSelectedProjects((current) => {
      const next = new Set(current);
      if (next.has(assessment.projectId)) {
        next.delete(assessment.projectId);
        return next;
      }
      const firstId = next.values().next().value as number | undefined;
      const first = displayRun?.assessments.find((item) => item.projectId === firstId);
      if (
        assessment.recommendation === "do_not_touch"
        || assessment.evidenceStale
        || (first && safeManageHomogeneousGroupKey(first) !== safeManageHomogeneousGroupKey(assessment))
      ) {
        return current;
      }
      next.add(assessment.projectId);
      return next;
    });
  }, [decisionsAllowed, displayRun?.assessments]);

  const recordGroupDecision = useCallback(async () => {
    if (!decisionsAllowed || selectedAssessments.length === 0) return;
    setGroupBusy(true);
    setError(null);
    try {
      const results = await api.safeManageDecisionsRecordAtomic(
        selectedAssessments.map((assessment) => ({
          projectId: assessment.projectId,
          analysisRunId: assessment.analysisRunId,
          decision: groupDecision,
          evidenceRevision: assessment.evidenceRevision
        }))
      );
      const changed = new Set(results.map((item) => item.projectId));
      setOverview((current) => current ? {
        ...current,
        decisions: [
          ...results,
          ...current.decisions.filter((item) => !changed.has(item.projectId))
        ]
      } : current);
      if (results.length === 1) {
        const assessment = selectedAssessments[0];
        if (groupDecision === "clean_regenerables") {
          await openRegenerableTargets(assessment);
        } else if (groupDecision === "archive" || groupDecision === "prepare_removal") {
          onPrepareDecision(assessment, groupDecision);
        }
      }
      onStatus(
        `${results.length} exact project decision${results.length === 1 ? "" : "s"} recorded atomically. `
        + (results.length > 1 && ["archive", "clean_regenerables", "prepare_removal"].includes(groupDecision)
          ? "No disk action was prepared; continue through an exact OperationPlan for each project."
          : "No files were changed.")
      );
      setSelectedProjects(new Set());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setGroupBusy(false);
    }
  }, [decisionsAllowed, groupDecision, onPrepareDecision, onStatus, openRegenerableTargets, selectedAssessments]);

  return (
    <section className="pane-section compact safe-manage safe-manage-portfolio">
      <header className="safe-manage-header">
        <div className="safe-manage-heading">
          <span className="safe-manage-heading-icon"><ListChecks size={17} /></span>
          <div>
            <span>Assisted local decision</span>
            <h2>Discover says what exists. Safe Manage helps decide what to do with it.</h2>
          </div>
        </div>
        <p>
          Compare the whole portfolio using objective local evidence. Recommendations are a
          starting point for review; they never authorize or execute a disk action.
        </p>
        <div className="safe-manage-assurances" aria-label="Safe Manage assurances">
          <span><HardDrive size={13} /> Current local catalog</span>
          <span><ShieldCheck size={13} /> Unknown evidence stays unknown</span>
          <span><CheckCircle2 size={13} /> Analysis changes no files</span>
        </div>
      </header>

      <div className="safe-manage-portfolio-actions">
        <button
          type="button"
          className="action-button"
          disabled={analysisActive}
          onClick={() => void beginAnalysis()}
        >
          {analysisActive ? <Loader2 size={15} className="spin" /> : <RefreshCw size={15} />}
          {displayRun ? "Analyze current computer again" : "Analyze current computer"}
        </button>
        <button
          type="button"
          className="secondary-button"
          disabled={!analysisActive}
          onClick={() => void cancelAnalysis()}
        >
          <PauseCircle size={15} />
          {latestRun?.state === "cancelling" ? "Stopping…" : "Stop analysis"}
        </button>
        {displayRun ? (
          <small>
            Evidence snapshot {formatTimestamp(Date.parse(displayRun.completedAt ?? displayRun.createdAt))} · rules {displayRun.rulesetVersion}
          </small>
        ) : null}
      </div>

      {latestRun && !TERMINAL_ANALYSIS_STATES.has(latestRun.state) ? (
        <div className="safe-manage-analysis-progress" role="status">
          <div>
            <strong>{latestRun.message}</strong>
            <small>{latestRun.processedProjects} of {latestRun.totalProjects} projects</small>
          </div>
          <progress
            max={Math.max(1, latestRun.totalProjects)}
            value={latestRun.processedProjects}
          />
        </div>
      ) : null}

      {showingPriorComplete ? (
        <p className="safe-manage-notice">
          The newest run is {latestRun?.state}. The portfolio below is the last complete result;
          partial evidence has not replaced it.
        </p>
      ) : null}
      {incompleteRunIsReviewOnly ? (
        <section className="safe-manage-notice safe-manage-review-only" role="status">
          <div>
            <strong>This first result is review-only.</strong>
            <span>
              The analysis ended {latestRun?.state}. You can inspect the evidence already found,
              but selection, decisions and OperationPlan continuation require one complete baseline.
            </span>
          </div>
          <button
            type="button"
            className="action-button"
            disabled={analysisActive}
            onClick={() => void beginAnalysis()}
          >
            <RefreshCw size={14} /> Analyze again
          </button>
        </section>
      ) : null}
      {latestRun?.error ? <p className="scan-error">{latestRun.error}</p> : null}
      {error ? <p className="scan-error" role="alert">{error}</p> : null}

      {displayRun ? (
        <>
          <PortfolioCounts run={displayRun} />
          <div className="safe-manage-portfolio-toolbar">
            <label className="project-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="search"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Find a project or reason…"
                aria-label="Find a Safe Manage recommendation"
              />
            </label>
            <label>
              <span className="sr-only">Filter recommendations</span>
              <select value={filter} onChange={(event) => setFilter(event.target.value as PortfolioFilter)}>
                <option value="all">All recommendations</option>
                {Object.entries(RECOMMENDATION_COPY).map(([value, copy]) => (
                  <option key={value} value={value}>{copy.label}</option>
                ))}
              </select>
            </label>
            <small>{assessments.length} of {displayRun.assessments.length} shown</small>
          </div>

          {decisionsAllowed && selectedAssessments.length > 0 ? (
            <section className="safe-manage-group-review" aria-label="Exact grouped Safe Manage decision">
              <div>
                <strong>{selectedAssessments.length} homogeneous project{selectedAssessments.length === 1 ? "" : "s"} selected</strong>
                <small>
                  Same analysis, recommendation, confidence, reason and evidence completeness.
                  The decision is one database transaction; it does not create or execute a disk plan.
                </small>
              </div>
              <details>
                <summary>Review the exact included projects</summary>
                <ul>
                  {selectedAssessments.map((assessment) => (
                    <li key={assessment.projectId}>
                      <strong>{assessment.projectName}</strong>
                      <small>{displayLocalPath(assessment.projectPath)}</small>
                    </li>
                  ))}
                </ul>
              </details>
              <label>
                <span>Decision for every listed project</span>
                <select
                  value={groupDecision}
                  disabled={groupBusy}
                  onChange={(event) => setGroupDecision(event.target.value as SafeManageDecisionKind)}
                >
                  {Object.entries(DECISION_COPY).map(([value, label]) => (
                    <option key={value} value={value}>{label}</option>
                  ))}
                </select>
              </label>
              <div className="button-row">
                <button type="button" className="action-button" disabled={groupBusy} onClick={() => void recordGroupDecision()}>
                  {groupBusy ? <Loader2 size={14} className="spin" /> : <ListChecks size={14} />}
                  Record exact group decision
                </button>
                <button type="button" className="secondary-button" disabled={groupBusy} onClick={() => setSelectedProjects(new Set())}>
                  Clear selection
                </button>
              </div>
            </section>
          ) : null}

          {assessments.length === 0 ? (
            <div className="empty-state compact-empty">
              <Search size={22} />
              <p>No recommendation matches this filter.</p>
              <button type="button" className="secondary-button" onClick={() => { setQuery(""); setFilter("all"); }}>
                Clear filters
              </button>
            </div>
          ) : (
            <div className="safe-manage-project-list">
              {assessments.map((assessment) => {
                const open = expanded.has(assessment.projectId);
                const latestDecision = decisionsAllowed
                  ? decisions.get(assessment.projectId)
                  : undefined;
                const busy = decisionBusy.has(assessment.projectId);
                const continuedDecision = latestDecision?.decision === "archive"
                  || latestDecision?.decision === "prepare_removal"
                  ? latestDecision.decision
                  : null;
                const groupEligible = decisionsAllowed
                  && assessment.recommendation !== "do_not_touch"
                  && !assessment.evidenceStale
                  && (!selectedGroupKey || selectedGroupKey === safeManageHomogeneousGroupKey(assessment));
                const targetReview = regenerableReview?.projectId === assessment.projectId
                  ? regenerableReview
                  : null;
                const materialCoverage: SafeManageEvidenceCoverage = assessment.materiallySimilarProjectCount != null
                  ? "complete"
                  : assessment.fileKindProfile.coverage === "unavailable"
                    && assessment.duplicateEvidence.coverage === "unavailable"
                    ? "unavailable"
                    : "partial";
                return (
                  <article className="safe-manage-project-card" key={assessment.projectId}>
                    <div className="safe-manage-project-summary">
                      {decisionsAllowed ? (
                        <label
                          className="safe-manage-group-select"
                          title={groupEligible
                            ? "Include this project in one homogeneous, atomic decision."
                            : "Group decisions require current, homogeneous evidence and never include Do not touch projects."}
                        >
                          <input
                            type="checkbox"
                            checked={selectedProjects.has(assessment.projectId)}
                            disabled={!groupEligible || groupBusy}
                            onChange={() => toggleGroupProject(assessment)}
                            aria-label={`Select ${assessment.projectName} for a grouped decision`}
                          />
                        </label>
                      ) : <span className="safe-manage-group-select" aria-hidden="true" />}
                      <button
                        type="button"
                        className="safe-manage-project-expand"
                        aria-expanded={open}
                        onClick={() => setExpanded((current) => {
                          const next = new Set(current);
                          if (next.has(assessment.projectId)) next.delete(assessment.projectId);
                          else next.add(assessment.projectId);
                          return next;
                        })}
                      >
                        {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                        <span>
                          <strong>{assessment.projectName}</strong>
                          <small>{displayLocalPath(assessment.projectPath)}</small>
                        </span>
                      </button>
                      <span className={`safe-manage-recommendation ${RECOMMENDATION_COPY[assessment.recommendation].tone}`}>
                        {RECOMMENDATION_COPY[assessment.recommendation].label}
                      </span>
                      <span className="safe-manage-confidence">{assessment.confidence} confidence</span>
                    </div>

                    <p className="safe-manage-project-reason">{assessment.reason}</p>
                    <div className="safe-manage-project-facts">
                      <span><Clock3 size={13} /> {assessment.lastActivityMs == null ? "Activity unknown" : formatTimestamp(assessment.lastActivityMs)}</span>
                      <span><HardDrive size={13} /> {formatOptionalBytes(assessment.physicalBytes)}{assessment.footprintPartial ? "+" : ""}</span>
                      <span>{assessment.hasGit ? gitFact(assessment.gitUncommitted ?? null, assessment.gitHasRemote ?? null) : "No local Git metadata"}</span>
                      <span>{assessment.apps.length > 0 ? assessment.apps.join(", ") : "No app association"}</span>
                      <span>
                        File profile {assessment.fileKindProfile.coverage}: {assessment.fileKindProfile.inspectedFileCount} inspected
                      </span>
                      <span>
                        Copy evidence {assessment.duplicateEvidence.coverage}: metadata hints {safeManageBoundedEvidenceCount(
                          assessment.duplicateEvidence.possibleCopyFileCount,
                          assessment.duplicateEvidence.coverage
                        )}
                        {" · "}indexed text {safeManageBoundedEvidenceCount(
                          assessment.duplicateEvidence.confirmedIndexedTextCopyCount,
                          assessment.duplicateEvidence.coverage
                        )}
                      </span>
                      <span>
                        {assessment.materiallySimilarProjectCount == null
                          ? `Material similarity ${materialCoverage}`
                          : `${assessment.materiallySimilarProjectCount} materially similar project${assessment.materiallySimilarProjectCount === 1 ? "" : "s"}`}
                      </span>
                    </div>

                    {extraRecommendation?.(assessment)}

                    {latestDecision ? (
                      <p className={`safe-manage-decision-state ${latestDecision.evidenceStale ? "stale" : ""}`}>
                        {latestDecision.evidenceStale ? "Previous decision is stale" : DECISION_COPY[latestDecision.decision]}
                        {latestDecision.evidenceStale ? " — analyze again before preparing an action." : "."}
                      </p>
                    ) : null}

                    {latestDecision && !latestDecision.evidenceStale && latestDecision.decision === "clean_regenerables" ? (
                      <button type="button" className="secondary-button safe-manage-continue" disabled={busy} onClick={() => void openRegenerableTargets(assessment)}>
                        Review exact regenerable folders
                      </button>
                    ) : null}
                    {latestDecision && !latestDecision.evidenceStale && continuedDecision ? (
                      <button type="button" className="secondary-button safe-manage-continue" disabled={busy} onClick={() => onPrepareDecision(assessment, continuedDecision)}>
                        Continue to OperationPlan / Risk Report
                      </button>
                    ) : null}

                    <div className="safe-manage-decision-actions" aria-label={decisionsAllowed
                      ? `Decisions for ${assessment.projectName}`
                      : `Review actions for ${assessment.projectName}`}>
                      {decisionsAllowed ? (
                        <>
                          <button type="button" disabled={busy} onClick={() => void recordDecision(assessment, "keep")}><CheckCircle2 size={14} /> Keep</button>
                          <button type="button" disabled={busy} onClick={() => void recordDecision(assessment, "ignore")}>Ignore</button>
                          <button type="button" disabled={busy} onClick={() => {
                            setExpanded((current) => new Set(current).add(assessment.projectId));
                            void recordDecision(assessment, "request_deeper_review");
                          }}><Search size={14} /> Review more deeply</button>
                          <button type="button" disabled={busy || assessment.recommendation === "do_not_touch"} onClick={() => void recordDecision(assessment, "archive")}><Archive size={14} /> Archive</button>
                          <button type="button" disabled={busy || assessment.recommendation === "do_not_touch"} onClick={() => void recordDecision(assessment, "clean_regenerables")}><Sparkles size={14} /> Clean regenerables</button>
                          <button type="button" disabled={busy || assessment.recommendation === "do_not_touch"} onClick={() => void recordDecision(assessment, "prepare_removal")}><Trash2 size={14} /> Prepare removal</button>
                        </>
                      ) : null}
                      <button type="button" className="secondary-button" onClick={() => onInspectProject(assessment.projectId)}><FolderOpen size={14} /> Inspect project</button>
                    </div>

                    {open ? <ProjectEvidence assessment={assessment} /> : null}
                    {decisionsAllowed && targetReview ? (
                      <RegenerableTargetReview
                        state={targetReview}
                        assessment={assessment}
                        onScan={startRegenerableScan}
                        onCancelScan={cancelRegenerableScan}
                        onPrepare={(target) => onPrepareDecision(assessment, "clean_regenerables", target)}
                      />
                    ) : null}
                  </article>
                );
              })}
            </div>
          )}
        </>
      ) : loading ? (
        <p className="muted result-empty"><Loader2 size={15} className="spin" /> Loading previous Safe Manage results…</p>
      ) : (
        <div className="empty-state">
          <ListChecks size={28} />
          <h3>Start with a portfolio-level view</h3>
          <p>Code Hangar will compare every registered project using the current local catalog. The analysis is optional, cancelable and read-only.</p>
          <button type="button" className="action-button" onClick={() => void beginAnalysis()}>Analyze now</button>
        </div>
      )}
    </section>
  );
}

function PortfolioCounts({ run }: { run: SafeManageAnalysisRun }) {
  const items = [
    ["Active", run.counts.active],
    ["Dormant", run.counts.dormant],
    ["Archive candidates", run.counts.archiveCandidates],
    ["Cleanup candidates", run.counts.cleanupCandidates],
    ["Need review", run.counts.needsReview]
  ] as const;
  return (
    <section className="safe-manage-counts" aria-label="Portfolio classification">
      <div className="safe-manage-count-total">
        <strong>{run.counts.total}</strong>
        <span>projects analyzed</span>
      </div>
      {items.map(([label, count]) => (
        <div key={label}><strong>{count}</strong><span>{label}</span></div>
      ))}
    </section>
  );
}

function RegenerableTargetReview({
  state,
  assessment,
  onScan,
  onCancelScan,
  onPrepare
}: {
  state: RegenerableReviewState;
  assessment: SafeManageProjectAssessment;
  onScan: (target: SafeManageRegenerableTarget) => void;
  onCancelScan: () => void;
  onPrepare: (target: SafeManageRegenerableTarget) => void;
}) {
  return (
    <section className="safe-manage-regenerable-review" aria-label={`Exact regenerable targets for ${assessment.projectName}`}>
      <div className="safe-manage-regenerable-head">
        <div>
          <h4>Choose an exact regenerable folder</h4>
          <p>
            Normal discovery keeps dependency, cache and build containers opaque. Expand only one
            allowlisted folder to metadata before it can enter an OperationPlan. Project roots,
            shared nodes, links, Protected Zones and partial inventories stay ineligible.
          </p>
        </div>
        {state.scanJobId ? (
          <button type="button" className="secondary-button" onClick={onCancelScan}>
            <PauseCircle size={14} /> Stop exact scan
          </button>
        ) : null}
      </div>
      {state.error ? <p className="scan-error" role="alert">{state.error}</p> : null}
      {state.loading ? (
        <p className="muted"><Loader2 size={14} className="spin" /> Reading bounded metadata…</p>
      ) : null}
      {!state.loading && state.targets.length === 0 ? (
        <p className="safe-manage-notice">
          No exact allowlisted regenerable folder is currently eligible. Nothing can be prepared
          from the whole project or from incomplete evidence.
        </p>
      ) : null}
      {state.targets.length > 0 ? (
        <ul className="safe-manage-regenerable-list">
          {state.targets.map((target) => (
            <li key={`${target.navId}-${target.nodeId}`}>
              <div>
                <strong>{displayLocalPath(target.path)}</strong>
                <small>
                  {target.kind} · {formatOptionalBytes(target.bytes)} · {regenerableEvidenceLabel(target)}
                </small>
                {target.scanError ? <small className="scan-error">{target.scanError}</small> : null}
              </div>
              {target.operationPlanEligible ? (
                <button type="button" className="action-button" disabled={Boolean(state.scanJobId)} onClick={() => onPrepare(target)}>
                  <ListChecks size={14} /> Prepare this exact folder
                </button>
              ) : (
                <button type="button" className="secondary-button" disabled={Boolean(state.scanJobId)} onClick={() => onScan(target)}>
                  <Search size={14} /> {target.evidenceState === "expanded_partial" ? "Retry exact metadata scan" : "Scan exact metadata"}
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

function regenerableEvidenceLabel(target: SafeManageRegenerableTarget) {
  switch (target.evidenceState) {
    case "expanded_complete": return "exact inventory complete";
    case "expanded_partial": return "expanded evidence partial — blocked";
    case "opaque_measured": return "container measured, contents intentionally opaque";
    default: return "container evidence partial — blocked";
  }
}

function ProjectEvidence({ assessment }: { assessment: SafeManageProjectAssessment }) {
  return (
    <section className="safe-manage-evidence" aria-label={`Evidence for ${assessment.projectName}`}>
      <SafeManageComparisonEvidence assessment={assessment} />
      <div>
        <h4>Signals behind this recommendation</h4>
        <ul className="safe-manage-signal-list">
          {assessment.signals.map((signal) => (
            <li key={signal.code} className={`signal-${signal.state}`}>
              {signal.state === "unknown" ? <AlertTriangle size={14} /> : <CheckCircle2 size={14} />}
              <span><strong>{signal.label}</strong><small>{signal.detail} · {signal.source}</small></span>
            </li>
          ))}
        </ul>
      </div>
      <div>
        <h4>Potentially important files</h4>
        {assessment.importantFiles.length > 0 ? (
          <ul>
            {assessment.importantFiles.map((file) => (
              <li key={`${file.nodeId ?? "path"}-${file.path}`}>
                <strong>{file.displayName}</strong>
                <small>{displayLocalPath(file.path)} · {file.reason}</small>
              </li>
            ))}
          </ul>
        ) : <p className="muted">No important-file marker was available in this catalog snapshot.</p>}
      </div>
      <div>
        <h4>Risky relationships</h4>
        {assessment.riskRelations.length > 0 ? (
          <ul>
            {assessment.riskRelations.map((relation, index) => (
              <li key={`${relation.kind}-${index}`}>
                <strong>{relation.label}</strong>
                <small>{relation.confidence} confidence · projects {relation.relatedProjectIds.join(", ") || "unknown"}</small>
              </li>
            ))}
          </ul>
        ) : <p className="muted">No risky relationship was established. Unknown coverage remains visible in the signals above.</p>}
      </div>
    </section>
  );
}

export function SafeManageComparisonEvidence({
  assessment
}: {
  assessment: SafeManageProjectAssessment;
}) {
  const fileKinds = safeManagePositiveFileKindCounts(assessment.fileKindProfile);
  const duplicateEvidence = assessment.duplicateEvidence;
  const materiallySimilarProjectCount = assessment.materiallySimilarProjectCount ?? null;
  const materialCoverage: SafeManageEvidenceCoverage = materiallySimilarProjectCount != null
    ? "complete"
    : assessment.fileKindProfile.coverage === "unavailable"
      && duplicateEvidence.coverage === "unavailable"
      ? "unavailable"
      : "partial";
  const indexedTextScope = duplicateEvidence.indexedTextFileCount > 0
    ? `${duplicateEvidence.coverage === "complete" ? "" : "at least "}${duplicateEvidence.indexedTextFileCount} already-indexed safe-text file${duplicateEvidence.indexedTextFileCount === 1 ? "" : "s"}`
    : duplicateEvidence.coverage === "complete"
      ? "0 already-indexed safe-text files"
      : "the available already-indexed safe-text scope";

  return (
    <div className="safe-manage-comparison-evidence">
      <h4>Bounded portfolio comparison</h4>
      <p className="safe-manage-comparison-intro">
        Read-only local catalog evidence for deciding what deserves review. Counts never authorize
        an archive, cleanup or removal, and this summary exposes no file paths.
      </p>
      <div className="safe-manage-comparison-grid">
        <section aria-label="File-kind profile">
          <ComparisonHeading label="File-kind profile" coverage={assessment.fileKindProfile.coverage} />
          <small>
            {safeManageEvidenceCoverageDescription(
              assessment.fileKindProfile.coverage,
              assessment.fileKindProfile.inspectedFileCount
            )}
          </small>
          {fileKinds.length > 0 ? (
            <ul className="safe-manage-kind-counts">
              {fileKinds.map((item) => (
                <li key={item.kind}>
                  <strong>{item.fileCount}</strong>
                  <span>{item.label}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="muted">
              {assessment.fileKindProfile.coverage === "complete"
                ? "No positive file-kind count is present in this declared comparable scope."
                : "No positive file-kind count is available; this does not mean zero files."}
            </p>
          )}
        </section>

        <section aria-label="Copy and duplicate evidence">
          <ComparisonHeading label="Copy and duplicate evidence" coverage={duplicateEvidence.coverage} />
          <small>
            {safeManageEvidenceCoverageDescription(
              duplicateEvidence.coverage,
              duplicateEvidence.inspectedFileCount
            )}
          </small>
          <dl className="safe-manage-comparison-metrics">
            <div>
              <dt>Possible metadata-copy files</dt>
              <dd>{safeManageBoundedEvidenceCount(
                duplicateEvidence.possibleCopyFileCount,
                duplicateEvidence.coverage
              )}</dd>
              <small>
                Low-confidence, metadata-only hints based on matching catalog-relative name and
                size. They do not prove byte identity or project redundancy.
              </small>
            </div>
            <div>
              <dt>Confirmed indexed-text duplicate files</dt>
              <dd>{safeManageBoundedEvidenceCount(
                duplicateEvidence.confirmedIndexedTextCopyCount,
                duplicateEvidence.coverage
              )}</dd>
              <small>
                Byte-identical only within {indexedTextScope}, reusing existing full local BLAKE3
                hashes. Other file bodies were not opened or covered.
              </small>
            </div>
          </dl>
        </section>

        <section aria-label="Material similarity evidence">
          <ComparisonHeading label="Material similarity" coverage={materialCoverage} />
          {materiallySimilarProjectCount == null ? (
            <p className="muted">
              {materialCoverage === "unavailable"
                ? "Unavailable — the bounded portfolio comparison had no usable coverage; no zero is inferred."
                : "Partial — the bounded portfolio comparison could not establish a complete count; no zero is inferred."}
            </p>
          ) : (
            <>
              <p className="safe-manage-material-count">
                <strong>{materiallySimilarProjectCount}</strong>
                <span>materially similar project{materiallySimilarProjectCount === 1 ? "" : "s"}</span>
              </p>
              <small>
                Complete for this bounded comparison. This is a review signal from catalog
                structure and existing indexed evidence, not proof that a project is redundant.
              </small>
            </>
          )}
        </section>
      </div>
    </div>
  );
}

function ComparisonHeading({
  label,
  coverage
}: {
  label: string;
  coverage: SafeManageEvidenceCoverage;
}) {
  return (
    <div className="safe-manage-comparison-heading">
      <strong>{label}</strong>
      <span className={`safe-manage-coverage coverage-${coverage}`}>
        {EVIDENCE_COVERAGE_COPY[coverage]}
      </span>
    </div>
  );
}

function gitFact(dirty: boolean | null, hasRemote: boolean | null) {
  const worktree = dirty == null ? "Git state unknown" : dirty ? "Uncommitted Git work" : "Git worktree clean";
  const remote = hasRemote == null ? "remote unknown" : hasRemote ? "remote recorded" : "no remote recorded";
  return `${worktree} · ${remote}`;
}

export function SafeManageFirstRunPrompt({
  open,
  onAnalyzeNow,
  onLater,
  onSuppress
}: {
  open: boolean;
  onAnalyzeNow: () => void;
  onLater: () => void;
  onSuppress: () => void;
}) {
  if (!open) return null;
  return (
    <aside className="safe-manage-first-run" role="region" aria-labelledby="safe-manage-first-run-title">
      <span className="safe-manage-heading-icon"><ListChecks size={18} /></span>
      <div>
        <span>Initial discovery complete</span>
        <h2 id="safe-manage-first-run-title">I found everything. Would you like help deciding what still matters and what is only taking space and attention?</h2>
        <p>This optional read-only analysis compares all discovered projects. You can continue to Code Hangar now and run it later from Safe Manage.</p>
      </div>
      <div className="button-row">
        <button type="button" className="action-button" onClick={onAnalyzeNow}>Analyze now</button>
        <button type="button" className="secondary-button" onClick={onLater}>Continue — do it later</button>
        <button type="button" className="secondary-button" onClick={onSuppress}>Do not suggest automatically again</button>
      </div>
    </aside>
  );
}
