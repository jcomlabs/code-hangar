import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { ArchiveRestore, ArrowRight, CheckCircle2, Compass, FileDiff, FileText, FolderOpen, Inbox, ListChecks, SlidersHorizontal } from "lucide-react";
import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import type { AdapterSummary, DashboardSummary, GitRepoSummary, ProjectReviewCheckpoint, ProjectSummary, SessionDiscoveryCandidate, WatcherStatus } from "../types";
import { CountUp } from "../ui";

export function OverviewView({
  showFlow,
  selectedProjectId,
  realProjectCount,
  mutationAvailable,
  dashboard,
  watcherStatus,
  dashboardLoading,
  gitStatus,
  adapters,
  demosVisible,
  demoPreference,
  reduceMotion,
  formatBytes,
  formatOptionalBytes,
  onOpenProject,
  onAddProjects,
  onSetShowDemoProjects,
  onOpenScanFolders,
  onUnderstandProject,
  onOpenFiles,
  reviewProjectGroups,
  reviewInventoryReady,
  onOpenRecap,
  onOpenProjectRecap,
  onDiscover,
  onReview,
  onRecovery
}: {
  showFlow: boolean;
  selectedProjectId: number | null;
  realProjectCount: number;
  mutationAvailable: boolean;
  dashboard: DashboardSummary | null;
  watcherStatus: WatcherStatus | null;
  dashboardLoading: boolean;
  gitStatus: GitRepoSummary | null;
  adapters: AdapterSummary[];
  demosVisible: boolean;
  demoPreference: boolean | null;
  reduceMotion: boolean;
  formatBytes: (value: number) => string;
  formatOptionalBytes: (value?: number | null) => string;
  onOpenProject: (projectId: number) => void;
  onAddProjects: () => void;
  onSetShowDemoProjects: (show: boolean) => void;
  onOpenScanFolders: () => void;
  onUnderstandProject: () => void;
  onOpenFiles: () => void;
  reviewProjectGroups: ReviewProjectGroup[];
  reviewInventoryReady: boolean;
  onOpenRecap: () => void;
  onOpenProjectRecap: (projectId: number) => void;
  onDiscover: () => void;
  onReview: () => void;
  onRecovery: () => void;
}) {
  const [footprintListOpen, setFootprintListOpen] = useState(false);
  const [inventoryDetailsOpen, setInventoryDetailsOpen] = useState(false);
  const [reviewCheckpoints, setReviewCheckpoints] = useState<ProjectReviewCheckpoint[]>([]);
  const [reviewInboxLoading, setReviewInboxLoading] = useState(true);
  const [reviewInboxError, setReviewInboxError] = useState<string | null>(null);
  const [reviewInboxRetry, setReviewInboxRetry] = useState(0);
  const footprintSlices = useMemo(() => buildFootprintSlices(dashboard?.largestProjects ?? []), [dashboard?.largestProjects]);
  const recapStep = overviewRecapStep(selectedProjectId);
  const understandStep = overviewUnderstandProjectStep(selectedProjectId);
  const tweakStep = overviewTweakStep(selectedProjectId, mutationAvailable);
  const safeManageStep = overviewSafeManageStep(selectedProjectId);
  const inventoryHealth = overviewInventoryHealth(dashboard, watcherStatus);
  const reviewInboxItems = useMemo(
    () => buildReviewInboxItems(reviewProjectGroups, reviewCheckpoints),
    [reviewCheckpoints, reviewProjectGroups]
  );
  const reviewInboxNewSessions = reviewInboxItems.reduce((total, item) => total + item.unreviewedCount, 0);
  const reviewInboxUnknownDates = reviewInboxItems.reduce((total, item) => total + item.unknownTimestampCount, 0);

  useEffect(() => {
    if (!showFlow || !reviewInventoryReady) return;
    let current = true;
    setReviewInboxLoading(true);
    setReviewInboxError(null);
    void api.projectReviewCheckpoints()
      .then((checkpoints) => {
        if (current) setReviewCheckpoints(checkpoints);
      })
      .catch((reason: unknown) => {
        if (current) setReviewInboxError(String(reason));
      })
      .finally(() => {
        if (current) setReviewInboxLoading(false);
      });
    return () => { current = false; };
  }, [reviewInboxRetry, reviewInventoryReady, showFlow]);

  return (
    <>
      {realProjectCount === 0 ? (
        <div className="welcome-card" data-help="Add a real local folder to start using Code Hangar with your own projects. Demo projects are only examples.">
          <div>
            <span>First run</span>
            <h3>Add your first projects</h3>
            <p>Add a local project so Code Hangar can reconstruct what its AI sessions recorded, help you understand the result, and guide one small reversible correction when this edition supports editing.</p>
          </div>
          <div className="welcome-actions">
            <button type="button" className="action-button" onClick={onAddProjects} data-help="Open Add Projects to add a folder directly or search a wider location for project candidates. Code Hangar does not modify files while scanning.">
              <FolderOpen size={16} />
              Add Projects
            </button>
            <button type="button" className="secondary-button" onClick={() => onSetShowDemoProjects(!demosVisible)} data-help="Show or hide built-in demo projects in the Projects list. This changes only the local UI preference.">
              {demosVisible ? "Hide demo projects" : "Show demo projects"}
            </button>
            <small>{demoPreference === null ? "Automatic: demos are shown while no real projects exist." : "Using your saved demo preference."}</small>
          </div>
        </div>
      ) : null}

      {showFlow ? (
        <div className="overview-flow" data-help="Start here: reconstruct what an AI recorded, understand the affected code, then make at most one small reversible correction.">
          <div>
            <span>Recommended flow</span>
            <h3>{mutationAvailable ? "See what the AI did. Understand it. Change one thing safely." : "See what the AI did. Understand it locally."}</h3>
            <p>{overviewFlowSafetyCopy(mutationAvailable)}</p>
          </div>
          <div className="flow-step-grid">
            <button type="button" onClick={onOpenRecap} data-help={recapStep.help}>
              <FileDiff size={16} />
              <strong>{recapStep.label}</strong>
              <small>{recapStep.detail}</small>
            </button>
            <button type="button" disabled={!understandStep.enabled} onClick={onUnderstandProject} data-help={understandStep.help}>
              <FileText size={16} />
              <strong>{understandStep.label}</strong>
              <small>{understandStep.detail}</small>
            </button>
            <button type="button" disabled={!tweakStep.enabled} onClick={onOpenFiles} data-help={tweakStep.help}>
              <SlidersHorizontal size={16} />
              <strong>{tweakStep.label}</strong>
              <small>{tweakStep.detail}</small>
            </button>
          </div>
          <div className="overview-support-actions" aria-label="Inventory and safety tools">
            <span>Inventory and safety tools</span>
            <button type="button" onClick={onDiscover} data-help="Open passive discovery for forgotten projects, unreferenced files and duplicate candidates.">
              <Compass size={15} /> Discover
            </button>
            <button type="button" disabled={!safeManageStep.enabled} onClick={onReview} data-help={safeManageStep.help}>
              <ListChecks size={15} /> Safe Manage
            </button>
            <button type="button" onClick={onRecovery} data-help="Open Recover to review local recovery records and previous file versions when they exist.">
              <ArchiveRestore size={16} />
              Recover
            </button>
          </div>
        </div>
      ) : null}

      {showFlow ? (
        <div className="dashboard-card review-inbox" data-help="Projects with AI conversations newer than the point where you last marked them reviewed. Opening one compares those records with file changes Git can see now.">
          <div className="overview-recap-heading">
            <Inbox size={18} />
            <div>
              <span>Review Inbox</span>
              <div className="heading-with-help">
                <h3>{reviewInboxTitle(reviewInventoryReady, reviewInboxLoading, reviewInboxError, reviewInboxNewSessions, reviewInboxUnknownDates)}</h3>
                <ConceptHelp concept="whatChanged" />
              </div>
              <p>Review recorded AI work across projects. Opening an item checks local Git and current file reality before you mark it reviewed.</p>
            </div>
            {reviewInboxError ? <button type="button" className="secondary-button compact" onClick={() => setReviewInboxRetry((value) => value + 1)}>Retry</button> : null}
          </div>
          {reviewInventoryReady && !reviewInboxLoading && !reviewInboxError && reviewInboxItems.length > 0 ? (
            <div className="review-inbox-list">
              {reviewInboxItems.slice(0, 6).map((item) => (
                <button type="button" key={item.project.id} onClick={() => onOpenProjectRecap(item.project.id)}>
                  <span className="review-inbox-project"><strong>{item.project.name}</strong><small>{reviewInboxItemDetail(item)}</small></span>
                  <span className="review-inbox-time">{item.latestModifiedMs == null ? "Date unavailable" : new Date(item.latestModifiedMs).toLocaleString()}</span>
                  <ArrowRight size={15} />
                </button>
              ))}
              {reviewInboxItems.length > 6 ? <small className="review-inbox-overflow">{reviewInboxItems.length - 6} more project{reviewInboxItems.length - 6 === 1 ? "" : "s"} remain in the inbox.</small> : null}
            </div>
          ) : null}
          {reviewInventoryReady && !reviewInboxLoading && !reviewInboxError && reviewInboxItems.length === 0 ? (
            <div className="review-inbox-clear"><CheckCircle2 size={17} /><span><strong>No newer session records</strong><small>Open What changed on any project when you want to compare current local Git evidence.</small></span></div>
          ) : null}
        </div>
      ) : null}

      <div className="dashboard-card" data-help="Inventory Map shows what Code Hangar currently knows from local scans: projects, files, searchable text, protected entries and incomplete scans.">
        <div className="card-title-row">
          <div className="heading-with-help">
            <h3>Inventory Map</h3>
            <ConceptHelp concept="inventory" />
          </div>
          {!dashboardLoading || dashboard ? (
            <button type="button" className="secondary-button compact" onClick={() => setInventoryDetailsOpen((open) => !open)} data-help="Show lower-priority counters, local Git signals and the built-in detector list.">
              {inventoryDetailsOpen ? "Hide details" : "Show details"}
            </button>
          ) : null}
        </div>
        {dashboardLoading && !dashboard ? <p className="muted help-copy">Loading local inventory totals. Projects and files remain usable while this runs.</p> : null}
        <div className={`overview-health-strip ${inventoryHealth.tone}`} data-help={inventoryHealth.help}>
          <div className="overview-health-primary">
            <span>{inventoryHealth.eyebrow}</span>
            <strong>{inventoryHealth.title}</strong>
            <small>{inventoryHealth.detail}</small>
          </div>
          <div className="overview-health-track" aria-hidden="true">
            <span style={{ width: `${inventoryHealth.progress}%` }} />
          </div>
          <div className="overview-health-facts">
            {inventoryHealth.facts.map((fact) => (
              <span key={fact.label}>
                <strong><CountUp value={fact.value} reduceMotion={reduceMotion} /></strong>
                <small>{fact.label}</small>
              </span>
            ))}
          </div>
          {watcherStatus && watcherStatus.staleProjects > 0 ? (
            <button type="button" className="secondary-button slim" onClick={onOpenScanFolders} data-help="Open Scan Folders so you can rescan changed roots, inspect missing roots or disable folders that no longer matter.">
              Review scan folders
            </button>
          ) : null}
        </div>
        {dashboardLoading && !dashboard ? (
          <div className="dashboard-grid" aria-hidden="true">
            {Array.from({ length: 6 }).map((_, index) => (
              <div className="metric-row metric-row-skeleton" key={`metric-skeleton-${index}`}>
                <span className="skeleton skeleton-line skeleton-line-label" />
                <strong><span className="skeleton skeleton-line skeleton-line-value" /></strong>
              </div>
            ))}
          </div>
        ) : (
          <div className="dashboard-grid">
            <Metric label="Projects mapped" value={dashboard?.totalProjects ?? 0} help="Local folders currently registered as Code Hangar projects." reduceMotion={reduceMotion} />
            <Metric label="Files/folders mapped" value={dashboard?.totalItems ?? 0} help="All file and folder metadata entries in the inventory. This is not stored file content." reduceMotion={reduceMotion} />
            <Metric label="Context documents" value={dashboard?.contextFiles ?? 0} help="Files that look useful for understanding a project, such as README, AGENTS, docs and config context." reduceMotion={reduceMotion} />
            <Metric label="Searchable text" value={dashboard?.indexedDocuments ?? 0} help="Small, non-sensitive text bodies indexed for Document Search." reduceMotion={reduceMotion} />
            <Metric label="Protected entries" value={dashboard?.protectedFiles ?? 0} help="Files or folders under protected or sensitive policy. They remain excluded from normal preview/search actions." reduceMotion={reduceMotion} />
            <Metric label="Incomplete scans" value={dashboard?.partialItems ?? 0} help="Folders where scan stopped, was cancelled or hit a safe limit. Their sizes are minimum counts until scanning finishes." reduceMotion={reduceMotion} />
          </div>
        )}
        {dashboardLoading && !dashboard ? null : inventoryDetailsOpen ? (
          <div className="dashboard-grid overview-detail-grid">
            <Metric label="Not searchable by content" value={dashboard?.nonIndexedItems ?? 0} help="Inventory entries with no indexed text body, so Document Search cannot match their contents." reduceMotion={reduceMotion} />
            <Metric label="Sensitive-looking files" value={dashboard?.sensitiveFiles ?? 0} help="Files whose name or location matches a sensitive pattern. They stay excluded from normal preview and search." reduceMotion={reduceMotion} />
            <Metric label="Scan folders" value={dashboard?.scanRoots ?? 0} help="Root folders Code Hangar has been pointed at for local scanning." reduceMotion={reduceMotion} />
            <div className="metric-row" data-help="Whether local disk state has drifted from the last scan for the registered roots.">
              <span>Disk freshness</span>
              <strong className="metric-text-value">{dashboard?.staleOrDirty ?? "not evaluated"}</strong>
            </div>
          </div>
        ) : null}
      </div>

      {inventoryDetailsOpen ? (
        <>
          <div className="dashboard-card" data-help="Local Git Signals come from files already on disk, usually .git metadata. Code Hangar does not run Git commands or contact remotes.">
            <div className="heading-with-help">
              <h3>Local Git Signals</h3>
              <ConceptHelp concept="git" />
            </div>
            <p className="muted help-copy">Passive local metadata only. No remote Git command or remote API is used.</p>
            <dl className="inspector-list small">
              <dt>Projects</dt>
              <dd>{dashboard?.gitProjects ?? 0} with local Git metadata</dd>
              <dt>Current</dt>
              <dd>{gitStatus?.hasGit ? gitStatus.currentBranch ?? "detached" : "not detected for selected project"}</dd>
            </dl>
          </div>

          <div className="dashboard-card" data-help="Local Detectors are built-in passive classifiers. They inspect already-scanned metadata and safe text context; they do not execute project code or contact the network.">
            <h3>Local Detectors</h3>
            <p className="muted help-copy">{adapters.length} built-in detectors · {dashboard?.adaptersNeedingReview ?? 0} needing review. They help Code Hangar explain projects, not change them.</p>
            {adapters.length ? (
              <div className="adapter-list">
                {adapters.map((adapter) => (
                  <div className="adapter-row" key={adapter.id} data-help={`${detectorLabel(adapter)}: ${detectorDescription(adapter)}`}>
                    <strong>{detectorLabel(adapter)}</strong>
                    <span>{detectorDescription(adapter)}</span>
                    <small>{adapter.enabled ? "Active local detector" : "Disabled detector"}</small>
                  </div>
                ))}
              </div>
            ) : null}
          </div>
        </>
      ) : null}

      {dashboard?.largestProjects.length ? (
        <div className="dashboard-card" data-help="Largest projects on disk uses local file-size information. Incomplete values are minimum counts until scanning finishes.">
          <div className="heading-with-help">
            <h3>Largest projects on disk</h3>
            <ConceptHelp concept="space" />
          </div>
          <p className="muted help-copy">Estimated space used by each project. Small projects are grouped as Other in the chart; open the list for the complete ranking.</p>
          <div className="footprint-overview">
            <FootprintDonut
              slices={footprintSlices}
              formatBytes={formatBytes}
              onOpenProject={onOpenProject}
              onOpenOther={() => setFootprintListOpen(true)}
            />
            <div className="footprint-summary-list">
              {footprintSlices.map((slice) => (
                <button
                  className="footprint-legend-row"
                  type="button"
                  key={slice.key}
                  style={{ "--slice-color": slice.color } as CSSProperties}
                  onClick={() => slice.projectId ? onOpenProject(slice.projectId) : setFootprintListOpen(true)}
                  data-help={slice.projectId ? `Open ${slice.name} from the footprint chart.` : "Open the full footprint list for smaller projects grouped as Other."}
                >
                  <span className="slice-dot" />
                  <strong>{slice.name}</strong>
                  <small>{slice.percent.toFixed(slice.percent >= 10 ? 0 : 1)}% · {formatBytes(slice.bytes)}</small>
                </button>
              ))}
            </div>
          </div>
          <button type="button" className="secondary-button compact" onClick={() => setFootprintListOpen((open) => !open)} data-help="Show or hide the full project footprint ranking, sorted like a disk usage viewer.">
            {footprintListOpen ? "Hide footprint list" : "Open footprint list"}
          </button>
          {footprintListOpen ? (
            <div className="largest-list footprint-tree-list" data-help="TreeSize-style project ranking. Click a row to open that project; incomplete values are minimum counts until scanning finishes.">
              {dashboard.largestProjects.map((project, index) => (
                <button className="largest-row button-row" type="button" key={project.projectId} data-help={`Open project ${project.name} from the footprint ranking.`} onClick={() => onOpenProject(project.projectId)}>
                  <strong>{index + 1}. {project.name}</strong>
                  <span>
                    {formatOptionalBytes(project.physicalBytes ?? project.allocatedBytes ?? project.apparentBytes)}
                    {project.footprintPartial ? " incomplete physical estimate" : " physical estimate"}
                  </span>
                  <span>{formatBytes(project.apparentBytes)} total file sizes</span>
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
    </>
  );
}

export interface ReviewProjectGroup {
  project: ProjectSummary;
  sessions: SessionDiscoveryCandidate[];
}

export interface ReviewInboxItem {
  project: ProjectSummary;
  unreviewedCount: number;
  unknownTimestampCount: number;
  latestModifiedMs: number | null;
  hasCheckpoint: boolean;
}

export function buildReviewInboxItems(
  groups: ReviewProjectGroup[],
  checkpoints: ProjectReviewCheckpoint[]
): ReviewInboxItem[] {
  const checkpointByProject = new Map(checkpoints.map((checkpoint) => [checkpoint.projectId, checkpoint]));
  return groups
    .map(({ project, sessions }): ReviewInboxItem => {
      const checkpoint = checkpointByProject.get(project.id);
      const datedForReview = checkpoint
        ? sessions.filter((session) => session.modifiedMs != null && session.modifiedMs > checkpoint.sessionCutoffMs)
        : sessions;
      const unknownTimestampCount = checkpoint
        ? sessions.filter((session) => session.modifiedMs == null).length
        : 0;
      const latestModifiedMs = datedForReview.reduce<number | null>((latest, session) => {
        if (session.modifiedMs == null) return latest;
        return latest == null ? session.modifiedMs : Math.max(latest, session.modifiedMs);
      }, null);
      return {
        project,
        unreviewedCount: datedForReview.length,
        unknownTimestampCount,
        latestModifiedMs,
        hasCheckpoint: Boolean(checkpoint)
      };
    })
    .filter((item) => item.unreviewedCount > 0 || item.unknownTimestampCount > 0)
    .sort((left, right) =>
      (right.latestModifiedMs ?? -1) - (left.latestModifiedMs ?? -1)
      || left.project.name.localeCompare(right.project.name)
    );
}

export function reviewInboxTitle(
  inventoryReady: boolean,
  loading: boolean,
  error: string | null,
  unreviewedCount: number,
  unknownTimestampCount: number
) {
  if (!inventoryReady) return "Restoring the local session index";
  if (loading) return "Checking saved review checkpoints";
  if (error) return "Review checkpoints are temporarily unavailable";
  if (unreviewedCount > 0) {
    return `${unreviewedCount} new session record${unreviewedCount === 1 ? " needs" : "s need"} review${unknownTimestampCount > 0 ? ` · ${unknownTimestampCount} undated` : ""}`;
  }
  if (unknownTimestampCount > 0) {
    return `${unknownTimestampCount} undated session record${unknownTimestampCount === 1 ? "" : "s"} need a manual review`;
  }
  return "No newer session records";
}

export function reviewInboxItemDetail(item: ReviewInboxItem) {
  const dated = `${item.unreviewedCount} new session record${item.unreviewedCount === 1 ? "" : "s"}`;
  const undated = item.unknownTimestampCount > 0
    ? `${item.unknownTimestampCount} undated`
    : null;
  if (!item.hasCheckpoint) {
    return `Not reviewed yet · ${item.unreviewedCount} session record${item.unreviewedCount === 1 ? "" : "s"}`;
  }
  return undated ? `${dated} · ${undated}` : dated;
}

export function overviewUnderstandProjectStep(selectedProjectId: number | null) {
  if (selectedProjectId != null) {
    return {
      enabled: true,
      label: "Understand the code",
      detail: "Read its context, then explain the files that matter.",
      help: "Open the selected project's local context before trusting or changing its code."
    };
  }
  return {
    enabled: false,
    label: "Understand the code",
    detail: "Choose a project before reading its context.",
    help: "Choose a project in the first step before reading its context."
  };
}

export function overviewRecapStep(selectedProjectId: number | null) {
  if (selectedProjectId != null) {
    return {
      label: "See what the AI did",
      detail: "Open What changed, starting since your last review.",
      help: "Reconstruct recorded edits, local Git evidence and retained history without running project code."
    };
  }
  return {
    label: "Choose a project",
    detail: "Pick one from the sidebar to inspect its recent work.",
    help: "Focus the project list so you can choose which local project to reconstruct."
  };
}

export function overviewTweakStep(selectedProjectId: number | null, mutationAvailable: boolean) {
  if (selectedProjectId == null) {
    return {
      enabled: false,
      label: mutationAvailable ? "Make one safe tweak" : "Inspect one file",
      detail: "Choose a project before opening its files.",
      help: "Choose a project before opening a file."
    };
  }
  if (mutationAvailable) {
    return {
      enabled: true,
      label: "Make one safe tweak",
      detail: "Open a file, then use Values for one reversible edit.",
      help: "Open Files. A recognised value can be changed only after a byte-minimal preview, confirmation and verified snapshot."
    };
  }
  return {
    enabled: true,
    label: "Inspect one file",
    detail: "Open its source. This edition stays read-only.",
    help: "Open Files to inspect local source. This edition cannot write project files."
  };
}

export function overviewRecapQueueSessions(sessions: SessionDiscoveryCandidate[]) {
  return [...sessions]
    .sort((left, right) => (right.modifiedMs ?? 0) - (left.modifiedMs ?? 0))
    .slice(0, 3);
}

export function overviewFlowSafetyCopy(mutationAvailable: boolean) {
  return mutationAvailable
    ? "The reconstruction stays evidence-led and local. Explain before changing; every supported tweak is one file, one value or selection, with a verified snapshot and durable undo."
    : "The reconstruction stays evidence-led and local. Read the recorded changes and source together; this build is read-only.";
}

export function overviewSafeManageStep(selectedProjectId: number | null) {
  if (selectedProjectId != null) {
    return {
      enabled: true,
      detail: "Review ownership, protected paths and shared files.",
      help: "Open the same read-only local review used by inspector and command palette entry points."
    };
  }
  return {
    enabled: false,
    detail: "Choose a project first to load a review.",
    help: "Safe Manage needs a selected project. Pick one in the sidebar or use Choose a project first."
  };
}

export function overviewInventoryHealth(dashboard: DashboardSummary | null, watcherStatus: WatcherStatus | null) {
  if (!dashboard) {
    return {
      tone: "loading" as const,
      eyebrow: "Inventory health",
      title: "Loading local map",
      detail: "Projects stay usable while Code Hangar totals the local catalog.",
      help: "Code Hangar is loading local inventory totals.",
      progress: 14,
      facts: [
        { label: "Projects", value: 0 },
        { label: "Files", value: 0 },
        { label: "Context", value: 0 }
      ]
    };
  }

  const staleRoots = Math.max(0, watcherStatus?.staleProjects ?? 0);
  const scanRoots = Math.max(0, dashboard.scanRoots);
  const partialItems = Math.max(0, dashboard.partialItems);
  const healthyRoots = Math.max(0, scanRoots - staleRoots);
  const progress = scanRoots > 0
    ? Math.max(4, Math.min(100, Math.round((healthyRoots / scanRoots) * 100)))
    : dashboard.totalProjects > 0
      ? 100
      : 0;

  if (staleRoots > 0) {
    return {
      tone: "attention" as const,
      eyebrow: "Inventory health",
      title: `${staleRoots} scan root${staleRoots === 1 ? " needs" : "s need"} attention`,
      detail: "Review scan folders or re-scan a project before trusting changed totals.",
      help: "Some registered roots changed, disappeared, or still need a scan.",
      progress,
      facts: [
        { label: "Projects", value: dashboard.totalProjects },
        { label: "Roots", value: scanRoots },
        { label: "Context", value: dashboard.contextFiles }
      ]
    };
  }

  if (partialItems > 0) {
    return {
      tone: "attention" as const,
      eyebrow: "Inventory health",
      title: "Inventory mapped with partial scans",
      detail: "The map is useful, but some folders still show lower-bound counts.",
      help: "Some scan entries are partial, so their sizes and counts are conservative lower bounds.",
      progress: 82,
      facts: [
        { label: "Projects", value: dashboard.totalProjects },
        { label: "Partial", value: partialItems },
        { label: "Context", value: dashboard.contextFiles }
      ]
    };
  }

  return {
    tone: "ready" as const,
    eyebrow: "Inventory health",
    title: "Inventory looks current",
    detail: "The local map is ready for context reading and passive discovery.",
    help: "No stale roots or partial scans are currently reported in the overview.",
    progress: 100,
    facts: [
      { label: "Projects", value: dashboard.totalProjects },
      { label: "Files", value: dashboard.totalItems },
      { label: "Context", value: dashboard.contextFiles }
    ]
  };
}

// Dashboard headline metric card. The number animates up on first paint and
// whenever it grows (e.g. after a scan) via the shared CountUp primitive, which
// also owns the reduced-motion fallback.
function Metric({ label, value, help, reduceMotion }: { label: string; value: number; help: string; reduceMotion: boolean }) {
  return (
    <div className="metric-row" data-help={help}>
      <span>{label}</span>
      <strong><CountUp value={value} reduceMotion={reduceMotion} /></strong>
    </div>
  );
}

interface FootprintSlice {
  key: string;
  name: string;
  bytes: number;
  percent: number;
  color: string;
  projectId: number | null;
  start: number;
  end: number;
}

const FOOTPRINT_COLORS = ["#11675f", "#2f5d9f", "#8b5e00", "#8a3ffc", "#c2410c", "#667085"];

function projectFootprintBytes(project: DashboardSummary["largestProjects"][number]) {
  return Math.max(0, project.physicalBytes ?? project.allocatedBytes ?? project.apparentBytes ?? 0);
}

function buildFootprintSlices(projects: DashboardSummary["largestProjects"]): FootprintSlice[] {
  const top = projects.slice(0, 5);
  const otherBytes = projects.slice(5).reduce((sum, project) => sum + projectFootprintBytes(project), 0);
  const entries = [
    ...top.map((project) => ({
      key: String(project.projectId),
      name: project.name,
      bytes: projectFootprintBytes(project),
      projectId: project.projectId
    })),
    ...(otherBytes > 0 ? [{ key: "other", name: "Other smaller projects", bytes: otherBytes, projectId: null }] : [])
  ].filter((entry) => entry.bytes > 0);
  const total = entries.reduce((sum, entry) => sum + entry.bytes, 0) || 1;
  let cursor = -90;
  return entries.map((entry, index) => {
    const degrees = (entry.bytes / total) * 360;
    const slice = {
      ...entry,
      percent: (entry.bytes / total) * 100,
      color: FOOTPRINT_COLORS[index % FOOTPRINT_COLORS.length],
      start: cursor,
      end: cursor + degrees
    };
    cursor += degrees;
    return slice;
  });
}

function FootprintDonut({
  slices,
  formatBytes,
  onOpenProject,
  onOpenOther
}: {
  slices: FootprintSlice[];
  formatBytes: (value: number) => string;
  onOpenProject: (projectId: number) => void;
  onOpenOther: () => void;
}) {
  const total = slices.reduce((sum, slice) => sum + slice.bytes, 0);
  if (slices.length === 0) {
    return <div className="footprint-donut empty">No footprint data yet.</div>;
  }
  return (
    <div className="footprint-donut" data-help="Clickable project footprint chart. Each segment opens its project; Other opens the full ranking.">
      <svg viewBox="0 0 180 180" role="img" aria-label="Largest project footprint chart">
        {slices.map((slice) => (
          <path
            key={slice.key}
            d={donutSegmentPath(90, 90, 76, 42, slice.start, slice.end)}
            fill={slice.color}
            tabIndex={0}
            role="button"
            aria-label={`${slice.name}: ${formatBytes(slice.bytes)}`}
            onClick={() => slice.projectId ? onOpenProject(slice.projectId) : onOpenOther()}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                slice.projectId ? onOpenProject(slice.projectId) : onOpenOther();
              }
            }}
          />
        ))}
      </svg>
      <div className="footprint-donut-center">
        <span>Total shown</span>
        <strong>{formatBytes(total)}</strong>
      </div>
    </div>
  );
}

function polarToCartesian(cx: number, cy: number, radius: number, angle: number) {
  const radians = (angle * Math.PI) / 180;
  return {
    x: cx + radius * Math.cos(radians),
    y: cy + radius * Math.sin(radians)
  };
}

function donutSegmentPath(cx: number, cy: number, outerRadius: number, innerRadius: number, startAngle: number, endAngle: number) {
  const safeEnd = endAngle - startAngle >= 359.99 ? startAngle + 359.99 : endAngle;
  const largeArc = safeEnd - startAngle > 180 ? 1 : 0;
  const outerStart = polarToCartesian(cx, cy, outerRadius, startAngle);
  const outerEnd = polarToCartesian(cx, cy, outerRadius, safeEnd);
  const innerStart = polarToCartesian(cx, cy, innerRadius, safeEnd);
  const innerEnd = polarToCartesian(cx, cy, innerRadius, startAngle);
  return [
    `M ${outerStart.x} ${outerStart.y}`,
    `A ${outerRadius} ${outerRadius} 0 ${largeArc} 1 ${outerEnd.x} ${outerEnd.y}`,
    `L ${innerStart.x} ${innerStart.y}`,
    `A ${innerRadius} ${innerRadius} 0 ${largeArc} 0 ${innerEnd.x} ${innerEnd.y}`,
    "Z"
  ].join(" ");
}

function detectorLabel(adapter: AdapterSummary) {
  if (adapter.name === "generic_git_project") return "Git project detector";
  if (adapter.name === "generic_markdown_context") return "Markdown context detector";
  if (adapter.name === "generic_model_workflow_assets") return "Model and workflow asset detector";
  return adapter.name.replace(/^generic_/, "").replaceAll("_", " ");
}

function detectorDescription(adapter: AdapterSummary) {
  if (adapter.name === "generic_git_project") return "Recognises local repository metadata already present on disk.";
  if (adapter.name === "generic_markdown_context") return "Finds README, AGENTS and documentation files that explain a project.";
  if (adapter.name === "generic_model_workflow_assets") return "Labels local model, workflow, dataset and asset-like files for review.";
  return adapter.description;
}
