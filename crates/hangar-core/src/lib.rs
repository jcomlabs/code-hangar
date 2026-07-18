use serde::{Deserialize, Serialize};

/// One AI tool the Deep Scan can look for, plus whether it appears to be installed
/// on this machine (its config/home directory exists). Lets the UI show only the
/// tools actually present instead of a fixed list. WSL tools are handled
/// separately (opt-in), so this probe is Windows/host-side only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub id: String,
    pub label: String,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub state: String,
    pub message: String,
    pub elapsed_ms: u64,
    pub db_open_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemResourceProfile {
    pub logical_cpu_count: u64,
    pub total_memory_bytes: Option<u64>,
    pub available_memory_bytes: Option<u64>,
    pub gpu_acceleration: String,
    pub dedicated_vram_bytes: Option<u64>,
    pub plans: Vec<PerformanceModePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceModePlan {
    pub mode: String,
    pub label: String,
    pub cpu_threads: u64,
    pub process_priority: String,
    pub scan_batch_size: u64,
    pub memory_budget_bytes: Option<u64>,
    pub notes: Vec<String>,
}

/// Live, local-only snapshot of how much of this machine Code Hangar itself is
/// using right now. Sampled from the current process; no network or telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResourceUsage {
    pub cpu_percent: f64,
    pub logical_cpu_count: u64,
    pub memory_working_set_bytes: Option<u64>,
    pub memory_private_bytes: Option<u64>,
    pub total_memory_bytes: Option<u64>,
    pub available_memory_bytes: Option<u64>,
    pub gpu_summary: String,
    pub gpu_usage_percent: Option<f64>,
    pub sampled: bool,
}

/// The user's configured AI Assist provider (connector edition only). Persisted in the
/// encrypted settings table — NEVER the API key, which lives only in the OS keychain. `mode`
/// is `off` (default — nothing leaves the machine), `local` (a loopback model server), or
/// `api` (an external endpoint the user configures). `format` is the wire protocol:
/// `chat_completions` (the Chat Completions–compatible standard most local servers and API
/// providers speak) or `messages_api`. No provider is ever hardcoded as a default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderConfig {
    pub mode: String,
    pub base_url: String,
    pub model: String,
    pub format: String,
}

/// One reachable OpenAI-compatible server found by an explicit loopback-only discovery scan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiLocalProviderCandidate {
    pub label: String,
    pub base_url: String,
    pub format: String,
    pub models: Vec<String>,
}

/// Literal provider request disclosure assembled from freshly gated content. The compact JSON is
/// exactly the body sent by `hangar-ai`; authentication headers are never included.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSendDisclosure {
    pub method: String,
    pub url: String,
    pub request_body: String,
    pub fallback_request_body: Option<String>,
    pub transport: String,
    pub mode: String,
    pub model: String,
    pub format: String,
    pub send_chars: u64,
    pub est_tokens: u64,
}

/// An optional AI-enriched project summary (connector edition). Built from the SAME local context
/// the no-network summary uses, then sent to the user's configured provider through the secret
/// send-gate. Off unless the user has configured a provider and asked for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProjectSummary {
    pub summary: String,
    /// Rough token estimate of the context that was sent (chars / 4), for a cost hint.
    pub estimated_input_tokens: u64,
    /// The model that produced the summary (from the configured provider).
    pub model: String,
}

/// One deterministic, language-aware section of a file offered by the guided
/// walkthrough. The id and snippet hash are derived from the current gated
/// bytes; a stale webview cannot use either to select different content.
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

/// Local-only walkthrough composition preview. `blocked` is populated by the
/// same path/content gate used immediately before a provider send.
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

/// One bounded, section-scoped follow-up answer. Conversation state is held in
/// memory only and is never added to the durable catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiFollowUpResult {
    pub conversation_id: String,
    pub section_id: String,
    pub turn: u8,
    pub remaining_turns: u8,
    pub answer: String,
}

/// A canonical learning term. The encrypted catalog stores exactly these three
/// fields (plus database timestamps), never source text or file paths.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiGlossaryEntry {
    pub term: String,
    pub definition: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiGlossaryState {
    pub enabled: bool,
    pub seeds: Vec<AiGlossaryEntry>,
    pub entries: Vec<AiGlossaryEntry>,
}

/// A user's local note attached to an exact source selection. `anchor_state` is
/// recomputed from fresh authorized bytes when listed: current, moved,
/// ambiguous, stale, or unchecked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeAnnotation {
    pub id: i64,
    pub node_id: i64,
    pub snippet_hash: String,
    pub line_start: u64,
    pub line_end: u64,
    pub note: String,
    pub anchor_state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub source: String,
    pub context_count: i64,
    pub pinned: bool,
    pub protected_level: Option<String>,
    pub scan_state: String,
    pub scan_root_id: Option<i64>,
    /// The name the user gave this project in the Antigravity (Gemini) IDE, when it
    /// differs from the folder basename. Surfaced as a "named: …" label. Enriched by
    /// the API layer from the Antigravity registry; the DB layer leaves it `None`.
    #[serde(default)]
    pub antigravity_name: Option<String>,
    /// Whether this project is "current" in the app that owns it — i.e. its root
    /// appears in a recent app-activity signal (e.g. the Antigravity summaries proto,
    /// or a Claude `lastSessionModified`). Lets the frontend keep a project Active
    /// even when no parsed session links to it (the ExampleProj case: its conversations are
    /// `.pb`, never the `.db` the app parses). Serialized as `isCurrent`. The DB layer
    /// sets `false`; the API layer flips it `true` from the per-app signals.
    #[serde(default)]
    pub is_current: bool,
    /// The source app that owns this project (e.g. `"codex"`, `"claude"`, `"cursor"`,
    /// `"antigravity"`, `"hermes"`), for showing an app badge. Derived by the API
    /// layer from the registry that owns the path (the most specific deliberate one
    /// when several match). The DB layer leaves it `None`.
    #[serde(default)]
    pub app: Option<String>,
    /// Every app whose registry/sessions claim this project root (the primary `app`
    /// plus any others it is also used in). Lets the app FILTER find a project under
    /// each of its apps even though the badge shows only the primary. The DB layer
    /// leaves it empty; the API layer fills it. Serialized as `apps`.
    #[serde(default)]
    pub apps: Vec<String>,
}

