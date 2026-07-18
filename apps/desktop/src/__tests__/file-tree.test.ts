import { describe, expect, it } from "vitest";

import type { NavItem } from "../types";
import { buildTreeFilterKeep, fileTreeEmptyCopy, visibleTreeItems } from "../views/project-center/FileTree";
import type { TreePage, TreeSort } from "../views/project-center/types";

function navItem(overrides: Partial<NavItem>): NavItem {
  return {
    id: overrides.id ?? 1,
    projectId: 1,
    nodeId: overrides.nodeId ?? overrides.id ?? 1,
    parentNavId: null,
    path: overrides.path ?? `C:\\root\\${overrides.displayName ?? "item"}`,
    displayPath: overrides.displayPath ?? overrides.path ?? `C:\\root\\${overrides.displayName ?? "item"}`,
    displayName: overrides.displayName ?? "item",
    itemKind: overrides.itemKind ?? "file",
    priority: 0,
    isContext: false,
    isMarkdown: false,
    isSensitive: false,
    protectedLevel: null,
    childCount: 0,
    fullyScanned: true,
    collapseDefault: false,
    scanError: null,
    aggregateApparentBytes: null,
    aggregateAllocatedBytes: null,
    aggregatePhysicalBytes: null,
    aggregateBytesPartial: false,
    children: [],
    ...overrides
  };
}

const sort = (key: TreeSort["key"], dir: TreeSort["dir"] = "asc"): TreeSort => ({ key, dir });
const names = (items: NavItem[]) => items.map((item) => item.displayName);

describe("file tree item filtering and sorting", () => {
  it("keeps folders and files visible by default", () => {
    const items = [
      navItem({ id: 1, displayName: "docs", itemKind: "directory" }),
      navItem({ id: 2, displayName: "README.md", itemKind: "file" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("default")))).toEqual(["docs", "README.md"]);
  });

  it("can hide folders or files independently", () => {
    const items = [
      navItem({ id: 1, displayName: "docs", itemKind: "directory" }),
      navItem({ id: 2, displayName: "README.md", itemKind: "file" })
    ];

    expect(names(visibleTreeItems(items, false, true, sort("default")))).toEqual(["README.md"]);
    expect(names(visibleTreeItems(items, true, false, sort("default")))).toEqual(["docs"]);
  });

  it("sorts by shown size while preserving name order for ties", () => {
    const items = [
      navItem({ id: 1, displayName: "b.bin", aggregatePhysicalBytes: 1024 }),
      navItem({ id: 2, displayName: "a.bin", aggregatePhysicalBytes: 1024 }),
      navItem({ id: 3, displayName: "large.bin", aggregatePhysicalBytes: 4096 })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("size", "desc")))).toEqual(["large.bin", "a.bin", "b.bin"]);
    expect(names(visibleTreeItems(items, true, true, sort("size", "asc")))).toEqual(["a.bin", "b.bin", "large.bin"]);
  });

  it("uses apparent file size before allocated or physical size for files", () => {
    const items = [
      navItem({ id: 1, displayName: "allocated-large.txt", aggregateApparentBytes: 100, aggregatePhysicalBytes: 8192 }),
      navItem({ id: 2, displayName: "content-large.txt", aggregateApparentBytes: 200, aggregatePhysicalBytes: 4096 })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("size", "desc")))).toEqual(["content-large.txt", "allocated-large.txt"]);
  });

  it("sorts by name case-insensitively and naturally", () => {
    const items = [
      navItem({ id: 1, displayName: "banana.txt" }),
      navItem({ id: 2, displayName: "Apple.txt" }),
      navItem({ id: 3, displayName: "cherry.txt" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("name", "asc")))).toEqual(["Apple.txt", "banana.txt", "cherry.txt"]);
    expect(names(visibleTreeItems(items, true, true, sort("name", "desc")))).toEqual(["cherry.txt", "banana.txt", "Apple.txt"]);
  });

  it("orders numeric names naturally (file2 before file10)", () => {
    const items = [
      navItem({ id: 1, displayName: "file10.txt" }),
      navItem({ id: 2, displayName: "file2.txt" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("name", "asc")))).toEqual(["file2.txt", "file10.txt"]);
  });

  it("keeps logs behind project files in curated and name sorts", () => {
    const items = [
      navItem({ id: 1, displayName: ".logs", itemKind: "directory" }),
      navItem({ id: 2, displayName: "README.md" }),
      navItem({ id: 3, displayName: "src", itemKind: "directory" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("default")))).toEqual(["README.md", "src", ".logs"]);
    expect(names(visibleTreeItems(items, true, true, sort("name", "asc")))).toEqual(["README.md", "src", ".logs"]);
    expect(names(visibleTreeItems(items, true, true, sort("name", "desc")))).toEqual(["src", "README.md", ".logs"]);
  });

  it("sorts by modified date with newest first by default", () => {
    const items = [
      navItem({ id: 1, displayName: "old.txt", modifiedAt: "1000" }),
      navItem({ id: 2, displayName: "new.txt", modifiedAt: "3000" }),
      navItem({ id: 3, displayName: "mid.txt", modifiedAt: "2000" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("date", "desc")))).toEqual(["new.txt", "mid.txt", "old.txt"]);
    expect(names(visibleTreeItems(items, true, true, sort("date", "asc")))).toEqual(["old.txt", "mid.txt", "new.txt"]);
  });

  it("sinks entries without a modified date to the bottom when newest-first", () => {
    const items = [
      navItem({ id: 1, displayName: "undated.txt" }),
      navItem({ id: 2, displayName: "dated.txt", modifiedAt: "5000" })
    ];

    expect(names(visibleTreeItems(items, true, true, sort("date", "desc")))).toEqual(["dated.txt", "undated.txt"]);
  });
});

