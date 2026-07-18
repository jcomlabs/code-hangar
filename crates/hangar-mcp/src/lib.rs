//! Connected-AI-app server runtime: a hand-rolled JSON-RPC 2.0 stdio transport
//! speaking the Model Context Protocol (MCP), so the AI apps Code Hangar catalogs
//! (Claude, Cursor, Codex, …) can read — and, when the user enables AI write mode,
//! annotate — the curated knowledge: comments on projects, folders and files, plus
//! a no-bodies project-context summary.
//!
//! Safety shape (mirrored in SECURITY_INVARIANTS.md):
//! - **stdio only.** One short-lived child process, one peer, no socket/port/
//!   listener. stdout carries ONLY framed JSON-RPC; every diagnostic goes to stderr.
//! - **One policy path.** Every tool call is translated into a
//!   [`hangar_agent::AgentRequest`] and dispatched through
//!   [`hangar_api::dispatch_agent_request`], which runs the SAME token auth, scope
//!   and project gates, and audit log as the in-process named-pipe server. This
//!   crate holds no DB handle and re-implements no policy.
//! - **No file bodies.** No tool or resource exposes file contents; the read_body
//!   path is never linked here.

use std::io::{self, BufRead, Read, Write};

use hangar_agent::{AgentMethod, AgentRequest, AgentResponse};
use hangar_api::AppState;
use serde_json::{json, Value};

/// The MCP protocol revisions this server implements. On `initialize` we echo the
/// client's offered version when it is one of these (so a newer client is not forced
/// to downgrade), otherwise we answer with our latest supported (`SUPPORTED_PROTOCOL_VERSIONS[0]`).
/// Newest first. We still reject JSON-RPC batching, which the 2025 revisions require —
/// see the batch refusal in `handle_line` — so this only negotiates the version string;
/// no batching capability is implied.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// The version answered when the client offers none, or one we do not implement.
const LATEST_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];
/// Identity reported in `initialize` → `serverInfo`.
const SERVER_NAME: &str = "code-hangar";
/// The crate version, which IS the product version: this crate and the desktop app
/// both inherit the single `version.workspace` value, so `CARGO_PKG_VERSION` already
/// tracks the shipped Code Hangar version — no separate sync is needed.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Largest single JSON-RPC line accepted from the peer (bytes). Matches the agent
/// protocol response bound so a misbehaving peer cannot push us to allocate without
/// limit.
const MAX_LINE_BYTES: usize = hangar_agent::MAX_RESPONSE_BYTES;

// JSON-RPC 2.0 error codes used here.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// MCP resource-not-found (spec code, distinct from JSON-RPC method-not-found). Used
/// when a `resources/read` names a resource we do not expose.
const RESOURCE_NOT_FOUND: i64 = -32002;

/// How a tool is gated, so `tools/list` can advertise only what the held app could
/// actually invoke. This mirrors the enforcement in `hangar-api` — it never replaces
/// it (every `tools/call` still runs the full authenticated, scope- and toggle-gated
/// dispatch); it just hides tools an app cannot use from the advertised catalog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolGate {
    /// A read-only tool available whenever the app holds this read scope (every read
    /// tool currently requires one — `read_structure`, `read_graph` or
    /// `history_search`), so an unresolved token, holding no scope, is shown none.
    Read(&'static str),
    /// An own-comment write: needs the given scope (`comments_write`). The global
    /// `comment_write_enabled` toggle is additionally enforced at call time.
    CommentWrite(&'static str),
    /// A queue-only `request_*` tool: needs the given scope AND the total-control
    /// tier toggle. It only FILES a request; nothing executes until a human approves.
    Request(&'static str),
    /// `request_final_remove`: like `Request`, but ALSO needs the final-removal opt-in.
    RequestFinalRemove(&'static str),
}

/// How a tool behaves, surfaced to the client as MCP `annotations` so an app (and its
/// user) can reason about a tool before calling it. Read tools are read-only; the
/// queue-only `request_*` tools change nothing themselves (they only file a request a
/// human must approve); comment writes mutate but are non-destructive and reversible.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolBehavior {
    ReadOnly,
    CommentWrite,
    QueueOnlyRequest,
}

/// One MCP tool, paired with the agent method it is translated into. The tool's
/// JSON `arguments` are passed straight through as the agent request `params`
/// because the agent param structs already use the same camelCase keys.
struct ToolDef {
    name: &'static str,
    /// A short human-readable title (MCP `title`), distinct from the machine `name`.
    title: &'static str,
    description: &'static str,
    method: AgentMethod,
    input_schema: fn() -> Value,
    gate: ToolGate,
    behavior: ToolBehavior,
}

impl ToolBehavior {
    /// The MCP `annotations` object for this behavior. Read tools set
    /// `readOnlyHint:true`; the queue-only request tools set every hint to false
    /// (they neither read, mutate, nor destroy directly — they only file a request);
    /// comment writes are non-read, non-destructive mutations.
    fn annotations(self) -> Value {
        match self {
            ToolBehavior::ReadOnly => json!({ "readOnlyHint": true }),
            ToolBehavior::QueueOnlyRequest => json!({
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false
            }),
            ToolBehavior::CommentWrite => json!({
                "readOnlyHint": false,
                "destructiveHint": false
            }),
        }
    }
}

impl ToolGate {
    /// Whether the held app — with the given catalog context — may be advertised this
    /// tool. Read/own-comment-write tools appear when the app holds the scope;
    /// `request_*` tools additionally require the total-control tier; final-remove
    /// additionally requires the final-removal opt-in. When the token did not resolve
    /// (`has_scope` is false for everything), no tool passes — every tool requires some
    /// scope — so an unknown/revoked token is advertised nothing at all, the safest
    /// possible minimum. (A real read still needs a real read scope, so an app that
    /// authenticates but holds no scopes likewise sees an empty catalog.)
    fn visible_to(self, ctx: &hangar_api::McpCatalogContext) -> bool {
        match self {
            ToolGate::Read(scope) | ToolGate::CommentWrite(scope) => ctx.has_scope(scope),
            ToolGate::Request(scope) => ctx.total_control_enabled && ctx.has_scope(scope),
            ToolGate::RequestFinalRemove(scope) => {
                ctx.total_control_enabled && ctx.final_remove_enabled && ctx.has_scope(scope)
            }
        }
    }
}

fn node_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "nodeId": { "type": "integer", "description": "The Code Hangar node id of the project, folder or file." }
        },
        "required": ["nodeId"],
        "additionalProperties": false
    })
}

