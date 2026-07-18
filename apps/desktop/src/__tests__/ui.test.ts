import { describe, expect, it } from "vitest";

import {
  COUNT_UP_DURATION_MS,
  compactLocalPath,
  computeCountUpValue,
  countUpEasing,
  displayLocalPath,
  formatBytes,
  formatDependentRow,
  groupPlanRowsForDisplay,
  prefersReducedMotion,
  orphanReferenceStatusText,
  orphanReferenceSummary,
  placeHelpPopover,
  previewFileTypeLabel,
  quickOpenLocationLabel,
  storedBooleanPreference,
  textMentionsDependencyCache
} from "../ui";
import type { DanglingAfter } from "../types";

function dependent(overrides: Partial<DanglingAfter> = {}): DanglingAfter {
  return {
    referrerNodeId: 1,
    path: "docs/guide.md",
    missingPath: "assets/logo.png",
    confidence: "High",
    projectId: 7,
    projectName: "Website",
    dependencyKind: "reference",
    crossProject: false,
    ...overrides
  };
}

describe("UI labels", () => {
  it("uses file-name context for human preview type labels", () => {
    expect(previewFileTypeLabel({
      displayName: "package.json",
      displayPath: "package.json",
      path: "C:\\project\\package.json",
      fileKind: "markdown"
    })).toBe("JSON package manifest");

    expect(previewFileTypeLabel({
      displayName: "AGENTS.md",
      displayPath: "AGENTS.md",
      path: "C:\\project\\AGENTS.md",
      fileKind: "markdown"
    })).toBe("Markdown");
  });

  it("keeps Quick Open root files distinguishable by project", () => {
    expect(quickOpenLocationLabel("README.md", "CodeHangar")).toBe("CodeHangar / README.md");
    expect(quickOpenLocationLabel("README.md")).toBe("README.md");
    expect(quickOpenLocationLabel("C:\\Users\\sample-user\\OneDrive\\Documents\\Work\\SampleAgent", null, { compactLocalPaths: true })).toBe("C:\\...\\Work\\SampleAgent");
    expect(quickOpenLocationLabel("README.md", "CodeHangar", { compactLocalPaths: true })).toBe("CodeHangar / README.md");
  });

  it("hides Windows long-path prefixes in UI labels", () => {
    expect(displayLocalPath("\\\\?\\C:\\Work\\SampleProject")).toBe("C:\\Work\\SampleProject");
    expect(displayLocalPath("\\\\?\\UNC\\server\\share\\Project")).toBe("\\\\server\\share\\Project");
    expect(quickOpenLocationLabel("\\\\?\\C:\\Work\\SampleProject", "SampleProject")).toBe("SampleProject / C:\\Work\\SampleProject");
  });

  it("compacts long local paths without losing the useful project tail", () => {
    expect(compactLocalPath("C:\\Users\\sample-user\\OneDrive\\Documents\\Work\\SampleAgent")).toBe("C:\\...\\Work\\SampleAgent");
    expect(compactLocalPath("\\\\?\\UNC\\server\\share\\workspaces\\AI\\CodeHangar")).toBe("\\\\server\\share\\...\\AI\\CodeHangar");
    expect(compactLocalPath("C:\\Work\\SampleProject")).toBe("C:\\Work\\SampleProject");
  });

  it("scales byte labels through GiB and TiB", () => {
    expect(formatBytes(48241.6 * 1024 * 1024)).toBe("47.1 GiB");
    expect(formatBytes(1.5 * 1024 * 1024 * 1024)).toBe("1.5 GiB");
    expect(formatBytes(2 * 1024 * 1024 * 1024 * 1024)).toBe("2.0 TiB");
  });

  it("uses explicit boolean preferences while preserving a default", () => {
    expect(storedBooleanPreference(null, true)).toBe(true);
    expect(storedBooleanPreference(null, false)).toBe(false);
    expect(storedBooleanPreference("true", false)).toBe(true);
    expect(storedBooleanPreference("false", true)).toBe(false);
  });
});

