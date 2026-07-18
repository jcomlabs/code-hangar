use hangar_core::{
    SessionChangeCoverage, SessionChangeEdit, SessionChangeSet, SessionDiffHunk, SessionDiffLine,
    SessionFileChange,
};
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

const MAX_JSONL_RECORD_BYTES: usize = 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 1200;
const MAX_CAPTURE_BYTES: usize = 192 * 1024;
const MAX_CAPTURE_LINES: usize = 800;

struct PendingChange {
    path: String,
    edit: SessionChangeEdit,
}

#[derive(Default)]
struct ParseState {
    source_codex: bool,
    source_claude: bool,
    saw_turn_diff: bool,
    saw_direct_edit: bool,
    latest_request: Option<String>,
    changes: Vec<PendingChange>,
    redacted_count: u32,
    parsed_records: u64,
    omitted_records: u64,
}

pub(crate) fn unsupported_change_set(path: String) -> SessionChangeSet {
    SessionChangeSet {
        path,
        source_kind: "Unsupported session store".to_string(),
        coverage: SessionChangeCoverage {
            level: "none".to_string(),
            label: "No reconstructable edit history".to_string(),
            note: "This local session format can be read as a conversation, but it does not expose deterministic file-edit records that Code Hangar can reconstruct yet."
                .to_string(),
        },
        files: Vec::new(),
        edit_count: 0,
        added_lines: 0,
        removed_lines: 0,
        redacted_count: 0,
        parsed_records: 0,
        omitted_records: 0,
    }
}

pub(crate) fn build_session_change_set(
    path: &Path,
    requested_path: String,
) -> Result<SessionChangeSet, String> {
    let file = File::open(path).map_err(super::to_message)?;
    parse_reader(BufReader::new(file), requested_path).map_err(super::to_message)
}

pub(crate) struct GitChangeEvidence<'a> {
    pub committed_patch: &'a str,
    pub staged_patch: &'a str,
    pub working_patch: &'a str,
    pub untracked_paths: &'a [String],
    pub output_truncated: bool,
    pub committed_since_requested: bool,
    pub committed_since_unavailable: bool,
}

pub(crate) fn build_git_change_set(
    requested_path: String,
    evidence: GitChangeEvidence<'_>,
) -> SessionChangeSet {
    let mut state = ParseState {
        source_codex: true,
        saw_turn_diff: true,
        parsed_records: 2 + u64::from(evidence.committed_since_requested),
        ..ParseState::default()
    };
    let committed_changes = changes_from_patch(
        evidence.committed_patch,
        "Git commits since last review",
        None,
        &mut state.redacted_count,
    );
    state.changes.extend(committed_changes);
    let staged_changes = changes_from_patch(
        evidence.staged_patch,
        "Git staged diff",
        None,
        &mut state.redacted_count,
    );
    state.changes.extend(staged_changes);
    let working_changes = changes_from_patch(
        evidence.working_patch,
        "Git working tree diff",
        None,
        &mut state.redacted_count,
    );
    state.changes.extend(working_changes);
    for path in evidence.untracked_paths {
        state.changes.push(PendingChange {
            path: sanitize_text(path, 4096, &mut state.redacted_count),
            edit: SessionChangeEdit {
                source: "Git status".to_string(),
                summary: "Untracked file recorded by Git".to_string(),
                provenance: Some("Current local Git status".to_string()),
                confidence: Some("observed".to_string()),
                reality: None,
                request: None,
                hunks: vec![SessionDiffHunk {
                    header: "Content not opened automatically".to_string(),
                    old_start: None,
                    new_start: None,
                    lines: vec![SessionDiffLine {
                        kind: "note".to_string(),
                        content: "Git reports this path as untracked. Code Hangar did not read its body for this recap."
                            .to_string(),
                        old_line: None,
                        new_line: None,
                    }],
                }],
                added_lines: 0,
                removed_lines: 0,
            },
        });
    }
    let mut change_set = finalize(requested_path, state);
    change_set.source_kind = "Local Git".to_string();
    change_set.coverage = SessionChangeCoverage {
        level: if evidence.output_truncated || evidence.committed_since_unavailable {
            "direct_edits".to_string()
        } else {
            "full".to_string()
        },
        label: if evidence.output_truncated || evidence.committed_since_unavailable {
            "Bounded local Git evidence".to_string()
        } else {
            "Current local Git evidence".to_string()
        },
        note: if evidence.output_truncated {
            "Tracked staged and working-tree patches exceeded the bounded review window. The visible paths are evidence, but this is not the complete diff. Untracked file bodies are never opened automatically."
                .to_string()
        } else if evidence.committed_since_unavailable {
            "Current staged and working-tree evidence is available, but the saved reviewed commit can no longer be resolved locally. The committed-since portion is therefore unavailable. No remote was contacted."
                .to_string()
        } else if evidence.committed_since_requested {
            "Combines commits since the last review checkpoint with local staged and working-tree patches. Untracked paths are listed from Git status without opening their bodies. No fetch, pull, push or other remote Git operation is used."
                .to_string()
        } else {
            "Combines local staged and working-tree patches. Commit history will be compared after the first Mark reviewed checkpoint. Untracked paths are listed from Git status without opening their bodies. No fetch, pull, push or other remote Git operation is used."
                .to_string()
        },
    };
    change_set
}

