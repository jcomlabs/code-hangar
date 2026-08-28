use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_PENDING_REQUESTS: usize = 64;
const MAX_INBOX_FILE_BYTES: u64 = 64 * 1024;
const MAX_INBOX_REQUEST_AGE_MS: u64 = 10 * 60 * 1_000;
const MAX_INBOX_CLOCK_SKEW_MS: u64 = 60 * 1_000;
const SHARED_INSTANCE_MUTEX: &str = "Local\\JCOMLabs.CodeHangar.Desktop.SingleInstance.v1";

#[derive(Debug, Serialize, Deserialize)]
struct DiskRequest {
    paths: Vec<String>,
    #[serde(default)]
    activate: bool,
    #[serde(default)]
    created_at_unix_ms: u64,
}

#[derive(Default)]
pub struct ShellOpenInbox {
    pending: Mutex<VecDeque<String>>,
}

impl ShellOpenInbox {
    pub fn push_many(&self, paths: impl IntoIterator<Item = String>) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        for path in paths {
            if pending.len() >= MAX_PENDING_REQUESTS {
                break;
            }
            if !pending.iter().any(|existing| same_path(existing, &path)) {
                pending.push_back(path);
            }
        }
    }

    pub fn take_pending(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|mut pending| pending.drain(..).collect())
            .unwrap_or_default()
    }
}

fn same_path(left: &str, right: &str) -> bool {
    left.replace('\\', "/")
        .eq_ignore_ascii_case(&right.replace('\\', "/"))
}

/// Positional local paths passed by Explorer/file associations. Unknown flags
/// and URLs are ignored here and rejected again by the API boundary.
pub fn paths_from_process_args() -> Vec<String> {
    std::env::args_os()
        .skip(1)
        .filter_map(valid_argument_path)
        .take(32)
        .collect()
}

pub fn background_start_requested() -> bool {
    std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--background")
}

pub fn should_activate_existing_instance(background_requested: bool, has_paths: bool) -> bool {
    !background_requested || has_paths
}

fn valid_argument_path(argument: OsString) -> Option<String> {
    let path = PathBuf::from(&argument);
    let text = path.to_string_lossy().to_string();
    // This parser runs before the hardened API boundary. Keep it purely
    // lexical: probing `exists()` here would already touch a UNC/mapped drive.
    if text.contains("://")
        || text.starts_with(r"\\")
        || text.starts_with("//")
        || !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        || !argument_uses_local_windows_drive(&path)
    {
        return None;
    }
    Some(text)
}

#[cfg(windows)]
fn argument_uses_local_windows_drive(path: &std::path::Path) -> bool {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    let root = format!("{}:\\", bytes[0] as char)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    (unsafe { GetDriveTypeW(root.as_ptr()) }) != DRIVE_REMOTE
}

#[cfg(not(windows))]
fn argument_uses_local_windows_drive(_path: &std::path::Path) -> bool {
    true
}

fn inbox_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from).map(|base| {
        base.join("local.codehangar.desktop")
            .join("shell-open-inbox")
    })
}

fn write_disk_request(paths: Vec<String>, activate: bool) -> Result<(), String> {
    if paths.is_empty() && !activate {
        return Ok(());
    }
    let directory = inbox_dir().ok_or_else(|| "APPDATA is unavailable.".to_string())?;
    std::fs::create_dir_all(&directory)
        .map_err(|err| format!("Could not create the shell-open inbox: {err}"))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let stem = format!("request-{}-{nonce}", std::process::id());
    let temporary = directory.join(format!("{stem}.tmp"));
    let destination = directory.join(format!("{stem}.json"));
    let body = serde_json::to_vec(&DiskRequest {
        paths,
        activate,
        created_at_unix_ms: unix_time_ms(),
    })
    .map_err(|err| format!("Could not encode the shell-open request: {err}"))?;
    let protected = hangar_security::protect_local_bytes(&body)
        .map_err(|err| format!("Could not protect the shell-open request: {err}"))?;
    std::fs::write(&temporary, protected)
        .map_err(|err| format!("Could not write the shell-open request: {err}"))?;
    std::fs::rename(&temporary, &destination)
        .map_err(|err| format!("Could not publish the shell-open request: {err}"))?;
    Ok(())
}

