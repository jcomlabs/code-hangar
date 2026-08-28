export type ProjectCatalogScope = "current" | "all";
export type DocumentSearchScope = ProjectCatalogScope | "project";
export type DuplicateSearchScope = ProjectCatalogScope | "file";

export const DOCUMENT_SEARCH_DEFAULT_RESULTS = 20;
export const DOCUMENT_SEARCH_MAX_RESULTS = 500;
export const DOCUMENT_SEARCH_MAX_QUERY_CHARS = 256;
export const DOCUMENT_SEARCH_MAX_QUERY_TERMS = 16;
export const DOCUMENT_SEARCH_MATCH_EXPLANATION =
  "Every content term must match the same indexed document (AND); term order does not matter.";

export interface ProjectPickerCandidate {
  id: number;
  name: string;
  path: string;
  antigravityName?: string | null;
}

export type ProjectPickerResolution =
  | { kind: "empty"; project: null; matches: [] }
  | { kind: "none"; project: null; matches: [] }
  | { kind: "ambiguous"; project: null; matches: ProjectPickerCandidate[] }
  | { kind: "resolved"; project: ProjectPickerCandidate; matches: ProjectPickerCandidate[] };

interface DocumentSearchCriteria {
  query: string;
  scope: DocumentSearchScope;
  projectId: number | null;
  indexedKind: string;
  pathFilter: string;
  nameFilter: string;
  limit: number;
  includeFixtureProjects: boolean;
}

interface OrphanSearchCriteria {
  mode: "lost" | "assets";
  scope: ProjectCatalogScope;
  projectId: number | null;
  minPreset: string;
  customMiB: number;
  includePartial: boolean;
  stalePreset: string;
  signals: string[];
  keyword: string;
  assetKind: string;
  minConfidence: string;
  includeFixtureProjects: boolean;
}

interface DuplicateSearchCriteria {
  scope: DuplicateSearchScope;
  projectId: number | null;
  currentFileNodeId: number | null;
  minPreset: string;
  customMiB: number;
  fileKind: string;
  limit: number;
  includeFixtureProjects: boolean;
}

export function documentSearchCriteriaKey(criteria: DocumentSearchCriteria) {
  return JSON.stringify([
    "documents",
    normalizeSearchText(criteria.query),
    criteria.scope,
    criteria.scope === "current" || criteria.scope === "project" ? criteria.projectId : null,
    criteria.indexedKind,
    normalizeSearchText(criteria.pathFilter),
    normalizeSearchText(criteria.nameFilter),
    boundedDocumentSearchLimit(criteria.limit),
    criteria.includeFixtureProjects
  ]);
}

export function boundedDocumentSearchLimit(limit?: number | null) {
  if (limit == null) return DOCUMENT_SEARCH_DEFAULT_RESULTS;
  if (!Number.isFinite(limit)) return DOCUMENT_SEARCH_MAX_RESULTS;
  const integer = Math.trunc(limit);
  if (integer === 0) return DOCUMENT_SEARCH_MAX_RESULTS;
  return Math.min(Math.max(integer, 1), DOCUMENT_SEARCH_MAX_RESULTS);
}

export function documentSearchQueryTerms(query: string) {
  return query
    .split(/\s+/u)
    .map((token) => token.replace(/[^\p{L}\p{N}_.-]+/gu, ""))
    .filter(Boolean);
}

export function documentSearchQueryError(query: string): string | null {
  const characterCount = query.length > DOCUMENT_SEARCH_MAX_QUERY_CHARS * 2
    ? DOCUMENT_SEARCH_MAX_QUERY_CHARS + 1
    : Array.from(query).length;
  if (characterCount > DOCUMENT_SEARCH_MAX_QUERY_CHARS) {
    return `Use at most ${DOCUMENT_SEARCH_MAX_QUERY_CHARS} characters in Content keywords.`;
  }
  if (documentSearchQueryTerms(query).length > DOCUMENT_SEARCH_MAX_QUERY_TERMS) {
    return `Use at most ${DOCUMENT_SEARCH_MAX_QUERY_TERMS} content terms. Every term is combined with AND.`;
  }
  return null;
}

export function prepareDocumentSearchFilters<T extends { query: string; limit?: number | null }>(
  filters: T
): T & { limit: number } {
  const queryError = documentSearchQueryError(filters.query);
  if (queryError) throw new RangeError(queryError);
  return {
    ...filters,
    limit: boundedDocumentSearchLimit(filters.limit)
  };
}