fn add_comment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "nodeId": { "type": "integer", "description": "The node id (project, folder or file) to annotate." },
            "body": { "type": "string", "description": "The comment text. Stored locally and encrypted; never sent anywhere." }
        },
        "required": ["nodeId", "body"],
        "additionalProperties": false
    })
}

fn edit_comment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "commentId": { "type": "integer", "description": "The id of a comment THIS app authored. Human comments cannot be edited." },
            "body": { "type": "string", "description": "The replacement comment text." }
        },
        "required": ["commentId", "body"],
        "additionalProperties": false
    })
}

fn project_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "projectId": { "type": "integer", "description": "The Code Hangar project (node) id." }
        },
        "required": ["projectId"],
        "additionalProperties": false
    })
}

fn request_comment_change_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "commentId": { "type": "integer", "description": "The id of the comment to change (may be a human comment)." },
            "action": { "type": "string", "enum": ["edit", "delete"], "description": "Whether to request an edit or a delete." },
            "body": { "type": "string", "description": "The proposed new text (required for an edit)." }
        },
        "required": ["commentId", "action"],
        "additionalProperties": false
    })
}

fn no_args_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn nav_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "navId": { "type": "integer", "description": "The nav-tree id of the folder (from list_project_nav)." }
        },
        "required": ["navId"],
        "additionalProperties": false
    })
}

fn project_graph_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "projectId": { "type": "integer", "description": "The Code Hangar project (node) id." },
            "limit": {
                "type": "integer",
                "minimum": 25,
                "maximum": 1000,
                "description": "Maximum number of graph nodes to return (default 300; allowed range 25 to 1000)."
            }
        },
        "required": ["projectId"],
        "additionalProperties": false
    })
}

fn nav_children_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "projectId": { "type": "integer", "description": "The Code Hangar project (node) id." },
            "parentNavId": { "type": "integer", "description": "The folder to list children of (omit for the project root)." },
            "limit": { "type": "integer", "description": "Page size (default 200)." },
            "offset": { "type": "integer", "description": "Page offset (default 0)." }
        },
        "required": ["projectId"],
        "additionalProperties": false
    })
}

fn orphan_assets_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "projectId": { "type": "integer", "description": "The project to scan for orphan assets." },
            "minSizeBytes": { "type": "integer", "description": "Ignore candidates smaller than this." },
            "assetKind": { "type": "string", "description": "Optional asset-kind filter." },
            "minConfidence": { "type": "string", "description": "Optional minimum confidence (e.g. \"high\")." },
            "includePartial": { "type": "boolean", "description": "Include candidates with partial footprints." },
            "limit": { "type": "integer", "description": "Maximum candidates to return (default 50)." }
        },
        "required": ["projectId"],
        "additionalProperties": false
    })
}

fn duplicate_candidates_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "projectId": { "type": "integer", "description": "The project to scan for duplicate files." },
            "minSizeBytes": { "type": "integer", "description": "Ignore groups whose files are smaller than this." },
            "fileKind": { "type": "string", "description": "Optional file-kind filter." },
            "limit": { "type": "integer", "description": "Maximum duplicate groups to return (default 25)." }
        },
        "required": ["projectId"],
        "additionalProperties": false
    })
}

fn search_sessions_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Text to search for (at least 3 characters)." },
            "projectId": { "type": "integer", "description": "Restrict the search to this project's sessions." },
            "limit": { "type": "integer", "description": "Maximum hits to return (1-50, default 20)." }
        },
        "required": ["query", "projectId"],
        "additionalProperties": false
    })
}

fn node_action_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "nodeId": { "type": "integer", "description": "The Code Hangar node id of the target project, folder or file." },
            "actionLabel": { "type": "string", "description": "A short human-readable label for what this is for (shown to the user)." },
            "level": { "type": "string", "description": "Backup level (e.g. \"standard\"). Optional." },
            "includeProtected": { "type": "boolean", "description": "Whether to propose including protected/sensitive files (the user still opts in)." }
        },
        "required": ["nodeId", "actionLabel"],
        "additionalProperties": false
    })
}

fn entry_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entryId": { "type": "integer", "description": "The id of a prior holding-area entry to remove." }
        },
        "required": ["entryId"],
        "additionalProperties": false
    })
}

