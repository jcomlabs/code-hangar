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
  AiSuggestionApplyResult,
  AiUsageStatus,
  AiWalkthroughPreview,
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationReadGrant,
  AutomationStatus,
  CodeAnnotation,
  ConnectedAppStatus,
  EditSnapshotRestoreResult,
  RecapAiSourceMode
} from "./types";

const unavailable = async (): Promise<never> => {
  throw new Error("AI Connector is not enabled in this edition.");
};

const browserFixtureProvider: AiProviderConfig = {
  mode: "local",
  baseUrl: "http://127.0.0.1:11434/v1",
  model: "fixture-local-model",
  format: "chat_completions"
};

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

function notifyAiUsageChanged(): void {
  if (typeof window !== "undefined") window.dispatchEvent(new Event(AI_USAGE_CHANGED_EVENT));
}

async function metered<T>(operation: Promise<T>): Promise<T> {
  try {
    return await operation;
  } finally {
    notifyAiUsageChanged();
  }
}

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
  connectedAppRegister: (hostId: string, projectIds: number[]) => call<ConnectedAppStatus>("mcp_appconfig_register", { hostId, projectIds }),
  connectedAppRemove: (hostId: string) => call<ConnectedAppStatus>("mcp_appconfig_remove", { hostId }),
  agentRequestsPending: () => optionalCommand<AgentActionRequest[]>("agent_requests_pending", undefined, async () => []),
  agentRequestResolve: (requestId: number, approve: boolean, inputs: ResolveInputs) =>
    call<AgentActionRequest>("agent_request_resolve", { requestId, approve, inputs }),
  aiExplainPreview: (nodeId: number) =>
    optionalCommand<AiExplainPreview>("ai_explain_preview", { nodeId }, async () => ({ blocked: [], sendChars: 414, estTokens: 104, language: "Markdown" })),
  aiExplainFile: (nodeId: number, level: string, model: string) =>
    metered(optionalCommand<string>("ai_explain_file", { nodeId, level, model }, unavailable)),
  aiExplainText: (nodeId: number, snippet: string, level: string, model: string) =>
    metered(optionalCommand<string>("ai_explain_text", { nodeId, snippet, level, model }, unavailable)),
  aiReviewFile: (nodeId: number, level: string, model: string) =>
    metered(optionalCommand<string>("ai_review_file", { nodeId, level, model }, unavailable)),
  aiReviewText: (nodeId: number, snippet: string, level: string, model: string) =>
    metered(optionalCommand<string>("ai_review_text", { nodeId, snippet, level, model }, unavailable)),
  aiSendDisclosure: (nodeId: number, snippet: string | null, lens: string, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_send_disclosure", { nodeId, snippet, lens, level, model }, async () => ({
      method: "POST",
      url: "http://127.0.0.1:11434/v1/chat/completions",
      requestBody: JSON.stringify({ model: model || browserFixtureProvider.model, stream: true, messages: [{ role: "user", content: "[gated local fixture content]" }] }, null, 2),
      fallbackRequestBody: JSON.stringify({ model: model || browserFixtureProvider.model, stream: false, messages: [{ role: "user", content: "[gated local fixture content]" }] }, null, 2),
      transport: "Streaming SSE, with one disclosed non-stream fallback",
      mode: "local",
      model: model || browserFixtureProvider.model,
      format: "chat_completions",
      sendChars: snippet?.length ?? 414,
      estTokens: Math.ceil((snippet?.length ?? 414) / 4)
    })),
  aiReadStream: async (nodeId: number, snippet: string | null, lens: string, level: string, model: string, onDelta: (delta: string) => void) => {
    if (!hasTauriRuntime()) return unavailable();
    const onEvent = new Channel<string>();
    onEvent.onmessage = onDelta;
    return metered(optionalCommand<string>("ai_read_stream", { nodeId, snippet, lens, level, model, onEvent }, unavailable));
  },
  aiWalkthroughPreview: (nodeId: number) =>
    optionalCommand<AiWalkthroughPreview>("ai_walkthrough_preview", { nodeId }, unavailable),
  aiWalkthroughFile: (nodeId: number, sectionIds: string[], level: string, model: string) =>
    metered(optionalCommand<string>("ai_walkthrough_file", { nodeId, sectionIds, level, model }, unavailable)),
  aiFollowUpPreview: (nodeId: number, sectionId: string, conversationId: string | null, question: string) =>
    optionalCommand<AiExplainPreview>("ai_follow_up_preview", { nodeId, sectionId, conversationId, question }, unavailable),
  aiFollowUp: (nodeId: number, sectionId: string, conversationId: string | null, question: string, level: string, model: string) =>
    metered(optionalCommand<AiFollowUpResult>("ai_follow_up", { nodeId, sectionId, conversationId, question, level, model }, unavailable)),
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
  aiNarrateSessionChanges: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, level: string, model: string) =>
    metered(optionalCommand<string>("ai_narrate_session_changes", { projectId, sessionPaths, sourceMode, level, model }, unavailable)),
  aiExplainChange: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, filePath: string, editIndex: number, level: string, model: string) =>
    metered(optionalCommand<string>("ai_explain_change", {
      request: { projectId, sessionPaths, sourceMode, filePath, editIndex, level, model }
    }, unavailable)),
  aiReviewChangeSet: (projectId: number, sessionPaths: string[], sourceMode: RecapAiSourceMode, level: string, model: string) =>
    metered(optionalCommand<string>("ai_review_change_set", { projectId, sessionPaths, sourceMode, level, model }, unavailable)),
  aiRewriteText: (nodeId: number, snippet: string, instruction: string, level: string, model: string) =>
    metered(optionalCommand<AiRewriteProposal>("ai_rewrite_text", { nodeId, snippet, instruction, level, model }, unavailable)),
  applyAiSuggestion: (proposalId: string) =>
    optionalCommand<AiSuggestionApplyResult>("apply_ai_suggestion", { proposalId }, unavailable),
  aiEditSessionsForNode: (nodeId: number, limit = 20) =>
    optionalCommand<AiEditSessionSummary[]>("ai_edit_sessions_for_node", { nodeId, limit }, unavailable),
  undoAiEditSession: (nodeId: number, sessionId: string) =>
    optionalCommand<EditSnapshotRestoreResult>("undo_ai_edit_session", { nodeId, sessionId }, unavailable),
  aiSummarizeProject: (projectId: number, level: string, model: string) =>
    metered(optionalCommand<AiProjectSummary>("ai_summarize_project", { projectId, level, model }, unavailable)),
  aiSummarizeProjectPreview: (projectId: number, level: string) =>
    optionalCommand<AiExplainPreview>("ai_summarize_project_preview", { projectId, level }, unavailable),
  aiSummarizeProjectDisclosure: (projectId: number, level: string, model: string) =>
    optionalCommand<AiSendDisclosure>("ai_summarize_project_disclosure", { projectId, level, model }, async () => ({
      method: "POST",
      url: "http://127.0.0.1:11434/v1/chat/completions",
      requestBody: JSON.stringify({ model: model || browserFixtureProvider.model, stream: false, messages: [{ role: "user", content: "[gated local project context]" }] }, null, 2),
      fallbackRequestBody: null,
      transport: "Complete response; no automatic retry.",
      mode: "local",
      model: model || browserFixtureProvider.model,
      format: "chat_completions",
      sendChars: 512,
      estTokens: 128
    })),
  aiKeySet: (key: string) => optionalCommand<void>("ai_key_set", { key }, async () => undefined),
  aiKeyStatus: () => optionalCommand<boolean>("ai_key_status", {}, async () => false),
  aiKeyClear: () => optionalCommand<void>("ai_key_clear", {}, async () => undefined),
  aiProviderGet: () =>
    optionalCommand<AiProviderConfig>("ai_provider_get", {}, async () => browserFixtureProvider),
  aiProviderSet: (mode: string, baseUrl: string, model: string, format: string) =>
    optionalCommand<void>("ai_provider_set", { mode, baseUrl, model, format }, async () => undefined),
  aiProviderTest: (mode: string, baseUrl: string, model: string, format: string) =>
    metered(optionalCommand<string>("ai_provider_test", { mode, baseUrl, model, format }, unavailable)),
  aiProviderModels: (mode: string, baseUrl: string, model: string, format: string) =>
    optionalCommand<string[]>("ai_provider_models", { mode, baseUrl, model, format }, async () => [browserFixtureProvider.model]),
  aiLocalDiscover: () =>
    optionalCommand<AiLocalProviderCandidate[]>("ai_local_discover", undefined, async () => []),
  aiUsageStatus: (projectedInputTokens?: number, projectedOutputTokens?: number) =>
    optionalCommand<AiUsageStatus>("ai_usage_status", {
      projectedInputTokens: projectedInputTokens ?? null,
      projectedOutputTokens: projectedOutputTokens ?? null
    }, async () => browserFixtureUsage(projectedInputTokens, projectedOutputTokens)),
  aiUsageSetSoftCap: (softCapTokens: number | null) =>
    optionalCommand<AiUsageStatus>("ai_usage_set_soft_cap", { softCapTokens }, async () => ({ ...browserFixtureUsage(), softCapTokens })),
  aiUsageReset: () => optionalCommand<AiUsageStatus>("ai_usage_reset", undefined, async () => ({ ...browserFixtureUsage(), requestCount: 0, estimatedInputTokens: 0, estimatedOutputTokens: 0, estimatedTotalTokens: 0 }))
};
