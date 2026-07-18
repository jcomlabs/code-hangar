import { useEffect, useMemo, useState, type Dispatch, type MouseEvent, type SetStateAction } from "react";
import { CheckCircle2, ChevronDown, ChevronRight, ListChecks, Loader2, PlusCircle, Search } from "lucide-react";
import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import { displayAppText } from "../app-meta";
import { markdownToPlainText } from "../markdown";
import { formatScanDuration } from "../scanProgress";
import type {
  DocumentHit,
  DuplicateCandidates,
  DuplicateConfirmation,
  DuplicateConfirmProgress,
  DuplicateGroup,
  DiscoverySignal,
  FilePreview,
  LostProjectCandidates,
  OrphanCandidates,
  ProjectSummary,
  ProjectDiscoveryCandidate,
  ProjectDiscoveryReport,
  SessionDiscoveryCandidate
} from "../types";
import { SectionTitle, displayLocalPath, formatBytes, formatOptionalBytes, formatTimestamp, plainConfidenceLabel, textMentionsDependencyCache } from "../ui";

interface LostPreset {
  name: string;
  stalePreset: string;
  signals: string[];
  keyword: string;
  minPreset: string;
  customMiB: number;
  includePartial: boolean;
}

type FileMenuTarget = { nodeId: number; projectId?: number | null; path: string; label: string; itemKind?: string };
type PlanTarget = { nodeId: number; label: string; kind: string };
type LostCandidate = LostProjectCandidates["candidates"][number];
type OrphanCandidate = OrphanCandidates["candidates"][number];

export interface DuplicateConfirmUiState {
  loading: boolean;
  jobId?: string;
  progress?: DuplicateConfirmProgress;
  result?: DuplicateConfirmation;
  error?: string;
}

export type DuplicateConfirmStateMap = Record<string, DuplicateConfirmUiState>;

export function searchMinPresetForMode(value: string, advancedMode: boolean, fallback: string) {
  return !advancedMode && (value === "custom" || value === "0") ? fallback : value;
}

export function hiddenDiscoveryCandidateCount(loadedCount: number, visibleCount: number) {
  return Math.max(0, loadedCount - visibleCount);
}

function useElapsedSeconds(active: boolean) {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!active) {
      setSeconds(0);
      return;
    }
    const startedAt = Date.now();
    const timer = window.setInterval(
      () => setSeconds(Math.max(1, Math.round((Date.now() - startedAt) / 1000))),
      1_000
    );
    return () => window.clearInterval(timer);
  }, [active]);
  return seconds;
}

const LOST_SIGNAL_OPTIONS = [
  { id: "no_recent_opens", label: "No recent opens", help: "No file in this project or folder has been opened through Code Hangar recently." },
  { id: "no_context", label: "No context files", help: "No README, AGENTS, docs, prompts or other priority context files were found here." },
  { id: "git_absent", label: "Git absent", help: "No local .git metadata was detected, so this may be a loose or exported folder." },
  { id: "name_markers", label: "Old/draft/test/archive name", help: "The name or path contains words commonly used for unfinished or archived work." },
  { id: "partial_inventory", label: "Incomplete inventory", help: "The scan stopped early in this subtree, so its footprint is only a minimum count until scanning finishes." },
  { id: "keyword_match", label: "Keyword match", help: "The project or folder path matches the keyword filter below." }
];

export function orphanCandidatePathLabel(path: string) {
  const cleaned = displayLocalPath(path).trim().replace(/[\\/]+$/, "");
  if (!cleaned || cleaned === ".") return "Project root";
  return cleaned;
}

export function orphanCandidateFacts(candidate: {
  candidateKind?: string | null;
  projectName?: string | null;
  confidence: string;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}) {
  const owner = candidate.projectName ? `Project: ${candidate.projectName}` : candidate.candidateKind || "Candidate";
  return [
    owner,
    plainConfidenceLabel(candidate.confidence, "local signal"),
    `${formatOptionalBytes(candidate.physicalBytes)}${candidate.footprintPartial ? "+" : ""}`
  ];
}

export function orphanResultCountLabel(
  result: { candidates: unknown[]; total: number } | null,
  loading: boolean
) {
  if (loading) return "searching";
  if (!result) return "not run";
  const shown = result.candidates.length;
  return result.total > shown ? `${shown} of ${result.total} shown` : String(result.total);
}

export function groupLostCandidatesForDisplay(
  candidates: LostCandidate[],
  projects: Array<Pick<ProjectSummary, "id" | "name">>
) {
  const projectNameById = new Map(projects.map((project) => [project.id, project.name]));
  const projectCandidates: LostCandidate[] = [];
  const folderGroups = new Map<number, { projectId: number; projectName: string; candidates: LostCandidate[] }>();
  for (const candidate of candidates) {
    if (candidate.candidateKind === "project") {
      projectCandidates.push(candidate);
      continue;
    }
    const group = folderGroups.get(candidate.projectId) ?? {
      projectId: candidate.projectId,
      projectName: projectNameById.get(candidate.projectId) ?? `Project ${candidate.projectId}`,
      candidates: []
    };
    group.candidates.push(candidate);
    folderGroups.set(candidate.projectId, group);
  }
  return { projectCandidates, folderGroups: [...folderGroups.values()] };
}

export function projectCandidateSignalChips(signals: DiscoverySignal[], limit = 8) {
  const groups = new Map<string, {
    label: string;
    count: number;
    details: string[];
    confidences: string[];
  }>();
  for (const signal of signals) {
    const label = signal.label.trim() || signal.kind.replaceAll("_", " ");
    const key = label.toLocaleLowerCase();
    const group = groups.get(key) ?? { label, count: 0, details: [], confidences: [] };
    group.count += 1;
    const detail = signal.detail?.trim();
    if (detail && !group.details.includes(detail)) group.details.push(detail);
    if (signal.confidence && !group.confidences.includes(signal.confidence)) group.confidences.push(signal.confidence);
    groups.set(key, group);
  }
  const chips = Array.from(groups.entries()).map(([key, group]) => {
    const compactLabel = compactProjectSignalLabel(group.label);
    const visibleDetails = group.details.slice(0, 3);
    const detailCopy = visibleDetails.length
      ? ` Details: ${visibleDetails.join("; ")}${group.details.length > visibleDetails.length ? `; +${group.details.length - visibleDetails.length} more` : ""}.`
      : "";
    const countCopy = group.count > 1 ? ` ${group.count} matching signals.` : "";
    const confidenceCopy = group.confidences.length ? ` Match strength: ${group.confidences.map((value) => plainConfidenceLabel(value, "signal")).join(", ")}.` : "";
    return {
      key,
      label: group.count > 1 ? `${compactLabel} (${group.count})` : compactLabel,
      help: `${group.label}.${countCopy}${detailCopy}${confidenceCopy}`.replace(/\s+/g, " ").trim()
    };
  });
  return {
    visible: chips.slice(0, limit),
    hiddenCount: Math.max(0, chips.length - limit)
  };
}

function compactProjectSignalLabel(label: string) {
  const normalized = label.toLocaleLowerCase();
  if (normalized === "ai session working directory") return "AI sessions";
  if (normalized === "recent local activity") return "Recent activity";
  if (normalized === "project markers found in a known local folder") return "Project markers";
  if (normalized === "referenced by local session metadata") return "Session references";
  if (normalized === "deliberately opened in a local ai app") return "Opened in AI app";
  if (normalized === "readme project context") return "README context";
  if (normalized === "claude project context") return "Claude context";
  return label;
}