/// The full, fixed tool catalogue: a set of pre-cooked, read-only (plus the gated
/// comment writes) functions an app can use to DISCOVER a project's main
/// functionalities — its catalog, context, navigation, dependency graph and cleanup
/// intelligence — never its file or session bodies. Reads need the
/// `read_structure`/`read_graph`/`comments_read`/`history_search` scope as noted;
/// the comment writes need `comments_write` AND the global AI-write toggle. No tool
/// deletes a comment or exposes a file body, and no tool executes an irreversible or
/// human-data action directly — `request_comment_change` only queues a request.
fn tools() -> Vec<ToolDef> {
    vec![
        // ---- Comments (read + gated write) ----
        ToolDef {
            name: "list_comments",
            title: "List comments",
            description: "List the comments attached to a project, folder or file (by node id). Read-only.",
            method: AgentMethod::CommentsList,
            input_schema: node_id_schema,
            gate: ToolGate::Read("comments_read"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "add_comment",
            title: "Add a comment",
            description: "Add a comment to a project, folder or file. Requires the user to have enabled AI write mode. The comment is recorded as authored by this app and can never overwrite a human comment.",
            method: AgentMethod::CommentsAdd,
            input_schema: add_comment_schema,
            gate: ToolGate::CommentWrite("comments_write"),
            behavior: ToolBehavior::CommentWrite,
        },
        ToolDef {
            name: "edit_comment",
            title: "Edit your comment",
            description: "Edit a comment THIS app previously wrote. Human comments and other apps' comments are refused.",
            method: AgentMethod::CommentsEdit,
            input_schema: edit_comment_schema,
            gate: ToolGate::CommentWrite("comments_write"),
            behavior: ToolBehavior::CommentWrite,
        },
        ToolDef {
            name: "request_comment_change",
            title: "Request a comment change",
            description: "Request that a comment (including a human's) be edited or deleted. Available only under the user's default-off 'total control' setting. This does NOT change anything: it queues a request the user must approve in Code Hangar, which then acts on their own behalf and can back up the comment first.",
            method: AgentMethod::RequestCommentChange,
            input_schema: request_comment_change_schema,
            gate: ToolGate::Request("comments_write"),
            behavior: ToolBehavior::QueueOnlyRequest,
        },
        // ---- Discovery: structure & context (read_structure) ----
        ToolDef {
            name: "list_catalog",
            title: "List catalog",
            description: "List the projects this app may see: id, name, path, owning app and scan state. Scoped to your grants — never the user's full inventory. The starting point for discovery. Takes no arguments.",
            method: AgentMethod::ListCatalog,
            input_schema: no_args_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "get_project_context",
            title: "Get project context",
            description: "Get a project's curated context: its summary and the list of context files (names and paths only — never file contents).",
            method: AgentMethod::AgentProjectContext,
            input_schema: project_id_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_context_files",
            title: "List context files",
            description: "List just a project's curated context files (names and paths, no bodies).",
            method: AgentMethod::ListContextFiles,
            input_schema: project_id_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_project_nav",
            title: "List project navigation",
            description: "Page through a project's folder/file navigation tree (names, sizes and flags — never file contents). Pass a parentNavId to descend.",
            method: AgentMethod::ListProjectNav,
            input_schema: nav_children_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "explain_folder",
            title: "Explain a folder",
            description: "Explain what a folder is and why (its role and detected signals), by nav id from list_project_nav. No file contents.",
            method: AgentMethod::ExplainFolder,
            input_schema: nav_id_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "project_git_status",
            title: "Project Git status",
            description: "A project's Git metadata: whether it is a repo, current branch, head ref and origin URL. No working-tree contents.",
            method: AgentMethod::ProjectGitStatus,
            input_schema: project_id_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_adapters",
            title: "List AI-app adapters",
            description: "List the AI-app adapters Code Hangar understands (static capability metadata, not user data). Takes no arguments.",
            method: AgentMethod::ListAdapters,
            input_schema: no_args_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_my_requests",
            title: "List your requests",
            description: "List the requests THIS app has filed and their status (id, method, status, created/resolved time). Own-app-scoped — never another app's requests. Read-only; the way to see whether a queued request was approved or denied. Takes no arguments.",
            method: AgentMethod::ListMyRequests,
            input_schema: no_args_schema,
            gate: ToolGate::Read("read_structure"),
            behavior: ToolBehavior::ReadOnly,
        },
        // ---- Discovery: dependency graph & cleanup intelligence (read_graph) ----
        ToolDef {
            name: "get_project_graph",
            title: "Get project graph",
            description: "The dependency-graph map for a project: nodes, edges and detected issues (structure only, no file contents).",
            method: AgentMethod::GetProjectGraph,
            input_schema: project_graph_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "node_relationships",
            title: "Node relationships",
            description: "A node's incoming and outgoing relationships within its project. Edges into projects you cannot see are omitted.",
            method: AgentMethod::NodeRelationships,
            input_schema: node_id_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_orphan_assets",
            title: "List orphan assets",
            description: "List candidate orphan assets within a project (files nothing appears to reference). Read-only analysis; deletes are never performed here.",
            method: AgentMethod::ListOrphanAssets,
            input_schema: orphan_assets_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "node_orphan_status",
            title: "Node orphan status",
            description: "Whether a single node looks orphaned, and why (reference count, confidence).",
            method: AgentMethod::NodeOrphanStatus,
            input_schema: node_id_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "list_duplicate_candidates",
            title: "List duplicate candidates",
            description: "List likely-duplicate file groups within a project (partial-hash heuristic). Members in projects you cannot see are omitted.",
            method: AgentMethod::ListDuplicateCandidates,
            input_schema: duplicate_candidates_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        ToolDef {
            name: "confirm_duplicate_group",
            title: "Confirm duplicate group",
            description: "Full-hash confirmation of a node's duplicate group, with reclaimable bytes. Members in projects you cannot see are omitted.",
            method: AgentMethod::ConfirmDuplicateGroup,
            input_schema: node_id_schema,
            gate: ToolGate::Read("read_graph"),
            behavior: ToolBehavior::ReadOnly,
        },
        // ---- Discovery: sessions (history_search) ----
        ToolDef {
            name: "search_sessions",
            title: "Search session history",
            description: "Search a project's AI session history for a phrase and get redacted snippets (never the full transcript). Needs a projectId.",
            method: AgentMethod::DeepHistorySearch,
            input_schema: search_sessions_schema,
            gate: ToolGate::Read("history_search"),
            behavior: ToolBehavior::ReadOnly,
        },
        // ---- Total-control request kinds. Each only QUEUES a request the user must
        // approve in Code Hangar; the app then acts as the user. Available only under
        // the default-off total-control tier, and only with the matching scopes.
        // (There is deliberately no request_file_access tool: its approval mints a read
        // grant no MCP tool can redeem — a dead end — so it is not advertised or
        // dispatched here. The backend RequestReadBody method remains for the in-app
        // surface.)
        ToolDef {
            name: "request_backup_protected",
            title: "Request protected backup",
            description: "Request a verified backup of a target that INCLUDES protected/sensitive files. Queues a request; the user approves it, picks the backup folder and opts in. Reversible (copies, never deletes).",
            method: AgentMethod::RequestBackupProtected,
            input_schema: node_action_schema,
            gate: ToolGate::Request("execute_plan"),
            behavior: ToolBehavior::QueueOnlyRequest,
        },
        ToolDef {
            name: "request_move_to_holding",
            title: "Request move to holding",
            description: "Request moving a target into the holding area (reversible). Queues a request; the user approves it, and a verified backup is made first. Nothing moves until then.",
            method: AgentMethod::RequestMoveToHolding,
            input_schema: node_action_schema,
            gate: ToolGate::Request("execute_plan"),
            behavior: ToolBehavior::QueueOnlyRequest,
        },
        ToolDef {
            name: "request_final_remove",
            title: "Request final removal",
            description: "Request the irreversible permanent removal of a prior holding-area entry. Queues a request behind the highest-friction approval; performed only if the user has enabled final removal.",
            method: AgentMethod::RequestPermanentDelete,
            input_schema: entry_id_schema,
            gate: ToolGate::RequestFinalRemove("execute_plan"),
            behavior: ToolBehavior::QueueOnlyRequest,
        },
    ]
}

/// Render one tool's JSON (name, title, description, schema, annotations). The
/// `title` and `annotations` are on-brand safety UX: a human-readable label and
/// machine-readable behavior hints (read-only / queue-only / non-destructive) so an
/// app and its user can reason about a tool before invoking it.
fn tool_json(tool: &ToolDef) -> Value {
    json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": (tool.input_schema)(),
        "annotations": tool.behavior.annotations(),
    })
}