describe("help popover placement", () => {
  it("keeps a panel inside the right and bottom edges", () => {
    expect(placeHelpPopover(
      { left: 970, right: 1000, top: 700, bottom: 730 },
      380,
      220,
      1024,
      768
    )).toEqual({ left: 620, top: 472, maxHeight: 680, side: "above" });
  });

  it("opens below when there is room and clamps against the left edge", () => {
    expect(placeHelpPopover(
      { left: 4, right: 30, top: 20, bottom: 46 },
      360,
      180,
      800,
      600
    )).toEqual({ left: 12, top: 54, maxHeight: 534, side: "below" });
  });
});

describe("count-up animation math", () => {
  it("eases out: fast at the start, settling at the end", () => {
    expect(countUpEasing(0)).toBe(0);
    expect(countUpEasing(1)).toBe(1);
    // Ease-out is past the halfway value at the midpoint of time.
    expect(countUpEasing(0.5)).toBeGreaterThan(0.5);
    // Monotonic and clamped outside [0,1].
    expect(countUpEasing(-1)).toBe(0);
    expect(countUpEasing(2)).toBe(1);
  });

  it("holds the start value at t=0 and lands exactly on the target at the end", () => {
    expect(computeCountUpValue(0, 240, 0)).toBe(0);
    expect(computeCountUpValue(0, 240, COUNT_UP_DURATION_MS)).toBe(240);
    // Overshooting time never exceeds the target.
    expect(computeCountUpValue(0, 240, COUNT_UP_DURATION_MS * 2)).toBe(240);
  });

  it("returns an in-between integer partway through the sweep", () => {
    const mid = computeCountUpValue(0, 1000, COUNT_UP_DURATION_MS / 2);
    expect(mid).toBeGreaterThan(0);
    expect(mid).toBeLessThan(1000);
    expect(Number.isInteger(mid)).toBe(true);
  });

  it("snaps immediately for a zero-length or non-positive duration", () => {
    expect(computeCountUpValue(0, 240, 10, 0)).toBe(240);
  });
});

describe("orphan reference labels", () => {
  it("does not call priority context a known reference when no refs are counted", () => {
    const status = {
      nodeId: 1,
      evaluated: true,
      isCandidate: false,
      candidateKind: null,
      confidence: null,
      reason: "Priority context files are not treated as orphan assets.",
      incomingReferences: 0,
      protectedOrSensitive: false,
      physicalBytes: 1024,
      footprintPartial: false
    };

    expect(orphanReferenceSummary(true, status)).toMatchObject({
      state: "Protected context file",
      countLabel: "0",
      reason: "Priority context files are not treated as orphan assets."
    });
    expect(orphanReferenceStatusText("README.md", status)).toBe(
      "README.md is protected as priority context; zero counted references does not make it an orphan candidate."
    );
  });

  it("keeps true orphan candidates explicit and non-destructive", () => {
    const status = {
      nodeId: 2,
      evaluated: true,
      isCandidate: true,
      candidateKind: "image",
      confidence: "High",
      reason: "No known local references and asset-like path.",
      incomingReferences: 0,
      protectedOrSensitive: false,
      physicalBytes: 4096,
      footprintPartial: false
    };

    expect(orphanReferenceSummary(true, status)).toMatchObject({
      state: "No known reference",
      tone: "review",
      countLabel: "0",
      confidenceLabel: "Strong local signal"
    });
    expect(orphanReferenceStatusText("unused.png", status)).toContain("This is not a delete recommendation.");
  });
});

