import type { ProjectSummary } from "../types";

export const ORGANIZE_MODEL_LOCATION_PREVIEW = 8;
export const ORGANIZE_MODEL_FILE_PREVIEW = 8;

export function organizeDisclosureItems<T>(items: T[], showAll: boolean, previewLimit: number): T[] {
  return showAll ? items : items.slice(0, previewLimit);
}

export function organizeModelResultCounts(result: { candidates: readonly unknown[]; total: number } | null) {
  const shown = result?.candidates.length ?? 0;
  const total = Math.max(shown, result?.total ?? 0);
  return { shown, total, limited: total > shown };
}

export function organizeInventoryFingerprint(
  projects: readonly Pick<ProjectSummary, "id" | "path" | "source" | "contextCount" | "scanState">[]
): string {
  const inventoryRows = projects
    .filter((project) => project.source !== "fixture")
    .map((project) => [project.id, project.path, project.scanState, project.contextCount] as const)
    .sort((left, right) => left[0] - right[0] || left[1].localeCompare(right[1]));
  return JSON.stringify(inventoryRows);
}

export function organizeProjectReviewReason(
  project: Pick<ProjectSummary, "contextCount" | "isCurrent">
): string | null {
  // Currentness is unavailable for some apps and unwalked roots, so its absence
  // is not enough to call a project inactive.
  if ((project.contextCount ?? 0) === 0 && !project.isCurrent) return "no context, no recognized activity signal";
  if ((project.contextCount ?? 0) === 0) return "no context files";
  return null;
}

export function splitProjectsForLocationReview<T extends Pick<ProjectSummary, "contextCount" | "isCurrent">>(
  items: T[]
) {
  const needsReview: T[] = [];
  const mapped: T[] = [];
  for (const item of items) {
    if (organizeProjectReviewReason(item)) needsReview.push(item);
    else mapped.push(item);
  }
  return { needsReview, mapped };
}

export function organizeProjectReviewSummary(
  projects: readonly Pick<ProjectSummary, "source" | "contextCount" | "isCurrent">[]
) {
  const realProjects = projects.filter((project) => project.source !== "fixture");
  const needsReview = realProjects.filter((project) => organizeProjectReviewReason(project)).length;
  const mapped = realProjects.length - needsReview;
  return {
    total: realProjects.length,
    mapped,
    needsReview,
    progress: realProjects.length > 0 ? Math.round((mapped / realProjects.length) * 100) : 0
  };
}
