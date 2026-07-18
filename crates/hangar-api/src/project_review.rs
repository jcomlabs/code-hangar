use hangar_core::{
    ExportResult, ProjectReviewCheckpoint, ReviewLedgerEntry, SessionChangeCoverage,
    SessionChangeSet, SessionFileReality,
};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const GIT_OUTPUT_CAP: usize = 4 * 1024 * 1024;
const REALITY_READ_CAP: u64 = 2 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(8);

struct BoundedCommandOutput {
    text: String,
    truncated: bool,
}

pub(crate) fn project_session_change_set(
    state: &super::AppState,
    project_id: i64,
    path: String,
) -> Result<SessionChangeSet, String> {
    read_project_session_change_set(state, project_id, path, true)
}

fn read_project_session_change_set(
    state: &super::AppState,
    project_id: i64,
    path: String,
    retain: bool,
) -> Result<SessionChangeSet, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let (canonical, fragment) = super::resolve_allowed_session_file(&path)?;
    let mut change_set = if let Some(composer_id) = fragment
        .as_deref()
        .and_then(|value| value.strip_prefix("cursor-ide-chat="))
    {
        super::session_changes::build_cursor_change_set(
            path.clone(),
            hangar_discovery::cursor_ide_chat_changes(&canonical, composer_id)?,
        )
    } else if fragment.is_some()
        || canonical.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        super::session_changes::unsupported_change_set(path.clone())
    } else {
        super::session_changes::build_session_change_set(&canonical, path.clone())?
    };
    reconcile_change_set(&mut change_set, Path::new(&project.path));
    let modified_ms = fs::metadata(&canonical)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_ms);
    if retain {
        let ledger_copy = ledger_safe_change_set(&change_set, Path::new(&project.path));
        state
            .db()?
            .store_review_evidence(project_id, &path, modified_ms, &ledger_copy)
            .map_err(super::to_message)?;
    }
    Ok(change_set)
}

pub(crate) fn project_git_change_set(
    state: &super::AppState,
    project_id: i64,
) -> Result<SessionChangeSet, String> {
    read_project_git_change_set(state, project_id, true)
}

/// Current index/working-tree evidence for correction review. Unlike Recap it
/// neither includes commit history nor retains a ledger entry merely because a
/// user opened a review dialog.
#[cfg(feature = "mutation")]
pub(crate) fn current_project_git_change_set(
    state: &super::AppState,
    project_id: i64,
) -> Result<SessionChangeSet, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    read_git_change_set(Path::new(&project.path), None)
}

fn read_project_git_change_set(
    state: &super::AppState,
    project_id: i64,
    retain: bool,
) -> Result<SessionChangeSet, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let root = Path::new(&project.path);
    let checkpoint = state
        .db()?
        .project_review_checkpoint(project_id)
        .map_err(super::to_message)?;
    let mut change_set = read_git_change_set(
        root,
        checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.git_head.as_deref()),
    )?;
    reconcile_git_change_set(&mut change_set);
    let observed_ms = system_time_ms(SystemTime::now());
    if retain {
        let ledger_copy = ledger_safe_change_set(&change_set, root);
        state
            .db()?
            .store_review_evidence(
                project_id,
                &format!("git:{}", project.path),
                observed_ms,
                &ledger_copy,
            )
            .map_err(super::to_message)?;
    }
    Ok(change_set)
}

pub(crate) fn project_review_checkpoint(
    state: &super::AppState,
    project_id: i64,
) -> Result<Option<ProjectReviewCheckpoint>, String> {
    state
        .db()?
        .project_review_checkpoint(project_id)
        .map_err(super::to_message)
}

pub(crate) fn project_review_checkpoints(
    state: &super::AppState,
) -> Result<Vec<ProjectReviewCheckpoint>, String> {
    state
        .db()?
        .project_review_checkpoints()
        .map_err(super::to_message)
}

pub(crate) fn mark_project_reviewed(
    state: &super::AppState,
    project_id: i64,
    session_cutoff_ms: i64,
) -> Result<ProjectReviewCheckpoint, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let root = Path::new(&project.path);
    let fingerprint = read_git_change_set(root, None)
        .ok()
        .and_then(|change_set| serde_json::to_vec(&change_set).ok())
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
    let git_head = read_git_head(root);
    state
        .db()?
        .set_project_review_checkpoint(
            project_id,
            session_cutoff_ms.max(0),
            fingerprint.as_deref(),
            git_head.as_deref(),
        )
        .map_err(super::to_message)
}

pub(crate) fn project_review_ledger(
    state: &super::AppState,
    project_id: i64,
    limit: usize,
) -> Result<Vec<ReviewLedgerEntry>, String> {
    state
        .db()?
        .project_review_ledger(project_id, limit)
        .map_err(super::to_message)
}

pub(crate) fn project_recap(
    state: &super::AppState,
    project_id: i64,
    session_paths: Vec<String>,
) -> Result<SessionChangeSet, String> {
    build_project_recap(state, project_id, session_paths, true)
}

