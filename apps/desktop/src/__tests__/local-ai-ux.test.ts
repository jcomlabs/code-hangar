// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const ui = readFileSync(new URL("../views/AiAssist.tsx", import.meta.url), "utf8");
const projectSummary = readFileSync(new URL("../views/ProjectAiSummary.tsx", import.meta.url), "utf8");
const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const connector = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const fixtures = readFileSync(new URL("../fixtures.ts", import.meta.url), "utf8");
const aiBackend = readFileSync(new URL("../../../../crates/hangar-ai/src/lib.rs", import.meta.url), "utf8");
const tauri = readFileSync(new URL("../../src-tauri/src/main.rs", import.meta.url), "utf8");
const editionGate = readFileSync(new URL("../../../../scripts/check-frontend-edition.mjs", import.meta.url), "utf8");

describe("local-first AI UX contract", () => {
  it("discovers loopback providers only after an explicit click", () => {
    expect(ui).toContain("Find local models");
    expect(ui).toContain("onClick={() => void discoverLocal()}");
    expect(ui).toContain("const found = await api.aiLocalDiscover()");
    const mountEffect = ui.slice(ui.indexOf("// Local backend reads only"), ui.indexOf("if (!config)"));
    expect(mountEffect).not.toContain("aiLocalDiscover");
  });

  it("probes a fixed numeric loopback allowlist with no discovery DNS name", () => {
    for (const endpoint of [
      "http://127.0.0.1:11434/v1",
      "http://127.0.0.1:1234/v1",
      "http://127.0.0.1:8000/v1",
      "http://127.0.0.1:8080/v1"
    ]) expect(aiBackend).toContain(endpoint);
    const discovery = aiBackend.slice(aiBackend.indexOf("pub fn discover_local_providers"), aiBackend.indexOf("#[cfg(test)]"));
    expect(discovery).not.toContain("localhost");
    expect(discovery).not.toContain("0.0.0.0");
  });

  it("quick-fills GPT-5.6 only in the Connector edition and keeps the provider off by default", () => {
    expect(ui).toContain('label: "OpenAI · GPT-5.6"');
    expect(ui).toContain('model: "gpt-5.6"');
    expect(ui).toContain('baseUrl: "https://api.openai.com/v1"');
    expect(ui).toContain('model: preset.model ?? ""');
    expect(ui).toContain('const EMPTY_CONFIG: AiProviderConfig = { mode: "off"');
    expect(editionGate).toContain('"gpt-5.6"');
  });

  it("requires the literal request review before Explain or project-summary sends", () => {
    expect(ui).toContain("Review exactly what is sent");
    expect(ui).toContain("await api.aiSendDisclosure(");
    expect(ui).toContain("<pre>{disclosure.requestBody}</pre>");
    expect(ui).toContain("Possible non-stream fallback body");
    expect(ui).toContain("if (!canSend || !disclosure) return;");
    expect(ui).toContain("[key, lens, level, model]");
    expect(ui).toContain("startAiStreamingTask(key, lens");
    expect(projectSummary).toContain("aiSummarizeProjectDisclosure(projectId, level, providerModel)");
    expect(projectSummary).toContain("if (!disclosure) return;");
    expect(projectSummary).toContain("Review these exact request bytes");
    expect(projectSummary).toContain("<pre>{disclosure.requestBody}</pre>");
    expect(connector).toContain('"ai_summarize_project_disclosure"');
    expect(connector).toContain("new Channel<string>()");
    expect(connector).toContain('"ai_read_stream"');
  });

  it("keeps discovery, disclosure and streaming out of non-Connector handlers", () => {
    const baseHandler = tauri.slice(
      tauri.indexOf("#[cfg(not(feature = \"mutation\"))]"),
      tauri.indexOf("#[cfg(all(feature = \"mutation\", not(feature = \"agent_automation\")))]")
    );
    const mutationHandler = tauri.slice(
      tauri.indexOf("#[cfg(all(feature = \"mutation\", not(feature = \"agent_automation\")))]"),
      tauri.indexOf("#[cfg(feature = \"agent_automation\")]")
    );
    for (const command of ["ai_send_disclosure", "ai_summarize_project_disclosure", "ai_read_stream", "ai_local_discover"]) {
      expect(baseHandler).not.toContain(command);
      expect(mutationHandler).not.toContain(command);
    }
  });

  it("exposes Connector-only panels to browser acceptance without widening the Local fixture", () => {
    expect(fixtures).toContain('const connectorAcceptanceFixture = import.meta.env.MODE === "connector"');
    expect(fixtures).toContain('connectorAcceptanceFixture ? ["core", "agent_automation"] : ["core"]');
    expect(app).toContain('if (!import.meta.env.DEV || !connectorBuild || acceptanceAiPanelOpened.current) return');
    expect(app).toContain('get("acceptanceAiPanel") !== "file"');
    expect(connector).toContain('model: "fixture-local-model"');
    expect(connector).toContain('url: "http://127.0.0.1:11434/v1/chat/completions"');
    expect(connector).toContain('browserFixtureUsage(projectedInputTokens, projectedOutputTokens)');
  });
});
