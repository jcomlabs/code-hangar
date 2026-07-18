import { describe, expect, it } from "vitest";

import {
  ORGANIZE_MODEL_FILE_PREVIEW,
  organizeDisclosureItems,
  organizeInventoryFingerprint,
  organizeModelResultCounts,
  organizeProjectReviewSummary,
  organizeProjectReviewReason,
  splitProjectsForLocationReview
} from "../views/organize-helpers";

describe("Organize project location review", () => {
  it("flags only contextless projects as review candidates", () => {
    expect(organizeProjectReviewReason({ contextCount: 0, isCurrent: false })).toBe("no context, no recognized activity signal");
    expect(organizeProjectReviewReason({ contextCount: 0, isCurrent: true })).toBe("no context files");
    expect(organizeProjectReviewReason({ contextCount: 2, isCurrent: false })).toBeNull();
  });

  it("splits location groups into review-first and mapped projects without dropping rows", () => {
    const review = { contextCount: 0, isCurrent: false, name: "Needs review" };
    const mapped = { contextCount: 3, isCurrent: false, name: "Mapped" };
    const grouped = splitProjectsForLocationReview([mapped, review]);

    expect(grouped.needsReview).toEqual([review]);
    expect(grouped.mapped).toEqual([mapped]);
    expect([...grouped.needsReview, ...grouped.mapped]).toEqual([review, mapped]);
  });

  it("summarizes real project progress without counting hidden demos", () => {
    const summary = organizeProjectReviewSummary([
      { source: "scan", contextCount: 3, isCurrent: false },
      { source: "scan", contextCount: 0, isCurrent: false },
      { source: "scan", contextCount: 1, isCurrent: true },
      { source: "fixture", contextCount: 0, isCurrent: false }
    ]);

    expect(summary).toEqual({ total: 3, mapped: 2, needsReview: 1, progress: 67 });
  });

  it("keeps large model locations compact until the user asks for every row", () => {
    const items = Array.from({ length: 25 }, (_, index) => index + 1);

    expect(organizeDisclosureItems(items, false, ORGANIZE_MODEL_FILE_PREVIEW)).toEqual(items.slice(0, 8));
    expect(organizeDisclosureItems(items, true, ORGANIZE_MODEL_FILE_PREVIEW)).toEqual(items);
  });

  it("distinguishes the returned model preview from the full candidate count", () => {
    const result = {
      candidates: Array.from({ length: 1000 }, (_, nodeId) => ({ nodeId })),
      total: 5000
    };

    expect(organizeModelResultCounts(result)).toEqual({ shown: 1000, total: 5000, limited: true });
    expect(organizeModelResultCounts({ candidates: result.candidates, total: 10 })).toEqual({
      shown: 1000,
      total: 1000,
      limited: false
    });
  });

  it("invalidates cached review results when the real inventory changes", () => {
    const base = [{ id: 7, path: "C:\\models", source: "scan", contextCount: 2, scanState: "scanned" as const }];
    const unchangedWithFixture = [
      ...base,
      { id: 99, path: "fixture", source: "fixture", contextCount: 0, scanState: "scanned" as const }
    ];
    const rescanned = [{ ...base[0], scanState: "outdated" as const }];

    expect(organizeInventoryFingerprint(unchangedWithFixture)).toBe(organizeInventoryFingerprint(base));
    expect(organizeInventoryFingerprint(rescanned)).not.toBe(organizeInventoryFingerprint(base));
  });
});
