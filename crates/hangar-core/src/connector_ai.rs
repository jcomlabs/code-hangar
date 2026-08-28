//! Wire contracts compiled only into the opt-in Connector edition.
//!
//! Keeping these types in a feature-gated module prevents the Local edition from
//! carrying AI/provider vocabulary or serialisation entry points.

use serde::{Deserialize, Serialize};

use super::{AiGlossaryEntry, SafeManageConfidence, SafeManageRecommendation};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderConfig {
    pub mode: String,
    pub base_url: String,
    pub model: String,
    pub format: String,
}

/// Non-secret binding proving which exact provider origin owns the single saved credential.
/// The fingerprint is one-way and exists only to detect out-of-band key replacement; no key
/// bytes, headers or provider responses are stored in SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderCredentialBinding {
    pub origin: String,
    pub fingerprint: String,
    pub version: String,
    pub status: AiProviderCredentialBindingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderCredentialBindingStatus {
    Pending,
    Active,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiCredentialUse {
    None,
    BearerSaved,
    XApiKeySaved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiLocalProviderCandidate {
    pub label: String,
    pub base_url: String,
    pub format: String,
    pub models: Vec<String>,
}

/// Literal request bytes and destination frozen by the backend before a send.
/// The preview id is a short-lived one-shot capability and never contains a key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSendDisclosure {
    pub preview_id: String,
    /// Present only for persisted Connector advisory receipts. The receipt is
    /// evidence of what was reviewed; it is not an authorization capability.
    #[serde(default)]
    pub receipt_id: Option<String>,
    pub expires_at_unix: u64,
    pub method: String,
    pub url: String,
    pub request_body: String,
    pub fallback_request_body: Option<String>,
    pub transport: String,
    pub mode: String,
    pub model: String,
    pub format: String,
    pub credential_use: AiCredentialUse,
    pub send_chars: u64,
    pub est_tokens: u64,
}

/// Backend-curated context classes available to an explicitly requested Safe
/// Manage advisory. The Local edition never compiles this vocabulary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiSafeManageContextKind {
    Readme,
    Manifest,
    CoreFile,
    SessionExcerpt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSafeManageContextCandidate {
    /// Random, process-local selector. It cannot be decoded into a path and is
    /// accepted only while bound to the exact analysis run and project.
    pub selection_id: String,
    pub kind: AiSafeManageContextKind,
    pub label: String,
    pub detail: String,
    pub max_excerpt_chars: u64,
}

/// Non-content provenance persisted for one explicitly selected source. No
/// display name, file/session path or excerpt body is represented here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSafeManageAdvisorySourceReceipt {
    pub selection_id: String,
    pub kind: AiSafeManageContextKind,
    pub content_hash: String,
    pub excerpt_chars: u64,
    pub redaction_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiSafeManageAdvisoryReceiptStatus {
    Prepared,
    Completed,
    Failed,
}

/// Durable audit receipt for an advisory request/result. Request and result
/// bodies, credentials, provider/model configuration and source paths are
/// deliberately impossible to express in this type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSafeManageAdvisoryReceipt {
    pub receipt_id: String,
    pub project_id: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub status: AiSafeManageAdvisoryReceiptStatus,
    pub request_hash: String,
    pub request_chars: u64,
    pub result_hash: Option<String>,
    pub result_chars: Option<u64>,
    pub sources: Vec<AiSafeManageAdvisorySourceReceipt>,
    /// Stable, non-sensitive failure category only; never a provider error body.
    pub failure_code: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSafeManageAdvisoryResult {
    pub advisory: String,
    /// The local deterministic result that was current when the exact provider
    /// payload was rebuilt and sent. It is never replaced in SQLite.
    pub deterministic_recommendation: SafeManageRecommendation,
    pub deterministic_confidence: SafeManageConfidence,
    /// A strictly parsed Connector recommendation. `None` means that the
    /// provider returned useful prose but did not satisfy the typed contract;
    /// callers must never infer a fallback action from that prose.
    pub ai_recommendation: Option<SafeManageRecommendation>,
    pub ai_confidence: Option<SafeManageConfidence>,
    pub recommendation_changed: bool,
    pub receipt: AiSafeManageAdvisoryReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProjectSummary {
    pub summary: String,
    pub estimated_input_tokens: u64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiWalkthroughSection {
    pub id: String,
    pub title: String,
    pub start_line: u64,
    pub end_line: u64,
    pub snippet_hash: String,
    pub send_chars: u64,
    pub context_bytes: u64,
    pub est_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiWalkthroughPreview {
    pub blocked: Vec<String>,
    pub language: String,
    pub sections: Vec<AiWalkthroughSection>,
    pub default_section_ids: Vec<String>,
    pub send_chars: u64,
    pub est_tokens: u64,
    pub source_chars: u64,
    pub max_batch_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiFollowUpResult {
    pub conversation_id: String,
    pub section_id: String,
    pub turn: u8,
    pub remaining_turns: u8,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiGlossaryState {
    pub enabled: bool,
    pub seeds: Vec<AiGlossaryEntry>,
    pub entries: Vec<AiGlossaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiEditSessionSummary {
    pub session_id: String,
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub first_snapshot_id: i64,
    pub edit_count: u64,
    pub started_at: String,
    pub last_edit_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiRewriteProposal {
    pub proposal_id: String,
    pub session_id: String,
    pub node_id: i64,
    pub language: String,
    pub original: String,
    pub replacement: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestionApplyResult {
    pub node_id: i64,
    pub snapshot_id: i64,
    pub session_id: String,
    pub message: String,
}
