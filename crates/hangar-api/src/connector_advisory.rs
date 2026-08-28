//! Connector-only Safe Manage advisory context selection and receipts.
//!
//! The webview receives random selectors, fixed safe labels and exact outbound
//! payloads. It never supplies a path or body. Source locators remain in this
//! process-local store; SQLite receives only typed hashes/counts.

use super::{
    ai_assist, ensure_ai_prepared_request_still_matches, file_preview,
    lock_ai_credential_operation, now_millis, project_get, redact_path_occurrences,
    resolve_ai_provider_with_model, resolve_safe_manage_ai_assessment, session_preview,
    stage_ai_prepared_safe_manage_send, take_ai_prepared_safe_manage_send, to_message, AppState,
};
use hangar_core::{
    AiSafeManageAdvisoryReceipt, AiSafeManageAdvisoryResult, AiSafeManageAdvisorySourceReceipt,
    AiSafeManageContextCandidate, AiSafeManageContextKind, FileKind, PreviewMode, PreviewState,
    ProjectDiscoveryReport, SafeManageProjectAssessment,
};
#[cfg(test)]
use hangar_core::{SafeManageConfidence, SafeManageRecommendation};
use hangar_db::NewConnectorAdvisoryReceipt;
use std::collections::{HashMap, HashSet};

const CONTEXT_SELECTION_TTL_MS: u128 = 10 * 60 * 1000;
const CONTEXT_SELECTION_CAP: usize = 256;
const MAX_CONTEXT_CANDIDATES: usize = 28;
const MAX_SELECTED_CONTEXTS: usize = 6;
const SELECTED_EXCERPT_MAX_CHARS: usize = 2_400;
const SELECTED_EXCERPT_READ_CHARS: usize = 8_000;

