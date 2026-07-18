use hangar_core::{
    ControlledCheckRun, CorrectionCheckItem, CorrectionStaticCheckReport, ProjectCheckDefinition,
};
use pulldown_cmark::{Event, Parser, Tag};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CHECKED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
const CHECK_TIMEOUT_SECONDS: u64 = 120;
const CHECK_MEMORY_LIMIT_MIB: u64 = 2048;
const CHECK_PROCESS_LIMIT: u32 = 32;
const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const RISK_DISCLOSURE: &str = "This runs code supplied by this project. A known command is not a sandbox and does not make the project harmless. Approval applies only to this project and this exact manifest fingerprint.";

#[derive(Clone)]
struct CommandSpec {
    definition: ProjectCheckDefinition,
    executable: String,
    args: Vec<String>,
}

pub(crate) fn static_correction_check(
    state: &super::AppState,
    node_id: i64,
) -> Result<CorrectionStaticCheckReport, String> {
    let (path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&path, &project_paths)?;
    let project_id = state
        .db()?
        .node_project_id(node_id)
        .map_err(super::to_message)?
        .ok_or_else(|| {
            "Static check unavailable: this file is no longer attached to a project.".to_string()
        })?;
    let metadata = fs::metadata(&path).map_err(|error| {
        format!("Static check unavailable: the file could not be inspected ({error}).")
    })?;
    if metadata.len() > MAX_CHECKED_FILE_BYTES {
        return Err(
            "Static check unavailable: this file is above the safe analysis size limit."
                .to_string(),
        );
    }
    let source = String::from_utf8(fs::read(&path).map_err(|error| {
        format!("Static check unavailable: the file could not be read ({error}).")
    })?)
    .map_err(|_| "Static check unavailable: this file is not UTF-8 text.".to_string())?;

    let structural = structural_check(&path, &source);
    let references = reference_check(state, node_id, project_id, Path::new(&path), &source)?;
    let status = if structural.status == "failed" {
        "failed"
    } else if references.status == "warning" || structural.status == "warning" {
        "warning"
    } else {
        "passed"
    };
    Ok(CorrectionStaticCheckReport {
        node_id,
        project_id,
        path,
        status: status.to_string(),
        checks: vec![structural, references],
        checked_at: chrono::Utc::now().to_rfc3339(),
        executed_project_code: false,
    })
}

fn structural_check(path: &str, source: &str) -> CorrectionCheckItem {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "md" | "markdown" | "mdx") {
        let token_count = Parser::new(source).count();
        return CorrectionCheckItem {
            id: "structural".to_string(),
            label: "Structural parse".to_string(),
            status: "passed".to_string(),
            detail: format!("Markdown parsed locally into {token_count} token(s)."),
        };
    }
    let supported = matches!(
        extension.as_str(),
        "json"
            | "toml"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "py"
            | "pyw"
            | "rs"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "java"
            | "kt"
            | "kts"
            | "cs"
            | "css"
            | "scss"
    );
    if !supported {
        return CorrectionCheckItem {
            id: "structural".to_string(),
            label: "Structural parse".to_string(),
            status: "not_applicable".to_string(),
            detail: "No deterministic structural parser is available for this file type."
                .to_string(),
        };
    }
    match super::value_edit::validate_content_after_edit(path, source) {
        Ok(()) => CorrectionCheckItem {
            id: "structural".to_string(),
            label: "Structural parse".to_string(),
            status: "passed".to_string(),
            detail: "The complete file passed its local format/source validity guard.".to_string(),
        },
        Err(detail) => CorrectionCheckItem {
            id: "structural".to_string(),
            label: "Structural parse".to_string(),
            status: "failed".to_string(),
            detail,
        },
    }
}

