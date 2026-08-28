import { describe, expect, it } from "vitest";

import {
  attemptDeepScanInventoryStart,
  deepScanBuildProjectState,
  deepScanOutcomeFromScanState,
  deepScanTerminalPercent,
  deepScanTerminalPresentation,
  type DeepScanOutcome
} from "../deepScanState";

describe("Deep Scan terminal truth", () => {
  const matrix: Array<{
    state: string;
    outcome: DeepScanOutcome;
    title: string;
    action: "retry" | "resume" | null;
    project: string;
  }> = [
    { state: "completed", outcome: "completed", title: "Inventory ready", action: null, project: "indexed" },
    { state: "partial", outcome: "partial", title: "Inventory incomplete", action: "resume", project: "partial" },
    { state: "cancelled", outcome: "cancelled", title: "Inventory scan stopped", action: "resume", project: "stopped" },
    { state: "failed", outcome: "failed", title: "Deep Scan failed", action: "retry", project: "failed" }
  ];

  it.each(matrix)("presents $state distinctly", ({ state, outcome, title, action, project }) => {
    expect(deepScanOutcomeFromScanState(state)).toBe(outcome);
    const presentation = deepScanTerminalPresentation("done", outcome);
    expect(presentation.title).toBe(title);
    expect(presentation.action).toBe(action);
    expect(presentation.inventoryReady).toBe(outcome === "completed");
    expect(deepScanBuildProjectState(state, 0, 0, false)).toBe(project);
  });

  it("reserves ready, 100 percent and indexed for completed inventory", () => {
    const incomplete: DeepScanOutcome[] = ["partial", "cancelled", "failed", "inventory-not-started", "mapped"];
    for (const outcome of incomplete) {
      const presentation = deepScanTerminalPresentation("done", outcome);
      expect(presentation.inventoryReady).toBe(false);
      expect(presentation.title).not.toMatch(/\bready\b/i);
      expect(deepScanTerminalPercent(outcome, 200, 100)).toBe(99);
    }
    for (const state of ["partial", "cancelled", "failed"]) {
      expect(deepScanBuildProjectState(state, 0, 0, false)).not.toBe("indexed");
    }
    expect(deepScanTerminalPercent("completed", 1, 100)).toBe(100);
    expect(deepScanBuildProjectState("completed", 0, -1, false)).toBe("indexed");
  });

  it("keeps live project progress provisional until the whole job completes", () => {
    expect(deepScanBuildProjectState("running", 0, 1, false)).toBe("processed");
    expect(deepScanBuildProjectState("running", 1, 1, false)).toBe("indexing");
    expect(deepScanBuildProjectState("running", 2, 1, false)).toBe("queued");
  });

  it("reports a null job id as inventory-not-started", async () => {
    const result = await attemptDeepScanInventoryStart([7], async () => null);
    expect(result).toEqual({
      kind: "not-started",
      error: "The inventory service did not return a scan job."
    });
    expect(deepScanTerminalPresentation("done", "inventory-not-started")).toMatchObject({
      title: "Projects added; inventory not started",
      action: "retry",
      autoDismiss: false
    });
  });

  it("reports a rejected inventory start and retains the exact error", async () => {
    const result = await attemptDeepScanInventoryStart([7], async () => {
      throw new Error("scanner admission refused");
    });
    expect(result).toEqual({ kind: "not-started", error: "scanner admission refused" });
  });

  it("accepts only a concrete backend job as started", async () => {
    const result = await attemptDeepScanInventoryStart([7], async () => ({
      jobId: "scan-42",
      message: "Queued"
    }));
    expect(result).toEqual({
      kind: "started",
      status: { jobId: "scan-42", message: "Queued" }
    });
  });
});
