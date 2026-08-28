import { describe, expect, it } from "vitest";

import { reorderTabs, tabStripEdgeTarget } from "../hooks/useTabDrag";

describe("tab drag ordering", () => {
  const tabs = [
    { nodeId: 1, label: "one" },
    { nodeId: 2, label: "two" },
    { nodeId: 3, label: "three" },
    { nodeId: 4, label: "four" }
  ];

  it("moves a tab after the final target", () => {
    expect(reorderTabs(tabs, 1, 4, "after").map((tab) => tab.nodeId)).toEqual([2, 3, 4, 1]);
  });

  it("maps empty strip space beyond the final tab to an after-last drop", () => {
    expect(tabStripEdgeTarget(1, 500, [
      { nodeId: 1, left: 0, right: 100 },
      { nodeId: 2, left: 100, right: 200 },
      { nodeId: 3, left: 200, right: 300 },
      { nodeId: 4, left: 300, right: 400 }
    ])).toEqual({ nodeId: 4, position: "after" });
  });
});