fn reference_check(
    state: &super::AppState,
    node_id: i64,
    project_id: i64,
    file_path: &Path,
    source: &str,
) -> Result<CorrectionCheckItem, String> {
    let relationships = state
        .db()?
        .node_relationships(node_id)
        .map_err(super::to_message)?;
    let root = canonical_project_root(state, project_id)?;
    let mut missing = Vec::new();
    let extension = file_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "md" | "markdown" | "mdx") {
        for event in Parser::new(source) {
            let target = match event {
                Event::Start(Tag::Link { dest_url, .. })
                | Event::Start(Tag::Image { dest_url, .. }) => dest_url.into_string(),
                _ => continue,
            };
            if let Some(reason) = local_link_problem(&root, file_path, &target) {
                missing.push(reason);
            }
            if missing.len() >= 20 {
                break;
            }
        }
    }
    let issue_count = relationships.issues.len();
    if missing.is_empty() && issue_count == 0 {
        return Ok(CorrectionCheckItem {
            id: "references".to_string(),
            label: "References and links".to_string(),
            status: "passed".to_string(),
            detail: format!(
                "No missing local link or indexed relationship issue was found ({} outgoing, {} incoming).",
                relationships.outgoing.len(),
                relationships.incoming.len()
            ),
        });
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!(
            "Missing or outside-project local links: {}.",
            missing.join(", ")
        ));
    }
    if issue_count > 0 {
        let examples = relationships
            .issues
            .iter()
            .take(5)
            .map(|issue| issue.target.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        details.push(format!(
            "Indexed relationship issues: {issue_count} ({examples})."
        ));
    }
    Ok(CorrectionCheckItem {
        id: "references".to_string(),
        label: "References and links".to_string(),
        status: "warning".to_string(),
        detail: details.join(" "),
    })
}

fn local_link_problem(root: &Path, file_path: &Path, target: &str) -> Option<String> {
    let trimmed = target.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || trimmed.contains("://")
        || ["mailto:", "data:", "javascript:"].iter().any(|prefix| {
            trimmed
                .get(..prefix.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
        })
    {
        return None;
    }
    let without_fragment = trimmed.split_once('#').map_or(trimmed, |(path, _)| path);
    let without_fragment = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(path, _)| path);
    if without_fragment.is_empty() {
        return None;
    }
    let decoded =
        decode_link_path(without_fragment).unwrap_or_else(|| without_fragment.to_string());
    let candidate = if decoded.starts_with('/') || decoded.starts_with('\\') {
        root.join(decoded.trim_start_matches(['/', '\\']))
    } else {
        file_path.parent().unwrap_or(root).join(&decoded)
    };
    if !candidate.exists() {
        return Some(target.to_string());
    }
    match fs::canonicalize(&candidate) {
        Ok(canonical) if canonical.starts_with(root) => None,
        _ => Some(target.to_string()),
    }
}

