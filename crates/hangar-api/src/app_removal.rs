//! Reversible removal of a project's registration from the AI apps that track it.
//!
//! Part of the "remove project everywhere" feature: making a project disappear from
//! the AI apps' OWN records (Antigravity/Cursor/Hermes/…), not just from Code Hangar.
//! Every removal records exactly what it changed so it can always be restored — either
//! immediately (the in-session Undo) or later from the Recover view (the persisted
//! manifest). Feature-gated behind `mutation`; never compiled into the strict `core`
//! lane.
//!
//! Each app needs a different reversible primitive, so a removal is a list of records of
//! six kinds:
//!
//! - `file`: a per-project registry FILE backed up then deleted (Antigravity, single-root file).
//! - `dir`: a per-project registry FOLDER backed up then deleted (Cursor workspace).
//! - `json_key`: one key surgically removed from a SHARED json file; restore re-inserts exactly that key/value (Cursor `storage.json`, Claude `~/.claude.json`).
//! - `json_array_item`: one element surgically removed from a SHARED json array; restore re-appends exactly it (an Antigravity per-project file that ALSO registers sibling roots — only the target's `projectResources.resources[]` entry is removed so the siblings stay registered).
//! - `toml_table`: one `[projects.'<path>']` table removed from a SHARED toml file; restore re-inserts exactly it (Codex `config.toml`).
//! - `db_rows`: rows deleted from a SHARED sqlite table; restore re-INSERTs exactly those rows (Hermes `state.db`).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn kind_file() -> String {
    "file".to_string()
}

/// One reversible change made while removing a project from an AI app. `kind` selects how
/// [`restore_app_removal`] puts it back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppRemovalRecord {
    /// The AI app slug, e.g. `"antigravity"` / `"cursor"` / `"hermes"`.
    pub app: String,
    /// The file/folder/db this record concerns (where the restore writes).
    pub original_path: String,
    /// Where the verified backup copy lives (`file`/`dir` kinds); empty otherwise.
    #[serde(default)]
    pub backup_path: String,
    /// `"file"` | `"dir"` | `"json_key"` | `"db_rows"`.
    #[serde(default = "kind_file")]
    pub kind: String,
    /// `json_key`: JSON pointer to the object the key was removed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
    /// `json_key`: the removed key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_key: Option<String>,
    /// `json_key`: the removed value, re-inserted on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_value: Option<Value>,
    /// `db_rows`: the sqlite table the rows came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_table: Option<String>,
    /// `db_rows`: JSON array of the deleted rows (col -> value), re-INSERTed on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_rows_json: Option<String>,
    /// Set true once this record has been restored, so it is not restored (and clobbered) twice.
    #[serde(default)]
    pub restored: bool,
}

// ----- small helpers -------------------------------------------------------------------

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Lowercased, backslash-separated, trailing-slash-trimmed — for tolerant path comparison
/// across drive-letter case, forward/back slashes and WSL paths.
fn norm_path(path: &Path) -> String {
    path.to_string_lossy()
        .to_lowercase()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string()
}

/// Resolve the deepest existing ancestor, then re-attach any missing tail. This keeps
/// containment comparisons correct on Windows runners where TEMP may use an 8.3 alias
/// (`RUNNER~1`) while USERPROFILE uses the long spelling. Resolving the ancestor also
/// prevents an in-profile junction from disguising an out-of-profile restore target.
fn canonicalize_with_missing_tail(path: &Path) -> PathBuf {
    let mut cursor = path;
    let mut missing: Vec<OsString> = Vec::new();

    while !cursor.exists() {
        let Some(name) = cursor.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent;
    }

    let Ok(mut resolved) = fs::canonicalize(cursor) else {
        return path.to_path_buf();
    };
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    resolved
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode `%XX` percent-escapes (used in Cursor/VS Code `file://` URIs).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Turn a `file:///c%3A/AI/Proj` URI into a Windows path.
fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))?;
    Some(PathBuf::from(percent_decode(rest).replace('/', "\\")))
}

/// User-writable config roots a restore is allowed to write under. Combined with the
/// per-app `.gemini`/`.cursor`/`.hermes` segment check this blocks a tampered manifest from
/// redirecting a restore to a system/program directory.
fn is_managed_registry_path(path: &Path) -> bool {
    let target = norm_path(&canonicalize_with_missing_tail(path));
    if target.is_empty() {
        return false;
    }
    let mut roots: Vec<String> = Vec::new();
    for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "HOME"] {
        if let Ok(value) = std::env::var(key) {
            roots.push(norm_path(&canonicalize_with_missing_tail(Path::new(
                &value,
            ))));
        }
    }
    let under_user_area = roots.iter().any(|root| {
        !root.is_empty() && (target == *root || target.starts_with(&format!("{root}\\")))
    });
    // Dotted per-app config dirs are an escape hatch ONLY for WSL/UNC paths that the
    // under_user_area roots cannot cover (e.g. \\wsl.localhost\...\.hermes\). It is anchored to
    // a WSL UNC prefix so it can never accept a non-WSL out-of-user-area path like
    // C:\somewhere\.codex\ — the real %APPDATA%\Cursor etc. are already under_user_area.
    let is_wsl_unc = target.starts_with("\\\\wsl.localhost\\") || target.starts_with("\\\\wsl$\\");
    let managed_wsl_segment = is_wsl_unc
        && ["\\.gemini\\", "\\.cursor\\", "\\.hermes\\", "\\.codex\\"]
            .iter()
            .any(|seg| target.contains(seg));
    under_user_area || managed_wsl_segment
}

fn is_safe_sql_ident(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn sqlite_value_to_json(value: &rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match value {
        V::Null => Value::Null,
        V::Integer(i) => Value::from(*i),
        V::Real(f) => Value::from(*f),
        V::Text(s) => Value::from(s.clone()),
        V::Blob(b) => Value::from(b.clone()),
    }
}

fn json_to_sqlite_value(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match value {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                V::Integer(i)
            } else if let Some(f) = n.as_f64() {
                V::Real(f)
            } else {
                V::Null
            }
        }
        Value::String(s) => V::Text(s.clone()),
        Value::Array(items) => {
            // A blob is serialized as an array of byte numbers.
            let bytes: Option<Vec<u8>> = items
                .iter()
                .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect();
            match bytes {
                Some(b) => V::Blob(b),
                None => V::Text(value.to_string()),
            }
        }
        other => V::Text(other.to_string()),
    }
}

/// Atomic write: temp file in the same dir then rename, so a crash never leaves a torn file.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Could not create {}: {err}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", nanos()));
    fs::write(&tmp, bytes).map_err(|err| format!("Could not write temp file: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("Could not finalize {}: {err}", path.display())
    })?;
    Ok(())
}

/// Atomic write that ABORTS if the file changed since `expected` was read — a best-effort
/// compare-and-swap for a surgical edit of a SHARED, actively-written config (Cursor
/// `storage.json`, the running Claude's `~/.claude.json`). On a detected change it returns an
/// error to retry, having changed nothing.
///
/// LIMITATION: the read-compare and the rename are separate syscalls with no lock between
/// them, and Windows `rename` has no content compare-and-swap, so this NARROWS but does not
/// fully CLOSE the window — a concurrent app write landing strictly between our compare and
/// our rename can still be overwritten. It is the common-case guard (the app wrote before our
/// read → we abort cleanly), not an absolute no-clobber guarantee.
fn atomic_write_cas(path: &Path, bytes: &[u8], expected: &[u8]) -> Result<(), String> {
    match fs::read(path) {
        Ok(current) if current == expected => atomic_write(path, bytes),
        Ok(_) => Err(format!(
            "{} changed while we were editing it (the app may be running). Nothing was changed — close the app or try again.",
            path.display()
        )),
        Err(err) => Err(format!("Could not re-read {} before writing: {err}", path.display())),
    }
}

/// Refuse to act through a symlink / reparse point so a pre-seeded link can't redirect
/// our read/write/delete to another target.
fn ensure_not_reparse(path: &Path) -> Result<(), String> {
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
                    "Refusing to act through a symlink or reparse point at {}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Refuse any existing symlink/reparse component in the full path. Checking only the leaf
/// is not enough for a restore: a tampered recovery manifest could point through a parent
/// junction and redirect an otherwise user-profile-looking path outside the managed area.
fn ensure_no_reparse_components(path: &Path) -> Result<(), String> {
    for component in path.ancestors() {
        ensure_not_reparse(component)?;
    }
    Ok(())
}

fn normalized_path_is_within(path: &Path, root: &Path) -> bool {
    let path = norm_path(path);
    let root = norm_path(root);
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}\\")))
}

// ----- backup primitives ---------------------------------------------------------------

/// A collision-proof backup path: `<app>-<nanos>-<name>` inside `backup_dir`.
fn unique_backup_path(app: &str, name: &str, backup_dir: &Path) -> PathBuf {
    backup_dir.join(format!("{app}-{}-{name}", nanos()))
}