pub(crate) fn project_review_receipt_export(
    state: &super::AppState,
    project_id: i64,
    session_paths: Vec<String>,
    scope: String,
    path: String,
) -> Result<ExportResult, String> {
    let (scope_label, scope_note) = match scope.as_str() {
        "since" => (
            "Since last review",
            "Selected session records newer than the saved review checkpoint, plus current local Git evidence and eligible retained evidence.",
        ),
        "all" => (
            "All available session records",
            "All selected session records, plus current local Git evidence. Git history and retained evidence still use the saved review baseline when one exists.",
        ),
        _ => return Err("Review receipt scope must be 'since' or 'all'.".to_string()),
    };
    if path.trim().is_empty() {
        return Err("Choose a destination for the review receipt.".to_string());
    }

    let selected_session_count = session_paths
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>()
        .len()
        .min(30);
    let checkpoint = project_review_checkpoint(state, project_id)?;
    let recap = build_project_recap(state, project_id, session_paths, false)?;
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let html = build_review_receipt_html(
        &recap,
        scope_label,
        scope_note,
        selected_session_count,
        checkpoint.is_some(),
        &generated_at,
    );
    fs::write(&path, html.as_bytes()).map_err(super::to_message)?;
    Ok(ExportResult {
        path,
        bytes_written: html.len() as u64,
    })
}

