// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const connector = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const meter = readFileSync(new URL("../views/AiUsageMeter.tsx", import.meta.url), "utf8");
const tauri = readFileSync(new URL("../../src-tauri/src/main.rs", import.meta.url), "utf8");
const editionCheck = readFileSync(new URL("../../../../scripts/check-frontend-edition.mjs", import.meta.url), "utf8");
const aiBackend = readFileSync(new URL("../../../../crates/hangar-ai/src/lib.rs", import.meta.url), "utf8");
const aiAssist = readFileSync(new URL("../views/AiAssist.tsx", import.meta.url), "utf8");
const learning = readFileSync(new URL("../views/AiLearningTools.tsx", import.meta.url), "utf8");
const recap = readFileSync(new URL("../views/RecapAiLayer.tsx", import.meta.url), "utf8");
const rewrite = readFileSync(new URL("../views/RewriteDialog.tsx", import.meta.url), "utf8");
const apiBackend = readFileSync(new URL("../../../../crates/hangar-api/src/ai_assist.rs", import.meta.url), "utf8");

const inferenceCommands = [
  "ai_explain_file",
  "ai_explain_text",
  "ai_review_file",
  "ai_review_text",
  "ai_read_stream",
  "ai_walkthrough_file",
  "ai_follow_up",
  "ai_narrate_session_changes",
  "ai_explain_change",
  "ai_review_change_set",
  "ai_rewrite_text",
  "ai_summarize_project",
  "ai_provider_test"
] as const;

describe("AI session usage contract", () => {
  it("refreshes the shared meter after every model inference, including failures", () => {
    expect(connector).toContain("finally {\n    notifyAiUsageChanged();");
    for (const command of inferenceCommands) {
      expect(connector).toMatch(new RegExp(`metered\\\\?\\(optionalCommand<[^\\n]+>\\(\"${command}\"|metered\\(optionalCommand<[^\\n]+>\\(\"${command}\"`));
    }
  });

  it("does not count local previews, discovery, model listing or request disclosure as inference", () => {
    for (const command of [
      "ai_explain_preview",
      "ai_send_disclosure",
      "ai_walkthrough_preview",
      "ai_follow_up_preview",
      "ai_change_set_preview",
      "ai_summarize_project_preview",
      "ai_summarize_project_disclosure",
      "ai_provider_models",
      "ai_local_discover"
    ]) {
      const line = connector.split("\n").find((candidate: string) => candidate.includes(`\"${command}\"`));
      expect(line, command).toBeDefined();
      expect(line, command).not.toContain("metered(");
    }
  });

  it("keeps the cap advisory and avoids inventing provider prices", () => {
    expect(meter).toContain("Soft-cap warning.");
    expect(meter).toContain("It remains your choice to continue.");
    expect(meter).toContain("Token estimates are not a provider bill");
    expect(meter).toContain("Local calls have no per-token API charge.");
    expect(meter).toContain("No prompt or answer is kept by this meter.");
    expect(meter).not.toMatch(/[€£]|\b(?:USD|EUR|GBP)\b|estimated cost|price per token/i);
  });

  it("projects each operation's bounded output instead of one misleading global value", () => {
    expect(aiAssist).toContain("projectedInputTokens={showEndpoint ? 17 : undefined}");
    expect(aiAssist).toContain("projectedOutputTokens={16}");
    expect(learning).toContain("projectedOutputTokens={900}");
    expect(recap).toContain('mode === "learn" ? 900 : 1_200');
    expect(rewrite).toContain("Math.min(4_096, Math.max(256");
    expect(apiBackend).toContain("const REWRITE_MAX_TOKENS: u32 = 4096;");
    expect(apiBackend).toContain("rewrite_output_allowance(snippet)");
    expect(apiBackend).not.toContain("const REWRITE_MAX_TOKENS: u32 = 16384;");
  });

  it("instruments every backend path that can contact a model", () => {
    for (const functionName of ["explain", "explain_stream", "provider_test"]) {
      const start = aiBackend.indexOf(`pub fn ${functionName}`);
      expect(start, functionName).toBeGreaterThanOrEqual(0);
      const next = aiBackend.indexOf("\npub fn ", start + 1);
      const body = aiBackend.slice(start, next < 0 ? undefined : next);
      expect(body, functionName).toContain("record_usage(");
    }
  });

  it("exposes usage only in the Connector edition", () => {
    for (const command of [
      "ai_usage_status",
      "ai_usage_set_soft_cap",
      "ai_usage_reset",
      "ai_summarize_project_preview",
      "ai_summarize_project_disclosure"
    ]) {
      expect(editionCheck).toContain(`\"${command}\"`);
    }
    const baseHandler = tauri.slice(
      tauri.indexOf("#[cfg(not(feature = \"mutation\"))]"),
      tauri.indexOf("#[cfg(all(feature = \"mutation\", not(feature = \"agent_automation\")))]")
    );
    const mutationHandler = tauri.slice(
      tauri.indexOf("#[cfg(all(feature = \"mutation\", not(feature = \"agent_automation\")))]"),
      tauri.indexOf("#[cfg(feature = \"agent_automation\")]")
    );
    for (const command of ["ai_usage_status", "ai_usage_set_soft_cap", "ai_usage_reset"]) {
      expect(baseHandler).not.toContain(command);
      expect(mutationHandler).not.toContain(command);
    }
  });
});