/// Back up a single FILE (verified byte-for-byte), then delete the original. A failure at
/// any point leaves the original intact.
#[cfg(test)]
fn backup_and_remove_file(
    registry_file: &Path,
    app: &str,
    backup_dir: &Path,
) -> Result<AppRemovalRecord, String> {
    let original = fs::read(registry_file)
        .map_err(|err| format!("Could not read {}: {err}", registry_file.display()))?;
    backup_and_remove_file_expected(registry_file, app, backup_dir, &original)
}

fn backup_and_remove_file_expected(
    registry_file: &Path,
    app: &str,
    backup_dir: &Path,
    original: &[u8],
) -> Result<AppRemovalRecord, String> {
    // Full-ancestor reparse check (matching backup_and_remove_dir + restore): refuse if ANY
    // path component is a symlink/junction, so a redirected ancestor can't make us delete a
    // file outside the intended registry location. The leaf-only check missed that.
    ensure_no_reparse_components(registry_file)?;
    fs::create_dir_all(backup_dir)
        .map_err(|err| format!("Could not create the backup folder: {err}"))?;
    let file_name = registry_file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The file to back up has no name.".to_string())?;
    let backup_path = unique_backup_path(app, file_name, backup_dir);
    ensure_not_reparse(&backup_path)?;

    fs::write(&backup_path, original)
        .map_err(|err| format!("Could not write the backup: {err}"))?;
    let reread =
        fs::read(&backup_path).map_err(|err| format!("Could not verify the backup: {err}"))?;
    if reread != original {
        let _ = fs::remove_file(&backup_path);
        return Err("The backup did not verify.".to_string());
    }
    match fs::read(registry_file) {
        Ok(current) if current == original => {}
        Ok(_) => {
            let _ = fs::remove_file(&backup_path);
            return Err(format!(
                "{} changed while it was being backed up. Nothing was removed — close the app or try again.",
                registry_file.display()
            ));
        }
        Err(err) => {
            let _ = fs::remove_file(&backup_path);
            return Err(format!(
                "Could not re-read {} before removing it: {err}",
                registry_file.display()
            ));
        }
    }
    fs::remove_file(registry_file)
        .map_err(|err| format!("Could not remove the registry file: {err}"))?;
    Ok(AppRemovalRecord {
        app: app.to_string(),
        original_path: registry_file.display().to_string(),
        backup_path: backup_path.display().to_string(),
        kind: "file".to_string(),
        json_pointer: None,
        json_key: None,
        json_value: None,
        db_table: None,
        db_rows_json: None,
        restored: false,
    })
}

/// Move a whole registry folder atomically into the managed recovery area. Used for
/// Cursor's per-workspace folder, which is one project's registration. A same-volume
/// rename preserves the exact tree and either removes the registration in one step or
/// leaves it untouched; unlike copy-then-remove it cannot partially delete a live folder.
fn backup_and_remove_dir(
    dir: &Path,
    app: &str,
    backup_dir: &Path,
) -> Result<AppRemovalRecord, String> {
    ensure_no_reparse_components(dir)?;
    fs::create_dir_all(backup_dir)
        .map_err(|err| format!("Could not create the backup folder: {err}"))?;
    ensure_no_reparse_components(backup_dir)?;
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The folder to back up has no name.".to_string())?;
    let backup_path = unique_backup_path(app, name, backup_dir);
    if backup_path.exists() {
        return Err("A backup with that name already exists.".to_string());
    }
    fs::rename(dir, &backup_path).map_err(|err| {
        format!(
            "Could not move the registry folder into the recovery area; nothing was removed. \
             The registry and recovery folders must be on the same disk and the AI app must \
             not be locking the folder: {err}"
        )
    })?;
    Ok(AppRemovalRecord {
        app: app.to_string(),
        original_path: dir.display().to_string(),
        backup_path: backup_path.display().to_string(),
        kind: "dir".to_string(),
        json_pointer: None,
        json_key: None,
        json_value: None,
        db_table: None,
        db_rows_json: None,
        restored: false,
    })
}

// ----- Antigravity ---------------------------------------------------------------------

/// True when a `projectResources.resources[]` entry registers `target` as its OWN root. We
/// match on the entry's primary `folderUri` (the root that entry registers); only when there
/// is no `folderUri` do we fall back to the nested `gitFolder.folderUri`. This deliberately
/// will NOT remove an entry whose primary root is a SIBLING even if its git folder happens to
/// be the target — under-removing is safe (the target stays listed), over-removing would
/// silently de-register a sibling.
fn antigravity_resource_matches(resource: &Value, target: &str) -> bool {
    if let Some(folder) = resource.get("folderUri").and_then(|v| v.as_str()) {
        return decode_file_uri(folder)
            .map(|p| norm_path(&p))
            .map(|n| !n.is_empty() && n == target)
            .unwrap_or(false);
    }
    if let Some(git) = resource
        .get("gitFolder")
        .and_then(|g| g.get("folderUri"))
        .and_then(|v| v.as_str())
    {
        return decode_file_uri(git)
            .map(|p| norm_path(&p))
            .map(|n| !n.is_empty() && n == target)
            .unwrap_or(false);
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AntigravityRemovalMode {
    NotPresent,
    WholeFile,
    Surgical,
}

fn antigravity_removal_mode(value: &Value, target: &str) -> Result<AntigravityRemovalMode, String> {
    let resources = value
        .pointer("/projectResources/resources")
        .and_then(|node| node.as_array())
        .ok_or_else(|| "The registry has no projectResources.resources array.".to_string())?;
    let matching_roots = resources
        .iter()
        .filter(|resource| antigravity_resource_matches(resource, target))
        .count();
    Ok(if matching_roots == 0 {
        AntigravityRemovalMode::NotPresent
    } else if matching_roots == resources.len() {
        AntigravityRemovalMode::WholeFile
    } else {
        AntigravityRemovalMode::Surgical
    })
}

/// Surgically remove only the target's `projectResources.resources[]` entries from a
/// multi-root Antigravity file, keeping every sibling entry registered. Each removed element
/// is recorded as a `json_array_item` so restore re-appends exactly it. Writes with a
/// compare-and-swap so a concurrent Antigravity write is not clobbered. Empty when nothing
/// matched (then the caller leaves the file untouched).
fn surgically_remove_antigravity_resources(
    registry_file: &Path,
    target: &str,
) -> Result<Vec<AppRemovalRecord>, String> {
    ensure_not_reparse(registry_file)?;
    let text = fs::read_to_string(registry_file)
        .map_err(|err| format!("Could not read {}: {err}", registry_file.display()))?;
    let original_bytes = text.clone().into_bytes();
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("Could not parse {}: {err}", registry_file.display()))?;
    let pointer = "/projectResources/resources";
    let Some(array) = value.pointer_mut(pointer).and_then(|n| n.as_array_mut()) else {
        return Ok(Vec::new());
    };
    let mut removed: Vec<Value> = Vec::new();
    let mut kept: Vec<Value> = Vec::new();
    for element in array.drain(..) {
        if antigravity_resource_matches(&element, target) {
            removed.push(element);
        } else {
            kept.push(element);
        }
    }
    *array = kept;
    if removed.is_empty() {
        return Ok(Vec::new());
    }
    let serialized = serde_json::to_vec_pretty(&value)
        .map_err(|err| format!("Could not serialize {}: {err}", registry_file.display()))?;
    atomic_write_cas(registry_file, &serialized, &original_bytes)?;
    Ok(removed
        .into_iter()
        .map(|element| AppRemovalRecord {
            app: "antigravity".to_string(),
            original_path: registry_file.display().to_string(),
            backup_path: String::new(),
            kind: "json_array_item".to_string(),
            json_pointer: Some(pointer.to_string()),
            json_key: None,
            json_value: Some(element),
            db_table: None,
            db_rows_json: None,
            restored: false,
        })
        .collect())
}

/// Remove a project from Antigravity's own project list, reversibly. The per-project registry
/// file (`~/.gemini/config/projects/<uuid>.json`) can bundle SEVERAL folder roots, so deleting
/// the whole file would silently de-register the sibling projects too. Therefore: if the file
/// registers ONLY this project's root, it is backed up and deleted (`file` kind). If it ALSO
/// lists sibling roots, only the target's `resources[]` entries are surgically removed
/// (`json_array_item`), leaving the siblings registered. Empty when the project is not in
/// Antigravity.
pub fn remove_antigravity_registration(
    project_root: &str,
    backup_dir: &str,
) -> Result<Vec<AppRemovalRecord>, String> {
    let target = norm_path(Path::new(project_root));
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let Some(registry_file) =
        hangar_discovery::antigravity_registry_file_for_root(Path::new(project_root))
    else {
        return Ok(Vec::new());
    };
    // Parse once and fail closed. If the app changes format, the file becomes malformed, or the
    // target is no longer present, we must not guess that the whole registry belongs to this one
    // project and remove it.
    ensure_not_reparse(&registry_file)?;
    let original = fs::read(&registry_file)
        .map_err(|err| format!("Could not read {}: {err}", registry_file.display()))?;
    let value: Value = serde_json::from_slice(&original)
        .map_err(|err| format!("Could not parse {}: {err}", registry_file.display()))?;
    let removal_mode = antigravity_removal_mode(&value, &target)
        .map_err(|reason| format!("{} {reason} Nothing was removed.", registry_file.display()))?;
    match removal_mode {
        AntigravityRemovalMode::NotPresent => Ok(Vec::new()),
        AntigravityRemovalMode::Surgical => {
            surgically_remove_antigravity_resources(&registry_file, &target)
        }
        AntigravityRemovalMode::WholeFile => backup_and_remove_file_expected(
            &registry_file,
            "antigravity",
            Path::new(backup_dir),
            &original,
        )
        .map(|record| vec![record]),
    }
}

// ----- Cursor --------------------------------------------------------------------------

fn cursor_workspace_storage_dir() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("Cursor\\User\\workspaceStorage"))
}

