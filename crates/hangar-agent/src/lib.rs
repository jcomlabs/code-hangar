#![cfg_attr(not(feature = "agent_automation"), allow(dead_code))]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub const PROTOCOL_VERSION: &str = "codehangar-agent/1";
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Most concurrently-served local pipe clients. There is realistically a single
/// legitimate peer; capping live clients stops a malicious same-user process from
/// flooding the accept loop with connections and exhausting threads/memory (each
/// client thread reserves a `MAX_REQUEST_BYTES` buffer). Excess connections are
/// refused immediately rather than queued.
pub const MAX_LIVE_PIPE_CLIENTS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMethod {
    Status,
    AgentProjectContext,
    AgentReadBody,
    AgentPlanBuild,
    AgentPlanExecute,
    DeepHistorySearch,
    /// List the comments attached to a node (project/folder/file). Read-only;
    /// needs the `comments_read` scope and project membership.
    CommentsList,
    /// Add a comment authored by the connected AI app itself. The server assigns
    /// the author/source from the authenticated agent (never client-supplied), so
    /// an app can never spoof a human ("user") record. Needs the `comments_write`
    /// scope, project membership, and the global AI-write toggle.
    CommentsAdd,
    /// Edit a comment the connected AI app authored itself. The human/AI boundary
    /// (`guard_comment_actor`) refuses any edit of a human or another agent's
    /// comment. Needs the `comments_write` scope, project membership, and the
    /// global AI-write toggle.
    CommentsEdit,
    /// REQUEST that a human comment be edited or deleted on the app's behalf. The
    /// agent never performs this; it files a pending request that the user must
    /// approve in Code Hangar (which then acts AS the user, with an optional
    /// backup). Available only under the default-off "total control" tier.
    RequestCommentChange,
    // ---- Discovery surface: pre-cooked, read-only, body-free functions so an app
    // can learn a project's main functionalities. Each is scope- AND project-gated
    // in hangar-api; nothing here exposes file/session contents.
    /// List the projects this app is scoped to (id, name, path, app badge, scan
    /// state) — server-side intersected with the agent's grants, never the full
    /// inventory. Needs `read_structure`.
    ListCatalog,
    /// List a project's curated context files (names/paths only, no bodies). Needs
    /// `read_structure` + project membership.
    ListContextFiles,
    /// Page through a project's navigation tree (folders/files, sizes, flags — no
    /// bodies). Needs `read_structure` + project membership.
    ListProjectNav,
    /// Explain what a folder is (role, signals). Needs `read_structure`; the handler
    /// resolves the folder's project and enforces membership before returning.
    ExplainFolder,
    /// The dependency-graph map for a project (nodes + edges + issues, no bodies).
    /// Needs `read_graph` + project membership.
    GetProjectGraph,
    /// A node's incoming/outgoing relationships (cross-project edges are dropped).
    /// Needs `read_graph` + project membership.
    NodeRelationships,
    /// Candidate orphan assets within a project. Needs `read_graph` + membership.
    ListOrphanAssets,
    /// Whether a node looks orphaned (and why). Needs `read_graph` + membership.
    NodeOrphanStatus,
    /// Likely-duplicate file groups within a project; member rows from un-granted
    /// projects are dropped. Needs `read_graph` + project membership.
    ListDuplicateCandidates,
    /// Full-hash confirmation of a node's duplicate group; member rows from
    /// un-granted projects are dropped. Needs `read_graph` + project membership.
    ConfirmDuplicateGroup,
    /// A project's Git metadata (branch, head, origin — no working-tree contents).
    /// Needs `read_structure` + project membership.
    ProjectGitStatus,
    /// The AI-app adapters Code Hangar understands (static capability metadata, not
    /// user data). Needs `read_structure`.
    ListAdapters,
    /// List the CALLING app's OWN queued/approved/denied requests (id, method,
    /// status, created_at, resolved_at). Read-only and strictly own-app-scoped: it
    /// NEVER returns another app's requests or any other app's data. Needs a base
    /// read scope (`read_structure`) so the total-control request loop is observable
    /// — an app can finally see what became of a request it filed. Not a write (it
    /// only SELECTs), so the read-only panic switch lets it through.
    ListMyRequests,
    // ---- Total-control request kinds. Each only FILES a pending request (under the
    // default-off total-control tier); a human approves and the app performs it AS the
    // user via the Gate-3 executors. The agent never executes. Targets outside the
    // agent's grants are allowed but flagged cross-scope for an extra approval step.
    /// Request a per-file read grant for a node the agent lacks the standing
    /// `read_body` scope for. Approval mints a short-lived per-node grant; no mutation.
    RequestReadBody,
    /// Request a verified backup that INCLUDES protected/sensitive files
    /// (`include_protected=true`). Reversible (copies, never deletes), but behind the
    /// strengthened six-friction gate because it copies secret bytes.
    RequestBackupProtected,
    /// Request moving a target to the holding area (reversible-to-holding). Approval
    /// requires a verified backup covering every file; behind the strengthened gate.
    RequestMoveToHolding,
    /// Request the irreversible permanent removal of a prior holding-area entry.
    /// Highest-friction gate + the runtime final-remove opt-in.
    RequestPermanentDelete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequest {
    pub protocol: String,
    pub request_id: String,
    pub token: Option<String>,
    pub method: AgentMethod,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResponse {
    pub protocol: String,
    pub request_id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl AgentResponse {
    pub fn success(request_id: impl Into<String>, result: Value) -> Self {
        Self {
            protocol: PROTOCOL_VERSION.to_string(),
            request_id: request_id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL_VERSION.to_string(),
            request_id: request_id.into(),
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }
}

pub type RequestHandler = Arc<dyn Fn(AgentRequest) -> AgentResponse + Send + Sync + 'static>;

pub fn random_token(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0_u8; bytes];
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(format!(
                "Windows secure random generation failed with status {status}."
            ));
        }
    }
    #[cfg(not(windows))]
    {
        return Err("Local agent tokens are supported only on Windows.".to_string());
    }

    let mut encoded = String::with_capacity(buffer.len() * 2);
    for byte in buffer {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(encoded)
}

#[cfg(all(windows, feature = "agent_automation"))]
pub struct LocalAgentServer {
    endpoint: String,
}

#[cfg(all(windows, feature = "agent_automation"))]
impl LocalAgentServer {
    pub fn start(endpoint_id: &str, handler: RequestHandler) -> Result<Self, String> {
        let safe_id = endpoint_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect::<String>();
        if safe_id.len() < 16 {
            return Err("Local agent endpoint id is invalid.".to_string());
        }
        let endpoint = format!(r"\\.\pipe\codehangar-{safe_id}");
        let thread_endpoint = endpoint.clone();
        std::thread::Builder::new()
            .name("codehangar-local-agent".to_string())
            .spawn(move || serve_named_pipe(&thread_endpoint, handler))
            .map_err(|error| format!("Could not start local agent server: {error}"))?;
        Ok(Self { endpoint })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Releases a reserved pipe-client slot on drop, so the live-client count is
/// decremented on EVERY exit of the client thread — normal return, panic unwind
/// (a panic would skip a manual decrement and permanently leak the slot), or, if
/// the thread never starts, when the closure that owns this guard is dropped on
/// spawn failure.
#[cfg(all(windows, feature = "agent_automation"))]
struct PipeClientSlot(Arc<std::sync::atomic::AtomicUsize>);

#[cfg(all(windows, feature = "agent_automation"))]
impl Drop for PipeClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(all(windows, feature = "agent_automation"))]
fn serve_named_pipe(endpoint: &str, handler: RequestHandler) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    use std::sync::atomic::{AtomicUsize, Ordering};

    let wide = std::ffi::OsStr::new(endpoint)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    // Bound the number of live client threads (and their per-thread 1 MiB buffers)
    // so a connection flood from a malicious same-user peer cannot exhaust threads
    // or memory. The count is incremented when a slot is reserved and decremented
    // when the client thread finishes (or on a spawn failure).
    let live_clients = Arc::new(AtomicUsize::new(0));

    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                MAX_RESPONSE_BYTES as u32,
                MAX_REQUEST_BYTES as u32,
                0,
                std::ptr::null(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return;
        }

        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            // Reserve a slot. fetch_add returns the prior count; if we were already at
            // the cap, release the reservation and refuse this client immediately
            // instead of spawning an unbounded thread.
            if live_clients.fetch_add(1, Ordering::SeqCst) >= MAX_LIVE_PIPE_CLIENTS {
                live_clients.fetch_sub(1, Ordering::SeqCst);
                unsafe {
                    DisconnectNamedPipe(pipe);
                    CloseHandle(pipe);
                }
                continue;
            }
            // This guard releases the reserved slot on any thread exit (return,
            // panic) and — because the closure owns it — also when the closure is
            // dropped on a spawn failure. No manual decrement is needed anywhere.
            let slot = PipeClientSlot(Arc::clone(&live_clients));
            let client_handler = Arc::clone(&handler);
            let pipe_value = pipe as usize;
            if std::thread::Builder::new()
                .name("codehangar-local-agent-client".to_string())
                .spawn(move || {
                    let _slot = slot;
                    let pipe = pipe_value as windows_sys::Win32::Foundation::HANDLE;
                    handle_pipe_client(pipe, &client_handler);
                    unsafe {
                        DisconnectNamedPipe(pipe);
                        CloseHandle(pipe);
                    }
                })
                .is_err()
            {
                // The thread never started; the dropped closure already released the
                // slot via its guard. Just clean up the pipe handle.
                unsafe {
                    DisconnectNamedPipe(pipe);
                    CloseHandle(pipe);
                }
            }
        } else {
            unsafe { CloseHandle(pipe) };
        }
    }
}