fn build_review_receipt_html(
    recap: &SessionChangeSet,
    scope_label: &str,
    scope_note: &str,
    selected_session_count: usize,
    has_checkpoint: bool,
    generated_at: &str,
) -> String {
    let mut applied = 0u64;
    let mut reverted = 0u64;
    let mut drifted = 0u64;
    let mut missing = 0u64;
    let mut unverified = 0u64;
    for file in &recap.files {
        match file.reality.as_ref().map(|reality| reality.status.as_str()) {
            Some("applied") => applied += 1,
            Some("reverted") => reverted += 1,
            Some("drifted") => drifted += 1,
            Some("file_missing") => missing += 1,
            _ => unverified += 1,
        }
    }

    let coverage_label = escape_html(&recap.coverage.label);
    let coverage_note = escape_html(&recap.coverage.note);
    let scope_label = escape_html(scope_label);
    let scope_note = escape_html(scope_note);
    let generated_at = escape_html(generated_at);
    let parser_note = if recap.redacted_count == 0 && recap.omitted_records == 0 {
        "No redactions or parser omissions were reported inside the bounded sources. This does not prove that the evidence is complete.".to_string()
    } else {
        format!(
            "{} sensitive value(s) were redacted and {} record(s) were omitted or unavailable.",
            recap.redacted_count, recap.omitted_records
        )
    };

    let mut html = String::with_capacity(7_000);
    write!(
        html,
        r#"<!doctype html>
<html lang="en" data-receipt-schema="code-hangar/review-receipt/v1">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Code Hangar Review Receipt</title>
<style>
:root{{color-scheme:light dark;font-family:Inter,Segoe UI,system-ui,sans-serif;background:#f4f7f6;color:#17211f}}*{{box-sizing:border-box}}body{{margin:0;padding:32px 18px;background:#f4f7f6;color:#17211f}}main{{width:min(860px,100%);margin:auto;background:#fff;border:1px solid #cfdad6;border-radius:8px;overflow:hidden}}header{{padding:26px 28px 22px;border-bottom:3px solid #16766d}}.brand{{margin:0 0 16px;color:#16766d;font-size:13px;font-weight:800;text-transform:uppercase}}h1{{margin:0;font-size:30px;letter-spacing:0}}.lead{{max-width:680px;margin:8px 0 0;color:#53645f;line-height:1.55}}.badge{{display:inline-block;margin-top:16px;padding:5px 8px;border:1px solid #9abdb7;border-radius:5px;background:#e8f5f2;color:#0e5d55;font-size:12px;font-weight:750}}section{{padding:22px 28px;border-bottom:1px solid #dce4e1}}h2{{margin:0 0 12px;font-size:17px}}.grid{{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:8px}}.metric{{padding:12px;border:1px solid #dce4e1;border-radius:6px;background:#f8faf9}}.metric strong{{display:block;font-size:23px}}.metric span{{display:block;margin-top:3px;color:#60706b;font-size:11px}}dl{{display:grid;grid-template-columns:170px minmax(0,1fr);gap:9px 14px;margin:0}}dt{{color:#60706b;font-size:12px;font-weight:700}}dd{{margin:0;font-size:13px;line-height:1.45}}.states{{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:7px}}.state{{padding:9px;border-left:3px solid #9abdb7;background:#f8faf9}}.state strong,.state span{{display:block}}.state span{{margin-top:2px;color:#60706b;font-size:10px}}.unknowns{{padding:12px;border:1px solid #e2b35f;border-radius:6px;background:#fff8e8;color:#694b17;line-height:1.5}}footer{{padding:18px 28px;color:#60706b;font-size:11px;line-height:1.5}}@media(max-width:640px){{body{{padding:0}}main{{border:0;border-radius:0}}header,section,footer{{padding-left:18px;padding-right:18px}}.grid{{grid-template-columns:repeat(2,minmax(0,1fr))}}.states{{grid-template-columns:repeat(2,minmax(0,1fr))}}dl{{grid-template-columns:1fr}}}}@media(prefers-color-scheme:dark){{:root,body{{background:#101513;color:#edf4f1}}main{{background:#171e1b;border-color:#35423e}}.lead,dt,.metric span,.state span,footer{{color:#aebdb8}}section{{border-color:#35423e}}.metric,.state{{background:#1d2723;border-color:#35423e}}.badge{{background:#153a35;color:#bce9e2}}.unknowns{{background:#342b17;color:#f0d79d;border-color:#8d6d2d}}}}</style>
</head>
<body><main>
<header><p class="brand">Code Hangar</p><h1>Review Receipt</h1><p class="lead">A local, deterministic summary of the evidence visible when this receipt was created.</p><span class="badge">Private-safe export: project identity and evidence content omitted</span></header>
<section><h2>Review window</h2><dl><dt>Generated (UTC)</dt><dd>{generated_at}</dd><dt>Scope</dt><dd>{scope_label}</dd><dt>Scope boundary</dt><dd>{scope_note}</dd><dt>Saved review baseline</dt><dd>{}</dd><dt>Session records selected</dt><dd>{selected_session_count} (maximum 30 per bounded recap)</dd></dl></section>
<section><h2>Recorded evidence</h2><div class="grid"><div class="metric"><strong>{}</strong><span>files represented</span></div><div class="metric"><strong>{}</strong><span>recorded edits</span></div><div class="metric"><strong>+{}</strong><span>lines added</span></div><div class="metric"><strong>-{}</strong><span>lines removed</span></div></div></section>
<section><h2>Evidence coverage</h2><dl><dt>Coverage label</dt><dd>{coverage_label}</dd><dt>Boundary reported by Code Hangar</dt><dd>{coverage_note}</dd><dt>Parser accounting</dt><dd>{}</dd></dl></section>
<section><h2>Observed file states</h2><div class="states"><div class="state"><strong>{applied}</strong><span>appears applied</span></div><div class="state"><strong>{reverted}</strong><span>appears reverted</span></div><div class="state"><strong>{drifted}</strong><span>drifted</span></div><div class="state"><strong>{missing}</strong><span>file missing</span></div><div class="state"><strong>{unverified}</strong><span>unverified</span></div></div></section>
<section><h2>Unknowns</h2><div class="unknowns">This receipt intentionally contains no prompts, transcript text, diff content, file names, project name or local paths. It summarizes bounded local evidence and does not certify security, correctness or production readiness.</div></section>
<footer>Generated locally by Code Hangar. No remote Git operation or network request is required to create this receipt.</footer>
</main></body></html>"#,
        if has_checkpoint { "Present" } else { "None" },
        recap.files.len(),
        recap.edit_count,
        recap.added_lines,
        recap.removed_lines,
        escape_html(&parser_note),
    )
    .expect("writing receipt HTML into a String cannot fail");
    html
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            character if character.is_control() && !matches!(character, '\n' | '\r' | '\t') => {}
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(feature = "agent_automation")]
pub(crate) fn project_recap_for_ai(
    state: &super::AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
) -> Result<SessionChangeSet, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let recap = match source_mode {
        "combined" => build_project_recap(state, project_id, session_paths, false)?,
        "git" => read_project_git_change_set(state, project_id, false)?,
        "session" => {
            if session_paths.len() != 1 {
                return Err(
                    "A session explanation requires exactly one selected local session."
                        .to_string(),
                );
            }
            read_project_session_change_set(state, project_id, session_paths[0].clone(), false)?
        }
        _ => return Err("That What changed explanation scope is not supported.".to_string()),
    };
    Ok(ledger_safe_change_set(&recap, Path::new(&project.path)))
}

fn build_project_recap(
    state: &super::AppState,
    project_id: i64,
    session_paths: Vec<String>,
    retain_current: bool,
) -> Result<SessionChangeSet, String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "That project is no longer registered.".to_string())?;
    let requested_count = session_paths.len();
    let mut seen_paths = HashSet::new();
    let bounded_paths = session_paths
        .into_iter()
        .filter(|path| seen_paths.insert(path.to_ascii_lowercase()))
        .take(30)
        .collect::<Vec<_>>();
    let capped = requested_count > bounded_paths.len();
    let mut sources = Vec::new();
    let mut failed_sources = 0u64;
    for path in &bounded_paths {
        match read_project_session_change_set(state, project_id, path.clone(), retain_current) {
            Ok(change_set) => sources.push(("session", change_set)),
            Err(_) => failed_sources += 1,
        }
    }
    match read_project_git_change_set(state, project_id, retain_current) {
        Ok(change_set) => sources.push(("git", change_set)),
        Err(_) => failed_sources += 1,
    }
    let current_source_count = sources.len();
    let current_coverage_levels = sources
        .iter()
        .map(|(_, source)| source.coverage.level.clone())
        .collect::<Vec<_>>();
    let current_refs = bounded_paths
        .iter()
        .map(|path| path.to_ascii_lowercase())
        .chain(std::iter::once(
            format!("git:{}", project.path).to_ascii_lowercase(),
        ))
        .collect::<HashSet<_>>();
    let checkpoint = state
        .db()?
        .project_review_checkpoint(project_id)
        .map_err(super::to_message)?;
    let ledger = match project_review_ledger(state, project_id, 100) {
        Ok(ledger) => ledger,
        Err(_) => {
            // A retained ledger must pass its hash-chain checks before contributing evidence.
            // Leave an invalid historical source out without hiding current Git/session evidence.
            failed_sources += 1;
            Vec::new()
        }
    };
    let mut retained_count = 0usize;
    let mut retained_has_evidence = false;
    for entry in ledger {
        if current_refs.contains(&entry.source_ref.to_ascii_lowercase()) {
            continue;
        }
        if !ledger_entry_is_after_checkpoint(entry.source_modified_ms, checkpoint.as_ref()) {
            continue;
        }
        let mut retained = entry.change_set;
        retained_has_evidence |= retained.coverage.level != "none" || !retained.files.is_empty();
        for file in &mut retained.files {
            for edit in &mut file.edits {
                edit.provenance = Some(format!(
                    "Retained encrypted review evidence · observed {}",
                    entry.observed_at
                ));
                edit.confidence = Some("retained".to_string());
                edit.reality = Some(reality(
                    "unverified",
                    "Retained observation",
                    "This edit was retained from an earlier review and was not compared again.",
                ));
            }
            file.reality = Some(reality(
                "unverified",
                "Retained observation",
                "This file-state label was retained from an earlier review. Open the live source again to compare it with the current project.",
            ));
        }
        retained_count += 1;
        sources.push(("ledger", retained));
    }

    let coverage = combined_coverage(
        current_coverage_levels.iter().map(String::as_str),
        failed_sources > 0 || capped,
        retained_has_evidence,
    );
    let mut combined = SessionChangeSet {
        path: format!("project:{project_id}"),
        source_kind: "Combined local evidence".to_string(),
        coverage,
        files: Vec::new(),
        edit_count: 0,
        added_lines: 0,
        removed_lines: 0,
        redacted_count: 0,
        parsed_records: 0,
        omitted_records: failed_sources,
    };
    for (kind, mut source) in sources {
        combined.redacted_count += source.redacted_count;
        combined.parsed_records += source.parsed_records;
        combined.omitted_records += source.omitted_records;
        for mut file in source.files.drain(..) {
            for edit in &mut file.edits {
                edit.source = format!("{} · {}", source.source_kind, edit.source);
                if edit.provenance.is_none() {
                    edit.provenance = Some(match kind {
                        "git" => "Current local Git evidence".to_string(),
                        "ledger" => "Retained encrypted review evidence".to_string(),
                        _ => "Recorded local AI session evidence".to_string(),
                    });
                }
            }
            if let Some(existing) = combined.files.iter_mut().find(|existing| {
                hangar_core::normalize_path(&existing.path)
                    .eq_ignore_ascii_case(&hangar_core::normalize_path(&file.path))
            }) {
                existing.added_lines += file.added_lines;
                existing.removed_lines += file.removed_lines;
                existing.edits.extend(file.edits);
                if existing.reality.as_ref().map(|value| value.status.as_str())
                    != file.reality.as_ref().map(|value| value.status.as_str())
                {
                    existing.reality = Some(reality(
                        "unverified",
                        "Sources disagree",
                        "Local evidence sources describe different current states. Review the live file before drawing a conclusion.",
                    ));
                }
            } else {
                combined.files.push(file);
            }
        }
    }
    combined.edit_count = combined
        .files
        .iter()
        .map(|file| file.edits.len() as u64)
        .sum();
    combined.added_lines = combined.files.iter().map(|file| file.added_lines).sum();
    combined.removed_lines = combined.files.iter().map(|file| file.removed_lines).sum();
    let coverage_reason = combined.coverage.note.clone();
    combined.coverage.note = format!(
        "Combined {current_source_count} current source{} with {retained_count} retained ledger entr{}. Current sources are local session records and the local Git working tree; retained evidence is labelled separately. {coverage_reason}{}{}",
        if current_source_count == 1 { "" } else { "s" },
        if retained_count == 1 { "y" } else { "ies" },
        if failed_sources > 0 {
            format!(" {failed_sources} source{} could not be read.", if failed_sources == 1 { "" } else { "s" })
        } else {
            String::new()
        },
        if capped { " Only the 30 most recent session paths were read in this bounded recap." } else { "" }
    );
    Ok(combined)
}