export function DiscoverProjectDiscoveryView({
  loading,
  error,
  report,
  runProjectDiscovery,
  addCandidateAsRoot,
  addVisibleCandidatesAsRoots,
  onOpenSession
}: {
  loading: boolean;
  error: string | null;
  report: ProjectDiscoveryReport | null;
  runProjectDiscovery: (limit?: number, kind?: "projects" | "sessions", includeTechnicalCandidates?: boolean) => void;
  addCandidateAsRoot: (candidate: ProjectDiscoveryCandidate) => void;
  addVisibleCandidatesAsRoots: (candidates: ProjectDiscoveryCandidate[]) => void;
  onOpenSession: (session: SessionDiscoveryCandidate) => void;
}) {
  const [showTechnicalCandidates, setShowTechnicalCandidates] = useState(false);
  const [showLowerConfidence, setShowLowerConfidence] = useState(false);
  const [findMode, setFindMode] = useState<"projects" | "sessions">("projects");
  const [sessionQuery, setSessionQuery] = useState("");
  const [sessionGroupCollapsed, setSessionGroupCollapsed] = useState<Record<string, boolean>>({});
  const sessionLinkedPaths = useMemo(
    () => (report?.sessions ?? []).flatMap((session) => session.linkedProjectPaths),
    [report]
  );
  const visibleCandidates = useMemo(
    () =>
      report?.candidates.filter((candidate) =>
        shouldShowProjectCandidate(
          candidate,
          showTechnicalCandidates,
          showLowerConfidence,
          candidateHasLinkedSession(candidate, sessionLinkedPaths)
        )
      ) ?? [],
    [report, showTechnicalCandidates, showLowerConfidence, sessionLinkedPaths]
  );
  const hiddenTechnicalCount = useMemo(
    () => report?.candidates.filter((candidate) => isTechnicalDiscoveryCandidate(candidate)).length ?? 0,
    [report]
  );
  const hiddenLowConfidenceCount = useMemo(
    () =>
      report?.candidates.filter(
        (candidate) =>
          !candidate.alreadyRegistered &&
          candidate.overlapKind === "none" &&
          !isTechnicalDiscoveryCandidate(candidate) &&
          !candidateHasLinkedSession(candidate, sessionLinkedPaths)
      ).length ?? 0,
    [report, sessionLinkedPaths]
  );
  const visibleSessions = useMemo(() => {
    const query = sessionQuery.trim().toLocaleLowerCase();
    const sessions = report?.sessions ?? [];
    if (!query) return sessions;
    return sessions.filter((session) => [
      session.displayName,
      session.path,
      displayAppText(session.sourceLabel),
      displayAppText(session.sessionKind),
      session.association,
      ...session.linkedProjectPaths
    ].some((value) => value.toLocaleLowerCase().includes(query)));
  }, [report, sessionQuery]);
  const sessionGroups = useMemo(() => {
    const map = new Map<string, SessionDiscoveryCandidate[]>();
    for (const session of visibleSessions) {
      const key = displayAppText(session.sessionKind || "Other");
      const bucket = map.get(key);
      if (bucket) bucket.push(session);
      else map.set(key, [session]);
    }
    // Apps first (most sessions first), high-volume agents (Hermes/NemoClaw) last.
    return [...map.entries()].sort((a, b) => {
      const ah = isHermesSessionGroup(a[0]) ? 1 : 0;
      const bh = isHermesSessionGroup(b[0]) ? 1 : 0;
      if (ah !== bh) return ah - bh;
      return b[1].length - a[1].length;
    });
  }, [visibleSessions]);
  const allSessionGroupCount = useMemo(
    () => new Set((report?.sessions ?? []).map((session) => displayAppText(session.sessionKind || "Other"))).size,
    [report]
  );
  const isSessionGroupCollapsed = (key: string, count: number) =>
    key in sessionGroupCollapsed ? sessionGroupCollapsed[key] : defaultSessionGroupCollapsed(key, count);
  const toggleSessionGroup = (key: string, count: number) =>
    setSessionGroupCollapsed((current) => ({ ...current, [key]: !isSessionGroupCollapsed(key, count) }));
  const addableCandidates = visibleCandidates.filter((candidate) => !discoveryBlocksAdd(candidate));
  const availableSourceCount = report?.searchedLocations.filter((source) => source.exists).length ?? 0;
  const hiddenProjectCount = hiddenDiscoveryCandidateCount(report?.candidates.length ?? 0, visibleCandidates.length);

  return (
    <section className="pane-section">
      <div className="filter-panel discovery-run-panel" data-help="This discovery run is local-only. It checks shallow project markers and bounded app metadata; candidates are added only if you choose them.">
        <p className="discovery-section-head">{report ? "Refresh local inventory" : "Start local discovery"}</p>
        <div className="discovery-run-actions">
          <button className={`action-button ${findMode === "projects" ? "active" : ""}`} type="button" disabled={loading} onClick={() => { setFindMode("projects"); void runProjectDiscovery(500, "projects"); }} data-help="Run a fresh local scan for root project folders that an AI coding session has actually worked in. Shows only projects with at least one linked session.">
            {loading && findMode === "projects" ? "Finding local projects..." : "Find local projects"}
          </button>
          <button className={`action-button ${findMode === "sessions" ? "active" : ""}`} type="button" disabled={loading} onClick={() => { setFindMode("sessions"); void runProjectDiscovery(500, "sessions"); }} data-help="Run a fresh complete local session scan across ChatGPT, Claude, Cursor, Antigravity/Gemini, Hermes/NemoClaw, OpenClaw and similar tools. Includes project-linked and standalone sessions.">
            {loading && findMode === "sessions" ? "Finding local sessions..." : "Find local sessions"}
          </button>
        </div>
        {loading ? <DiscoveryProgress /> : null}
      </div>
      {error ? <p className="scan-error">Project discovery failed: {error}</p> : null}
      {report ? (
        <>
          <div className="discovery-summary-grid" aria-label="Discovery summary">
            {findMode === "projects" ? (
              <>
                <DiscoveryMetric
                  label="Visible roots"
                  value={`${visibleCandidates.length.toLocaleString()} shown`}
                  detail={hiddenProjectCount > 0 ? `${hiddenProjectCount.toLocaleString()} hidden by filters or overlap` : "Filtered to likely root projects"}
                />
                <DiscoveryMetric
                  label="Project hits"
                  value={`${report.totalCandidates.toLocaleString()} total`}
                  detail={report.candidates.length < report.totalCandidates ? `${report.candidates.length.toLocaleString()} loaded in this view` : "All loaded"}
                />
              </>
            ) : (
              <>
                <DiscoveryMetric
                  label="Sessions"
                  value={`${visibleSessions.length.toLocaleString()} showing`}
                  detail={report.sessions.length < report.totalSessions ? `${report.sessions.length.toLocaleString()} loaded · ${report.totalSessions.toLocaleString()} total` : `${report.totalSessions.toLocaleString()} total`}
                />
                <DiscoveryMetric
                  label="Session groups"
                  value={`${sessionGroups.length.toLocaleString()} showing`}
                  detail={`${allSessionGroupCount.toLocaleString()} app group${allSessionGroupCount === 1 ? "" : "s"} loaded`}
                />
              </>
            )}
            <DiscoveryMetric
              label="Sources"
              value={`${availableSourceCount.toLocaleString()} available`}
              detail={`${report.searchedLocations.length.toLocaleString()} known local locations checked`}
            />
            <DiscoveryMetric
              label="Run time"
              value={formatScanDuration(report.durationMs)}
              detail="Last local discovery pass"
            />
          </div>
          {findMode === "projects" ? (
          <>
          <div className="discovery-manage-card" data-help="Controls for the project results below: bulk-add, which candidates are shown, and which local locations were checked.">
            <p className="discovery-section-head">Review controls</p>
            <div className="discovery-bulk-actions">
              <button
                className="action-button"
                type="button"
                disabled={loading || addableCandidates.length === 0}
                onClick={() => void addVisibleCandidatesAsRoots(addableCandidates)}
                data-help={addableCandidates.length === 0 ? "No visible candidate can be added. Registered or overlapping roots are intentionally skipped." : `Add the ${addableCandidates.length} visible candidate${addableCandidates.length === 1 ? "" : "s"} that do not overlap existing scan roots.`}
              >
                <PlusCircle size={14} />
                Add all visible
              </button>
              <span className="muted">
                {addableCandidates.length > 0
                  ? `${addableCandidates.length.toLocaleString()} ready to add; overlaps stay skipped.`
                  : "No visible roots are ready to add."}
              </span>
            </div>
            <label className="toggle-row" data-help="By default only folders that a local AI session has worked in are shown. Turn this on to also include folders with project markers but no linked session yet.">
              <input
                type="checkbox"
                checked={showLowerConfidence}
                onChange={(event) => setShowLowerConfidence(event.target.checked)}
              />
              Include folders without a linked AI session{hiddenLowConfidenceCount > 0 && !showLowerConfidence ? ` (${hiddenLowConfidenceCount} hidden)` : ""}
            </label>
            <details className="advanced-filter-details discovery-technical-details">
              <summary data-help="Reveal lower-level skills, model/vendor folders and overlap rules without changing the discovery inputs.">Technical candidates</summary>
              <label className="toggle-row" data-help="Include lower-level technical candidates such as skills, model/vendor folders and nested tool outputs.">
                <input
                  type="checkbox"
                  checked={showTechnicalCandidates}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    setShowTechnicalCandidates(checked);
                    if (checked && hiddenTechnicalCount === 0) {
                      void runProjectDiscovery(500, "projects", true);
                    }
                  }}
                />
                Include skills and technical folders
              </label>
              <p className="muted filter-note" data-help="Overlap protects the inventory from duplicated roots. A candidate marked Overlap is inside an existing scan root, contains one, or crosses registered roots.">
                Registered roots and overlaps stay excluded from bulk add, so the inventory does not duplicate coverage.
              </p>
            </details>
            <details className="advanced-filter-details">
              <summary data-help="Show the local folders and metadata stores that were checked during this passive discovery run.">Sources checked ({availableSourceCount}/{report.searchedLocations.length} available)</summary>
              <div className="source-list">
                {report.searchedLocations.map((source) => (
                  <div className={`source-row ${source.exists ? "available" : "missing"}`} key={`${source.sourceKind}-${source.path}`} data-help={source.detail ?? `Checked ${displayAppText(source.sourceLabel)}.`}>
                    <strong>{displayAppText(source.sourceLabel)}</strong>
                    <span>{source.exists ? "available" : "not found"}</span>
                    <small>{displayLocalPath(source.path)}</small>
                  </div>
                ))}
              </div>
            </details>
          </div>
          <div className="discovery-results-head">
            <p className="discovery-section-head">Project candidates</p>
            <span className="relationship-count-pill">{visibleCandidates.length}</span>
          </div>
          <div className="orphan-list discovery-candidate-list">
            {visibleCandidates.length === 0 ? <p className="muted result-empty">No new root project folders are visible. Indexed and overlapping folders stay hidden; include unlinked folders, expand Technical candidates, or run Deep Scan on a different folder.</p> : null}
            {visibleCandidates.map((candidate) => {
              const blocksAdd = discoveryBlocksAdd(candidate);
              const overlapText = discoveryOverlapText(candidate);
              const signalChips = projectCandidateSignalChips(candidate.signals);
              return (
                <div className={`orphan-row discovery-candidate ${candidate.alreadyRegistered ? "registered" : ""}`} key={candidate.path} data-help={`${plainConfidenceLabel(candidate.confidence, "project match")}. Found from local folder and app clues. ${overlapText}`}>
                  <div className="orphan-row-main static-row">
                    <strong>{candidate.displayName}</strong>
                    <span>{displayLocalPath(candidate.path)}</span>
                    <small>
                      {formatDiscoveryKind(candidate.projectKind)} · {plainConfidenceLabel(candidate.confidence, "project match")} · {candidate.estimatedFiles?.toLocaleString() ?? "unknown"} items{candidate.estimatePartial ? "+" : ""} · {formatOptionalBytes(candidate.estimatedBytes)}
                    </small>
                    {candidate.overlapKind !== "none" ? <small>{overlapText}</small> : null}
                    {candidate.containsRegisteredRoots.length > 0 ? <small>Contains registered root: {candidate.containsRegisteredRoots.join("; ")}</small> : null}
                    <div className="signal-chip-list">
                      {signalChips.visible.map((signal) => (
                        <span className="signal-chip" key={`${candidate.path}-${signal.key}`} data-help={signal.help}>
                          {signal.label}
                        </span>
                      ))}
                      {signalChips.hiddenCount > 0 ? <span className="signal-chip">+{signalChips.hiddenCount} more</span> : null}
                    </div>
                  </div>
                  <button
                    className="orphan-row-plan"
                    type="button"
                    disabled={blocksAdd}
                    data-help={blocksAdd ? `${overlapText} Overlap means this path is already covered by, inside, or parent of an existing Code Hangar scan root; adding it would duplicate inventory.` : "Add this candidate as a scan root. This creates local inventory metadata only; it does not modify project files."}
                    onClick={() => void addCandidateAsRoot(candidate)}
                  >
                    <PlusCircle size={14} />
                    {candidate.alreadyRegistered ? "Added" : blocksAdd ? "Overlap" : "Add to Projects"}
                  </button>
                </div>
              );
            })}
          </div>
          </>
          ) : null}
          {findMode === "sessions" ? (
          <div className="discovery-session-section">
            <SectionTitle icon={<Search size={15} />} label="Local Sessions" trailing={<ConceptHelp concept="sessions" />} />
            <p className="muted help-copy">
              Sessions are local conversation/task records. Linked sessions point at a registered project; loose sessions have no registered project match yet.
            </p>
            <div className="discovery-manage-card" data-help="Filter the session results below. This searches the discovery results already loaded in this view.">
              <p className="discovery-section-head">Filter sessions</p>
              <label className="field-label" data-help="Filter local sessions by app, path, linked project path or association. This searches the discovery results already loaded in this view.">
                Search sessions
                <input
                  type="search"
                  value={sessionQuery}
                  onChange={(event) => setSessionQuery(event.target.value)}
                  placeholder="ChatGPT, Claude, project path..."
                />
              </label>
            </div>
            <div className="discovery-results-head">
              <p className="discovery-section-head">Sessions by app</p>
              <span className="relationship-count-pill">{visibleSessions.length}</span>
            </div>
            <div className="orphan-list discovery-candidate-list">
              {visibleSessions.length === 0 ? (
                <div className="empty-action-panel" data-help={sessionQuery.trim() ? "The loaded session inventory has no match for this filter. Clear it to restore all groups." : "The complete local session scan returned no sessions."}>
                  <strong>{sessionQuery.trim() ? "No sessions match this search." : "No local sessions found."}</strong>
                  {sessionQuery.trim() ? (
                    <button className="action-button" type="button" onClick={() => setSessionQuery("")}>Clear search</button>
                  ) : (
                    <button className="action-button" type="button" disabled={loading} onClick={() => void runProjectDiscovery(500, "sessions")}>Find local sessions</button>
                  )}
                </div>
              ) : null}
              {sessionGroups.map(([groupKey, groupSessions]) => {
                const collapsed = isSessionGroupCollapsed(groupKey, groupSessions.length);
                return (
                  <div className="session-group" key={groupKey}>
                    <button
                      type="button"
                      className="session-group-header"
                      aria-expanded={!collapsed}
                      onClick={() => toggleSessionGroup(groupKey, groupSessions.length)}
                      data-help={`${groupSessions.length} ${groupKey} session${groupSessions.length === 1 ? "" : "s"}. Click to ${collapsed ? "expand" : "collapse"} this group.${defaultSessionGroupCollapsed(groupKey, groupSessions.length) ? " Longer or high-volume group; collapsed by default to keep the list readable." : ""}`}
                    >
                      {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                      <span>{groupKey}</span>
                      <small>{groupSessions.length}</small>
                    </button>
                    {!collapsed
                      ? groupSessions.map((session) => {
                          const sessionFacts = discoverSessionFacts(session);
                          return (
                           <button
                             type="button"
                             className="orphan-row discovery-candidate session-candidate session-candidate-button"
                             key={`${session.sourceKind}-${session.path}`}
                             onClick={() => onOpenSession(session)}
                             data-help={discoverSessionHelp(session)}
                           >
                             <div className="orphan-row-main static-row">
                               <div className="session-card-title">
                                 <strong>{session.displayName}</strong>
                                 <span>{displayAppText(session.sourceLabel)}</span>
                              </div>
                               <div className="session-card-meta">
                                 <span>{displayAppText(session.sessionKind)}</span>
                                 <span>{plainConfidenceLabel(session.confidence, "project link")}</span>
                                 {session.modifiedMs != null ? <span>{formatTimestamp(session.modifiedMs)}</span> : null}
                               </div>
                               <div className="session-card-facts" aria-label="Session links">
                                 {sessionFacts.map((fact) => <small key={fact}>{fact}</small>)}
                               </div>
                             </div>
                           </button>
                          );
                        })
                       : null}
                  </div>
                );
              })}
            </div>
          </div>
          ) : null}
        </>
      ) : (
        <p className="muted result-empty">Run discovery when you want Code Hangar to look for project folders or local sessions it can map to projects.</p>
      )}
    </section>
  );
}