/// A local, no-network "what this project is" card built from a project's README,
/// top-level markdown, and manifest files. Heuristic and read-only; the connector
/// edition can later enrich it with an AI summary using the same on-disk context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextSummary {
    /// Detected stacks/types, e.g. `["Node.js", "Rust"]`. Empty when nothing recognized.
    pub kinds: Vec<String>,
    /// The README's first heading (its title), if a README was found.
    pub readme_title: Option<String>,
    /// A short excerpt — the README's first meaningful paragraph, length-bounded.
    pub readme_excerpt: Option<String>,
    /// Suggested run commands inferred from the manifests, e.g. `["npm run dev"]`.
    pub run_commands: Vec<String>,
    /// The manifest files detected at the project root (package.json, Cargo.toml, …).
    pub manifest_files: Vec<String>,
    /// Top-level markdown file names found (README first).
    pub markdown_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub source: String,
    pub context_count: i64,
    pub protected_level: Option<String>,
    pub scan_state: String,
    pub scan_root_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NavItem {
    pub id: i64,
    pub project_id: i64,
    pub node_id: Option<i64>,
    pub parent_nav_id: Option<i64>,
    pub path: String,
    pub display_path: String,
    pub display_name: String,
    pub item_kind: String,
    pub priority: i64,
    pub is_context: bool,
    pub is_markdown: bool,
    pub is_sensitive: bool,
    pub protected_level: Option<String>,
    pub child_count: i64,
    pub fully_scanned: bool,
    pub collapse_default: bool,
    pub scan_error: Option<String>,
    pub aggregate_apparent_bytes: Option<u64>,
    pub aggregate_allocated_bytes: Option<u64>,
    pub aggregate_physical_bytes: Option<u64>,
    pub aggregate_bytes_partial: bool,
    /// Last-modified time of the underlying node as a Unix-epoch-seconds string
    /// (read from the already-scanned `node` table; never a fresh disk read).
    /// `None` for rows without a node or a stored mtime. Powers the file-tree
    /// "sort by date" control.
    pub modified_at: Option<String>,
    pub children: Vec<NavItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NavChildrenPage {
    pub items: Vec<NavItem>,
    pub total: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderExplanation {
    pub nav_id: i64,
    pub project_id: i64,
    pub display_path: String,
    pub display_name: String,
    pub item_kind: String,
    pub classification: String,
    pub confidence: String,
    pub summary: String,
    pub signals: Vec<String>,
    pub caveats: Vec<String>,
    pub child_count: i64,
    pub apparent_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
    pub protected_level: Option<String>,
    pub fully_scanned: bool,
    pub scan_error: Option<String>,
}

/// Handle returned when an investigation starts: the ad-hoc root + the scan job to poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationHandle {
    pub root_id: i64,
    pub job_id: String,
    pub path: String,
}

/// A registered project related to an investigated folder by path (the reverse lookup).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationOwner {
    /// "inside-project" (the folder is inside this project), "contains-project" (this
    /// registered project lives inside the folder), or "same-path".
    pub relation: String,
    pub name: String,
    pub path: String,
}