pub(crate) fn build_cursor_change_set(
    requested_path: String,
    records: Vec<hangar_discovery::CursorRecordedEdit>,
) -> SessionChangeSet {
    let mut redacted_count = 0u32;
    let parsed_records = records.len() as u64;
    let mut changes = Vec::new();
    for record in records {
        let path = sanitize_text(&record.path, 4096, &mut redacted_count);
        let request = record
            .request
            .as_deref()
            .map(|value| sanitize_text(value, MAX_PROMPT_BYTES, &mut redacted_count));
        let lines = record
            .lines
            .into_iter()
            .map(|line| SessionDiffLine {
                kind: match line.kind.as_str() {
                    "added" | "removed" | "context" | "note" => line.kind,
                    _ => "note".to_string(),
                },
                content: sanitize_text(&line.content, MAX_CAPTURE_BYTES, &mut redacted_count),
                old_line: line.old_line,
                new_line: line.new_line,
            })
            .collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        let old_start = lines.iter().find_map(|line| line.old_line);
        let new_start = lines.iter().find_map(|line| line.new_line);
        let added_lines = lines.iter().filter(|line| line.kind == "added").count() as u64;
        let removed_lines = lines.iter().filter(|line| line.kind == "removed").count() as u64;
        changes.push(PendingChange {
            path,
            edit: SessionChangeEdit {
                source: "Cursor edit_file_v2".to_string(),
                summary: "Recorded Cursor file edit".to_string(),
                provenance: Some("Recorded Cursor diff bubble".to_string()),
                confidence: Some("observed".to_string()),
                reality: None,
                request,
                hunks: vec![SessionDiffHunk {
                    header: "Recorded Cursor edit".to_string(),
                    old_start,
                    new_start,
                    lines,
                }],
                added_lines,
                removed_lines,
            },
        });
    }

    let mut files: Vec<SessionFileChange> = Vec::new();
    for change in changes {
        if let Some(existing) = files.iter_mut().find(|file| file.path == change.path) {
            existing.added_lines += change.edit.added_lines;
            existing.removed_lines += change.edit.removed_lines;
            existing.edits.push(change.edit);
        } else {
            files.push(SessionFileChange {
                path: change.path,
                added_lines: change.edit.added_lines,
                removed_lines: change.edit.removed_lines,
                edits: vec![change.edit],
                reality: None,
            });
        }
    }
    let edit_count = files.iter().map(|file| file.edits.len() as u64).sum();
    let added_lines = files.iter().map(|file| file.added_lines).sum();
    let removed_lines = files.iter().map(|file| file.removed_lines).sum();
    let has_edits = edit_count > 0;
    SessionChangeSet {
        path: requested_path,
        source_kind: "Cursor".to_string(),
        coverage: SessionChangeCoverage {
            level: if has_edits { "direct_edits" } else { "none" }.to_string(),
            label: if has_edits {
                "Recorded Cursor edits"
            } else {
                "No reconstructable Cursor edits"
            }
            .to_string(),
            note: if has_edits {
                "Reconstructed only from Cursor edit_file_v2 bubbles with recorded precomputed diffs. The reader is bounded to the recent conversation window, 500 edits and 4 MiB; terminal commands, other tools and older omitted records may also have changed files."
            } else {
                "This conversation has no supported Cursor edit_file_v2 diff bubbles in the bounded local record. Code Hangar will not infer changes from prose."
            }
            .to_string(),
        },
        files,
        edit_count,
        added_lines,
        removed_lines,
        redacted_count,
        parsed_records,
        omitted_records: 0,
    }
}

