import { projectAppMetas } from "./app-meta";
import type { ProjectScanState, ProjectSummary, QuickOpenResult } from "./types";

export type DemoProjectPreference = boolean | null;
export type ProjectSort = "name" | "size" | "recent";
export type ProjectStatusFilter = "all" | "ready" | "scanning" | "needs-scan";
export type ProjectStatusBucket = Exclude<ProjectStatusFilter, "all">;

interface SidebarProjectOptions {
  sort: ProjectSort;
  appFilter: string;
  statusFilter: ProjectStatusFilter;
  query?: string;
  archivedProjectIds?: ReadonlySet<number>;
  recencyByProjectId?: ReadonlyMap<number, number>;
  getStatusBucket?: (project: ProjectSummary) => ProjectStatusBucket;
}

export interface OrderedSidebarProjects {
  active: ProjectSummary[];
  archived: ProjectSummary[];
  all: ProjectSummary[];
}

interface CompactSidebarProjectOptions {
  limit: number;
  selectedProjectId?: number | null;
}

export interface CompactSidebarProjectList<T> {
  projects: T[];
  hiddenCount: number;
  compacted: boolean;
}

export type ProjectSearchKeyAction = "open-first" | "clear" | "none";
export type QuickOpenSearchStatus = "idle" | "loading" | "error";

interface ArchivedProjectVisibilityOptions {
  isArchived: boolean;
  archivedCollapsed: boolean;
  isSelected: boolean;
}

export function isDemoProject(project: ProjectSummary) {
  return project.source === "fixture";
}

export function shouldShowDemoProjects(projects: ProjectSummary[], preference: DemoProjectPreference) {
  if (preference !== null) return preference;
  return projects.every(isDemoProject);
}

export function visibleProjects(projects: ProjectSummary[], preference: DemoProjectPreference) {
  return shouldShowDemoProjects(projects, preference)
    ? projects
    : projects.filter((project) => !isDemoProject(project));
}

export function visibleProjectItems<T extends { projectId: number }>(
  items: T[],
  projects: ProjectSummary[],
  preference: DemoProjectPreference
) {
  const visibleProjectIds = new Set(visibleProjects(projects, preference).map((project) => project.id));
  return items.filter((item) => visibleProjectIds.has(item.projectId));
}

export function composeQuickOpenResults(
  query: string,
  fileResults: QuickOpenResult[],
  projects: ProjectSummary[],
  preference: DemoProjectPreference
) {
  const visibleProjectIds = new Set(visibleProjects(projects, preference).map((project) => project.id));
  const visibleFiles = fileResults.filter((item) => visibleProjectIds.has(item.projectId));
  const projectResults = visibleProjects(projects, preference)
    .map((project) => ({ project, score: scoreQuickOpenProject(project, query) }))
    .filter((item) => item.score > 0)
    .map(({ project, score }): QuickOpenResult => ({
      nodeId: project.id,
      projectId: project.id,
      label: project.name,
      path: project.path,
      itemKind: "project",
      score
    }));

  return [...projectResults, ...visibleFiles]
    .sort((left, right) => (
      right.score - left.score
      || kindRank(left.itemKind) - kindRank(right.itemKind)
      || left.label.localeCompare(right.label)
    ))
    .slice(0, 20);
}

export function starterQuickOpenResults(
  projects: ProjectSummary[],
  selectedProjectId?: number | null,
  limit = 8
): QuickOpenResult[] {
  if (limit <= 0 || projects.length === 0) return [];
  const selectedProject = selectedProjectId == null
    ? null
    : projects.find((project) => project.id === selectedProjectId) ?? null;
  const rankedProjects = projects
    .filter((project) => project.id !== selectedProject?.id)
    .sort((left, right) => (
      starterProjectRank(right) - starterProjectRank(left)
      || left.name.localeCompare(right.name)
    ));
  const orderedProjects = selectedProject ? [selectedProject, ...rankedProjects] : rankedProjects;
  return orderedProjects.slice(0, limit).map((project, index) => ({
    nodeId: project.id,
    projectId: project.id,
    label: project.name,
    path: project.path,
    itemKind: "project",
    score: 1_000 - index
  }));
}

export function quickOpenSearchMessage(
  query: string,
  resultCount: number,
  status: QuickOpenSearchStatus
) {
  const trimmedLength = query.trim().length;
  if (trimmedLength === 0) return null;
  if (status === "loading") return "Searching indexed files...";
  if (status === "error") {
    return resultCount > 0
      ? "Indexed file search is unavailable. Project matches are still shown."
      : "Indexed file search is unavailable. Try again.";
  }
  if (trimmedLength < 2 && resultCount === 0) return "Type one more character to search indexed files.";
  if (resultCount === 0) return "No projects or files match this query.";
  return null;
}