/// Report for an investigated folder: what it is, who (if anyone) owns it, and its size.
/// Carries `root_node_id` so the same Gate-3 backup/move/delete pipeline can act on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderInvestigation {
    pub root_id: i64,
    pub root_node_id: Option<i64>,
    pub path: String,
    pub explanation: Option<FolderExplanation>,
    pub owners: Vec<InvestigationOwner>,
    pub is_orphan: bool,
    pub file_count: u64,
    pub total_bytes: u64,
    pub has_git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    pub nav_id: i64,
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub display_name: String,
    pub priority: i64,
    pub context_rank: i64,
    pub context_group: String,
    pub recommendation_reason: String,
    pub recommended: bool,
    pub is_sensitive: bool,
    pub protected_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub display_path: String,
    pub display_name: String,
    pub mode: PreviewMode,
    pub state: PreviewState,
    pub file_kind: FileKind,
    pub size_bytes: Option<u64>,
    pub truncated: bool,
    pub preview_limit_bytes: u64,
    pub system_error_code: Option<i64>,
    pub was_revealed: bool,
    pub source: Option<String>,
    pub rendered_html: Option<String>,
    pub blocked_reason: Option<String>,
    pub headings: Vec<String>,
    pub links: Vec<MarkdownLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownLink {
    pub label: String,
    pub target: String,
    pub is_remote: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRelationship {
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub display_name: String,
    pub item_kind: String,
    pub kind: String,
    pub confidence: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipIssue {
    pub node_id: i64,
    pub project_id: i64,
    pub kind: String,
    pub confidence: String,
    pub target: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRelationships {
    pub node_id: i64,
    pub outgoing: Vec<NodeRelationship>,
    pub incoming: Vec<NodeRelationship>,
    pub issues: Vec<RelationshipIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub display_name: String,
    pub item_kind: String,
    pub graph_kind: String,
    pub confidence: String,
    pub details: Vec<String>,
    pub physical_bytes: Option<u64>,
    pub protected_or_sensitive: bool,
    pub shared_project_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source_node_id: i64,
    pub target_node_id: i64,
    pub kind: String,
    pub confidence: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphIssue {
    pub node_id: i64,
    pub project_id: Option<i64>,
    #[serde(default)]
    pub source_path: Option<String>,
    pub kind: String,
    pub confidence: String,
    pub target: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphMap {
    pub project_id: i64,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub issues: Vec<GraphIssue>,
    pub total_nodes: i64,
    pub total_edges: i64,
    pub total_issues: i64,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanCandidate {
    pub node_id: i64,
    pub project_id: i64,
    pub project_name: String,
    pub path: String,
    pub display_name: String,
    pub confidence: String,
    pub reason: String,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanCandidates {
    pub candidates: Vec<OrphanCandidate>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanStatus {
    pub node_id: i64,
    pub evaluated: bool,
    pub is_candidate: bool,
    pub candidate_kind: Option<String>,
    pub confidence: Option<String>,
    pub reason: Option<String>,
    pub incoming_references: i64,
    pub protected_or_sensitive: bool,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LostProjectCandidate {
    pub project_id: i64,
    pub node_id: Option<i64>,
    pub nav_id: Option<i64>,
    pub candidate_kind: String,
    pub display_name: String,
    pub path: String,
    pub confidence: String,
    pub reason: String,
    pub signals: Vec<String>,
    pub apparent_bytes: u64,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LostProjectCandidates {
    pub candidates: Vec<LostProjectCandidate>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMember {
    pub node_id: i64,
    pub project_id: i64,
    pub project_name: String,
    pub path: String,
    pub display_name: String,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub id: i64,
    pub size_bytes: u64,
    pub hash_partial: String,
    pub confidence: String,
    pub reason: String,
    pub member_count: u64,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
    pub members: Vec<DuplicateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCandidates {
    pub groups: Vec<DuplicateGroup>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedDuplicateGroup {
    pub full_hash: String,
    pub size_bytes: u64,
    pub member_count: usize,
    pub reclaimable_bytes: u64,
    pub confidence: String,
    pub members: Vec<DuplicateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateConfirmation {
    pub target_node_id: i64,
    #[serde(default)]
    pub confirmed_groups: Vec<ConfirmedDuplicateGroup>,
    #[serde(default)]
    pub checked_files: usize,
    #[serde(default)]
    pub bytes_hashed: u64,
    #[serde(default)]
    pub reclaimable_bytes: u64,
    #[serde(default)]
    pub partial: bool,
}

/// Live progress of an on-demand full-hash duplicate confirmation. `total_*` are the denominators
/// known up front (the candidate group); `*_hashed`/`checked_files` climb as files are verified.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateConfirmProgress {
    pub checked_files: u64,
    pub total_files: u64,
    pub bytes_hashed: u64,
    pub total_bytes: u64,
}

/// Status of an on-demand duplicate-confirmation job. Full-hash verification streams every byte of
/// each candidate, so it runs on a background thread the user can watch (progress) and cancel.
/// Mirrors the plan-preview job lifecycle: running → completed | cancelled | failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateConfirmStatus {
    pub job_id: String,
    /// `running` | `cancelling` | `completed` | `cancelled` | `failed`.
    pub state: String,
    pub target_node_id: i64,
    pub message: String,
    pub error: Option<String>,
    pub progress: DuplicateConfirmProgress,
    pub result: Option<DuplicateConfirmation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySignal {
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySourceHit {
    pub source_kind: String,
    pub source_label: String,
    pub path: String,
    pub exists: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDiscoveryCandidate {
    pub path: String,
    pub display_name: String,
    pub project_kind: String,
    pub confidence: String,
    pub score: u64,
    pub source_kinds: Vec<String>,
    pub signals: Vec<DiscoverySignal>,
    pub already_registered: bool,
    pub existing_project_id: Option<i64>,
    pub overlap_kind: String,
    pub nested_under_registered: Option<String>,
    pub contains_registered_roots: Vec<String>,
    pub estimated_files: Option<u64>,
    pub estimated_bytes: Option<u64>,
    pub estimate_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiscoveryCandidate {
    pub path: String,
    pub display_name: String,
    pub source_kind: String,
    pub source_label: String,
    pub session_kind: String,
    pub confidence: String,
    pub linked_project_paths: Vec<String>,
    pub linked_registered_project_ids: Vec<i64>,
    pub association: String,
    /// Source-file last-modified time in epoch milliseconds, when stat succeeds.
    /// Used by the UI to sort sessions (and their projects) by recency.
    #[serde(default)]
    pub modified_ms: Option<i64>,
}

/// Bounded, read-only, secret-redacted preview of a local session/transcript
/// file so the user can read what a conversation referenced without leaving the
/// app. Transient: never written to SQLite, FTS or logs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionPreview {
    pub path: String,
    pub display_name: String,
    pub session_kind: String,
    pub size_bytes: u64,
    /// Maximum source/transcript bytes requested for this response. The initial
    /// preview is bounded; the UI can explicitly request a larger cumulative
    /// window or the complete session.
    #[serde(default)]
    pub preview_limit_bytes: u64,
    pub truncated: bool,
    /// The raw Source view may remain a bounded disk window even after the full
    /// readable conversation has been streamed and assembled.
    #[serde(default)]
    pub source_truncated: bool,
    pub text: String,
    /// Optional conversation-only JSONL window used by the readable view. The
    /// raw bounded window stays in `text` so Source remains an honest disk view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_text: Option<String>,
    pub redacted_count: u32,
    pub revealed: bool,
    /// File created / last-modified time in epoch milliseconds, when the platform
    /// and filesystem report it. Shown in the File details panel.
    #[serde(default)]
    pub created_ms: Option<i64>,
    #[serde(default)]
    pub modified_ms: Option<i64>,
}

/// A deterministic, read-only reconstruction of file changes recorded in one
/// local AI session. This is evidence from the transcript, not a claim about the
/// current working tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangeSet {
    pub path: String,
    pub source_kind: String,
    pub coverage: SessionChangeCoverage,
    pub files: Vec<SessionFileChange>,
    pub edit_count: u64,
    pub added_lines: u64,
    pub removed_lines: u64,
    pub redacted_count: u32,
    pub parsed_records: u64,
    pub omitted_records: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangeCoverage {
    /// `full`, `direct_edits`, or `none`. Kept as a string so new transcript
    /// sources can add honest intermediate levels without breaking old clients.
    pub level: String,
    pub label: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileChange {
    pub path: String,
    pub edits: Vec<SessionChangeEdit>,
    pub added_lines: u64,
    pub removed_lines: u64,
    /// Best-effort comparison against the current inventoried file. `None`
    /// means no project-root-authorized comparison was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<SessionFileReality>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileReality {
    /// `applied`, `reverted`, `drifted`, `file_missing`, or `unverified`.
    pub status: String,
    pub label: String,
    pub note: String,
    /// Epoch milliseconds when this compare-only label was evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_ms: Option<i64>,
}

/// The user's durable "reviewed through here" marker for one project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReviewCheckpoint {
    pub project_id: i64,
    pub reviewed_at: String,
    pub session_cutoff_ms: i64,
    /// Fingerprint of the bounded local Git evidence at checkpoint time. This
    /// is comparison metadata, not a Git object id and never triggers network.
    pub git_fingerprint: Option<String>,
    /// Local Git HEAD observed when the user marked the project reviewed. It
    /// is used only as a validated local diff baseline; no remote is contacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
}

/// A normalized, secret-redacted session reconstruction retained in the
/// encrypted catalog so a review does not disappear when a tool rotates logs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewLedgerEntry {
    pub id: i64,
    pub project_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<i64>,
    pub source_kind: String,
    pub source_ref: String,
    pub source_modified_ms: Option<i64>,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_entry_hash: Option<String>,
    #[serde(default)]
    pub entry_hash: String,
    #[serde(default)]
    pub encoded_bytes: u64,
    pub change_set: SessionChangeSet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangeEdit {
    pub source: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Comparison of this recorded edit against the current authorized file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<SessionFileReality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub hunks: Vec<SessionDiffHunk>,
    pub added_lines: u64,
    pub removed_lines: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiffHunk {
    pub header: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_start: Option<u64>,
    pub lines: Vec<SessionDiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiffLine {
    /// `context`, `added`, `removed`, or `note`.
    pub kind: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDiscoveryReport {
    pub candidates: Vec<ProjectDiscoveryCandidate>,
    pub sessions: Vec<SessionDiscoveryCandidate>,
    pub searched_locations: Vec<DiscoverySourceHit>,
    pub duration_ms: u64,
    pub total_candidates: u64,
    pub total_sessions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewMode {
    Rendered,
    Source,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewState {
    Ready,
    Blocked,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    Text,
    Markdown,
    Binary,
    Directory,
    Symlink,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuickOpenResult {
    pub node_id: i64,
    pub project_id: i64,
    pub label: String,
    pub path: String,
    pub item_kind: String,
    pub score: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHit {
    pub node_id: i64,
    pub project_id: i64,
    pub title: String,
    pub path: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSearchResult {
    pub hits: Vec<DocumentHit>,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPolicy {
    pub allow_sensitive_reveal: bool,
    pub relax_non_strong_protected_preview: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecentItem {
    pub node_id: i64,
    pub project_id: Option<i64>,
    pub item_kind: String,
    pub path: String,
    pub opened_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PinnedItem {
    pub node_id: i64,
    pub project_id: Option<i64>,
    pub item_kind: String,
    pub path: String,
    pub pinned_at: String,
}

/// A user (or, in a later phase, a connected AI app) comment attached to a
/// project, folder or file node. `source` is the channel/identity class ("user"
/// vs an agent client) and `author` the display name; both default to "user".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: i64,
    pub node_id: i64,
    pub project_id: Option<i64>,
    pub body: String,
    pub author: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A connected AI app's pending REQUEST for an action it cannot perform itself.
/// Under "total control" the agent may only insert one of these; a human reviews
/// it and, on approval, the app performs it AS the user. `current_*` fields are
/// enrichment for the reviewer (the target comment's present state), not stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionRequest {
    pub id: i64,
    pub agent_id: Option<i64>,
    pub agent_name: String,
    /// "comment_edit" | "comment_delete" | "read_body" | "backup_protected" |
    /// "move_to_holding" | "final_remove".
    pub kind: String,
    pub target_comment_id: Option<i64>,
    pub proposed_body: Option<String>,
    pub detail: Option<String>,
    /// "pending" | "approved" | "rejected".
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub current_body: Option<String>,
    pub current_source: Option<String>,
    /// Generic target descriptor for non-comment kinds ("comment", "node", or a
    /// holding-area entry); None for the original comment kinds.
    #[serde(default)]
    pub target_kind: Option<String>,
    /// Generic target id (node id / quarantine-entry id); comment kinds use
    /// `target_comment_id` instead.
    #[serde(default)]
    pub target_id: Option<i64>,
    /// The target's resolved owning project (None for project-less targets).
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Serialized kind-specific request payload (e.g. the app-built OperationPlan).
    #[serde(default)]
    pub payload_json: Option<String>,
    /// Outcome written at resolve (e.g. created backup/entry id, or an error).
    #[serde(default)]
    pub result_json: Option<String>,
    /// True when the target is outside the agent's grants — allowed, but the
    /// approval gate adds an extra cross-project authorization step.
    #[serde(default)]
    pub cross_scope: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanRoot {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherStatus {
    pub generated_at_ms: u64,
    pub poll_interval_ms: u64,
    pub debounce_ms: u64,
    pub stale_projects: u64,
    pub changed_projects: u64,
    pub projects: Vec<WatcherProjectStatus>,
    pub focused: Option<FocusedWatcherStatus>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherProjectStatus {
    pub project_id: Option<i64>,
    pub scan_root_id: i64,
    pub name: String,
    pub path: String,
    pub state: String,
    pub reason: String,
    pub last_scanned_at: Option<String>,
    pub root_modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FocusedWatcherStatus {
    pub project_id: i64,
    pub state: String,
    pub changed_context_files: u64,
    pub current_node: Option<WatcherNodeStatus>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherNodeStatus {
    pub node_id: i64,
    pub path: String,
    pub display_name: String,
    pub state: String,
    pub is_markdown: bool,
    pub is_context: bool,
    pub stored_mtime: Option<String>,
    pub live_mtime: Option<String>,
    pub stored_size: Option<u64>,
    pub live_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub job_id: String,
    pub state: String,
    pub scan_phase: String,
    pub scanned_files: u64,
    pub indexed_documents: u64,
    pub started_at_ms: u64,
    pub phase_started_at_ms: u64,
    pub last_progress_at_ms: u64,
    pub updated_at_ms: u64,
    pub estimated_total_files: Option<u64>,
    pub estimated_total_bytes: Option<u64>,
    pub worker_count: Option<u64>,
    pub estimate_ms: Option<u64>,
    pub scan_ms: Option<u64>,
    pub body_read_ms: Option<u64>,
    pub persist_ms: Option<u64>,
    pub finalize_ms: Option<u64>,
    pub accounting_select_ms: Option<u64>,
    pub accounting_compute_ms: Option<u64>,
    pub accounting_update_ms: Option<u64>,
    pub partial: bool,
    pub root_ids: Vec<i64>,
    pub root_paths: Vec<String>,
    pub current_path: Option<String>,
    pub error: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedZone {
    pub id: i64,
    pub pattern_type: String,
    pub pattern: String,
    pub level: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    pub size_apparent: Option<u64>,
    pub size_allocated: Option<u64>,
    pub modified_at: Option<String>,
    pub readonly: bool,
    pub is_symlink: bool,
    pub is_reparse: bool,
    pub reparse_kind: Option<String>,
    pub volume_id: Option<String>,
    pub inode_key: Option<String>,
    pub link_count: Option<u64>,
    pub inaccessible: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoSummary {
    pub project_id: i64,
    pub has_git: bool,
    pub current_branch: Option<String>,
    pub head_ref: Option<String>,
    pub origin_url: Option<String>,
    pub metadata_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFootprintSummary {
    pub project_id: i64,
    pub name: String,
    pub path: String,
    pub apparent_bytes: u64,
    pub allocated_bytes: Option<u64>,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub total_projects: u64,
    pub total_items: u64,
    pub context_files: u64,
    pub indexed_documents: u64,
    pub non_indexed_items: u64,
    pub partial_items: u64,
    pub git_projects: u64,
    pub sensitive_files: u64,
    pub protected_files: u64,
    pub scan_roots: u64,
    pub largest_projects: Vec<ProjectFootprintSummary>,
    pub stale_or_dirty: String,
    pub adapters_needing_review: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterSummary {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub adapter_type: String,
    pub source: String,
    pub enabled: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecurityStatus {
    pub outbound_network: String,
    pub mutation_executor: String,
    pub agent_ipc: String,
    pub active_features: Vec<String>,
    pub notes: Vec<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationAgentSummary {
    pub id: i64,
    pub name: String,
    pub scopes: Vec<String>,
    pub project_ids: Vec<i64>,
    pub enabled: bool,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCredential {
    pub agent: AutomationAgentSummary,
    pub token: String,
    pub endpoint: String,
    pub protocol: String,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationStatus {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub protocol: Option<String>,
    pub registered_agents: u64,
    pub message: String,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationActivityEntry {
    pub id: i64,
    pub agent_id: Option<i64>,
    pub agent_name: Option<String>,
    pub method: String,
    pub status: String,
    pub detail: String,
    pub created_at: String,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationReadGrant {
    pub id: i64,
    pub agent_id: i64,
    pub node_id: i64,
    pub expires_at_ms: i64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryOperation {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub target_node_id: Option<i64>,
    pub target_fingerprint: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub error: Option<String>,
    pub total_items: u64,
    pub done_items: u64,
    pub pending_items: u64,
    pub failed_items: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPending {
    pub enabled: bool,
    pub pending: bool,
    pub operations: Vec<RecoveryOperation>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResolveResult {
    pub action: String,
    pub recovered_operations: u64,
    pub rolled_back_items: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationTokenResult {
    pub action: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationValidationIssue {
    pub node_id: Option<i64>,
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationBackupSummary {
    pub backup_id: i64,
    pub manifest_path: String,
    pub total_bytes: u64,
    pub verified: bool,
    pub item_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationMoveEntry {
    pub original_path: String,
    pub stored_path: Option<String>,
    pub outcome: String,
    pub bytes: u64,
    pub space_recovered: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationMoveSummary {
    pub operation_id: i64,
    pub entries: Vec<MutationMoveEntry>,
    pub space_recovered: u64,
    pub moved: u64,
    pub skipped: u64,
    pub failed: u64,
    /// Empty source directories removed after the move (recursive folder cleanup).
    pub removed_dirs: u64,
    /// Reparse points (junction/symlink) removed as links during an opt-in empty.
    pub removed_links: u64,
}

/// Read-only preview of what an opt-in "empty the folder completely" would include: the
/// sensitive/protected files (copied to the backup then removed) and the reparse links
/// (removed without following). Surfaced in the per-project confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MutationProtectedPreview {
    pub protected: Vec<String>,
    pub reparse: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationRestoreSummary {
    pub entry_id: i64,
    pub outcome: String,
    pub original_path: String,
    pub restored_path: Option<String>,
    pub conflict_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationFinalRemoveSummary {
    pub entry_id: i64,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationLockInspection {
    pub path: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationActivityOperation {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub target_node_id: Option<i64>,
    pub target_fingerprint: Option<String>,
    pub recovered_bytes: Option<u64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationActivityItem {
    pub id: i64,
    pub operation_id: i64,
    pub node_id: Option<i64>,
    pub action: String,
    pub from_path: Option<String>,
    pub to_path: Option<String>,
    pub bytes: Option<u64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationActivityBackup {
    pub id: i64,
    pub level: String,
    pub destination: String,
    pub manifest_path: String,
    pub total_bytes: Option<u64>,
    pub verified: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationStoredEntry {
    pub id: i64,
    pub operation_id: Option<i64>,
    pub original_path: String,
    pub stored_path: String,
    pub size: Option<u64>,
    pub file_count: Option<u64>,
    pub risk_level: Option<String>,
    pub backup_id: Option<i64>,
    pub space_recovered: u64,
    pub scheduled_delete_at: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationActivityLog {
    pub enabled: bool,
    pub operations: Vec<MutationActivityOperation>,
    pub items: Vec<MutationActivityItem>,
    pub backups: Vec<MutationActivityBackup>,
    pub stored_entries: Vec<MutationStoredEntry>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditSnapshotSummary {
    pub id: i64,
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub origin: String,
    pub session_id: Option<String>,
    pub created_at: String,
    pub status: String,
    pub bytes: u64,
    pub blake3_before: String,
    pub blake3_after: Option<String>,
    pub restored_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditSnapshotRestoreResult {
    pub restored_snapshot_id: i64,
    pub safety_snapshot_id: i64,
    pub node_id: i64,
    pub message: String,
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

/// A bounded, read-only review of one proposed in-app file change. The draft is
/// not persisted; applying still re-reads the file and enforces whole-file CAS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEditPreview {
    pub node_id: i64,
    pub project_id: i64,
    pub before_hash: String,
    pub after_hash: String,
    pub added_lines: u64,
    pub removed_lines: u64,
    pub hunks: Vec<SessionDiffHunk>,
    pub diff_truncated: bool,
    pub validation: EditValidationSummary,
    pub git_context: EditGitContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditValidationSummary {
    /// `passed` or `warning`. A blocking result is returned as an error instead
    /// of a preview that could accidentally be applied.
    pub status: String,
    pub label: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditGitContext {
    /// `clean`, `modified`, `staged`, `staged_and_modified`, `untracked`,
    /// `not_repository`, or `unavailable`.
    pub state: String,
    pub label: String,
    pub note: String,
    pub other_changed_files: u64,
}

/// What restoring one verified snapshot would change relative to the current
/// authorized file. This command never restores or writes anything.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditSnapshotComparison {
    pub snapshot_id: i64,
    pub node_id: i64,
    pub added_lines: u64,
    pub removed_lines: u64,
    pub hunks: Vec<SessionDiffHunk>,
    pub diff_truncated: bool,
    pub already_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditableValueSet {
    pub node_id: i64,
    pub path: String,
    pub format: String,
    pub source_hash: String,
    pub values: Vec<EditableValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditableValue {
    pub id: String,
    pub path: String,
    pub label: String,
    pub kind: String,
    pub display_value: String,
    pub raw_value: String,
    pub start_byte: u64,
    pub end_byte: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValueEditRequest {
    pub value_id: String,
    pub expected_source_hash: String,
    pub expected_raw_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValueEditResult {
    pub node_id: i64,
    pub snapshot_id: i64,
    pub source_hash: String,
    pub value: EditableValue,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionCheckItem {
    pub id: String,
    pub label: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionStaticCheckReport {
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub status: String,
    pub checks: Vec<CorrectionCheckItem>,
    pub checked_at: String,
    pub executed_project_code: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCheckDefinition {
    pub id: String,
    pub label: String,
    pub command_label: String,
    pub manifest_path: String,
    pub fingerprint: String,
    pub approved: bool,
    pub approved_at: Option<String>,
    pub timeout_seconds: u64,
    pub memory_limit_mib: u64,
    pub process_limit: u32,
    pub risk_disclosure: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlledCheckRun {
    pub project_id: i64,
    pub node_id: i64,
    pub check_id: String,
    pub label: String,
    pub command_label: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub rollback_snapshot_id: Option<i64>,
    pub rollback_available: bool,
    pub checked_at: String,
    pub limits_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Cleanup-risk tier for a previewed operation item — how much review a future
/// backup / quarantine-move / delete of it would need. Green = safe
/// (temp/caches), Yellow = rebuildable, Orange = project-specific, Red =
/// shared/dangerous, Black = never touch (Protected Zones, secrets, system).
pub enum RiskTier {
    Green,
    Yellow,
    Orange,
    Red,
    Black,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableBytes {
    pub owned: u64,
    pub orphaned_on_removal: u64,
    pub total: u64,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableSummary {
    pub target_node_id: i64,
    pub project_id: i64,
    pub target_path: String,
    pub target_kind: String,
    pub recoverable_bytes: RecoverableBytes,
    pub shared_count: u64,
    pub protected_count: u64,
    pub sensitive_count: u64,
    pub partial_footprint: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationPlanTarget {
    pub node_id: i64,
    pub project_id: i64,
    pub kind: String,
    pub path: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// One previewed entry of an Operation Plan. For large targets a single compact
/// `recursive_dir` item stands in for the whole subtree. Describes a potential
/// future action only — nothing is executed.
pub struct OperationPlanItem {
    pub node_id: Option<i64>,
    pub path: String,
    pub display_name: String,
    pub item_kind: String,
    pub action_label: String,
    pub risk: RiskTier,
    pub confidence: String,
    pub size_apparent: u64,
    pub physical_bytes: Option<u64>,
    pub hardlink_group: Option<String>,
    pub frees_space: bool,
    pub recursive_dir: bool,
    pub child_count: u64,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SharedAssetRef {
    pub project_id: i64,
    pub project_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SharedAsset {
    pub node_id: i64,
    pub path: String,
    pub display_name: String,
    pub physical_bytes: Option<u64>,
    pub referenced_by: Vec<SharedAssetRef>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DanglingAfter {
    pub referrer_node_id: i64,
    pub path: String,
    pub missing_path: String,
    pub confidence: String,
    /// The project the referrer lives in (None when it cannot be resolved).
    pub project_id: Option<i64>,
    /// Human-readable name of that project, for badging in the Risk Report.
    pub project_name: Option<String>,
    /// What kind of dependency would break: "workflow" for a workflow→model
    /// reference, or the relationship-issue kind for a local broken reference.
    pub dependency_kind: String,
    /// True when the referrer is in a DIFFERENT project than the one being
    /// removed — i.e. deleting this would break something the user may not be
    /// looking at. Cross-project dangling is the higher-risk case.
    pub cross_project: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SensitiveFileRef {
    pub node_id: Option<i64>,
    pub path: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedHit {
    pub node_id: Option<i64>,
    pub path: String,
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitWarning {
    pub project_id: i64,
    pub message: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfidenceSummary {
    pub high: u64,
    pub medium: u64,
    pub low: u64,
    pub unknown: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// A **preview-only** plan of the future backup / quarantine-move / delete
/// actions for a target project or file. `read_only_preview` is always true and
/// no plan is persisted; it exists so the user can see impact and risk first.
pub struct OperationPlan {
    pub plan_id: String,
    pub schema: String,
    pub created_at: String,
    pub target: OperationPlanTarget,
    pub action_label: String,
    pub items: Vec<OperationPlanItem>,
    pub recoverable_bytes: RecoverableBytes,
    pub shared_assets: Vec<SharedAsset>,
    pub dangling_after: Vec<DanglingAfter>,
    pub sensitive_files: Vec<SensitiveFileRef>,
    pub protected_hits: Vec<ProtectedHit>,
    pub git_warnings: Vec<GitWarning>,
    pub confidence_summary: ConfidenceSummary,
    pub recommended_action: String,
    pub read_only_preview: bool,
    pub plan_stale: bool,
    pub partial_footprint: bool,
    /// True when the dangling-impact scan hit its per-query row cap, so more
    /// dependents than listed may exist.
    pub dangling_truncated: bool,
    pub external_services_unaffected: bool,
    pub target_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RiskTierCount {
    pub tier: RiskTier,
    pub count: u64,
    pub physical_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Read-only risk projection of an [`OperationPlan`]: tier roll-ups, recoverable
/// bytes with caveats, and shared/dangling/sensitive/protected/git sections.
/// Describes potential future operations; runs none.
pub struct RiskReport {
    pub schema: String,
    pub generated_at: String,
    pub target: OperationPlanTarget,
    pub action_label: String,
    pub read_only_preview: bool,
    pub external_services_unaffected: bool,
    pub recoverable_bytes: RecoverableBytes,
    pub risk_counts: Vec<RiskTierCount>,
    pub shared_assets: Vec<SharedAsset>,
    pub dangling_after: Vec<DanglingAfter>,
    /// True when the dangling-impact scan hit its per-query row cap.
    pub dangling_truncated: bool,
    pub sensitive_files: Vec<SensitiveFileRef>,
    pub protected_hits: Vec<ProtectedHit>,
    pub git_warnings: Vec<GitWarning>,
    pub confidence_summary: ConfidenceSummary,
    pub recommended_action: String,
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreviewStatus {
    pub job_id: String,
    pub state: String,
    pub target_node_id: i64,
    pub action_label: String,
    pub message: String,
    pub error: Option<String>,
    pub plan: Option<OperationPlan>,
    pub report: Option<RiskReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScannedFile {
    pub absolute_path: String,
    pub relative_path: String,
    pub display_path: String,
    pub display_name: String,
    pub item_kind: String,
    pub is_markdown: bool,
    pub is_context: bool,
    pub is_sensitive: bool,
    pub protected_level: Option<String>,
    pub child_count: i64,
    pub fully_scanned: bool,
    pub collapse_default: bool,
    pub scan_error: Option<String>,
    pub identity: Option<FileIdentity>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanOutcome {
    pub scanned_files: u64,
    pub indexed_documents: u64,
    pub inaccessible_items: u64,
    pub cancelled: bool,
    pub files: Vec<ScannedFile>,
    pub git: Option<GitRepoSummary>,
}

pub fn display_name_for_path(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn display_path_for_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the wire contract the frontend matches on: the Fix C fields serialize
    /// as camelCase `isCurrent` and `app`, default to `false`/`null`, and round-trip.
    #[test]
    fn project_summary_serializes_is_current_and_app_camel_case() {
        let summary = ProjectSummary {
            id: 1,
            name: "ExampleProj".to_string(),
            path: r"D:\Example".to_string(),
            source: "registry".to_string(),
            context_count: 0,
            pinned: false,
            protected_level: None,
            scan_state: "scanned".to_string(),
            scan_root_id: None,
            antigravity_name: Some("ExampleProj".to_string()),
            is_current: true,
            app: Some("antigravity".to_string()),
            apps: vec!["antigravity".to_string(), "claude".to_string()],
        };
        let value = serde_json::to_value(&summary).unwrap();
        assert_eq!(value.get("isCurrent").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            value.get("app").and_then(|v| v.as_str()),
            Some("antigravity")
        );
        assert_eq!(
            value
                .get("apps")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(2)
        );
        // No snake_case leak.
        assert!(value.get("is_current").is_none());

        // Defaults: missing fields deserialize to false / None.
        let minimal = serde_json::json!({
            "id": 2, "name": "x", "path": "p", "source": "s",
            "contextCount": 0, "pinned": false, "protectedLevel": null,
            "scanState": "scanned", "scanRootId": null
        });
        let parsed: ProjectSummary = serde_json::from_value(minimal).unwrap();
        assert!(!parsed.is_current);
        assert_eq!(parsed.app, None);

        // Full round-trip is stable.
        let back: ProjectSummary = serde_json::from_value(value).unwrap();
        assert_eq!(back, summary);
    }

    /// Locks the Phase-6 full-hash confirmation wire contract: camelCase field
    /// names, sane defaults for the optional flags, and a stable round-trip.
    #[test]
    fn duplicate_confirmation_serializes_camel_case() {
        let confirmation = DuplicateConfirmation {
            target_node_id: 7,
            confirmed_groups: vec![ConfirmedDuplicateGroup {
                full_hash: "abc123".to_string(),
                size_bytes: 4096,
                member_count: 2,
                reclaimable_bytes: 4096,
                confidence: "High".to_string(),
                members: vec![DuplicateMember {
                    node_id: 7,
                    project_id: 1,
                    project_name: "demo".to_string(),
                    path: "copy-a.dat".to_string(),
                    display_name: "copy-a.dat".to_string(),
                    physical_bytes: Some(4096),
                    footprint_partial: false,
                }],
            }],
            checked_files: 3,
            bytes_hashed: 12288,
            reclaimable_bytes: 4096,
            partial: false,
        };
        let value = serde_json::to_value(&confirmation).unwrap();
        assert_eq!(value.get("targetNodeId").and_then(|v| v.as_i64()), Some(7));
        assert_eq!(value.get("checkedFiles").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            value.get("bytesHashed").and_then(|v| v.as_u64()),
            Some(12288)
        );
        assert_eq!(
            value.get("reclaimableBytes").and_then(|v| v.as_u64()),
            Some(4096)
        );
        let group = value
            .get("confirmedGroups")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .unwrap();
        assert_eq!(
            group.get("fullHash").and_then(|v| v.as_str()),
            Some("abc123")
        );
        assert_eq!(
            group.get("confidence").and_then(|v| v.as_str()),
            Some("High")
        );
        // No snake_case leak.
        assert!(value.get("target_node_id").is_none());

        // Defaults: missing optional fields deserialize to their zero values.
        let minimal = serde_json::json!({ "targetNodeId": 9 });
        let parsed: DuplicateConfirmation = serde_json::from_value(minimal).unwrap();
        assert_eq!(parsed.target_node_id, 9);
        assert!(parsed.confirmed_groups.is_empty());
        assert_eq!(parsed.checked_files, 0);
        assert_eq!(parsed.bytes_hashed, 0);
        assert_eq!(parsed.reclaimable_bytes, 0);
        assert!(!parsed.partial);

        // Full round-trip is stable.
        let back: DuplicateConfirmation = serde_json::from_value(value).unwrap();
        assert_eq!(back, confirmation);
    }
}
