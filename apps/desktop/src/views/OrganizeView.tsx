import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Boxes,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  FolderTree,
  HardDrive,
  Layers,
  ListChecks
} from "lucide-react";
import { api } from "../api";
import type { DuplicateCandidates, OrphanCandidate, OrphanCandidates, ProjectSummary } from "../types";
import { formatBytes, formatOptionalBytes } from "../ui";
import {
  ORGANIZE_MODEL_FILE_PREVIEW,
  ORGANIZE_MODEL_LOCATION_PREVIEW,
  organizeDisclosureItems,
  organizeInventoryFingerprint,
  organizeModelResultCounts,
  organizeProjectReviewSummary,
  organizeProjectReviewReason,
  splitProjectsForLocationReview
} from "./organize-helpers";

type OrganizeTab = "models" | "projects";

interface OrganizeSessionCache {
  inventoryFingerprint: string | null;
  tab: OrganizeTab;
  models: OrphanCandidates | null;
  duplicates: DuplicateCandidates | null;
  loaded: boolean;
  dupRequested: boolean;
  showAllModelGroups: boolean;
  expandedModelGroups: string[];
  fullyShownModelGroups: string[];
}

let organizeSessionCache: OrganizeSessionCache = {
  inventoryFingerprint: null,
  tab: "projects",
  models: null,
  duplicates: null,
  loaded: false,
  dupRequested: false,
  showAllModelGroups: false,
  expandedModelGroups: [],
  fullyShownModelGroups: []
};

