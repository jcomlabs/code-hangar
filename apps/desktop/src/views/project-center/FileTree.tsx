import { memo, useMemo, useState } from "react";
import type { MouseEvent, ReactNode } from "react";
import {
  ArrowDown,
  ArrowUp,
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  Clock,
  Database,
  FileArchive,
  FileAudio,
  FileCode,
  FileCog,
  FileImage,
  FileJson,
  FileQuestion,
  FileSpreadsheet,
  FileText,
  FileType,
  FileVideo,
  Files,
  Folder,
  HardDrive,
  Lock,
  RefreshCcw,
  Search,
  Sparkles,
  X
} from "lucide-react";

import { aiToolFile } from "../../ai-tool-files";
import { ConceptHelp } from "../../BeginnerHelp";
import type { NavItem } from "../../types";
import { SectionTitle, formatBytes } from "../../ui";
import { DEFAULT_TREE_SORT, DEFAULT_TREE_SORT_DIR } from "./types";
import type { TreePage, TreeSort, TreeSortKey } from "./types";

// The three optional sort keys, in toolbar order. "default" (curated inventory
// order) is the off-state you return to by cycling a pill past its two directions.
const TREE_SORT_PILLS: { key: Exclude<TreeSortKey, "default">; label: string; icon: ReactNode; help: string }[] = [
  {
    key: "name",
    label: "Name",
    icon: <CaseSensitive size={13} />,
    help: "Sort the file tree alphabetically (A–Z). Click again to reverse (Z–A); a third click restores the curated order."
  },
  {
    key: "date",
    label: "Date",
    icon: <Clock size={13} />,
    help: "Sort the file tree by last-modified date, newest first. Click again for oldest first; a third click restores the curated order."
  },
  {
    key: "size",
    label: "Size",
    icon: <HardDrive size={13} />,
    help: "Sort by the size shown in the tree: file length for files, estimated footprint for folders. Click again for smallest first; a third click restores the curated order."
  }
];

export interface FileTreeEmptyCopy {
  kind: "empty" | "filter" | "hidden";
  title: string;
  body: string;
  action: "clear-filter" | "show-all" | null;
}

export function fileTreeEmptyCopy({
  rootItemCount,
  visibleRootCount,
  showFolders,
  showFiles,
  query
}: {
  rootItemCount: number;
  visibleRootCount: number;
  showFolders: boolean;
  showFiles: boolean;
  query: string;
}): FileTreeEmptyCopy | null {
  if (visibleRootCount > 0) return null;
  if (!showFolders && !showFiles) {
    return {
      kind: "hidden",
      title: "Nothing is visible",
      body: "Folders and files are both hidden.",
      action: "show-all"
    };
  }
  const normalizedQuery = query.trim();
  if (normalizedQuery) {
    return {
      kind: "filter",
      title: "No loaded matches",
      body: `No loaded file or folder matches "${normalizedQuery}".`,
      action: "clear-filter"
    };
  }
  if (rootItemCount === 0) {
    return {
      kind: "empty",
      title: "No tree items loaded",
      body: "This project has no inventoried files or folders yet.",
      action: null
    };
  }
  return {
    kind: "hidden",
    title: "No visible items",
    body: "The current folder and file visibility choices hide every root item.",
    action: "show-all"
  };
}