function DiscoveryMetric({ label, value, detail }: { label: string; value: string; detail: string }) {
  return (
    <div className="discovery-metric">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function DiscoveryProgress() {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const start = Date.now();
    const id = setInterval(() => setSeconds(Math.max(0, Math.round((Date.now() - start) / 1000))), 1_000);
    return () => clearInterval(id);
  }, []);
  return (
    <div
      className="discovery-progress"
      role="status"
      aria-live="polite"
      data-help="Discovery is scanning known local folders and bounded app/session metadata. This is local-only and read-only; it usually finishes within a few seconds."
    >
      <div className="discovery-progress-track" aria-hidden="true"><span /></div>
      <span className="discovery-progress-copy">Searching local folders and session metadata… {seconds}s</span>
    </div>
  );
}

function discoveryBlocksAdd(candidate: ProjectDiscoveryCandidate) {
  return candidate.alreadyRegistered
    || candidate.overlapKind === "inside_registered_root"
    || candidate.overlapKind === "contains_registered_root"
    || candidate.overlapKind === "mixed_overlap";
}

function shouldShowProjectCandidate(
  candidate: ProjectDiscoveryCandidate,
  showTechnicalCandidates: boolean,
  showLowerConfidence: boolean,
  hasLinkedSession: boolean
) {
  if (candidate.alreadyRegistered || candidate.overlapKind !== "none") return false;
  if (isTechnicalDiscoveryCandidate(candidate)) return showTechnicalCandidates;
  // Any folder a local AI session has actually worked in is a real project,
  // regardless of confidence score. The toggle also reveals session-less folders.
  if (showLowerConfidence) return true;
  return hasLinkedSession;
}

function isHermesSessionGroup(kind: string) {
  const lower = kind.toLocaleLowerCase();
  return lower.includes("hermes") || lower.includes("nemoclaw");
}