fn cursor_global_storage_json() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("Cursor\\User\\globalStorage\\storage.json"))
}

/// Remove a project from Cursor, reversibly. Each `workspaceStorage/<hash>/` folder whose
/// `workspace.json` points at this project root is backed up and deleted (a clean
/// per-project registration), and the matching key in `storage.json` →
/// `profileAssociations.workspaces` is removed surgically (recorded as a `json_key` so
/// restore re-inserts exactly that key). Returns one record per change; empty when the
/// project is not in Cursor.
pub fn remove_cursor_registration(
    project_root: &str,
    backup_dir: &str,
) -> Result<Vec<AppRemovalRecord>, String> {
    let target = norm_path(Path::new(project_root));
    // Guard against an empty/never-populated project root: it would norm-equal every bare
    // `file:///` entry (which also decodes to "") and wipe unrelated Cursor registrations.
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let backup_dir = Path::new(backup_dir);
    let mut records = Vec::new();

    // 1. Per-workspace folders.
    if let Some(ws_root) = cursor_workspace_storage_dir() {
        if let Ok(entries) = fs::read_dir(&ws_root) {
            for entry in entries.flatten() {
                let folder = entry.path();
                if !folder.is_dir() {
                    continue;
                }
                let workspace_json = folder.join("workspace.json");
                let Ok(text) = fs::read_to_string(&workspace_json) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let folder_uri = value.get("folder").and_then(|v| v.as_str());
                let matches = folder_uri
                    .and_then(decode_file_uri)
                    .map(|p| norm_path(&p))
                    .map(|decoded| !decoded.is_empty() && decoded == target)
                    .unwrap_or(false);
                if matches {
                    records.push(backup_and_remove_dir(&folder, "cursor", backup_dir)?);
                }
            }
        }
    }

    // 2. Surgical key removal from the shared storage.json.
    if let Some(storage) = cursor_global_storage_json() {
        if let Ok(text) = fs::read_to_string(&storage) {
            let original_bytes = text.clone().into_bytes();
            if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                let pointer = "/profileAssociations/workspaces";
                if let Some(map) = value
                    .pointer_mut(pointer)
                    .and_then(|node| node.as_object_mut())
                {
                    let matching: Vec<String> = map
                        .keys()
                        .filter(|key| {
                            decode_file_uri(key)
                                .map(|p| norm_path(&p))
                                .map(|decoded| !decoded.is_empty() && decoded == target)
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    let mut new_json_keys = Vec::new();
                    for key in matching {
                        if let Some(removed) = map.remove(&key) {
                            new_json_keys.push(AppRemovalRecord {
                                app: "cursor".to_string(),
                                original_path: storage.display().to_string(),
                                backup_path: String::new(),
                                kind: "json_key".to_string(),
                                json_pointer: Some(pointer.to_string()),
                                json_key: Some(key),
                                json_value: Some(removed),
                                db_table: None,
                                db_rows_json: None,
                                restored: false,
                            });
                        }
                    }
                    if !new_json_keys.is_empty() {
                        ensure_not_reparse(&storage)?;
                        let serialized = serde_json::to_vec_pretty(&value)
                            .map_err(|err| format!("Could not serialize storage.json: {err}"))?;
                        // CAS: never clobber a concurrent Cursor write.
                        atomic_write_cas(&storage, &serialized, &original_bytes)?;
                        records.extend(new_json_keys);
                    }
                }
            }
        }
    }

    Ok(records)
}

// ----- Hermes --------------------------------------------------------------------------

/// Best-effort: fold any WAL into the main DB so reads see committed rows.
fn checkpoint_sqlite(db: &Path) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db)
        .map_err(|err| format!("Could not open the database: {err}"))?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    Ok(())
}

/// Remove a project from a Hermes `state.db`, reversibly and surgically. Exports the
/// matching `sessions` rows to the record (as JSON), then deletes only those rows. Restore
/// re-INSERTs exactly those rows, leaving every other session untouched — so removing or
/// restoring one project never disturbs another. Returns `None` if no session matched.
pub fn remove_hermes_registration(
    state_db: &Path,
    project_root: &str,
    _backup_dir: &Path,
) -> Result<Option<AppRemovalRecord>, String> {
    if !state_db.is_file() {
        return Ok(None);
    }
    ensure_not_reparse(state_db)?;
    checkpoint_sqlite(state_db)?;

    let conn = rusqlite::Connection::open(state_db)
        .map_err(|err| format!("Could not open the Hermes database: {err}"))?;

    // Tolerant match: collect the DISTINCT stored cwd strings whose NORMALIZED form equals the
    // target (so a trailing-slash / case / slash-direction variant is matched, like every other
    // app matcher), rather than a byte-exact cwd that can silently miss the registration
    // discovery surfaced. Then export and delete exactly those cwd values — never a non-match.
    let target_norm = norm_path(Path::new(project_root));
    if target_norm.is_empty() {
        return Ok(None);
    }
    let matching_cwds: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT cwd FROM sessions")
            .map_err(|err| format!("Could not read the Hermes sessions: {err}"))?;
        let mut out = Vec::new();
        let mut query = stmt
            .query([])
            .map_err(|err| format!("Could not query the Hermes sessions: {err}"))?;
        while let Some(row) = query
            .next()
            .map_err(|err| format!("Could not read a Hermes row: {err}"))?
        {
            let cwd: Option<String> = row
                .get(0)
                .map_err(|err| format!("Could not read a Hermes cwd: {err}"))?;
            if let Some(cwd) = cwd {
                if norm_path(Path::new(&cwd)) == target_norm {
                    out.push(cwd);
                }
            }
        }
        out
    };
    if matching_cwds.is_empty() {
        return Ok(None);
    }
    let placeholders = (1..=matching_cwds.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Export the matching rows generically (whatever columns the table has).
    let rows: Vec<Value> = {
        let sql = format!("SELECT * FROM sessions WHERE cwd IN ({placeholders})");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| format!("Could not read the Hermes sessions: {err}"))?;
        let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut out = Vec::new();
        let mut query = stmt
            .query(rusqlite::params_from_iter(matching_cwds.iter()))
            .map_err(|err| format!("Could not query the Hermes sessions: {err}"))?;
        while let Some(row) = query
            .next()
            .map_err(|err| format!("Could not read a Hermes row: {err}"))?
        {
            let mut obj = serde_json::Map::new();
            for (index, column) in columns.iter().enumerate() {
                let value: rusqlite::types::Value = row
                    .get(index)
                    .map_err(|err| format!("Could not read a Hermes column: {err}"))?;
                obj.insert(column.clone(), sqlite_value_to_json(&value));
            }
            out.push(Value::Object(obj));
        }
        out
    };

    if rows.is_empty() {
        return Ok(None);
    }

    let delete_sql = format!("DELETE FROM sessions WHERE cwd IN ({placeholders})");
    conn.execute(
        &delete_sql,
        rusqlite::params_from_iter(matching_cwds.iter()),
    )
    .map_err(|err| format!("Could not remove the Hermes sessions: {err}"))?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");

    let rows_json = serde_json::to_string(&rows)
        .map_err(|err| format!("Could not serialize the Hermes rows: {err}"))?;
    Ok(Some(AppRemovalRecord {
        app: "hermes".to_string(),
        original_path: state_db.display().to_string(),
        backup_path: String::new(),
        kind: "db_rows".to_string(),
        json_pointer: None,
        json_key: None,
        json_value: None,
        db_table: Some("sessions".to_string()),
        db_rows_json: Some(rows_json),
        restored: false,
    }))
}

