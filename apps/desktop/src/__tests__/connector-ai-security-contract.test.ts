// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const connector = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const tauri = readFileSync(new URL("../../src-tauri/src/main.rs", import.meta.url), "utf8");
const api = readFileSync(new URL("../../../../crates/hangar-api/src/lib.rs", import.meta.url), "utf8");
const assist = readFileSync(new URL("../../../../crates/hangar-api/src/ai_assist.rs", import.meta.url), "utf8");
const advisoryApi = readFileSync(new URL("../../../../crates/hangar-api/src/connector_advisory.rs", import.meta.url), "utf8");
const ai = readFileSync(new URL("../../../../crates/hangar-ai/src/lib.rs", import.meta.url), "utf8");
const coreAi = readFileSync(new URL("../../../../crates/hangar-core/src/connector_ai.rs", import.meta.url), "utf8");
const database = readFileSync(new URL("../../../../crates/hangar-db/src/lib.rs", import.meta.url), "utf8");
const advisoryDatabase = readFileSync(new URL("../../../../crates/hangar-db/src/connector_advisory.rs", import.meta.url), "utf8");
const apiManifest = readFileSync(new URL("../../../../crates/hangar-api/Cargo.toml", import.meta.url), "utf8");
const aiAssistUi = readFileSync(new URL("../views/AiAssist.tsx", import.meta.url), "utf8");
const aiLearningUi = readFileSync(new URL("../views/AiLearningTools.tsx", import.meta.url), "utf8");
const editionLayer = readFileSync(new URL("../views/ConnectorEditionLayer.tsx", import.meta.url), "utf8");
const projectSummaryUi = readFileSync(new URL("../views/ProjectAiSummary.tsx", import.meta.url), "utf8");
const recapAiUi = readFileSync(new URL("../views/RecapAiLayer.tsx", import.meta.url), "utf8");

function between(source: string, start: string, end: string): string {
  const body = source.split(start)[1]?.split(end)[0];
  expect(body, `missing source contract boundary ${start}`).toBeDefined();
  return body ?? "";
}