#[derive(Debug, Clone)]
enum StoredContextSource {
    File {
        node_id: i64,
    },
    Session {
        path: String,
        linked_project_paths: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct StoredContextSelection {
    project_id: i64,
    analysis_run_id: String,
    evidence_revision: String,
    kind: AiSafeManageContextKind,
    source: StoredContextSource,
    created_ms: u128,
}

#[derive(Debug)]
struct ContextCandidateDraft {
    kind: AiSafeManageContextKind,
    label: String,
    detail: String,
    source: StoredContextSource,
}

#[derive(Debug, Default)]
pub(crate) struct SafeManageContextSelectionStore {
    selections: HashMap<String, StoredContextSelection>,
}

#[derive(Debug)]
struct PreparedContext {
    excerpts: Vec<ai_assist::SafeManageAdvisoryContextExcerpt>,
    receipts: Vec<AiSafeManageAdvisorySourceReceipt>,
}

/// Return backend-curated candidates for one immutable, enrichable assessment.
/// Every returned id is random and process-local; no path or inventory id is
/// encoded in the response.
pub fn ai_safe_manage_context_candidates(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
) -> Result<Vec<AiSafeManageContextCandidate>, String> {
    let assessment =
        resolve_safe_manage_ai_assessment(state, project_id, analysis_run_id, evidence_revision)?;
    ai_assist::validate_safe_manage_ai_enrichment_assessment(&assessment)?;
    let db = state.db()?;
    let mut drafts = Vec::new();
    let mut seen_file_nodes = HashSet::new();

    for file in db.project_context_files(project_id).map_err(to_message)? {
        if file.is_sensitive
            || file.protected_level.is_some()
            || !seen_file_nodes.insert(file.node_id)
        {
            continue;
        }
        let Some(kind) = curated_file_kind(&file.display_name) else {
            continue;
        };
        drafts.push(ContextCandidateDraft {
            kind,
            label: safe_file_label(kind, &file.display_name),
            detail: file_context_detail(kind).to_string(),
            source: StoredContextSource::File {
                node_id: file.node_id,
            },
        });
    }

    for file in db
        .connector_advisory_core_files(project_id)
        .map_err(to_message)?
    {
        if !seen_file_nodes.insert(file.node_id) {
            continue;
        }
        drafts.push(ContextCandidateDraft {
            kind: AiSafeManageContextKind::CoreFile,
            label: safe_file_label(AiSafeManageContextKind::CoreFile, &file.display_name),
            detail: file_context_detail(AiSafeManageContextKind::CoreFile).to_string(),
            source: StoredContextSource::File {
                node_id: file.node_id,
            },
        });
    }

    if let Some(mut report) = state
        .read_discovery_cache()
        .and_then(|json| serde_json::from_str::<ProjectDiscoveryReport>(&json).ok())
    {
        report
            .sessions
            .sort_by_key(|session| std::cmp::Reverse(session.modified_ms));
        for (ordinal, session) in report
            .sessions
            .into_iter()
            .filter(|session| session.linked_registered_project_ids.contains(&project_id))
            .take(8)
            .enumerate()
        {
            drafts.push(ContextCandidateDraft {
                kind: AiSafeManageContextKind::SessionExcerpt,
                label: format!(
                    "{} linked session {}",
                    safe_session_source_label(&session.source_kind),
                    ordinal + 1
                ),
                detail: "Bounded conversation excerpt; credentials and absolute paths are removed before preview."
                    .to_string(),
                source: StoredContextSource::Session {
                    path: session.path,
                    linked_project_paths: session.linked_project_paths,
                },
            });
        }
    }

    drafts.truncate(MAX_CONTEXT_CANDIDATES);
    if drafts.is_empty() {
        return Ok(Vec::new());
    }
    register_candidates(
        state,
        project_id,
        analysis_run_id,
        evidence_revision,
        drafts,
    )
}

/// Prepare and persist a fingerprint-only receipt before staging the exact
/// one-shot provider request. The selected ids are resolved from the backend
/// store; the webview cannot substitute a path or content body.
pub fn ai_safe_manage_advisory_disclosure(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
    selected_context_ids: &[String],
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let assessment =
        resolve_safe_manage_ai_assessment(state, project_id, analysis_run_id, evidence_revision)?;
    ai_assist::validate_safe_manage_ai_enrichment_assessment(&assessment)?;
    let prepared_context = prepare_selected_context(state, &assessment, selected_context_ids)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let request = ai_assist::ai_prepare_safe_manage_advisory_with_config(
        &assessment,
        &prepared_context.excerpts,
        &config,
    )?;
    let (request_hash, request_chars) = request_fingerprint(&request)?;
    let receipt_id = format!(
        "sm-advisory-{}",
        hangar_agent::random_token(24)
            .map_err(|_| "Could not create a secure advisory receipt id.".to_string())?
    );
    let db = state.db()?;
    db.connector_advisory_receipt_prepare(&NewConnectorAdvisoryReceipt {
        receipt_id: receipt_id.clone(),
        project_id,
        analysis_run_id: analysis_run_id.to_string(),
        evidence_revision: evidence_revision.to_string(),
        request_hash,
        request_chars,
        sources: prepared_context.receipts,
    })
    .map_err(to_message)?;

    match stage_ai_prepared_safe_manage_send(
        state,
        request,
        receipt_id.clone(),
        selected_context_ids.to_vec(),
    ) {
        Ok(disclosure) => Ok(disclosure),
        Err(error) => {
            let _ = db.connector_advisory_receipt_fail(&receipt_id, "internal_error");
            Err(error)
        }
    }
}

/// Consume first, rebuild from the same backend-bound selections, compare the
/// exact request, then send. Only a response fingerprint/count is persisted.
pub fn ai_safe_manage_advisory(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
    model: &str,
    preview_id: &str,
) -> Result<AiSafeManageAdvisoryResult, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let (reviewed, receipt_id, selected_context_ids) =
        take_ai_prepared_safe_manage_send(state, preview_id)?;
    let db = state.db()?;

    let assessment = match resolve_safe_manage_ai_assessment(
        state,
        project_id,
        analysis_run_id,
        evidence_revision,
    ) {
        Ok(value) => value,
        Err(error) => {
            let _ = db.connector_advisory_receipt_fail(&receipt_id, "evidence_changed");
            return Err(error);
        }
    };
    let prepared_context = match prepare_selected_context(state, &assessment, &selected_context_ids)
    {
        Ok(value) => value,
        Err(error) => {
            let code = if error.starts_with("Not sent") {
                "payload_blocked"
            } else {
                "selection_expired"
            };
            let _ = db.connector_advisory_receipt_fail(&receipt_id, code);
            return Err(error);
        }
    };
    let config = match resolve_ai_provider_with_model(state, model) {
        Ok(value) => value,
        Err(error) => {
            let _ = db.connector_advisory_receipt_fail(&receipt_id, "provider_changed");
            return Err(error);
        }
    };
    let rebuilt = match ai_assist::ai_prepare_safe_manage_advisory_with_config(
        &assessment,
        &prepared_context.excerpts,
        &config,
    ) {
        Ok(value) => value,
        Err(error) => {
            let _ = db.connector_advisory_receipt_fail(&receipt_id, "payload_blocked");
            return Err(error);
        }
    };
    if let Err(error) = ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt) {
        let _ = db.connector_advisory_receipt_fail(&receipt_id, "provider_changed");
        return Err(error);
    }