export const ProjectFileTree = memo(function ProjectFileTree({
  rootTreeItems,
  activeNodeId,
  expandedTree,
  treePages,
  treeLoading,
  toggleExpandedTree,
  loadTreeChildren,
  continueSubtreeScan,
  explainFolder,
  showTreeMenu,
  showProtectedMetadata,
  openNode
}: {
  rootTreeItems: NavItem[];
  activeNodeId: number | null;
  expandedTree: Set<number>;
  treePages: Record<string, TreePage>;
  treeLoading: Set<string>;
  toggleExpandedTree: (navId: number) => void;
  loadTreeChildren: (parentNavId: number | null, options?: { append?: boolean; offset?: number }) => void | Promise<unknown>;
  continueSubtreeScan: (navId: number) => void;
  explainFolder: (item: NavItem) => void;
  showTreeMenu: (item: NavItem, event: MouseEvent<HTMLElement>) => void;
  showProtectedMetadata: boolean;
  openNode: (nodeId: number) => void;
}) {
  const [treeShowFolders, setTreeShowFolders] = useState(true);
  const [treeShowFiles, setTreeShowFiles] = useState(true);
  const [treeSort, setTreeSort] = useState<TreeSort>(DEFAULT_TREE_SORT);
  const [treeFilter, setTreeFilter] = useState("");
  // When a filter is active, keep a node if its own name matches OR a loaded
  // descendant matches — so the path to each match stays visible. Operates on
  // already-loaded nodes only; deep matches in unloaded folders need Quick Open.
  const filterKeep = useMemo(
    () => (treeFilter.trim() ? buildTreeFilterKeep(rootTreeItems, treePages, treeFilter) : null),
    [treeFilter, rootTreeItems, treePages]
  );
  const sortedRootTreeItems = useMemo(
    () => visibleTreeItems(rootTreeItems, treeShowFolders, treeShowFiles, treeSort, filterKeep),
    [filterKeep, rootTreeItems, treeShowFiles, treeShowFolders, treeSort]
  );
  const emptyCopy = fileTreeEmptyCopy({
    rootItemCount: rootTreeItems.length,
    visibleRootCount: sortedRootTreeItems.length,
    showFolders: treeShowFolders,
    showFiles: treeShowFiles,
    query: treeFilter
  });
  // Collapse every currently-open folder (reuses the per-node toggle, so no extra
  // state plumbing); a one-click way to reset orientation after drilling deep.
  const collapseAllTree = () => {
    for (const navId of expandedTree) toggleExpandedTree(navId);
  };

  // One pill = one 3-state cycle: off → primary direction → reversed → off (curated).
  const cycleTreeSort = (key: Exclude<TreeSortKey, "default">) => {
    setTreeSort((current) => {
      const primary = DEFAULT_TREE_SORT_DIR[key];
      if (current.key !== key) return { key, dir: primary };
      if (current.dir === primary) return { key, dir: primary === "asc" ? "desc" : "asc" };
      return DEFAULT_TREE_SORT;
    });
  };

  return (
    <div className="file-tree">
      <div className="file-tree-header">
        <SectionTitle icon={<Database size={15} />} label="File Tree" trailing={<ConceptHelp concept="fileTree" />} />
        <div className="tree-toolbar" aria-label="File tree display and sort controls">
          <button className={treeShowFolders ? "active" : ""} type="button" onClick={() => setTreeShowFolders((value) => !value)} data-help="Show or hide folders in the file tree. Enabled by default.">
            <Folder size={13} /> Folders
          </button>
          <button className={treeShowFiles ? "active" : ""} type="button" onClick={() => setTreeShowFiles((value) => !value)} data-help="Show or hide files in the file tree. Enabled by default.">
            <Files size={13} /> Files
          </button>
          <span className="tree-toolbar-divider" aria-hidden="true" />
          <span className="tree-toolbar-label">Sort</span>
          {TREE_SORT_PILLS.map((pill) => {
            const active = treeSort.key === pill.key;
            return (
              <button
                key={pill.key}
                className={`tree-sort-pill${active ? " active" : ""}`}
                type="button"
                aria-pressed={active}
                onClick={() => cycleTreeSort(pill.key)}
                data-help={pill.help}
              >
                {pill.icon} {pill.label}
                {active ? (
                  treeSort.dir === "asc"
                    ? <ArrowUp className="tree-sort-dir" size={12} />
                    : <ArrowDown className="tree-sort-dir" size={12} />
                ) : null}
              </button>
            );
          })}
        </div>
        <div className="tree-filter">
          <Search className="tree-filter-icon" size={13} aria-hidden="true" />
          <input
            type="text"
            className="tree-filter-input"
            value={treeFilter}
            placeholder="Filter loaded files…"
            aria-label="Filter the file tree by name"
            spellCheck={false}
            data-help="Type to filter the loaded file tree by name; matching files and the folders that contain them stay visible. Deep matches in unopened folders need Quick Open (Ctrl+P)."
            onChange={(event) => setTreeFilter(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && treeFilter) {
                event.preventDefault();
                setTreeFilter("");
              }
            }}
          />
          {treeFilter ? (
            <button className="tree-filter-clear" type="button" aria-label="Clear the file tree filter" data-help="Clear the file tree filter." onClick={() => setTreeFilter("")}>
              <X size={13} />
            </button>
          ) : null}
          <button className="tree-filter-collapse" type="button" aria-label="Collapse all folders" data-help="Collapse every open folder in the file tree." onClick={collapseAllTree} disabled={expandedTree.size === 0}>
            <ChevronsDownUp size={13} />
          </button>
        </div>
      </div>
      {sortedRootTreeItems.length ? (
        <Tree
          items={sortedRootTreeItems}
          activeNodeId={activeNodeId}
          expanded={expandedTree}
          filterKeep={filterKeep}
          pages={treePages}
          loading={treeLoading}
          onToggle={async (item) => {
            toggleExpandedTree(item.id);
            if (!expandedTree.has(item.id) && !treePages[String(item.id)]) {
              await loadTreeChildren(item.id);
            }
          }}
          onLoadMore={(parentNavId, currentCount) => { void loadTreeChildren(parentNavId, { append: true, offset: currentCount }); }}
          onContinueScan={(item) => void continueSubtreeScan(item.id)}
          onExplainFolder={(item) => void explainFolder(item)}
          onContextMenu={showTreeMenu}
          showProtectedMetadata={showProtectedMetadata}
          onOpen={openNode}
          showFolders={treeShowFolders}
          showFiles={treeShowFiles}
          sort={treeSort}
        />
      ) : emptyCopy ? (
        <div className={`file-tree-empty ${emptyCopy.kind}`} role="status">
          <FileQuestion size={20} />
          <strong>{emptyCopy.title}</strong>
          <span>{emptyCopy.body}</span>
          {emptyCopy.action === "clear-filter" ? (
            <button className="secondary-button compact" type="button" onClick={() => setTreeFilter("")}>Clear filter</button>
          ) : emptyCopy.action === "show-all" ? (
            <button className="secondary-button compact" type="button" onClick={() => {
              setTreeShowFolders(true);
              setTreeShowFiles(true);
            }}>Show all</button>
          ) : null}
        </div>
      ) : null}
      {treePages.root?.hasMore ? (
        <button className="tree-more" type="button" data-help="Load more root-level file tree items." onClick={() => void loadTreeChildren(null, { append: true, offset: treePages.root?.nextOffset ?? treePages.root?.items.length ?? 0 })}>
          Show next root items
        </button>
      ) : null}
    </div>
  );
});

