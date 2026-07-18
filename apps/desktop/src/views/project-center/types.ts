import type { MouseEvent, PointerEvent as ReactPointerEvent } from "react";

import type { NavItem } from "../../types";

export type ProjectView = "context" | "recap" | "files" | "space" | "connections" | "sessions";
// Re-exported from the shared types so there is a single PreviewMode definition (incl. the
// frontend-only "edit" and "values" views); api.ts maps both to "source" at the backend boundary.
export type { PreviewMode } from "../../types";

// File-tree sorting. `key` picks the property; `dir` the direction. The "default"
// key means the curated inventory order (context files first, then by priority) —
// the off-state you return to after cycling a sort pill, so `dir` is ignored there.
export type TreeSortKey = "default" | "name" | "date" | "size";
export type TreeSortDir = "asc" | "desc";
export interface TreeSort {
  key: TreeSortKey;
  dir: TreeSortDir;
}
// Natural first-press direction per key: names read A→Z, but dates and sizes are
// most useful newest/biggest-first.
export const DEFAULT_TREE_SORT_DIR: Record<Exclude<TreeSortKey, "default">, TreeSortDir> = {
  name: "asc",
  date: "desc",
  size: "desc"
};
export const DEFAULT_TREE_SORT: TreeSort = { key: "default", dir: "desc" };

// Everything the Edit toggle + in-pane editor need, bundled so it threads as one prop.
export interface EditorBinding {
  /** Whether this file is editable in this edition (Local/Connector, text, not truncated/sensitive). */
  available: boolean;
  /** The editable buffer; null when not currently editing. */
  draft: string | null;
  saving: boolean;
  /** Unsaved changes vs the on-disk baseline. */
  dirty: boolean;
  /** A prior Save can be reverted on disk. */
  canUndo: boolean;
  onChange: (value: string) => void;
  /** Applies the exact draft represented by the reviewed after-hash. */
  onSave: (reviewedAfterHash: string) => Promise<boolean>;
  onRevert: () => void;
  onUndo: () => void;
}

export interface OpenTab {
  nodeId: number;
  projectId: number;
  label: string;
  path: string;
}

export interface TreePage {
  items: NavItem[];
  total: number;
  hasMore: boolean;
  nextOffset?: number;
}

export interface TabStripProps {
  tabs: OpenTab[];
  activeNodeId: number | null;
  draggedTabNodeId: number | null;
  tabDropTargetNodeId: number | null;
  showTabMenu: (tab: OpenTab, event: MouseEvent<HTMLElement>) => void;
  suppressNextTabClick: () => boolean;
  openNode: (nodeId: number) => void;
  startTabPointerDrag: (tab: OpenTab, event: ReactPointerEvent<HTMLButtonElement>) => void;
  closeTab: (nodeId: number) => void;
}