    let advisory = match hangar_ai::send_prepared(reviewed) {
        Ok(value) => value,
        Err(error) => {
            let _ = db.connector_advisory_receipt_fail(&receipt_id, "provider_failed");
            return Err(error);
        }
    };
    if let Err(error) =
        resolve_safe_manage_ai_assessment(state, project_id, analysis_run_id, evidence_revision)
    {
        let _ = db.connector_advisory_receipt_fail(&receipt_id, "evidence_changed");
        return Err(format!(
            "The provider replied, but Safe Manage evidence changed while it was working. The response was not accepted as a current recommendation; analyze again. {error}"
        ));
    }
    let parsed = ai_assist::parse_safe_manage_ai_recommendation(&advisory);
    let ai_recommendation = parsed.map(|value| value.recommendation);
    let ai_confidence = parsed.map(|value| value.confidence);
    let recommendation_changed = ai_recommendation
        .map(|recommendation| recommendation != assessment.recommendation)
        .unwrap_or(false);
    let result_hash = blake3::hash(advisory.as_bytes()).to_hex().to_string();
    let receipt = db
        .connector_advisory_receipt_complete(
            &receipt_id,
            &result_hash,
            advisory.chars().count() as u64,
        )
        .map_err(to_message)?;
    Ok(AiSafeManageAdvisoryResult {
        advisory,
        deterministic_recommendation: assessment.recommendation,
        deterministic_confidence: assessment.confidence,
        ai_recommendation,
        ai_confidence,
        recommendation_changed,
        receipt,
    })
}

pub fn ai_safe_manage_advisory_receipts(
    state: &AppState,
    project_id: i64,
    limit: Option<usize>,
) -> Result<Vec<AiSafeManageAdvisoryReceipt>, String> {
    project_get(state, project_id)?
        .ok_or_else(|| "That project is no longer registered in Code Hangar.".to_string())?;
    state
        .db()?
        .connector_advisory_receipts(project_id, limit.unwrap_or(20))
        .map_err(to_message)
}

fn register_candidates(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
    drafts: Vec<ContextCandidateDraft>,
) -> Result<Vec<AiSafeManageContextCandidate>, String> {
    let now = u128::from(now_millis());
    let mut store = state
        .ai_safe_manage_contexts
        .lock()
        .map_err(|_| "The advisory context selector store is unavailable.".to_string())?;
    store.selections.retain(|_, selection| {
        now.saturating_sub(selection.created_ms) <= CONTEXT_SELECTION_TTL_MS
    });
    store.selections.retain(|_, selection| {
        selection.project_id != project_id
            || (selection.analysis_run_id == analysis_run_id
                && selection.evidence_revision == evidence_revision)
    });
    while store.selections.len().saturating_add(drafts.len()) > CONTEXT_SELECTION_CAP {
        let Some(oldest) = store
            .selections
            .iter()
            .min_by_key(|(_, selection)| selection.created_ms)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        store.selections.remove(&oldest);
    }

    let mut candidates = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let selection_id = unique_selection_id(&store.selections)?;
        store.selections.insert(
            selection_id.clone(),
            StoredContextSelection {
                project_id,
                analysis_run_id: analysis_run_id.to_string(),
                evidence_revision: evidence_revision.to_string(),
                kind: draft.kind,
                source: draft.source,
                created_ms: now,
            },
        );
        candidates.push(AiSafeManageContextCandidate {
            selection_id,
            kind: draft.kind,
            label: draft.label,
            detail: draft.detail,
            max_excerpt_chars: SELECTED_EXCERPT_MAX_CHARS as u64,
        });
    }
    Ok(candidates)
}