describe("dependent row segmentation", () => {
  it("keeps the referrer→target path as the strong primary", () => {
    const row = formatDependentRow(dependent());
    expect(row.primary).toBe("docs/guide.md → assets/logo.png");
  });

  it("emits no secondary facts for a same-project reference dependent", () => {
    // Parity with the old string, which was just the bare "path → missingPath".
    expect(formatDependentRow(dependent()).segments).toEqual([]);
  });

  it("flags a cross-project dependent with a ⚠ warn pill and its owning project", () => {
    const row = formatDependentRow(dependent({ crossProject: true, projectName: "Docs Site" }));
    expect(row.segments).toEqual([
      { text: "⚠ cross-project", tone: "warn" },
      { text: "in Docs Site" }
    ]);
  });

  it("does not add an 'in project' fact for a cross-project dependent with no project name", () => {
    const row = formatDependentRow(dependent({ crossProject: true, projectName: null }));
    expect(row.segments).toEqual([{ text: "⚠ cross-project", tone: "warn" }]);
  });

  it("tags a workflow dependency as a muted tag, preserving all facts together", () => {
    const row = formatDependentRow(dependent({ crossProject: true, projectName: "Pipelines", dependencyKind: "workflow" }));
    expect(row.primary).toBe("docs/guide.md → assets/logo.png");
    expect(row.segments).toEqual([
      { text: "⚠ cross-project", tone: "warn" },
      { text: "in Pipelines" },
      { text: "workflow", tone: "tag" }
    ]);
  });

  it("splits dependency-cache rows out of the direct list without dropping them", () => {
    const cacheA = { primary: ".local/cargo/registry/src/index.crates.io-abc/aho-corasick/.github/workflows/ci.yml → .local/cargo/registry/src/index.crates.io-abc/aho-corasick/README.md" };
    const direct = { primary: "src/main.rs → README.md" };
    const cacheB = { primary: "node_modules/pkg/index.js → node_modules/pkg/README.md" };
    const grouped = groupPlanRowsForDisplay([cacheA, direct, cacheB]);

    // Direct list keeps only non-cache rows; the cache rows are preserved verbatim
    // (grouped, never dropped) so a sensitive file inside a cache stays reachable.
    expect(grouped.rows).toEqual([direct]);
    expect(grouped.cacheRows).toEqual([cacheA, cacheB]);
  });

  it("treats Python virtualenv package folders as dependency caches", () => {
    expect(textMentionsDependencyCache("C:\\AI\\App\\.venv\\Lib\\site-packages\\llvmlite\\binding\\llvmlite.dll")).toBe(true);
    expect(textMentionsDependencyCache("venv/lib/python3.12/site-packages/pkg/module.py")).toBe(true);
  });

  it("leaves the list untouched and reports no cache rows when nothing lives in a cache", () => {
    const rows = [{ primary: "src/main.rs → README.md" }, { primary: "docs/guide.md → LICENSE" }];
    const grouped = groupPlanRowsForDisplay(rows);
    expect(grouped.rows).toEqual(rows);
    expect(grouped.cacheRows).toEqual([]);
  });

  it("routes every row to the cache group when all rows are inside caches (direct list empty)", () => {
    const rows = [
      { primary: "node_modules/a/index.js → node_modules/a/README.md" },
      { primary: "vendor/b/lib.go → vendor/b/README.md" }
    ];
    const grouped = groupPlanRowsForDisplay(rows);
    expect(grouped.rows).toEqual([]);
    expect(grouped.cacheRows).toEqual(rows);
    // No information loss: direct + cache always reconstitutes the full input set.
    expect([...grouped.rows, ...grouped.cacheRows]).toEqual(rows);
  });
});

describe("reduced-motion resolution", () => {
  it("honours an explicit override in either direction", () => {
    expect(prefersReducedMotion(true)).toBe(true);
    expect(prefersReducedMotion(false)).toBe(false);
  });

  it("defaults to no reduction when matchMedia is unavailable (node/SSR)", () => {
    // The node test environment has no window.matchMedia; the guard must not throw.
    expect(prefersReducedMotion()).toBe(false);
  });
});
