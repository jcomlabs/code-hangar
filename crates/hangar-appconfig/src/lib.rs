//! Safe, per-host registration of the `code-hangar` connected-AI-app (MCP) server
//! into each AI app's configuration file.
//!
//! Hosts and formats (verified June 2026):
//! - **Claude** — `~/.claude.json`, JSON, top-level `mcpServers` (global scope).
//! - **Cursor** — `~/.cursor/mcp.json`, JSON, top-level `mcpServers`.
//! - **Codex** — `~/.codex/config.toml`, TOML, `[mcp_servers.code-hangar]`.
//!
//! Every mutating operation follows the same safe pipeline:
//!   1. Parse the existing config. An UNPARSEABLE config aborts that host — it is
//!      never overwritten.
//!   2. Before registration, back up the original file to
//!      `<config><.codehangar.bak>` and verify the copy by re-reading it.
//!   3. Round-trip merge ONLY our `code-hangar` entry, preserving every other key
//!      and (JSON) key order / (TOML) formatting and comments.
//!   4. Write to a sibling temp file, `fsync`, atomically rename over the original,
//!      then re-read and verify the entry is present (register) or gone (unregister).
//!   5. A hash-only state sidecar binds the registered bytes to the original backup.
//!      If the host did not edit its config, unregister restores the original bytes
//!      exactly. If it did, only our entry is removed from the current document.
//!
//! The token and database path live in the host config's `env` in plaintext, so the
//! token is a same-Windows-user secret (documented). Each host gets its own token;
//! revoking removes both the DB credential and this config entry.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use toml_edit::{Array, DocumentMut, Item, Table};

/// The single config key we own across every host. Also a valid TOML bare key.
const SERVER_KEY: &str = "code-hangar";
/// The connected-app server executable's filename. Defined here (a guardrail-exempt
/// crate) so callers can reference it without embedding the literal name elsewhere.
pub const SERVER_EXE_NAME: &str = "code-hangar-mcp.exe";
const BACKUP_SUFFIX: &str = ".codehangar.bak";
const STATE_SUFFIX: &str = ".codehangar.state";
const TEMP_SUFFIX: &str = ".codehangar.tmp";
const STATE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationState {
    schema_version: u8,
    original_exists: bool,
    original_hash: Option<String>,
    registered_hash: String,
}

/// An AI app whose config Code Hangar can register itself into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Claude,
    Cursor,
    Codex,
}

#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
}

impl Host {
    /// The big-three hosts, in presentation order.
    pub const ALL: [Host; 3] = [Host::Claude, Host::Cursor, Host::Codex];

    pub fn id(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Cursor => "cursor",
            Host::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Host::Claude => "Claude",
            Host::Cursor => "Cursor",
            Host::Codex => "ChatGPT",
        }
    }

    pub fn from_id(id: &str) -> Option<Host> {
        Host::ALL.into_iter().find(|host| host.id() == id)
    }

    fn format(self) -> Format {
        match self {
            Host::Codex => Format::Toml,
            _ => Format::Json,
        }
    }

    /// Config path segments relative to the user's home directory.
    fn relative_segments(self) -> &'static [&'static str] {
        match self {
            Host::Claude => &[".claude.json"],
            Host::Cursor => &[".cursor", "mcp.json"],
            Host::Codex => &[".codex", "config.toml"],
        }
    }
}

/// How to launch the server: the absolute exe path, any args, the ordered env
/// (token + db path), and the cold-start timeout Codex needs for the SQLCipher open.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub startup_timeout_sec: u64,
}

/// What we can see about one host's config without modifying it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub host: String,
    pub label: String,
    pub config_path: String,
    /// The config file is present on disk.
    pub config_exists: bool,
    /// The config is absent (we can create it) or parses cleanly. `false` means it
    /// exists but is malformed, so we refuse to touch it.
    pub readable: bool,
    /// Our `code-hangar` entry is present.
    pub registered: bool,
}