/// Derive the Linux path Hermes stores in `cwd` from a Code Hangar project root, covering
/// BOTH forms discovery produces: a `\\wsl.localhost\<distro>\home\<user>\proj` UNC path →
/// `/home/<user>/proj`, and a Windows-mounted-drive path `X:\a\b` → `/mnt/x/a/b`. Returns
/// `None` for anything else (so the caller no-ops).
fn linux_cwd_for_project(project_root: &str) -> Option<String> {
    let windows = project_root.replace('/', "\\");
    if let Some(rest) = windows
        .strip_prefix("\\\\wsl.localhost\\")
        .or_else(|| windows.strip_prefix("\\\\wsl$\\"))
    {
        let mut segments = rest.split('\\').filter(|s| !s.is_empty());
        let _distro = segments.next()?;
        let inner: Vec<&str> = segments.collect();
        if inner.is_empty() {
            return None;
        }
        return Some(format!("/{}", inner.join("/")));
    }
    let bytes = windows.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = windows[3..].replace('\\', "/");
        let rest = rest.trim_end_matches('/');
        return Some(format!("/mnt/{drive}/{rest}"));
    }
    None
}

/// Remove a project's Hermes sessions, when the project lives on WSL (Linux home OR a
/// `/mnt`-mounted Windows drive). Tries every WSL Hermes `state.db` with the derived Linux
/// cwd; a no-op (empty) for non-WSL projects or when no session matches, so it can never
/// touch the wrong rows.
fn remove_hermes_for_project(
    project_root: &str,
    backup_dir: &Path,
) -> Result<Vec<AppRemovalRecord>, String> {
    let Some(linux_cwd) = linux_cwd_for_project(project_root) else {
        return Ok(Vec::new());
    };
    if linux_cwd.is_empty() || linux_cwd == "/" {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for state_db in hangar_discovery::wsl_hermes_state_dbs() {
        if let Some(record) = remove_hermes_registration(&state_db, &linux_cwd, backup_dir)? {
            records.push(record);
        }
    }
    Ok(records)
}

// ----- Codex --------------------------------------------------------------------------

fn user_home() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
}

fn codex_config_path() -> Option<PathBuf> {
    Some(user_home()?.join(".codex").join("config.toml"))
}

/// Remove a project from Codex's `~/.codex/config.toml`, reversibly and surgically. Removes
/// the `[projects.'<path>']` table(s) that match the project root and records each as a
/// `toml_table` (restore re-inserts exactly that table), leaving every other project and the
/// global config untouched. Empty when the project is not in Codex.
pub fn remove_codex_registration(
    project_root: &str,
    _backup_dir: &str,
) -> Result<Vec<AppRemovalRecord>, String> {
    let target = norm_path(Path::new(project_root));
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let Some(config) = codex_config_path() else {
        return Ok(Vec::new());
    };
    let Ok(text) = fs::read_to_string(&config) else {
        return Ok(Vec::new());
    };
    let original_bytes = text.clone().into_bytes();
    let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else {
        // Unparseable — never overwrite.
        return Ok(Vec::new());
    };
    let keys: Vec<String> = match doc.get("projects").and_then(|item| item.as_table()) {
        Some(table) => table
            .iter()
            .map(|(key, _)| key.to_string())
            .filter(|key| {
                let norm = norm_path(Path::new(key));
                !norm.is_empty() && norm == target
            })
            .collect(),
        None => Vec::new(),
    };
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    ensure_not_reparse(&config)?;
    let mut records = Vec::new();
    {
        let projects = doc["projects"]
            .as_table_mut()
            .ok_or_else(|| "ChatGPT CLI projects is not a table.".to_string())?;
        for key in &keys {
            if let Some(item) = projects.remove(key) {
                let mut wrapper = toml_edit::Table::new();
                wrapper.insert(key, item);
                let mut frag = toml_edit::DocumentMut::new();
                frag.insert("projects", toml_edit::Item::Table(wrapper));
                records.push(AppRemovalRecord {
                    app: "codex".to_string(),
                    original_path: config.display().to_string(),
                    backup_path: String::new(),
                    kind: "toml_table".to_string(),
                    json_pointer: None,
                    json_key: Some(key.clone()),
                    json_value: Some(Value::String(frag.to_string())),
                    db_table: None,
                    db_rows_json: None,
                    restored: false,
                });
            }
        }
    }
    if !records.is_empty() {
        atomic_write_cas(&config, doc.to_string().as_bytes(), &original_bytes)?;
    }
    Ok(records)
}

// ----- Claude -------------------------------------------------------------------------

fn claude_config_path() -> Option<PathBuf> {
    Some(user_home()?.join(".claude.json"))
}

/// Remove a project from Claude's `~/.claude.json`, reversibly and surgically. Removes every
/// `projects` key matching the project root (Claude can store the same project under several
/// slash/case variants) and records each as a `json_key`. Uses a compare-and-swap write so a
/// concurrent write by the RUNNING Claude is never clobbered (it aborts to retry instead).
/// Empty when the project is not in Claude.
pub fn remove_claude_registration(
    project_root: &str,
    _backup_dir: &str,
) -> Result<Vec<AppRemovalRecord>, String> {
    let target = norm_path(Path::new(project_root));
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let Some(config) = claude_config_path() else {
        return Ok(Vec::new());
    };
    let Ok(text) = fs::read_to_string(&config) else {
        return Ok(Vec::new());
    };
    let original_bytes = text.clone().into_bytes();
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return Ok(Vec::new());
    };
    let keys: Vec<String> = match value.get("projects").and_then(|v| v.as_object()) {
        Some(map) => map
            .keys()
            .filter(|key| {
                let norm = norm_path(Path::new(key));
                !norm.is_empty() && norm == target
            })
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    ensure_not_reparse(&config)?;
    let mut records = Vec::new();
    {
        let map = value
            .get_mut("projects")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| "Claude projects is not an object.".to_string())?;
        for key in &keys {
            if let Some(removed) = map.remove(key) {
                records.push(AppRemovalRecord {
                    app: "claude".to_string(),
                    original_path: config.display().to_string(),
                    backup_path: String::new(),
                    kind: "json_key".to_string(),
                    json_pointer: Some("/projects".to_string()),
                    json_key: Some(key.clone()),
                    json_value: Some(removed),
                    db_table: None,
                    db_rows_json: None,
                    restored: false,
                });
            }
        }
    }
    if !records.is_empty() {
        let serialized = serde_json::to_vec_pretty(&value)
            .map_err(|err| format!("Could not serialize ~/.claude.json: {err}"))?;
        atomic_write_cas(&config, &serialized, &original_bytes)?;
    }
    Ok(records)
}

// ----- orchestration -------------------------------------------------------------------

/// The outcome of a "remove from every AI app" run: the records of every change that was
/// ACTUALLY made on disk (so they can always be persisted and reversed) plus any per-app
/// warnings for apps that could not be updated (e.g. one was running and held its config).
#[derive(Debug, Clone, Default)]
pub struct AppRemovalOutcome {
    pub records: Vec<AppRemovalRecord>,
    pub warnings: Vec<String>,
}

/// Remove a project from EVERY supported AI app that registers it, reversibly. Each app's
/// registration is recorded before it is changed. Covers Antigravity + Cursor + Codex
/// (Windows files/config) and Hermes (WSL `state.db`), plus Claude's `~/.claude.json` —
/// the latter edited with a compare-and-swap so the running Claude is never clobbered.
///
/// BEST-EFFORT: each app runs independently; a failure in one (e.g. Codex's config changed
/// under the CAS because Codex is running) is captured as a warning and does NOT abort the
/// others or discard the records already collected. The caller MUST persist `records` even
/// when `warnings` is non-empty, so any change actually made on disk stays recoverable.
pub fn remove_project_app_registrations(
    project_root: &str,
    backup_dir: &str,
) -> Result<AppRemovalOutcome, String> {
    // A blank root must never reach a matcher (it would norm-equal degenerate empty entries).
    if norm_path(Path::new(project_root)).is_empty() {
        return Ok(AppRemovalOutcome::default());
    }
    let mut out = AppRemovalOutcome::default();
    // Each app is run eagerly and independently; the per-app Result is captured here so one
    // app's failure cannot short-circuit the others or discard already-collected records.
    let steps: [(&str, Result<Vec<AppRemovalRecord>, String>); 5] = [
        (
            "Antigravity",
            remove_antigravity_registration(project_root, backup_dir),
        ),
        (
            "Cursor",
            remove_cursor_registration(project_root, backup_dir),
        ),
        (
            "ChatGPT",
            remove_codex_registration(project_root, backup_dir),
        ),
        (
            "Claude",
            remove_claude_registration(project_root, backup_dir),
        ),
        (
            "Hermes",
            remove_hermes_for_project(project_root, Path::new(backup_dir)),
        ),
    ];
    for (app, result) in steps {
        match result {
            Ok(records) => out.records.extend(records),
            Err(err) => out.warnings.push(format!("{app}: {err}")),
        }
    }
    Ok(out)
}

// ----- restore -------------------------------------------------------------------------