fn read_disk_requests() -> (Vec<String>, bool) {
    let Some(directory) = inbox_dir() else {
        return (Vec::new(), false);
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (Vec::new(), false);
    };
    // Non-request leftovers must not consume the bounded request budget. This
    // also prevents an abandoned `.tmp` file from starving valid JSON behind it.
    let mut request_files = Vec::with_capacity(MAX_PENDING_REQUESTS);
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.extension().and_then(|value| value.to_str()) == Some("json")
            && request_files.len() < MAX_PENDING_REQUESTS
        {
            request_files.push(entry_path);
        } else if stale_inbox_artifact(&entry_path) {
            // A crash can leave a temporary write or a claimed request behind.
            // Only our tightly named, old inbox artifacts are eligible here.
            let _ = std::fs::remove_file(entry_path);
        }
    }
    request_files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut paths = Vec::new();
    let mut activate = false;
    for file_path in request_files {
        // Claim before reading. Even if final cleanup is blocked by antivirus,
        // the original `.json` name is gone and the one-shot request cannot be
        // replayed on every 250 ms poll.
        let Some(file_name) = file_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let claim_path = file_path.with_file_name(format!(
            "{file_name}.processing-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        if std::fs::rename(&file_path, &claim_path).is_err() {
            continue;
        }
        let safe_size = std::fs::metadata(&claim_path)
            .map(|metadata| metadata.is_file() && metadata.len() <= MAX_INBOX_FILE_BYTES)
            .unwrap_or(false);
        if safe_size {
            if let Ok(protected) = std::fs::read(&claim_path) {
                if let Ok(body) = hangar_security::unprotect_local_bytes(&protected) {
                    if let Ok(request) = serde_json::from_slice::<DiskRequest>(&body) {
                        if disk_request_is_fresh(request.created_at_unix_ms, unix_time_ms()) {
                            paths.extend(request.paths.into_iter().take(32));
                            activate |= request.activate;
                        }
                    }
                }
            }
        }
        // The inbox is a one-shot local mailbox. Removing this entry never
        // touches the file/folder the request points at.
        let _ = std::fs::remove_file(claim_path);
    }
    (paths, activate)
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn disk_request_is_fresh(created_at_unix_ms: u64, now_unix_ms: u64) -> bool {
    created_at_unix_ms > 0
        && created_at_unix_ms <= now_unix_ms.saturating_add(MAX_INBOX_CLOCK_SKEW_MS)
        && now_unix_ms.saturating_sub(created_at_unix_ms) <= MAX_INBOX_REQUEST_AGE_MS
}

fn stale_inbox_artifact(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if !is_owned_inbox_artifact_name(name) {
        return false;
    }
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_millis() as u64 > MAX_INBOX_REQUEST_AGE_MS)
}

fn is_owned_inbox_artifact_name(name: &str) -> bool {
    name.starts_with("request-") && (name.ends_with(".tmp") || name.contains(".json.processing-"))
}

#[cfg(feature = "agent_automation")]
fn edition_instance_mutex() -> &'static str {
    "Local\\JCOMLabs.CodeHangar.Desktop.Edition.Connector.v1"
}

#[cfg(not(feature = "agent_automation"))]
fn edition_instance_mutex() -> &'static str {
    "Local\\JCOMLabs.CodeHangar.Desktop.Edition.Local.v1"
}

#[cfg(windows)]
pub fn become_primary_or_forward(
    paths: Vec<String>,
    activate_existing: bool,
) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mutex_name = wide(SHARED_INSTANCE_MUTEX);
    let restart_handoff = std::env::var_os("CODEHANGAR_RESTART_AFTER_PID").is_some();

    for attempt in 0..=100 {
        // SAFETY: the name is a valid, NUL-terminated UTF-16 buffer and the security
        // attributes pointer is intentionally null (current-user local namespace).
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        if handle.is_null() {
            return Err("Could not create the Code Hangar instance lock.".to_string());
        }
        // GetLastError must be read immediately after CreateMutexW.
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if !already_running {
            let edition_name = wide(edition_instance_mutex());
            let edition_handle =
                unsafe { CreateMutexW(std::ptr::null(), 0, edition_name.as_ptr()) };
            if edition_handle.is_null() {
                unsafe {
                    CloseHandle(handle);
                }
                return Err("Could not create the Code Hangar edition lock.".to_string());
            }
            let edition_already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            if edition_already_running {
                unsafe {
                    CloseHandle(edition_handle);
                    CloseHandle(handle);
                }
                return Err("The Code Hangar edition lock is already owned.".to_string());
            }
            // Keep the mutex for the lifetime of the process. `HANDLE` is a raw
            // value rather than an RAII owner, so leaving this scope does not close
            // either the shared-catalog or edition lease; Windows releases both
            // when the process exits.
            return Ok(true);
        }

        // This process did not create the named mutex, so it does not own the
        // single-instance lease. Close only its local reference before either
        // retrying a deliberate restart hand-off or forwarding to the owner.
        unsafe {
            CloseHandle(handle);
        }
        if restart_handoff && attempt < 100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }

        let edition_name = wide(edition_instance_mutex());
        let edition_probe = unsafe { CreateMutexW(std::ptr::null(), 0, edition_name.as_ptr()) };
        if edition_probe.is_null() {
            return Err("Could not inspect the running Code Hangar edition.".to_string());
        }
        let same_edition = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        unsafe {
            CloseHandle(edition_probe);
        }
        if !same_edition && paths.is_empty() {
            if activate_existing {
                show_other_edition_notice();
            }
            return Ok(false);
        }
        // A path request is edition-neutral and may safely open in the other
        // installed edition. Plain launches never raise the wrong edition: they
        // receive the explicit switch notice above instead.
        write_disk_request(paths, activate_existing)?;
        return Ok(false);
    }

    Err("Could not complete the Code Hangar restart hand-off.".to_string())
}