/// The absolute config path for a host under the given home directory.
pub fn host_config_path(host: Host, home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    for segment in host.relative_segments() {
        path.push(segment);
    }
    path
}

/// The user's home directory (`%USERPROFILE%` on Windows), if resolvable.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read-only inspection of a host's config.
pub fn status(host: Host, home: &Path) -> HostStatus {
    let path = host_config_path(host, home);
    let exists = path.exists();
    let (readable, registered) = if !exists {
        (true, false)
    } else {
        match host.format() {
            Format::Json => match read_json(&path) {
                Ok(value) => (true, json_registered(&value)),
                Err(_) => (false, false),
            },
            Format::Toml => match read_toml(&path) {
                Ok(doc) => (true, toml_registered(&doc)),
                Err(_) => (false, false),
            },
        }
    };
    HostStatus {
        host: host.id().to_string(),
        label: host.label().to_string(),
        config_path: path.display().to_string(),
        config_exists: exists,
        readable,
        registered,
    }
}

/// Register (or replace) our entry in the host's config.
pub fn register(host: Host, home: &Path, spec: &ServerSpec) -> Result<(), String> {
    let path = host_config_path(host, home);
    let current_bytes = if path.exists() {
        Some(fs::read(&path).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let (already_registered, sanitized_text, registered_text) = match host.format() {
        Format::Json => {
            let mut value = if path.exists() {
                read_json(&path).map_err(|error| unreadable(&path, &error))?
            } else {
                json!({})
            };
            if !value.is_object() {
                return Err(unreadable(
                    &path,
                    "the top-level value is not a JSON object",
                ));
            }
            let already_registered = json_registered(&value);
            let mut sanitized = value.clone();
            json_remove_server(&mut sanitized);
            let mut sanitized_text =
                serde_json::to_string_pretty(&sanitized).map_err(|e| e.to_string())?;
            sanitized_text.push('\n');
            json_set_server(&mut value, spec)?;
            let mut text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            text.push('\n');
            (already_registered, sanitized_text, text)
        }
        Format::Toml => {
            let mut doc = if path.exists() {
                read_toml(&path).map_err(|error| unreadable(&path, &error))?
            } else {
                DocumentMut::new()
            };
            let already_registered = toml_registered(&doc);
            let mut sanitized = doc.clone();
            toml_remove_server(&mut sanitized);
            let sanitized_text = sanitized.to_string();
            toml_set_server(&mut doc, spec)?;
            (already_registered, sanitized_text, doc.to_string())
        }
    };

    let (original_exists, original_hash) = registration_baseline(
        &path,
        current_bytes.as_deref(),
        already_registered,
        sanitized_text.as_bytes(),
    )?;
    let state = RegistrationState {
        schema_version: STATE_SCHEMA_VERSION,
        original_exists,
        original_hash,
        registered_hash: bytes_hash(registered_text.as_bytes()),
    };
    write_registration_state(&path, &state)?;
    if let Err(error) = atomic_write(&path, &registered_text) {
        let _ = remove_registration_state(&path);
        return Err(error);
    }
    if !status(host, home).registered {
        let _ = restore_original_if_unchanged(&path, &state);
        let _ = remove_registration_state(&path);
        return Err("the registration could not be verified after writing".to_string());
    }
    Ok(())
}

/// Remove only our entry, leaving every other server and key untouched. Returns
/// `true` if an entry was actually removed.
pub fn unregister(host: Host, home: &Path) -> Result<bool, String> {
    let path = host_config_path(host, home);
    if !path.exists() {
        return Ok(false);
    }
    let current_bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let updated_text = match host.format() {
        Format::Json => {
            let mut value = read_json(&path).map_err(|error| unreadable(&path, &error))?;
            if !json_registered(&value) {
                return Ok(false);
            }
            json_remove_server(&mut value);
            let mut text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            text.push('\n');
            text
        }
        Format::Toml => {
            let mut doc = read_toml(&path).map_err(|error| unreadable(&path, &error))?;
            if !toml_registered(&doc) {
                return Ok(false);
            }
            toml_remove_server(&mut doc);
            doc.to_string()
        }
    };

    if let Some(state) = read_registration_state(&path).ok().flatten() {
        if restore_original_if_unchanged(&path, &state)? {
            if status(host, home).registered {
                return Err(
                    "the removal could not be verified after restoring the original config"
                        .to_string(),
                );
            }
            remove_registration_state(&path)?;
            return Ok(true);
        }
    }

    atomic_write(&path, &updated_text)?;
    if status(host, home).registered {
        let _ = atomic_write_bytes(&path, &current_bytes);
        return Err("the removal could not be verified after writing".to_string());
    }
    remove_registration_state(&path)?;
    Ok(true)
}

// ---- JSON (Claude, Cursor) -------------------------------------------------

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn json_set_server(value: &mut Value, spec: &ServerSpec) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "the top-level value is not a JSON object".to_string())?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "the existing \"mcpServers\" value is not an object".to_string())?;
    servers.insert(SERVER_KEY.to_string(), json_server_entry(spec));
    Ok(())
}

