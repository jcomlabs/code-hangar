import { useCallback, useReducer, useRef } from "react";
import { api } from "./api";
import type { ContextFile, GitRepoSummary, NavItem } from "./types";

export interface ProjectNavPage {
  items: NavItem[];
  total: number;
  hasMore: boolean;
  /** Backend offset for the next page. Injected reveal-path rows do not advance it. */
  nextOffset?: number;
}

// Upper bound on the first project nav-tree read before the load is treated as failed. Generous
// so a genuinely slow (but progressing) read on a large local DB still succeeds; short enough that
// a stalled read surfaces an actionable error + Retry instead of an indefinite spinner.
const PROJECT_LOAD_TIMEOUT_MS = 30000;

export type ProjectLoadStatus = "idle" | "loading" | "ready" | "error";
export type SelectedProjectActivation = "reuse" | "wait" | "reload";

export function selectedProjectActivation(status: ProjectLoadStatus): SelectedProjectActivation {
  if (status === "ready") return "reuse";
  if (status === "loading") return "wait";
  return "reload";
}

export interface ProjectWorkspaceState {
  activeProjectId: number | null;
  generation: number;
  loadStatus: ProjectLoadStatus;
  error: string | null;
  treePages: Record<string, ProjectNavPage>;
  expandedTree: Set<number>;
  treeLoading: Set<string>;
  contextFiles: ContextFile[];
  gitStatus: GitRepoSummary | null;
}

export type ProjectWorkspaceAction =
  | { type: "begin"; projectId: number | null; generation: number }
  | { type: "load-start"; generation: number }
  | {
      type: "root-success";
      projectId: number;
      generation: number;
      rootPage: ProjectNavPage;
      resetExpansion: boolean;
    }
  | {
      type: "load-success";
      projectId: number;
      generation: number;
      rootPage: ProjectNavPage;
      contextFiles: ContextFile[];
      gitStatus: GitRepoSummary;
      resetExpansion: boolean;
    }
  | {
      type: "side-data-success";
      projectId: number;
      generation: number;
      contextFiles: ContextFile[];
      gitStatus: GitRepoSummary;
    }
  | { type: "load-error"; projectId: number; generation: number; error: string }
  | { type: "tree-loading"; key: string; loading: boolean }
  | { type: "tree-page"; key: string; page: ProjectNavPage; append: boolean }
  | { type: "reveal-path"; projectId: number; generation: number; path: NavItem[] }
  | { type: "toggle-expanded"; navId: number };

export const initialProjectWorkspaceState: ProjectWorkspaceState = {
  activeProjectId: null,
  generation: 0,
  loadStatus: "idle",
  error: null,
  treePages: {},
  expandedTree: new Set(),
  treeLoading: new Set(),
  contextFiles: [],
  gitStatus: null
};

