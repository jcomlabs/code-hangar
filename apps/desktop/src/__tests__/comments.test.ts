import { describe, expect, it } from "vitest";
import { fixtureApi } from "../fixtures";
import { formatCommentMeta, validateCommentBody } from "../comments";
import type { Comment } from "../types";

const base: Comment = {
  id: 1,
  nodeId: 1,
  projectId: null,
  body: "body",
  author: "user",
  source: "user",
  createdAt: "2026-06-22T10:00:00Z",
  updatedAt: "2026-06-22T10:00:00Z",
};

describe("comment helpers", () => {
  it("validates body: trims, rejects empty, caps length", () => {
    expect(validateCommentBody("  hello  ")).toMatchObject({ ok: true, trimmed: "hello" });
    expect(validateCommentBody("   ").ok).toBe(false);
    expect(validateCommentBody("x".repeat(5000)).ok).toBe(false);
  });

  it("formats local-user metadata with an edited marker", () => {
    expect(formatCommentMeta(base)).toContain("You");
    expect(formatCommentMeta(base)).not.toContain("edited");
    expect(formatCommentMeta({ ...base, updatedAt: "2026-06-22T11:00:00Z" })).toContain("edited");
  });
});

describe("comment fixture round-trip", () => {
  it("adds, lists, counts, edits and deletes local comments", async () => {
    const created = await fixtureApi.commentAdd(42, "  first  ");
    expect(created.body).toBe("first");
    expect(created.source).toBe("user");
    expect(await fixtureApi.commentsCountForNode(42)).toBe(1);

    const edited = await fixtureApi.commentEdit(created.id, "edited");
    expect(edited.body).toBe("edited");

    await fixtureApi.commentDelete(created.id);
    expect(await fixtureApi.commentsCountForNode(42)).toBe(0);

    await expect(fixtureApi.commentAdd(42, "   ")).rejects.toThrow();
  });
});