/** Parse a Windows/WSL path into a drive/root + the parent folder, for location grouping. */
function locationOf(path: string): { drive: string; parent: string } {
  const normalized = path.replace(/\//g, "\\").replace(/^\\\\\?\\/i, "");
  const segments = normalized.split("\\").filter(Boolean);
  const drive = segments[0] ?? "(unknown)";
  const parent = segments.length > 1 ? segments.slice(0, -1).join("\\") : drive;
  return { drive, parent };
}

function groupBy<T>(items: T[], key: (item: T) => string): Map<string, T[]> {
  const map = new Map<string, T[]>();
  for (const item of items) {
    const k = key(item);
    const list = map.get(k) ?? [];
    list.push(item);
    map.set(k, list);
  }
  return map;
}

function toggledStringSet(values: ReadonlySet<string>, value: string): Set<string> {
  const next = new Set(values);
  if (next.has(value)) next.delete(value);
  else next.add(value);
  return next;
}

/**
 * Organize — a cross-project overview of (a) scattered & duplicate AI model files and
 * (b) projects grouped by disk location, so the user can SEE the sprawl before tidying it.
 * Read-only: every action routes into the existing, proven flows (Inspect / Safe Manage).
 */
export function OrganizeView({
  active,
  projects,
  onOpenNode,
  onSafeManageProject
}: {
  active: boolean;
  projects: ProjectSummary[];
  onOpenNode: (nodeId: number, projectId: number) => void;
  onSafeManageProject: (projectId: number) => void;
}) {
  const inventoryFingerprint = organizeInventoryFingerprint(projects);
  const includeFixtureProjects = projects.some((project) => project.source === "fixture");
  const cachedSession = organizeSessionCache.inventoryFingerprint === inventoryFingerprint
    ? organizeSessionCache
    : null;
  const [tab, setTab] = useState<OrganizeTab>(() => cachedSession?.tab ?? "projects");
  const [models, setModels] = useState<OrphanCandidates | null>(() => cachedSession?.models ?? null);
  const [duplicates, setDuplicates] = useState<DuplicateCandidates | null>(() => cachedSession?.duplicates ?? null);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(() => cachedSession?.loaded ?? false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [scanSeconds, setScanSeconds] = useState(0);
  const [dupLoading, setDupLoading] = useState(false);
  const [dupRequested, setDupRequested] = useState(() => cachedSession?.dupRequested ?? false);
  const [dupError, setDupError] = useState<string | null>(null);
  const [showAllModelGroups, setShowAllModelGroups] = useState(() => cachedSession?.showAllModelGroups ?? false);
  const [expandedModelGroups, setExpandedModelGroups] = useState(
    () => new Set(cachedSession?.expandedModelGroups ?? [])
  );
  const [fullyShownModelGroups, setFullyShownModelGroups] = useState(
    () => new Set(cachedSession?.fullyShownModelGroups ?? [])
  );
  const modelScanRun = useRef(0);
  const duplicateScanRun = useRef(0);
  const previousInventoryFingerprint = useRef(inventoryFingerprint);

  useEffect(() => () => {
    modelScanRun.current += 1;
    duplicateScanRun.current += 1;
  }, []);

  useEffect(() => {
    if (previousInventoryFingerprint.current !== inventoryFingerprint) {
      previousInventoryFingerprint.current = inventoryFingerprint;
      modelScanRun.current += 1;
      duplicateScanRun.current += 1;
      setModels(null);
      setDuplicates(null);
      setLoading(false);
      setLoaded(false);
      setLoadError(null);
      setScanSeconds(0);
      setDupLoading(false);
      setDupRequested(false);
      setDupError(null);
      setShowAllModelGroups(false);
      setExpandedModelGroups(new Set());
      setFullyShownModelGroups(new Set());
      organizeSessionCache = {
        inventoryFingerprint,
        tab,
        models: null,
        duplicates: null,
        loaded: false,
        dupRequested: false,
        showAllModelGroups: false,
        expandedModelGroups: [],
        fullyShownModelGroups: []
      };
      return;
    }

    organizeSessionCache = {
      inventoryFingerprint,
      tab,
      models,
      duplicates,
      loaded,
      dupRequested,
      showAllModelGroups,
      expandedModelGroups: Array.from(expandedModelGroups),
      fullyShownModelGroups: Array.from(fullyShownModelGroups)
    };
  }, [
    inventoryFingerprint,
    tab,
    models,
    duplicates,
    loaded,
    dupRequested,
    showAllModelGroups,
    expandedModelGroups,
    fullyShownModelGroups
  ]);

  useEffect(() => {
    if (!loading) return;
    const startedAt = Date.now();
    setScanSeconds(0);
    const id = window.setInterval(
      () => setScanSeconds(Math.max(1, Math.round((Date.now() - startedAt) / 1000))),
      1_000
    );
    return () => window.clearInterval(id);
  }, [loading]);

  const loadModelCandidates = useCallback(() => {
    if (!active || loading) return;
    const run = modelScanRun.current + 1;
    modelScanRun.current = run;
    setLoading(true);
    setLoadError(null);
    setLoaded(false);
    setModels(null);
    setShowAllModelGroups(false);
    setExpandedModelGroups(new Set());
    setFullyShownModelGroups(new Set());
    // The duplicate-model scan partial-hashes model files across the WHOLE inventory and is heavy
    // enough to stall the app on large, model-rich disks, so it remains on-demand below.
    void api
      .orphanAssetCandidates({ assetKind: "model", minConfidence: "Low", includePartial: true, limit: 1000, includeFixtureProjects })
      .then((modelResult) => {
        if (modelScanRun.current !== run) return;
        setModels(modelResult);
        setLoaded(true);
      })
      // Handle the rejection locally: without this the model scan failing leaves the tab stuck
      // blank (no spinner, no empty-state) and raises an unhandled promise rejection.
      .catch((error: unknown) => {
        if (modelScanRun.current !== run) return;
        setLoadError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (modelScanRun.current === run) setLoading(false);
      });
  }, [active, includeFixtureProjects, loading]);

  const findDuplicates = useCallback(() => {
    if (dupLoading) return;
    const run = duplicateScanRun.current + 1;
    duplicateScanRun.current = run;
    setDupLoading(true);
    setDupRequested(true);
    setDupError(null);
    void api
      .duplicateCandidates({ fileKind: "model", limit: 400, includeFixtureProjects })
      .then((dupResult) => {
        if (duplicateScanRun.current === run) setDuplicates(dupResult);
      })
      .catch((error: unknown) => {
        if (duplicateScanRun.current === run) {
          setDupError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (duplicateScanRun.current === run) setDupLoading(false);
      });
  }, [dupLoading, includeFixtureProjects]);

  const modelGroups = useMemo(() => {
    const candidates = models?.candidates ?? [];
    const grouped = groupBy(candidates, (candidate) => locationOf(candidate.path).parent);
    return Array.from(grouped.entries())
      .map(([parent, items]) => ({
        parent,
        items,
        bytes: items.reduce((sum, item) => sum + (item.physicalBytes ?? 0), 0)
      }))
      .sort((a, b) => b.bytes - a.bytes);
  }, [models]);

  const modelTotals = useMemo(() => {
    const candidates = models?.candidates ?? [];
    const bytes = candidates.reduce((sum, item) => sum + (item.physicalBytes ?? 0), 0);
    const dupGroups = duplicates?.groups ?? [];
    const dupReclaimable = dupGroups.reduce(
      // Reclaiming a duplicate set keeps ONE copy, so reclaimable = total footprint minus one
      // copy — never the full footprint. With sizeBytes ≈ one copy's size this is correct for
      // distinct copies ((n-1)*size) AND for hardlinked sets (one shared extent − one copy ≈ 0,
      // not a full copy overstated).
      (sum, group) =>
        sum + Math.max(0, (group.physicalBytes ?? group.sizeBytes * group.memberCount) - group.sizeBytes),
      0
    );
    return { count: candidates.length, bytes, dupGroups: dupGroups.length, dupReclaimable };
  }, [models, duplicates]);

  const modelResultCounts = useMemo(() => organizeModelResultCounts(models), [models]);
  const visibleModelGroups = useMemo(
    () => organizeDisclosureItems(modelGroups, showAllModelGroups, ORGANIZE_MODEL_LOCATION_PREVIEW),
    [modelGroups, showAllModelGroups]
  );

  const projectGroups = useMemo(() => {
    const real = projects.filter((project) => project.source !== "fixture");
    const grouped = groupBy(real, (project) => {
      const { drive, parent } = locationOf(project.path);
      return parent || drive;
    });
    return Array.from(grouped.entries())
      .map(([parent, items]) => ({ parent, items: items.sort((a, b) => a.name.localeCompare(b.name)) }))
      .sort((a, b) => b.items.length - a.items.length || a.parent.localeCompare(b.parent));
  }, [projects]);
  const projectReviewSummary = useMemo(() => organizeProjectReviewSummary(projects), [projects]);

  return (
    <section className="pane-section compact organize-view">
      <div className="organize-tabs">
        <button type="button" className={tab === "models" ? "active" : ""} onClick={() => setTab("models")} data-help="Model-like files without an indexed reference, plus duplicate candidates, grouped by folder for review.">
          <Boxes size={14} /> Models
        </button>
        <button type="button" className={tab === "projects" ? "active" : ""} onClick={() => setTab("projects")} data-help="Your projects grouped by disk location, with the small/idle ones flagged.">
          <FolderTree size={14} /> Projects by location
        </button>
      </div>

      {tab === "models" ? (
        <div className="organize-body">
          {!loaded && !loading && !loadError ? (
            <div className="organize-start-panel" data-help="Model review can inspect large local inventories, so it starts only when you ask for it. Project grouping stays instant and read-only.">
              <strong>Model review is on demand</strong>
              <p>Scan indexed checkpoints, GGUFs or safetensors when you want to review files without indexed references. Nothing is moved or hashed fully from this button.</p>
              <button type="button" className="action-button" onClick={() => void loadModelCandidates()}>
                Scan model files
              </button>
            </div>
          ) : null}
          {loading ? (
            <div className="organize-loading-panel" data-help="This reads local inventory metadata for model-like files. You can switch tabs while it runs.">
              <strong>Scanning local model files{scanSeconds > 0 ? ` · ${scanSeconds}s` : ""}</strong>
              <span>Large model inventories can take a while. Projects by location stays available immediately.</span>
              <button type="button" className="secondary-button slim" onClick={() => setTab("projects")}>Show project locations</button>
            </div>
          ) : null}
          {loadError ? (
            <div className="organize-start-panel warning-card">
              <strong>Model scan did not finish</strong>
              <p>{loadError}</p>
              <button type="button" className="secondary-button slim" onClick={() => void loadModelCandidates()}>Try again</button>
            </div>
          ) : null}
          {loaded ? (
            <div className="organize-summary" data-help="These model-like files have no incoming reference in the current local index. That makes them review candidates, not proof that they are unused.">
              <span>
                <strong>{modelResultCounts.total}</strong> file{modelResultCounts.total === 1 ? "" : "s"} without an indexed reference · {modelGroups.length} location{modelGroups.length === 1 ? "" : "s"}
              </span>
              <span>
                {modelResultCounts.limited ? `Showing ${modelResultCounts.shown} · ` : ""}{formatBytes(modelTotals.bytes)} in loaded results
              </span>
              {modelTotals.dupGroups > 0 ? (
                <span><strong>{modelTotals.dupGroups}</strong> duplicate set{modelTotals.dupGroups === 1 ? "" : "s"} · ~{formatBytes(modelTotals.dupReclaimable)} reclaimable</span>
              ) : null}
            </div>
          ) : null}
          {loaded && modelGroups.length > 0 ? (
            <p className="organize-result-note">
              Review candidates only. Expand a location to inspect its first {ORGANIZE_MODEL_FILE_PREVIEW} files; nothing here confirms that a model is safe to remove.
            </p>
          ) : null}
          {loaded && modelGroups.length === 0 ? (
            <p className="muted result-empty">No model files without indexed references were found.</p>
          ) : null}
          {visibleModelGroups.map((group) => (
            <ModelLocationGroup
              expanded={expandedModelGroups.has(group.parent)}
              fullyShown={fullyShownModelGroups.has(group.parent)}
              group={group}
              key={group.parent}
              onOpenNode={onOpenNode}
              onSafeManageProject={onSafeManageProject}
              onToggleExpanded={() => {
                setExpandedModelGroups((values) => toggledStringSet(values, group.parent));
              }}
              onToggleFullyShown={() => {
                setFullyShownModelGroups((values) => toggledStringSet(values, group.parent));
              }}
            />
          ))}
          {modelGroups.length > ORGANIZE_MODEL_LOCATION_PREVIEW ? (
            <button
              type="button"
              className="secondary-button organize-disclosure-button"
              onClick={() => setShowAllModelGroups((value) => !value)}
            >
              {showAllModelGroups ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
              {showAllModelGroups
                ? `Show first ${ORGANIZE_MODEL_LOCATION_PREVIEW} locations`
                : `Show ${modelGroups.length - ORGANIZE_MODEL_LOCATION_PREVIEW} more locations`}
            </button>
          ) : null}
          {loaded ? (
            <section className="organize-group" data-help="Each set contains files with the same size and a matching content sample. Open Discover > Duplicates and compare the complete files before deciding what to keep.">
              <header className="organize-group-head">
                <Layers size={13} />
                <strong>Duplicate model sets</strong>
                {duplicates ? <span>{duplicates.groups.length}</span> : null}
              </header>
              {!dupRequested ? (
                <button type="button" className="secondary-button slim" onClick={findDuplicates} data-help="Compare a small content sample from same-size model files. This reads local files only when you request it.">Find duplicate model sets</button>
              ) : null}
              {dupLoading ? <small className="muted">Scanning model files for duplicates…</small> : null}
              {dupError ? <small className="muted">Could not scan duplicates: {dupError}</small> : null}
              {dupRequested && !dupLoading && !dupError && duplicates && duplicates.groups.length === 0 ? (
                <small className="muted">No duplicate model sets found.</small>
              ) : null}
              {(duplicates?.groups ?? []).slice(0, 30).map((group) => (
                <div className="organize-row" key={group.id}>
                  <div>
                    <strong>{group.memberCount} copies · {formatBytes(group.sizeBytes)} each</strong>
                    <small>{group.members.map((member) => member.projectName).filter((value, index, self) => self.indexOf(value) === index).join(", ")}</small>
                  </div>
                  <div className="organize-row-actions">
                    <small>~{formatOptionalBytes(group.physicalBytes)} space used</small>
                    <button type="button" className="secondary-button slim" data-help="Inspect the first copy in the file inspector. The other copies remain listed here." onClick={() => group.members[0] && onOpenNode(group.members[0].nodeId, group.members[0].projectId)}>Inspect first</button>
                  </div>
                </div>
              ))}
            </section>
          ) : null}
        </div>
      ) : (
        <div className="organize-body">
          {projectGroups.length === 0 ? <p className="muted result-empty">No projects to organize yet.</p> : null}
          {projectGroups.length > 0 ? (
            <div
              className="organize-summary organize-project-summary"
              data-help={`${projectReviewSummary.mapped} projects have context or a recognized activity signal. ${projectReviewSummary.needsReview} still need review across ${projectGroups.length} disk locations.`}
            >
              <span><strong>{projectReviewSummary.total}</strong> projects</span>
              <span><strong>{projectGroups.length}</strong> locations</span>
              <span><strong>{projectReviewSummary.mapped}</strong> mapped</span>
              <span className={projectReviewSummary.needsReview > 0 ? "attention" : undefined}><strong>{projectReviewSummary.needsReview}</strong> need review</span>
              <div
                className="organize-project-progress"
                role="progressbar"
                aria-label="Projects mapped with context or activity"
                aria-valuemin={0}
                aria-valuemax={projectReviewSummary.total}
                aria-valuenow={projectReviewSummary.mapped}
              >
                <span style={{ width: `${projectReviewSummary.progress}%` }} />
              </div>
            </div>
          ) : null}
          {projectGroups.map((group) => (
            <ProjectLocationGroup
              group={group}
              key={group.parent}
              onSafeManageProject={onSafeManageProject}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function ModelLocationGroup({
  expanded,
  fullyShown,
  group,
  onOpenNode,
  onSafeManageProject,
  onToggleExpanded,
  onToggleFullyShown
}: {
  expanded: boolean;
  fullyShown: boolean;
  group: { parent: string; items: OrphanCandidate[]; bytes: number };
  onOpenNode: (nodeId: number, projectId: number) => void;
  onSafeManageProject: (projectId: number) => void;
  onToggleExpanded: () => void;
  onToggleFullyShown: () => void;
}) {
  const visibleItems = organizeDisclosureItems(group.items, fullyShown, ORGANIZE_MODEL_FILE_PREVIEW);

  return (
    <section className="organize-group" data-help={`${group.items.length} model review candidate(s) in this folder, ${formatBytes(group.bytes)} in the loaded inventory.`}>
      <button
        type="button"
        className="organize-group-head organize-group-toggle"
        aria-expanded={expanded}
        onClick={onToggleExpanded}
      >
        <ChevronRight className="organize-group-chevron" size={14} />
        <HardDrive size={13} />
        <strong title={group.parent}>{group.parent}</strong>
        <span>{group.items.length} file{group.items.length === 1 ? "" : "s"} · {formatBytes(group.bytes)}</span>
      </button>
      {expanded ? (
        <div className="organize-group-content">
          {visibleItems.map((model) => (
            <div className="organize-row" key={model.nodeId}>
              <div>
                <strong>{model.displayName}</strong>
                <small>{model.projectName} · {model.confidence} confidence{model.footprintPartial ? " · partial size" : ""}</small>
              </div>
              <div className="organize-row-actions">
                <small>{formatOptionalBytes(model.physicalBytes)}</small>
                <button type="button" className="secondary-button slim" data-help="Inspect this model file and its local metadata." onClick={() => onOpenNode(model.nodeId, model.projectId)}>Inspect</button>
                <button type="button" className="secondary-button slim" data-help="Open Safe Manage for this file's project to review ownership, references and protected paths. Nothing is changed." onClick={() => onSafeManageProject(model.projectId)}>Safe Manage</button>
              </div>
            </div>
          ))}
          {group.items.length > ORGANIZE_MODEL_FILE_PREVIEW ? (
            <button
              type="button"
              className="organize-file-disclosure"
              onClick={onToggleFullyShown}
            >
              {fullyShown ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
              {fullyShown
                ? `Show first ${ORGANIZE_MODEL_FILE_PREVIEW} files`
                : `Show all ${group.items.length} files in this location`}
            </button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function ProjectLocationGroup({
  group,
  onSafeManageProject
}: {
  group: { parent: string; items: ProjectSummary[] };
  onSafeManageProject: (projectId: number) => void;
}) {
  const { needsReview, mapped } = splitProjectsForLocationReview(group.items);
  const hasReviewItems = needsReview.length > 0;
  return (
    <section className="organize-group" data-help={`${group.items.length} project(s) under this location. Projects needing review are shown first; mapped projects stay available below.`}>
      <header className="organize-group-head">
        <HardDrive size={13} />
        <strong title={group.parent}>{group.parent}</strong>
        <span>{group.items.length} project{group.items.length === 1 ? "" : "s"} · {hasReviewItems ? `${needsReview.length} need review` : "mapped"}</span>
      </header>
      {needsReview.map((project) => (
        <ProjectLocationRow project={project} key={project.id} onSafeManageProject={onSafeManageProject} />
      ))}
      {mapped.length ? (
        <details className="organize-subgroup">
          <summary>{hasReviewItems ? "Mapped projects with context/activity" : "Mapped projects"} ({mapped.length})</summary>
          {mapped.map((project) => (
            <ProjectLocationRow project={project} key={project.id} onSafeManageProject={onSafeManageProject} subdued />
          ))}
        </details>
      ) : null}
    </section>
  );
}

function ProjectLocationRow({
  project,
  onSafeManageProject,
  subdued = false
}: {
  project: ProjectSummary;
  onSafeManageProject: (projectId: number) => void;
  subdued?: boolean;
}) {
  const reason = organizeProjectReviewReason(project);
  const contextLabel = `${project.contextCount ?? 0} context file${project.contextCount === 1 ? "" : "s"}`;
  return (
    <div className={`organize-row${reason ? " attention" : ""}${subdued ? " subdued" : ""}`}>
      <div>
        <strong>{project.name}</strong>
        <small>{project.app ?? "—"} · {reason ?? contextLabel}</small>
      </div>
      <div className="organize-row-actions">
        {reason ? <span className="organize-flag" data-help="Review candidate: this row lacks enough local context/activity signal to treat as clearly understood.">Needs review</span> : null}
        <button type="button" className="secondary-button slim organize-review-button" data-help="Review this project's ownership, references and protected paths in Safe Manage. Nothing is changed." onClick={() => onSafeManageProject(project.id)}>
          <ListChecks size={13} />
          Review
        </button>
      </div>
    </div>
  );
}