fn unique_selection_id(
    selections: &HashMap<String, StoredContextSelection>,
) -> Result<String, String> {
    for _ in 0..4 {
        let id = format!(
            "sm-context-{}",
            hangar_agent::random_token(24)
                .map_err(|_| "Could not create a secure context selector.".to_string())?
        );
        if !selections.contains_key(&id) {
            return Ok(id);
        }
    }
    Err("Could not create a unique context selector.".to_string())
}

fn prepare_selected_context(
    state: &AppState,
    assessment: &SafeManageProjectAssessment,
    selected_context_ids: &[String],
) -> Result<PreparedContext, String> {
    validate_selected_ids(selected_context_ids)?;
    let now = u128::from(now_millis());
    let selections = {
        let mut store = state
            .ai_safe_manage_contexts
            .lock()
            .map_err(|_| "The advisory context selector store is unavailable.".to_string())?;
        store.selections.retain(|_, selection| {
            now.saturating_sub(selection.created_ms) <= CONTEXT_SELECTION_TTL_MS
        });
        let mut selected = Vec::with_capacity(selected_context_ids.len());
        for selection_id in selected_context_ids {
            let selection = store.selections.get(selection_id).ok_or_else(|| {
                "That context selection is missing or expired. Choose the context again; nothing was sent."
                    .to_string()
            })?;
            if selection.project_id != assessment.project_id
                || selection.analysis_run_id != assessment.analysis_run_id
                || selection.evidence_revision != assessment.evidence_revision
            {
                return Err(
                    "That context selection belongs to different Safe Manage evidence. Choose the context again; nothing was sent."
                        .to_string(),
                );
            }
            selected.push((selection_id.clone(), selection.clone()));
        }
        selected
    };

    let mut excerpts = Vec::with_capacity(selections.len());
    let mut receipts = Vec::with_capacity(selections.len());
    for (selection_id, selection) in selections {
        let (excerpt, redaction_count) = read_and_gate_selection(state, assessment, &selection)?;
        let excerpt_chars = excerpt.chars().count() as u64;
        let content_hash = blake3::hash(excerpt.as_bytes()).to_hex().to_string();
        excerpts.push(ai_assist::SafeManageAdvisoryContextExcerpt {
            selection_id: selection_id.clone(),
            kind: selection.kind,
            excerpt,
            redaction_count,
        });
        receipts.push(AiSafeManageAdvisorySourceReceipt {
            selection_id,
            kind: selection.kind,
            content_hash,
            excerpt_chars,
            redaction_count,
        });
    }
    Ok(PreparedContext { excerpts, receipts })
}