fn json_server_entry(spec: &ServerSpec) -> Value {
    let mut env = serde_json::Map::new();
    for (key, val) in &spec.env {
        env.insert(key.clone(), Value::String(val.clone()));
    }
    json!({
        "command": spec.command,
        "args": spec.args,
        "env": Value::Object(env),
    })
}

fn json_registered(value: &Value) -> bool {
    value
        .get("mcpServers")
        .and_then(|servers| servers.get(SERVER_KEY))
        .is_some()
}

fn json_remove_server(value: &mut Value) {
    if let Some(servers) = value.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(SERVER_KEY);
    }
}

// ---- TOML (Codex) ----------------------------------------------------------

fn read_toml(path: &Path) -> Result<DocumentMut, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    text.parse::<DocumentMut>().map_err(|e| e.to_string())
}

fn toml_set_server(doc: &mut DocumentMut, spec: &ServerSpec) -> Result<(), String> {
    let servers_item = doc
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()));
    let servers = servers_item
        .as_table_mut()
        .ok_or_else(|| "the existing \"mcp_servers\" value is not a table".to_string())?;
    // Render as [mcp_servers.code-hangar] rather than a standalone [mcp_servers].
    servers.set_implicit(true);

    let mut entry = Table::new();
    entry["command"] = toml_edit::value(spec.command.clone());
    let mut args = Array::new();
    for arg in &spec.args {
        args.push(arg.clone());
    }
    entry["args"] = toml_edit::value(args);
    entry["startup_timeout_sec"] = toml_edit::value(spec.startup_timeout_sec as i64);
    let mut env = Table::new();
    for (key, val) in &spec.env {
        env[key] = toml_edit::value(val.clone());
    }
    entry["env"] = Item::Table(env);

    servers.insert(SERVER_KEY, Item::Table(entry));
    Ok(())
}

fn toml_registered(doc: &DocumentMut) -> bool {
    doc.get("mcp_servers")
        .and_then(Item::as_table)
        .map(|servers| servers.contains_key(SERVER_KEY))
        .unwrap_or(false)
}

fn toml_remove_server(doc: &mut DocumentMut) {
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) {
        servers.remove(SERVER_KEY);
    }
}

// ---- shared safe-write primitives ------------------------------------------

fn unreadable(path: &Path, detail: &str) -> String {
    format!(
        "{} could not be parsed and was left untouched: {detail}",
        path.display()
    )
}

