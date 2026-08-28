#![cfg(windows)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use hangar_api::AppState;
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("Catalog acceptance helper failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    run_with_guard(&args, ensure_sandbox_for_register)
}

fn run_with_guard<F>(args: &[String], register_guard: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let [mode, db_path, project_path, report_path] = args else {
        return Err(
            "usage: acceptance_catalog <register|check> <database> <project> <report.json>"
                .to_string(),
        );
    };
    if mode != "register" && mode != "check" {
        return Err(format!("unknown mode: {mode}"));
    }
    if mode == "register" {
        // AppState::open performs migrations and startup reset handling, so the sandbox
        // refusal must happen before crossing that mutating boundary.
        register_guard()?;
    }

    let db_path = absolute(Path::new(db_path))?;
    let project_path = fs::canonicalize(project_path).map_err(to_message)?;
    let report_path = absolute(Path::new(report_path))?;
    let state = AppState::open(&db_path)?;
    wait_for_ready(&state)?;

    let scan = if mode == "register" {
        let root = hangar_api::roots_add(&state, project_path.to_string_lossy().to_string())?;
        let job_id =
            hangar_api::scan_start(&state, Some(vec![root.id]), Some("balanced".to_string()))?;
        let status = wait_for_scan(&state, &job_id)?;
        if status.state != "completed" || status.partial {
            return Err(format!(
                "fixture scan did not complete cleanly: state={}, partial={}, error={:?}",
                status.state, status.partial, status.error
            ));
        }
        Some(status)
    } else {
        None
    };

    let expected = normalized(&project_path);
    let project = hangar_api::projects_list(&state)?
        .into_iter()
        .find(|project| normalized(Path::new(&project.path)) == expected)
        .ok_or_else(|| {
            format!(
                "registered lifecycle project is missing: {}",
                project_path.display()
            )
        })?;
    let roots = hangar_api::roots_list(&state)?;
    let root = roots
        .iter()
        .find(|root| normalized(Path::new(&root.path)) == expected)
        .ok_or_else(|| "lifecycle scan root is missing".to_string())?;
    let context = hangar_api::project_context_files(&state, project.id)?;
    let context_paths: Vec<String> = context.iter().map(|file| file.path.clone()).collect();
    for expected_context in ["README.md", "AGENTS.md"] {
        if !context_paths
            .iter()
            .any(|path| path.eq_ignore_ascii_case(expected_context))
        {
            return Err(format!(
                "missing context file after {mode}: {expected_context}"
            ));
        }
    }

    let report = json!({
        "schemaVersion": 1,
        "status": "PASS",
        "mode": mode,
        "database": db_path,
        "project": {
            "id": project.id,
            "name": project.name,
            "path": project.path,
            "scanState": project.scan_state,
            "contextCount": project.context_count,
        },
        "root": root,
        "contextFiles": context_paths,
        "scan": scan,
    });
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).map_err(to_message)?;
    }
    let mut text = serde_json::to_string_pretty(&report).map_err(to_message)?;
    text.push('\n');
    fs::write(&report_path, text).map_err(to_message)?;
    println!(
        "Catalog lifecycle evidence written to {}",
        report_path.display()
    );
    Ok(())
}

/// Fail-closed sandbox guard for the MUTATING `register` mode. `register` calls
/// `roots_add` + `scan_start`, which write to and scan the catalog DB passed on the command
/// line — the acceptance harness points that at the REAL
/// `%APPDATA%\local.codehangar.desktop\codehangar.sqlite3`. So `register` must only ever run
/// inside the disposable Windows Sandbox: accept the WDAGUtilityAccount auto-logon, or the
/// `CODEHANGAR_SANDBOX_AGENT=1` sentinel the host sets in the .wsb LogonCommand. The read-only
/// `check` mode is intentionally NOT gated, so recorded evidence can be re-validated anywhere.
fn ensure_sandbox_for_register() -> Result<(), String> {
    let in_sandbox = env::var("USERNAME")
        .map(|user| user.eq_ignore_ascii_case("WDAGUtilityAccount"))
        .unwrap_or(false)
        || env::var("CODEHANGAR_SANDBOX_AGENT")
            .map(|flag| flag == "1")
            .unwrap_or(false);
    if !in_sandbox {
        return Err(
            "Refusing to run 'register' outside Windows Sandbox: it scans and writes the catalog \
             database passed on the command line (the acceptance harness targets the real \
             %APPDATA% catalog). Run inside the acceptance Sandbox, or use the read-only 'check' mode."
                .to_string(),
        );
    }
    Ok(())
}

fn wait_for_ready(state: &AppState) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = hangar_api::startup_status(state);
        match status.state.as_str() {
            "ready" => return Ok(()),
            "failed" => return Err(format!("inventory open failed: {}", status.message)),
            _ if Instant::now() >= deadline => {
                return Err("inventory did not become ready within 30 seconds".to_string())
            }
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn wait_for_scan(state: &AppState, job_id: &str) -> Result<hangar_core::ScanStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = hangar_api::scan_status(state, job_id.to_string())?;
        if matches!(status.state.as_str(), "completed" | "failed" | "cancelled") {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(format!("scan did not finish within 120 seconds: {job_id}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn normalized(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    if let Some(path) = normalized.strip_prefix(r"\\?\unc\") {
        format!(r"\\{path}")
    } else {
        normalized
            .strip_prefix(r"\\?\")
            .unwrap_or(&normalized)
            .to_string()
    }
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map_err(to_message)
            .map(|current| current.join(path))
    }
}

fn to_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_register_never_opens_or_creates_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("must-not-exist.sqlite3");
        let report = dir.path().join("report.json");
        let args = vec![
            "register".to_string(),
            db.to_string_lossy().to_string(),
            dir.path().to_string_lossy().to_string(),
            report.to_string_lossy().to_string(),
        ];

        let error = run_with_guard(&args, || Err("outside sandbox".to_string())).unwrap_err();

        assert_eq!(error, "outside sandbox");
        assert!(!db.exists(), "the guard must run before AppState::open");
        assert!(!report.exists());
    }
}