export const Tree = memo(function Tree({
  items,
  activeNodeId,
  expanded,
  filterKeep,
  pages,
  loading,
  onToggle,
  onLoadMore,
  onContinueScan,
  onExplainFolder,
  onContextMenu,
  showProtectedMetadata,
  onOpen,
  showFolders,
  showFiles,
  sort
}: {
  items: NavItem[];
  activeNodeId: number | null;
  expanded: Set<number>;
  /** When non-null, only nav ids in this set render (a name match or an ancestor of one). */
  filterKeep: Set<number> | null;
  pages: Record<string, TreePage>;
  loading: Set<string>;
  onToggle: (item: NavItem) => void | Promise<void>;
  onLoadMore: (parentNavId: number, currentCount: number) => void | Promise<void>;
  onContinueScan: (item: NavItem) => void;
  onExplainFolder: (item: NavItem) => void;
  onContextMenu: (item: NavItem, event: MouseEvent<HTMLElement>) => void;
  showProtectedMetadata: boolean;
  onOpen: (nodeId: number) => void;
  showFolders: boolean;
  showFiles: boolean;
  sort: TreeSort;
}) {
  const visibleItems = useMemo(
    () => visibleTreeItems(items, showFolders, showFiles, sort, filterKeep),
    [items, showFiles, showFolders, sort, filterKeep]
  );
  return (
    <div className="tree">
      {visibleItems.map((item) => (
        <TreeNode
          key={item.id}
          item={item}
          activeNodeId={activeNodeId}
          expanded={expanded}
          filterKeep={filterKeep}
          pages={pages}
          loading={loading}
          onToggle={onToggle}
          onLoadMore={onLoadMore}
          onContinueScan={onContinueScan}
          onExplainFolder={onExplainFolder}
          onContextMenu={onContextMenu}
          showProtectedMetadata={showProtectedMetadata}
          onOpen={onOpen}
          showFolders={showFolders}
          showFiles={showFiles}
          sort={sort}
        />
      ))}
    </div>
  );
});