/// Refuse to write through a symlink / reparse point. A same-user attacker could
/// pre-seed a host config path (or its `.bak`/`.tmp` sibling) as a symlink or
/// junction so our write clobbers the link target instead of the intended file;
/// reject any such pre-seeded link rather than following it.
fn reject_reparse_point(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            #[allow(unused_mut)]
            let mut is_link = meta.file_type().is_symlink();
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                is_link = is_link || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            }
            if is_link {
                return Err(format!(
                    "refusing to write through a symlink or reparse point at {}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar: OsString = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn bytes_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn read_registration_state(path: &Path) -> Result<Option<RegistrationState>, String> {
    let state_path = sidecar_path(path, STATE_SUFFIX);
    if !state_path.exists() {
        return Ok(None);
    }
    reject_reparse_point(&state_path)?;
    let text = fs::read_to_string(&state_path).map_err(|error| error.to_string())?;
    let state: RegistrationState = serde_json::from_str(&text).map_err(|error| {
        format!(
            "could not read Code Hangar registration state at {}: {error}",
            state_path.display()
        )
    })?;
    Ok(Some(state))
}

fn write_registration_state(path: &Path, state: &RegistrationState) -> Result<(), String> {
    let state_path = sidecar_path(path, STATE_SUFFIX);
    let text = serde_json::to_string(state).map_err(|error| error.to_string())?;
    atomic_write(&state_path, &text)
}

fn remove_registration_state(path: &Path) -> Result<(), String> {
    let state_path = sidecar_path(path, STATE_SUFFIX);
    reject_reparse_point(&state_path)?;
    match fs::remove_file(&state_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn backup_bytes(path: &Path, original: &[u8]) -> Result<(), String> {
    let mut backup_path: OsString = path.as_os_str().to_os_string();
    backup_path.push(BACKUP_SUFFIX);
    let backup_path = PathBuf::from(backup_path);
    reject_reparse_point(&backup_path)?;
    fs::write(&backup_path, original).map_err(|e| e.to_string())?;
    let reread = fs::read(&backup_path).map_err(|e| e.to_string())?;
    if reread != original {
        return Err(format!(
            "could not verify the backup written to {}",
            backup_path.display()
        ));
    }
    Ok(())
}

/// Copy the existing pre-registration file to `<path><.codehangar.bak>` and
/// verify the copy. This backup never contains the token Code Hangar is about to add.
fn backup(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    reject_reparse_point(path)?;
    let original = fs::read(path).map_err(|e| e.to_string())?;
    backup_bytes(path, &original)
}

fn registration_baseline(
    path: &Path,
    current_bytes: Option<&[u8]>,
    already_registered: bool,
    sanitized_bytes: &[u8],
) -> Result<(bool, Option<String>), String> {
    if already_registered {
        if let Some(state) = read_registration_state(path).ok().flatten() {
            let registered_bytes_are_unchanged = current_bytes
                .map(bytes_hash)
                .is_some_and(|hash| hash == state.registered_hash);
            if state.schema_version == STATE_SCHEMA_VERSION && registered_bytes_are_unchanged {
                if !state.original_exists && state.original_hash.is_none() {
                    return Ok((false, None));
                }
                if let Some(expected_hash) = state.original_hash.as_deref() {
                    let backup_path = sidecar_path(path, BACKUP_SUFFIX);
                    if let Ok(bytes) = fs::read(&backup_path) {
                        if bytes_hash(&bytes) == expected_hash {
                            return Ok((true, Some(expected_hash.to_string())));
                        }
                    }
                }
            }
        }

        // A legacy registration, or a host-edited config, needs a fresh token-free
        // baseline. Reusing the older backup here would discard the host's changes
        // after a token rotation followed by Disconnect.
        backup_bytes(path, sanitized_bytes)?;
        return Ok((true, Some(bytes_hash(sanitized_bytes))));
    }

    if let Some(current_bytes) = current_bytes {
        backup(path)?;
        Ok((true, Some(bytes_hash(current_bytes))))
    } else {
        Ok((false, None))
    }
}

fn restore_original_if_unchanged(path: &Path, state: &RegistrationState) -> Result<bool, String> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Ok(false);
    }
    let current = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if bytes_hash(&current) != state.registered_hash {
        return Ok(false);
    }

    if state.original_exists {
        let Some(expected_hash) = state.original_hash.as_deref() else {
            return Ok(false);
        };
        let backup_path = sidecar_path(path, BACKUP_SUFFIX);
        reject_reparse_point(&backup_path)?;
        let original = match fs::read(&backup_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        if bytes_hash(&original) != expected_hash {
            return Ok(false);
        }
        atomic_write_bytes(path, &original)?;
    } else {
        reject_reparse_point(path)?;
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(true)
}

/// Write to a sibling temp file, fsync, then atomically rename over the target.
fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    atomic_write_bytes(path, contents.as_bytes())
}

fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Never follow a pre-seeded symlink/junction at the final path or the temp.
    reject_reparse_point(path)?;
    let mut temp_path: OsString = path.as_os_str().to_os_string();
    temp_path.push(TEMP_SUFFIX);
    let temp_path = PathBuf::from(temp_path);
    reject_reparse_point(&temp_path)?;
    {
        let mut file = fs::File::create(&temp_path).map_err(|e| e.to_string())?;
        file.write_all(contents).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        error.to_string()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn spec() -> ServerSpec {
        ServerSpec {
            command: r"C:\Apps\code-hangar-mcp.exe".to_string(),
            args: vec![],
            env: vec![
                ("CODEHANGAR_MCP_TOKEN".to_string(), "tok-123".to_string()),
                (
                    "CODEHANGAR_DB_PATH".to_string(),
                    r"C:\Roaming\local.codehangar.desktop\codehangar.sqlite3".to_string(),
                ),
            ],
            startup_timeout_sec: 20,
        }
    }

    #[test]
    fn registers_into_absent_json_then_reports_registered() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        assert!(!status(host, home.path()).registered);
        register(host, home.path(), &spec()).unwrap();

        let st = status(host, home.path());
        assert!(st.config_exists && st.readable && st.registered);

        let value = read_json(&host_config_path(host, home.path())).unwrap();
        let entry = &value["mcpServers"]["code-hangar"];
        assert_eq!(entry["command"], r"C:\Apps\code-hangar-mcp.exe");
        assert_eq!(entry["env"]["CODEHANGAR_MCP_TOKEN"], "tok-123");
    }

    #[test]
    fn register_preserves_other_servers_and_keys_and_backs_up() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        fs::write(
            &path,
            r#"{"numStartups":7,"mcpServers":{"existing":{"command":"other"}}}"#,
        )
        .unwrap();

        register(Host::Claude, home.path(), &spec()).unwrap();

        let value = read_json(&path).unwrap();
        // Our entry was added; the other server and unrelated key survive.
        assert!(value["mcpServers"]["code-hangar"].is_object());
        assert_eq!(value["mcpServers"]["existing"]["command"], "other");
        assert_eq!(value["numStartups"], 7);
        // A verified backup of the original was written.
        let mut bak: OsString = path.as_os_str().to_os_string();
        bak.push(BACKUP_SUFFIX);
        assert!(PathBuf::from(bak).exists());
    }

    #[test]
    fn refuses_to_touch_unparseable_json() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is : not json ").unwrap();

        let error = register(Host::Cursor, home.path(), &spec()).unwrap_err();
        assert!(error.contains("left untouched"));
        // The malformed file is unchanged.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ this is : not json ");
        assert!(!status(Host::Cursor, home.path()).readable);
    }

    #[test]
    fn json_round_trip_register_then_unregister_restores_other_content() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        fs::write(&path, r#"{"mcpServers":{"existing":{"command":"other"}}}"#).unwrap();

        register(Host::Claude, home.path(), &spec()).unwrap();
        assert!(unregister(Host::Claude, home.path()).unwrap());

        let value = read_json(&path).unwrap();
        assert!(value["mcpServers"].get("code-hangar").is_none());
        assert_eq!(value["mcpServers"]["existing"]["command"], "other");
        // A second unregister is a no-op.
        assert!(!unregister(Host::Claude, home.path()).unwrap());
    }

    #[test]
    fn json_round_trip_restores_exact_bytes_without_persisting_the_token() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{\n  \"mcpServers\": {}\n}";
        fs::write(&path, original).unwrap();

        register(Host::Cursor, home.path(), &spec()).unwrap();

        let state_path = sidecar_path(&path, STATE_SUFFIX);
        let state_text = fs::read_to_string(&state_path).unwrap();
        assert!(!state_text.contains("tok-123"));
        assert!(!state_text.contains("code-hangar-mcp"));
        let backup_path = sidecar_path(&path, BACKUP_SUFFIX);
        assert_eq!(fs::read(&backup_path).unwrap(), original);

        assert!(unregister(Host::Cursor, home.path()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup_path).unwrap(), original);
        assert!(!state_path.exists());
    }

    #[test]
    fn absent_config_is_removed_again_after_an_unchanged_round_trip() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());

        register(Host::Cursor, home.path(), &spec()).unwrap();
        assert!(path.exists());
        assert!(unregister(Host::Cursor, home.path()).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn unregister_preserves_host_changes_made_while_connected() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        let original = br#"{"mcpServers":{"existing":{"command":"other"}}}"#;
        fs::write(&path, original).unwrap();
        register(Host::Claude, home.path(), &spec()).unwrap();

        let mut changed = read_json(&path).unwrap();
        changed["theme"] = json!("dark");
        fs::write(&path, serde_json::to_string_pretty(&changed).unwrap()).unwrap();

        assert!(unregister(Host::Claude, home.path()).unwrap());
        let after = read_json(&path).unwrap();
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["mcpServers"]["existing"]["command"], "other");
        assert!(after["mcpServers"].get(SERVER_KEY).is_none());
        assert_eq!(
            fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap(),
            original
        );
        assert!(!sidecar_path(&path, STATE_SUFFIX).exists());
    }

    #[test]
    fn reconnect_rotates_the_entry_without_overwriting_the_original_backup() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = br#"{"mcpServers":{},"keep":true}"#;
        fs::write(&path, original).unwrap();

        register(Host::Cursor, home.path(), &spec()).unwrap();
        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        register(Host::Cursor, home.path(), &rotated).unwrap();

        assert_eq!(
            fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap(),
            original
        );
        assert!(unregister(Host::Cursor, home.path()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn reconnect_keeps_host_changes_made_after_the_original_registration() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"mcpServers":{},"keep":true}"#).unwrap();

        register(Host::Cursor, home.path(), &spec()).unwrap();
        let mut changed = read_json(&path).unwrap();
        changed["theme"] = json!("dark");
        fs::write(&path, serde_json::to_string_pretty(&changed).unwrap()).unwrap();

        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        register(Host::Cursor, home.path(), &rotated).unwrap();
        assert!(unregister(Host::Cursor, home.path()).unwrap());

        let after = read_json(&path).unwrap();
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["keep"], true);
        assert!(after["mcpServers"].get(SERVER_KEY).is_none());
        let backup = fs::read_to_string(sidecar_path(&path, BACKUP_SUFFIX)).unwrap();
        assert!(!backup.contains("tok-123"));
        assert!(!backup.contains("tok-456"));
    }

    #[test]
    fn registers_into_codex_toml_and_preserves_comments() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Codex, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "# my codex config\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();

        register(Host::Codex, home.path(), &spec()).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my codex config"));
        assert!(text.contains("model = \"o3\""));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains("[mcp_servers.code-hangar]"));
        assert!(text.contains("[mcp_servers.code-hangar.env]"));
        assert!(text.contains("startup_timeout_sec = 20"));

        let doc = read_toml(&path).unwrap();
        assert!(toml_registered(&doc));

        assert!(unregister(Host::Codex, home.path()).unwrap());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mcp_servers.other]"));
        assert!(!text.contains("code-hangar"));
    }

    #[test]
    fn status_of_absent_config_is_readable_but_unregistered() {
        let home = tempdir().unwrap();
        let st = status(Host::Codex, home.path());
        assert_eq!(st.label, "ChatGPT");
        assert!(!st.config_exists);
        assert!(st.readable);
        assert!(!st.registered);
    }
}
