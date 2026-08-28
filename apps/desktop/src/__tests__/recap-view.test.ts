import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { fixtureApi } from "../fixtures";
import type { SessionDiscoveryCandidate } from "../types";
import { nextRecapFileLimit, RecapChangeSet, RecapView, reviewCheckpointSavedMessage } from "../views/RecapView";
import { projectViewLabel, projectViewPrefersWideCanvas } from "../workspaceRoute";

const session: SessionDiscoveryCandidate = {
  path: "fixture://codex-session.jsonl",
  displayName: "Safer project removal",
  sourceKind: "codex",
  sourceLabel: "Codex",
  sessionKind: "Codex session",
  confidence: "high",
  linkedProjectPaths: ["fixture://project"],
  linkedRegisteredProjectIds: [1],
  association: "registered_project",
  modifiedMs: 123
};
const recapSource = readFileSync(new URL("../views/RecapView.tsx", import.meta.url), "utf8");
const beginnerHelpSource = readFileSync(new URL("../BeginnerHelp.tsx", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../api.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

describe("project recap", () => {
  it("keeps large file recaps progressive while retaining an explicit show-all path", () => {
    expect(nextRecapFileLimit(20, 73)).toBe(40);
    expect(nextRecapFileLimit(60, 73)).toBe(73);
  });

  it("treats What changed as a wide project workspace", () => {
    expect(projectViewLabel("recap")).toBe("What changed");
    expect(projectViewPrefersWideCanvas("recap")).toBe(true);
  });

  it("states the evidence boundary before loading a session", () => {
    const html = renderToStaticMarkup(createElement(RecapView, {
      projectId: 1,
      sessions: [session],
      onOpenSession: vi.fn()
    }));

    expect(html).toContain("See what your AI tools changed, in plain language");
    expect(html).toContain("Read-only review");
    expect(html).toContain("Nothing on this page can commit, push, change a branch");
    expect(html).toContain("Safer project removal");
    expect(html).toContain("New since I last reviewed");
    expect(html).toContain("Changes Git can see");
    expect(html).toContain("AI conversations");
    expect(beginnerHelpSource).toContain("A commit is a named snapshot");
    expect(beginnerHelpSource).toContain("does not commit, push or change branches");
  });

  it("provides representative deterministic diff evidence in fixture mode", async () => {
    const result = await fixtureApi.sessionChangeSet(session.path);

    expect(result.coverage.level).toBe("full");
    expect(result.files).toHaveLength(1);
    expect(result.files[0].edits[0].request).toContain("safer");
    expect(result.files[0].edits[0].reality?.status).toBe("applied");
    expect(result.addedLines).toBe(2);
    expect(result.removedLines).toBe(1);
  });

  it("renders the honest coverage banner and recorded request before the diff", async () => {
    const result = await fixtureApi.sessionChangeSet(session.path);
    const html = renderToStaticMarkup(createElement(RecapChangeSet, { changeSet: result }));

    expect(html).toContain("recap-coverage coverage-full");
    expect(html).toContain(result.coverage.label);
    expect(html).toContain(result.coverage.note);
    expect(html).toContain("You asked");
    expect(html).toContain("Make project removal safer");
    expect(html).toContain("+2");
    expect(html).toContain("-1");
  });

  it("keeps checkpoint, fused evidence, ledger and per-edit reality visible in the contract", () => {
    expect(recapSource).toContain("markProjectReviewed");
    expect(recapSource).toContain("All local clues together");
    expect(recapSource).toContain("retainedLedger");
    expect(recapSource).toContain("edit.reality");
    expect(apiSource).toContain('"project_recap"');
    expect(apiSource).toContain('"project_review_ledger"');
    expect(apiSource).toContain('"project_review_receipt_export"');
    expect(recapSource).toContain("Save private review record");
    expect(recapSource).toContain("How Code Hangar worked this out");
    expect(recapSource).toContain("Show technical source");
  });

  it("confirms the saved review boundary and explains the next new-changes scope", () => {
    const message = reviewCheckpointSavedMessage("2026-08-23T12:00:00.000Z");

    expect(message).toContain("Review point saved");
    expect(message).toContain("Future “New since I last reviewed” views start after this point");
  });

  it("never hides What changed prose or paths behind a silent ellipsis", () => {
    const recapCss = styles.slice(styles.indexOf(".recap-home"), styles.indexOf(".overview-recap-queue"));
    expect(recapCss).not.toContain("text-overflow: ellipsis");
    expect(recapSource).toContain("ExpandableText");
    expect(recapSource).toContain("Show the full request");
  });
});