const TreeNode = memo(function TreeNode({
  item,
  activeNodeId,
  expanded,
  filterKeep,
  pages,
  loading,
  onToggle,
  onLoadMore,
  onContinueScan,
  onExplainFolder,
  onContextMenu,
  showProtectedMetadata,
  onOpen,
  showFolders,
  showFiles,
  sort
}: {
  item: NavItem;
  activeNodeId: number | null;
  expanded: Set<number>;
  filterKeep: Set<number> | null;
  pages: Record<string, TreePage>;
  loading: Set<string>;
  onToggle: (item: NavItem) => void | Promise<void>;
  onLoadMore: (parentNavId: number, currentCount: number) => void | Promise<void>;
  onContinueScan: (item: NavItem) => void;
  onExplainFolder: (item: NavItem) => void;
  onContextMenu: (item: NavItem, event: MouseEvent<HTMLElement>) => void;
  showProtectedMetadata: boolean;
  onOpen: (nodeId: number) => void;
  showFolders: boolean;
  showFiles: boolean;
  sort: TreeSort;
}) {
  // While filtering, force-open the path to matches (without mutating the real
  // expanded state) so results are visible; otherwise honor the user's toggles.
  const isOpen = filterKeep ? filterKeep.has(item.id) : expanded.has(item.id);
  const fileVisual = item.itemKind === "directory" ? null : fileVisualForName(item.displayName || item.path);
  const aiTool = item.itemKind === "directory" ? null : aiToolFile(item.path || item.displayName);
  const labelClassName = [
    "tree-label",
    fileVisual?.previewable ? "tree-file-previewable" : "",
    fileVisual ? `tree-file-${fileVisual.kind}` : ""
  ].filter(Boolean).join(" ");
  const rowHelp = item.itemKind === "directory"
    ? `File tree folder ${item.displayName}. Click to explain and expand it; right-click for safe actions. Path: ${item.displayPath || item.path}.`
    : `File tree file ${item.displayName}. ${fileVisual?.previewable ? "Usually viewable as text inside Code Hangar." : "May need the Windows default app if Code Hangar cannot preview it."} Right-click for safe actions. Path: ${item.displayPath || item.path}.`;
  const childItems = pages[String(item.id)]?.items ?? item.children;
  const childLoading = loading.has(String(item.id));

  return (
    <div className="tree-node">
      <div className={`tree-row ${item.nodeId === activeNodeId ? "selected" : ""} ${showProtectedMetadata && (item.isSensitive || item.protectedLevel) ? "protected-visible" : ""}`} data-tree-node-id={item.nodeId ?? undefined} data-help={rowHelp} onContextMenu={(event) => onContextMenu(item, event)}>
        {item.itemKind === "directory" ? (
          <button className="tree-chevron" type="button" onClick={() => void onToggle(item)} aria-label={isOpen ? `Collapse ${item.displayName}` : `Expand ${item.displayName}`} data-help={`${isOpen ? "Collapse" : "Expand"} folder ${item.displayName}.`}>
            {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          </button>
        ) : (
          <span className="tree-spacer" />
        )}
        <button
          className={labelClassName}
          type="button"
          data-help={item.itemKind === "directory" ? `Explain and expand folder ${item.displayName}.` : `File tree: ${fileVisual?.previewable ? "view" : "select"} ${item.displayName}. ${fileVisual?.description ?? "Unknown file type."}`}
          onClick={() => {
            if (item.itemKind === "directory") {
              onExplainFolder(item);
              void onToggle(item);
              return;
            }
            if (item.nodeId) onOpen(item.nodeId);
          }}
        >
          {item.itemKind === "directory" ? <Folder size={14} /> : fileVisual?.icon ?? <FileQuestion size={14} />}
          <span>{item.displayName}</span>
        </button>
        <span className="tree-meta">
          {treeItemDisplaySize(item) != null ? (
            <span className="tree-size" data-help={treeItemSizeHelp(item)}>
              {formatBytes(treeItemDisplaySize(item) ?? 0)}{item.itemKind === "directory" && item.aggregateBytesPartial ? "+" : ""}
            </span>
          ) : null}
          {!item.fullyScanned || item.scanError ? <RefreshCcw size={12} /> : null}
          {aiTool ? <span className="tree-aitool" title={`${aiTool.tool} · ${aiTool.role} — ${aiTool.impact}`}><Sparkles size={12} /></span> : null}
          {item.isSensitive || item.protectedLevel ? <Lock size={12} /> : null}
        </span>
      </div>
      {item.itemKind === "directory" && (!item.fullyScanned || item.scanError) ? (
        <button className="tree-action" type="button" data-help={`Continue scanning partial folder ${item.displayName}.`} onClick={() => onContinueScan(item)}>
          Continue scan
        </button>
      ) : null}
      {item.itemKind === "directory" && isOpen ? (
        <div className="tree-children">
          {childLoading ? <div className="tree-loading">Loading...</div> : null}
          {!childLoading && childItems.length === 0 ? <div className="tree-empty-folder">Empty folder</div> : (
            <Tree
              items={childItems}
              activeNodeId={activeNodeId}
              expanded={expanded}
              filterKeep={filterKeep}
              pages={pages}
              loading={loading}
              onToggle={onToggle}
              onLoadMore={onLoadMore}
              onContinueScan={onContinueScan}
              onExplainFolder={onExplainFolder}
              onContextMenu={onContextMenu}
              showProtectedMetadata={showProtectedMetadata}
              onOpen={onOpen}
              showFolders={showFolders}
              showFiles={showFiles}
              sort={sort}
            />
          )}
          {pages[String(item.id)]?.hasMore ? (
            <button className="tree-more" type="button" data-help={`Load more files from folder ${item.displayName}.`} onClick={() => void onLoadMore(item.id, pages[String(item.id)]?.nextOffset ?? pages[String(item.id)]?.items.length ?? 0)}>
              Show next items
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
});

function treeItemSize(item: NavItem) {
  return treeItemDisplaySize(item) ?? 0;
}

function treeItemDisplaySize(item: NavItem) {
  if (item.itemKind === "directory") {
    return item.aggregatePhysicalBytes ?? item.aggregateAllocatedBytes ?? item.aggregateApparentBytes ?? null;
  }
  return item.aggregateApparentBytes ?? item.aggregatePhysicalBytes ?? item.aggregateAllocatedBytes ?? null;
}

function treeItemSizeHelp(item: NavItem) {
  if (item.itemKind === "directory") {
    return item.aggregateBytesPartial
      ? "Estimated folder footprint from local inventory metadata. The plus means part of the subtree is not fully scanned."
      : "Estimated folder footprint from local inventory metadata.";
  }
  return "File length from local inventory metadata.";
}

// modifiedAt is a Unix-epoch-seconds string from the backend; missing/odd values
// sort as 0 (oldest) so they sink to the bottom when sorting newest-first.
function treeItemTime(item: NavItem) {
  const raw = item.modifiedAt;
  if (raw == null || raw === "") return 0;
  const value = Number(raw);
  return Number.isFinite(value) ? value : 0;
}

// Natural, case-insensitive name order so "file2" sorts before "file10".
function compareTreeName(left: NavItem, right: NavItem) {
  return left.displayName.localeCompare(right.displayName, undefined, { numeric: true, sensitivity: "base" });
}

function treeLogNoiseRank(item: NavItem) {
  const haystack = `${item.displayName} ${item.displayPath || ""} ${item.path || ""}`
    .replaceAll("\\", "/")
    .toLowerCase();
  const name = item.displayName.toLowerCase();
  return name === ".logs"
    || name === "logs"
    || name.endsWith(".log")
    || haystack.includes("/.logs/")
    || haystack.includes("/logs/")
    ? 1
    : 0;
}

function derateLogNoise(items: NavItem[]) {
  if (!items.some((item) => treeLogNoiseRank(item) > 0)) return items;
  return [...items.filter((item) => treeLogNoiseRank(item) === 0), ...items.filter((item) => treeLogNoiseRank(item) > 0)];
}

// Set of nav ids to keep for a filter query: a node whose name matches, plus
// every ancestor of a match (so the path to it stays visible). Walks only the
// already-loaded forest (root items + their paged/eager children).
export function buildTreeFilterKeep(roots: NavItem[], pages: Record<string, TreePage>, query: string): Set<number> {
  const keep = new Set<number>();
  const needle = query.trim().toLowerCase();
  if (!needle) return keep;
  const childrenOf = (item: NavItem): NavItem[] => pages[String(item.id)]?.items ?? item.children ?? [];
  const visit = (item: NavItem): boolean => {
    let descendantMatch = false;
    for (const child of childrenOf(item)) {
      if (visit(child)) descendantMatch = true;
    }
    if (item.displayName.toLowerCase().includes(needle) || descendantMatch) {
      keep.add(item.id);
      return true;
    }
    return false;
  };
  for (const root of roots) visit(root);
  return keep;
}

export function visibleTreeItems(items: NavItem[], showFolders: boolean, showFiles: boolean, sort: TreeSort, filterKeep?: Set<number> | null) {
  const filtered = items.filter(
    (item) => (item.itemKind === "directory" ? showFolders : showFiles) && (!filterKeep || filterKeep.has(item.id))
  );
  if (sort.key === "default") return derateLogNoise(filtered);
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...filtered].sort((left, right) => {
    const noise = treeLogNoiseRank(left) - treeLogNoiseRank(right);
    if (noise !== 0) return noise;
    let primary: number;
    if (sort.key === "name") primary = compareTreeName(left, right);
    else if (sort.key === "size") primary = treeItemSize(left) - treeItemSize(right);
    else primary = treeItemTime(left) - treeItemTime(right);
    // Stable, readable tiebreaker: equal keys fall back to ascending name order.
    return sign * primary || compareTreeName(left, right);
  });
}

type FileVisualKind = "archive" | "audio" | "code" | "config" | "data" | "executable" | "image" | "markdown" | "spreadsheet" | "text" | "unknown" | "video";

interface FileVisual {
  kind: FileVisualKind;
  icon: ReactNode;
  previewable: boolean;
  description: string;
}

const TEXT_EXTENSIONS = new Set([
  "bat", "cmd", "conf", "cfg", "css", "csv", "dockerignore", "editorconfig", "env", "gitattributes",
  "gitignore", "html", "ini", "js", "jsx", "json", "jsonc", "log", "md", "mdx", "markdown", "ps1",
  "py", "rs", "sh", "sql", "toml", "ts", "tsx", "txt", "xml", "yaml", "yml"
]);
const MARKDOWN_EXTENSIONS = new Set(["md", "mdx", "markdown"]);
const JSON_EXTENSIONS = new Set(["json", "jsonc"]);
const CODE_EXTENSIONS = new Set(["bat", "c", "cmd", "cpp", "cs", "css", "go", "h", "html", "java", "js", "jsx", "kt", "lua", "php", "ps1", "py", "rb", "rs", "sh", "sql", "ts", "tsx"]);
const CONFIG_EXTENSIONS = new Set(["conf", "cfg", "dockerignore", "editorconfig", "env", "gitattributes", "gitignore", "ini", "lock", "toml", "yaml", "yml"]);
const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "svg", "tif", "tiff", "webp"]);
const VIDEO_EXTENSIONS = new Set(["avi", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "webm", "wmv"]);
const AUDIO_EXTENSIONS = new Set(["aac", "flac", "m4a", "mp3", "ogg", "wav", "wma"]);
const ARCHIVE_EXTENSIONS = new Set(["7z", "br", "bz2", "gz", "rar", "tar", "tgz", "xz", "zip"]);
const SPREADSHEET_EXTENSIONS = new Set(["ods", "tsv", "xls", "xlsm", "xlsx"]);
const EXECUTABLE_EXTENSIONS = new Set(["app", "com", "dll", "exe", "msi"]);
const DATA_EXTENSIONS = new Set(["db", "parquet", "sqlite", "sqlite3"]);
const TEXT_LIKE_NAMES = new Set(["agents", "dockerfile", "license", "makefile", "readme"]);

function fileVisualForName(name: string): FileVisual {
  const normalized = name.toLowerCase();
  const baseName = normalized.split(/[\\/]/).pop() ?? normalized;
  const extension = baseName.includes(".") ? baseName.split(".").pop() ?? "" : "";
  const extensionOrName = extension || baseName;
  const previewable = TEXT_EXTENSIONS.has(extensionOrName) || TEXT_LIKE_NAMES.has(baseName);
  if (MARKDOWN_EXTENSIONS.has(extensionOrName) || baseName === "readme") {
    return { kind: "markdown", icon: <FileText size={14} />, previewable: true, description: "Markdown/context file, viewable inside Code Hangar." };
  }
  if (JSON_EXTENSIONS.has(extensionOrName)) {
    return { kind: "data", icon: <FileJson size={14} />, previewable: true, description: "JSON file, viewable as text inside Code Hangar." };
  }
  if (CODE_EXTENSIONS.has(extensionOrName)) {
    return { kind: "code", icon: <FileCode size={14} />, previewable: true, description: "Source/script file, viewable as text inside Code Hangar." };
  }
  if (CONFIG_EXTENSIONS.has(extensionOrName) || TEXT_LIKE_NAMES.has(baseName)) {
    return { kind: "config", icon: <FileCog size={14} />, previewable: true, description: "Config/plain text file, viewable inside Code Hangar when policy allows." };
  }
  if (TEXT_EXTENSIONS.has(extensionOrName) || previewable) {
    return { kind: "text", icon: <FileType size={14} />, previewable: true, description: "Plain text file, viewable inside Code Hangar." };
  }
  if (IMAGE_EXTENSIONS.has(extensionOrName)) {
    return { kind: "image", icon: <FileImage size={14} />, previewable: false, description: "Image file; open with Windows for full viewing." };
  }
  if (VIDEO_EXTENSIONS.has(extensionOrName)) {
    return { kind: "video", icon: <FileVideo size={14} />, previewable: false, description: "Video file; open with Windows for playback." };
  }
  if (AUDIO_EXTENSIONS.has(extensionOrName)) {
    return { kind: "audio", icon: <FileAudio size={14} />, previewable: false, description: "Audio file; open with Windows for playback." };
  }
  if (ARCHIVE_EXTENSIONS.has(extensionOrName)) {
    return { kind: "archive", icon: <FileArchive size={14} />, previewable: false, description: "Archive/package file; Code Hangar shows metadata only." };
  }
  if (SPREADSHEET_EXTENSIONS.has(extensionOrName)) {
    return { kind: "spreadsheet", icon: <FileSpreadsheet size={14} />, previewable: false, description: "Spreadsheet file; open with Windows or a spreadsheet app." };
  }
  if (EXECUTABLE_EXTENSIONS.has(extensionOrName)) {
    return { kind: "executable", icon: <FileCog size={14} />, previewable: false, description: "Executable/binary file; Code Hangar shows metadata only." };
  }
  if (DATA_EXTENSIONS.has(extensionOrName)) {
    return { kind: "data", icon: <FileJson size={14} />, previewable: false, description: "Database/dataset file; Code Hangar shows metadata only." };
  }
  return { kind: "unknown", icon: <FileQuestion size={14} />, previewable: false, description: "Unclassified file type; Code Hangar may only show metadata." };
}