export function defaultSessionGroupCollapsed(kind: string, count: number) {
  return isHermesSessionGroup(kind) || count > 8;
}

function normalizeDiscoveryPath(path: string) {
  return path.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLocaleLowerCase();
}

function candidateHasLinkedSession(candidate: ProjectDiscoveryCandidate, linkedPaths: string[]) {
  // Trust the backend's own session-reference signal first: it was matched
  // against every local session during discovery, not just the loaded window,
  // and it canonicalises paths (OneDrive, WSL, case) better than a string match.
  if (candidate.signals.some((signal) => signal.kind === "session_path")) {
    return true;
  }
  const target = normalizeDiscoveryPath(candidate.path);
  if (!target) return false;
  return linkedPaths.some((raw) => {
    const linked = normalizeDiscoveryPath(raw);
    if (!linked) return false;
    return linked === target || linked.startsWith(`${target}/`) || target.startsWith(`${linked}/`);
  });
}

function isTechnicalDiscoveryCandidate(candidate: ProjectDiscoveryCandidate) {
  if (candidate.projectKind === "technical_candidate") return true;
  const segments = discoveryPathSegments(candidate.path);
  const last = segments[segments.length - 1] ?? "";
  const hiddenTerminalSegments = new Set([
    ".codex",
    ".claude",
    ".gemini",
    ".hermes",
    ".openclaw",
    ".nemoclaw",
    ".continue",
    ".roo",
    ".lmstudio",
    "pinokio"
  ]);
  if (hiddenTerminalSegments.has(last)) return true;
  const hiddenSegments = new Set([
    "sessions",
    "skills",
    "brain",
    "scratch",
    "custom_nodes",
    "models",
    "checkpoints",
    ".vendor",
    ".external",
    "resources",
    "local-agent-mode-sessions",
    "server-logs",
    "site-packages",
    "dist-packages",
    "node_modules",
    ".venv",
    "venv"
  ]);
  if (segments.some((segment) => hiddenSegments.has(segment))) return true;
  return hasDiscoverySegmentSequence(segments, [".gemini", "antigravity"])
    || hasDiscoverySegmentSequence(segments, ["appdata", "roaming"])
    || hasDiscoverySegmentSequence(segments, ["appdata", "local"]);
}

function discoveryPathSegments(path: string) {
  return path.toLocaleLowerCase().split(/[\\/]+/).filter(Boolean);
}

function hasDiscoverySegmentSequence(segments: string[], sequence: string[]) {
  if (sequence.length === 0 || segments.length < sequence.length) return false;
  for (let index = 0; index <= segments.length - sequence.length; index += 1) {
    if (sequence.every((part, offset) => segments[index + offset] === part)) return true;
  }
  return false;
}

function sessionAssociationHelp(association: string) {
  if (association === "registered_project") return "Code Hangar found a path in this session that belongs to a project already in Projects.";
  if (association === "unregistered_project_reference") return "This session mentions a local project folder that is not registered yet.";
  if (association === "loose_session") return "This session was found locally, but no project path could be linked from its metadata.";
  return "Local session metadata discovered on this machine.";
}

export function discoverSessionFacts(session: SessionDiscoveryCandidate): string[] {
  if (!session.linkedProjectPaths.length) return ["No project path"];
  const labels = session.linkedProjectPaths.map(compactLocalPathLabel);
  if (labels.length === 1) return [`Project: ${labels[0]}`];
  const visible = labels.slice(0, 2).join(", ");
  return [`Projects: ${visible}${labels.length > 2 ? ` +${labels.length - 2}` : ""}`];
}

export function discoverSessionHelp(session: SessionDiscoveryCandidate): string {
  const linkedPaths = session.linkedProjectPaths.length
    ? ` Linked paths: ${session.linkedProjectPaths.join("; ")}.`
    : " No linked project path found.";
  return `${displayAppText(session.sessionKind)} session from ${displayAppText(session.sourceLabel)}. ${sessionAssociationHelp(session.association)} Local metadata path: ${session.path}.${linkedPaths} Click to open it read-only in the workspace, with secrets masked and details on the right.`;
}

function compactLocalPathLabel(path: string) {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.at(-1) ?? path;
}

function formatDiscoveryKind(kind: string) {
  if (kind === "ai_assisted_project") return "AI-assisted project";
  if (kind === "pinokio_app") return "Pinokio app";
  if (kind === "code_project") return "Code project";
  if (kind === "technical_candidate") return "Technical candidate";
  if (kind === "documentation_project") return "Documentation project";
  return "Project candidate";
}

function discoveryOverlapText(candidate: ProjectDiscoveryCandidate) {
  if (candidate.alreadyRegistered) return "Already registered in Code Hangar.";
  if (candidate.overlapKind === "inside_registered_root") return `Inside registered root: ${candidate.nestedUnderRegistered ?? "known root"}.`;
  if (candidate.overlapKind === "contains_registered_root") return "Contains an existing registered root.";
  if (candidate.overlapKind === "mixed_overlap") return "Overlaps existing registered roots in more than one way.";
  return "No registered-root overlap detected.";
}

export function DiscoverSearchView({
  documentQuery,
  setDocumentQuery,
  documentScope,
  setDocumentScope,
  documentKind,
  setDocumentKind,
  documentPathFilter,
  setDocumentPathFilter,
  documentNameFilter,
  setDocumentNameFilter,
  documentLimit,
  setDocumentLimit,
  documentSearching,
  runDocumentSearch,
  documentSearchRan,
  documentHits,
  documentSearchTruncated,
  documentSearchDuration,
  documentSearchError,
  projects,
  openNode,
  showFileMenu,
  selectedProjectId,
  showReview
}: {
  documentQuery: string;
  setDocumentQuery: (value: string) => void;
  documentScope: "current" | "all";
  setDocumentScope: (value: "current" | "all") => void;
  documentKind: "context" | "markdown" | "all";
  setDocumentKind: (value: "context" | "markdown" | "all") => void;
  documentPathFilter: string;
  setDocumentPathFilter: (value: string) => void;
  documentNameFilter: string;
  setDocumentNameFilter: (value: string) => void;
  documentLimit: number;
  setDocumentLimit: (value: number) => void;
  documentSearching: boolean;
  runDocumentSearch: () => void;
  documentSearchRan: boolean;
  documentHits: DocumentHit[];
  documentSearchTruncated: boolean;
  documentSearchDuration: number | null;
  documentSearchError: string | null;
  projects: ProjectSummary[];
  openNode: (nodeId: number) => void;
  showFileMenu: (target: FileMenuTarget, event: MouseEvent<HTMLElement>) => void;
  selectedProjectId: number | null;
  showReview: () => void;
}) {
  const projectNameById = useMemo(() => new Map(projects.map((project) => [project.id, project.name])), [projects]);
  const currentProjectLabel = currentProjectScopeLabel(projects, selectedProjectId);
  const currentScopeNeedsProject = !canRunDocumentSearch(documentScope, selectedProjectId);
  const showDocumentResults = documentSearchRan && !currentScopeNeedsProject;
  const searchEnabled = canSubmitDocumentSearch(documentQuery, documentSearching, currentScopeNeedsProject);
  return (
    <section className="pane-section">
      <form className="filter-panel" aria-busy={documentSearching} data-help="Document Search runs only when you press Search. It searches indexed non-sensitive content." onSubmit={(event) => {
        event.preventDefault();
        if (searchEnabled) void runDocumentSearch();
      }}>
        <input autoFocus className="search-input" type="search" value={documentQuery} onChange={(event) => setDocumentQuery(event.target.value)} placeholder="Search indexed content" data-help="Search indexed document body text. Sensitive, protected and non-indexed file bodies are excluded." />
        <div className="filter-grid">
          <label data-help="Limit the search to the active project, or search every indexed project.">
            Where to search
            <select value={documentScope} onChange={(event) => setDocumentScope(event.target.value as "current" | "all")}>
              <option value="current">{currentProjectLabel}</option>
              <option value="all">All projects</option>
            </select>
          </label>
          <label data-help="Priority context means README, AGENTS, docs, prompts and project config files that explain the project.">
            Type
            <select value={documentKind} onChange={(event) => setDocumentKind(event.target.value as "context" | "markdown" | "all")}>
              <option value="context">Priority context</option>
              <option value="markdown">Markdown</option>
              <option value="all">All indexed</option>
            </select>
          </label>
          <label data-help="Filter results to paths containing this text, such as docs, prompts or package.">
            Path contains
            <input type="text" value={documentPathFilter} onChange={(event) => setDocumentPathFilter(event.target.value)} placeholder="optional path text" />
          </label>
          <label data-help="Filter results to file names or titles containing this text, such as README or AGENTS.">
            Name contains
            <input type="text" value={documentNameFilter} onChange={(event) => setDocumentNameFilter(event.target.value)} placeholder="optional name text" />
          </label>
          <label data-help="Cap the returned rows. Unlimited removes the UI cap and can take a long time on very common terms.">
            Limit
            <select value={documentLimit} onChange={(event) => setDocumentLimit(Number(event.target.value))}>
              <option value={10}>10</option>
              <option value={25}>25</option>
              <option value={50}>50</option>
              <option value={0}>Unlimited</option>
            </select>
          </label>
        </div>
        {documentKind === "context" ? <p className="muted filter-note">Priority context is the small set of files Code Hangar treats as project explanations: README, AGENTS, docs, prompts and selected config files.</p> : null}
        {currentScopeNeedsProject ? <p className="warning-inline">Choose a project in the sidebar before using Current project, or choose All projects under Where to search.</p> : null}
        {documentLimit === 0 ? <p className="warning-inline">Unlimited search can take a long time and may make the app unresponsive until it finishes. Prefer project, type, name and path filters first.</p> : null}
        <button className="action-button" type="submit" disabled={!searchEnabled} data-help={currentScopeNeedsProject ? "Choose a project first, or choose All projects under Where to search." : documentQuery.trim().length < 2 ? "Type at least two characters before searching." : "Run this document search once with the selected filters."}>{currentScopeNeedsProject ? "Choose project first" : documentSearching ? "Searching..." : documentQuery.trim().length < 2 ? "Type 2+ characters" : "Search"}</button>
        {documentSearchError ? <p className="warning-inline" role="alert">{documentSearchError}</p> : null}
      </form>
      {showDocumentResults && !documentSearching ? <p className="muted result-empty">Showing {documentHits.length}{documentSearchTruncated ? "+" : ""} result{documentHits.length === 1 ? "" : "s"}{documentLimit === 0 ? " from unlimited mode" : ""}{documentSearchDuration != null ? ` in ${documentSearchDuration} ms` : ""}.</p> : null}
      <div className="search-hits">
        {documentSearching ? <p className="search-progress-row" role="status"><Loader2 className="spin" size={15} /> Searching indexed documents...</p> : null}
        {showDocumentResults && !documentSearching && documentHits.length === 0 ? <p className="muted result-empty">No indexed documents match this search.</p> : null}
        {showDocumentResults && !documentSearching ? documentHits.map((hit) => {
          const projectName = projectNameById.get(hit.projectId);
          const pathLabel = documentHitPathLabel(hit, projectName);
          const snippet = documentHitSnippet(hit);
          return (
            <button className="hit-row document-hit-row" key={hit.nodeId} type="button" data-help={`Open search result ${hit.title} at ${pathLabel}. Right-click for safe file actions.`} onClick={() => openNode(hit.nodeId)} onContextMenu={(event) => showFileMenu({ nodeId: hit.nodeId, projectId: hit.projectId, path: hit.path, label: hit.title }, event)}>
              <div className="hit-row-title">
                <strong>{hit.title}</strong>
                <span className="hit-path-pill">{pathLabel}</span>
              </div>
              {snippet ? <small className="hit-snippet">{snippet}</small> : null}
            </button>
          );
        }) : null}
      </div>
      {documentSearchRan && selectedProjectId ? (
        <div className="project-next-step" data-help="After discovery, open Safe Manage for a read-only local review of the selected project.">
          <div>
            <strong>Next: Safe Manage</strong>
            <span>Use the selected project as the target and review ownership, references, protection and scan gaps.</span>
          </div>
          <button type="button" onClick={showReview}>
            <ListChecks size={15} />
            Safe Manage
          </button>
        </div>
      ) : null}
    </section>
  );
}