fn ledger_entry_is_after_checkpoint(
    source_modified_ms: Option<i64>,
    checkpoint: Option<&ProjectReviewCheckpoint>,
) -> bool {
    let Some(checkpoint) = checkpoint else {
        return true;
    };
    source_modified_ms.is_some_and(|modified_ms| modified_ms > checkpoint.session_cutoff_ms)
}

fn combined_coverage<'a>(
    current_levels: impl IntoIterator<Item = &'a str>,
    has_source_gap: bool,
    has_retained_evidence: bool,
) -> SessionChangeCoverage {
    let mut source_count = 0usize;
    let mut saw_none = false;
    let mut saw_partial = false;
    let mut saw_direct = false;
    let mut saw_full = false;
    for level in current_levels {
        source_count += 1;
        match level {
            "none" => saw_none = true,
            "partial" => saw_partial = true,
            "direct" | "direct_edits" => saw_direct = true,
            "full" => saw_full = true,
            _ => saw_partial = true,
        }
    }

    let has_current_evidence = saw_partial || saw_direct || saw_full;
    if !has_current_evidence && !has_retained_evidence && !has_source_gap {
        return SessionChangeCoverage {
            level: "none".to_string(),
            label: "No reconstructable local evidence".to_string(),
            note: if source_count == 0 {
                "No current evidence source was available, and no retained change evidence was included."
                    .to_string()
            } else {
                "Every current source reported that it had no reconstructable change evidence."
                    .to_string()
            },
        };
    }

    if has_source_gap || saw_none || saw_partial || has_retained_evidence {
        return SessionChangeCoverage {
            level: "partial".to_string(),
            label: "Partial fused evidence".to_string(),
            note: "At least one source was unavailable, bounded, reported no reconstructable evidence, or came from an unverified retained observation. The visible changes are evidence, not a complete account."
                .to_string(),
        };
    }

    if saw_direct {
        return SessionChangeCoverage {
            level: "direct_edits".to_string(),
            label: "Recorded direct edits".to_string(),
            note: "Every current source was readable, but at least one reconstructs direct edits only and may omit changes made through other tools."
                .to_string(),
        };
    }

    SessionChangeCoverage {
        level: "full".to_string(),
        label: "Fused local evidence".to_string(),
        note: "Every included current source reported full coverage for the bounded evidence it supports."
            .to_string(),
    }
}