export function projectWorkspaceReducer(
  state: ProjectWorkspaceState,
  action: ProjectWorkspaceAction
): ProjectWorkspaceState {
  switch (action.type) {
    case "begin":
      return {
        ...initialProjectWorkspaceState,
        activeProjectId: action.projectId,
        generation: action.generation,
        loadStatus: action.projectId === null ? "idle" : "loading"
      };
    case "load-start":
      return {
        ...state,
        generation: action.generation,
        loadStatus: "loading",
        error: null
      };
    case "root-success":
      // The active project alone is not enough: two refreshes of the same project may overlap.
      // Only the newest generation may replace the tree, otherwise a slower old response can
      // overwrite fresher scan results.
      if (state.activeProjectId !== action.projectId || state.generation !== action.generation) return state;
      return {
        ...state,
        loadStatus: "ready",
        error: null,
        treePages: { root: action.rootPage },
        expandedTree: action.resetExpansion ? new Set() : state.expandedTree,
        treeLoading: new Set(),
        contextFiles: [],
        gitStatus: null
      };
    case "side-data-success":
      if (state.activeProjectId !== action.projectId || state.generation !== action.generation) return state;
      return {
        ...state,
        contextFiles: action.contextFiles,
        gitStatus: action.gitStatus
      };
    case "load-success":
      if (state.activeProjectId !== action.projectId || state.generation !== action.generation) return state;
      return {
        ...state,
        loadStatus: "ready",
        error: null,
        treePages: { root: action.rootPage },
        expandedTree: action.resetExpansion ? new Set() : state.expandedTree,
        treeLoading: new Set(),
        contextFiles: action.contextFiles,
        gitStatus: action.gitStatus
      };
    case "load-error":
      // Never let an old timeout clobber a newer same-project load.
      if (
        state.activeProjectId !== action.projectId
        || state.generation !== action.generation
        || state.loadStatus !== "loading"
      ) return state;
      return {
        ...state,
        loadStatus: "error",
        error: action.error,
        treeLoading: new Set()
      };
    case "tree-loading": {
      const treeLoading = new Set(state.treeLoading);
      if (action.loading) treeLoading.add(action.key);
      else treeLoading.delete(action.key);
      return { ...state, treeLoading };
    }
    case "tree-page": {
      const previous = action.append ? state.treePages[action.key]?.items ?? [] : [];
      const merged = new Map(previous.map((item) => [item.id, item]));
      for (const item of action.page.items) merged.set(item.id, item);
      return {
        ...state,
        treePages: {
          ...state.treePages,
          [action.key]: { ...action.page, items: [...merged.values()] }
        }
      };
    }
    case "reveal-path": {
      if (state.activeProjectId !== action.projectId || state.generation !== action.generation) return state;
      const treePages = { ...state.treePages };
      const expandedTree = new Set(state.expandedTree);
      action.path.forEach((item, index) => {
        const key = item.parentNavId == null ? "root" : String(item.parentNavId);
        const page = treePages[key];
        const items = page?.items ? [...page.items] : [];
        const existingIndex = items.findIndex((candidate) => candidate.id === item.id);
        if (existingIndex >= 0) items[existingIndex] = item;
        else items.push(item);
        const parent = index > 0 ? action.path[index - 1] : null;
        const total = Math.max(page?.total ?? parent?.childCount ?? items.length, items.length);
        treePages[key] = {
          items,
          total,
          hasMore: page?.hasMore ?? total > items.length,
          nextOffset: page?.nextOffset ?? 0
        };
        if (item.itemKind === "directory") expandedTree.add(item.id);
      });
      return { ...state, treePages, expandedTree };
    }
    case "toggle-expanded": {
      const expandedTree = new Set(state.expandedTree);
      if (expandedTree.has(action.navId)) expandedTree.delete(action.navId);
      else expandedTree.add(action.navId);
      return { ...state, expandedTree };
    }
  }
}

function yieldProjectWork() {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, 0);
  });
}