describe("file tree name filter", () => {
  it("keeps a match and its ancestor folder, drops unrelated siblings", () => {
    const config = navItem({ id: 10, displayName: "config.json", itemKind: "file" });
    const readme = navItem({ id: 11, displayName: "README.md", itemKind: "file" });
    const docs = navItem({ id: 2, displayName: "docs", itemKind: "directory", children: [config, readme] });
    const other = navItem({ id: 3, displayName: "other", itemKind: "directory", children: [] });
    const roots = [docs, other];

    const keep = buildTreeFilterKeep(roots, {}, "config");
    expect(keep.has(2)).toBe(true); // ancestor folder of the match stays visible
    expect(keep.has(10)).toBe(true); // the matching file
    expect(keep.has(11)).toBe(false); // a non-matching sibling is dropped
    expect(keep.has(3)).toBe(false); // an unrelated folder is dropped

    // visibleTreeItems applies the keep-set at each level it renders
    expect(names(visibleTreeItems(roots, true, true, sort("default"), keep))).toEqual(["docs"]);
    expect(names(visibleTreeItems(docs.children, true, true, sort("default"), keep))).toEqual(["config.json"]);
  });

  it("matches case-insensitively and walks paged children", () => {
    const deep = navItem({ id: 20, displayName: "Secret.env", itemKind: "file" });
    const vault = navItem({ id: 4, displayName: "vault", itemKind: "directory", children: [] });
    const pages: Record<string, TreePage> = { "4": { items: [deep], total: 1, hasMore: false } };

    const keep = buildTreeFilterKeep([vault], pages, "secret");
    expect(keep.has(4)).toBe(true);
    expect(keep.has(20)).toBe(true);
  });

  it("returns an empty keep-set for a blank query", () => {
    const file = navItem({ id: 1, displayName: "a.txt" });
    expect(buildTreeFilterKeep([file], {}, "   ").size).toBe(0);
  });
});

describe("file tree empty feedback", () => {
  it("stays quiet while at least one root item is visible", () => {
    expect(fileTreeEmptyCopy({
      rootItemCount: 3,
      visibleRootCount: 1,
      showFolders: true,
      showFiles: true,
      query: ""
    })).toBeNull();
  });

  it("explains when both visibility choices are off", () => {
    expect(fileTreeEmptyCopy({
      rootItemCount: 3,
      visibleRootCount: 0,
      showFolders: false,
      showFiles: false,
      query: ""
    })).toMatchObject({ kind: "hidden", action: "show-all" });
  });

  it("reports a loaded-tree filter miss with a direct reset", () => {
    expect(fileTreeEmptyCopy({
      rootItemCount: 3,
      visibleRootCount: 0,
      showFolders: true,
      showFiles: true,
      query: "  missing  "
    })).toEqual({
      kind: "filter",
      title: "No loaded matches",
      body: "No loaded file or folder matches \"missing\".",
      action: "clear-filter"
    });
  });

  it("distinguishes an empty inventory from hidden results", () => {
    expect(fileTreeEmptyCopy({
      rootItemCount: 0,
      visibleRootCount: 0,
      showFolders: true,
      showFiles: true,
      query: ""
    })).toMatchObject({ kind: "empty", action: null });
  });
});