fn read_git_change_set(
    root: &Path,
    reviewed_head: Option<&str>,
) -> Result<SessionChangeSet, String> {
    if !root.is_dir() || !root.join(".git").exists() {
        return Ok(SessionChangeSet {
            path: format!("git:{}", root.to_string_lossy()),
            source_kind: "Local Git".to_string(),
            coverage: hangar_core::SessionChangeCoverage {
                level: "none".to_string(),
                label: "No local Git evidence".to_string(),
                note: "This registered project has no local .git working tree. No Git diff was invented."
                    .to_string(),
            },
            files: Vec::new(),
            edit_count: 0,
            added_lines: 0,
            removed_lines: 0,
            redacted_count: 0,
            parsed_records: 0,
            omitted_records: 0,
        });
    }
    let (committed, committed_since_requested, committed_since_unavailable) =
        if let Some(head) = reviewed_head.filter(|value| valid_git_oid(value)) {
            let range = format!("{head}..HEAD");
            match run_git(
                root,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--unified=3",
                    &range,
                    "--",
                    ".",
                ],
            ) {
                Ok(output) => (output, true, false),
                Err(_) => (
                    BoundedCommandOutput {
                        text: String::new(),
                        truncated: false,
                    },
                    true,
                    true,
                ),
            }
        } else {
            (
                BoundedCommandOutput {
                    text: String::new(),
                    truncated: false,
                },
                reviewed_head.is_some(),
                reviewed_head.is_some(),
            )
        };
    let staged = run_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            "--cached",
            "--",
            ".",
        ],
    )?;
    let working = run_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            "--",
            ".",
        ],
    )?;
    let status = run_git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    let untracked = status
        .text
        .split('\0')
        .filter_map(|entry| entry.strip_prefix("?? "))
        .filter(|path| !path.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(super::session_changes::build_git_change_set(
        format!("git:{}", root.to_string_lossy()),
        super::session_changes::GitChangeEvidence {
            committed_patch: &committed.text,
            staged_patch: &staged.text,
            working_patch: &working.text,
            untracked_paths: &untracked,
            output_truncated: committed.truncated
                || staged.truncated
                || working.truncated
                || status.truncated,
            committed_since_requested,
            committed_since_unavailable,
        },
    ))
}

fn read_git_head(root: &Path) -> Option<String> {
    if !root.is_dir() || !root.join(".git").exists() {
        return None;
    }
    let output = run_git(root, &["rev-parse", "--verify", "HEAD"]).ok()?;
    let head = output.text.trim();
    valid_git_oid(head).then(|| head.to_ascii_lowercase())
}

fn valid_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn require_read_only_git_command(args: &[&str]) -> Result<(), String> {
    let Some(subcommand) = args.first().copied() else {
        return Err("Local Git inspection refused an empty command.".to_string());
    };
    if matches!(subcommand, "diff" | "status" | "rev-parse") {
        return Ok(());
    }
    Err(format!(
        "Local Git inspection refused the non-read-only '{subcommand}' command."
    ))
}

fn run_git(root: &Path, args: &[&str]) -> Result<BoundedCommandOutput, String> {
    require_read_only_git_command(args)?;
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        // Repository-local config is untrusted input. Disable executable helpers before Git
        // reads the index or working tree; diff callers also pass --no-ext-diff/--no-textconv.
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_ATTR_SOURCE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIFF_OPTS")
        .env_remove("GIT_EXTERNAL_DIFF")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Local Git evidence is unavailable ({error})."))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Local Git output could not be captured.".to_string())?;
    let reader = thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0u8; 16 * 1024];
        let mut truncated = false;
        loop {
            let read = stdout.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            let remaining = GIT_OUTPUT_CAP.saturating_sub(kept.len());
            let copy = read.min(remaining);
            kept.extend_from_slice(&buffer[..copy]);
            truncated |= copy < read;
        }
        (kept, truncated)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Local Git status could not be read ({error})."))?
        {
            break status;
        }
        if started.elapsed() >= GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Local Git inspection timed out after 8 seconds.".to_string());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (bytes, truncated) = reader
        .join()
        .map_err(|_| "Local Git output reader stopped unexpectedly.".to_string())?;
    if !status.success() {
        return Err("Local Git could not inspect this working tree.".to_string());
    }
    Ok(BoundedCommandOutput {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    })
}

fn reconcile_git_change_set(change_set: &mut SessionChangeSet) {
    for file in &mut change_set.files {
        let observed = SessionFileReality {
            status: "applied".to_string(),
            label: "Present in the working tree".to_string(),
            note: "This evidence was read from the current local Git index and working tree."
                .to_string(),
            observed_ms: system_time_ms(SystemTime::now()),
        };
        for edit in &mut file.edits {
            edit.reality = Some(observed.clone());
        }
        file.reality = Some(observed);
    }
}

fn reconcile_change_set(change_set: &mut SessionChangeSet, project_root: &Path) {
    for file in &mut change_set.files {
        file.reality = Some(compare_file_change(project_root, file));
    }
}

fn ledger_safe_change_set(change_set: &SessionChangeSet, project_root: &Path) -> SessionChangeSet {
    let mut safe = change_set.clone();
    let mut excluded_edits = 0u64;
    safe.files.retain(|file| {
        let Some(target) = authorized_change_path(project_root, &file.path) else {
            excluded_edits += file.edits.len() as u64;
            return false;
        };
        let relative = target.strip_prefix(project_root).unwrap_or(&target);
        let relative_text = relative.to_string_lossy();
        let allowed = !hangar_protect::is_sensitive_path(&relative_text)
            && hangar_protect::protected_level_for_path(&relative_text).is_none()
            && !hangar_protect::is_heavy_or_protected_container_path(&relative_text);
        if !allowed {
            excluded_edits += file.edits.len() as u64;
        }
        allowed
    });
    safe.edit_count = safe.files.iter().map(|file| file.edits.len() as u64).sum();
    safe.added_lines = safe.files.iter().map(|file| file.added_lines).sum();
    safe.removed_lines = safe.files.iter().map(|file| file.removed_lines).sum();
    safe.omitted_records = safe.omitted_records.saturating_add(excluded_edits);
    safe
}