export function documentHitPathLabel(hit: Pick<DocumentHit, "path"> & Partial<Pick<DocumentHit, "title">>, projectName?: string | null) {
  const displayPath = displayLocalPath(hit.path).trim() || "Project root";
  const title = hit.title?.trim();
  const location = title && displayPath.toLowerCase() === title.toLowerCase() ? "Project root" : displayPath;
  return projectName ? `${projectName} / ${location}` : location;
}

export function canRunDocumentSearch(scope: "current" | "all", selectedProjectId: number | null) {
  return canRunCurrentProjectScope(scope, selectedProjectId);
}

export function canSubmitDocumentSearch(query: string, searching: boolean, currentScopeNeedsProject: boolean) {
  return query.trim().length >= 2 && !searching && !currentScopeNeedsProject;
}

export function canRunCurrentProjectScope(scope: "current" | "all" | "file", selectedProjectId: number | null) {
  return scope !== "current" || selectedProjectId != null;
}

export function currentProjectScopeLabel(projects: Array<Pick<ProjectSummary, "id" | "name">>, selectedProjectId: number | null, label = "Current project") {
  const projectName = selectedProjectId == null
    ? null
    : projects.find((project) => project.id === selectedProjectId)?.name ?? null;
  return `${label}${projectName ? `: ${projectName}` : " (choose one)"}`;
}

export function documentHitSnippet(hit: Pick<DocumentHit, "title" | "path" | "snippet">) {
  const snippet = markdownToPlainText(hit.snippet);
  if (!snippet) return null;
  const title = markdownToPlainText(hit.title);
  const path = documentHitPathLabel(hit);
  if (snippet === title || snippet === hit.path.trim() || snippet === path) return null;
  return snippet;
}

