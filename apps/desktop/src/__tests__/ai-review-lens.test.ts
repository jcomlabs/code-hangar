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

  it("uses dedicated read-only IPC commands for files and selections", () => {
    expect(connector).toContain('"ai_review_file"');
    expect(connector).toContain('"ai_review_text"');
    expect(connector).toContain('"ai_read_stream"');
  });

  it("asks evidence-led questions and explicitly forbids rewrites", () => {
    expect(backend).toContain("be-careful");
    expect(backend).toContain("double-check");
    expect(backend).toContain("heads-up");
    expect(backend).toContain("Example:");
    expect(backend).toContain("Do not rewrite code");
    expect(backend).toContain("same read and secret gates as Explain");
  });

  it("renders the fixed review vocabulary and keeps malformed small-model output", () => {
    expect(ui).toContain("parseAiReviewSections");
    expect(ui).toContain("This model returned plain text instead of the review structure");
    expect(ui).toContain("Nothing was discarded");
  });
});