fn read_and_gate_selection(
    state: &AppState,
    assessment: &SafeManageProjectAssessment,
    selection: &StoredContextSelection,
) -> Result<(String, u64), String> {
    let (raw, known_paths, prior_redactions) = match &selection.source {
        StoredContextSource::File { node_id } => {
            let preview = file_preview(
                state,
                *node_id,
                Some(assessment.project_id),
                PreviewMode::Source,
                Some(false),
                None,
            )?;
            if preview.state != PreviewState::Ready
                || !matches!(preview.file_kind, FileKind::Text | FileKind::Markdown)
            {
                return Err(
                    "Not sent — the selected file is no longer a readable, non-Protected text file. Nothing left your machine."
                        .to_string(),
                );
            }
            let source = preview.source.ok_or_else(|| {
                "Not sent — the selected file has no safe source preview. Nothing left your machine."
                    .to_string()
            })?;
            (
                source,
                vec![assessment.project_path.clone(), preview.path],
                0,
            )
        }
        StoredContextSource::Session {
            path,
            linked_project_paths,
        } => {
            let preview = session_preview(path.clone(), false)?;
            let text = preview.rendered_text.unwrap_or(preview.text);
            let mut paths = linked_project_paths.clone();
            paths.push(assessment.project_path.clone());
            paths.push(path.clone());
            (text, paths, u64::from(preview.redacted_count))
        }
    };

    let initial = raw
        .trim()
        .chars()
        .take(SELECTED_EXCERPT_READ_CHARS)
        .collect::<String>();
    if initial.is_empty() {
        return Err(
            "Not sent — the selected context excerpt is empty. Nothing left your machine."
                .to_string(),
        );
    }
    let mut redacted = initial;
    let mut redaction_count = prior_redactions;
    for path in known_paths {
        if path.len() < 3 {
            continue;
        }
        let next = redact_path_occurrences(&redacted, &path);
        if next != redacted {
            redaction_count = redaction_count.saturating_add(1);
            redacted = next;
        }
    }
    let (redacted, generic_path_redactions) = redact_absolute_path_tokens(&redacted);
    redaction_count = redaction_count.saturating_add(generic_path_redactions);
    let mut excerpt = redacted
        .trim()
        .chars()
        .take(SELECTED_EXCERPT_MAX_CHARS)
        .collect::<String>();
    if redacted.trim().chars().count() > SELECTED_EXCERPT_MAX_CHARS {
        excerpt.push('…');
    }
    if contains_absolute_path_token(&excerpt) {
        return Err(
            "Not sent — an absolute path remained in the selected context after redaction. Nothing left your machine."
                .to_string(),
        );
    }
    ai_assist::validate_safe_manage_advisory_excerpt(&excerpt)?;
    Ok((excerpt, redaction_count))
}

fn validate_selected_ids(selected_context_ids: &[String]) -> Result<(), String> {
    if selected_context_ids.is_empty() || selected_context_ids.len() > MAX_SELECTED_CONTEXTS {
        return Err(format!(
            "Select between one and {MAX_SELECTED_CONTEXTS} context items for AI recommendation enrichment."
        ));
    }
    let mut unique = HashSet::new();
    for selection_id in selected_context_ids {
        if selection_id.len() > 96
            || !selection_id.starts_with("sm-context-")
            || !selection_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || !unique.insert(selection_id)
        {
            return Err(
                "A context selector is invalid or repeated. Choose the context again; nothing was sent."
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn request_fingerprint(request: &hangar_ai::PreparedRequest) -> Result<(String, u64), String> {
    let disclosure = request.disclosure();
    let encoded = serde_json::to_vec(disclosure)
        .map_err(|error| format!("Could not fingerprint the reviewed advisory request: {error}"))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"code-hangar-connector-safe-manage-request-v1\0");
    hasher.update(&encoded);
    hasher.update(b"\0");
    hasher.update(request.model().as_bytes());
    hasher.update(b"\0");
    hasher.update(request.format().as_tag().as_bytes());
    hasher.update(b"\0");
    hasher.update(if request.is_local() { b"local" } else { b"api" });
    let chars = disclosure.request_body.chars().count().saturating_add(
        disclosure
            .fallback_request_body
            .as_deref()
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0),
    ) as u64;
    Ok((hasher.finalize().to_hex().to_string(), chars))
}

fn curated_file_kind(display_name: &str) -> Option<AiSafeManageContextKind> {
    match display_name.trim().to_ascii_lowercase().as_str() {
        "readme" | "readme.md" | "readme.txt" => Some(AiSafeManageContextKind::Readme),
        "package.json" | "pyproject.toml" | "cargo.toml" | "go.mod" | "requirements.txt"
        | "pom.xml" | "build.gradle" | "build.gradle.kts" | "composer.json" => {
            Some(AiSafeManageContextKind::Manifest)
        }
        _ => None,
    }
}

fn safe_file_label(kind: AiSafeManageContextKind, display_name: &str) -> String {
    match kind {
        AiSafeManageContextKind::Readme => "README context".to_string(),
        AiSafeManageContextKind::Manifest => {
            match display_name.trim().to_ascii_lowercase().as_str() {
                "package.json" => "package.json manifest",
                "pyproject.toml" => "pyproject.toml manifest",
                "cargo.toml" => "Cargo.toml manifest",
                "go.mod" => "go.mod manifest",
                "requirements.txt" => "requirements.txt manifest",
                "pom.xml" => "pom.xml manifest",
                "build.gradle" => "Gradle manifest",
                "build.gradle.kts" => "Gradle Kotlin manifest",
                "composer.json" => "composer.json manifest",
                _ => "Project manifest",
            }
            .to_string()
        }
        AiSafeManageContextKind::CoreFile => {
            match display_name.trim().to_ascii_lowercase().as_str() {
                "main.rs" => "Rust main entry file",
                "lib.rs" => "Rust library entry file",
                "main.py" => "Python main entry file",
                "app.py" => "Python app entry file",
                "app.tsx" | "app.ts" => "Application entry file",
                "index.tsx" | "index.ts" => "TypeScript entry file",
                "main.tsx" | "main.ts" => "TypeScript main entry file",
                "program.cs" => "C# entry file",
                "main.go" => "Go main entry file",
                "mod.rs" => "Rust module entry file",
                _ => "Core project file",
            }
            .to_string()
        }
        AiSafeManageContextKind::SessionExcerpt => "Linked AI session".to_string(),
    }
}

fn file_context_detail(kind: AiSafeManageContextKind) -> &'static str {
    match kind {
        AiSafeManageContextKind::Readme => {
            "Bounded README excerpt, re-read and safety-gated immediately before sending."
        }
        AiSafeManageContextKind::Manifest => {
            "Bounded manifest excerpt, re-read and safety-gated immediately before sending."
        }
        AiSafeManageContextKind::CoreFile => {
            "Bounded central-file excerpt, re-read and safety-gated immediately before sending."
        }
        AiSafeManageContextKind::SessionExcerpt => {
            "Bounded linked-session excerpt with credentials and paths removed."
        }
    }
}

fn safe_session_source_label(source_kind: &str) -> &'static str {
    let lower = source_kind.to_ascii_lowercase();
    if lower.contains("claude") {
        "Claude"
    } else if lower.contains("codex") {
        "Codex"
    } else if lower.contains("cursor") {
        "Cursor"
    } else if lower.contains("gemini") || lower.contains("antigravity") {
        "Gemini"
    } else if lower.contains("hermes") {
        "Hermes"
    } else {
        "AI"
    }
}

