import { describe, expect, it } from "vitest";

import { sessionsSinceReview, undatedSessionFingerprint } from "../reviewCheckpoint";
import type { ProjectReviewCheckpoint, SessionDiscoveryCandidate } from "../types";

function session(path: string, modifiedMs: number | null, sourceKind = "codex"): SessionDiscoveryCandidate {
  return {
    path,
    displayName: path,
    sourceKind,
    sourceLabel: sourceKind,
    sessionKind: "local session",
    confidence: "high",
    linkedProjectPaths: [],
    linkedRegisteredProjectIds: [7],
    association: "registered_project",
    modifiedMs
  };
}

function checkpoint(
  undatedSessionFingerprint: string | null,
  sessionCutoffMs = 200
): ProjectReviewCheckpoint {
  return {
    projectId: 7,
    reviewedAt: "2026-08-23T12:00:00Z",
    sessionCutoffMs,
    undatedSessionFingerprint,
    gitFingerprint: null,
    gitHead: null
  };
}

describe("undated session review checkpoints", () => {
  it("uses a stable, content-free aggregate regardless of discovery order or Windows path spelling", () => {
    const first = [session("C:/Work/A.jsonl", null), session("C:\\Work\\B.jsonl", null, "Claude")];
    const second = [session("c:/work/b.jsonl", null, "claude"), session("c:\\work\\a.jsonl", null)];

    expect(undatedSessionFingerprint(first)).toBe(undatedSessionFingerprint(second));
    expect(undatedSessionFingerprint(first)).toMatch(/^v1:2:[0-9a-f]{8}:[0-9a-f]{8}$/);
    expect(undatedSessionFingerprint([session("dated.jsonl", 100)])).toBeNull();
  });

  it("clears acknowledged undated records while retaining genuinely newer dated sessions", () => {
    const sessions = [session("old.jsonl", 100), session("new.jsonl", 250), session("unknown.jsonl", null)];
    const saved = checkpoint(undatedSessionFingerprint(sessions));

    expect(sessionsSinceReview(sessions, saved).map((item) => item.path)).toEqual(["new.jsonl"]);
  });

  it("makes a newly discovered undated record reappear after the previous set was acknowledged", () => {
    const acknowledged = [session("unknown-a.jsonl", null)];
    const saved = checkpoint(undatedSessionFingerprint(acknowledged));
    const discoveredLater = [...acknowledged, session("unknown-b.jsonl", null)];

    expect(sessionsSinceReview(acknowledged, saved)).toEqual([]);
    expect(sessionsSinceReview(discoveredLater, saved).map((item) => item.path)).toEqual([
      "unknown-a.jsonl",
      "unknown-b.jsonl"
    ]);
  });
});