fn parse_reader<R: BufRead>(mut reader: R, requested_path: String) -> io::Result<SessionChangeSet> {
    let mut state = ParseState::default();
    while let Some((line, exceeded)) = read_bounded_line(&mut reader, MAX_JSONL_RECORD_BYTES)? {
        if exceeded {
            state.omitted_records += 1;
            continue;
        }
        let record: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                state.omitted_records += 1;
                continue;
            }
        };
        state.parsed_records += 1;
        observe_record(&record, &mut state);
    }
    Ok(finalize(requested_path, state))
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    let mut exceeded = false;
    let mut saw_data = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if saw_data {
                Ok(Some((line, exceeded)))
            } else {
                Ok(None)
            };
        }
        saw_data = true;
        let consumed = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(buffer.len());
        if !exceeded {
            let remaining = limit.saturating_sub(line.len());
            let copied = consumed.min(remaining);
            line.extend_from_slice(&buffer[..copied]);
            if copied < consumed || (line.len() == limit && buffer.get(copied) != Some(&b'\n')) {
                exceeded = true;
            }
        }
        let ended = buffer[..consumed].last() == Some(&b'\n');
        reader.consume(consumed);
        if ended {
            return Ok(Some((line, exceeded)));
        }
    }
}

fn observe_record(record: &Value, state: &mut ParseState) {
    let Some(object) = record.as_object() else {
        return;
    };
    let outer_type = string_field(object, "type").unwrap_or_default();
    if matches!(
        outer_type,
        "session_meta" | "event_msg" | "response_item" | "turn_context"
    ) {
        state.source_codex = true;
    }
    if object.contains_key("parentUuid") || matches!(outer_type, "assistant" | "user") {
        state.source_claude = true;
    }

    if let Some(prompt) = extract_user_prompt(object) {
        let prompt = sanitize_text(&prompt, MAX_PROMPT_BYTES, &mut state.redacted_count);
        if !prompt.is_empty() {
            state.latest_request = Some(prompt);
        }
    }

    observe_codex_change(object, state);
    observe_claude_changes(object, state);
}

fn observe_codex_change(record: &Map<String, Value>, state: &mut ParseState) {
    let payload = record.get("payload").and_then(Value::as_object);
    let Some(payload) = payload else {
        return;
    };
    if string_field(payload, "type") == Some("turn_diff") {
        state.source_codex = true;
        state.saw_turn_diff = true;
        if let Some(patch) = first_string(payload, &["unified_diff", "diff", "patch", "turn_diff"])
        {
            let changes = changes_from_patch(
                patch,
                "ChatGPT turn diff",
                state.latest_request.clone(),
                &mut state.redacted_count,
            );
            state.changes.extend(changes);
        } else {
            state.omitted_records += 1;
        }
        return;
    }

    let name = string_field(payload, "name").unwrap_or_default();
    if name != "apply_patch" {
        return;
    }
    let Some(patch) = tool_patch_input(payload) else {
        state.omitted_records += 1;
        return;
    };
    state.source_codex = true;
    state.saw_direct_edit = true;
    state.changes.extend(changes_from_patch(
        &patch,
        "ChatGPT apply_patch",
        state.latest_request.clone(),
        &mut state.redacted_count,
    ));
}