#[cfg(not(windows))]
pub fn become_primary_or_forward(
    _paths: Vec<String>,
    _activate_existing: bool,
) -> Result<bool, String> {
    Ok(true)
}

#[cfg(windows)]
fn show_other_edition_notice() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };
    let title = wide("Code Hangar edition already running");
    let message = wide(
        "The other Code Hangar edition is using the shared local catalog. Exit it from the notification-area menu, then open this edition again. Your catalog will be preserved.",
    );
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn initialize_primary(app: tauri::AppHandle, initial_paths: Vec<String>) {
    use tauri::{Emitter, Manager};

    let inbox = app.state::<ShellOpenInbox>();
    inbox.push_many(initial_paths);
    let (initial_disk_paths, initial_activate) = read_disk_requests();
    inbox.push_many(initial_disk_paths);
    if initial_activate {
        crate::resident::show_main_window(&app);
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let (paths, activate) = read_disk_requests();
        if paths.is_empty() && !activate {
            continue;
        }
        app.state::<ShellOpenInbox>().push_many(paths);
        crate::resident::show_main_window(&app);
        // The event contains no path. The frontend explicitly drains the managed
        // queue through a Tauri command, keeping path data off the event bus.
        let _ = app.emit("shell-open-available", ());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_deduplicates_case_and_separator_variants() {
        let inbox = ShellOpenInbox::default();
        inbox.push_many([
            r"C:\Work\Project\README.md".to_string(),
            "c:/work/project/readme.md".to_string(),
        ]);
        assert_eq!(inbox.take_pending(), vec![r"C:\Work\Project\README.md"]);
    }

    #[test]
    fn non_path_arguments_are_ignored() {
        assert_eq!(valid_argument_path(OsString::from("--inspect")), None);
        assert_eq!(
            valid_argument_path(OsString::from("codehangar://project/readme.md")),
            None
        );
        assert_eq!(
            valid_argument_path(OsString::from(r"\\server\share\README.md")),
            None
        );
        assert_eq!(
            valid_argument_path(OsString::from("//server/share/README.md")),
            None
        );
    }

    #[test]
    fn background_handoff_stays_quiet_unless_it_carries_a_path() {
        assert!(!should_activate_existing_instance(true, false));
        assert!(should_activate_existing_instance(true, true));
        assert!(should_activate_existing_instance(false, false));
    }

    #[test]
    fn disk_requests_are_bounded_by_age_and_clock_skew() {
        let now = 2_000_000_u64;
        assert!(disk_request_is_fresh(now, now));
        assert!(disk_request_is_fresh(
            now.saturating_sub(MAX_INBOX_REQUEST_AGE_MS),
            now
        ));
        assert!(!disk_request_is_fresh(
            now.saturating_sub(MAX_INBOX_REQUEST_AGE_MS + 1),
            now
        ));
        assert!(disk_request_is_fresh(now + MAX_INBOX_CLOCK_SKEW_MS, now));
        assert!(!disk_request_is_fresh(
            now + MAX_INBOX_CLOCK_SKEW_MS + 1,
            now
        ));
        assert!(!disk_request_is_fresh(0, now));
    }

    #[test]
    fn inbox_cleanup_recognizes_only_owned_temporary_names() {
        assert!(is_owned_inbox_artifact_name("request-1-2.tmp"));
        assert!(is_owned_inbox_artifact_name(
            "request-1-2.json.processing-3-4"
        ));
        assert!(!is_owned_inbox_artifact_name("unrelated.tmp"));
        assert!(!is_owned_inbox_artifact_name("request-not-ours.txt"));
    }

    #[test]
    fn editions_use_distinct_probe_leases_behind_the_shared_catalog_lease() {
        assert_ne!(edition_instance_mutex(), SHARED_INSTANCE_MUTEX);
        if cfg!(feature = "agent_automation") {
            assert!(edition_instance_mutex().contains("Connector"));
        } else {
            assert!(edition_instance_mutex().contains("Local"));
        }
    }
}