fn decode_link_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push((hex_digit(hi)? << 4) | hex_digit(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(feature = "mutation")]
pub(crate) fn detect_project_checks(
    state: &super::AppState,
    project_id: i64,
) -> Result<Vec<ProjectCheckDefinition>, String> {
    Ok(detect_specs(state, project_id)?
        .into_iter()
        .map(|spec| spec.definition)
        .collect())
}

#[cfg(feature = "mutation")]
pub(crate) fn approve_project_check(
    state: &super::AppState,
    project_id: i64,
    check_id: &str,
    fingerprint: &str,
) -> Result<ProjectCheckDefinition, String> {
    validate_check_identity(check_id, fingerprint)?;
    let mut specs = detect_specs(state, project_id)?;
    let spec = specs
        .iter_mut()
        .find(|candidate| candidate.definition.id == check_id)
        .ok_or_else(|| "That project check is no longer detected.".to_string())?;
    if spec.definition.fingerprint != fingerprint {
        return Err(
            "Approval refused: the project manifest changed. Review the detected check again."
                .to_string(),
        );
    }
    let approved_at = state
        .db()?
        .set_project_check_approval(project_id, check_id, fingerprint)
        .map_err(super::to_message)?;
    spec.definition.approved = true;
    spec.definition.approved_at = Some(approved_at);
    Ok(spec.definition.clone())
}

#[cfg(feature = "mutation")]
pub(crate) fn revoke_project_check_approval(
    state: &super::AppState,
    project_id: i64,
    check_id: &str,
) -> Result<bool, String> {
    if check_id.is_empty() || check_id.len() > 64 {
        return Err("That project check id is not valid.".to_string());
    }
    state
        .db()?
        .revoke_project_check_approval(project_id, check_id)
        .map_err(super::to_message)
}

#[cfg(feature = "mutation")]
pub(crate) fn run_project_check(
    state: &super::AppState,
    project_id: i64,
    node_id: i64,
    check_id: &str,
    fingerprint: &str,
) -> Result<ControlledCheckRun, String> {
    validate_check_identity(check_id, fingerprint)?;
    if state
        .db()?
        .node_project_id(node_id)
        .map_err(super::to_message)?
        != Some(project_id)
    {
        return Err(
            "Check refused: the correction file is not attached to this project.".to_string(),
        );
    }
    let specs = detect_specs(state, project_id)?;
    let spec = specs
        .into_iter()
        .find(|candidate| candidate.definition.id == check_id)
        .ok_or_else(|| "That project check is no longer detected.".to_string())?;
    if spec.definition.fingerprint != fingerprint {
        return Err("Check refused: the project manifest changed after approval.".to_string());
    }
    if !spec.definition.approved {
        return Err("Check refused: approve this exact project check first.".to_string());
    }
    let rollback_snapshot_id = super::edit_snapshot::list_snapshots(state, node_id, 1)?
        .first()
        .map(|snapshot| snapshot.id);
    let root = canonical_project_root(state, project_id)?;
    let process = run_bounded_process(&root, &spec)?;
    let (stdout, stdout_redactions) = super::redact_secrets(&process.stdout);
    let (stderr, stderr_redactions) = super::redact_secrets(&process.stderr);
    let status = if process.timed_out {
        "timed_out"
    } else if process.exit_code == Some(0) {
        "passed"
    } else {
        "failed"
    };
    let redaction_note = if stdout_redactions + stderr_redactions > 0 {
        format!(
            " {} possible secret(s) were redacted from local output.",
            stdout_redactions + stderr_redactions
        )
    } else {
        String::new()
    };
    Ok(ControlledCheckRun {
        project_id,
        node_id,
        check_id: spec.definition.id,
        label: spec.definition.label,
        command_label: spec.definition.command_label,
        status: status.to_string(),
        exit_code: process.exit_code,
        duration_ms: process.duration_ms,
        stdout,
        stderr: format!("{}{redaction_note}", stderr),
        output_truncated: process.output_truncated,
        rollback_snapshot_id,
        rollback_available: rollback_snapshot_id.is_some(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        limits_summary: format!(
            "{}s wall timeout, {} MiB job memory, {} processes maximum, below-normal priority, 64 KiB retained per output stream.",
            CHECK_TIMEOUT_SECONDS, CHECK_MEMORY_LIMIT_MIB, CHECK_PROCESS_LIMIT
        ),
    })
}

#[cfg(feature = "mutation")]
fn validate_check_identity(check_id: &str, fingerprint: &str) -> Result<(), String> {
    if check_id.is_empty()
        || check_id.len() > 64
        || !check_id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b':' | b'-' | b'_'))
    {
        return Err("That project check id is not valid.".to_string());
    }
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err("That project check fingerprint is not valid.".to_string());
    }
    Ok(())
}