fn observe_claude_changes(record: &Map<String, Value>, state: &mut ParseState) {
    let Some(message) = record.get("message").and_then(Value::as_object) else {
        return;
    };
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in content {
        let Some(tool) = part.as_object() else {
            continue;
        };
        if string_field(tool, "type") != Some("tool_use") {
            continue;
        }
        let name = string_field(tool, "name").unwrap_or_default();
        if !matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
            continue;
        }
        let Some(input) = tool.get("input").and_then(Value::as_object) else {
            state.omitted_records += 1;
            continue;
        };
        state.source_claude = true;
        state.saw_direct_edit = true;
        match name {
            "Edit" => observe_claude_edit(input, "Claude Edit", state),
            "Write" => observe_claude_write(input, state),
            "MultiEdit" => observe_claude_multi_edit(input, state),
            "NotebookEdit" => observe_claude_notebook_edit(input, state),
            _ => {}
        }
    }
}

fn observe_claude_edit(input: &Map<String, Value>, source: &str, state: &mut ParseState) {
    let Some(path) = first_string(input, &["file_path", "path"]) else {
        state.omitted_records += 1;
        return;
    };
    let old = first_string(input, &["old_string", "old_text"]).unwrap_or_default();
    let new = first_string(input, &["new_string", "new_text"]).unwrap_or_default();
    let summary = if input.get("replace_all").and_then(Value::as_bool) == Some(true) {
        "Replaced every recorded match"
    } else {
        "Replaced recorded text"
    };
    push_direct_change(path, old, new, source, summary, state);
}

fn observe_claude_write(input: &Map<String, Value>, state: &mut ParseState) {
    let Some(path) = first_string(input, &["file_path", "path"]) else {
        state.omitted_records += 1;
        return;
    };
    let content = first_string(input, &["content"]).unwrap_or_default();
    push_direct_change(
        path,
        "",
        content,
        "Claude Write",
        "Recorded a file write",
        state,
    );
}

fn observe_claude_multi_edit(input: &Map<String, Value>, state: &mut ParseState) {
    let Some(path) = first_string(input, &["file_path", "path"]) else {
        state.omitted_records += 1;
        return;
    };
    let Some(edits) = input.get("edits").and_then(Value::as_array) else {
        state.omitted_records += 1;
        return;
    };
    for edit in edits {
        let Some(edit) = edit.as_object() else {
            state.omitted_records += 1;
            continue;
        };
        let old = first_string(edit, &["old_string", "old_text"]).unwrap_or_default();
        let new = first_string(edit, &["new_string", "new_text"]).unwrap_or_default();
        push_direct_change(
            path,
            old,
            new,
            "Claude MultiEdit",
            "Replaced recorded text",
            state,
        );
    }
}

fn observe_claude_notebook_edit(input: &Map<String, Value>, state: &mut ParseState) {
    let Some(path) = first_string(input, &["notebook_path", "file_path", "path"]) else {
        state.omitted_records += 1;
        return;
    };
    let old = first_string(input, &["old_source"]).unwrap_or_default();
    let new = first_string(input, &["new_source", "content"]).unwrap_or_default();
    push_direct_change(
        path,
        old,
        new,
        "Claude NotebookEdit",
        "Changed a recorded notebook cell",
        state,
    );
}

fn push_direct_change(
    path: &str,
    old: &str,
    new: &str,
    source: &str,
    summary: &str,
    state: &mut ParseState,
) {
    let path = sanitize_text(path, 4096, &mut state.redacted_count);
    let mut lines = direct_lines(old, "removed", &mut state.redacted_count);
    lines.extend(direct_lines(new, "added", &mut state.redacted_count));
    let removed_lines = lines.iter().filter(|line| line.kind == "removed").count() as u64;
    let added_lines = lines.iter().filter(|line| line.kind == "added").count() as u64;
    state.changes.push(PendingChange {
        path,
        edit: SessionChangeEdit {
            source: source.to_string(),
            summary: summary.to_string(),
            provenance: Some("Recorded session tool action".to_string()),
            confidence: Some("observed".to_string()),
            reality: None,
            request: state.latest_request.clone(),
            hunks: vec![SessionDiffHunk {
                header: "Recorded edit (exact line positions unavailable)".to_string(),
                old_start: None,
                new_start: None,
                lines,
            }],
            added_lines,
            removed_lines,
        },
    });
}

