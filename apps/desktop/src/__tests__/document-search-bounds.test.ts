import { describe, expect, it } from "vitest";

import {
  boundedDocumentSearchLimit,
  DOCUMENT_SEARCH_DEFAULT_RESULTS,
  DOCUMENT_SEARCH_MATCH_EXPLANATION,
  DOCUMENT_SEARCH_MAX_QUERY_CHARS,
  DOCUMENT_SEARCH_MAX_QUERY_TERMS,
  DOCUMENT_SEARCH_MAX_RESULTS,
  documentSearchCriteriaKey,
  documentSearchQueryError,
  documentSearchQueryTerms,
  prepareDocumentSearchFilters
} from "../documentSearch";
import { canSubmitDocumentSearch } from "../views/DiscoverView";

describe("bounded document search contract", () => {
  it.each([
    [undefined, DOCUMENT_SEARCH_DEFAULT_RESULTS],
    [0, DOCUMENT_SEARCH_MAX_RESULTS],
    [-1, 1],
    [25.9, 25],
    [DOCUMENT_SEARCH_MAX_RESULTS + 1, DOCUMENT_SEARCH_MAX_RESULTS],
    [Number.POSITIVE_INFINITY, DOCUMENT_SEARCH_MAX_RESULTS]
  ])("normalizes result limit %s to %s", (input, expected) => {
    expect(boundedDocumentSearchLimit(input)).toBe(expected);
  });

  it("normalizes the legacy zero sentinel before a request crosses the API boundary", () => {
    expect(prepareDocumentSearchFilters({ query: "reader performance", limit: 0 })).toEqual({
      query: "reader performance",
      limit: DOCUMENT_SEARCH_MAX_RESULTS
    });
  });

  it("counts the same whitespace-delimited searchable terms as the FTS boundary", () => {
    expect(documentSearchQueryTerms("  README!!! cloud_safe foo/bar -- ...  ")).toEqual([
      "README",
      "cloud_safe",
      "foobar",
      "--",
      "..."
    ]);
  });

  it("rejects overlong queries instead of truncating away required AND terms", () => {
    const tooManyCharacters = "x".repeat(DOCUMENT_SEARCH_MAX_QUERY_CHARS + 1);
    const tooManyTerms = Array.from(
      { length: DOCUMENT_SEARCH_MAX_QUERY_TERMS + 1 },
      (_, index) => `term${index}`
    ).join(" ");

    expect(documentSearchQueryError(tooManyCharacters)).toContain(`${DOCUMENT_SEARCH_MAX_QUERY_CHARS} characters`);
    expect(documentSearchQueryError(tooManyTerms)).toContain(`${DOCUMENT_SEARCH_MAX_QUERY_TERMS} content terms`);
    expect(() => prepareDocumentSearchFilters({ query: tooManyTerms, limit: 10 })).toThrow(RangeError);
    expect(canSubmitDocumentSearch(tooManyTerms, false, false)).toBe(false);
  });

  it("states the AND behavior explicitly", () => {
    expect(DOCUMENT_SEARCH_MATCH_EXPLANATION).toContain("Every content term");
    expect(DOCUMENT_SEARCH_MATCH_EXPLANATION).toContain("(AND)");
  });

  it("uses the effective bounded limit in stale-result criteria", () => {
    const criteria = {
      query: "README",
      scope: "all" as const,
      projectId: null,
      indexedKind: "context",
      pathFilter: "",
      nameFilter: "",
      limit: 0,
      includeFixtureProjects: false
    };

    expect(documentSearchCriteriaKey(criteria)).toBe(documentSearchCriteriaKey({
      ...criteria,
      limit: DOCUMENT_SEARCH_MAX_RESULTS
    }));
  });
});