/// How a record was satisfied. `AlreadyPresent` means the app re-created its
/// registration since the removal, so nothing was written back — crucially the
/// pre-removal backup copy must then be KEPT on disk: a re-created registration
/// (e.g. Cursor recreating an empty workspace folder) is not the pre-removal
/// data, and deleting the backup would destroy the only copy of it.
enum RecordRestore {
    Restored,
    AlreadyPresent,
}

/// Put one recorded change back. Atomic where it writes; never follows a reparse point.
fn restore_record(record: &AppRemovalRecord) -> Result<RecordRestore, String> {
    let original = Path::new(&record.original_path);
    ensure_no_reparse_components(original)?;
    match record.kind.as_str() {
        "file" => {
            if original.exists() {
                // Already present — the app re-created its registration since the removal; do
                // not clobber its current file with the stale pre-removal backup (mirrors the
                // "dir" arm). The registration is back either way; the backup stays on disk.
                return Ok(RecordRestore::AlreadyPresent);
            }
            ensure_not_reparse(original)?;
            let bytes = fs::read(&record.backup_path)
                .map_err(|err| format!("Could not read the backup to restore: {err}"))?;
            atomic_write(original, &bytes).map(|_| RecordRestore::Restored)
        }
        "dir" => {
            if original.exists() {
                // Already present — do not clobber a re-created registration; keep the backup.
                return Ok(RecordRestore::AlreadyPresent);
            }
            let backup = Path::new(&record.backup_path);
            ensure_no_reparse_components(backup)?;
            if let Some(parent) = original.parent() {
                fs::create_dir_all(parent)
                    .map_err(|err| format!("Could not create the restore folder: {err}"))?;
                ensure_no_reparse_components(parent)?;
            }
            fs::rename(backup, original)
                .map(|_| RecordRestore::Restored)
                .map_err(|err| {
                    format!(
                        "Could not restore the registry folder atomically; the recovery copy is still intact: {err}"
                    )
                })
        }
        "json_key" => {
            ensure_not_reparse(original)?;
            let pointer = record
                .json_pointer
                .as_deref()
                .ok_or_else(|| "Missing json pointer for restore.".to_string())?;
            let key = record
                .json_key
                .clone()
                .ok_or_else(|| "Missing json key for restore.".to_string())?;
            let value = record.json_value.clone().unwrap_or(Value::Null);
            let text = fs::read_to_string(original)
                .map_err(|err| format!("Could not read {}: {err}", original.display()))?;
            let expected = text.clone().into_bytes();
            let mut doc: Value = serde_json::from_str(&text)
                .map_err(|err| format!("Could not parse {}: {err}", original.display()))?;
            // Navigate/create the pointer target object.
            let segments: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
            let mut node = &mut doc;
            for segment in &segments {
                if !node.is_object() {
                    *node = Value::Object(serde_json::Map::new());
                }
                node = node
                    .as_object_mut()
                    .unwrap()
                    .entry((*segment).to_string())
                    .or_insert(Value::Object(serde_json::Map::new()));
            }
            if !node.is_object() {
                *node = Value::Object(serde_json::Map::new());
            }
            node.as_object_mut().unwrap().insert(key, value);
            let serialized = serde_json::to_vec_pretty(&doc)
                .map_err(|err| format!("Could not serialize {}: {err}", original.display()))?;
            // CAS: never clobber a concurrent write by the running app between our read and write.
            atomic_write_cas(original, &serialized, &expected).map(|_| RecordRestore::Restored)
        }
        "json_array_item" => {
            ensure_not_reparse(original)?;
            let pointer = record
                .json_pointer
                .as_deref()
                .ok_or_else(|| "Missing json pointer for restore.".to_string())?;
            let value = record.json_value.clone().unwrap_or(Value::Null);
            let text = fs::read_to_string(original)
                .map_err(|err| format!("Could not read {}: {err}", original.display()))?;
            let expected = text.clone().into_bytes();
            let mut doc: Value = serde_json::from_str(&text)
                .map_err(|err| format!("Could not parse {}: {err}", original.display()))?;
            // Navigate/create the pointer target array, then re-append the removed element.
            let segments: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
            let mut node = &mut doc;
            for segment in &segments {
                if !node.is_object() {
                    *node = Value::Object(serde_json::Map::new());
                }
                node = node
                    .as_object_mut()
                    .unwrap()
                    .entry((*segment).to_string())
                    .or_insert(Value::Array(Vec::new()));
            }
            if !node.is_array() {
                *node = Value::Array(Vec::new());
            }
            node.as_array_mut().unwrap().push(value);
            let serialized = serde_json::to_vec_pretty(&doc)
                .map_err(|err| format!("Could not serialize {}: {err}", original.display()))?;
            atomic_write_cas(original, &serialized, &expected).map(|_| RecordRestore::Restored)
        }
        "db_rows" => {
            ensure_not_reparse(original)?;
            let table = record
                .db_table
                .as_deref()
                .ok_or_else(|| "Missing db table for restore.".to_string())?;
            if !is_safe_sql_ident(table) {
                return Err("Unsafe table name in restore record.".to_string());
            }
            let rows: Vec<Value> =
                serde_json::from_str(record.db_rows_json.as_deref().unwrap_or("[]"))
                    .map_err(|err| format!("Could not parse the rows to restore: {err}"))?;
            let mut conn = rusqlite::Connection::open(original)
                .map_err(|err| format!("Could not open the database to restore: {err}"))?;
            // One transaction: all rows re-insert or none — never a half-applied restore.
            let tx = conn
                .transaction()
                .map_err(|err| format!("Could not start a restore transaction: {err}"))?;
            for row in &rows {
                let Some(object) = row.as_object() else {
                    continue;
                };
                let columns: Vec<String> = object.keys().cloned().collect();
                if !columns.iter().all(|c| is_safe_sql_ident(c)) {
                    return Err("Unsafe column name in restore record.".to_string());
                }
                let placeholders = (1..=columns.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                // OR IGNORE makes the re-insert idempotent: if a crash struck between the
                // commit and persisting `restored=true`, a retry re-runs this restore; with a
                // PRIMARY KEY (Hermes `sessions.id`) the duplicate is silently skipped rather
                // than aborting forever, and without one it cannot create duplicate rows.
                let sql = format!(
                    "INSERT OR IGNORE INTO {table} ({}) VALUES ({placeholders})",
                    columns.join(", ")
                );
                let values: Vec<rusqlite::types::Value> = columns
                    .iter()
                    .map(|c| json_to_sqlite_value(&object[c]))
                    .collect();
                tx.execute(&sql, rusqlite::params_from_iter(values))
                    .map_err(|err| format!("Could not re-insert a Hermes row: {err}"))?;
            }
            tx.commit()
                .map_err(|err| format!("Could not commit the restore: {err}"))?;
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            Ok(RecordRestore::Restored)
        }
        "toml_table" => {
            ensure_not_reparse(original)?;
            let key = record
                .json_key
                .clone()
                .ok_or_else(|| "Missing toml key for restore.".to_string())?;
            let fragment = match record.json_value.as_ref() {
                Some(Value::String(s)) => s.clone(),
                _ => return Err("Missing toml fragment for restore.".to_string()),
            };
            let text = fs::read_to_string(original)
                .map_err(|err| format!("Could not read {}: {err}", original.display()))?;
            let expected = text.clone().into_bytes();
            let mut doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|err| format!("Could not parse {}: {err}", original.display()))?;
            let frag = fragment
                .parse::<toml_edit::DocumentMut>()
                .map_err(|err| format!("Could not parse the restore fragment: {err}"))?;
            let item = frag
                .get("projects")
                .and_then(|i| i.as_table())
                .and_then(|t| t.get(&key))
                .cloned()
                .ok_or_else(|| "The restore fragment is missing its table.".to_string())?;
            if doc.get("projects").is_none() {
                doc.insert("projects", toml_edit::Item::Table(toml_edit::Table::new()));
            }
            doc["projects"]
                .as_table_mut()
                .ok_or_else(|| "projects is not a table.".to_string())?
                .insert(&key, item);
            // CAS: never clobber a concurrent write by the running Codex.
            atomic_write_cas(original, doc.to_string().as_bytes(), &expected)
                .map(|_| RecordRestore::Restored)
        }
        other => Err(format!("Unknown removal record kind: {other}")),
    }
}

/// Reverse one removal record, so the AI app lists the project again. (Direct/test path;
/// the manifest path adds containment checks and the backup-preservation semantics.)
pub fn restore_app_removal(record: &AppRemovalRecord) -> Result<(), String> {
    restore_record(record).map(|_| ())
}

// ----- persisted manifest --------------------------------------------------------------

/// A persisted "remove from AI apps", recoverable from Recover even after a restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedAppRemoval {
    pub id: String,
    pub project_name: String,
    pub removed_at_unix: u64,
    pub records: Vec<AppRemovalRecord>,
}