/// Conservative token redaction for remaining absolute paths. Known full paths
/// are removed first, so this is a defence-in-depth catch for unrelated paths
/// embedded in source/session text.
fn redact_absolute_path_tokens(input: &str) -> (String, u64) {
    let mut output = String::with_capacity(input.len());
    let mut count = 0u64;
    let mut quoted_path: Option<char> = None;
    let mut path_continuation = false;
    for part in input.split_inclusive(char::is_whitespace) {
        let core_len = part.trim_end_matches(char::is_whitespace).len();
        let (core, whitespace) = part.split_at(core_len);
        let has_separator = core.contains('\\') || core.contains('/');
        let is_path = quoted_path.is_some()
            || token_contains_absolute_path(core)
            || (path_continuation && has_separator)
            || core.contains('\\');
        if is_path && !core.is_empty() {
            if quoted_path.is_none() {
                quoted_path = unmatched_quote(core);
            } else if let Some(quote) = quoted_path {
                if core.contains(quote) {
                    quoted_path = None;
                }
            }
            path_continuation = quoted_path.is_some() || has_separator;
            output.push_str("[redacted absolute path]");
            count = count.saturating_add(1);
        } else {
            path_continuation = false;
            output.push_str(core);
        }
        output.push_str(whitespace);
    }
    (output, count)
}

fn contains_absolute_path_token(input: &str) -> bool {
    input.split_whitespace().any(token_contains_absolute_path)
}

fn token_contains_absolute_path(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    });
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("file:/") || trimmed.starts_with("\\\\") || trimmed.starts_with("//") {
        return true;
    }
    if trimmed.starts_with('/') && !lower.starts_with("//") {
        return true;
    }
    let bytes = trimmed.as_bytes();
    bytes.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'/' | b'\\')
    })
}