fn compare_file_change(
    project_root: &Path,
    change: &mut hangar_core::SessionFileChange,
) -> SessionFileReality {
    let Some(target) = authorized_change_path(project_root, &change.path) else {
        return set_all_edit_realities(
            change,
            reality(
                "unverified",
                "Not compared",
                "The recorded path cannot be proven to stay inside this project.",
            ),
        );
    };
    if !target.exists() {
        return set_all_edit_realities(
            change,
            reality(
                "file_missing",
                "File missing",
                "The recorded file is not present at this project path now.",
            ),
        );
    }
    let relative = target.strip_prefix(project_root).unwrap_or(&target);
    let relative_text = relative.to_string_lossy();
    if hangar_protect::is_sensitive_path(&relative_text)
        || hangar_protect::protected_level_for_path(&relative_text).is_some()
    {
        return set_all_edit_realities(
            change,
            reality(
                "unverified",
                "Protected from comparison",
                "Code Hangar did not open this sensitive or Protected file to compare it.",
            ),
        );
    }
    let metadata = match fs::metadata(&target) {
        Ok(metadata) => metadata,
        Err(_) => {
            return set_all_edit_realities(
                change,
                reality(
                    "unverified",
                    "Could not compare",
                    "The current file could not be inspected.",
                ),
            )
        }
    };
    if metadata.len() > REALITY_READ_CAP {
        return set_all_edit_realities(
            change,
            reality(
                "unverified",
                "Too large to compare",
                "The current file is above the bounded comparison limit.",
            ),
        );
    }
    let content = match fs::read_to_string(&target) {
        Ok(content) => content.replace("\r\n", "\n"),
        Err(_) => {
            return set_all_edit_realities(
                change,
                reality(
                    "unverified",
                    "Could not compare",
                    "The current file is not readable UTF-8 text.",
                ),
            )
        }
    };
    let mut statuses = Vec::new();
    for edit in &mut change.edits {
        let observed = compare_edit_content(&content, edit);
        statuses.push(observed.status.clone());
        edit.reality = Some(observed);
    }
    let Some(first) = change.edits.first().and_then(|edit| edit.reality.clone()) else {
        return reality(
            "unverified",
            "No comparable edits",
            "This file has no recorded edit evidence to compare.",
        );
    };
    if statuses.iter().all(|status| status == &statuses[0]) {
        first
    } else {
        reality(
            "drifted",
            "Mixed edit states",
            "Recorded edits in this file have different current states. Review the per-edit labels.",
        )
    }
}

fn compare_edit_content(
    content: &str,
    edit: &hangar_core::SessionChangeEdit,
) -> SessionFileReality {
    let mut after_matches = 0usize;
    let mut before_matches = 0usize;
    let mut comparable = 0usize;
    for hunk in &edit.hunks {
        let after = hunk
            .lines
            .iter()
            .filter(|line| line.kind != "removed" && line.kind != "note")
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let before = hunk
            .lines
            .iter()
            .filter(|line| line.kind != "added" && line.kind != "note")
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if after.trim().is_empty() && before.trim().is_empty() {
            continue;
        }
        comparable += 1;
        after_matches += (!after.trim().is_empty() && content.contains(&after)) as usize;
        before_matches += (!before.trim().is_empty() && content.contains(&before)) as usize;
    }
    if comparable == 0 {
        return reality(
            "unverified",
            "No comparable lines",
            "The transcript records an action but not enough before/after text to compare safely.",
        );
    }
    if after_matches == comparable && before_matches == 0 {
        reality(
            "applied",
            "Appears applied",
            "Every recorded after-block was found and the corresponding before-blocks were not.",
        )
    } else if before_matches == comparable && after_matches == 0 {
        reality(
            "reverted",
            "Appears reverted",
            "Every recorded before-block was found and the corresponding after-blocks were not.",
        )
    } else {
        reality(
            "drifted",
            "Changed again",
            "The current file matches neither the complete recorded before-state nor after-state. This is compare-only evidence, not a proposed fix.",
        )
    }
}

fn set_all_edit_realities(
    change: &mut hangar_core::SessionFileChange,
    observed: SessionFileReality,
) -> SessionFileReality {
    for edit in &mut change.edits {
        edit.reality = Some(observed.clone());
    }
    observed
}

fn authorized_change_path(root: &Path, recorded: &str) -> Option<PathBuf> {
    let recorded_path = Path::new(recorded);
    let candidate = if recorded_path.is_absolute() {
        recorded_path.to_path_buf()
    } else {
        if recorded_path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return None;
        }
        root.join(recorded_path)
    };
    let root_text = hangar_core::normalize_path(&root.to_string_lossy());
    let candidate_text = hangar_core::normalize_path(&candidate.to_string_lossy());
    let root_prefix = format!("{}/", root_text.trim_end_matches('/'));
    if !(candidate_text.eq_ignore_ascii_case(&root_text)
        || candidate_text
            .to_ascii_lowercase()
            .starts_with(&root_prefix.to_ascii_lowercase()))
    {
        return None;
    }
    if candidate.exists() {
        let identity = hangar_fs::inspect_path_identity(&candidate);
        if identity.inaccessible || identity.is_reparse || identity.reparse_kind.is_some() {
            return None;
        }
        let canonical_root = fs::canonicalize(root).ok()?;
        let canonical_candidate = fs::canonicalize(&candidate).ok()?;
        let canonical_root_text = hangar_core::normalize_path(&canonical_root.to_string_lossy());
        let canonical_candidate_text =
            hangar_core::normalize_path(&canonical_candidate.to_string_lossy());
        let canonical_prefix = format!("{}/", canonical_root_text.trim_end_matches('/'));
        if !canonical_candidate_text.eq_ignore_ascii_case(&canonical_root_text)
            && !canonical_candidate_text
                .to_ascii_lowercase()
                .starts_with(&canonical_prefix.to_ascii_lowercase())
        {
            return None;
        }
    }
    Some(candidate)
}