export function orphanSearchCriteriaKey(criteria: OrphanSearchCriteria) {
  const common = [
    "orphans",
    criteria.mode,
    criteria.scope,
    criteria.scope === "current" ? criteria.projectId : null,
    criteria.minPreset,
    criteria.minPreset === "custom" ? criteria.customMiB : null,
    criteria.includePartial,
    criteria.includeFixtureProjects
  ];
  return JSON.stringify(criteria.mode === "lost"
    ? [
        ...common,
        criteria.stalePreset,
        [...new Set(criteria.signals)].sort(),
        normalizeSearchText(criteria.keyword)
      ]
    : [...common, criteria.assetKind, criteria.minConfidence]);
}

export function duplicateSearchCriteriaKey(criteria: DuplicateSearchCriteria) {
  return JSON.stringify([
    "duplicates",
    criteria.scope,
    criteria.scope === "current" ? criteria.projectId : null,
    criteria.scope === "file" ? criteria.currentFileNodeId : null,
    criteria.minPreset,
    criteria.minPreset === "custom" ? criteria.customMiB : null,
    criteria.fileKind,
    criteria.limit,
    criteria.includeFixtureProjects
  ]);
}

export function retainRunningDuplicateConfirmations<T extends { loading: boolean }>(
  confirmations: Record<string, T>
): Record<string, T> {
  return Object.fromEntries(
    Object.entries(confirmations).filter(([, confirmation]) => confirmation.loading)
  );
}

export function scopeForDiscoveryEntry(
  currentScope: ProjectCatalogScope,
  selectedProjectId: number | null
): ProjectCatalogScope;
export function scopeForDiscoveryEntry(
  currentScope: DuplicateSearchScope,
  selectedProjectId: number | null
): DuplicateSearchScope;
export function scopeForDiscoveryEntry(
  currentScope: DuplicateSearchScope,
  selectedProjectId: number | null
): DuplicateSearchScope {
  if (currentScope === "current" && selectedProjectId == null) return "all";
  return currentScope;
}

export function scopeForDocumentSearchEntry(
  currentScope: DocumentSearchScope,
  selectedProjectId: number | null
): DocumentSearchScope {
  if (currentScope === "current" && selectedProjectId == null) return "all";
  return currentScope;
}

/**
 * Resolve a typed project value without silently choosing the wrong project.
 * Only an exact normalized name, alias, folder name or path resolves. Partial
 * matches are returned for guidance, but never silently become the search scope.
 */
export function resolveProjectPickerInput(
  projects: readonly ProjectPickerCandidate[],
  input: string
): ProjectPickerResolution {
  const normalizedInput = normalizeProjectPickerText(input);
  if (!normalizedInput) return { kind: "empty", project: null, matches: [] };
  const compactInput = normalizedInput.replaceAll(" ", "");

  const indexed = projects.map((project) => ({
    project,
    fields: projectPickerFields(project)
  }));
  const exact = indexed.filter(({ fields }) => fields.some((field) => (
    field.normalized === normalizedInput || field.compact === compactInput
  )));
  if (exact.length === 1) {
    return { kind: "resolved", project: exact[0].project, matches: [exact[0].project] };
  }
  if (exact.length > 1) {
    return { kind: "ambiguous", project: null, matches: stableProjectPickerOrder(exact.map(({ project }) => project)) };
  }

  const tokens = normalizedInput.split(" ").filter(Boolean);
  const fuzzy = indexed.filter(({ fields }) => tokens.every((token) =>
    fields.some((field) => field.words.some((word) => word.startsWith(token)))
  ));
  if (fuzzy.length > 0) {
    return { kind: "ambiguous", project: null, matches: stableProjectPickerOrder(fuzzy.map(({ project }) => project)) };
  }
  return { kind: "none", project: null, matches: [] };
}

export function projectPickerInputStatus(resolution: ProjectPickerResolution): string | null {
  switch (resolution.kind) {
    case "empty":
      return "Type a project name, alias or local path.";
    case "none":
      return "No project matches this name, alias or local path.";
    case "ambiguous":
      return resolution.matches.length === 1
        ? "A project is close. Choose its exact name or local path."
        : `${resolution.matches.length} projects match. Type the exact name or local path.`;
    case "resolved":
      return null;
  }
}

function projectPickerFields(project: ProjectPickerCandidate) {
  return [project.name, project.antigravityName ?? "", folderName(project.path), project.path]
    .map((value) => normalizeProjectPickerText(value))
    .filter(Boolean)
    .map((normalized) => ({
      normalized,
      compact: normalized.replaceAll(" ", ""),
      words: normalized.split(" ").filter(Boolean)
    }));
}

function normalizeProjectPickerText(value: string) {
  return value
    .replace(/([\p{Ll}\d])(\p{Lu})/gu, "$1 $2")
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim()
    .replace(/\s+/g, " ");
}

function folderName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function stableProjectPickerOrder(projects: ProjectPickerCandidate[]) {
  return projects.sort((left, right) => left.name.localeCompare(right.name) || left.path.localeCompare(right.path));
}

function normalizeSearchText(value: string) {
  return value.trim().toLocaleLowerCase();
}
