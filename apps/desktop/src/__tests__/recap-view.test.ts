import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { fixtureApi } from "../fixtures";
import type { SessionDiscoveryCandidate } from "../types";
import { parseRecapAiSections } from "../views/RecapAiLayer";
import { nextRecapFileLimit, RecapChangeSet, RecapView } from "../views/RecapView";
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
const recapAiSource = readFileSync(new URL("../views/RecapAiLayer.tsx", import.meta.url), "utf8");
const beginnerHelpSource = readFileSync(new URL("../BeginnerHelp.tsx", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../api.ts", import.meta.url), "utf8");
const connectorSource = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const recapAiStyles = readFileSync(new URL("../views/recap-ai.css", import.meta.url), "utf8");

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

  it("renders structured change narration and preserves an unstructured small-model response", () => {
    const sections = parseRecapAiSections("[story]\n- Changed the label.\n[unknowns]\n- Shell activity is not visible.");
    expect(sections.map((section) => section.key)).toEqual(["story", "unknowns"]);
    expect(sections[0].lines[0]).toBe("Changed the label.");
    expect(parseRecapAiSections("A small local model returned plain text."))
      .toEqual([{ key: "response", title: "Model response", lines: ["A small local model returned plain text."] }]);
  });

  it("keeps retrospective AI commands in the connector-only module", () => {
    for (const command of [
      "ai_change_set_preview",
      "ai_narrate_session_changes",
      "ai_explain_change",
      "ai_review_change_set"
    ]) {
      expect(connectorSource).toContain(`\"${command}\"`);
      expect(apiSource).not.toContain(`\"${command}\"`);
    }
    expect(recapAiSource).toContain("checked characters");
    expect(recapSource).toContain("edit.reality");
    expect(recapSource).not.toContain("Plain-language guide");
    expect(recapSource).not.toContain("aiNarrateSessionChanges");
  });

  it("makes the connector explanation prominent and binds it to the selected evidence scope", () => {
    expect(recapAiSource).toContain("useState(true)");
    expect(recapAiSource).toContain("Explain these changes with AI");
    expect(recapAiSource).toContain("AI Assist is off");
    expect(recapAiSource).toContain("sourceMode");
    expect(connectorSource).toContain("sourceMode: RecapAiSourceMode");
    expect(connectorSource).toContain("{ projectId, sessionPaths, sourceMode");
    expect(recapSource).toContain('selectedKey === "git" ? "git" : "combined"');
  });

  it("never hides What changed prose or paths behind a silent ellipsis", () => {
    const recapCss = styles.slice(styles.indexOf(".recap-home"), styles.indexOf(".overview-recap-queue"));
    expect(recapCss).not.toContain("text-overflow: ellipsis");
    expect(recapAiStyles).not.toContain("text-overflow: ellipsis");
    expect(recapSource).toContain("ExpandableText");
    expect(recapSource).toContain("Show the full request");
  });
});
