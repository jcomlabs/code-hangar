/** Wire contracts compiled only into the optional Connector frontend graph. */

import type { SafeManageConfidence, SafeManageRecommendation } from "./types";

export interface AiRewriteProposal {
  proposalId: string;
  sessionId: string;
  nodeId: number;
  language: string;
  original: string;
  replacement: string;
  summary: string;
}

export interface AiSuggestionApplyResult {
  nodeId: number;
  snapshotId: number;
  sessionId: string;
  message: string;
}

export interface AiEditSessionSummary {
  sessionId: string;
  nodeId: number;
  projectId: number;
  path: string;
  firstSnapshotId: number;
  editCount: number;
  startedAt: string;
  lastEditAt: string;
}

export interface AiExplainPreview {
  blocked: string[];
  sendChars: number;
  estTokens: number;
  language: string;
}

export interface AiWalkthroughSection {
  id: string;
  title: string;
  startLine: number;
  endLine: number;
  snippetHash: string;
  sendChars: number;
  contextBytes: number;
  estTokens: number;
}

export interface AiWalkthroughPreview {
  blocked: string[];
  language: string;
  sections: AiWalkthroughSection[];
  defaultSectionIds: string[];
  sendChars: number;
  estTokens: number;
  sourceChars: number;
  maxBatchBytes: number;
  truncated: boolean;
}

export interface AiFollowUpResult {
  conversationId: string;
  sectionId: string;
  turn: number;
  remainingTurns: number;
  answer: string;
}

export interface AiGlossaryEntry {
  term: string;
  definition: string;
  count: number;
}

export interface AiGlossaryState {
  enabled: boolean;
  seeds: AiGlossaryEntry[];
  entries: AiGlossaryEntry[];
}

export interface CodeAnnotation {
  id: number;
  nodeId: number;
  snippetHash: string;
  lineStart: number;
  lineEnd: number;
  note: string;
  anchorState: "current" | "moved" | "ambiguous" | "stale" | "unchecked";
  createdAt: string;
  updatedAt: string;
}

export type AiProviderMode = "off" | "local" | "api";
export type AiProviderFormat = "chat_completions" | "messages_api";

export interface AiProviderConfig {
  mode: AiProviderMode;
  baseUrl: string;
  model: string;
  format: AiProviderFormat;
}

export interface AiLocalProviderCandidate {
  label: string;
  baseUrl: string;
  format: AiProviderFormat;
  models: string[];
}

export interface AiSendDisclosure {
  previewId: string;
  receiptId: string | null;
  expiresAtUnix: number;
  method: string;
  url: string;
  requestBody: string;
  fallbackRequestBody: string | null;
  transport: string;
  mode: "local" | "api";
  model: string;
  format: AiProviderFormat;
  credentialUse: "none" | "bearer_saved" | "x_api_key_saved";
  sendChars: number;
  estTokens: number;
}

export type AiSafeManageContextKind = "readme" | "manifest" | "core_file" | "session_excerpt";

export interface AiSafeManageContextCandidate {
  selectionId: string;
  kind: AiSafeManageContextKind;
  label: string;
  detail: string;
  maxExcerptChars: number;
}

export interface AiSafeManageAdvisorySourceReceipt {
  selectionId: string;
  kind: AiSafeManageContextKind;
  contentHash: string;
  excerptChars: number;
  redactionCount: number;
}

export type AiSafeManageAdvisoryReceiptStatus = "prepared" | "completed" | "failed";

export interface AiSafeManageAdvisoryReceipt {
  receiptId: string;
  projectId: number;
  analysisRunId: string;
  evidenceRevision: string;
  status: AiSafeManageAdvisoryReceiptStatus;
  requestHash: string;
  requestChars: number;
  resultHash: string | null;
  resultChars: number | null;
  sources: AiSafeManageAdvisorySourceReceipt[];
  failureCode: string | null;
  createdAt: string;
  completedAt: string | null;
}

export interface AiSafeManageAdvisoryResult {
  advisory: string;
  deterministicRecommendation: SafeManageRecommendation;
  deterministicConfidence: SafeManageConfidence;
  aiRecommendation: SafeManageRecommendation | null;
  aiConfidence: SafeManageConfidence | null;
  recommendationChanged: boolean;
  receipt: AiSafeManageAdvisoryReceipt;
}

export interface AiUsageStatus {
  sessionStartedUnix: number;
  requestCount: number;
  estimatedInputTokens: number;
  estimatedOutputTokens: number;
  estimatedTotalTokens: number;
  softCapTokens: number | null;
  remainingTokens: number | null;
  overSoftCap: boolean;
  projectedTotalTokens: number;
  wouldExceedSoftCap: boolean;
  projectedOutputAllowance: number;
}

export interface AiProjectSummary {
  summary: string;
  estimatedInputTokens: number;
  model: string;
}

export interface AutomationAgentSummary {
  id: number;
  identityId: string;
  name: string;
  agentKind: "local_tool" | "connected_app";
  allowedTransport: "named_pipe" | "mcp_stdio";
  connectedHost?: "claude" | "cursor" | "codex" | null;
  scopes: string[];
  projectIds: number[];
  enabled: boolean;
  createdAt: string;
  lastSeenAt?: string | null;
}

export interface AutomationCredential {
  agent: AutomationAgentSummary;
  token: string;
  endpoint: string;
  protocol: string;
}

export interface AutomationStatus {
  enabled: boolean;
  endpoint?: string | null;
  protocol?: string | null;
  registeredAgents: number;
  message: string;
}

export interface ConnectedAppStatus {
  host: string;
  label: string;
  configPath: string;
  configExists: boolean;
  readable: boolean;
  registered: boolean;
  effectiveScopes: string[];
  effectiveProjectIds: number[];
  credentialActive: boolean;
  recoveryRequired: boolean;
  durableAgentId?: number | null;
  durableIdentityId?: string | null;
  durableCredentialEnabled: boolean;
  credentialOrphaned: boolean;
  orphanReason?: string | null;
}

export interface AgentActionRequest {
  id: number;
  agentId?: number | null;
  agentName: string;
  kind: string;
  targetCommentId?: number | null;
  proposedBody?: string | null;
  detail?: string | null;
  status: string;
  createdAt: string;
  resolvedAt?: string | null;
  currentBody?: string | null;
  currentSource?: string | null;
  targetKind?: string | null;
  targetId?: number | null;
  projectId?: number | null;
  payloadJson?: string | null;
  resultJson?: string | null;
  crossScope?: boolean;
}

export interface ResolveInputs {
  backupDir?: string | null;
  holdingRoot?: string | null;
  includeProtectedOptIn?: boolean;
  crossScopeAuthorized?: boolean;
}

export interface AutomationActivityEntry {
  id: number;
  agentId?: number | null;
  agentName?: string | null;
  method: string;
  status: string;
  detail: string;
  createdAt: string;
}

export interface AutomationReadGrant {
  id: number;
  agentId: number;
  nodeId: number;
  expiresAtMs: number;
  revoked: boolean;
}
