import { describe, expect, it } from "vitest";

import { removeProjectActionLabel } from "../removeProjectDialog";

describe("remove project action label", () => {
  it("names metadata removals instead of using an ambiguous generic command", () => {
    expect(removeProjectActionLabel({ fromApps: true, fromHangar: true, fromDisk: false })).toBe("Remove from apps & Code Hangar");
    expect(removeProjectActionLabel({ fromApps: true, fromHangar: false, fromDisk: false })).toBe("Remove from AI apps");
    expect(removeProjectActionLabel({ fromApps: false, fromHangar: true, fromDisk: false })).toBe("Remove from Code Hangar");
  });

  it("keeps disk removal behind Safe Manage", () => {
    expect(removeProjectActionLabel({ fromApps: false, fromHangar: false, fromDisk: true })).toBe("Continue to Safe Manage");
    expect(removeProjectActionLabel({ fromApps: false, fromHangar: false, fromDisk: false })).toBe("Choose what to remove");
  });
});
