import { sessionAppMeta } from "./app-meta";
import type { ProjectSummary, SessionDiscoveryCandidate } from "./types";

export type SessionSort = "recent" | "name";
export type SessionScope = "all" | "independent" | "projects";

export const SIDEBAR_SESSION_GROUP_ITEM_LIMIT = 12;
export const SIDEBAR_SESSION_SEARCH_ITEM_LIMIT = 5;
export const SIDEBAR_INDEPENDENT_SESSION_ITEM_LIMIT = 5;

export interface SidebarSessionGroups {
  projectGroups: { project: ProjectSummary; sessions: SessionDiscoveryCandidate[] }[];
  independent: SessionDiscoveryCandidate[];
  hermes: SessionDiscoveryCandidate[];
}

export interface DisplayedSidebarSessionGroups extends SidebarSessionGroups {
  count: number;
}

export interface CompactSidebarSessionGroups extends DisplayedSidebarSessionGroups {
  hiddenGroupCount: number;
  compacted: boolean;
}

export interface SidebarSessionItemPreview {
  visibleSessions: SessionDiscoveryCandidate[];
  hiddenCount: number;
  limit: number;
  canToggle: boolean;
}

interface SidebarSessionOptions {
  sort: SessionSort;
  appFilter: string;
  query?: string;
  scope?: SessionScope;
}

export function displayedSidebarSessionGroups(
  groups: SidebarSessionGroups,
  options: SidebarSessionOptions
): DisplayedSidebarSessionGroups {
  const sortSessions = (list: SessionDiscoveryCandidate[]) => {
    const copy = [...list];
    copy.sort((a, b) => {
      if (options.sort === "recent") {
        const diff = (b.modifiedMs ?? 0) - (a.modifiedMs ?? 0);
        if (diff !== 0) return diff;
      }
      return a.displayName.localeCompare(b.displayName);
    });
    return copy;
  };
  const filterSessions = (list: SessionDiscoveryCandidate[], project?: ProjectSummary) =>
    list.filter((session) => {
      if (options.appFilter !== "all" && sessionAppMeta(session).slug !== options.appFilter) return false;
      return sessionMatchesSidebarQuery(session, options.query ?? "", project);
    });
  const prep = (list: SessionDiscoveryCandidate[], project?: ProjectSummary) => sortSessions(filterSessions(list, project));
  const latestMs = (list: SessionDiscoveryCandidate[]) =>
    list.reduce((max, session) => Math.max(max, session.modifiedMs ?? 0), 0);
  const scope = options.scope ?? "all";
  const projectGroups = scope === "independent"
    ? []
    : groups.projectGroups
      .map((group) => ({ project: group.project, sessions: prep(group.sessions, group.project) }))
      .filter((group) => group.sessions.length > 0);
  if (options.sort === "recent") {
    projectGroups.sort((a, b) => latestMs(b.sessions) - latestMs(a.sessions));
  }
  const independent = scope === "projects" ? [] : prep(groups.independent);
  const hermes = scope === "all" ? prep(groups.hermes) : [];
  const count = projectGroups.reduce((sum, group) => sum + group.sessions.length, 0) + independent.length + hermes.length;
  return { projectGroups, independent, hermes, count };
}

export function previewSidebarSessionItems(
  sessions: SessionDiscoveryCandidate[],
  options: { searchActive?: boolean; showAll?: boolean; itemLimit?: number } = {}
): SidebarSessionItemPreview {
  const limit = options.searchActive
    ? SIDEBAR_SESSION_SEARCH_ITEM_LIMIT
    : Math.max(1, Math.floor(options.itemLimit ?? SIDEBAR_SESSION_GROUP_ITEM_LIMIT));
  if (options.showAll || sessions.length <= limit) {
    return {
      visibleSessions: sessions,
      hiddenCount: 0,
      limit,
      canToggle: sessions.length > limit
    };
  }
  return {
    visibleSessions: sessions.slice(0, limit),
    hiddenCount: sessions.length - limit,
    limit,
    canToggle: true
  };
}

export function compactSidebarSessionGroups(
  groups: DisplayedSidebarSessionGroups,
  limit: number
): CompactSidebarSessionGroups {
  const normalizedLimit = Math.max(0, Math.floor(limit));
  if (groups.projectGroups.length <= normalizedLimit) {
    return { ...groups, hiddenGroupCount: 0, compacted: false };
  }
  return {
    projectGroups: groups.projectGroups.slice(0, normalizedLimit),
    independent: groups.independent,
    hermes: groups.hermes,
    count: groups.count,
    hiddenGroupCount: groups.projectGroups.length - normalizedLimit,
    compacted: true
  };
}

export function sessionMatchesSidebarQuery(
  session: SessionDiscoveryCandidate,
  query: string,
  project?: ProjectSummary
) {
  const tokens = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return true;
  const meta = sessionAppMeta(session);
  const haystack = [
    session.displayName,
    session.path,
    session.sourceKind,
    session.sourceLabel,
    session.sessionKind,
    session.confidence,
    session.association,
    ...session.linkedProjectPaths,
    meta.slug,
    meta.label,
    project?.name ?? "",
    project?.path ?? ""
  ].join("\n").toLocaleLowerCase();
  return tokens.every((token) => haystack.includes(token));
}
