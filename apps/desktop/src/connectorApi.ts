import { Channel } from "@tauri-apps/api/core";
import { call, hasTauriRuntime, optionalCommand } from "./api";
import type {
  ResolveInputs,
  AgentActionRequest,
  AiExplainPreview,
  AiFollowUpResult,
  AiGlossaryState,
  AiEditSessionSummary,
  AiProjectSummary,
  AiLocalProviderCandidate,
  AiProviderConfig,
  AiSendDisclosure,
  AiRewriteProposal,
  AiSafeManageAdvisoryReceipt,
  AiSafeManageAdvisoryResult,
  AiSafeManageContextCandidate,
  AiSuggestionApplyResult,
  AiUsageStatus,
  AiWalkthroughPreview,
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationReadGrant,
  AutomationStatus,
  CodeAnnotation,
  ConnectedAppStatus
} from "./connectorTypes";
import type { EditSnapshotRestoreResult, RecapAiSourceMode } from "./types";

const unavailable = async (): Promise<never> => {
  throw new Error("AI Connector is not enabled in this edition.");
};

const browserFixtureProvider: AiProviderConfig = {
  mode: "local",
  baseUrl: "http://127.0.0.1:11434/v1",
  model: "fixture-local-model",
  format: "chat_completions"
};

function browserFixtureDisclosure(model: string, bodyLabel: string, stream: boolean): AiSendDisclosure {
  const requestBody = JSON.stringify({
    model: model || browserFixtureProvider.model,
    stream,
    messages: [{ role: "user", content: bodyLabel }]
  });
  return {
    previewId: "browser-fixture-preview",
    receiptId: null,
    expiresAtUnix: Math.floor(Date.now() / 1000) + 120,
    method: "POST",
    url: "http://127.0.0.1:11434/v1/chat/completions",
    requestBody,
    fallbackRequestBody: stream ? requestBody.replace('"stream":true', '"stream":false') : null,
    transport: stream ? "Local streaming with one disclosed fallback." : "Complete response; no automatic retry.",
    mode: "local",
    model: model || browserFixtureProvider.model,
    format: "chat_completions",
    credentialUse: "none",
    sendChars: requestBody.length,
    estTokens: Math.ceil(requestBody.length / 4)
  };
}

function browserFixtureModelsDisclosure(model: string): AiSendDisclosure {
  return {
    previewId: "browser-fixture-models-preview",
    receiptId: null,
    expiresAtUnix: Math.floor(Date.now() / 1000) + 120,
    method: "GET",
    url: "http://127.0.0.1:11434/v1/models",
    requestBody: "",
    fallbackRequestBody: null,
    transport: "Model-list lookup; no automatic retry.",
    mode: "local",
    model: model || browserFixtureProvider.model,
    format: "chat_completions",
    credentialUse: "none",
    sendChars: 0,
    estTokens: 0
  };
}

function browserFixtureUsage(projectedInputTokens = 0, projectedOutputTokens = 0): AiUsageStatus {
  const estimatedInputTokens = 2_400;
  const estimatedOutputTokens = 600;
  const estimatedTotalTokens = estimatedInputTokens + estimatedOutputTokens;
  const softCapTokens = 10_000;
  const projectedTotalTokens = estimatedTotalTokens + projectedInputTokens + projectedOutputTokens;
  return {
    sessionStartedUnix: 1_783_990_800,
    requestCount: 3,
    estimatedInputTokens,
    estimatedOutputTokens,
    estimatedTotalTokens,
    softCapTokens,
    remainingTokens: Math.max(0, softCapTokens - estimatedTotalTokens),
    overSoftCap: false,
    projectedTotalTokens,
    wouldExceedSoftCap: projectedTotalTokens > softCapTokens,
    projectedOutputAllowance: projectedOutputTokens
  };
}

export const AI_USAGE_CHANGED_EVENT = "codehangar-ai-usage-changed";