/// The FULL catalog (every tool), unfiltered. Only the per-token-filtered
/// [`tool_definitions_for`] is served on the wire; this is used by tests to assert on
/// the complete catalog shape.
#[cfg(test)]
fn tool_definitions() -> Value {
    Value::Array(tools().iter().map(tool_json).collect())
}

/// The catalog FILTERED to what the held app can actually invoke, given its resolved
/// scopes and the tier toggles: `request_*` tools only under total control (and
/// `request_final_remove` only with the final-removal opt-in on too), comment-writes
/// only with the comment-write scope, reads only with their read scope. An unresolved
/// (invalid/revoked) token leaves the read-only minimum. This is a per-session UX
/// filter, not the gate — every call is still fully re-checked in `hangar-api`.
fn tool_definitions_for(ctx: &hangar_api::McpCatalogContext) -> Value {
    Value::Array(
        tools()
            .iter()
            .filter(|tool| tool.gate.visible_to(ctx))
            .map(tool_json)
            .collect(),
    )
}

fn method_for_tool(name: &str) -> Option<AgentMethod> {
    tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.method)
}

/// Map a dispatched [`AgentResponse`] to an MCP `tools/call` result. A refusal
/// (bad token, missing scope, write gate off, human-record boundary) is reported
/// as `isError: true` tool content — the MCP convention for an execution-level
/// failure — not as a JSON-RPC protocol error.
fn agent_response_to_tool_result(response: AgentResponse) -> Value {
    if response.ok {
        let text = response
            .result
            .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()))
            .unwrap_or_default();
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        })
    } else {
        let message = response
            .error
            .unwrap_or_else(|| "The request was refused.".to_string());
        json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true
        })
    }
}

/// The MCP server bound to one opened encrypted database and one authentication
/// token (the per-app secret minted at registration, supplied to the binary via
/// the environment). It owns the [`AppState`] for the life of the stdio session.
pub struct ConnectedAppServer {
    state: AppState,
    token: String,
}

impl ConnectedAppServer {
    pub fn new(state: AppState, token: String) -> Self {
        Self { state, token }
    }

