//! `code-hangar-mcp` — the standalone connected-AI-app server.
//!
//! An AI app (Claude, Cursor, Codex, …) launches this as a child process and
//! speaks the Model Context Protocol over stdin/stdout. The process:
//!   1. requires its per-app token in `CODEHANGAR_MCP_TOKEN` (fails closed otherwise);
//!      consumes it once to bind the exact `CODEHANGAR_MCP_HOST` and immutable
//!      `CODEHANGAR_MCP_AGENT_ID`, then carries no token in tool request bodies;
//!   2. opens the SAME encrypted inventory the desktop app uses — by default
//!      `%APPDATA%\local.codehangar.desktop\codehangar.sqlite3`, or the explicit
//!      `CODEHANGAR_DB_PATH` if Code Hangar pins one. The DPAPI-wrapped key binds
//!      the database to this Windows user, so another user (or another machine)
//!      fails to open it;
//!   3. serves MCP over stdio via `hangar-mcp`, whose every call runs through
//!      hangar-api's authenticated, scope/project-gated, audited dispatch.
//!
//! stdout is reserved for the JSON-RPC stream. Every diagnostic goes to stderr.

#[cfg(not(windows))]
compile_error!(
    "code-hangar-mcp is a Windows-only Connector sidecar; the Local/base build never links it."
);

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hangar_api::AppState;
use hangar_mcp::ConnectedAppServer;

/// How long to wait for the cold SQLCipher open before giving up. The Codex host
/// config grants a comparable startup timeout.
const DB_READY_TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    if let Err(error) = run() {
        eprintln!("code-hangar-mcp: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // The per-app token is mandatory: without it there is nothing to authenticate
    // and the server must not start.
    let token = std::env::var("CODEHANGAR_MCP_TOKEN")
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            "CODEHANGAR_MCP_TOKEN is not set. Code Hangar launches this server with its \
             per-app token; do not run it by hand."
                .to_string()
        })?;
    let host_id = required_env("CODEHANGAR_MCP_HOST")?;
    let agent_identity_id = required_env("CODEHANGAR_MCP_AGENT_ID")?;

    let db_path = resolve_db_path_from(
        std::env::var_os("CODEHANGAR_DB_PATH"),
        std::env::var_os("APPDATA"),
    )?;
    eprintln!(
        "code-hangar-mcp: opening encrypted inventory at {}",
        db_path.display()
    );

    let state = AppState::open(&db_path)?;
    wait_for_db_ready(&state, DB_READY_TIMEOUT)?;

    eprintln!("code-hangar-mcp: ready; serving MCP over stdio.");
    ConnectedAppServer::bind(state, token, host_id, agent_identity_id)?
        .run_stdio()
        .map_err(|error| format!("stdio transport failed: {error}"))
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is not set. Reconnect this app in Code Hangar."))
}

/// Resolve the inventory path: an explicit `CODEHANGAR_DB_PATH` wins (Code Hangar
/// pins the exact file it opened); otherwise fall back to the desktop app's
/// `%APPDATA%\local.codehangar.desktop\codehangar.sqlite3`.
fn resolve_db_path_from(
    explicit: Option<OsString>,
    appdata: Option<OsString>,
) -> Result<PathBuf, String> {
    if let Some(explicit) = explicit {
        let path = PathBuf::from(explicit);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    let appdata = appdata.filter(|value| !value.is_empty()).ok_or_else(|| {
        "APPDATA is not set; cannot locate the Code Hangar inventory. Set CODEHANGAR_DB_PATH."
            .to_string()
    })?;
    Ok(PathBuf::from(appdata)
        .join("local.codehangar.desktop")
        .join("codehangar.sqlite3"))
}

/// Block until the asynchronously-opened database reports ready, or fail with the
/// open error / a timeout. A "failed" status aborts immediately; "starting" is
/// retried until the deadline.
fn wait_for_db_ready(state: &AppState, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = hangar_api::startup_status(state);
        match status.state.as_str() {
            "ready" => return Ok(()),
            "failed" => {
                return Err(format!(
                    "could not open the encrypted inventory: {}",
                    status.message
                ))
            }
            _ if Instant::now() >= deadline => {
                return Err(format!(
                    "the encrypted inventory did not become ready within {} seconds.",
                    timeout.as_secs()
                ))
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_db_path_wins() {
        let path = resolve_db_path_from(
            Some(OsString::from(r"D:\custom\hangar.sqlite3")),
            Some(OsString::from(r"C:\Users\someone\AppData\Roaming")),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from(r"D:\custom\hangar.sqlite3"));
    }

    #[test]
    fn falls_back_to_appdata_layout() {
        let path = resolve_db_path_from(
            None,
            Some(OsString::from(r"C:\Users\someone\AppData\Roaming")),
        )
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\someone\AppData\Roaming")
                .join("local.codehangar.desktop")
                .join("codehangar.sqlite3")
        );
    }

    #[test]
    fn empty_explicit_path_is_ignored() {
        let path = resolve_db_path_from(Some(OsString::new()), Some(OsString::from(r"C:\Roaming")))
            .unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\Roaming")
                .join("local.codehangar.desktop")
                .join("codehangar.sqlite3")
        );
    }

    #[test]
    fn missing_appdata_without_explicit_is_an_error() {
        assert!(resolve_db_path_from(None, None).is_err());
    }
}
