// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const ui = readFileSync(new URL("../views/AiAssist.tsx", import.meta.url), "utf8");
const connector = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../../../../crates/hangar-api/src/ai_assist.rs", import.meta.url), "utf8");

describe("AI review lens contract", () => {
  it("offers Explain and What to check as separate persisted tasks", () => {
    expect(ui).toContain('type AiLens = "explain" | "review"');
    expect(ui).toContain("What to check");
    expect(ui).toContain("read-only review checklist");
    expect(ui).toContain("startAiStreamingTask(key, lens");
  });

  it("uses one narrow read IPC pair for both lenses and requires a one-shot preview", () => {
    expect(connector).toContain('"ai_send_disclosure"');
    expect(connector).toContain('"ai_read_stream"');
    expect(connector).toContain("aiReadStream: async (previewId: string");
    expect(connector).toContain("{ previewId, onEvent }");
    expect(connector).not.toContain('"ai_review_file"');
    expect(connector).not.toContain('"ai_review_text"');
  });

  it("asks evidence-led questions and explicitly forbids rewrites", () => {
    expect(backend).toContain("be-careful");
    expect(backend).toContain("double-check");
    expect(backend).toContain("heads-up");
    expect(backend).toContain("Example:");
    expect(backend).toContain("Do not rewrite code");
    expect(backend).toContain("AiReadLens::Review");
    expect(backend).toContain("pub(crate) fn ai_prepare_read_for_path");
    const preparedRead = backend.split("pub(crate) fn ai_prepare_read_for_path")[1]?.split("fn selected_change_context")[0] ?? "";
    expect(preparedRead).toContain("hangar_ai::prepare_request");
    expect(preparedRead).not.toContain("send_prepared");
  });

  it("renders the fixed review vocabulary and keeps malformed small-model output", () => {
    expect(ui).toContain("parseAiReviewSections");
    expect(ui).toContain("This model returned plain text instead of the review structure");
    expect(ui).toContain("Nothing was discarded");
  });
});