#[cfg(all(windows, feature = "agent_automation"))]
fn handle_pipe_client(pipe: windows_sys::Win32::Foundation::HANDLE, handler: &RequestHandler) {
    use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile};

    let mut request_buffer = vec![0_u8; MAX_REQUEST_BYTES];
    let mut read = 0_u32;
    let read_ok = unsafe {
        ReadFile(
            pipe,
            request_buffer.as_mut_ptr(),
            request_buffer.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    } != 0;
    let response = if !read_ok || read == 0 {
        AgentResponse::failure("unknown", "The local request was empty or unreadable.")
    } else {
        request_buffer.truncate(read as usize);
        match serde_json::from_slice::<AgentRequest>(&request_buffer) {
            Ok(request) if request.protocol == PROTOCOL_VERSION => handler(request),
            Ok(request) => AgentResponse::failure(
                request.request_id,
                format!("Unsupported protocol. Expected {PROTOCOL_VERSION}."),
            ),
            Err(error) => {
                AgentResponse::failure("unknown", format!("Invalid request JSON: {error}"))
            }
        }
    };

    let encoded = serde_json::to_vec(&response).unwrap_or_else(|error| {
        format!(
            r#"{{"protocol":"{PROTOCOL_VERSION}","requestId":"unknown","ok":false,"error":"Response encoding failed: {error}"}}"#
        )
        .into_bytes()
    });
    if encoded.len() > MAX_RESPONSE_BYTES {
        return;
    }
    let mut written = 0_u32;
    let write_ok = unsafe {
        WriteFile(
            pipe,
            encoded.as_ptr(),
            encoded.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    } != 0;
    if write_ok && written as usize == encoded.len() {
        unsafe { FlushFileBuffers(pipe) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex_and_not_reused() {
        let first = random_token(32).unwrap();
        let second = random_token(32).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn protocol_round_trip_preserves_method_and_request_id() {
        let request = AgentRequest {
            protocol: PROTOCOL_VERSION.to_string(),
            request_id: "request-1".to_string(),
            token: None,
            method: AgentMethod::Status,
            params: Value::Null,
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: AgentRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.request_id, "request-1");
        assert_eq!(decoded.method, AgentMethod::Status);
    }

    #[cfg(all(windows, feature = "agent_automation"))]
    #[test]
    fn named_pipe_round_trip_is_local_and_message_bounded() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

        let endpoint_id = random_token(16).unwrap();
        let handler: RequestHandler = Arc::new(|request| {
            AgentResponse::success(request.request_id, serde_json::json!({ "local": true }))
        });
        let server = LocalAgentServer::start(&endpoint_id, handler).unwrap();
        let endpoint = std::ffi::OsStr::new(server.endpoint())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        for request_id in ["pipe-test-1", "pipe-test-2"] {
            let mut ready = false;
            for _ in 0..250 {
                if unsafe { WaitNamedPipeW(endpoint.as_ptr(), 20) } != 0 {
                    ready = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(ready, "local named pipe did not become ready");
            let pipe = unsafe {
                CreateFileW(
                    endpoint.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(pipe, INVALID_HANDLE_VALUE);
            let request = AgentRequest {
                protocol: PROTOCOL_VERSION.to_string(),
                request_id: request_id.to_string(),
                token: None,
                method: AgentMethod::Status,
                params: Value::Null,
            };
            let encoded = serde_json::to_vec(&request).unwrap();
            let mut written = 0_u32;
            assert_ne!(
                unsafe {
                    WriteFile(
                        pipe,
                        encoded.as_ptr(),
                        encoded.len() as u32,
                        &mut written,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let mut response = vec![0_u8; 4096];
            let mut read = 0_u32;
            assert_ne!(
                unsafe {
                    ReadFile(
                        pipe,
                        response.as_mut_ptr(),
                        response.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            unsafe { CloseHandle(pipe) };
            response.truncate(read as usize);
            let decoded: AgentResponse = serde_json::from_slice(&response).unwrap();
            assert!(decoded.ok);
            assert_eq!(decoded.request_id, request_id);
            assert_eq!(decoded.result.unwrap()["local"], true);
        }
    }
}