fn unmatched_quote(token: &str) -> Option<char> {
    ['"', '\'']
        .into_iter()
        .find(|quote| token.matches(*quote).count() % 2 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assessment(project_id: i64) -> SafeManageProjectAssessment {
        serde_json::from_value(serde_json::json!({
            "analysisRunId": "safe-manage-test-run",
            "projectId": project_id,
            "projectName": "Local project",
            "projectPath": "C:\\work\\local-project",
            "lifecycle": "needs_review",
            "recommendation": "review",
            "confidence": "unknown",
            "reasonCode": "insufficient_evidence",
            "reason": "Evidence is incomplete.",
            "rulesetVersion": "safe-manage-objective-v1",
            "evidenceRevision": "a".repeat(64),
            "evidenceStale": false,
            "lastActivityMs": null,
            "apps": [],
            "sessionCount": null,
            "hasGit": false,
            "gitHasRemote": null,
            "gitUncommitted": null,
            "apparentBytes": null,
            "physicalBytes": null,
            "footprintPartial": true,
            "signals": [],
            "importantFiles": [],
            "riskRelations": []
        }))
        .expect("assessment fixture")
    }

    #[test]
    fn selected_ids_are_opaque_unique_and_bounded() {
        assert!(validate_selected_ids(&["sm-context-valid_123".to_string()]).is_ok());
        assert!(validate_selected_ids(&[r"C:\Users\user\README.md".to_string()]).is_err());
        assert!(validate_selected_ids(&[
            "sm-context-repeat".to_string(),
            "sm-context-repeat".to_string(),
        ])
        .is_err());
        assert!(validate_selected_ids(&[]).is_err());
    }

    #[test]
    fn absolute_paths_are_redacted_including_quoted_paths_with_spaces() {
        let input =
            "Open C:\\Users\\user\\project\\README.md then '/home/user/My Project/main.py'.";
        let (redacted, count) = redact_absolute_path_tokens(input);
        assert!(count >= 2, "{redacted}");
        assert!(!redacted.contains("owner"), "{redacted}");
        assert!(!contains_absolute_path_token(&redacted), "{redacted}");
    }

    #[test]
    fn safe_labels_never_echo_an_arbitrary_filename_or_session_source() {
        assert_eq!(
            safe_file_label(AiSafeManageContextKind::CoreFile, "private-client-name.ts"),
            "Core project file"
        );
        assert_eq!(safe_session_source_label("custom-private-agent"), "AI");
    }

    #[test]
    fn connector_enriches_confident_results_but_never_a_do_not_touch_floor() {
        let mut candidate = assessment(7);
        for recommendation in [
            SafeManageRecommendation::Keep,
            SafeManageRecommendation::Review,
            SafeManageRecommendation::Archive,
            SafeManageRecommendation::CleanRegenerables,
            SafeManageRecommendation::RemovalCandidate,
        ] {
            candidate.recommendation = recommendation;
            candidate.confidence = SafeManageConfidence::High;
            assert!(
                ai_assist::validate_safe_manage_ai_enrichment_assessment(&candidate).is_ok(),
                "{recommendation:?}"
            );
        }
        candidate.recommendation = SafeManageRecommendation::DoNotTouch;
        assert!(ai_assist::validate_safe_manage_ai_enrichment_assessment(&candidate).is_err());
        candidate.recommendation = SafeManageRecommendation::Keep;
        candidate.evidence_stale = true;
        assert!(ai_assist::validate_safe_manage_ai_enrichment_assessment(&candidate).is_err());
    }

    #[test]
    fn forged_or_cross_project_context_selector_is_rejected_before_any_read() {
        let state = AppState::memory().expect("memory app state");
        let assessment = assessment(7);
        let forged = vec!["sm-context-forged-selector".to_string()];
        assert!(prepare_selected_context(&state, &assessment, &forged)
            .unwrap_err()
            .contains("missing or expired"));

        state
            .ai_safe_manage_contexts
            .lock()
            .expect("selector store")
            .selections
            .insert(
                forged[0].clone(),
                StoredContextSelection {
                    project_id: 8,
                    analysis_run_id: assessment.analysis_run_id.clone(),
                    evidence_revision: assessment.evidence_revision.clone(),
                    kind: AiSafeManageContextKind::Readme,
                    source: StoredContextSource::File { node_id: 999_999 },
                    created_ms: u128::from(now_millis()),
                },
            );
        assert!(prepare_selected_context(&state, &assessment, &forged)
            .unwrap_err()
            .contains("different Safe Manage evidence"));
    }
}