#[cfg(feature = "mutation")]
fn detect_specs(state: &super::AppState, project_id: i64) -> Result<Vec<CommandSpec>, String> {
    let root = canonical_project_root(state, project_id)?;
    let mut specs = Vec::new();
    if let Some(bytes) = read_small_manifest(&root.join("package.json"))? {
        let package: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!("Project checks unavailable: package.json is invalid ({error}).")
        })?;
        if let Some(scripts) = package.get("scripts").and_then(|value| value.as_object()) {
            for (script, id, label, args) in [
                ("test", "npm:test", "Project tests", vec!["test"]),
                ("build", "npm:build", "Project build", vec!["run", "build"]),
            ] {
                if scripts
                    .get(script)
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    specs.push(make_spec(
                        state,
                        project_id,
                        id,
                        label,
                        &format!("npm {script}"),
                        "package.json",
                        &bytes,
                        "npm.cmd",
                        &args,
                    )?);
                }
            }
        }
    }
    if let Some(bytes) = read_small_manifest(&root.join("Cargo.toml"))? {
        specs.push(make_spec(
            state,
            project_id,
            "cargo:check",
            "Rust compile check",
            "cargo check",
            "Cargo.toml",
            &bytes,
            "cargo.exe",
            &["check"],
        )?);
        specs.push(make_spec(
            state,
            project_id,
            "cargo:test",
            "Rust tests",
            "cargo test",
            "Cargo.toml",
            &bytes,
            "cargo.exe",
            &["test"],
        )?);
    }
    if let Some(bytes) = read_small_manifest(&root.join("go.mod"))? {
        specs.push(make_spec(
            state,
            project_id,
            "go:test",
            "Go tests",
            "go test ./...",
            "go.mod",
            &bytes,
            "go.exe",
            &["test", "./..."],
        )?);
    }
    let pytest_manifest = if root.join("pytest.ini").is_file() {
        Some(("pytest.ini", root.join("pytest.ini")))
    } else if root.join("pyproject.toml").is_file() {
        Some(("pyproject.toml", root.join("pyproject.toml")))
    } else {
        None
    };
    if let Some((manifest_name, manifest_path)) = pytest_manifest {
        if let Some(bytes) = read_small_manifest(&manifest_path)? {
            let text = String::from_utf8_lossy(&bytes);
            if manifest_name == "pytest.ini" || text.contains("[tool.pytest.ini_options]") {
                specs.push(make_spec(
                    state,
                    project_id,
                    "python:pytest",
                    "Python tests",
                    "python -m pytest",
                    manifest_name,
                    &bytes,
                    "python.exe",
                    &["-m", "pytest"],
                )?);
            }
        }
    }
    Ok(specs)
}

#[cfg(feature = "mutation")]
#[allow(clippy::too_many_arguments)]
fn make_spec(
    state: &super::AppState,
    project_id: i64,
    id: &str,
    label: &str,
    command_label: &str,
    manifest_path: &str,
    manifest: &[u8],
    executable: &str,
    args: &[&str],
) -> Result<CommandSpec, String> {
    let mut hasher = blake3::Hasher::new();
    for value in [
        "controlled-check-v1",
        id,
        command_label,
        manifest_path,
        executable,
    ] {
        hasher.update(value.as_bytes());
        hasher.update(&[0]);
    }
    hasher.update(manifest);
    for arg in args {
        hasher.update(&[0]);
        hasher.update(arg.as_bytes());
    }
    let fingerprint = hasher.finalize().to_hex().to_string();
    let approval = state
        .db()?
        .project_check_approval(project_id, id)
        .map_err(super::to_message)?;
    let approved = approval
        .as_ref()
        .is_some_and(|(stored, _)| stored == &fingerprint);
    let approved_at = approval
        .as_ref()
        .filter(|(stored, _)| stored == &fingerprint)
        .map(|(_, approved_at)| approved_at.clone());
    Ok(CommandSpec {
        definition: ProjectCheckDefinition {
            id: id.to_string(),
            label: label.to_string(),
            command_label: command_label.to_string(),
            manifest_path: manifest_path.to_string(),
            fingerprint,
            approved,
            approved_at,
            timeout_seconds: CHECK_TIMEOUT_SECONDS,
            memory_limit_mib: CHECK_MEMORY_LIMIT_MIB,
            process_limit: CHECK_PROCESS_LIMIT,
            risk_disclosure: RISK_DISCLOSURE.to_string(),
        },
        executable: executable.to_string(),
        args: args.iter().map(|value| (*value).to_string()).collect(),
    })
}

