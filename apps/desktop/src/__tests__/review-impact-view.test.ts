import { describe, expect, it } from "vitest";
import {
  SAFE_MANAGE_RISK_GUIDE,
  reviewConfidenceLabel,
  riskTierVisibility,
  safeManageAdvancedPrompt,
  safeManageAttentionCounts,
  sensitiveProtectedRows
} from "../views/ReviewImpactView";

describe("Safe Manage advanced detail prompt", () => {
  it("offers a local action when Simple mode hides exact review detail", () => {
    expect(safeManageAdvancedPrompt(false)).toEqual({
      buttonLabel: "Show detailed evidence",
      note: "Counts stay compact. Detailed evidence expands only the categories with findings."
    });
  });

  it("does not render a redundant prompt once advanced details are visible", () => {
    expect(safeManageAdvancedPrompt(true)).toBeNull();
  });
});

describe("Safe Manage risk summary", () => {
  it("keeps all five tiers scannable while preserving their review guidance", () => {
    expect(SAFE_MANAGE_RISK_GUIDE.map((entry) => entry.tier)).toEqual(["green", "yellow", "orange", "red", "black"]);
    expect(SAFE_MANAGE_RISK_GUIDE.every((entry) => entry.summary.length < entry.detail.length)).toBe(true);
  });

  it("gives visual weight only to tiers with findings", () => {
    const visibility = riskTierVisibility([
      { tier: "green", count: 0, physicalBytes: 0 },
      { tier: "yellow", count: 2, physicalBytes: 2048 },
      { tier: "red", count: 0, physicalBytes: 0 }
    ]);

    expect(visibility.active.map((tier) => tier.tier)).toEqual(["yellow"]);
    expect(visibility.clearCount).toBe(2);
  });

  it("labels confidence as evidence strength and includes unknown evidence when present", () => {
    expect(reviewConfidenceLabel({ high: 1, medium: 2, low: 3, unknown: 4 })).toBe("1 strong · 2 possible · 3 weak · 4 unclear");
    expect(reviewConfidenceLabel({ high: 1, medium: 0, low: 0, unknown: 0 })).toBe("1 strong · 0 possible · 0 weak");
  });
});

describe("Safe Manage attention summary", () => {
  it("keeps all category counts available before expanding detailed paths", () => {
    expect(safeManageAttentionCounts({
      sharedAssets: [{ nodeId: 1, path: "model.gguf", displayName: "model.gguf", referencedBy: [], confidence: "high" }],
      danglingAfter: [{ referrerNodeId: 2, path: "workflow.json", missingPath: "model.gguf", crossProject: true, projectId: 2, projectName: "Other", dependencyKind: "workflow", confidence: "high" }],
      gitWarnings: []
    }, 3)).toEqual({
      shared: 1,
      dependents: 1,
      sensitiveProtected: 3,
      gitWarnings: 0
    });
  });
});

describe("Safe Manage protection rows", () => {
  it("merges sensitive and protected signals for the same path", () => {
    expect(sensitiveProtectedRows(
      [{ nodeId: 1, path: ".env", signature: "dotenv" }],
      [
        { nodeId: 1, path: ".ENV", level: "NO_PREVIEW" },
        { nodeId: 2, path: ".git", level: "NO_PREVIEW" }
      ]
    )).toEqual([
      {
        primary: ".env",
        segments: [
          { text: "sensitive", tone: "tag" },
          { text: "NO_PREVIEW", tone: "tag" }
        ]
      },
      {
        primary: ".git",
        segments: [{ text: "NO_PREVIEW", tone: "tag" }]
      }
    ]);
  });

  it("does not duplicate repeated policy tags", () => {
    expect(sensitiveProtectedRows(
      [
        { nodeId: 1, path: "secrets\\token.txt", signature: "token" },
        { nodeId: 1, path: "secrets/token.txt", signature: "token" }
      ],
      []
    )).toHaveLength(1);
  });
});
