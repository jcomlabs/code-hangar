// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const ui = readFileSync(new URL("../views/AiLearningTools.tsx", import.meta.url), "utf8");
const connector = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../../../../crates/hangar-api/src/ai_assist.rs", import.meta.url), "utf8");
const api = readFileSync(new URL("../../../../crates/hangar-api/src/lib.rs", import.meta.url), "utf8");

describe("optional guided learning contract", () => {
  it("keeps every learning aid independently collapsed", () => {
    expect(ui.match(/<details/g)?.length).toBe(4);
    expect(ui).toContain("Walk me through this file");
    expect(ui).toContain("Ask about one section");
    expect(ui).toContain("Personal glossary");
    expect(ui).toContain("Anchored notes");
    expect(ui).not.toContain("<details open");
  });

  it("shows a cost preview before both provider sends", () => {
    expect(ui).toContain("costLine(selectedCost");
    expect(ui).toContain("Check send");
    expect(ui).toContain("costLine(followPreview.estTokens");
    expect(ui).toContain("Walk through selected");
    expect(ui).toContain("across this file are available");
    expect(ui).not.toContain("first safe 60 KiB");
  });

  it("uses dedicated connector-only commands", () => {
    for (const command of [
      "ai_walkthrough_preview",
      "ai_walkthrough_file",
      "ai_follow_up_preview",
      "ai_follow_up",
      "ai_glossary_state",
      "ai_annotations_for_node"
    ]) {
      expect(connector).toContain(`"${command}"`);
    }
  });

  it("keeps walkthrough and follow-up provider paths read-only", () => {
    for (const [start, end] of [
      ["pub(crate) fn ai_walkthrough_file_for_path", "fn follow_up_context"],
      ["pub(crate) fn ai_follow_up_for_path", "/// Full, non-truncated bytes"]
    ]) {
      const body = backend.split(start)[1]?.split(end)[0] ?? "";
      expect(body).toContain("hangar_ai::explain");
      for (const forbidden of ["write_file_content", "apply_value_edit", "hangar_mutation", "Command::new"]) {
        expect(body).not.toContain(forbidden);
      }
    }
    expect(api).toContain("AI_FOLLOW_UP_MAX_TURNS: usize = 3");
  });

  it("derives annotation anchors in Rust and never accepts glossary definitions from JS", () => {
    expect(backend).toContain("pub(crate) fn hash_snippet");
    expect(backend).toContain("unique_snippet_line_range");
    expect(connector).toContain("aiGlossaryRecord: (terms: string[])");
    expect(connector).not.toContain("aiGlossaryRecord: (term: string, definition:");
    expect(ui).toContain("Delete this local note permanently");
    expect(ui).toContain("!annotationDeleteAcknowledged");
  });
});
