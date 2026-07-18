import { lazy, Suspense, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { AlertTriangle, Boxes, ChevronRight, Database, FileText, Files, FolderSearch, GitFork, Lock, MessageSquare, Pause, Play, RefreshCcw, Search, X } from "lucide-react";
import type {
  ContextFile,
  DashboardSummary,
  FilePreview,
  GraphMap,
  GraphMapExpansionState,
  GraphIssue,
  GraphNode,
  NodeRelationships,
  ProjectContextSummary,
  ProjectScanState,
  ProjectSummary,
  SessionDiscoveryCandidate
} from "../types";
import { api } from "../api";
import { displayAppText, sessionAppMeta } from "../app-meta";
import { ConceptHelp } from "../BeginnerHelp";
import { graphMapItemCounts } from "../graphMapExpansion";
import { formatOptionalBytes, formatTimestamp, plainConfidenceLabel, textMentionsDependencyCache } from "../ui";
import { CommentsPanel } from "./CommentsPanel";

const connectorFrontendBuild = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector";
const EmptyConnectorSummary = () => null;
const ProjectAiSummary = connectorFrontendBuild
  ? lazy(() => import("./ProjectAiSummary").then((module) => ({ default: module.ProjectAiSummary })))
  : EmptyConnectorSummary;

export interface ProjectScanAction {
  kind: "scan" | "folders";
  label: string;
  help: string;
  disabled: boolean;
  onSelect: () => void;
}

export interface FileContextMenuTarget {
  nodeId: number;
  projectId?: number | null;
  path: string;
  label: string;
  itemKind?: string;
}

export type FileContextMenuHandler = (target: FileContextMenuTarget, event: MouseEvent<HTMLElement>) => void;
export type SessionContextMenuHandler = (session: SessionDiscoveryCandidate, event: MouseEvent<HTMLElement>) => void;

export const LARGE_LIST_PAGE_SIZE = 40;

export function nextProgressiveListLimit(current: number, total: number, pageSize = LARGE_LIST_PAGE_SIZE) {
  return Math.min(total, Math.max(pageSize, current + pageSize));
}

function ProgressiveListControls({
  shown,
  total,
  noun,
  onMore,
  onAll
}: {
  shown: number;
  total: number;
  noun: string;
  onMore: () => void;
  onAll: () => void;
}) {
  if (shown >= total) return null;
  return (
    <div className="progressive-list-controls" aria-label={`${shown} of ${total} ${noun} shown`}>
      <span>{shown} of {total} shown</span>
      <button type="button" className="secondary-button compact" onClick={onMore}>Load more</button>
      <button type="button" className="secondary-button compact" onClick={onAll}>Show all</button>
    </div>
  );
}

export interface ProjectAccountingState {
  label: string;
  tone: "attention" | "ready";
  help: string;
}

export function projectAccountingState(
  hasFootprint: boolean,
  footprintPartial: boolean,
  scanState: ProjectScanState | null
): ProjectAccountingState {
  if (!hasFootprint) {
    return {
      label: "Awaiting totals",
      tone: "attention",
      help: "No completed footprint totals are available for this project yet."
    };
  }
  if (footprintPartial) {
    return {
      label: "Incomplete count",
      tone: "attention",
      help: "Part of the inventoried project tree is still missing from these footprint totals."
    };
  }
  if (scanState && scanState !== "scanned") {
    return {
      label: "Last scan complete",
      tone: "attention",
      help: "The previous scan completed, but this project now needs a fresh scan before these totals can be treated as current."
    };
  }
  return {
    label: "Complete known scan",
    tone: "ready",
    help: "The latest known project scan completed and its footprint totals are available."
  };
}

/** Local, no-network "what this project does" card from README + manifests. */
function ProjectSummaryCard({ projectId, connectorBuild }: { projectId: number; connectorBuild: boolean }) {
  const [summary, setSummary] = useState<ProjectContextSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setSummary(null);
    setError(null);
    void api.projectContextSummary(projectId)
      .then((result) => {
        if (!cancelled) setSummary(result);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);
  if (error) {
    return (
      <section className="project-home-section project-summary-card">
        <p className="muted">Project summary unavailable: {error}</p>
      </section>
    );
  }
  if (!summary) return null;
  const hasContent =
    summary.kinds.length > 0 ||
    !!summary.readmeTitle ||
    !!summary.readmeExcerpt ||
    summary.runCommands.length > 0;
  if (!hasContent) return null;
  return (
    <section className="project-home-section project-summary-card" data-help="A quick, fully local read of what this project is — from its README and manifest files. No AI, no network.">
      <h3>{summary.readmeTitle ?? "About this project"}</h3>
      {summary.kinds.length > 0 ? (
        <div className="project-summary-kinds">
          {summary.kinds.map((kind) => (
            <span key={kind} className="chip">{kind}</span>
          ))}
        </div>
      ) : null}
      {summary.readmeExcerpt ? <p className="muted">{summary.readmeExcerpt}</p> : null}
      {summary.runCommands.length > 0 ? (
        <p className="muted">
          <strong>Run:</strong>{" "}
          {summary.runCommands.map((command, index) => (
            <span key={command}>
              {index > 0 ? " · " : ""}
              <code>{command}</code>
            </span>
          ))}
        </p>
      ) : null}
      {connectorBuild ? (
        <Suspense fallback={null}>
          <ProjectAiSummary projectId={projectId} />
        </Suspense>
      ) : null}
    </section>
  );
}

export function ProjectContextHome({
  project,
  files,
  scanState,
  scanAction,
  overlapWarning,
  onOpen,
  onContextMenu,
  onOpenFiles,
  connectorBuild = false
}: {
  project: ProjectSummary | null;
  files: ContextFile[];
  scanState: ProjectScanState | null;
  scanAction: ProjectScanAction | null;
  overlapWarning: string | null;
  onOpen: (nodeId: number) => void;
  onContextMenu: FileContextMenuHandler;
  onOpenFiles: () => void;
  /** True only in the AI Connector edition; gates the optional AI summary and comments hint. */
  connectorBuild?: boolean;
}) {
  const [showAllContext, setShowAllContext] = useState(false);
  const recommendedFiles = useMemo(() => files.filter((file) => file.recommended), [files]);
  const displayedFiles = showAllContext
    ? files
    : recommendedFiles.length
      ? recommendedFiles.slice(0, 16)
      : files.slice(0, 16);
  const hiddenLoadedCount = Math.max(0, files.length - displayedFiles.length);
  const showScanAction = Boolean(project && scanAction && scanState !== "scanned");
  if (!project) {
    return (
      <div className="project-home">
        <div className="project-choice-empty">
          <span className="project-choice-eyebrow">Start here</span>
          <h2>Choose a project</h2>
          <p>Pick a project from the left sidebar, or use Quick Open, to see its recorded AI work, understand the code and inspect one safe next step.</p>
          <div className="project-choice-steps">
            <span><strong>1</strong> Select a project</span>
            <span><strong>2</strong> See what the AI did</span>
            <span><strong>3</strong> Understand before changing</span>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="project-home">
      <div className="project-home-intro">
        <span>Start here</span>
        <h2>{project.name}</h2>
        <p>Read the files most likely to explain this project first. The full file inventory stays one click away under Files.</p>
      </div>
      {showScanAction && scanAction ? (
        <div className="project-scan-next-step" data-help={scanAction.help}>
          <div>
            <strong>{scanState === "scanning" ? "Scan running" : "Needs a fresh scan"}</strong>
            <span>{scanAction.kind === "scan" ? "Refresh the local inventory for this project before relying on context or size totals." : "This project needs scan attention, but Code Hangar cannot resolve its scan root yet."}</span>
          </div>
          <button type="button" onClick={scanAction.onSelect} disabled={scanAction.disabled}>
            {scanAction.kind === "scan" ? <RefreshCcw size={15} /> : <FolderSearch size={15} />}
            {scanAction.label}
          </button>
        </div>
      ) : null}
      <ProjectSummaryCard key={project.id} projectId={project.id} connectorBuild={connectorBuild} />
      {overlapWarning ? (
        <div className="project-home-warning" data-help="This project path overlaps another registered project or scan root. Code Hangar keeps both read-only, but counts and context may be duplicated until you unregister one of the roots.">
          {overlapWarning}
        </div>
      ) : null}
      <section className="project-home-section">
        <div className="project-home-heading">
          <div>
            <div className="heading-with-help">
              <h3>{showAllContext ? "All loaded context files" : "Recommended context"}</h3>
              <ConceptHelp concept="context" />
            </div>
            <p>{recommendedFiles.length || files.length} recommended of {project?.contextCount ?? files.length} local context file{(project?.contextCount ?? files.length) === 1 ? "" : "s"}. Protected content stays blocked by default.</p>
          </div>
          {files.length > displayedFiles.length || showAllContext ? (
            <button
              type="button"
              className="secondary-button"
              data-help={showAllContext ? "Return to the short recommended context list." : "Show lower-priority loaded context files, including nested READMEs, without changing the full file inventory."}
              onClick={() => setShowAllContext((current) => !current)}
            >
              {showAllContext ? "Show recommended" : `Show all loaded (${files.length})`}
            </button>
          ) : null}
        </div>
        {displayedFiles.length ? (
          <div className="project-context-grid">
            {displayedFiles.map((file) => {
              const pathLabel = contextPathLabel(file, project);
              return (
                <button type="button" key={file.nodeId} onClick={() => onOpen(file.nodeId)} onContextMenu={(event) => onContextMenu({ nodeId: file.nodeId, projectId: file.projectId, path: file.path, label: file.displayName, itemKind: "file" }, event)} data-help={`Open ${file.displayName}. ${file.recommendationReason} Right-click for File Explorer, Safe Manage and more tools. Path: ${pathLabel}.`}>
                  <FileText size={16} />
                  <span>
                    <strong>{file.displayName}</strong>
                    <small>{file.contextGroup} · {pathLabel}</small>
                  </span>
                  {file.protectedLevel || file.isSensitive ? <Lock size={13} /> : <ChevronRight size={14} />}
                </button>
              );
            })}
          </div>
        ) : (
          <div className="project-context-empty">
            <FileText size={20} />
            <strong>No priority context files yet</strong>
            <span>{showScanAction ? "Code Hangar needs a fresh scan before this context list is reliable. You can still browse the current inventory." : "Code Hangar did not find a README, AGENTS file or loaded docs for this project. The inventory may still contain useful files."}</span>
            <button type="button" className="secondary-button" onClick={onOpenFiles} data-help="Open the full file tree for this project.">
              <Files size={15} />
              Browse files
            </button>
          </div>
        )}
        {!showAllContext && hiddenLoadedCount > 0 ? (
          <p className="project-home-note" data-help="The hidden files are still available. They are mostly lower-priority nested READMEs or secondary context files.">
            {hiddenLoadedCount} lower-priority loaded context file{hiddenLoadedCount === 1 ? "" : "s"} hidden from the recommended list.
          </p>
        ) : null}
      </section>
      {files.length === 0 ? (
        <div className="project-next-step" data-help="No context files are loaded for this project yet. Browse the file inventory first; Safe Manage is still available from the sidebar or top project button.">
          <div>
            <strong>Next: Browse files</strong>
            <span>Look through the local inventory before reviewing impact. This keeps the read-first workflow intact.</span>
          </div>
          <button type="button" onClick={onOpenFiles}>
            <Files size={15} />
            Browse files
          </button>
        </div>
      ) : (
        <div className="project-next-step" data-help="After reading the context, open one relevant file. Source and Values keep the next step bounded to one file; protected content remains blocked.">
          <div>
            <strong>Next: Understand one file</strong>
            <span>Open the full tree, choose the file that matters, and inspect its source before considering any small correction.</span>
          </div>
          <button type="button" onClick={onOpenFiles}>
            <Files size={15} />
            Browse files
          </button>
        </div>
      )}
      <CommentsPanel nodeId={project?.id ?? null} connectorBuild={connectorBuild} />
    </div>
  );
}

function contextPathLabel(file: ContextFile, project: ProjectSummary | null) {
  const normalizedPath = normalizeDisplayPath(file.path);
  const normalizedRoot = project ? normalizeDisplayPath(project.path) : "";
  const lowerPath = normalizedPath.toLocaleLowerCase();
  const lowerRoot = normalizedRoot.toLocaleLowerCase();
  let label = normalizedPath;
  if (normalizedRoot && lowerPath.startsWith(`${lowerRoot}/`)) {
    label = normalizedPath.slice(normalizedRoot.length + 1);
  } else if (normalizedPath.startsWith("fixture://")) {
    label = normalizedPath.replace(/^fixture:\/\/[^/]+\//, "");
  }
  return label === file.displayName ? `./${label}` : label;
}

function normalizeDisplayPath(path: string) {
  return path.replace(/\\/g, "/").replace(/^\/\/\?\/UNC\//, "//").replace(/^\/\/\?\//, "");
}

export function ProjectSpaceHome({
  project,
  footprint,
  scanState,
  scanAction,
  formatOptionalBytes,
  projectStateLabel
}: {
  project: ProjectSummary | null;
  footprint: DashboardSummary["largestProjects"][number] | null;
  scanState: ProjectScanState | null;
  scanAction: ProjectScanAction | null;
  formatOptionalBytes: (value?: number | null) => string;
  projectStateLabel: (state: ProjectScanState) => string;
}) {
  const physicalBytes = footprint?.physicalBytes ?? footprint?.allocatedBytes ?? null;
  const apparentBytes = footprint?.apparentBytes ?? null;
  const physicalShare = physicalBytes != null && apparentBytes != null && apparentBytes > 0
    ? Math.min(100, Math.max(2, Math.round((physicalBytes / apparentBytes) * 100)))
    : 0;
  const inventoryLabel = scanState ? projectStateLabel(scanState) : "Unknown";
  const accounting = projectAccountingState(Boolean(footprint), Boolean(footprint?.footprintPartial), scanState);
  return (
    <div className="project-home">
      <div className="project-home-intro">
        <span>Space on disk</span>
        <div className="heading-with-help">
          <h2>{project?.name ?? "Project"} space</h2>
          <ConceptHelp concept="space" />
        </div>
        <p>Space used on disk avoids counting known shared copies twice. Total file sizes adds every file length. Incomplete scans show minimum values until scanning finishes.</p>
      </div>
      <div className="project-space-card" data-help="Physical space is the conservative local footprint. Apparent size is the sum of file lengths before hardlink accounting.">
        <div className="project-space-primary">
          <span>Space used on disk</span>
          <strong>{formatOptionalBytes(physicalBytes)}</strong>
          <small>{physicalShare > 0 ? `${physicalShare}% of the total file sizes` : "Awaiting scan totals"}</small>
        </div>
        <div className="project-space-meter" aria-hidden="true">
          <span style={{ width: `${physicalShare}%` }} />
        </div>
        <div className="project-space-facts">
          <span><strong>{formatOptionalBytes(apparentBytes)}</strong><small>Total file sizes</small></span>
          <span><strong>{inventoryLabel}</strong><small>Inventory state</small></span>
          <span className={accounting.tone} data-help={accounting.help}><strong>{accounting.label}</strong><small>Accounting</small></span>
        </div>
        {scanAction && scanState !== "scanned" ? (
          <div className="project-space-actions">
            <button type="button" className="secondary-button compact" disabled={scanAction.disabled} onClick={scanAction.onSelect} data-help={scanAction.help}>
              {scanAction.kind === "scan" ? <RefreshCcw size={15} /> : <FolderSearch size={15} />}
              {scanAction.label}
            </button>
          </div>
        ) : null}
      </div>
      <p className="project-home-note">Space accounting is descriptive. It does not decide what is safe to remove.</p>
    </div>
  );
}

export function ProjectConnectionsHome({
  preview,
  relationships,
  relationshipsNodeId,
  relationshipsLoading,
  graphMap,
  graphMapLoading,
  graphMapError,
  graphMapExpansion,
  onExpandGraphMap,
  onPauseGraphMap,
  onContinueGraphMap,
  onOpen,
  onContextMenu
}: {
  preview: FilePreview | null;
  relationships: NodeRelationships | null;
  relationshipsNodeId: number | null;
  relationshipsLoading: boolean;
  graphMap: GraphMap | null;
  graphMapLoading: boolean;
  graphMapError: string | null;
  graphMapExpansion: GraphMapExpansionState;
  onExpandGraphMap: () => void;
  onPauseGraphMap: () => void;
  onContinueGraphMap: () => void;
  onOpen: (nodeId: number) => void;
  onContextMenu: FileContextMenuHandler;
}) {
  const [mapFilter, setMapFilter] = useState<MapFilter>("all");
  const [directNodeLimit, setDirectNodeLimit] = useState(LARGE_LIST_PAGE_SIZE);
  const [cacheNodeLimit, setCacheNodeLimit] = useState(LARGE_LIST_PAGE_SIZE);
  const [issueLimit, setIssueLimit] = useState(LARGE_LIST_PAGE_SIZE);
  const [cacheIssueLimit, setCacheIssueLimit] = useState(LARGE_LIST_PAGE_SIZE);
  const graphNodes = graphMap?.nodes.filter((node) => node.graphKind !== "project") ?? [];
  const workflows = graphNodes.filter((node) => node.graphKind === "workflow");
  const models = graphNodes.filter((node) => node.graphKind.startsWith("model:"));
  const caches = graphNodes.filter((node) => node.graphKind === "cache");
  const groupedAllNodes = groupGraphNodesForDisplay(graphNodes);
  const dependencyCacheNodeIds = new Set(groupedAllNodes.cacheNodes.map((node) => node.nodeId));
  const directWorkflows = workflows.filter((node) => !dependencyCacheNodeIds.has(node.nodeId));
  const cachedWorkflows = workflows.filter((node) => dependencyCacheNodeIds.has(node.nodeId));
  const directModels = models.filter((node) => !dependencyCacheNodeIds.has(node.nodeId));
  const cachedModels = models.filter((node) => dependencyCacheNodeIds.has(node.nodeId));
  const modelUses = graphMap?.edges.filter((edge) => edge.kind === "workflow_references_model").length ?? 0;
  // A model is unreferenced within this bounded local map when no mapped workflow points
  // to it. That is a review signal, not proof that another tool or manual command does not use it.
  const referencedModelIds = new Set(
    (graphMap?.edges ?? [])
      .filter((edge) => edge.kind === "workflow_references_model")
      .map((edge) => edge.targetNodeId),
  );
  const orphanModels = directModels.filter((node) => !referencedModelIds.has(node.nodeId));
  const reclaimableOrphanBytes = orphanModels
    .filter((node) => node.sharedProjectIds.length <= 1 && !node.protectedOrSensitive)
    .reduce((sum, node) => sum + (node.physicalBytes ?? 0), 0);
  // "Assets" generalizes the orphans view: every model + cache by physical size (largest first),
  // with the SAME reclaimable rule (shared/protected excluded — Gate 2, they may be owned
  // elsewhere). "Risk" gathers the assets to review before any reclaim: shared across projects or
  // protected/sensitive (the reference issues are shown in their own section below).
  const assetNodes = [...models, ...caches].sort(
    (a, b) => (b.physicalBytes ?? 0) - (a.physicalBytes ?? 0),
  );
  const reclaimableAssetBytes = assetNodes
    .filter((node) => node.sharedProjectIds.length <= 1 && !node.protectedOrSensitive)
    .reduce((sum, node) => sum + (node.physicalBytes ?? 0), 0);
  const riskNodes = graphNodes.filter(
    (node) => node.sharedProjectIds.length > 1 || node.protectedOrSensitive,
  );
  const graphNodeById = new Map(graphNodes.map((node) => [node.nodeId, node]));
  const groupedWorkflows = groupGraphNodesForDisplay(workflows);
  const groupedIssues = groupGraphIssuesForDisplay(graphMap?.issues ?? [], graphNodeById);
  const shownIssues = groupedIssues.issues.slice(0, issueLimit);
  const shownCacheIssues = groupedIssues.cacheIssues.slice(0, cacheIssueLimit);
  const loadedMapItems = graphMap ? graphMapItemCounts(graphMap).loadedItems : 0;
  const totalMapItems = graphMap ? graphMapItemCounts(graphMap).totalItems : 0;
  const mapIsTruncated = loadedMapItems < totalMapItems;
  const mapExpansionPercent = totalMapItems > 0
    ? Math.min(100, Math.round((graphMapExpansion.loadedItems / totalMapItems) * 100))
    : 100;
  const graphCounts = useMemo<GraphOverviewCounts>(
    () => ({
      mapped: totalMapItems,
      workflows: directWorkflows.length,
      cachedWorkflows: cachedWorkflows.length,
      models: directModels.length,
      cachedModels: cachedModels.length,
      caches: groupedAllNodes.cacheNodes.length,
      modelUses,
      unreferenced: orphanModels.length,
      risk: riskNodes.length,
      issues: groupedIssues.issues.length,
      cacheIssues: groupedIssues.cacheIssues.length
    }),
    [
      cachedModels.length,
      cachedWorkflows.length,
      directModels.length,
      directWorkflows.length,
      groupedAllNodes.cacheNodes.length,
      groupedIssues.cacheIssues.length,
      groupedIssues.issues.length,
      modelUses,
      orphanModels.length,
      riskNodes.length,
      totalMapItems,
    ]
  );
  const mapFilters = useMemo(() => graphFiltersForCounts(graphCounts), [graphCounts]);
  const summaryItems = useMemo(() => graphSummaryItems(graphCounts), [graphCounts]);
  useEffect(() => {
    if (!mapFilters.includes(mapFilter)) setMapFilter("all");
  }, [mapFilter, mapFilters]);
  const visibleGraphNodes = mapFilter === "workflows"
    ? workflows
    : mapFilter === "models"
      ? models
      : mapFilter === "caches"
        ? groupedAllNodes.cacheNodes
        : mapFilter === "assets"
          ? assetNodes
          : mapFilter === "orphans"
            ? orphanModels
            : mapFilter === "risk"
              ? riskNodes
              : mapFilter === "issues"
                ? []
                : graphNodes;
  const { nodes: directGraphNodes, cacheNodes } = groupGraphNodesForDisplay(visibleGraphNodes);
  const shownDirectGraphNodes = directGraphNodes.slice(0, directNodeLimit);
  const shownCacheNodes = cacheNodes.slice(0, cacheNodeLimit);
  const relationshipsReady = preview !== null && relationshipsNodeId === preview.nodeId && relationships !== null;
  const outgoing = relationshipsReady ? relationships.outgoing : [];
  const incoming = relationshipsReady ? relationships.incoming : [];
  const issues = relationshipsReady ? relationships.issues : [];
  useEffect(() => {
    setDirectNodeLimit(LARGE_LIST_PAGE_SIZE);
    setCacheNodeLimit(LARGE_LIST_PAGE_SIZE);
    setIssueLimit(LARGE_LIST_PAGE_SIZE);
    setCacheIssueLimit(LARGE_LIST_PAGE_SIZE);
  }, [graphMap?.projectId, mapFilter]);
  const mapHelpConcept = mapFilter === "models" || mapFilter === "orphans"
    ? "models"
    : mapFilter === "workflows"
      ? "workflows"
      : mapFilter === "caches"
        ? "caches"
        : "connections";
  return (
    <div className="project-home">
      <div className="project-home-intro">
        <span>Local dependency map</span>
        <div className="heading-with-help">
          <h2>Hangar Map</h2>
          <ConceptHelp concept="connections" />
        </div>
        <p>See which local workflows use which models, where caches sit, and which links need review. Code Hangar builds this map by carefully reading local file information and supported workflow files; no remote service is contacted.</p>
      </div>
      <div className="connection-summary">
        {summaryItems.map((item) => (
          <span key={item.key}>
            <strong>{item.count}</strong> {item.label}
            {item.key === "mapped" && mapIsTruncated ? <small>{loadedMapItems} loaded now · complete total known locally</small> : null}
            {item.key === "workflows" && cachedWorkflows.length ? <small>{cachedWorkflows.length} more inside dependency caches</small> : null}
            {item.key === "models" && cachedModels.length ? <small>{cachedModels.length} more inside dependency caches</small> : null}
          </span>
        ))}
      </div>
      {graphMap?.partial ? <div className="project-home-warning graph-map-warning" data-help="This project contains an incomplete scan, so the dependency map is a lower-bound and may be missing workflows or models."><AlertTriangle size={15} />Map incomplete. Continue the project scan before relying on totals.</div> : null}
      {graphMap && (mapIsTruncated || graphMapExpansion.status !== "idle") ? (
        <section className={`graph-map-expansion ${graphMapExpansion.status === "error" ? "error" : ""}`} aria-live="polite">
          <div className="graph-map-expansion-copy">
            <AlertTriangle size={16} aria-hidden="true" />
            <span>
              <strong>{graphMapExpansion.status === "complete" ? "Complete map loaded" : `${loadedMapItems} of ${totalMapItems} mapped items loaded`}</strong>
              <small>{graphMapExpansion.message ?? "The initial map is intentionally bounded. Load the remainder in local batches when you need the complete inventory."}</small>
            </span>
          </div>
          <div className="graph-map-expansion-progress" aria-label={`${mapExpansionPercent}% of the complete Hangar Map loaded`}>
            <span style={{ width: `${mapExpansionPercent}%` }} />
          </div>
          <div className="graph-map-expansion-actions">
            {graphMapExpansion.status === "loading" || graphMapExpansion.status === "pausing" ? (
              <button type="button" className="secondary-button compact" disabled={graphMapExpansion.status === "pausing"} onClick={onPauseGraphMap} data-help="Pause after the current local batch finishes. Already loaded results remain available.">
                <Pause size={14} aria-hidden="true" />{graphMapExpansion.status === "pausing" ? "Pausing..." : "Pause"}
              </button>
            ) : graphMapExpansion.status === "paused" ? (
              <button type="button" className="secondary-button compact" onClick={onContinueGraphMap} data-help="Continue loading from the next local batch. Code Hangar does not restart from zero.">
                <Play size={14} aria-hidden="true" />Continue
              </button>
            ) : mapIsTruncated ? (
              <button type="button" className="secondary-button compact" onClick={onExpandGraphMap} data-help="Load every remaining mapped item from the local inventory in controlled batches. A warning explains the temporary resource cost first.">
                <Play size={14} aria-hidden="true" />{graphMapExpansion.status === "error" ? "Retry complete map" : "Load complete map"}
              </button>
            ) : null}
          </div>
        </section>
      ) : null}
      <div className="graph-map-filters" role="tablist" aria-label="Hangar Map view">
        {mapFilters.map((filter) => (
          <button
            key={filter}
            type="button"
            className={`segmented ${mapFilter === filter ? "active" : ""}`}
            onClick={() => setMapFilter(filter)}
            role="tab"
            aria-selected={mapFilter === filter}
            data-help={graphFilterHelp(filter)}
          >
            {graphFilterLabel(filter)}
          </button>
        ))}
      </div>
      {graphMapLoading ? (
        <section className="project-home-section" aria-busy="true">
          <div className="connection-group-head">
            <h3>Building the local dependency map</h3>
          </div>
          <div className="connection-list connection-list-skeleton" aria-hidden="true">
            <span className="skeleton connection-skeleton-row" />
            <span className="skeleton connection-skeleton-row" />
            <span className="skeleton connection-skeleton-row" />
          </div>
        </section>
      ) : null}
      {graphMapError ? <div className="graph-map-error" role="alert">Could not load the Hangar Map: {graphMapError}</div> : null}
      {!graphMapLoading && !graphMapError && graphMap && mapFilter !== "issues" ? (
        <section className="project-home-section">
          <div className="connection-group-head">
            <div className="heading-with-help"><h3>{graphFilterLabel(mapFilter)}</h3><ConceptHelp concept={mapHelpConcept} /></div>
            {visibleGraphNodes.length ? <span className="relationship-count-pill">{visibleGraphNodes.length}</span> : null}
          </div>
          {mapFilter === "orphans" && orphanModels.length ? (
            <p className="project-home-note" data-help="A model is unreferenced in this bounded local map when no mapped workflow points to it. That is a review signal, not proof that another tool does not use it. Shared or protected models are excluded from the reclaimable total.">
              No local workflow points to these models. Up to <strong>{formatOptionalBytes(reclaimableOrphanBytes)}</strong> could potentially be freed after review. Shared and protected models are excluded from that total.
            </p>
          ) : null}
          {mapFilter === "assets" && assetNodes.length ? (
            <p className="project-home-note" data-help="Every mapped model and cache, largest first. The reclaimable subtotal excludes shared and protected assets, which may be owned elsewhere (Gate 2).">
              Largest assets first. Up to <strong>{formatOptionalBytes(reclaimableAssetBytes)}</strong> could be reclaimable after review — shared and protected assets are excluded from that total.
            </p>
          ) : null}
          {mapFilter === "risk" && riskNodes.length ? (
            <p className="project-home-note" data-help="Assets shared across projects or protected/sensitive — review before treating them as recoverable. Any unresolved workflow references are listed below.">
              Shared or protected — review before reclaiming. These are excluded from reclaimable totals; reference issues needing review are listed below.
            </p>
          ) : null}
          {visibleGraphNodes.length ? (
            <div className="connection-list graph-node-list">
              {shownDirectGraphNodes.map((node) => (
                <button type="button" key={`${node.nodeId}-${node.graphKind}`} onClick={() => onOpen(node.nodeId)} onContextMenu={(event) => onContextMenu({ nodeId: node.nodeId, projectId: node.projectId, path: node.path, label: node.displayName, itemKind: node.itemKind }, event)} data-help={`${graphNodeHelp(node)} Right-click for File Explorer, Safe Manage and more tools.`}>
                  {graphNodeIcon(node)}
                  <span><strong>{node.displayName}</strong><small>{node.path}</small></span>
                  <small>{graphNodeSummary(node)}</small>
                </button>
              ))}
              <ProgressiveListControls
                shown={shownDirectGraphNodes.length}
                total={directGraphNodes.length}
                noun="mapped items"
                onMore={() => setDirectNodeLimit((current) => nextProgressiveListLimit(current, directGraphNodes.length))}
                onAll={() => setDirectNodeLimit(directGraphNodes.length)}
              />
              {cacheNodes.length ? (
                <details className="graph-cache-group">
                  <summary>Inside dependency caches ({cacheNodes.length})</summary>
                  <div className="connection-list">
                    {shownCacheNodes.map((node) => (
                      <button type="button" key={`${node.nodeId}-${node.graphKind}`} onClick={() => onOpen(node.nodeId)} onContextMenu={(event) => onContextMenu({ nodeId: node.nodeId, projectId: node.projectId, path: node.path, label: node.displayName, itemKind: node.itemKind }, event)} data-help={`${graphNodeHelp(node)} Right-click for File Explorer, Safe Manage and more tools.`}>
                        {graphNodeIcon(node)}
                        <span><strong>{node.displayName}</strong><small>{node.path}</small></span>
                        <small>{graphNodeSummary(node)}</small>
                      </button>
                    ))}
                  </div>
                  <ProgressiveListControls
                    shown={shownCacheNodes.length}
                    total={cacheNodes.length}
                    noun="cache entries"
                    onMore={() => setCacheNodeLimit((current) => nextProgressiveListLimit(current, cacheNodes.length))}
                    onAll={() => setCacheNodeLimit(cacheNodes.length)}
                  />
                </details>
              ) : null}
            </div>
          ) : <p className="muted result-empty">{graphEmptyMessage(mapFilter)}</p>}
          {cacheNodes.length ? <p className="muted result-empty">{cacheNodes.length} of {visibleGraphNodes.length} mapped items live inside dependency caches — grouped for readability, map inputs unchanged.</p> : null}
        </section>
      ) : null}
      {!graphMapLoading && graphMap && (mapFilter === "all" || mapFilter === "issues" || mapFilter === "risk") ? (
        <section className="project-home-section">
          <div className="connection-group-head">
            <div className="heading-with-help"><h3>References needing review</h3><ConceptHelp concept="references" /></div>
            {groupedIssues.issues.length ? <span className="relationship-count-pill warn">{groupedIssues.issues.length} direct</span> : null}
          </div>
          {graphMap.issues.length ? (
            <div className="connection-list graph-issue-list">
              {shownIssues.map((issue, index) => (
                <button type="button" key={`${issue.nodeId}-${issue.kind}-${issue.target}-${index}`} onClick={() => onOpen(issue.nodeId)} onContextMenu={(event) => {
                  const node = graphNodeById.get(issue.nodeId);
                  onContextMenu({ nodeId: issue.nodeId, projectId: issue.projectId ?? node?.projectId, path: graphIssueContextPath(issue, node), label: node?.displayName ?? issue.target, itemKind: node?.itemKind ?? "file" }, event);
                }} data-help={`${graphIssueActionHelp(issue.kind, issue.target)} ${graphIssueHelp(issue.kind)} Right-click for more tools.`}>
                  <AlertTriangle size={15} />
                  <span><strong>{graphIssueLabel(issue.kind)}</strong><small>{issue.target}</small></span>
                  <small>{plainConfidenceLabel(issue.confidence, "signal")}</small>
                </button>
              ))}
              <ProgressiveListControls
                shown={shownIssues.length}
                total={groupedIssues.issues.length}
                noun="references"
                onMore={() => setIssueLimit((current) => nextProgressiveListLimit(current, groupedIssues.issues.length))}
                onAll={() => setIssueLimit(groupedIssues.issues.length)}
              />
              {groupedIssues.cacheIssues.length ? (
                <details className="graph-cache-group">
                  <summary>References from dependency caches ({groupedIssues.cacheIssues.length})</summary>
                  <div className="connection-list">
                    {shownCacheIssues.map((issue, index) => (
                      <button type="button" key={`${issue.nodeId}-${issue.kind}-${issue.target}-cache-${index}`} onClick={() => onOpen(issue.nodeId)} onContextMenu={(event) => {
                        const node = graphNodeById.get(issue.nodeId);
                        onContextMenu({ nodeId: issue.nodeId, projectId: issue.projectId ?? node?.projectId, path: graphIssueContextPath(issue, node), label: node?.displayName ?? issue.target, itemKind: node?.itemKind ?? "file" }, event);
                      }} data-help={`${graphIssueActionHelp(issue.kind, issue.target)} ${graphIssueHelp(issue.kind)} Right-click for more tools.`}>
                        <AlertTriangle size={15} />
                        <span><strong>{graphIssueLabel(issue.kind)}</strong><small>{issue.target}</small></span>
                        <small>{plainConfidenceLabel(issue.confidence, "signal")}</small>
                      </button>
                    ))}
                  </div>
                  <ProgressiveListControls
                    shown={shownCacheIssues.length}
                    total={groupedIssues.cacheIssues.length}
                    noun="cache references"
                    onMore={() => setCacheIssueLimit((current) => nextProgressiveListLimit(current, groupedIssues.cacheIssues.length))}
                    onAll={() => setCacheIssueLimit(groupedIssues.cacheIssues.length)}
                  />
                </details>
              ) : null}
            </div>
          ) : <p className="plan-clean-note"><span className="plan-clean-check" aria-hidden="true">✓</span>No missing, ambiguous or invalid workflow references were found.</p>}
          {groupedIssues.cacheIssues.length ? <p className="muted result-empty">{groupedIssues.cacheIssues.length} of {graphMap.issues.length} references come from dependency caches — grouped for readability, map inputs unchanged.</p> : null}
        </section>
      ) : null}
      {preview ? <section className="project-home-section file-connections-section">
        <div className="connection-group-head">
          <div className="heading-with-help"><h3>Open file connections</h3><ConceptHelp concept="references" /></div>
          {[...outgoing, ...incoming].length ? <span className="relationship-count-pill">{outgoing.length + incoming.length}</span> : null}
        </div>
        <p className="project-home-note"><strong>{preview.displayName}</strong> · {outgoing.length} outgoing · {incoming.length} incoming · {issues.length} need review</p>
      {relationshipsLoading ? (
        <div className="connection-list connection-list-skeleton" aria-hidden="true" data-help="Connections are loading in the background so the file preview can open first.">
          <span className="skeleton connection-skeleton-row" />
          <span className="skeleton connection-skeleton-row" />
        </div>
      ) : null}
        {[...outgoing, ...incoming].length ? (
          <div className="connection-list">
            {[...outgoing, ...incoming].map((relationship, index) => (
              <button type="button" key={`${relationship.nodeId}-${relationship.path}-${index}`} onClick={() => onOpen(relationship.nodeId)} onContextMenu={(event) => onContextMenu({ nodeId: relationship.nodeId, projectId: relationship.projectId, path: relationship.path, label: relationship.displayName, itemKind: relationship.itemKind }, event)} data-help={`Open connected file ${relationship.displayName}. Right-click for File Explorer, Safe Manage and more tools.`}>
                <FileText size={15} />
                <span><strong>{relationship.displayName}</strong><small>{relationship.path}</small></span>
                <small>{plainConfidenceLabel(relationship.confidence, "link")}</small>
              </button>
            ))}
          </div>
        ) : !relationshipsLoading && relationshipsReady ? <p className="plan-clean-note"><span className="plan-clean-check" aria-hidden="true">✓</span>No known local file connections.</p> : null}
      </section> : <p className="project-home-note">Open a file to inspect its direct Markdown or workflow relationships below the project map.</p>}
    </div>
  );
}

export type MapFilter = "all" | "workflows" | "models" | "caches" | "assets" | "orphans" | "risk" | "issues";

export interface GraphOverviewCounts {
  mapped: number;
  workflows: number;
  cachedWorkflows: number;
  models: number;
  cachedModels: number;
  caches: number;
  modelUses: number;
  unreferenced: number;
  risk: number;
  issues: number;
  cacheIssues: number;
}

export interface GraphSummaryItem {
  key: "mapped" | "workflows" | "models" | "caches" | "modelUses" | "unreferenced" | "issues" | "cacheIssues";
  count: number;
  label: string;
}

function countedLabel(count: number, singular: string, plural = `${singular}s`) {
  return count === 1 ? singular : plural;
}

export function graphSummaryItems(counts: GraphOverviewCounts): GraphSummaryItem[] {
  const items: GraphSummaryItem[] = [
    { key: "mapped", count: counts.mapped, label: countedLabel(counts.mapped, "mapped item") }
  ];
  if (counts.workflows > 0) items.push({ key: "workflows", count: counts.workflows, label: countedLabel(counts.workflows, "workflow") });
  if (counts.models > 0) items.push({ key: "models", count: counts.models, label: countedLabel(counts.models, "model") });
  if (counts.caches > 0) items.push({ key: "caches", count: counts.caches, label: "inside dependency caches" });
  if (counts.modelUses > 0) items.push({ key: "modelUses", count: counts.modelUses, label: countedLabel(counts.modelUses, "model use") });
  if (counts.unreferenced > 0) items.push({ key: "unreferenced", count: counts.unreferenced, label: "unreferenced" });
  items.push({ key: "issues", count: counts.issues, label: "need direct review" });
  if (counts.cacheIssues > 0) items.push({ key: "cacheIssues", count: counts.cacheIssues, label: "cache observations" });
  return items;
}

export function graphFiltersForCounts(counts: GraphOverviewCounts): MapFilter[] {
  const filters: MapFilter[] = ["all"];
  if (counts.workflows + counts.cachedWorkflows > 0) filters.push("workflows");
  if (counts.models + counts.cachedModels > 0) filters.push("models");
  if (counts.caches > 0) filters.push("caches");
  if (counts.models + counts.cachedModels + counts.caches > 0) filters.push("assets");
  if (counts.unreferenced > 0) filters.push("orphans");
  if (counts.risk > 0) filters.push("risk");
  if (counts.issues + counts.cacheIssues > 0) filters.push("issues");
  return filters;
}

export function groupGraphNodesForDisplay(nodes: GraphNode[]): { nodes: GraphNode[]; cacheNodes: GraphNode[] } {
  const cacheNodes: GraphNode[] = [];
  const directNodes: GraphNode[] = [];
  for (const node of nodes) {
    const nodeText = [node.path, node.displayName, ...node.details].join(" ");
    if (textMentionsDependencyCache(nodeText)) cacheNodes.push(node);
    else directNodes.push(node);
  }
  if (!cacheNodes.length) return { nodes, cacheNodes };
  return { nodes: directNodes, cacheNodes };
}

export function groupGraphIssuesForDisplay(issues: GraphIssue[], nodeById: Map<number, GraphNode>): { issues: GraphIssue[]; cacheIssues: GraphIssue[] } {
  const cacheIssues: GraphIssue[] = [];
  const directIssues: GraphIssue[] = [];
  for (const issue of issues) {
    const node = nodeById.get(issue.nodeId);
    const issueText = [
      issue.sourcePath,
      node?.path,
      node?.displayName,
      ...(node?.details ?? []),
      issue.target,
      issue.evidence ?? ""
    ].filter(Boolean).join(" ");
    if (textMentionsDependencyCache(issueText)) cacheIssues.push(issue);
    else directIssues.push(issue);
  }
  if (!cacheIssues.length) return { issues, cacheIssues };
  return { issues: directIssues, cacheIssues };
}

export function graphIssueContextPath(issue: GraphIssue, node?: GraphNode): string {
  return node?.path ?? issue.sourcePath ?? issue.target;
}

export function graphFilterLabel(filter: MapFilter) {
  if (filter === "all") return "All mapped items";
  if (filter === "workflows") return "Workflows";
  if (filter === "models") return "Models";
  if (filter === "caches") return "Caches";
  if (filter === "assets") return "Assets by size";
  if (filter === "orphans") return "Unreferenced models";
  if (filter === "risk") return "Risk";
  return "References to review";
}

function graphFilterHelp(filter: MapFilter) {
  if (filter === "workflows") return "Show bounded local JSON workflow files whose model references Code Hangar can inspect.";
  if (filter === "models") return "Show local model files such as checkpoints, LoRAs, VAEs, GGUF and ONNX assets.";
  if (filter === "caches") return "Show model, workflow and directory entries that live inside recognised dependency or tool caches. Their presence is descriptive and not a deletion recommendation.";
  if (filter === "assets") return "Show every local model and cache by physical size, largest first, with a reclaimable subtotal. Shared and protected assets are excluded from that total.";
  if (filter === "orphans") return "Show model files no local workflow references — candidates to review. Models shared with other projects or protected are not counted as reclaimable.";
  if (filter === "risk") return "Show assets shared across projects or protected/sensitive — review before reclaiming — plus workflow references that need human review.";
  if (filter === "issues") return "Show missing, ambiguous or invalid workflow references that need human review.";
  return "Show all workflows, models and caches currently known for this project.";
}

function graphEmptyMessage(filter: MapFilter) {
  if (filter === "all") return "No mapped workflows, models or caches were found in the current inventory.";
  if (filter === "workflows") return "No likely local workflow JSON files were found in the current inventory.";
  if (filter === "models") return "No recognised local model files were found in the current inventory.";
  if (filter === "caches") return "No model, workflow or directory entries were found inside recognised dependency or tool caches.";
  if (filter === "assets") return "No local model or cache assets were found in the current inventory.";
  if (filter === "orphans") return "Every mapped model is referenced by at least one local workflow.";
  if (filter === "risk") return "No shared or protected assets were found. Any reference issues appear below.";
  return "No reference issues were found.";
}

function graphNodeKindLabel(node: GraphNode) {
  if (node.graphKind === "workflow") return "Workflow";
  if (node.graphKind === "cache") return "Cache";
  if (node.graphKind.startsWith("model:")) return node.graphKind.slice("model:".length).replaceAll("_", " ");
  return "Asset";
}

function graphNodeHelp(node: GraphNode) {
  const shared = node.sharedProjectIds.length > 1
    ? ` This physical item is inventoried by ${node.sharedProjectIds.length} registered projects, so ownership is shared.`
    : "";
  const header = node.details.length ? ` Header summary: ${node.details.join("; ")}.` : "";
  if (node.graphKind === "workflow") return `Open workflow ${node.displayName}. Code Hangar parsed only bounded local JSON fields to map model references.${shared}`;
  if (node.graphKind === "cache") {
    const cacheDetails = node.details.length ? ` Cache classification: ${node.details.join("; ")}.` : "";
    return `Inspect cache folder ${node.displayName}. A cache may be shared and is not automatically safe to remove.${cacheDetails}${shared}`;
  }
  return `Open model asset ${node.displayName}. The category is inferred from its extension and folder location.${header}${shared}`;
}

function graphNodeIcon(node: GraphNode) {
  if (node.graphKind === "workflow") return <GitFork size={15} />;
  if (node.graphKind === "cache") return <Database size={15} />;
  return <Boxes size={15} />;
}

function graphNodeSummary(node: GraphNode) {
  const parts = [graphNodeKindLabel(node)];
  if (node.details.length) parts.push(node.details.join(" · "));
  if (node.physicalBytes != null) parts.push(formatOptionalBytes(node.physicalBytes));
  if (node.sharedProjectIds.length > 1) parts.push(`shared by ${node.sharedProjectIds.length} projects`);
  return parts.join(" · ");
}

function graphIssueLabel(kind: string) {
  if (kind === "missing_model_reference") return "Model not found";
  if (kind === "ambiguous_model_reference") return "Model name is ambiguous";
  if (kind === "duplicate_model_candidate") return "Possible duplicate model";
  if (kind === "shared_cache_candidate") return "Shared cache candidate";
  if (kind === "workflow_parse_error") return "Workflow JSON could not be read";
  return "Reference needs review";
}

function graphIssueActionHelp(kind: string, target: string) {
  if (kind === "shared_cache_candidate") return `Open cache folder ${target}.`;
  if (kind === "duplicate_model_candidate") return `Open model candidate ${target}.`;
  return `Open the workflow that reported ${target}.`;
}

function graphIssueHelp(kind: string) {
  if (kind === "missing_model_reference") return "No inventoried local model matched this workflow value.";
  if (kind === "ambiguous_model_reference") return "Several local models share this name, so Code Hangar cannot choose one safely.";
  if (kind === "duplicate_model_candidate") return "These model files have the same size and a matching content sample. They are only possible duplicates until you choose to compare the complete files.";
  if (kind === "shared_cache_candidate") return "This cache is probably shared across tools or registered projects, so Code Hangar treats ownership as needing review.";
  if (kind === "workflow_parse_error") return "The candidate file was not valid bounded JSON; no model references were inferred from it.";
  return "The local relationship could not be resolved with enough confidence.";
}

interface ProjectSessionViewState {
  query: string;
  appFilter: string;
  scrollTop: number;
}

const projectSessionViewCache = new Map<number, ProjectSessionViewState>();

export function filterProjectSessions(
  sessions: SessionDiscoveryCandidate[],
  query: string,
  appFilter: string
): SessionDiscoveryCandidate[] {
  const needle = query.trim().toLowerCase();
  return [...sessions]
    .filter((session) => appFilter === "all" || sessionAppMeta(session).slug === appFilter)
    .filter((session) => {
      if (!needle) return true;
      return [
        session.displayName,
        session.sourceLabel,
        session.sessionKind,
        session.path,
        ...session.linkedProjectPaths
      ].some((value) => value.toLowerCase().includes(needle));
    })
    .sort((left, right) =>
      (right.modifiedMs ?? 0) - (left.modifiedMs ?? 0)
      || left.displayName.localeCompare(right.displayName)
    );
}

export function projectSessionAppOptions(sessions: SessionDiscoveryCandidate[]) {
  const options = new Map<string, { slug: string; label: string; count: number }>();
  for (const session of sessions) {
    const meta = sessionAppMeta(session);
    const current = options.get(meta.slug);
    options.set(meta.slug, {
      slug: meta.slug,
      label: meta.label,
      count: (current?.count ?? 0) + 1
    });
  }
  return [...options.values()].sort((left, right) => left.label.localeCompare(right.label));
}

export function ProjectSessionsHome({
  projectId,
  sessions,
  onOpenSession,
  onContextMenu
}: {
  projectId: number | null;
  sessions: SessionDiscoveryCandidate[];
  onOpenSession: (session: SessionDiscoveryCandidate) => void;
  onContextMenu: SessionContextMenuHandler;
}) {
  const cachedView = projectId == null ? null : projectSessionViewCache.get(projectId);
  const [sessionQuery, setSessionQuery] = useState(() => cachedView?.query ?? "");
  const [appFilter, setAppFilter] = useState(() => cachedView?.appFilter ?? "all");
  const [sessionLimit, setSessionLimit] = useState(LARGE_LIST_PAGE_SIZE);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const viewStateRef = useRef<ProjectSessionViewState>({
    query: cachedView?.query ?? "",
    appFilter: cachedView?.appFilter ?? "all",
    scrollTop: cachedView?.scrollTop ?? 0
  });
  const appOptions = useMemo(() => projectSessionAppOptions(sessions), [sessions]);
  const orderedSessions = useMemo(
    () => filterProjectSessions(sessions, sessionQuery, appFilter),
    [appFilter, sessionQuery, sessions]
  );
  const visibleSessions = orderedSessions.slice(0, sessionLimit);

  useEffect(() => {
    setSessionLimit(LARGE_LIST_PAGE_SIZE);
  }, [appFilter, projectId, sessionQuery]);

  useEffect(() => {
    if (appFilter !== "all" && !appOptions.some((option) => option.slug === appFilter)) {
      setAppFilter("all");
    }
  }, [appFilter, appOptions]);

  useEffect(() => {
    viewStateRef.current = { ...viewStateRef.current, query: sessionQuery, appFilter };
    if (projectId != null) projectSessionViewCache.set(projectId, viewStateRef.current);
  }, [appFilter, projectId, sessionQuery]);

  useLayoutEffect(() => {
    const scrollContainer = rootRef.current?.closest<HTMLElement>(".preview-surface");
    if (!scrollContainer || projectId == null) return;
    const restoreScroll = () => {
      scrollContainer.scrollTop = viewStateRef.current.scrollTop;
    };
    restoreScroll();
    const frame = window.requestAnimationFrame(restoreScroll);
    const rememberScroll = () => {
      viewStateRef.current = { ...viewStateRef.current, scrollTop: scrollContainer.scrollTop };
      projectSessionViewCache.set(projectId, viewStateRef.current);
    };
    scrollContainer.addEventListener("scroll", rememberScroll, { passive: true });
    return () => {
      window.cancelAnimationFrame(frame);
      rememberScroll();
      scrollContainer.removeEventListener("scroll", rememberScroll);
    };
  }, [projectId]);

  const clearFilters = () => {
    setSessionQuery("");
    setAppFilter("all");
  };

  return (
    <div className="project-home" ref={rootRef}>
      <div className="project-home-intro">
        <span>Local sessions</span>
        <div className="heading-with-help">
          <h2>Sessions linked to this project</h2>
          <ConceptHelp concept="sessions" />
        </div>
        <p>These are local conversation records discovered from AI tools. They are read-only metadata; opening one does not change the original app or project files.</p>
      </div>
      {sessions.length > 0 ? (
        <div className="project-session-toolbar">
          <div className="project-session-search">
            <Search size={15} aria-hidden="true" />
            <input
              type="search"
              value={sessionQuery}
              onChange={(event) => setSessionQuery(event.target.value)}
              placeholder="Filter sessions"
              aria-label="Filter project sessions"
            />
            {sessionQuery ? (
              <button type="button" onClick={() => setSessionQuery("")} aria-label="Clear session search" data-help="Clear the project session search.">
                <X size={14} />
              </button>
            ) : null}
          </div>
          <select value={appFilter} onChange={(event) => setAppFilter(event.target.value)} aria-label="Filter project sessions by app">
            <option value="all">All apps</option>
            {appOptions.map((option) => (
              <option key={option.slug} value={option.slug}>{option.label} ({option.count})</option>
            ))}
          </select>
          <span className="project-session-count" aria-live="polite">{orderedSessions.length} of {sessions.length}</span>
        </div>
      ) : null}
      {sessions.length === 0 ? (
        <div className="empty-state">
          No discovered local sessions are linked to this project yet. Use Discover, then Find projects, then Find local sessions to refresh the session map.
        </div>
      ) : orderedSessions.length === 0 ? (
        <div className="project-session-empty">
          <strong>No matching sessions</strong>
          <span>Try a different title or application.</span>
          <button type="button" className="secondary-button compact" onClick={clearFilters}>Clear filters</button>
        </div>
      ) : (
        <div className="project-context-grid project-session-list">
          {visibleSessions.map((session) => {
            const meta = sessionAppMeta(session);
            const facts = sessionCardFacts(session);
            return (
              <button
                type="button"
                className="project-context-card session-card"
                key={`${session.sourceKind}-${session.path}`}
                onClick={() => onOpenSession(session)}
                onContextMenu={(event) => onContextMenu(session, event)}
                data-help={`${sessionCardHelp(session)} Right-click to open or copy its path.`}
              >
                <MessageSquare size={16} />
                <span>
                  <span className="row-title">
                    <strong>{session.displayName}</strong>
                    <span className={`app-badge app-badge--${meta.slug}`} title={`${meta.label} session`}>{meta.label}</span>
                  </span>
                  <small>{displayAppText(session.sourceLabel)}{session.modifiedMs != null ? ` · ${formatTimestamp(session.modifiedMs)}` : ""}</small>
                  <span className="session-facts" aria-label="Session summary">
                    {facts.map((fact) => <small key={fact}>{fact}</small>)}
                  </span>
                </span>
              </button>
            );
          })}
          <ProgressiveListControls
            shown={visibleSessions.length}
            total={orderedSessions.length}
            noun="sessions"
            onMore={() => setSessionLimit((current) => nextProgressiveListLimit(current, orderedSessions.length))}
            onAll={() => setSessionLimit(orderedSessions.length)}
          />
        </div>
      )}
    </div>
  );
}

export function sessionCardFacts(session: SessionDiscoveryCandidate): string[] {
  return [session.linkedProjectPaths.length ? "Project-linked" : "No project path"];
}

export function sessionCardHelp(session: SessionDiscoveryCandidate): string {
  const linkedPath = session.linkedProjectPaths[0] ? ` Linked path: ${session.linkedProjectPaths[0]}.` : "";
  return `Open ${session.displayName}. ${displayAppText(session.sourceLabel)} ${displayAppText(session.sessionKind)} session. ${sessionAssociationHelp(session.association)} Local metadata path: ${session.path}.${linkedPath}`;
}

function sessionAssociationLabel(association: string) {
  if (association === "registered_project") return "linked to this project";
  if (association === "unregistered_project_reference") return "mentions a project not added yet";
  if (association === "loose_session") return "loose session";
  return "session";
}

function sessionAssociationHelp(association: string) {
  if (association === "registered_project") return "Code Hangar found a project path in this session that matches a registered project.";
  if (association === "unregistered_project_reference") return "This session mentions a local project folder that has not been added to Projects.";
  if (association === "loose_session") return "This session was found locally, but no project path was linked from its metadata.";
  return "Local session metadata discovered on this machine.";
}