async function metered<T>(operation: Promise<T>): Promise<T> {
  try {
    return await operation;
  } finally {
    if (typeof window !== "undefined") window.dispatchEvent(new Event(AI_USAGE_CHANGED_EVENT));
  }
}

/** Describe only the credential scheme; the saved secret never crosses into the webview. */
export function formatAiCredentialUse(credentialUse: AiSendDisclosure["credentialUse"]): string {
  const labels: Record<AiSendDisclosure["credentialUse"], string> = {
    none: "No credential will be attached",
    bearer_saved: "Saved credential will be attached as Authorization: Bearer (value hidden)",
    x_api_key_saved: "Saved credential will be attached as x-api-key (value hidden)"
  };
  return labels[credentialUse];
}

/** Show the complete secret-free request immediately before consuming its one-shot id. */
export function confirmExactAiSend(disclosure: AiSendDisclosure): boolean {
  const fallback = disclosure.fallbackRequestBody
    ? `\n\nPossible disclosed loopback fallback:\n${disclosure.fallbackRequestBody}`
    : "";
  const requestBody = disclosure.requestBody || "(no request body)";
  return window.confirm(
    `Experimental AI Assist\n\nDestination: ${disclosure.method} ${disclosure.url}\nModel: ${disclosure.model}\nCredential use: ${formatAiCredentialUse(disclosure.credentialUse)}\nTransport: ${disclosure.transport}\n\nExact request body:\n${requestBody}${fallback}\n\nSend exactly this request now?`
  );
}

/**
 * Connector-only local IPC surface. The webview has no network client: provider,
 * key, connected-app and automation actions all cross typed Tauri IPC into the
 * native Connector backend, which owns every transport and security gate.
 */