describe("Connector AI fail-closed contract", () => {
  it("keeps every content-bearing send behind its reviewed one-shot preview id", () => {
    for (const [method, nextMethod, command] of [
      ["aiReadStream:", "aiWalkthroughPreview:", "ai_read_stream"],
      ["aiWalkthroughFile:", "aiFollowUpPreview:", "ai_walkthrough_file"],
      ["aiFollowUp:", "aiGlossaryState:", "ai_follow_up"],
      ["aiNarrateSessionChanges:", "aiExplainChange:", "ai_narrate_session_changes"],
      ["aiExplainChange:", "aiReviewChangeSet:", "ai_explain_change"],
      ["aiReviewChangeSet:", "aiRewriteDisclosure:", "ai_review_change_set"],
      ["aiRewriteText:", "applyAiSuggestion:", "ai_rewrite_text"],
      ["aiSummarizeProject:", "aiSummarizeProjectPreview:", "ai_summarize_project"],
      ["aiSafeManageAdvisory:", "aiKeySet:", "ai_safe_manage_advisory"]
    ] as const) {
      const body = between(connector, method, nextMethod);
      expect(body, method).toContain("previewId");
      expect(body, method).toContain(`\"${command}\"`);
    }
    for (const removedDirectCommand of [
      "ai_explain_file",
      "ai_explain_text",
      "ai_review_file",
      "ai_review_text"
    ]) {
      expect(connector).not.toContain(`\"${removedDirectCommand}\"`);
      expect(tauri).not.toContain(`fn ${removedDirectCommand}(`);
    }
  });

  it("keeps provider Test and both model-list refreshes behind typed one-shot previews", () => {
    for (const [disclosureMethod, confirmMethod, nextMethod, disclosureCommand, confirmCommand] of [
      ["aiProviderTestDisclosure:", "aiProviderTest:", "aiProviderModelsDisclosure:", "ai_provider_test_disclosure", "ai_provider_test"],
      ["aiProviderModelsDisclosure:", "aiProviderModels:", "aiLocalDiscover:", "ai_provider_models_disclosure", "ai_provider_models"]
    ] as const) {
      const prepare = between(connector, disclosureMethod, confirmMethod);
      expect(prepare).toContain(`\"${disclosureCommand}\"`);
      expect(prepare).toContain("mode");
      expect(prepare).toContain("baseUrl");
      expect(prepare).toContain("model");
      expect(prepare).toContain("format");
      expect(prepare).not.toContain("previewId: string");

      const confirm = between(connector, confirmMethod, nextMethod);
      expect(confirm).toContain(`\"${confirmCommand}\"`);
      expect(confirm).toContain("previewId");
      expect(confirm).not.toContain("baseUrl");
      expect(confirm).not.toContain("format");
    }

    const testConfirm = between(
      tauri,
      "async fn ai_provider_test(",
      "/// AI Assist: prepare the exact best-effort model-list GET"
    );
    const modelsConfirm = between(
      tauri,
      "async fn ai_provider_models(",
      "/// AI Assist: explicit loopback-only discovery"
    );
    for (const body of [testConfirm, modelsConfirm]) {
      expect(body).toContain("State<'_, AppState>");
      expect(body).toContain("preview_id: String");
      expect(body).not.toContain("base_url: String");
      expect(body).not.toContain("model: String");
      expect(body).not.toContain("format: String");
    }

    const providerApi = between(api, "pub fn ai_provider_test_disclosure(", "pub fn ai_local_discover(");
    expect(providerApi).toContain("AiPreparedKind::ProviderTest");
    expect(providerApi).toContain("AiPreparedKind::ProviderModels");
    expect(providerApi).toContain("take_ai_prepared_send(state, preview_id, AiPreparedKind::ProviderTest)");
    expect(providerApi).toContain("take_ai_prepared_send(state, preview_id, AiPreparedKind::ProviderModels)");
    expect(providerApi).toContain("hangar_ai::send_prepared(request)");
    expect(providerApi).toContain("hangar_ai::send_prepared_provider_models(request)");
    expect(providerApi).not.toContain("hangar_ai::provider_test(");
    expect(providerApi).not.toContain("hangar_ai::provider_models(");

    expect(ai).toContain('method: "GET".to_string()');
    expect(ai).toContain("request_body: String::new()");
    expect(ai).toContain("verify_prepared_request(&request)?");
  });

  it("consumes before validation, expires quickly, compares rebuilt bytes and never persists previews", () => {
    const stageAndTake = between(api, "const AI_PREPARED_SEND_TTL:", "pub fn ai_explain_preview");
    expect(stageAndTake).toContain("Duration::from_secs(2 * 60)");
    expect(stageAndTake).toContain("AI_PREPARED_SEND_CAP: usize = 32");
    const take = between(stageAndTake, "fn take_ai_prepared_send_pending(", "fn ensure_ai_prepared_request_still_matches(");
    expect(take).toContain(".remove(preview_id)");
    expect(take.indexOf(".remove(preview_id)")).toBeLessThan(take.indexOf("pending.created_at"));
    expect(take).toContain("monotonic_now.saturating_duration_since(pending.created_at)");
    expect(take).not.toContain("now_millis()");
    expect(stageAndTake).toContain("Wall time is presentation-only");
    expect(stageAndTake).toContain("reviewed.disclosure() != rebuilt.disclosure()");
    expect(stageAndTake).toContain("reviewed.model() != rebuilt.model()");
    expect(stageAndTake).toContain("ai_provider_credential_binding()");
    expect(stageAndTake).not.toContain("set_ai_provider_credential_binding(");
    expect(stageAndTake).not.toContain("fs::write");

    const assistProduction = assist.split("\n#[cfg(test)]")[0] ?? assist;
    expect(assistProduction).toContain("hangar_ai::prepare_request");
    expect(assistProduction).not.toContain("hangar_ai::send_prepared");
  });

  it("keeps Local native handlers and dependency graph free of the provider transport", () => {
    const baseHandler = between(
      tauri,
      '#[cfg(not(feature = "mutation"))]',
      '#[cfg(all(feature = "mutation", not(feature = "agent_automation")))]'
    );
    const mutationHandler = between(
      tauri,
      '#[cfg(all(feature = "mutation", not(feature = "agent_automation")))]',
      '#[cfg(feature = "agent_automation")]'
    );
    for (const marker of ["ai_send_disclosure", "ai_read_stream", "ai_provider_get", "ai_key_set", "ai_local_discover"]) {
      expect(baseHandler, marker).not.toContain(marker);
      expect(mutationHandler, marker).not.toContain(marker);
    }
    expect(apiManifest).toContain('hangar-ai = { path = "../hangar-ai", optional = true }');
    expect(apiManifest).toContain('agent_automation = ["mutation"');
    expect(apiManifest).toContain('"dep:hangar-ai"');
    expect(apiManifest.split("core = [")[1]?.split("]")[0] ?? "").not.toContain("hangar-ai");
  });

  it("keeps keys out of SQLite and responses, and keeps model output advisory", () => {
    const providerSettings = between(database, "const AI_PROVIDER_MODE_KEY", "const AI_GLOSSARY_ENABLED_KEY");
    for (const allowed of ["MODE", "BASE_URL", "MODEL", "FORMAT"]) {
      expect(providerSettings).toContain(`AI_PROVIDER_${allowed}_KEY`);
    }
    for (const metadata of ["CREDENTIAL_ORIGIN", "CREDENTIAL_FINGERPRINT", "CREDENTIAL_VERSION", "CREDENTIAL_STATUS"]) {
      expect(providerSettings).toContain(`AI_PROVIDER_${metadata}_KEY`);
    }
    expect(providerSettings).not.toMatch(/API_KEY|PASSWORD|SECRET/);
    expect(coreAi).toContain("pub credential_use: AiCredentialUse");
    expect(coreAi).toContain("BearerSaved");
    expect(coreAi).toContain("XApiKeySaved");

    const disclosureType = between(ai, "pub struct RequestDisclosure", "pub struct PreparedRequest");
    const disclosureFields = disclosureType.split("{")[1]?.split("}")[0] ?? "";
    expect(disclosureFields).not.toMatch(/key|credential|authorization/i);
    expect(ai).toContain("#[cfg(windows)]\nfn entry(");
    expect(ai).toContain("#[cfg(not(windows))]\nfn entry(");
    expect(ai).toContain("Windows secure credential storage");
    const keyStatus = between(tauri, "async fn ai_key_status(", "/// AI Assist: revoke the origin binding");
    expect(keyStatus).toContain("State<'_, AppState>");
    expect(keyStatus).toContain("hangar_api::ai_key_status(&app_state)");
    expect(tauri).not.toContain("async fn ai_key_get");

    const rewriteProposal = between(api, "pub fn ai_rewrite_text(", "pub fn ai_rewrite_disclosure(");
    expect(rewriteProposal).not.toContain("write_file_with_snapshot");
    expect(rewriteProposal).not.toContain("apply_ai_suggestion(");
    const apply = between(api, "pub fn apply_ai_suggestion(", "pub fn ai_summarize_project(");
    expect(apply).toContain("write_file_with_snapshot");
  });

  it("discloses the exact non-secret credential scheme in every AI send consumer", () => {
    expect(connector).toContain("export function formatAiCredentialUse(");
    expect(connector).toContain('none: "No credential will be attached"');
    expect(connector).toContain('bearer_saved: "Saved credential will be attached as Authorization: Bearer (value hidden)"');
    expect(connector).toContain('x_api_key_saved: "Saved credential will be attached as x-api-key (value hidden)"');
    const confirmation = between(connector, "export function confirmExactAiSend(", "/**\n * Connector-only local IPC surface");
    expect(confirmation).toContain("formatAiCredentialUse(disclosure.credentialUse)");

    for (const [name, source] of [
      ["AiAssist", aiAssistUi],
      ["Safe Manage", editionLayer],
      ["Project AI summary", projectSummaryUi]
    ] as const) {
      expect(source, name).toContain("formatAiCredentialUse");
      expect(source, name).toContain("formatAiCredentialUse(disclosure.credentialUse)");
    }

    for (const [name, source] of [
      ["AiAssist probes", aiAssistUi],
      ["AI learning tools", aiLearningUi],
      ["Connector rewrite", editionLayer],
      ["Recap AI", recapAiUi]
    ] as const) {
      expect(source, name).toContain("confirmExactAiSend(disclosure)");
    }
  });

  it("linearizes every prepared provider send against credential mutations", () => {
    for (const [start, end] of [
      ["pub fn ai_read_stream<", "pub fn ai_walkthrough_preview("],
      ["pub fn ai_walkthrough_file(", "pub fn ai_walkthrough_disclosure("],
      ["pub fn ai_follow_up(", "pub fn ai_follow_up_disclosure("],
      ["pub fn ai_narrate_session_changes(", "pub struct AiRecordedEditSelector"],
      ["pub fn ai_explain_change(", "pub fn ai_review_change_set("],
      ["pub fn ai_review_change_set(", "const AI_REWRITE_PROPOSAL_CAP"],
      ["pub fn ai_rewrite_text(", "pub fn ai_rewrite_disclosure("],
      ["pub fn ai_summarize_project(", "pub fn ai_summarize_project_preview("],
      ["pub fn ai_provider_test(", "pub fn ai_provider_models_disclosure("],
      ["pub fn ai_provider_models(", "pub fn ai_local_discover("]
    ] as const) {
      const body = between(api, start, end);
      expect(body, start).toContain("lock_ai_credential_operation(state)?");
      expect(body.indexOf("lock_ai_credential_operation(state)?"), start)
        .toBeLessThan(body.indexOf("take_ai_prepared_send("));
      expect(body, start).toContain("hangar_ai::send_prepared");
    }
    const advisory = between(advisoryApi, "pub fn ai_safe_manage_advisory(", "pub fn ai_safe_manage_advisory_receipts(");
    expect(advisory).toContain("lock_ai_credential_operation(state)?");
    expect(advisory.indexOf("lock_ai_credential_operation(state)?"))
      .toBeLessThan(advisory.indexOf("take_ai_prepared_safe_manage_send("));
    expect(advisory).toContain("hangar_ai::send_prepared(reviewed)");
  });

  it("keeps AI-enriched Safe Manage recommendations exact, redacted, one-shot and non-authoritative", () => {
    expect(connector).toContain('"ai_safe_manage_context_candidates"');
    expect(connector).toContain('"ai_safe_manage_advisory_disclosure"');
    expect(connector).toContain('"ai_safe_manage_advisory"');
    expect(connector).toContain('"ai_safe_manage_advisory_receipts"');
    expect(editionLayer).toContain("aiSafeManageContextCandidates(");
    expect(editionLayer).toContain("aiSafeManageAdvisoryDisclosure(");
    expect(editionLayer).toContain("selectedContextIds");
    expect(editionLayer).toContain('type="checkbox"');
    expect(editionLayer).toContain("Nothing is selected by default");
    expect(editionLayer).toContain("setDisclosure(null);");
    expect(editionLayer).toContain("reviewed.previewId");
    expect(editionLayer).toContain("Enrich this recommendation with AI");
    expect(editionLayer).toContain("Send this exact recommendation request");
    expect(editionLayer).toContain("cannot record your decision or run a disk action");
    expect(editionLayer).toContain("Deterministic baseline");
    expect(editionLayer).toContain("AI recommendation");
    expect(editionLayer).toContain("result.recommendationChanged");
    expect(editionLayer).toContain("does not record your decision, build an OperationPlan");

    const advisory = between(advisoryApi, "pub fn ai_safe_manage_advisory(", "pub fn ai_safe_manage_advisory_receipts(");
    expect(advisory.indexOf("take_ai_prepared_safe_manage_send(")).toBeLessThan(advisory.indexOf("resolve_safe_manage_ai_assessment("));
    expect(advisory).toContain("ensure_ai_prepared_request_still_matches");
    expect(advisory).toContain("hangar_ai::send_prepared(reviewed)");
    expect(advisory).toContain("parse_safe_manage_ai_recommendation(&advisory)");
    expect(advisory).toContain("connector_advisory_receipt_complete");
    expect(advisory).not.toContain("safe_manage_decision_record(");
    expect(advisory).not.toContain("operation_plan_build(");
    expect(advisory).not.toContain("apply_ai_suggestion(");
    expect(advisory).not.toContain("mutation_");

    const prepared = between(assist, "fn safe_manage_advisory_context(", "const CONTEXT_EXCERPT_MAX_CHARS");
    expect(prepared).toContain("protectedOrSensitiveFileDetailsOmitted");
    expect(prepared).toContain("explicitlySelectedContext");
    expect(prepared).toContain("context.selection_id");
    expect(prepared).toContain("secret_reasons(&context)");
    expect(prepared).toContain("Project/file/session names and paths");
    expect(prepared).toContain("materially different recommendation");
    expect(prepared).toContain("[recommended-action]");
    expect(prepared).toContain("cannot record the user's decision");
    expect(prepared).toContain("Missing or omitted context is unknown");
    expect(prepared).not.toContain('"projectPath"');
    expect(prepared).not.toContain('"projectName"');
    expect(prepared).not.toContain('"relatedProjectIds"');
  });

  it("binds explicit selectors in memory and persists only typed fingerprints/counts", () => {
    expect(advisoryApi).toContain("SafeManageContextSelectionStore");
    expect(advisoryApi).toContain("hangar_agent::random_token(24)");
    expect(advisoryApi).toContain("validate_selected_ids(selected_context_ids)");
    expect(advisoryApi).toContain("selection.project_id != assessment.project_id");
    expect(advisoryApi).toContain("file_preview(");
    expect(advisoryApi).toContain("session_preview(path.clone(), false)");
    expect(advisoryApi).toContain("redact_absolute_path_tokens");
    expect(advisoryApi).toContain("validate_safe_manage_advisory_excerpt");
    expect(advisoryApi).not.toContain("gated_context_excerpt(");

    const schema = between(
      advisoryDatabase,
      '"CREATE TABLE IF NOT EXISTS connector_safe_manage_advisory_receipt',
      'CREATE INDEX IF NOT EXISTS idx_connector_advisory_receipt_project'
    );
    for (const forbidden of ["request_body", "result_body", "payload", "credential", "secret", "source_path", "model", "endpoint", "url"]) {
      expect(schema.toLowerCase(), forbidden).not.toContain(forbidden);
    }
    expect(schema).toContain("request_hash");
    expect(schema).toContain("result_hash");
    expect(schema).toContain("source_provenance_json");
    expect(advisoryDatabase).toContain("AiSafeManageAdvisorySourceReceipt");
    expect(advisoryDatabase).not.toContain("request.request_body");
  });
});