fn removals_manifest_path(backup_dir: &Path) -> PathBuf {
    backup_dir.join("removals.json")
}

/// Read the manifest. Returns an empty list ONLY when the file is genuinely absent; a read
/// or parse failure is an error so a transient lock/corruption never causes a later write to
/// silently overwrite the good manifest with a partial one.
fn read_removals(backup_dir: &Path) -> Result<Vec<PersistedAppRemoval>, String> {
    let path = removals_manifest_path(backup_dir);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| {
            // Preserve the unparseable file aside instead of letting it be clobbered.
            let aside = backup_dir.join(format!("removals.corrupt-{}.json", nanos()));
            let _ = fs::copy(&path, &aside);
            format!(
                "The recovery manifest is unreadable (kept a copy at {}): {err}",
                aside.display()
            )
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("Could not read the recovery manifest: {err}")),
    }
}

fn write_removals(backup_dir: &Path, list: &[PersistedAppRemoval]) -> Result<(), String> {
    fs::create_dir_all(backup_dir)
        .map_err(|err| format!("Could not create the backup folder: {err}"))?;
    let path = removals_manifest_path(backup_dir);
    // Keep a .bak of the prior good manifest before replacing it.
    if path.exists() {
        let _ = fs::copy(&path, backup_dir.join("removals.bak.json"));
    }
    let json = serde_json::to_vec_pretty(list)
        .map_err(|err| format!("Could not serialize the recovery manifest: {err}"))?;
    atomic_write(&path, &json)
}

/// Persist a completed removal so it can be recovered from Recover at any time. Returns the
/// persisted entry (incl. its id, for a durable Undo). No-op (`None`) when nothing to record.
pub fn record_app_removal(
    backup_dir: &Path,
    project_name: &str,
    records: &[AppRemovalRecord],
) -> Result<Option<PersistedAppRemoval>, String> {
    if records.is_empty() {
        return Ok(None);
    }
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let entry = PersistedAppRemoval {
        id: format!("rm-{}", elapsed.as_nanos()),
        project_name: project_name.to_string(),
        removed_at_unix: elapsed.as_secs(),
        records: records.to_vec(),
    };
    let mut list = read_removals(backup_dir)?;
    list.push(entry.clone());
    write_removals(backup_dir, &list)?;
    Ok(Some(entry))
}

/// True when a record's change is still in effect (so it is worth showing as recoverable).
fn record_still_pending(record: &AppRemovalRecord) -> bool {
    if record.restored {
        return false;
    }
    match record.kind.as_str() {
        // A file/dir registration is pending while the original is gone and the backup exists.
        "file" | "dir" => {
            !Path::new(&record.original_path).exists() && Path::new(&record.backup_path).exists()
        }
        // Surgical edits hold their restore data inline, so they are pending until restored.
        "json_key" | "db_rows" | "toml_table" | "json_array_item" => true,
        _ => false,
    }
}

/// Every removal still pending recovery (at least one of its records is still in effect).
pub fn list_app_removals(backup_dir: &Path) -> Vec<PersistedAppRemoval> {
    read_removals(backup_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.records.iter().any(record_still_pending))
        .collect()
}