fn direct_lines(input: &str, kind: &str, redacted_count: &mut u32) -> Vec<SessionDiffLine> {
    let mut output = Vec::new();
    let mut captured = 0usize;
    for line in input.lines().take(MAX_CAPTURE_LINES) {
        if captured.saturating_add(line.len()) > MAX_CAPTURE_BYTES {
            break;
        }
        captured += line.len();
        output.push(SessionDiffLine {
            kind: kind.to_string(),
            content: sanitize_text(line, MAX_CAPTURE_BYTES, redacted_count),
            old_line: None,
            new_line: None,
        });
    }
    if input.lines().count() > output.len() || input.len() > captured {
        output.push(SessionDiffLine {
            kind: "note".to_string(),
            content: "Additional recorded lines were omitted from this bounded recap.".to_string(),
            old_line: None,
            new_line: None,
        });
    }
    output
}

fn changes_from_patch(
    patch: &str,
    source: &str,
    request: Option<String>,
    redacted_count: &mut u32,
) -> Vec<PendingChange> {
    PatchCollector::new(source, request, redacted_count).parse(patch)
}

struct PatchCollector<'a> {
    source: &'a str,
    request: Option<String>,
    redacted_count: &'a mut u32,
    path: Option<String>,
    hunks: Vec<SessionDiffHunk>,
    hunk: Option<SessionDiffHunk>,
    old_line: Option<u64>,
    new_line: Option<u64>,
    output: Vec<PendingChange>,
}

impl<'a> PatchCollector<'a> {
    fn new(source: &'a str, request: Option<String>, redacted_count: &'a mut u32) -> Self {
        Self {
            source,
            request,
            redacted_count,
            path: None,
            hunks: Vec::new(),
            hunk: None,
            old_line: None,
            new_line: None,
            output: Vec::new(),
        }
    }

    fn parse(mut self, patch: &str) -> Vec<PendingChange> {
        for line in patch.lines() {
            if let Some(path) = patch_file_marker(line) {
                self.finish_file();
                self.path = Some(sanitize_text(path, 4096, self.redacted_count));
                continue;
            }
            if let Some(path) = line.strip_prefix("diff --git ").and_then(diff_git_path) {
                self.finish_file();
                self.path = Some(sanitize_text(path, 4096, self.redacted_count));
                continue;
            }
            if let Some(path) = line.strip_prefix("+++ ").and_then(clean_diff_path) {
                if self.path.is_none() {
                    self.path = Some(sanitize_text(path, 4096, self.redacted_count));
                }
                continue;
            }
            if line.starts_with("--- ")
                || line == "*** Begin Patch"
                || line == "*** End Patch"
                || line.starts_with("index ")
            {
                continue;
            }
            if line.starts_with("@@") {
                self.finish_hunk();
                let (old_start, new_start) = parse_hunk_starts(line);
                self.old_line = old_start;
                self.new_line = new_start;
                self.hunk = Some(SessionDiffHunk {
                    header: sanitize_text(line, 4096, self.redacted_count),
                    old_start,
                    new_start,
                    lines: Vec::new(),
                });
                continue;
            }
            if self.path.is_none() {
                continue;
            }
            let (kind, content) = if let Some(value) = line.strip_prefix('+') {
                ("added", value)
            } else if let Some(value) = line.strip_prefix('-') {
                ("removed", value)
            } else if let Some(value) = line.strip_prefix(' ') {
                ("context", value)
            } else if line == "\\ No newline at end of file" {
                ("note", line)
            } else {
                continue;
            };
            if self.hunk.is_none() {
                self.hunk = Some(SessionDiffHunk {
                    header: "Recorded patch".to_string(),
                    old_start: None,
                    new_start: None,
                    lines: Vec::new(),
                });
            }
            let (old_line, new_line) = match kind {
                "added" => (None, self.take_new_line()),
                "removed" => (self.take_old_line(), None),
                "context" => (self.take_old_line(), self.take_new_line()),
                _ => (None, None),
            };
            if let Some(hunk) = self.hunk.as_mut() {
                if hunk.lines.len() < MAX_CAPTURE_LINES {
                    hunk.lines.push(SessionDiffLine {
                        kind: kind.to_string(),
                        content: sanitize_text(content, MAX_CAPTURE_BYTES, self.redacted_count),
                        old_line,
                        new_line,
                    });
                }
            }
        }
        self.finish_file();
        self.output
    }