export function DiscoverOrphansView({
  orphanMode,
  orphanScope,
  setOrphanScope,
  orphanMinPreset,
  setOrphanMinPreset,
  orphanCustomMiB,
  setOrphanCustomMiB,
  lostStalePreset,
  setLostStalePreset,
  lostKeyword,
  setLostKeyword,
  savedLostPresets,
  applyLostPreset,
  orphanAssetKind,
  setOrphanAssetKind,
  orphanMinConfidence,
  setOrphanMinConfidence,
  advancedMode,
  lostSignals,
  toggleLostSignal,
  lostPresetName,
  setLostPresetName,
  saveLostPreset,
  orphanIncludePartial,
  setOrphanIncludePartial,
  orphanLoading,
  orphanSearchError,
  runOrphanSearch,
  lostProjectCandidates,
  showForgottenProjectMenu,
  selectProject,
  showReview,
  setPlanTargetNode,
  buildPreviewPlan,
  orphanCandidates,
  openNode,
  showFileMenu,
  projects,
  selectedProjectId
}: {
  orphanMode: "lost" | "assets";
  orphanScope: "current" | "all";
  setOrphanScope: (value: "current" | "all") => void;
  orphanMinPreset: string;
  setOrphanMinPreset: (value: string) => void;
  orphanCustomMiB: number;
  setOrphanCustomMiB: (value: number) => void;
  lostStalePreset: string;
  setLostStalePreset: (value: string) => void;
  lostKeyword: string;
  setLostKeyword: (value: string) => void;
  savedLostPresets: LostPreset[];
  applyLostPreset: (name: string) => void;
  orphanAssetKind: string;
  setOrphanAssetKind: (value: string) => void;
  orphanMinConfidence: string;
  setOrphanMinConfidence: (value: string) => void;
  advancedMode: boolean;
  lostSignals: string[];
  toggleLostSignal: (id: string) => void;
  lostPresetName: string;
  setLostPresetName: (value: string) => void;
  saveLostPreset: () => void;
  orphanIncludePartial: boolean;
  setOrphanIncludePartial: (value: boolean) => void;
  orphanLoading: boolean;
  orphanSearchError: string | null;
  runOrphanSearch: () => void;
  lostProjectCandidates: LostProjectCandidates | null;
  showForgottenProjectMenu: (candidate: LostCandidate, event: MouseEvent<HTMLElement>) => void;
  selectProject: (projectId: number) => void;
  showReview: () => void;
  setPlanTargetNode: (target: PlanTarget | null) => void;
  buildPreviewPlan: (nodeId: number) => void;
  orphanCandidates: OrphanCandidates | null;
  openNode: (nodeId: number) => void;
  showFileMenu: (target: FileMenuTarget, event: MouseEvent<HTMLElement>) => void;
  projects: ProjectSummary[];
  selectedProjectId: number | null;
}) {
  const currentScopeNeedsProject = !canRunCurrentProjectScope(orphanScope, selectedProjectId);
  const currentProjectLabel = currentProjectScopeLabel(projects, selectedProjectId);
  const searchSeconds = useElapsedSeconds(orphanLoading);
  const projectNameById = useMemo(
    () => new Map(projects.map((project) => [project.id, project.name])),
    [projects]
  );
  const groupedLostCandidates = useMemo(
    () => groupLostCandidatesForDisplay(lostProjectCandidates?.candidates ?? [], projects),
    [lostProjectCandidates, projects]
  );
  const nestedFolderCount = groupedLostCandidates.folderGroups.reduce(
    (total, group) => total + group.candidates.length,
    0
  );
  useEffect(() => {
    const safePreset = searchMinPresetForMode(orphanMinPreset, advancedMode, "100m");
    if (safePreset !== orphanMinPreset) setOrphanMinPreset(safePreset);
  }, [advancedMode, orphanMinPreset, setOrphanMinPreset]);

  const renderLostCandidate = (candidate: LostCandidate) => {
    const projectName = projectNameById.get(candidate.projectId) ?? null;
    return (
      <div className="orphan-row" key={`${candidate.candidateKind}-${candidate.navId ?? candidate.projectId}-${candidate.path}`} onContextMenu={(event) => showForgottenProjectMenu(candidate, event)} data-help={`Review forgotten ${candidate.candidateKind} signal ${candidate.displayName} in ${projectName ?? `project ${candidate.projectId}`}. Right-click for safe actions.`}>
        <button className="orphan-row-main" type="button" onClick={() => selectProject(candidate.projectId)} data-help={`Select ${candidate.candidateKind} signal ${candidate.displayName} in ${projectName ?? `project ${candidate.projectId}`} for passive review.`}>
          <strong>{candidate.displayName}</strong>
          <span className="orphan-row-facts" aria-label="Candidate facts">
            {orphanCandidateFacts({ ...candidate, projectName }).map((fact) => <span className="orphan-fact-chip" key={fact}>{fact}</span>)}
          </span>
          <span className="orphan-path-pill">{orphanCandidatePathLabel(candidate.path)}</span>
          <small className="orphan-row-reason">{candidate.reason}</small>
        </button>
        <button className="orphan-row-plan" type="button" data-help={`Review ownership, references, protection and space for this ${candidate.candidateKind}. Nothing changes on disk.`} onClick={() => {
          selectProject(candidate.projectId);
          showReview();
          if (candidate.candidateKind === "folder" && candidate.nodeId != null) {
            setPlanTargetNode({ nodeId: candidate.nodeId, label: candidate.displayName, kind: "directory" });
            void buildPreviewPlan(candidate.nodeId);
          } else {
            setPlanTargetNode(null);
            void buildPreviewPlan(candidate.projectId);
          }
        }}>Safe Manage</button>
      </div>
    );
  };
  return (
    <section className="pane-section">
      <form className="filter-panel" aria-busy={orphanLoading} data-help="This discovery search runs only when you press Run search. Results need human review and are not delete recommendations." onSubmit={(event) => {
        event.preventDefault();
        if (!orphanLoading && !currentScopeNeedsProject) void runOrphanSearch();
      }}>
        <div className="filter-grid">
          <label data-help="Search only the active project, or search every local project in the inventory.">
            Where to search
            <select value={orphanScope} onChange={(event) => setOrphanScope(event.target.value as "current" | "all")}>
              <option value="current">{currentProjectLabel}</option>
              <option value="all">All projects</option>
            </select>
          </label>
          <label data-help="Ignore candidates smaller than this size. Lower values find more noise.">
            Smallest item
            <select value={orphanMinPreset} onChange={(event) => setOrphanMinPreset(event.target.value)}>
              <option value="100m">100 MiB</option>
              <option value="1g">1 GiB</option>
              {advancedMode ? <option value="custom">Custom MiB</option> : null}
              {advancedMode ? <option value="0">Any size</option> : null}
            </select>
          </label>
          {orphanMinPreset === "custom" ? (
            <label data-help="Custom minimum footprint in MiB for this passive search.">
              Custom MiB
              <input type="number" min={0} value={orphanCustomMiB} onChange={(event) => setOrphanCustomMiB(Number(event.target.value) || 0)} />
            </label>
          ) : null}
          {orphanMode === "lost" ? (
            <>
              <label data-help="Choose a built-in signal preset. Use Custom filters when you want exact signal checkboxes.">
                Signal preset
                <select value={lostStalePreset} onChange={(event) => setLostStalePreset(event.target.value)}>
                  <option value="any">Any passive signal</option>
                  <option value="forgotten">Forgotten: no recent opens</option>
                  <option value="unfinished">Unfinished: no context or draft name</option>
                  <option value="untracked">Loose folder: no local Git</option>
                  <option value="suspicious">2+ signals</option>
                  <option value="custom">Custom filters only</option>
                </select>
              </label>
              <label data-help="Find project or folder paths containing this word, for example old, archive, client name or experiment name.">
                Keyword
                <input type="text" value={lostKeyword} onChange={(event) => setLostKeyword(event.target.value)} placeholder="optional path/name word" />
              </label>
              <label data-help="Apply a Lost Projects preset saved in this browser session.">
                Saved preset
                <select value="" onChange={(event) => applyLostPreset(event.target.value)}>
                  <option value="">Choose saved preset</option>
                  {savedLostPresets.map((preset) => <option key={preset.name} value={preset.name}>{preset.name}</option>)}
                </select>
              </label>
            </>
          ) : (
            <>
              <label data-help="Restrict unreferenced assets by broad local file category.">
                Asset type
                <select value={orphanAssetKind} onChange={(event) => setOrphanAssetKind(event.target.value)}>
                  <option value="all">All assets</option>
                  <option value="image">Images</option>
                  <option value="media">Media</option>
                  <option value="model">Models</option>
                  <option value="data">Datasets/archives</option>
                </select>
              </label>
              <label data-help="Choose how strong the local filename, path and reference clues must be before a result is shown.">
                Match strength
                <select value={orphanMinConfidence} onChange={(event) => setOrphanMinConfidence(event.target.value)}>
                  <option value="Low">Weak or stronger</option>
                  <option value="Medium">Possible or stronger</option>
                  <option value="High">Strong only</option>
                </select>
              </label>
            </>
          )}
        </div>
        {orphanMode === "lost" && advancedMode ? (
          <details className="advanced-filter-details">
            <summary data-help="Show optional individual signals and controls for saving a reusable local preset.">Advanced signals and saved presets</summary>
            <div className="signal-grid" role="group" aria-label="Lost project signals">
              {LOST_SIGNAL_OPTIONS.map((signal) => (
                <label className="check-row compact" key={signal.id} data-help={signal.help}>
                  <input type="checkbox" checked={lostSignals.includes(signal.id)} onChange={() => toggleLostSignal(signal.id)} />
                  {signal.label}
                </label>
              ))}
            </div>
            <div className="preset-save-row">
              <input type="text" value={lostPresetName} onChange={(event) => setLostPresetName(event.target.value)} onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                saveLostPreset();
              }} placeholder="Preset name" data-help="Name a local Lost Projects filter preset. It is stored only in this browser profile." />
              <button type="button" onClick={saveLostPreset} data-help="Save the current Lost Projects filters as a local preset.">Save preset</button>
            </div>
          </details>
        ) : orphanMode === "lost" ? (
          <p className="muted filter-note">Simple mode uses signal presets. Switch to Advanced for individual signals and saved presets.</p>
        ) : (
          <p className="muted filter-note">Match strength is not a safety score. It only says how convincing the local clues are; every result still needs human review.</p>
        )}
        <label className="check-row" data-help="Include folders where the scanner stopped early. Their footprint is only a minimum count until scanning finishes.">
          <input type="checkbox" checked={orphanIncludePartial} onChange={(event) => setOrphanIncludePartial(event.target.checked)} />
          Include incompletely scanned folders
        </label>
        {currentScopeNeedsProject ? <p className="warning-inline">Choose a project in the sidebar before using Current project, or choose All projects under Where to search.</p> : null}
        {orphanMinPreset === "0" ? <p className="warning-inline">Any size can inspect thousands of folders and take a long time. Add a keyword or signal, or use a size threshold, before running it across all projects.</p> : null}
        <button className="action-button" type="submit" disabled={orphanLoading || currentScopeNeedsProject} data-help={currentScopeNeedsProject ? "Choose a project first, or choose All projects under Where to search." : "Run this local clue search with the selected filters."}>{currentScopeNeedsProject ? "Choose project first" : orphanLoading ? "Searching..." : "Run search"}</button>
        {orphanSearchError ? <p className="warning-inline" role="alert">{orphanSearchError}</p> : null}
      </form>
      {orphanMode === "lost" ? (
        <div className="orphan-list">
          {orphanLoading ? <p className="search-progress-row" role="status"><Loader2 className="spin" size={15} /> Searching local inventory{searchSeconds > 0 ? ` · ${searchSeconds}s` : ""}...</p> : null}
          {!orphanLoading && !lostProjectCandidates ? <p className="muted result-empty">Set the filters, then run the search when you are ready.</p> : null}
          {lostProjectCandidates ? (
            <div className="discovery-results-head">
              <p className="discovery-section-head">Review signals</p>
              <span className="relationship-count-pill">{orphanResultCountLabel(lostProjectCandidates, false)}</span>
            </div>
          ) : null}
          {lostProjectCandidates && lostProjectCandidates.candidates.length === 0 ? <p className="muted result-empty">No forgotten-project signals found with these filters.</p> : null}
          {groupedLostCandidates.projectCandidates.length > 0 ? (
            <>
              <div className="discovery-results-head lost-results-subhead">
                <p className="discovery-section-head">Whole projects</p>
                <span className="relationship-count-pill">{groupedLostCandidates.projectCandidates.length}</span>
              </div>
              {groupedLostCandidates.projectCandidates.map(renderLostCandidate)}
            </>
          ) : null}
          {nestedFolderCount > 0 ? (
            <div className="lost-folder-groups">
              <div className="discovery-results-head lost-results-subhead">
                <p className="discovery-section-head">Folders inside projects</p>
                <span className="relationship-count-pill">{nestedFolderCount}</span>
              </div>
              <p className="muted result-empty">Grouped by owning project. Missing opens, context or Git inside a model/output folder is only a review signal, not proof that the project was forgotten.</p>
              {groupedLostCandidates.folderGroups.map((group) => (
                <details className="lost-folder-project-group" key={group.projectId}>
                  <summary data-help={`Show ${group.candidates.length} folder review signal${group.candidates.length === 1 ? "" : "s"} inside ${group.projectName}.`}>
                    <span>{group.projectName}</span>
                    <small>{group.candidates.length} folder{group.candidates.length === 1 ? "" : "s"}</small>
                  </summary>
                  <div className="lost-folder-project-list">
                    {group.candidates.map(renderLostCandidate)}
                  </div>
                </details>
              ))}
            </div>
          ) : null}
        </div>
      ) : (
        <div className="orphan-list">
          {orphanLoading ? <p className="search-progress-row" role="status"><Loader2 className="spin" size={15} /> Searching local inventory{searchSeconds > 0 ? ` · ${searchSeconds}s` : ""}...</p> : null}
          {!orphanLoading && !orphanCandidates ? <p className="muted result-empty">Set the filters, then run the search when you are ready.</p> : null}
          {orphanCandidates ? (
            <div className="discovery-results-head">
              <p className="discovery-section-head">Unreferenced file signals</p>
              <span className="relationship-count-pill">{orphanResultCountLabel(orphanCandidates, false)}</span>
            </div>
          ) : null}
          {orphanCandidates && orphanCandidates.candidates.length === 0 ? <p className="muted result-empty">No unreferenced asset candidates found with these filters.</p> : null}
          {orphanCandidates?.candidates.map((candidate) => (
            <div className="orphan-row" key={candidate.nodeId}>
              <button className="orphan-row-main" type="button" onClick={() => void openNode(candidate.nodeId)} onContextMenu={(event) => showFileMenu({ nodeId: candidate.nodeId, projectId: candidate.projectId, path: candidate.path, label: candidate.displayName }, event)} data-help={`Open unreferenced asset candidate ${candidate.displayName} for metadata review. Right-click for safe file actions.`}>
                <strong>{candidate.displayName}</strong>
                <span className="orphan-row-facts" aria-label="Candidate facts">
                  {orphanCandidateFacts(candidate).map((fact) => <span className="orphan-fact-chip" key={fact}>{fact}</span>)}
                </span>
                <span className="orphan-path-pill">{orphanCandidatePathLabel(candidate.path)}</span>
                <small className="orphan-row-reason">{candidate.reason}</small>
              </button>
              <button className="orphan-row-plan" type="button" data-help="Review ownership, references, protection and space for this unreferenced file. Nothing changes on disk." onClick={() => {
                selectProject(candidate.projectId);
                setPlanTargetNode({ nodeId: candidate.nodeId, label: candidate.displayName, kind: "file" });
                showReview();
                void buildPreviewPlan(candidate.nodeId);
              }}>Safe Manage</button>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

export function duplicateConfirmationGroupKey(
  group: Pick<DuplicateGroup, "sizeBytes" | "hashPartial" | "members">
) {
  return `${group.sizeBytes}:${group.hashPartial}:${group.members
    .map((member) => member.nodeId)
    .sort((left, right) => left - right)
    .join(",")}`;
}

export function DiscoverDuplicatesView({
  duplicateScope,
  setDuplicateScope,
  preview,
  duplicateMinPreset,
  setDuplicateMinPreset,
  duplicateCustomMiB,
  setDuplicateCustomMiB,
  duplicateFileKind,
  setDuplicateFileKind,
  duplicateLimit,
  setDuplicateLimit,
  duplicateLoading,
  duplicateSearchError,
  loadDuplicateCandidates,
  duplicateHasRun,
  duplicateCandidates,
  advancedMode,
  openNode,
  showFileMenu,
  projects,
  selectedProjectId,
  confirmState,
  setConfirmState
}: {
  duplicateScope: "file" | "current" | "all";
  setDuplicateScope: (value: "file" | "current" | "all") => void;
  preview: FilePreview | null;
  duplicateMinPreset: string;
  setDuplicateMinPreset: (value: string) => void;
  duplicateCustomMiB: number;
  setDuplicateCustomMiB: (value: number) => void;
  duplicateFileKind: string;
  setDuplicateFileKind: (value: string) => void;
  duplicateLimit: number;
  setDuplicateLimit: (value: number) => void;
  duplicateLoading: boolean;
  duplicateSearchError: string | null;
  loadDuplicateCandidates: () => void;
  duplicateHasRun: boolean;
  duplicateCandidates: DuplicateCandidates | null;
  advancedMode: boolean;
  openNode: (nodeId: number) => void;
  showFileMenu: (target: FileMenuTarget, event: MouseEvent<HTMLElement>) => void;
  projects: ProjectSummary[];
  selectedProjectId: number | null;
  confirmState: DuplicateConfirmStateMap;
  setConfirmState: Dispatch<SetStateAction<DuplicateConfirmStateMap>>;
}) {
  const currentScopeNeedsProject = !canRunCurrentProjectScope(duplicateScope, selectedProjectId);
  const currentProjectLabel = currentProjectScopeLabel(projects, selectedProjectId);
  const searchSeconds = useElapsedSeconds(duplicateLoading);
  const confirmationRunning = Object.values(confirmState).some((confirmation) => confirmation.loading);
  const searchDisabled = duplicateLoading || currentScopeNeedsProject || confirmationRunning;
  const searchActionLabel = currentScopeNeedsProject
    ? "Choose project first"
    : confirmationRunning
      ? "Finish or cancel comparison"
      : duplicateLoading
        ? "Searching..."
        : "Find duplicate candidates";
  const searchActionHelp = currentScopeNeedsProject
    ? "Choose a project first, or choose All projects under Where to search."
    : confirmationRunning
      ? "Finish or cancel the active complete file comparison before replacing these candidate groups."
      : "Compare file sizes and small content samples to find candidates. No cleanup action is created.";
  useEffect(() => {
    const safePreset = searchMinPresetForMode(duplicateMinPreset, advancedMode, "10m");
    if (safePreset !== duplicateMinPreset) setDuplicateMinPreset(safePreset);
  }, [advancedMode, duplicateMinPreset, setDuplicateMinPreset]);

  const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

  const confirmGroup = async (groupKey: string, nodeId: number) => {
    setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: true } }));
    let jobId: string;
    try {
      jobId = await api.confirmDuplicateGroupStart(nodeId);
    } catch (error) {
      setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: false, error: errorText(error) } }));
      return;
    }
    setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: true, jobId } }));
    // Poll the background job until terminal. Full-hashing streams every byte, so this can run a
    // while on large models; the Cancel button stops it at the next file boundary.
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 250));
      let status;
      try {
        status = await api.confirmDuplicateGroupStatus(jobId);
      } catch (error) {
        setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: false, error: errorText(error) } }));
        return;
      }
      if (status.state === "completed") {
        setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: false, jobId, result: status.result ?? undefined, progress: status.progress } }));
        return;
      }
      if (status.state === "cancelled") {
        setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: false } }));
        return;
      }
      if (status.state === "failed") {
        setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: false, error: status.error ?? "Confirmation failed." } }));
        return;
      }
      setConfirmState((prev) => ({ ...prev, [groupKey]: { loading: true, jobId, progress: status.progress } }));
    }
  };

  const cancelConfirm = (groupKey: string) => {
    const jobId = confirmState[groupKey]?.jobId;
    if (jobId) {
      void api.confirmDuplicateGroupCancel(jobId);
    }
  };

  const groupedDuplicates = groupDuplicateGroupsForDisplay(duplicateCandidates?.groups ?? []);
  const renderDuplicateGroup = (group: DuplicateGroup) => {
    const firstMember = group.members[0];
    const groupKey = duplicateConfirmationGroupKey(group);
    const confirm = confirmState[groupKey];
    const confirmedCount = confirm?.result?.confirmedGroups.reduce((acc, confirmedGroup) => acc + confirmedGroup.memberCount, 0) ?? 0;
    return (
      <div className="duplicate-group" key={`${group.id}-${group.hashPartial}`} data-help={`${group.memberCount} files have the same size and matching sampled content. Confirm reads the complete files to check whether every byte is identical.`}>
        <div className="duplicate-group-summary">
          <strong>{group.memberCount} matching files</strong>
          <span>{formatBytes(group.sizeBytes)} each · {plainConfidenceLabel(group.confidence, "candidate match")}</span>
          <small>Same file size and matching content sample.</small>
          <details className="duplicate-technical-match">
            <summary>Technical match details</summary>
            <small>{group.reason}</small>
          </details>
          <small>{formatOptionalBytes(group.physicalBytes)} space used{group.footprintPartial ? "+" : ""}</small>
          {firstMember ? (
            <div className="duplicate-confirm-row">
              <button className="action-button subtle" type="button" disabled={confirm?.loading || Boolean(confirm?.result)} data-help={confirm?.result ? "Complete byte-for-byte comparison finished for this exact candidate group." : "Read every byte to confirm whether the files are identical. No file is deleted or moved."} onClick={() => void confirmGroup(groupKey, firstMember.nodeId)}>
                {confirm?.loading ? <Loader2 className="spin" size={13} /> : null}
                {confirm?.result ? <CheckCircle2 size={13} /> : null}
                {confirm?.loading
                  ? confirm.progress && confirm.progress.totalBytes > 0
                    ? `Comparing... ${Math.round((confirm.progress.bytesHashed / confirm.progress.totalBytes) * 100)}%`
                    : "Comparing..."
                  : confirm?.result
                    ? "Checked"
                    : "Confirm byte for byte"}
              </button>
              {confirm?.loading ? <button className="action-button subtle" type="button" data-help="Stop the complete comparison at the next file boundary. Nothing is changed on disk." onClick={() => cancelConfirm(groupKey)}>Cancel</button> : null}
              {confirm?.error ? <small className="warning-inline">Confirmation failed: {confirm.error}</small> : null}
              {confirm?.result && !confirm.loading ? (
                confirmedCount > 0 ? (
                  <small className="duplicate-confirm-result ok"><CheckCircle2 size={14} /> {confirmedCount} confirmed identical · {formatBytes(confirm.result.reclaimableBytes)} reclaimable{confirm.result.partial ? " · some files skipped" : ""}</small>
                ) : (
                  <small className="duplicate-confirm-result muted">The complete comparison found no identical files{confirm.result.partial ? " · some files skipped" : ""}</small>
                )
              ) : null}
            </div>
          ) : null}
        </div>
        <div className="duplicate-member-list">
          {group.members.map((member) => (
            <button className="duplicate-member-row" type="button" key={member.nodeId} onClick={() => void openNode(member.nodeId)} onContextMenu={(event) => showFileMenu({ nodeId: member.nodeId, projectId: member.projectId, path: member.path, label: member.displayName }, event)} data-help={`Open duplicate candidate ${member.displayName} for metadata review. Right-click for safe file actions.`}>
              <strong>{member.displayName}</strong>
              <span>{member.projectName}</span>
              <small>{member.path} · {formatOptionalBytes(member.physicalBytes)} space used{member.footprintPartial ? "+" : ""}</small>
            </button>
          ))}
        </div>
      </div>
    );
  };

  return (
    <section className="pane-section">
      <form className="filter-panel" aria-busy={duplicateLoading} data-help="Duplicate search can touch many large files. Filter by size and type before running it." onSubmit={(event) => {
        event.preventDefault();
        if (!searchDisabled) void loadDuplicateCandidates();
      }}>
        <div className="filter-grid">
          <label data-help="Search duplicate candidates for the open file, the active project, or every local project. Current file is usually the fastest path.">
            Where to search
            <select value={duplicateScope} onChange={(event) => setDuplicateScope(event.target.value as "file" | "current" | "all")}>
              <option value="file" disabled={!preview}>Current file{preview ? `: ${preview.displayName}` : ""}</option>
              <option value="current">{currentProjectLabel}</option>
              <option value="all">All projects</option>
            </select>
          </label>
          <label data-help="Ignore files smaller than this size. A larger minimum is faster and reduces noise.">
            Smallest file
            <select value={duplicateMinPreset} onChange={(event) => setDuplicateMinPreset(event.target.value)}>
              <option value="10m">10 MiB</option>
              <option value="100m">100 MiB</option>
              <option value="1g">1 GiB</option>
              {advancedMode ? <option value="custom">Custom MiB</option> : null}
              {advancedMode ? <option value="0">Any size</option> : null}
            </select>
          </label>
          {duplicateMinPreset === "custom" ? (
            <label data-help="Custom minimum size in MiB for duplicate candidate hashing.">
              Custom MiB
              <input type="number" min={0} value={duplicateCustomMiB} onChange={(event) => setDuplicateCustomMiB(Number(event.target.value) || 0)} />
            </label>
          ) : null}
          <label data-help="Restrict duplicate candidates to large local file categories most likely to matter for disk review.">
            File type
            <select value={duplicateFileKind} onChange={(event) => setDuplicateFileKind(event.target.value)}>
              <option value="all">All supported</option>
              <option value="model">Models</option>
              <option value="media">Media</option>
              <option value="data">Datasets/archives</option>
            </select>
          </label>
          <label data-help="Limit the number of candidate groups returned. Unlimited can take longer to render and review.">
            Result groups
            <select value={duplicateLimit} onChange={(event) => setDuplicateLimit(Number(event.target.value))}>
              <option value={10}>10</option>
              <option value={25}>25</option>
              <option value={50}>50</option>
              <option value={0}>Unlimited</option>
            </select>
          </label>
        </div>
        {duplicateScope === "file" ? <p className="muted filter-note">Current file first compares file sizes and a small content sample. Use Confirm byte for byte on any result before treating files as identical.</p> : null}
        {currentScopeNeedsProject ? <p className="warning-inline">Choose a project in the sidebar before using Current project, or choose All projects under Where to search.</p> : null}
        {duplicateMinPreset === "0" ? <p className="warning-inline">Any size can compare a very large number of files. Use a size threshold or narrow the file type unless you need an exhaustive Advanced search.</p> : null}
        {duplicateLimit === 0 ? <p className="warning-inline">Unlimited groups can make this panel slow if many large files share sizes. The first search reads only a small sample; complete comparison starts only when you choose it.</p> : null}
        <button className="action-button" type="submit" disabled={searchDisabled} data-help={searchActionHelp}>{searchActionLabel}</button>
        {duplicateSearchError ? <p className="warning-inline" role="alert">{duplicateSearchError}</p> : null}
      </form>
      <dl className="inspector-list small">
        <dt>Groups</dt>
        <dd>{duplicateGroupCountLabel(duplicateCandidates, duplicateHasRun, duplicateLoading)}</dd>
        <dt>Policy</dt>
        <dd>Review only, no cleanup actions</dd>
      </dl>
      <div className="duplicate-list">
        {duplicateLoading ? <p className="search-progress-row" role="status"><Loader2 className="spin" size={15} /> Comparing candidate files{searchSeconds > 0 ? ` · ${searchSeconds}s` : ""}...</p> : null}
        {!duplicateHasRun ? <p className="muted result-empty">Set filters and run the search when you are ready.</p> : null}
        {duplicateHasRun && duplicateCandidates && duplicateCandidates.groups.length === 0 ? <p className="muted result-empty">No duplicate candidates found with these filters.</p> : null}
        {groupedDuplicates.directGroups.map((group) => renderDuplicateGroup(group))}
        {duplicateHasRun && groupedDuplicates.directGroups.length === 0 && groupedDuplicates.cacheGroups.length ? (
          <p className="muted result-empty">Only dependency-cache duplicate groups matched these filters. Expand the cache group if you need to inspect them.</p>
        ) : null}
        {groupedDuplicates.cacheGroups.length ? (
          <>
            <details className="graph-cache-group duplicate-cache-group">
              <summary>Inside dependency caches ({groupedDuplicates.cacheGroups.length})</summary>
              <div className="duplicate-cache-list">
                {groupedDuplicates.cacheGroups.map((group) => renderDuplicateGroup(group))}
              </div>
            </details>
            <p className="muted result-empty">{groupedDuplicates.cacheGroups.length} of {duplicateCandidates?.groups.length ?? groupedDuplicates.cacheGroups.length} duplicate groups live inside dependency caches — grouped for readability, search inputs unchanged.</p>
          </>
        ) : null}
      </div>
    </section>
  );
}

export function groupDuplicateGroupsForDisplay(groups: DuplicateGroup[]) {
  const directGroups: DuplicateGroup[] = [];
  const cacheGroups: DuplicateGroup[] = [];
  for (const group of groups) {
    if (isDependencyCacheDuplicateGroup(group)) cacheGroups.push(group);
    else directGroups.push(group);
  }
  return { directGroups, cacheGroups };
}

export function duplicateGroupCountLabel(
  candidates: DuplicateCandidates | null,
  hasRun: boolean,
  loading: boolean
) {
  if (loading) return "searching";
  if (!hasRun) return "not run";
  const shown = candidates?.groups.length ?? 0;
  const total = candidates?.total ?? 0;
  return total > shown ? `${shown} of ${total} shown` : String(total);
}

function isDependencyCacheDuplicateGroup(group: DuplicateGroup) {
  return group.members.length > 0 && group.members.every((member) => textMentionsDependencyCache(member.path));
}