/// Restore a persisted removal by id. Restores each not-yet-restored record, marking it
/// restored as it goes; a record that fails containment or restore is SKIPPED (not aborted)
/// so its siblings still recover, and progress is PERSISTED so a retry never re-runs a record
/// that already succeeded (which would, for `db_rows`, duplicate). Drops the entry only when
/// every record is restored.
pub fn restore_app_removal_by_id(backup_dir: &Path, id: &str) -> Result<(), String> {
    let mut list = read_removals(backup_dir)?;
    let Some(pos) = list.iter().position(|entry| entry.id == id) else {
        return Err("That removal is no longer recorded.".to_string());
    };

    let mut errors: Vec<String> = Vec::new();
    // Indexes of records whose backup was ACTUALLY written back this run. Only these
    // may have their backup copy cleaned up below: a record satisfied because the app
    // re-created its registration (AlreadyPresent) keeps its backup — that fresh
    // registration is not the pre-removal data, and the backup may be the only copy
    // of it (e.g. a Cursor workspace folder holding chat history).
    let mut restored_this_run: Vec<usize> = Vec::new();
    for (index, record) in list[pos].records.iter_mut().enumerate() {
        if record.restored {
            continue;
        }
        // Containment: never let a tampered manifest redirect a restore outside the managed
        // registries, and keep file/dir backups strictly inside the backup dir. Skip on fail.
        if !is_managed_registry_path(Path::new(&record.original_path)) {
            errors.push(format!(
                "skipped an unmanaged location ({})",
                record.original_path
            ));
            continue;
        }
        if matches!(record.kind.as_str(), "file" | "dir")
            && !normalized_path_is_within(Path::new(&record.backup_path), backup_dir)
        {
            errors.push("skipped a backup outside the backup folder".to_string());
            continue;
        }
        match restore_record(record) {
            Ok(RecordRestore::Restored) => {
                record.restored = true;
                restored_this_run.push(index);
            }
            Ok(RecordRestore::AlreadyPresent) => record.restored = true,
            Err(err) => errors.push(err),
        }
    }

    // Clean up the backup copies of records that DID restore this run (surgical kinds
    // have none; AlreadyPresent records deliberately keep theirs — see above).
    for index in restored_this_run {
        let record = &list[pos].records[index];
        if matches!(record.kind.as_str(), "file" | "dir") && !record.backup_path.is_empty() {
            let backup = Path::new(&record.backup_path);
            if backup.is_dir() {
                let _ = fs::remove_dir_all(backup);
            } else {
                let _ = fs::remove_file(backup);
            }
        }
    }

    // Drop the entry only when fully restored; otherwise persist progress so a retry skips
    // the records already done.
    if list[pos].records.iter().all(|record| record.restored) {
        list.remove(pos);
    }
    write_removals(backup_dir, &list)?;

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Restored what it could; {} item(s) could not be restored: {}",
            errors.len(),
            errors.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn canonicalizes_an_existing_parent_before_comparing_a_missing_restore_target() {
        let dir = tempdir().unwrap();
        let managed = dir.path().join(".gemini").join("config").join("projects");
        fs::create_dir_all(&managed).unwrap();
        let missing = managed.join("project.json");

        let resolved = canonicalize_with_missing_tail(&missing);
        assert_eq!(
            resolved,
            fs::canonicalize(&managed).unwrap().join("project.json")
        );
    }

    #[test]
    fn file_remove_then_restore_roundtrips() {
        let dir = tempdir().unwrap();
        let registry = dir.path().join("abc-123.json");
        let body = br#"{"name":"My Proj"}"#;
        fs::write(&registry, body).unwrap();
        let backups = dir.path().join("backups");

        let record = backup_and_remove_file(&registry, "antigravity", &backups).unwrap();
        assert!(!registry.exists());
        assert!(Path::new(&record.backup_path).exists());
        assert_eq!(record.kind, "file");

        restore_app_removal(&record).unwrap();
        assert!(registry.exists());
        assert_eq!(fs::read(&registry).unwrap(), body);
    }

    #[test]
    fn dir_remove_then_restore_roundtrips() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("7d0d846999");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("workspace.json"), br#"{"folder":"file:///x"}"#).unwrap();
        fs::write(ws.join("state.vscdb"), b"binary").unwrap();
        let backups = dir.path().join("backups");

        let record = backup_and_remove_dir(&ws, "cursor", &backups).unwrap();
        assert!(!ws.exists());
        restore_app_removal(&record).unwrap();
        assert!(ws.join("workspace.json").exists());
        assert_eq!(fs::read(ws.join("state.vscdb")).unwrap(), b"binary");
    }

    #[test]
    fn backup_containment_requires_a_path_component_boundary() {
        let root = Path::new(r"C:\Users\me\AppData\CodeHangar\app-removal-backups");
        assert!(normalized_path_is_within(
            &root.join("cursor-123-workspace"),
            root
        ));
        assert!(!normalized_path_is_within(
            Path::new(r"C:\Users\me\AppData\CodeHangar\app-removal-backups-evil\payload"),
            root
        ));
    }

    #[test]
    fn json_key_remove_then_restore_reinserts_only_that_key() {
        let dir = tempdir().unwrap();
        let storage = dir.path().join("storage.json");
        fs::write(
            &storage,
            br#"{"profileAssociations":{"workspaces":{"file:///c%3A/AI/Proj":"__default__profile__","file:///c%3A/Other":"p2"}}}"#,
        )
        .unwrap();

        // Simulate a json_key removal of the first workspace, then restore it.
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&storage).unwrap()).unwrap();
        let removed = value
            .pointer_mut("/profileAssociations/workspaces")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("file:///c%3A/AI/Proj")
            .unwrap();
        atomic_write(&storage, &serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let record = AppRemovalRecord {
            app: "cursor".into(),
            original_path: storage.display().to_string(),
            backup_path: String::new(),
            kind: "json_key".into(),
            json_pointer: Some("/profileAssociations/workspaces".into()),
            json_key: Some("file:///c%3A/AI/Proj".into()),
            json_value: Some(removed),
            db_table: None,
            db_rows_json: None,
            restored: false,
        };
        restore_app_removal(&record).unwrap();
        let after: Value = serde_json::from_str(&fs::read_to_string(&storage).unwrap()).unwrap();
        let ws = after
            .pointer("/profileAssociations/workspaces")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(ws.len(), 2, "both keys present after restore");
        assert_eq!(ws["file:///c%3A/Other"], Value::from("p2"));
    }

    #[test]
    fn hermes_row_removal_is_surgical_and_reversible() {
        let dir = tempdir().unwrap();
        let state_db = dir.path().join("state.db");
        {
            let conn = rusqlite::Connection::open(&state_db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT, source TEXT, title TEXT, cwd TEXT, started_at REAL, ended_at REAL);
                 INSERT INTO sessions VALUES ('1','hermes','A','/home/u/proj', 1.0, 2.0);
                 INSERT INTO sessions VALUES ('2','hermes','B','/home/u/other', 1.0, 2.0);",
            )
            .unwrap();
        }
        let backups = dir.path().join("backups");

        let record = remove_hermes_registration(&state_db, "/home/u/proj", &backups)
            .unwrap()
            .expect("the project's session should match");
        assert_eq!(record.kind, "db_rows");
        {
            let conn = rusqlite::Connection::open(&state_db).unwrap();
            let remaining: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(remaining, 1);
        }

        // Restore re-inserts only the removed row; the other project is never disturbed.
        restore_app_removal(&record).unwrap();
        {
            let conn = rusqlite::Connection::open(&state_db).unwrap();
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(total, 2);
            let restored_title: String = conn
                .query_row(
                    "SELECT title FROM sessions WHERE cwd = '/home/u/proj'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(restored_title, "A");
        }

        assert!(
            remove_hermes_registration(&state_db, "/home/u/missing", &backups)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn linux_cwd_derivation_handles_unc_and_mounted_drive() {
        // UNC home form -> /home/...
        assert_eq!(
            linux_cwd_for_project("\\\\wsl.localhost\\Ubuntu\\home\\u\\proj").as_deref(),
            Some("/home/u/proj")
        );
        // Forward-slash UNC also works.
        assert_eq!(
            linux_cwd_for_project("//wsl.localhost/Ubuntu/home/u/proj").as_deref(),
            Some("/home/u/proj")
        );
        // Windows-mounted-drive form -> /mnt/<drive>/...
        assert_eq!(
            linux_cwd_for_project("C:\\AI\\proj").as_deref(),
            Some("/mnt/c/AI/proj")
        );
        // A relative/garbage path yields nothing (caller no-ops).
        assert_eq!(linux_cwd_for_project("proj").as_deref(), None);
        assert_eq!(linux_cwd_for_project("").as_deref(), None);
    }

    #[test]
    fn codex_toml_table_remove_then_restore_roundtrips() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        // Real Codex keys are single-quoted literals with single backslashes; `\\` in this
        // Rust string is one backslash in the file.
        fs::write(
            &config,
            "[projects.'C:\\AI\\Keep']\ntrust_level = \"trusted\"\n\n[projects.'C:\\AI\\Proj']\ntrust_level = \"trusted\"\n",
        )
        .unwrap();
        // Simulate the surgical removal directly against this config via the same primitives.
        let original = fs::read_to_string(&config).unwrap();
        let mut doc = original.parse::<toml_edit::DocumentMut>().unwrap();
        let item = doc["projects"]
            .as_table_mut()
            .unwrap()
            .remove("C:\\AI\\Proj")
            .unwrap();
        let mut wrapper = toml_edit::Table::new();
        wrapper.insert("C:\\AI\\Proj", item);
        let mut frag = toml_edit::DocumentMut::new();
        frag.insert("projects", toml_edit::Item::Table(wrapper));
        atomic_write(&config, doc.to_string().as_bytes()).unwrap();

        let record = AppRemovalRecord {
            app: "codex".into(),
            original_path: config.display().to_string(),
            backup_path: String::new(),
            kind: "toml_table".into(),
            json_pointer: None,
            json_key: Some("C:\\AI\\Proj".into()),
            json_value: Some(Value::String(frag.to_string())),
            db_table: None,
            db_rows_json: None,
            restored: false,
        };
        // After removal the kept project is still there, the removed one gone.
        let after = fs::read_to_string(&config).unwrap();
        assert!(after.contains("Keep"));
        assert!(!after.contains("Proj"));
        // Restore re-inserts exactly the removed table; the kept one is untouched.
        restore_app_removal(&record).unwrap();
        let restored = fs::read_to_string(&config).unwrap();
        let doc2 = restored.parse::<toml_edit::DocumentMut>().unwrap();
        let projects = doc2["projects"].as_table().unwrap();
        assert!(projects.contains_key("C:\\AI\\Keep"));
        assert!(projects.contains_key("C:\\AI\\Proj"));
    }

    #[test]
    fn empty_project_root_never_matches_anything() {
        // The critical guard: a blank root must remove nothing anywhere.
        assert!(remove_cursor_registration("", "backups")
            .unwrap()
            .is_empty());
        assert!(remove_codex_registration("", "backups").unwrap().is_empty());
        assert!(remove_claude_registration("", "backups")
            .unwrap()
            .is_empty());
        let outcome = remove_project_app_registrations("", "backups").unwrap();
        assert!(outcome.records.is_empty());
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn antigravity_multi_root_removal_keeps_siblings_and_roundtrips() {
        // An Antigravity per-project file that bundles THREE folder roots: removing one must
        // surgically drop ONLY its resource and leave the two siblings registered (not delete
        // the whole file, which would silently de-register the siblings too).
        let dir = tempdir().unwrap();
        let registry = dir.path().join("uuid-1.json");
        let proj_a = dir.path().join("ProjA");
        let proj_b = dir.path().join("ProjB");
        let proj_c = dir.path().join("ProjC");
        let uri = |p: &Path| format!("file:///{}", p.to_string_lossy().replace('\\', "/"));
        let body = serde_json::json!({
            "projectResources": {
                "resources": [
                    { "folderUri": uri(&proj_a) },
                    { "folderUri": uri(&proj_b) },
                    { "folderUri": uri(&proj_c) },
                ]
            }
        });
        fs::write(&registry, serde_json::to_vec_pretty(&body).unwrap()).unwrap();

        let target = norm_path(&proj_a);
        let records = surgically_remove_antigravity_resources(&registry, &target).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "json_array_item");
        assert_eq!(
            records[0].json_pointer.as_deref(),
            Some("/projectResources/resources")
        );
        assert!(record_still_pending(&records[0]));

        // The file still exists and now lists ONLY the two siblings.
        assert!(registry.exists(), "multi-root file must NOT be deleted");
        let after: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
        let roots_after: Vec<String> = after
            .pointer("/projectResources/resources")
            .and_then(|n| n.as_array())
            .unwrap()
            .iter()
            .filter_map(|r| r.get("folderUri").and_then(|v| v.as_str()))
            .filter_map(decode_file_uri)
            .map(|p| norm_path(&p))
            .collect();
        assert!(
            !roots_after.contains(&norm_path(&proj_a)),
            "target must be gone"
        );
        assert!(
            roots_after.contains(&norm_path(&proj_b)),
            "sibling B must stay"
        );
        assert!(
            roots_after.contains(&norm_path(&proj_c)),
            "sibling C must stay"
        );

        // Restore re-appends exactly the removed resource — the target is registered again,
        // and the siblings are untouched.
        restore_record(&records[0]).unwrap();
        let restored: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
        let roots_restored: Vec<String> = restored
            .pointer("/projectResources/resources")
            .and_then(|n| n.as_array())
            .unwrap()
            .iter()
            .filter_map(|r| r.get("folderUri").and_then(|v| v.as_str()))
            .filter_map(decode_file_uri)
            .map(|p| norm_path(&p))
            .collect();
        assert!(roots_restored.contains(&norm_path(&proj_a)));
        assert!(roots_restored.contains(&norm_path(&proj_b)));
        assert!(roots_restored.contains(&norm_path(&proj_c)));
        assert_eq!(roots_restored.len(), 3);
    }

    #[test]
    fn antigravity_registry_shape_is_fail_closed() {
        let target = norm_path(Path::new(r"C:\AI\Project"));
        let malformed_shape = serde_json::json!({ "name": "Project" });
        assert!(antigravity_removal_mode(&malformed_shape, &target).is_err());

        let unrelated = serde_json::json!({
            "projectResources": {
                "resources": [{ "folderUri": "file:///C:/AI/Other" }]
            }
        });
        assert_eq!(
            antigravity_removal_mode(&unrelated, &target).unwrap(),
            AntigravityRemovalMode::NotPresent
        );
    }

    #[test]
    fn backup_failure_leaves_the_original_intact() {
        let dir = tempdir().unwrap();
        let registry = dir.path().join("keep.json");
        fs::write(&registry, b"data").unwrap();
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"x").unwrap();
        let result = backup_and_remove_file(&registry, "antigravity", &blocker.join("sub"));
        assert!(result.is_err());
        assert!(registry.exists());
    }

    #[test]
    fn file_removal_refuses_when_the_registry_changed_after_inspection() {
        let dir = tempdir().unwrap();
        let registry = dir.path().join("project.json");
        let original = br#"{"project":"one"}"#;
        fs::write(&registry, original).unwrap();
        fs::write(&registry, br#"{"project":"one","newSibling":"two"}"#).unwrap();

        let result = backup_and_remove_file_expected(
            &registry,
            "antigravity",
            &dir.path().join("backups"),
            original,
        );

        assert!(result.is_err());
        assert!(
            registry.exists(),
            "a changed registry must never be removed"
        );
        assert!(dir
            .path()
            .join("backups")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn manifest_corruption_does_not_wipe_records() {
        let dir = tempdir().unwrap();
        let backups = dir.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        // A real removal recorded.
        let registry = dir.path().join("p.json");
        fs::write(&registry, b"{}").unwrap();
        let record = backup_and_remove_file(&registry, "antigravity", &backups).unwrap();
        record_app_removal(&backups, "P", std::slice::from_ref(&record))
            .unwrap()
            .unwrap();
        // Corrupt the manifest, then attempt another record — must NOT silently clobber.
        fs::write(removals_manifest_path(&backups), b"{ not json").unwrap();
        let registry2 = dir.path().join("q.json");
        fs::write(&registry2, b"{}").unwrap();
        let record2 = backup_and_remove_file(&registry2, "antigravity", &backups).unwrap();
        let result = record_app_removal(&backups, "Q", std::slice::from_ref(&record2));
        assert!(
            result.is_err(),
            "a corrupt manifest must abort, not overwrite"
        );
        assert!(
            backups.join("removals.json").exists(),
            "the original manifest survives"
        );
    }

    #[test]
    fn list_then_restore_by_id_clears_the_entry() {
        let dir = tempdir().unwrap();
        let registry = dir.path().join("xyz.json");
        let body = br#"{"name":"P"}"#;
        fs::write(&registry, body).unwrap();
        let backups = dir.path().join("backups");
        // Put the registry under a managed segment so containment passes.
        let managed = dir.path().join(".gemini").join("config").join("projects");
        fs::create_dir_all(&managed).unwrap();
        let managed_registry = managed.join("xyz.json");
        fs::write(&managed_registry, body).unwrap();

        let record = backup_and_remove_file(&managed_registry, "antigravity", &backups).unwrap();
        record_app_removal(&backups, "P", std::slice::from_ref(&record))
            .unwrap()
            .unwrap();

        let pending = list_app_removals(&backups);
        assert_eq!(pending.len(), 1);

        restore_app_removal_by_id(&backups, &pending[0].id).unwrap();
        assert!(managed_registry.exists());
        assert_eq!(fs::read(&managed_registry).unwrap(), body);
        assert!(list_app_removals(&backups).is_empty());
        assert!(!Path::new(&record.backup_path).exists());
    }

    #[test]
    fn restore_keeps_the_backup_when_the_app_recreated_its_registration() {
        // The app re-created its registration between the removal and the restore
        // (e.g. reopening the project in Cursor recreates an empty workspace).
        // Restore must not clobber the fresh registration — and must NOT delete
        // the backup either: that backup may be the only copy of the pre-removal
        // data, and the fresh registration is not it.
        let dir = tempdir().unwrap();
        let backups = dir.path().join("backups");
        let managed = dir.path().join(".gemini").join("config").join("projects");
        fs::create_dir_all(&managed).unwrap();
        let recreated_registry = managed.join("xyz.json");
        let original_body = br#"{"name":"P","history":"precious"}"#;
        fs::write(&recreated_registry, original_body).unwrap();
        let sibling_registry = managed.join("abc.json");
        let sibling_body = br#"{"name":"P-sibling"}"#;
        fs::write(&sibling_registry, sibling_body).unwrap();

        let recreated_record =
            backup_and_remove_file(&recreated_registry, "antigravity", &backups).unwrap();
        let sibling_record =
            backup_and_remove_file(&sibling_registry, "antigravity", &backups).unwrap();
        record_app_removal(
            &backups,
            "P",
            &[recreated_record.clone(), sibling_record.clone()],
        )
        .unwrap()
        .unwrap();
        assert!(!recreated_registry.exists());

        // The app re-creates ONE registration with fresh (different) content; the
        // sibling stays removed, keeping the entry visible in Recover.
        let recreated_body = br#"{"name":"P"}"#;
        fs::write(&recreated_registry, recreated_body).unwrap();

        let pending = list_app_removals(&backups);
        assert_eq!(pending.len(), 1);
        restore_app_removal_by_id(&backups, &pending[0].id).unwrap();

        // Fresh registration untouched; sibling actually restored (its backup
        // cleaned as usual); the recreated record's backup PRESERVED intact.
        assert_eq!(fs::read(&recreated_registry).unwrap(), recreated_body);
        assert_eq!(fs::read(&sibling_registry).unwrap(), sibling_body);
        assert!(list_app_removals(&backups).is_empty());
        assert!(!Path::new(&sibling_record.backup_path).exists());
        assert!(
            Path::new(&recreated_record.backup_path).exists(),
            "the pre-removal backup must survive an AlreadyPresent restore"
        );
        assert_eq!(
            fs::read(&recreated_record.backup_path).unwrap(),
            original_body
        );
    }

    // Adversarial: the atomic-move primitive must preserve a DEEP nested tree byte-for-byte,
    // and a FAILED move must never partially delete the live folder (the property the old
    // copy-then-remove could violate). Independent runtime check on a sandbox tempdir.
    #[test]
    fn dir_atomic_move_preserves_nested_tree_and_never_partial_deletes_on_failure() {
        let dir = tempdir().unwrap();

        // A fake Cursor-style workspace with a nested tree of binary + text files.
        let ws = dir.path().join("workspace-7d0d");
        fs::create_dir_all(ws.join("sub").join("deeper")).unwrap();
        fs::write(
            ws.join("workspace.json"),
            br#"{"folder":"file:///c%3A/AI/Fake"}"#,
        )
        .unwrap();
        fs::write(ws.join("state.vscdb"), [0u8, 1, 2, 3, 255]).unwrap();
        fs::write(ws.join("sub").join("notes.txt"), b"hello").unwrap();
        fs::write(
            ws.join("sub").join("deeper").join("blob.bin"),
            vec![7u8; 4096],
        )
        .unwrap();
        let recovery = dir.path().join("recovery");

        // Atomic move: the live folder is gone and the recovery copy holds the EXACT tree.
        let record = backup_and_remove_dir(&ws, "cursor", &recovery).unwrap();
        assert!(!ws.exists(), "the live folder is gone after an atomic move");
        let moved = Path::new(&record.backup_path);
        assert_eq!(
            fs::read(moved.join("state.vscdb")).unwrap(),
            [0u8, 1, 2, 3, 255]
        );
        assert_eq!(
            fs::read(moved.join("sub").join("notes.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            fs::read(moved.join("sub").join("deeper").join("blob.bin")).unwrap(),
            vec![7u8; 4096]
        );

        // Restore brings the exact tree back and consumes the recovery copy.
        restore_app_removal(&record).unwrap();
        assert_eq!(
            fs::read(ws.join("sub").join("deeper").join("blob.bin")).unwrap(),
            vec![7u8; 4096]
        );
        assert!(
            !moved.exists(),
            "a successful restore consumes the recovery copy"
        );

        // Failure path: the recovery parent is a FILE, so the move cannot proceed. The live
        // folder and every file in it must remain fully intact — no partial deletion.
        let ws2 = dir.path().join("workspace-abcd");
        fs::create_dir_all(&ws2).unwrap();
        fs::write(ws2.join("keep.json"), b"precious").unwrap();
        fs::write(ws2.join("inner.bin"), vec![9u8; 1024]).unwrap();
        let blocker = dir.path().join("blocker-is-a-file");
        fs::write(&blocker, b"not a dir").unwrap();

        let result = backup_and_remove_dir(&ws2, "cursor", &blocker.join("under"));
        assert!(
            result.is_err(),
            "a move that cannot create its recovery area must error"
        );
        assert!(ws2.exists(), "the source folder must survive a failed move");
        assert_eq!(
            fs::read(ws2.join("keep.json")).unwrap(),
            b"precious",
            "no partial deletion of the live folder"
        );
        assert_eq!(fs::read(ws2.join("inner.bin")).unwrap(), vec![9u8; 1024]);
    }
}