fn canonical_project_root(state: &super::AppState, project_id: i64) -> Result<PathBuf, String> {
    let project = super::project_get(state, project_id)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let root = fs::canonicalize(&project.path).map_err(|error| {
        format!("Project checks unavailable: the project root could not be opened ({error}).")
    })?;
    if !root.is_dir() {
        return Err(
            "Project checks unavailable: the registered project root is not a folder.".to_string(),
        );
    }
    Ok(root)
}

fn read_small_manifest(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Project checks unavailable: a manifest could not be inspected ({error})."
            ))
        }
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "Project checks unavailable: {} is unexpectedly large.",
            path.display()
        ));
    }
    fs::read(path).map(Some).map_err(|error| {
        format!("Project checks unavailable: a manifest could not be read ({error}).")
    })
}

#[cfg(feature = "mutation")]
struct ProcessResult {
    exit_code: Option<i32>,
    duration_ms: u64,
    stdout: String,
    stderr: String,
    output_truncated: bool,
    timed_out: bool,
}

#[cfg(all(feature = "mutation", windows))]
fn run_bounded_process(root: &Path, spec: &CommandSpec) -> Result<ProcessResult, String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    static CHECK_LOCK: Mutex<()> = Mutex::new(());
    let _guard = CHECK_LOCK.try_lock().map_err(|_| {
        "Another project check is already running. Wait for it to finish.".to_string()
    })?;

    struct Job(HANDLE);
    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(format!(
            "Project check could not create its resource boundary: {}.",
            std::io::Error::last_os_error()
        ));
    }
    let job = Job(handle);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_JOB_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = CHECK_PROCESS_LIMIT;
    limits.JobMemoryLimit = (CHECK_MEMORY_LIMIT_MIB * 1024 * 1024) as usize;
    let configured = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(format!(
            "Project check could not configure its resource boundary: {}.",
            std::io::Error::last_os_error()
        ));
    }

    let inherited = [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
    .collect::<Vec<_>>();
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .current_dir(root)
        .env_clear()
        .envs(inherited)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("CARGO_NET_OFFLINE", "true")
        .env("npm_config_offline", "true")
        .env("npm_config_audit", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Project code must not execute in the gap between CreateProcess and job assignment.
        // CREATE_SUSPENDED leaves the primary thread stopped until the Job Object owns it.
        .creation_flags(controlled_check_creation_flags());
    let started = Instant::now();
    let mut child = command.spawn().map_err(|error| {
        format!(
            "Project check could not start {} ({error}).",
            spec.definition.command_label
        )
    })?;
    let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
    if assigned == 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "Project check refused to run without its resource boundary: {}.",
            std::io::Error::last_os_error()
        ));
    }
    if let Err(error) = resume_suspended_process(&child) {
        let _ = unsafe { TerminateJobObject(job.0, 125) };
        let _ = child.wait();
        return Err(error);
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Project check stdout could not be captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Project check stderr could not be captured.".to_string())?;
    let stdout_thread = std::thread::spawn(move || read_limited(stdout));
    let stderr_thread = std::thread::spawn(move || read_limited(stderr));
    let deadline = started + Duration::from_secs(CHECK_TIMEOUT_SECONDS);
    let (exit_code, timed_out) = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Project check status could not be read ({error})."))?
        {
            break (status.code(), false);
        }
        if Instant::now() >= deadline {
            let terminated = unsafe { TerminateJobObject(job.0, 124) };
            if terminated == 0 {
                let error = std::io::Error::last_os_error();
                let _ = child.kill();
                return Err(format!(
                    "Timed-out project check could not terminate its resource boundary ({error})."
                ));
            }
            let status = child.wait().map_err(|error| {
                format!("Timed-out project check could not be reaped ({error}).")
            })?;
            break (status.code(), true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (stdout_bytes, stdout_truncated) = stdout_thread
        .join()
        .map_err(|_| "Project check stdout reader failed.".to_string())?;
    let (stderr_bytes, stderr_truncated) = stderr_thread
        .join()
        .map_err(|_| "Project check stderr reader failed.".to_string())?;
    Ok(ProcessResult {
        exit_code,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        output_truncated: stdout_truncated || stderr_truncated,
        timed_out,
    })
}

#[cfg(all(feature = "mutation", windows))]
fn controlled_check_creation_flags() -> u32 {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    };
    CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS | CREATE_SUSPENDED
}

#[cfg(all(feature = "mutation", windows))]
fn resume_suspended_process(child: &std::process::Child) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    struct Handle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Project check could not inspect its suspended process: {}.",
            std::io::Error::last_os_error()
        ));
    }
    let snapshot = Handle(snapshot);
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(format!(
                    "Project check could not open its suspended thread: {}.",
                    std::io::Error::last_os_error()
                ));
            }
            let thread = Handle(thread);
            let previous_count = unsafe { ResumeThread(thread.0) };
            if previous_count == u32::MAX {
                return Err(format!(
                    "Project check could not resume inside its resource boundary: {}.",
                    std::io::Error::last_os_error()
                ));
            }
            return Ok(());
        }
        found = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    Err(
        "Project check could not find its suspended primary thread; nothing was executed."
            .to_string(),
    )
}