export function orderSidebarProjects(projects: ProjectSummary[], options: SidebarProjectOptions): OrderedSidebarProjects {
  const archivedProjectIds = options.archivedProjectIds ?? new Set<number>();
  const getStatusBucket = options.getStatusBucket ?? ((project: ProjectSummary) => projectStatusBucketFromScanState(project.scanState ?? "scanned"));
  const matches = (project: ProjectSummary) => {
    if (options.appFilter !== "all" && !projectAppMetas(project).some((meta) => meta.slug === options.appFilter)) return false;
    if (options.statusFilter !== "all" && getStatusBucket(project) !== options.statusFilter) return false;
    return projectMatchesSidebarQuery(project, options.query ?? "");
  };
  const comparator = (a: ProjectSummary, b: ProjectSummary) => {
    if (options.sort === "size") {
      const diff = (b.contextCount ?? 0) - (a.contextCount ?? 0);
      if (diff !== 0) return diff;
    } else if (options.sort === "recent") {
      const diff = (options.recencyByProjectId?.get(b.id) ?? 0) - (options.recencyByProjectId?.get(a.id) ?? 0);
      if (diff !== 0) return diff;
    }
    return a.name.localeCompare(b.name);
  };
  const visible = projects.filter(matches);
  const active = visible.filter((project) => !archivedProjectIds.has(project.id)).sort(comparator);
  const archived = visible.filter((project) => archivedProjectIds.has(project.id)).sort(comparator);
  return { active, archived, all: [...active, ...archived] };
}

export function projectMatchesSidebarQuery(project: ProjectSummary, query: string) {
  const tokens = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return true;
  const appLabels = projectAppMetas(project).flatMap((meta) => [meta.slug, meta.label]);
  const haystack = [
    project.name,
    project.antigravityName ?? "",
    folderName(project.path),
    project.path,
    project.source,
    project.app ?? "",
    ...(project.apps ?? []),
    ...appLabels
  ].join("\n").toLocaleLowerCase();
  return tokens.every((token) => haystack.includes(token));
}

export function projectStatusBucketFromScanState(state: ProjectScanState): ProjectStatusBucket {
  if (state === "scanning") return "scanning";
  if (state === "outdated") return "needs-scan";
  return "ready";
}

export function resolveProjectScanState(
  scanState: ProjectScanState | null | undefined,
  watchState: string | null | undefined,
  rootScanning: boolean,
  cachedScannedProject: boolean
): ProjectScanState {
  if (rootScanning) return "scanning";
  if (watchState === "empty") return "scanned";
  if (watchState === "stale" || watchState === "missing" || watchState === "needs_scan") {
    return "outdated";
  }
  if (cachedScannedProject) return "outdated";
  return scanState ?? "scanned";
}

export function projectWatchLabel(state: string) {
  switch (state) {
    case "stale":
      return "Changed";
    case "missing":
      return "Missing";
    case "needs_scan":
      return "Needs scan";
    case "empty":
      return "Empty";
    default:
      return state;
  }
}

export function projectSearchKeyAction(key: string, query: string, resultCount: number): ProjectSearchKeyAction {
  if (key === "Enter" && resultCount > 0) return "open-first";
  if (key === "Escape" && query.trim().length > 0) return "clear";
  return "none";
}

export function shouldRenderProjectRow({
  isArchived,
  archivedCollapsed,
  isSelected
}: ArchivedProjectVisibilityOptions) {
  return !isArchived || !archivedCollapsed || isSelected;
}

export function compactSidebarProjects<T extends { id: number }>(
  projects: T[],
  options: CompactSidebarProjectOptions
): CompactSidebarProjectList<T> {
  const limit = Math.max(0, Math.floor(options.limit));
  if (limit === 0) {
    return { projects: [], hiddenCount: projects.length, compacted: projects.length > 0 };
  }
  if (projects.length <= limit) {
    return { projects, hiddenCount: 0, compacted: false };
  }
  const visible = projects.slice(0, limit);
  const selected = options.selectedProjectId == null
    ? undefined
    : projects.find((project) => project.id === options.selectedProjectId);
  if (selected && !visible.some((project) => project.id === selected.id)) {
    visible[visible.length - 1] = selected;
  }
  return {
    projects: visible,
    hiddenCount: projects.length - visible.length,
    compacted: true
  };
}

function scoreQuickOpenProject(project: ProjectSummary, query: string) {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return 0;
  const names = [
    project.name,
    project.antigravityName ?? "",
    folderName(project.path)
  ].filter(Boolean).map((value) => value.toLocaleLowerCase());
  const path = project.path.toLocaleLowerCase();
  const tokens = normalizedQuery.split(/\s+/).filter(Boolean);
  const haystack = [...names, path].join("\n");
  if (!tokens.every((token) => haystack.includes(token))) return 0;

  if (names.some((name) => name === normalizedQuery)) return 10_000;
  if (names.some((name) => name.startsWith(normalizedQuery))) return 9_000;
  if (names.some((name) => name.includes(normalizedQuery))) return 8_000;
  if (normalizedQuery.length >= 3 && path.includes(normalizedQuery)) return 6_000;
  return 7_000 + Math.min(tokens.length, 10) * 10;
}

function starterProjectRank(project: ProjectSummary) {
  let score = project.contextCount * 10;
  if (project.scanState === "scanned") score += 500;
  else if (project.scanState === "scanning") score += 250;
  if (project.isCurrent) score += 100;
  if (project.pinned) score += 100;
  return score;
}

function folderName(path: string) {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? path;
}

function kindRank(kind: string) {
  return kind === "project" ? 0 : 1;
}