    /// Serve a full session over the given reader/writer until EOF. Each inbound
    /// line is one JSON-RPC message; each response is one line. Notifications
    /// (no `id`) produce no output.
    ///
    /// Each line is read with a hard byte cap so a peer that streams bytes without
    /// a terminating newline cannot make us allocate without limit: we read at most
    /// `MAX_LINE_BYTES + 1` bytes per message, and a line that reaches the cap with
    /// no newline is reported as oversized and ends the session (rather than growing
    /// memory unbounded or risking a desync) — mirroring the fixed-buffer discipline
    /// of the named-pipe path.
    pub fn serve(&self, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        let mut raw = Vec::new();
        loop {
            raw.clear();
            let read = (&mut input)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut raw)?;
            if read == 0 {
                return Ok(()); // EOF
            }
            // Hit the cap without a terminating newline: oversized or never-ending
            // message. Report it once and end the session.
            if raw.len() > MAX_LINE_BYTES && raw.last() != Some(&b'\n') {
                write_line(
                    &mut output,
                    &error_response(
                        Value::Null,
                        INVALID_REQUEST,
                        "Request exceeds the maximum message size.",
                    ),
                )?;
                return Ok(());
            }
            let line = match std::str::from_utf8(&raw) {
                Ok(text) => text.trim_end_matches(['\r', '\n']),
                Err(_) => {
                    write_line(
                        &mut output,
                        &error_response(Value::Null, PARSE_ERROR, "Request was not valid UTF-8."),
                    )?;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(line) {
                write_line(&mut output, &response)?;
            }
        }
    }

    /// Serve over the process's real stdin/stdout. stdout is reserved for the
    /// JSON-RPC stream; callers must route logs to stderr.
    pub fn run_stdio(&self) -> io::Result<()> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        self.serve(stdin.lock(), stdout.lock())
    }

    /// Handle one JSON-RPC line. Returns `Some(json)` to send back, or `None` for
    /// a notification (a message with no `id`), which gets no response.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        if line.len() > MAX_LINE_BYTES {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "Request exceeds the maximum message size.",
            ));
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("Parse error: {error}"),
                ))
            }
        };

        // A top-level array is a JSON-RPC batch. We serve one message per line and
        // do not support batching, so fail the peer fast with an explicit error
        // rather than silently dropping it (which would hang a batching client).
        if message.is_array() {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "Batch requests are not supported; send one JSON-RPC message per line.",
            ));
        }

        // A request carries an `id`; a notification omits it and is never answered.
        let id = match message.get("id") {
            Some(id) => id.clone(),
            None => return None,
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        let response = match method {
            "initialize" => success_response(id, self.initialize_result(&params)),
            "ping" => success_response(id, json!({})),
            "tools/list" => success_response(id, json!({ "tools": self.visible_tools() })),
            "tools/call" => self.tools_call(id, params),
            // We advertise the resources capability but expose none yet; curated
            // resources arrive in a later milestone. Tools cover the same reads.
            "resources/list" => success_response(id, json!({ "resources": [] })),
            "resources/templates/list" => success_response(id, json!({ "resourceTemplates": [] })),
            "prompts/list" => success_response(id, json!({ "prompts": [] })),
            // The resources capability is advertised but exposes no readable resource
            // yet, so any concrete URI is "resource not found" (MCP -32002) rather than
            // "method not found" (-32601): the method exists, the resource does not.
            "resources/read" => error_response(
                id,
                RESOURCE_NOT_FOUND,
                "No readable resources are exposed; use the comment and context tools.",
            ),
            other => error_response(id, METHOD_NOT_FOUND, &format!("Method not found: {other}")),
        };
        Some(response)
    }

    /// The advertised catalog, filtered to what this session's app token can invoke.
    /// Resolving the context is a read-only lookup; if it fails (DB error) we fall
    /// back to the read-only minimum rather than leaking the full catalog.
    fn visible_tools(&self) -> Value {
        match hangar_api::mcp_catalog_context(&self.state, &self.token) {
            Ok(ctx) => tool_definitions_for(&ctx),
            // Fail closed: advertise as if the token did not resolve (read-only set).
            Err(_) => tool_definitions_for(&hangar_api::McpCatalogContext {
                scopes: None,
                total_control_enabled: false,
                final_remove_enabled: false,
            }),
        }
    }

    /// Negotiate the protocol version: echo the client's offered `protocolVersion`
    /// when we implement it (so a newer client keeps its revision), otherwise answer
    /// with our latest. We still refuse JSON-RPC batching (see `handle_line`), which
    /// the 2025 revisions mandate — only the version string is negotiated here.
    fn initialize_result(&self, params: &Value) -> Value {
        let offered = params.get("protocolVersion").and_then(Value::as_str);
        let protocol_version = match offered {
            Some(offered) if SUPPORTED_PROTOCOL_VERSIONS.contains(&offered) => offered,
            _ => LATEST_PROTOCOL_VERSION,
        };
        json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            },
            "instructions": "Code Hangar exposes your curated project knowledge. Read and (when you enable AI write mode in Code Hangar) annotate projects, folders and files with comments. File contents are never exposed, and access is limited to the projects you granted this app."
        })
    }

    fn tools_call(&self, id: Value, params: Value) -> String {
        // `params` must be a JSON object carrying at least `name`. A non-object (array,
        // string, number, null) is a malformed request — return -32602 with a clear
        // hint rather than the misleading "Unknown tool: " that a bare name lookup on a
        // non-object would otherwise produce.
        let Some(params) = params.as_object() else {
            return error_response(
                id,
                INVALID_PARAMS,
                "params must be an object with a name field.",
            );
        };
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let method = match method_for_tool(name) {
            Some(method) => method,
            None => {
                return error_response(id, INVALID_PARAMS, &format!("Unknown tool: {name}"));
            }
        };
        let request = AgentRequest {
            protocol: hangar_agent::PROTOCOL_VERSION.to_string(),
            request_id: request_id_label(&id),
            token: Some(self.token.clone()),
            method,
            params: arguments,
        };
        let response = hangar_api::dispatch_agent_request(&self.state, request);
        success_response(id, agent_response_to_tool_result(response))
    }
}