export const connectorApi = {
  automationStatus: () => optionalCommand<AutomationStatus>("automation_status", undefined, async () => ({
    enabled: false,
    endpoint: null,
    protocol: null,
    registeredAgents: 0,
    message: "Local automation is not compiled into this build."
  })),
  automationAgents: () => optionalCommand<AutomationAgentSummary[]>("automation_agents", undefined, async () => []),
  automationRegister: (name: string, scopes: string[], projectIds: number[]) =>
    call<AutomationCredential>("automation_register", { name, scopes, projectIds }),
  automationRevoke: (agentId: number) => call<boolean>("automation_revoke", { agentId }),
  automationForgetRevoked: (agentId: number) => call<boolean>("automation_forget_revoked", { agentId }),
  automationGrantRead: (agentId: number, nodeId: number, minutes = 10) =>
    call<AutomationReadGrant>("automation_grant_read", { agentId, nodeId, minutes }),
  automationActivity: (limit = 100) => optionalCommand<AutomationActivityEntry[]>("automation_activity", { limit }, async () => []),
  commentWriteEnabled: () => optionalCommand<boolean>("comment_write_enabled", undefined, async () => false),
  setCommentWriteEnabled: (enabled: boolean) => optionalCommand<void>("set_comment_write_enabled", { enabled }, async () => undefined),
  mcpFullControlEnabled: () => optionalCommand<boolean>("mcp_full_control_enabled", undefined, async () => false),
  setMcpFullControlEnabled: (enabled: boolean) => optionalCommand<void>("set_mcp_full_control_enabled", { enabled }, async () => undefined),
  mcpReadOnlyMode: () => optionalCommand<boolean>("mcp_read_only_mode", undefined, async () => false),
  setMcpReadOnlyMode: (enabled: boolean) => optionalCommand<void>("set_mcp_read_only_mode", { enabled }, async () => undefined),
  connectedAppStatus: () => optionalCommand<ConnectedAppStatus[]>("mcp_appconfig_status", undefined, async () => []),
  connectedAppRegister: (
    hostId: string,
    projectIds: number[],
    includeHistorySearch = false,
    includeMutationRequests = false
  ) => call<ConnectedAppStatus>("mcp_appconfig_register", {
    hostId,
    projectIds,
    includeHistorySearch,
    includeMutationRequests
  }),
  connectedAppRemove: (hostId: string) => call<ConnectedAppStatus>("mcp_appconfig_remove", { hostId }),
  connectedAppRevokeOrphan: (hostId: string) =>
    call<ConnectedAppStatus>("mcp_appconfig_revoke_orphan", { hostId }),
  connectedAppForgetOrphan: (hostId: string) =>
    call<ConnectedAppStatus>("mcp_appconfig_forget_orphan", { hostId }),
  agentRequestsPending: () => optionalCommand<AgentActionRequest[]>("agent_requests_pending", undefined, async () => []),
  agentRequestResolve: (requestId: number, approve: boolean, inputs: ResolveInputs) =>
    call<AgentActionRequest>("agent_request_resolve", { requestId, approve, inputs }),
  aiExplainPreview: (nodeId: number) =>
    optionalCommand<AiExplainPreview>("ai_explain_preview", { nodeId }, async () => ({ blocked: [], sendChars: 414, estTokens: 104, language: "Markdown" })),
  aiSendDisclosure: (nodeId: number, snippet: string | null, lens: string, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_send_disclosure", { nodeId, snippet, lens, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local fixture content]", true)),
  aiReadStream: async (previewId: string, onDelta: (delta: string) => void) => {
    if (!hasTauriRuntime()) return unavailable();
    const onEvent = new Channel<string>();
    onEvent.onmessage = onDelta;
    return metered(optionalCommand<string>("ai_read_stream", { previewId, onEvent }, unavailable));
  },
  aiWalkthroughPreview: (nodeId: number) =>
    optionalCommand<AiWalkthroughPreview>("ai_walkthrough_preview", { nodeId }, unavailable),
  aiWalkthroughDisclosure: (nodeId: number, sectionIds: string[], level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_walkthrough_disclosure", { nodeId, sectionIds, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local walkthrough sections]", false)),
  aiWalkthroughFile: (nodeId: number, sectionIds: string[], level: string, model: string, previewId: string) =>
    metered(optionalCommand<string>("ai_walkthrough_file", { nodeId, sectionIds, level, model, previewId }, unavailable)),
  aiFollowUpPreview: (nodeId: number, sectionId: string, conversationId: string | null, question: string) =>
    optionalCommand<AiExplainPreview>("ai_follow_up_preview", { nodeId, sectionId, conversationId, question }, unavailable),
  aiFollowUpDisclosure: (nodeId: number, sectionId: string, conversationId: string | null, question: string, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_follow_up_disclosure", { nodeId, sectionId, conversationId, question, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local follow-up context]", false)),
  aiFollowUp: (nodeId: number, sectionId: string, conversationId: string | null, question: string, level: string, model: string, previewId: string) =>
    metered(optionalCommand<AiFollowUpResult>("ai_follow_up", { nodeId, sectionId, conversationId, question, level, model, previewId }, unavailable)),
  aiGlossaryState: () => optionalCommand<AiGlossaryState>("ai_glossary_state", undefined, unavailable),
  setAiGlossaryEnabled: (enabled: boolean) =>
    optionalCommand<AiGlossaryState>("set_ai_glossary_enabled", { enabled }, unavailable),
  aiGlossaryRecord: (terms: string[]) =>
    optionalCommand<AiGlossaryState>("ai_glossary_record", { terms }, unavailable),
  aiAnnotationsForNode: (nodeId: number) =>
    optionalCommand<CodeAnnotation[]>("ai_annotations_for_node", { nodeId }, unavailable),
  aiAnnotationAdd: (nodeId: number, snippet: string, note: string) =>
    optionalCommand<CodeAnnotation>("ai_annotation_add", { nodeId, snippet, note }, unavailable),
  aiAnnotationDelete: (nodeId: number, annotationId: number) =>
    optionalCommand<boolean>("ai_annotation_delete", { nodeId, annotationId }, unavailable),
  aiChangeSetPreview: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, filePath?: string, editIndex?: number) =>
    optionalCommand<AiExplainPreview>("ai_change_set_preview", { projectId, sessionPaths, sourceMode, filePath, editIndex }, unavailable),
  aiChangeDisclosure: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, lens: "narration" | "learning" | "review", filePath: string | null, editIndex: number | null, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_change_disclosure", { projectId, sessionPaths, sourceMode, lens, filePath, editIndex, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local change evidence]", false)),
  aiNarrateSessionChanges: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, level: string, model: string, previewId: string) =>
    metered(optionalCommand<string>("ai_narrate_session_changes", { projectId, sessionPaths, sourceMode, level, model, previewId }, unavailable)),
  aiExplainChange: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, filePath: string, editIndex: number, level: string, model: string, previewId: string) =>
    metered(optionalCommand<string>("ai_explain_change", {
      request: { projectId, sessionPaths, sourceMode, filePath, editIndex, level, model, previewId }
    }, unavailable)),
  aiReviewChangeSet: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, level: string, model: string, previewId: string) =>
    metered(optionalCommand<string>("ai_review_change_set", { projectId, sessionPaths, sourceMode, level, model, previewId }, unavailable)),
  aiRewriteDisclosure: (nodeId: number, snippet: string, instruction: string, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_rewrite_disclosure", { nodeId, snippet, instruction, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local selected passage and intent]", false)),
  aiRewriteText: (nodeId: number, snippet: string, instruction: string, level: string, model: string, previewId: string) =>
    metered(optionalCommand<AiRewriteProposal>("ai_rewrite_text", { nodeId, snippet, instruction, level, model, previewId }, unavailable)),
  applyAiSuggestion: (proposalId: string) =>
    optionalCommand<AiSuggestionApplyResult>("apply_ai_suggestion", { proposalId }, unavailable),
  aiEditSessionsForNode: (nodeId: number, limit = 20) =>
    optionalCommand<AiEditSessionSummary[]>("ai_edit_sessions_for_node", { nodeId, limit }, unavailable),
  undoAiEditSession: (nodeId: number, sessionId: string) =>
    optionalCommand<EditSnapshotRestoreResult>("undo_ai_edit_session", { nodeId, sessionId }, unavailable),
  aiSummarizeProject: (previewId: string) =>
    metered(optionalCommand<AiProjectSummary>("ai_summarize_project", { previewId }, unavailable)),
  aiSummarizeProjectPreview: (projectId: number, level: string) =>
    optionalCommand<AiExplainPreview>("ai_summarize_project_preview", { projectId, level }, unavailable),
  aiSummarizeProjectDisclosure: (projectId: number, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_summarize_project_disclosure", { projectId, level, model }, async () =>
      browserFixtureDisclosure(model, "[gated local project context]", false)),
  aiSafeManageContextCandidates: (projectId: number, analysisRunId: string, evidenceRevision: string) =>
    optionalCommand<AiSafeManageContextCandidate[]>("ai_safe_manage_context_candidates", { projectId, analysisRunId, evidenceRevision }, async () => ([{
      selectionId: "sm-context-browser-readme",
      kind: "readme",
      label: "README context",
      detail: "Bounded README excerpt, re-read and safety-gated immediately before sending.",
      maxExcerptChars: 2_400
    }])),
  aiSafeManageAdvisoryDisclosure: (projectId: number, analysisRunId: string, evidenceRevision: string, selectedContextIds: string[], model: string) =>
    optionalCommand<AiSendDisclosure>("ai_safe_manage_advisory_disclosure", { projectId, analysisRunId, evidenceRevision, selectedContextIds, model }, async () => ({
      ...browserFixtureDisclosure(model, "[exact redacted deterministic Safe Manage baseline plus explicitly selected context]", false),
      receiptId: "sm-advisory-browser-receipt"
    })),
  aiSafeManageAdvisory: (projectId: number, analysisRunId: string, evidenceRevision: string, model: string, previewId: string) =>
    metered(optionalCommand<AiSafeManageAdvisoryResult>("ai_safe_manage_advisory", { projectId, analysisRunId, evidenceRevision, model, previewId }, async () => ({
      advisory: "[recommended-action]\narchive\n\n[confidence]\nmedium\n\n[why]\nThe selected context shows substantial dormant work.\n\n[evidence]\nOnly explicitly selected, redacted local excerpts were reviewed.\n\n[unknowns]\nUnknown evidence still requires human review.\n\n[next-read-only-checks]\nInspect the local evidence before deciding.",
      deterministicRecommendation: "review",
      deterministicConfidence: "unknown",
      aiRecommendation: "archive",
      aiConfidence: "medium",
      recommendationChanged: true,
      receipt: {
        receiptId: "sm-advisory-browser-receipt",
        projectId,
        analysisRunId,
        evidenceRevision,
        status: "completed",
        requestHash: "a".repeat(64),
        requestChars: 240,
        resultHash: "b".repeat(64),
        resultChars: 220,
        sources: [{
          selectionId: "sm-context-browser-readme",
          kind: "readme",
          contentHash: "c".repeat(64),
          excerptChars: 80,
          redactionCount: 0
        }],
        failureCode: null,
        createdAt: new Date(0).toISOString(),
        completedAt: new Date(0).toISOString()
      }
    }))),
  aiSafeManageAdvisoryReceipts: (projectId: number, limit = 20) =>
    optionalCommand<AiSafeManageAdvisoryReceipt[]>("ai_safe_manage_advisory_receipts", { projectId, limit }, async () => []),
  aiKeySet: (key: string) => optionalCommand<void>("ai_key_set", { key }, async () => undefined),
  aiKeyStatus: () => optionalCommand<boolean>("ai_key_status", {}, async () => false),
  aiKeyClear: () => optionalCommand<void>("ai_key_clear", {}, async () => undefined),
  aiProviderGet: () => optionalCommand<AiProviderConfig>("ai_provider_get", {}, async () => browserFixtureProvider),
  aiProviderSet: (mode: string, baseUrl: string, model: string, format: string) =>
    optionalCommand<void>("ai_provider_set", { mode, baseUrl, model, format }, async () => undefined),
  aiProviderTestDisclosure: (mode: string, baseUrl: string, model: string, format: string) =>
    optionalCommand<AiSendDisclosure>("ai_provider_test_disclosure", { mode, baseUrl, model, format }, async () => browserFixtureDisclosure(model, "ping", false)),
  aiProviderTest: (previewId: string) =>
    metered(optionalCommand<string>("ai_provider_test", { previewId }, unavailable)),
  aiProviderModelsDisclosure: (mode: string, baseUrl: string, model: string, format: string) =>
    optionalCommand<AiSendDisclosure>("ai_provider_models_disclosure", { mode, baseUrl, model, format }, async () => browserFixtureModelsDisclosure(model)),
  aiProviderModels: (previewId: string) =>
    optionalCommand<string[]>("ai_provider_models", { previewId }, async () => [browserFixtureProvider.model]),
  aiLocalDiscover: () => optionalCommand<AiLocalProviderCandidate[]>("ai_local_discover", undefined, async () => []),
  aiUsageStatus: (projectedInputTokens?: number, projectedOutputTokens?: number) =>
    optionalCommand<AiUsageStatus>("ai_usage_status", {
      projectedInputTokens: projectedInputTokens ?? null,
      projectedOutputTokens: projectedOutputTokens ?? null
    }, async () => browserFixtureUsage(projectedInputTokens, projectedOutputTokens)),
  aiUsageSetSoftCap: (softCapTokens: number | null) =>
    optionalCommand<AiUsageStatus>("ai_usage_set_soft_cap", { softCapTokens }, async () => ({ ...browserFixtureUsage(), softCapTokens })),
  aiUsageReset: () => optionalCommand<AiUsageStatus>("ai_usage_reset", undefined, async () => ({
    ...browserFixtureUsage(),
    requestCount: 0,
    estimatedInputTokens: 0,
    estimatedOutputTokens: 0,
    estimatedTotalTokens: 0
  }))
};