    fn take_old_line(&mut self) -> Option<u64> {
        let current = self.old_line;
        self.old_line = self.old_line.map(|line| line + 1);
        current
    }

    fn take_new_line(&mut self) -> Option<u64> {
        let current = self.new_line;
        self.new_line = self.new_line.map(|line| line + 1);
        current
    }

    fn finish_hunk(&mut self) {
        if let Some(hunk) = self.hunk.take() {
            if !hunk.lines.is_empty() {
                self.hunks.push(hunk);
            }
        }
    }

    fn finish_file(&mut self) {
        self.finish_hunk();
        let Some(path) = self.path.take() else {
            self.hunks.clear();
            return;
        };
        if self.hunks.is_empty() {
            return;
        }
        let added_lines = count_kind(&self.hunks, "added");
        let removed_lines = count_kind(&self.hunks, "removed");
        let hunks = std::mem::take(&mut self.hunks);
        self.output.push(PendingChange {
            path,
            edit: SessionChangeEdit {
                source: self.source.to_string(),
                summary: "Recorded file patch".to_string(),
                provenance: Some("Recorded session patch".to_string()),
                confidence: Some("observed".to_string()),
                reality: None,
                request: self.request.clone(),
                hunks,
                added_lines,
                removed_lines,
            },
        });
    }
}

fn finalize(requested_path: String, state: ParseState) -> SessionChangeSet {
    let ParseState {
        source_codex,
        source_claude,
        saw_turn_diff,
        saw_direct_edit,
        changes,
        redacted_count,
        parsed_records,
        omitted_records,
        ..
    } = state;
    let source_kind = match (source_codex, source_claude) {
        (true, true) => "ChatGPT and Claude",
        (true, false) => "ChatGPT",
        (false, true) => "Claude",
        (false, false) => "Unknown JSONL session",
    }
    .to_string();
    let coverage = if saw_turn_diff {
        SessionChangeCoverage {
            level: "full".to_string(),
            label: "Full recorded turn diff".to_string(),
            note: "Reconstructed from ChatGPT CLI turn-diff records. Changes made outside recorded diffs, including some shell commands, may still be absent."
                .to_string(),
        }
    } else if saw_direct_edit {
        SessionChangeCoverage {
            level: "direct_edits".to_string(),
            label: "Recorded direct edits only".to_string(),
            note: if source_claude {
                "Reconstructed from Claude Edit, Write, MultiEdit and NotebookEdit calls. File changes made through Bash or other tools are not inferable from these records."
                    .to_string()
            } else {
                "Reconstructed from recorded ChatGPT CLI apply_patch calls. Changes made through shell commands or unrecorded tools may be absent."
                    .to_string()
            },
        }
    } else {
        SessionChangeCoverage {
            level: "none".to_string(),
            label: "No reconstructable edits found".to_string(),
            note: "The conversation remains available, but this session contains no supported deterministic file-edit records. Code Hangar will not guess what changed."
                .to_string(),
        }
    };

    let mut files: Vec<SessionFileChange> = Vec::new();
    for change in changes {
        if let Some(existing) = files.iter_mut().find(|file| file.path == change.path) {
            existing.added_lines += change.edit.added_lines;
            existing.removed_lines += change.edit.removed_lines;
            existing.edits.push(change.edit);
        } else {
            files.push(SessionFileChange {
                path: change.path,
                added_lines: change.edit.added_lines,
                removed_lines: change.edit.removed_lines,
                edits: vec![change.edit],
                reality: None,
            });
        }
    }
    let edit_count = files.iter().map(|file| file.edits.len() as u64).sum();
    let added_lines = files.iter().map(|file| file.added_lines).sum();
    let removed_lines = files.iter().map(|file| file.removed_lines).sum();
    SessionChangeSet {
        path: requested_path,
        source_kind,
        coverage,
        files,
        edit_count,
        added_lines,
        removed_lines,
        redacted_count,
        parsed_records,
        omitted_records,
    }
}