fn reality(status: &str, label: &str, note: &str) -> SessionFileReality {
    SessionFileReality {
        status: status.to_string(),
        label: label.to_string(),
        note: note.to_string(),
        observed_ms: system_time_ms(SystemTime::now()),
    }
}

fn system_time_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::{SessionChangeEdit, SessionDiffHunk, SessionDiffLine, SessionFileChange};

    #[test]
    fn local_git_runner_accepts_only_the_read_only_subcommands() {
        for args in [
            &["diff", "--", "."][..],
            &["status", "--porcelain=v1"][..],
            &["rev-parse", "--verify", "HEAD"][..],
        ] {
            assert!(require_read_only_git_command(args).is_ok(), "{args:?}");
        }
        for subcommand in [
            "add", "branch", "checkout", "clean", "commit", "fetch", "merge", "pull", "push",
            "rebase", "remote", "reset", "restore", "switch", "tag",
        ] {
            let error = require_read_only_git_command(&[subcommand]).unwrap_err();
            assert!(error.contains("non-read-only"), "{subcommand}: {error}");
        }
        assert!(require_read_only_git_command(&[]).is_err());
    }

    fn change(path: &str) -> SessionFileChange {
        SessionFileChange {
            path: path.to_string(),
            edits: vec![SessionChangeEdit {
                source: "fixture".to_string(),
                summary: "fixture".to_string(),
                provenance: Some("fixture".to_string()),
                confidence: Some("observed".to_string()),
                reality: None,
                request: None,
                hunks: vec![SessionDiffHunk {
                    header: "@@ -1 +1 @@".to_string(),
                    old_start: Some(1),
                    new_start: Some(1),
                    lines: vec![
                        SessionDiffLine {
                            kind: "removed".to_string(),
                            content: "old".to_string(),
                            old_line: Some(1),
                            new_line: None,
                        },
                        SessionDiffLine {
                            kind: "added".to_string(),
                            content: "new".to_string(),
                            old_line: None,
                            new_line: Some(1),
                        },
                    ],
                }],
                added_lines: 1,
                removed_lines: 1,
            }],
            added_lines: 1,
            removed_lines: 1,
            reality: None,
        }
    }

    #[test]
    fn comparison_distinguishes_applied_reverted_drifted_and_missing() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("applied.txt"), "new\n").unwrap();
        fs::write(root.path().join("reverted.txt"), "old\n").unwrap();
        fs::write(root.path().join("drifted.txt"), "different\n").unwrap();
        for (path, expected) in [
            ("applied.txt", "applied"),
            ("reverted.txt", "reverted"),
            ("drifted.txt", "drifted"),
            ("missing.txt", "file_missing"),
        ] {
            let mut file = change(path);
            assert_eq!(compare_file_change(root.path(), &mut file).status, expected);
            assert_eq!(
                file.edits[0]
                    .reality
                    .as_ref()
                    .map(|value| value.status.as_str()),
                Some(expected)
            );
        }
    }

    #[test]
    fn comparison_refuses_paths_outside_the_project() {
        let root = tempfile::tempdir().unwrap();
        let mut file = change("../outside.txt");
        assert_eq!(
            compare_file_change(root.path(), &mut file).status,
            "unverified"
        );
    }

    #[test]
    fn ledger_copy_excludes_sensitive_and_unauthorized_paths() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("safe.txt"), "new\n").unwrap();
        fs::write(root.path().join(".env"), "SECRET=value\n").unwrap();
        fs::create_dir_all(root.path().join("node_modules/package")).unwrap();
        fs::write(root.path().join("node_modules/package/index.js"), "new\n").unwrap();
        let mut change_set =
            super::super::session_changes::unsupported_change_set("fixture-session".to_string());
        change_set.files = vec![
            change("safe.txt"),
            change(".env"),
            change("../outside.txt"),
            change("node_modules/package/index.js"),
        ];
        change_set.edit_count = 4;
        change_set.added_lines = 4;
        change_set.removed_lines = 4;

        let safe = ledger_safe_change_set(&change_set, root.path());
        assert_eq!(safe.files.len(), 1);
        assert_eq!(safe.files[0].path, "safe.txt");
        assert_eq!(safe.edit_count, 1);
        assert_eq!(safe.omitted_records, 3);
    }

    #[test]
    fn combined_coverage_never_promotes_absent_or_partial_sources() {
        let level = |levels: &[&str], has_gap, retained| {
            combined_coverage(levels.iter().copied(), has_gap, retained).level
        };

        assert_eq!(level(&["none"], false, false), "none");
        assert_eq!(level(&["full", "none"], false, false), "partial");
        assert_eq!(level(&["full", "partial"], false, false), "partial");
        assert_eq!(
            level(&["full", "direct_edits"], false, false),
            "direct_edits"
        );
        assert_eq!(level(&["full", "full"], false, false), "full");
        assert_eq!(level(&["full"], true, false), "partial");
        assert_eq!(level(&["full"], false, true), "partial");
    }

    #[test]
    fn retained_ledger_respects_the_review_checkpoint_cutoff() {
        let checkpoint = ProjectReviewCheckpoint {
            project_id: 7,
            reviewed_at: "2026-07-14T08:00:00Z".to_string(),
            session_cutoff_ms: 2_000,
            git_fingerprint: None,
            git_head: None,
        };
        assert!(!ledger_entry_is_after_checkpoint(
            Some(1_999),
            Some(&checkpoint)
        ));
        assert!(!ledger_entry_is_after_checkpoint(
            Some(2_000),
            Some(&checkpoint)
        ));
        assert!(ledger_entry_is_after_checkpoint(
            Some(2_001),
            Some(&checkpoint)
        ));
        assert!(!ledger_entry_is_after_checkpoint(None, Some(&checkpoint)));
        assert!(ledger_entry_is_after_checkpoint(None, None));
    }

    #[test]
    fn review_receipt_reports_counts_without_exporting_evidence_content() {
        let mut private_change = change(r"C:\Users\person\secret-project\private.rs");
        private_change.reality = Some(reality(
            "applied",
            "Appears applied",
            "The recorded after-state was observed.",
        ));
        private_change.edits[0].request = Some("API_TOKEN=do-not-export".to_string());
        private_change.edits[0].source = "private transcript body".to_string();
        let recap = SessionChangeSet {
            path: r"project:C:\Users\person\secret-project".to_string(),
            source_kind: "Combined local evidence".to_string(),
            coverage: SessionChangeCoverage {
                level: "partial".to_string(),
                label: "Partial <script>alert(1)</script>".to_string(),
                note: "One bounded source was unavailable.".to_string(),
            },
            files: vec![private_change],
            edit_count: 1,
            added_lines: 1,
            removed_lines: 1,
            redacted_count: 2,
            parsed_records: 12,
            omitted_records: 3,
        };

        let html = build_review_receipt_html(
            &recap,
            "Since last review",
            "Bounded local evidence.",
            1,
            true,
            "2026-07-14T10:00:00Z",
        );

        assert!(html.contains("Review Receipt"));
        assert!(html.contains("files represented"));
        assert!(html.contains("2 sensitive value(s) were redacted"));
        assert!(html.contains("Partial &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains(r"C:\Users\person"));
        assert!(!html.contains("private.rs"));
        assert!(!html.contains("API_TOKEN"));
        assert!(!html.contains("private transcript body"));
        assert!(!html.contains("<script>"));
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("local Git executable");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn local_git_change_set_includes_commits_since_reviewed_head() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        git(root.path(), &["config", "user.name", "Code Hangar Test"]);
        git(
            root.path(),
            &["config", "user.email", "code-hangar@example.invalid"],
        );
        fs::write(root.path().join("code.txt"), "before\n").unwrap();
        git(root.path(), &["add", "--", "code.txt"]);
        git(root.path(), &["commit", "--quiet", "-m", "baseline"]);
        let baseline = read_git_head(root.path()).expect("baseline HEAD");

        fs::write(root.path().join("code.txt"), "after\n").unwrap();
        git(root.path(), &["add", "--", "code.txt"]);
        git(root.path(), &["commit", "--quiet", "-m", "reviewed change"]);

        let result = read_git_change_set(root.path(), Some(&baseline)).unwrap();
        assert_eq!(result.coverage.level, "full");
        assert!(result.coverage.note.contains("commits since"));
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "code.txt");
        assert_eq!(
            result.files[0].edits[0].source,
            "Git commits since last review"
        );
        assert!(result.files[0].edits[0]
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .any(|line| line.kind == "added" && line.content == "after"));
    }

    #[test]
    fn local_git_evidence_disables_a_repository_fsmonitor_helper() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        git(root.path(), &["config", "user.name", "Code Hangar Test"]);
        git(
            root.path(),
            &["config", "user.email", "code-hangar@example.invalid"],
        );
        fs::write(root.path().join("tracked.txt"), "baseline\n").unwrap();
        git(root.path(), &["add", "--", "tracked.txt"]);
        git(root.path(), &["commit", "--quiet", "-m", "baseline"]);

        let marker = root.path().join("fsmonitor-was-run");
        let helper = root.path().join("hostile-fsmonitor.sh");
        let marker_path = marker.to_string_lossy().replace('\\', "/");
        fs::write(
            &helper,
            format!("#!/bin/sh\nprintf invoked > \"{marker_path}\"\nexit 1\n"),
        )
        .unwrap();
        let helper_path = helper.to_string_lossy().replace('\\', "/");
        let hostile_command = format!("sh \"{helper_path}\"");
        git(root.path(), &["config", "core.fsmonitor", &hostile_command]);
        fs::write(root.path().join("untracked.txt"), "local\n").unwrap();

        let raw_status = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(root.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("raw local Git status");
        assert!(raw_status.success());
        assert!(
            marker.exists(),
            "the hostile fsmonitor fixture was not executable by Git"
        );
        fs::remove_file(&marker).unwrap();

        let result = read_git_change_set(root.path(), None).unwrap();
        assert!(result.files.iter().any(|file| file.path == "untracked.txt"));
        assert!(
            !marker.exists(),
            "a project-controlled fsmonitor helper executed during local evidence collection"
        );
    }

    #[test]
    fn git_object_ids_accept_only_full_local_hashes() {
        assert!(valid_git_oid("0123456789012345678901234567890123456789"));
        assert!(valid_git_oid(&"a".repeat(64)));
        assert!(!valid_git_oid("HEAD"));
        assert!(!valid_git_oid("abc123"));
        assert!(!valid_git_oid(&format!("{}..HEAD", "a".repeat(40))));
    }
}
