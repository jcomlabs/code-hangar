export type DocumentSearchScope = "current" | "all";
export type DuplicateSearchScope = "file" | DocumentSearchScope;

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
  scope: DocumentSearchScope;
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
    criteria.scope === "current" ? criteria.projectId : null,
    criteria.indexedKind,
    normalizeSearchText(criteria.pathFilter),
    normalizeSearchText(criteria.nameFilter),
    criteria.limit,
    criteria.includeFixtureProjects
  ]);
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
  currentScope: DocumentSearchScope,
  selectedProjectId: number | null
): DocumentSearchScope;
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
  return scopeForDiscoveryEntry(currentScope, selectedProjectId);
}

function normalizeSearchText(value: string) {
  return value.trim().toLocaleLowerCase();
}