fn extract_user_prompt(record: &Map<String, Value>) -> Option<String> {
    if let Some(payload) = record.get("payload").and_then(Value::as_object) {
        let payload_type = string_field(payload, "type").unwrap_or_default();
        if payload_type == "user_message" {
            return first_string(payload, &["message", "text"]).map(str::to_string);
        }
        if payload_type == "message" && string_field(payload, "role") == Some("user") {
            return content_text(payload.get("content"));
        }
    }
    if string_field(record, "type") == Some("user") {
        if let Some(message) = record.get("message").and_then(Value::as_object) {
            return content_text(message.get("content"));
        }
    }
    None
}

fn content_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(Value::as_object)
                .filter(|part| {
                    matches!(
                        string_field(part, "type"),
                        Some("text") | Some("input_text")
                    )
                })
                .filter_map(|part| first_string(part, &["text", "content"]))
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn tool_patch_input(payload: &Map<String, Value>) -> Option<String> {
    let value = payload.get("input").or_else(|| payload.get("arguments"))?;
    if let Some(text) = value.as_str() {
        if text.contains("*** Begin Patch") || text.contains("diff --git") {
            return Some(text.to_string());
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if let Some(object) = parsed.as_object() {
                return first_string(object, &["patch", "input"]).map(str::to_string);
            }
        }
    }
    value
        .as_object()
        .and_then(|object| first_string(object, &["patch", "input"]))
        .map(str::to_string)
}

fn patch_file_marker(line: &str) -> Option<&str> {
    ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
}

fn diff_git_path(value: &str) -> Option<&str> {
    let mut parts = value.split_whitespace();
    let _old = parts.next()?;
    clean_diff_path(parts.next()?)
}

fn clean_diff_path(value: &str) -> Option<&str> {
    let path = value.trim().trim_matches('"');
    if path == "/dev/null" {
        None
    } else {
        Some(
            path.strip_prefix("b/")
                .or_else(|| path.strip_prefix("a/"))
                .unwrap_or(path),
        )
    }
}

fn parse_hunk_starts(header: &str) -> (Option<u64>, Option<u64>) {
    let mut old = None;
    let mut new = None;
    for token in header.split_whitespace() {
        if let Some(value) = token.strip_prefix('-') {
            old = value.split(',').next().and_then(|value| value.parse().ok());
        } else if let Some(value) = token.strip_prefix('+') {
            new = value.split(',').next().and_then(|value| value.parse().ok());
        }
    }
    (old, new)
}

fn count_kind(hunks: &[SessionDiffHunk], kind: &str) -> u64 {
    hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == kind)
        .count() as u64
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn first_string<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| string_field(object, key))
}