#[cfg(all(feature = "mutation", windows))]
fn read_limited(mut reader: impl std::io::Read) -> (Vec<u8>, bool) {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let remaining = OUTPUT_LIMIT_BYTES.saturating_sub(retained.len());
        if remaining > 0 {
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if read > remaining {
            truncated = true;
        }
    }
    (retained, truncated)
}

#[cfg(all(feature = "mutation", not(windows)))]
fn run_bounded_process(_root: &Path, _spec: &CommandSpec) -> Result<ProcessResult, String> {
    Err("Controlled project checks are currently available only on Windows.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "mutation", windows))]
    #[test]
    fn project_process_cannot_execute_before_job_assignment_and_resume() {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("project-code-ran");
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[IO.File]::WriteAllText($env:CODEHANGAR_SUSPEND_MARKER, 'ran')",
            ])
            .env("CODEHANGAR_SUSPEND_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(controlled_check_creation_flags());
        let mut child = command.spawn().unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !marker.exists(),
            "the project command executed while its primary thread should be suspended"
        );

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!job.is_null());
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            let _ = child.kill();
            let _ = child.wait();
            unsafe { CloseHandle(job) };
            panic!(
                "test process could not be assigned: {}",
                std::io::Error::last_os_error()
            );
        }
        resume_suspended_process(&child).unwrap();
        let status = child.wait().unwrap();
        unsafe { CloseHandle(job) };
        assert!(status.success());
        assert!(
            marker.exists(),
            "the command did not run after explicit resume"
        );
    }

    #[test]
    fn percent_decodes_local_link_paths_without_touching_remote_urls() {
        assert_eq!(
            decode_link_path("docs/My%20File.md").as_deref(),
            Some("docs/My File.md")
        );
        assert!(decode_link_path("bad%2").is_none());
    }

    #[test]
    fn check_identity_rejects_command_shaped_input() {
        #[cfg(feature = "mutation")]
        {
            assert!(validate_check_identity("npm:test", &"a".repeat(64)).is_ok());
            assert!(validate_check_identity("npm test && calc", &"a".repeat(64)).is_err());
            assert!(validate_check_identity("npm:test", "short").is_err());
        }
    }
}