export function useProjectWorkspace() {
  const [state, dispatch] = useReducer(projectWorkspaceReducer, initialProjectWorkspaceState);
  const generationRef = useRef(0);
  const activeProjectRef = useRef<number | null>(null);

  const beginProject = useCallback((projectId: number | null) => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    activeProjectRef.current = projectId;
    dispatch({ type: "begin", projectId, generation });
    return generation;
  }, []);

  const loadProjectData = useCallback(
    async (projectId: number, resetExpansion = true) => {
      // ANTI-HANG INVARIANT: the reducer drops any success/error whose generation is stale, so
      // every generation bump here MUST be followed by a load that dispatches a matching
      // root-success/load-error for that generation. Both branches below uphold it — beginProject
      // bumps then this fn proceeds to dispatch, and the else-branch bumps via load-start then
      // proceeds. A bare generation bump with no completing dispatch would leave loadStatus stuck
      // on "loading" forever (the spinner hang). Keep that pairing intact.
      let generation = generationRef.current;
      if (activeProjectRef.current !== projectId) {
        generation = beginProject(projectId);
      } else {
        generation += 1;
        generationRef.current = generation;
        dispatch({ type: "load-start", generation });
      }

      try {
        // Guard against a never-resolving load: on a very large local inventory (a multi-GB
        // encrypted DB with a big WAL) the first nav-tree read can be extremely slow, and without
        // a bound the view spins on "Loading the project root…" forever with no feedback. Time it
        // out so the error state (with its Retry) is reachable instead.
        // Capture the timer handle and clear it once the race settles, so the common (fast) path
        // does not leak a 30s pending timer per project load.
        let timeoutHandle: ReturnType<typeof setTimeout> | undefined;
        const rootResult = await Promise.race([
          api.projectNavChildren(projectId, null, 200, 0),
          new Promise<never>((_, reject) => {
            timeoutHandle = setTimeout(
              () =>
                reject(
                  new Error(
                    "Loading timed out — the local inventory is very large. Try again, or run a focused rescan."
                  )
                ),
              PROJECT_LOAD_TIMEOUT_MS
            );
          })
        ]).finally(() => clearTimeout(timeoutHandle));
        const rootPage: ProjectNavPage = { ...rootResult, nextOffset: rootResult.items.length };
        if (activeProjectRef.current !== projectId || generationRef.current !== generation) return false;
        dispatch({
          type: "root-success",
          projectId,
          generation,
          rootPage,
          resetExpansion
        });

        await yieldProjectWork();
        const contextFiles = await api.projectContextFiles(projectId);
        await yieldProjectWork();
        const gitStatus = await api.projectGitStatus(projectId);
        if (activeProjectRef.current !== projectId || generationRef.current !== generation) return true;
        dispatch({
          type: "side-data-success",
          projectId,
          generation,
          contextFiles,
          gitStatus
        });
        return true;
      } catch (error) {
        if (activeProjectRef.current !== projectId || generationRef.current !== generation) return false;
        dispatch({
          type: "load-error",
          projectId,
          generation,
          error: error instanceof Error ? error.message : String(error)
        });
        return false;
      }
    },
    [beginProject]
  );

  const loadTreeChildren = useCallback(
    async (
      projectId: number,
      parentNavId: number | null,
      options?: { append?: boolean; offset?: number }
    ) => {
      if (activeProjectRef.current !== projectId) return "The selected project changed before this folder loaded.";
      const generation = generationRef.current;
      const key = parentNavId === null ? "root" : String(parentNavId);
      dispatch({ type: "tree-loading", key, loading: true });
      try {
        const offset = options?.offset ?? 0;
        const result = await api.projectNavChildren(projectId, parentNavId, 200, offset);
        const page: ProjectNavPage = { ...result, nextOffset: offset + result.items.length };
        if (activeProjectRef.current !== projectId || generationRef.current !== generation) return null;
        dispatch({ type: "tree-page", key, page, append: options?.append ?? false });
        return null;
      } catch (error) {
        return error instanceof Error ? error.message : "Could not load tree children.";
      } finally {
        if (activeProjectRef.current === projectId && generationRef.current === generation) {
          dispatch({ type: "tree-loading", key, loading: false });
        }
      }
    },
    []
  );

  const toggleExpanded = useCallback((navId: number) => {
    dispatch({ type: "toggle-expanded", navId });
  }, []);

  const revealNode = useCallback(async (projectId: number, nodeId: number) => {
    if (activeProjectRef.current !== projectId) return false;
    const generation = generationRef.current;
    const path = await api.projectNavPath(projectId, nodeId);
    if (activeProjectRef.current !== projectId || generationRef.current !== generation || path.length === 0) {
      return false;
    }
    dispatch({ type: "reveal-path", projectId, generation, path });
    return true;
  }, []);

  return {
    state,
    beginProject,
    loadProjectData,
    loadTreeChildren,
    toggleExpanded,
    revealNode
  };
}