fn sanitize_text(input: &str, max_bytes: usize, redacted_count: &mut u32) -> String {
    let (redacted, count) = super::redact_secrets(input);
    *redacted_count = redacted_count.saturating_add(count);
    if redacted.len() <= max_bytes {
        return redacted;
    }
    let mut boundary = max_bytes;
    while !redacted.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}...", &redacted[..boundary])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Cursor;

    fn parse(input: &str) -> SessionChangeSet {
        parse_reader(Cursor::new(input.as_bytes()), "session.jsonl".to_string()).unwrap()
    }

    #[test]
    fn reconstructs_cursor_diff_bubbles_without_inventing_line_numbers() {
        let result = build_cursor_change_set(
            "state.vscdb#cursor-ide-chat=fixture".to_string(),
            vec![hangar_discovery::CursorRecordedEdit {
                path: "src/app.ts".to_string(),
                request: Some("Replace sk-test-12345678901234567890".to_string()),
                lines: vec![
                    hangar_discovery::CursorRecordedDiffLine {
                        kind: "removed".to_string(),
                        content: "const token = 'sk-test-12345678901234567890';".to_string(),
                        old_line: Some(7),
                        new_line: None,
                    },
                    hangar_discovery::CursorRecordedDiffLine {
                        kind: "added".to_string(),
                        content: "const token = readToken();".to_string(),
                        old_line: None,
                        new_line: Some(7),
                    },
                    hangar_discovery::CursorRecordedDiffLine {
                        kind: "context".to_string(),
                        content: "use(token);".to_string(),
                        old_line: None,
                        new_line: None,
                    },
                ],
            }],
        );
        assert_eq!(result.source_kind, "Cursor");
        assert_eq!(result.coverage.level, "direct_edits");
        assert_eq!(result.redacted_count, 2);
        let lines = &result.files[0].edits[0].hunks[0].lines;
        assert_eq!((lines[0].old_line, lines[0].new_line), (Some(7), None));
        assert_eq!((lines[1].old_line, lines[1].new_line), (None, Some(7)));
        assert_eq!((lines[2].old_line, lines[2].new_line), (None, None));
        assert!(!lines[0].content.contains("sk-test"));
        assert!(!result.files[0].edits[0]
            .request
            .as_deref()
            .unwrap_or_default()
            .contains("sk-test"));
    }

    #[test]
    fn reconstructs_codex_turn_diff_with_line_numbers() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = temp.path().join("codex-session.jsonl");
        hangar_test_fixtures::write_codex_session_fixture(&fixture).unwrap();
        let result = parse_reader(
            BufReader::new(File::open(fixture).unwrap()),
            "codex-session.jsonl".to_string(),
        )
        .unwrap();
        assert_eq!(result.coverage.level, "full");
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "src/app.ts");
        assert_eq!(
            result.files[0].edits[0].request.as_deref(),
            Some("Fix the fixture label")
        );
        assert_eq!(result.files[0].edits[0].hunks[0].lines[0].old_line, Some(2));
        assert_eq!(result.added_lines, 1);
        assert_eq!(result.removed_lines, 1);
    }

    #[test]
    fn reconstructs_codex_apply_patch_as_partial_coverage() {
        let input = concat!(
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n-old\n+new\n*** End Patch"}}"#,
            "\n"
        );
        let result = parse(input);
        assert_eq!(result.coverage.level, "direct_edits");
        assert_eq!(result.files[0].path, "src/app.ts");
        assert_eq!(result.edit_count, 1);
    }

    #[test]
    fn reconstructs_claude_edit_write_and_multi_edit() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = temp.path().join("claude-session.jsonl");
        hangar_test_fixtures::write_claude_session_fixture(&fixture).unwrap();
        let result = parse_reader(
            BufReader::new(File::open(fixture).unwrap()),
            "claude-session.jsonl".to_string(),
        )
        .unwrap();
        assert_eq!(result.source_kind, "Claude");
        assert_eq!(result.coverage.level, "direct_edits");
        assert_eq!(result.files.len(), 3);
        assert_eq!(result.edit_count, 4);
        assert!(result
            .files
            .iter()
            .flat_map(|file| &file.edits)
            .all(|edit| edit.request.as_deref() == Some("Update the app and notebook")));
        let notebook = result
            .files
            .iter()
            .find(|file| file.path == "analysis.ipynb")
            .unwrap();
        assert_eq!(notebook.edits[0].source, "Claude NotebookEdit");
        assert_eq!(
            notebook.edits[0].summary,
            "Changed a recorded notebook cell"
        );
        assert!(notebook.edits[0].hunks[0]
            .lines
            .iter()
            .all(|line| line.old_line.is_none() && line.new_line.is_none()));
    }

    #[test]
    fn redacts_secrets_before_returning_changes() {
        let input = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":".env","content":"OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz123456"}}]}}"#,
            "\n"
        );
        let result = parse(input);
        let rendered = &result.files[0].edits[0].hunks[0].lines[0].content;
        assert!(!rendered.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(result.redacted_count > 0);
    }

    #[test]
    fn skips_malformed_and_oversized_records_without_losing_next_line() {
        let oversized = format!(
            "{{\"value\":\"{}\"}}",
            "x".repeat(MAX_JSONL_RECORD_BYTES + 20)
        );
        let input =
            format!("not-json\n{oversized}\n{{\"type\":\"session_meta\",\"payload\":{{}}}}\n");
        let result = parse(&input);
        assert_eq!(result.parsed_records, 1);
        assert_eq!(result.omitted_records, 2);
        assert_eq!(result.source_kind, "ChatGPT");
    }
}