fn write_line(output: &mut impl Write, response: &str) -> io::Result<()> {
    output.write_all(response.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn request_id_label(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(response: &str) -> Value {
        serde_json::from_str(response).expect("valid JSON-RPC response")
    }

    fn server() -> ConnectedAppServer {
        ConnectedAppServer::new(AppState::memory().unwrap(), "unused-token".to_string())
    }

    /// A catalog context that resolves EVERY scope with both tier toggles on — the
    /// widest advertised set, used to assert on the full catalog shape without a
    /// registered token (registration needs the Windows named pipe).
    fn full_ctx() -> hangar_api::McpCatalogContext {
        hangar_api::McpCatalogContext {
            scopes: Some(vec![
                "comments_read".to_string(),
                "comments_write".to_string(),
                "read_structure".to_string(),
                "read_graph".to_string(),
                "history_search".to_string(),
                "execute_plan".to_string(),
            ]),
            total_control_enabled: true,
            final_remove_enabled: true,
        }
    }

    fn names_of(catalog: &Value) -> Vec<String> {
        catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn initialize_advertises_tools_and_identity() {
        let response = parse(
            &server()
                .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
                .unwrap(),
        );
        assert_eq!(response["id"], 1);
        // No client version offered -> our latest supported.
        assert_eq!(
            response["result"]["protocolVersion"],
            LATEST_PROTOCOL_VERSION
        );
        assert_eq!(response["result"]["serverInfo"]["name"], "code-hangar");
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_negotiates_the_protocol_version() {
        // A supported version the client offers is echoed back (client keeps its rev).
        for offered in SUPPORTED_PROTOCOL_VERSIONS {
            let response = parse(
                &server()
                    .handle_line(&format!(
                        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{offered}"}}}}"#
                    ))
                    .unwrap(),
            );
            assert_eq!(response["result"]["protocolVersion"], offered);
        }
        // An unsupported/older version falls back to our latest.
        let response = parse(
            &server()
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
                )
                .unwrap(),
        );
        assert_eq!(
            response["result"]["protocolVersion"],
            LATEST_PROTOCOL_VERSION
        );
    }

    #[test]
    fn full_catalog_covers_the_surface_with_no_delete_or_body_and_carries_metadata() {
        let catalog = tool_definitions_for(&full_ctx());
        let names = names_of(&catalog);
        assert!(names.contains(&"list_comments".to_string()));
        assert!(names.contains(&"add_comment".to_string()));
        assert!(names.contains(&"edit_comment".to_string()));
        assert!(names.contains(&"get_project_context".to_string()));
        // The discovery surface + the new own-request list + the queue-only requests.
        for expected in [
            "list_catalog",
            "list_context_files",
            "list_project_nav",
            "explain_folder",
            "project_git_status",
            "list_adapters",
            "list_my_requests",
            "get_project_graph",
            "node_relationships",
            "list_orphan_assets",
            "node_orphan_status",
            "list_duplicate_candidates",
            "confirm_duplicate_group",
            "search_sessions",
            "request_comment_change",
            "request_backup_protected",
            "request_move_to_holding",
            "request_final_remove",
        ] {
            assert!(names.contains(&expected.to_string()), "missing: {expected}");
        }
        // request_file_access is retired from the MCP catalog for v1 (its approval
        // minted a read grant no MCP tool could redeem) — never advertised again.
        assert!(!names.iter().any(|name| name == "request_file_access"));
        // No comment-deletion or file-body tool is ever advertised.
        assert!(!names.iter().any(|name| name.contains("delete")));
        assert!(!names.iter().any(|name| name.contains("body")));
        assert!(!names.iter().any(|name| name.contains("read_body")));

        // Every advertised tool carries a title and annotations (Fix 6): reads are
        // readOnlyHint:true; request_* are readOnly/destructive/idempotent all false
        // (they only file a request); comment writes are readOnly:false + destructive:false.
        for tool in catalog.as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert!(
                tool["title"].as_str().is_some_and(|t| !t.is_empty()),
                "tool {name} has no title"
            );
            let ann = &tool["annotations"];
            assert!(ann.is_object(), "tool {name} has no annotations");
            if name.starts_with("request_") {
                assert_eq!(ann["readOnlyHint"], false, "{name}");
                assert_eq!(ann["destructiveHint"], false, "{name}");
                assert_eq!(ann["idempotentHint"], false, "{name}");
            } else if name == "add_comment" || name == "edit_comment" {
                assert_eq!(ann["readOnlyHint"], false, "{name}");
                assert_eq!(ann["destructiveHint"], false, "{name}");
            } else {
                assert_eq!(ann["readOnlyHint"], true, "{name}");
            }
            // additionalProperties:false is preserved on every input schema.
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }

        // Belt-and-suspenders: no advertised tool maps to a body/plan/mutation method.
        for tool in tools() {
            assert!(
                !matches!(
                    tool.method,
                    AgentMethod::AgentReadBody
                        | AgentMethod::AgentPlanBuild
                        | AgentMethod::AgentPlanExecute
                ),
                "tool {} maps to a forbidden method",
                tool.name
            );
        }
    }

    #[test]
    fn project_graph_schema_advertises_the_authenticated_resource_bounds() {
        let definition = tools()
            .into_iter()
            .find(|tool| tool.name == "get_project_graph")
            .expect("get_project_graph tool");
        let schema = (definition.input_schema)();
        let limit = &schema["properties"]["limit"];

        assert_eq!(limit["minimum"], 25);
        assert_eq!(limit["maximum"], 1_000);
    }

    #[test]
    fn tools_list_is_scope_and_tier_aware() {
        // An unresolved token (no scopes, toggles off): only the read-only minimum,
        // and specifically NONE of the scoped reads or any request_/write tool.
        let none = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: None,
            total_control_enabled: false,
            final_remove_enabled: false,
        }));
        assert!(
            none.is_empty(),
            "unknown token must see no scoped tools, got {none:?}"
        );

        // read_structure alone: the structure reads (incl. list_my_requests) but no
        // graph read, no comment write, no request_ tool.
        let structure = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: Some(vec!["read_structure".to_string()]),
            total_control_enabled: false,
            final_remove_enabled: false,
        }));
        assert!(structure.contains(&"list_catalog".to_string()));
        assert!(structure.contains(&"list_my_requests".to_string()));
        assert!(!structure.contains(&"get_project_graph".to_string()));
        assert!(!structure.contains(&"add_comment".to_string()));
        assert!(!structure.iter().any(|n| n.starts_with("request_")));

        // comments_write surfaces the write tools; request_comment_change stays hidden
        // until total control is ON.
        let cw_no_tc = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: Some(vec!["comments_write".to_string()]),
            total_control_enabled: false,
            final_remove_enabled: false,
        }));
        assert!(cw_no_tc.contains(&"add_comment".to_string()));
        assert!(cw_no_tc.contains(&"edit_comment".to_string()));
        assert!(!cw_no_tc.contains(&"request_comment_change".to_string()));
        let cw_tc = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: Some(vec!["comments_write".to_string()]),
            total_control_enabled: true,
            final_remove_enabled: false,
        }));
        assert!(cw_tc.contains(&"request_comment_change".to_string()));

        // execute_plan under total control shows backup/move requests, but
        // request_final_remove ONLY when the final-removal opt-in is also on.
        let plan_tc = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: Some(vec!["execute_plan".to_string()]),
            total_control_enabled: true,
            final_remove_enabled: false,
        }));
        assert!(plan_tc.contains(&"request_backup_protected".to_string()));
        assert!(plan_tc.contains(&"request_move_to_holding".to_string()));
        assert!(!plan_tc.contains(&"request_final_remove".to_string()));
        let plan_tc_fr = names_of(&tool_definitions_for(&hangar_api::McpCatalogContext {
            scopes: Some(vec!["execute_plan".to_string()]),
            total_control_enabled: true,
            final_remove_enabled: true,
        }));
        assert!(plan_tc_fr.contains(&"request_final_remove".to_string()));

        // The full catalog stays in sync with the FULL context (no tool is
        // permanently orphaned by the filter).
        assert_eq!(
            names_of(&tool_definitions()).len(),
            names_of(&tool_definitions_for(&full_ctx())).len()
        );
    }

    #[test]
    fn notification_without_id_gets_no_response() {
        assert!(server()
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let response = parse(
            &server()
                .handle_line(r#"{"jsonrpc":"2.0","id":9,"method":"does/not/exist"}"#)
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let response = parse(
            &server()
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
                )
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn request_file_access_is_no_longer_a_dispatchable_tool() {
        // Retired from the catalog: calling it is an unknown tool, not a dead-end
        // grant request. (The backend RequestReadBody method still exists for the
        // in-app surface; it is simply not reachable over MCP.)
        assert!(method_for_tool("request_file_access").is_none());
        let response = parse(
            &server()
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"request_file_access","arguments":{"nodeId":1}}}"#,
                )
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown tool"));
    }

    #[test]
    fn tools_call_with_non_object_params_is_invalid_params() {
        // A non-object params value must yield -32602 with a helpful message, not the
        // misleading "Unknown tool: " a bare name lookup would produce.
        let response = parse(
            &server()
                .handle_line(r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":[1,2,3]}"#)
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("object with a name field"));
    }

    #[test]
    fn resources_read_is_resource_not_found() {
        // The method exists (resources capability advertised) but no resource is
        // exposed: MCP -32002, not JSON-RPC -32601.
        let response = parse(
            &server()
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":13,"method":"resources/read","params":{"uri":"codehangar://nope"}}"#,
                )
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], RESOURCE_NOT_FOUND);
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let response = parse(&server().handle_line("{not json").unwrap());
        assert_eq!(response["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn batch_request_array_is_rejected_not_swallowed() {
        // A top-level array must fail fast with an explicit error rather than be
        // silently dropped (which would hang a batching peer).
        let response = parse(
            &server()
                .handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#)
                .unwrap(),
        );
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn serve_reports_oversized_line_and_does_not_read_unbounded() {
        // A stream of non-newline bytes past the cap is reported as oversized and
        // ends the session — the read is bounded at MAX_LINE_BYTES + 1, never the
        // full (here, cap + 4096) input.
        let oversized = vec![b'a'; MAX_LINE_BYTES + 4096];
        let mut output = Vec::new();
        server()
            .serve(io::Cursor::new(oversized), &mut output)
            .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("maximum message size"));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn serve_handles_multiple_lines_and_skips_notifications() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n"
        );
        let mut output = Vec::new();
        server().serve(io::Cursor::new(input), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // The notification in the middle produces no response line.
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"id\":1"));
        assert!(lines[1].contains("\"id\":2"));
    }

    #[test]
    fn tool_call_with_invalid_token_is_reported_as_tool_error() {
        // The full translation pipeline reaches dispatch: a bogus token is refused
        // at authentication and surfaces as isError content, not a protocol error.
        let response = parse(
            &server()
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_comments","arguments":{"nodeId":1}}}"#,
                )
                .unwrap(),
        );
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(text.contains("token") || text.contains("authentication"));
    }

    #[test]
    fn response_mapping_distinguishes_success_from_refusal() {
        let ok =
            agent_response_to_tool_result(AgentResponse::success("r", json!({ "comments": [] })));
        assert_eq!(ok["isError"], false);
        assert!(ok["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("comments"));

        let err = agent_response_to_tool_result(AgentResponse::failure(
            "r",
            "Agent is not scoped to this project.",
        ));
        assert_eq!(err["isError"], true);
        assert!(err["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not scoped"));
    }

    // A real, authenticated round trip through the MCP layer: register an agent via
    // the same path the UI uses, then list comments for a node in its project. The
    // named-pipe endpoint (Windows-only) is what `automation_register` needs to mint
    // the credential, so this end-to-end check is gated to Windows like the agent
    // crate's pipe test.
    #[cfg(windows)]
    #[test]
    fn authenticated_list_comments_round_trip() {
        let state = AppState::memory().unwrap();
        let projects = hangar_api::projects_list(&state).unwrap();
        let (project_id, node_id) = projects
            .iter()
            .find_map(|project| {
                hangar_api::project_context_files(&state, project.id)
                    .ok()
                    .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
            })
            .expect("a project with a context-file node");

        hangar_api::start_local_automation(&state).unwrap();
        let credential = hangar_api::automation_register(
            &state,
            "hermes-local".to_string(),
            vec!["comments_read".to_string()],
            vec![project_id],
        )
        .unwrap();

        let server = ConnectedAppServer::new(state, credential.token);
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"list_comments","arguments":{{"nodeId":{node_id}}}}}}}"#
        );
        let response = parse(&server.handle_line(&request).unwrap());
        assert_eq!(response["result"]["isError"], false);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("comments"));
        // Out-of-scope and write-gate refusals are covered end to end by the
        // hangar-api `automation_comment_tools_*` unit test, which exercises the
        // same dispatch path this server calls into.
    }

    // The scope-aware `tools/list` over the wire with a REAL registered token: an app
    // granted only `read_structure` sees its structure reads (including the new
    // list_my_requests) but neither the graph reads nor any request_ tool.
    #[cfg(windows)]
    #[test]
    fn tools_list_over_wire_reflects_the_token_scopes() {
        let state = AppState::memory().unwrap();
        let project_id = hangar_api::projects_list(&state).unwrap()[0].id;
        hangar_api::start_local_automation(&state).unwrap();
        let credential = hangar_api::automation_register(
            &state,
            "hermes-structure".to_string(),
            vec!["read_structure".to_string()],
            vec![project_id],
        )
        .unwrap();

        let server = ConnectedAppServer::new(state, credential.token);
        let response = parse(
            &server
                .handle_line(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#)
                .unwrap(),
        );
        let names = names_of(&response["result"]["tools"]);
        assert!(names.contains(&"list_catalog".to_string()));
        assert!(names.contains(&"list_my_requests".to_string()));
        assert!(!names.contains(&"get_project_graph".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("request_")));
    }

    // list_my_requests over the wire returns ONLY the calling app's own requests, and
    // only the safe loop-status fields. Under total control the app files a request,
    // then reads it back and sees its id/method/status.
    #[cfg(windows)]
    #[test]
    fn list_my_requests_returns_own_requests_only() {
        let state = AppState::memory().unwrap();
        let projects = hangar_api::projects_list(&state).unwrap();
        let (project_id, node_id) = projects
            .iter()
            .find_map(|project| {
                hangar_api::project_context_files(&state, project.id)
                    .ok()
                    .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
            })
            .expect("a project with a context-file node");

        hangar_api::set_mcp_full_control_enabled(&state, true).unwrap();
        hangar_api::start_local_automation(&state).unwrap();
        let credential = hangar_api::automation_register(
            &state,
            "hermes-req".to_string(),
            vec!["read_structure".to_string(), "execute_plan".to_string()],
            vec![project_id],
        )
        .unwrap();
        let server = ConnectedAppServer::new(state, credential.token);

        // Empty to start.
        let empty = parse(
            &server
                .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_my_requests","arguments":{}}}"#)
                .unwrap(),
        );
        assert_eq!(empty["result"]["isError"], false);
        let text = empty["result"]["content"][0]["text"].as_str().unwrap();
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["requests"].as_array().unwrap().len(), 0);

        // File a queue-only backup request, then read it back.
        let file_req = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"request_backup_protected","arguments":{{"nodeId":{node_id},"actionLabel":"backup"}}}}}}"#
        );
        let filed = parse(&server.handle_line(&file_req).unwrap());
        assert_eq!(filed["result"]["isError"], false);

        let listed = parse(
            &server
                .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_my_requests","arguments":{}}}"#)
                .unwrap(),
        );
        let text = listed["result"]["content"][0]["text"].as_str().unwrap();
        let body: Value = serde_json::from_str(text).unwrap();
        let requests = body["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["method"], "backup_protected");
        assert_eq!(requests[0]["status"], "pending");
        assert!(requests[0]["id"].is_number());
        // Only the safe loop-status fields are exposed — no payload/target leakage.
        assert!(requests[0].get("payloadJson").is_none());
        assert!(requests[0].get("targetId").is_none());
    }
}
