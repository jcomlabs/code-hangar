import { describe, expect, it } from "vitest";
import { loadStartupSideData, type StartupSideDataLoaders } from "../startupSideData";

function loaders(overrides: Partial<StartupSideDataLoaders> = {}): StartupSideDataLoaders {
  return {
    recentItems: async () => [],
    pinnedItems: async () => [],
    roots: async () => [],
    zones: async () => [],
    security: async () => ({
      outboundNetwork: "denied",
      mutationExecutor: "disabled",
      agentIpc: "disabled",
      activeFeatures: ["core"],
      notes: []
    }),
    ...overrides
  };
}

describe("startup side data", () => {
  it("keeps successful metadata when one non-critical loader fails", async () => {
    const result = await loadStartupSideData(loaders({
      roots: async () => {
        throw new Error("roots unavailable");
      }
    }));

    expect(result.data.recentItems).toEqual([]);
    expect(result.data.pinnedItems).toEqual([]);
    expect(result.data.zones).toEqual([]);
    expect(result.data.security?.activeFeatures).toEqual(["core"]);
    expect(result.data).not.toHaveProperty("roots");
    expect(result.failures).toEqual([{ key: "roots", message: "roots unavailable" }]);
  });

  it("returns every value without warnings when all loaders succeed", async () => {
    const result = await loadStartupSideData(loaders());

    expect(Object.keys(result.data).sort()).toEqual([
      "pinnedItems",
      "recentItems",
      "roots",
      "security",
      "zones"
    ]);
    expect(result.failures).toEqual([]);
  });
});
