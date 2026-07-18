use hangar_core::{
    EditGitContext, EditValidationSummary, FileEditPreview, SessionDiffHunk, SessionDiffLine,
};
use std::path::Path;

const DIFF_CONTEXT_LINES: usize = 3;
const DIFF_OUTPUT_LINE_CAP: usize = 4_000;
const MAX_MYERS_DISTANCE: usize = 2_048;

pub(crate) struct DiffPreview {
    pub hunks: Vec<SessionDiffHunk>,
    pub added_lines: u64,
    pub removed_lines: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy)]
enum OpKind {
    Context,
    Added,
    Removed,
}

struct DiffOp {
    kind: OpKind,
    content: String,
    old_line: Option<u64>,
    new_line: Option<u64>,
}

pub(crate) fn preview_file_edit(
    state: &super::AppState,
    node_id: i64,
    content: &str,
    expected_content: Option<&str>,
) -> Result<FileEditPreview, String> {
    let (path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&path, &project_paths)?;
    let previous_bytes = std::fs::read(&path).map_err(|error| {
        format!("Change review unavailable: the file could not be read ({error}).")
    })?;
    let previous = String::from_utf8(previous_bytes)
        .map_err(|_| "Change review unavailable: this file is not UTF-8 text.".to_string())?;
    if let Some(expected) = expected_content {
        if blake3::hash(expected.as_bytes()) != blake3::hash(previous.as_bytes()) {
            return Err(
                "Change review stopped: the file changed on disk. Reload it before reviewing your draft."
                    .to_string(),
            );
        }
    }
    if previous == content {
        return Err("There is no change to review.".to_string());
    }
    super::edit_snapshot::validate_edit_content(content)?;
    let validation = validation_summary(Path::new(&path), &previous, content)?;
    let project_id = state
        .db()?
        .node_project_id(node_id)
        .map_err(super::to_message)?
        .ok_or_else(|| {
            "Change review unavailable: the file is no longer attached to a project.".to_string()
        })?;
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| {
            "Change review unavailable: the project is no longer registered.".to_string()
        })?;
    let relative_path = Path::new(&path)
        .strip_prefix(Path::new(&project.path))
        .map(|value| hangar_core::normalize_path(&value.to_string_lossy()))
        .map_err(|_| {
            "Change review unavailable: the file is outside its registered project.".to_string()
        })?;
    let git_context = git_context(state, project_id, &relative_path);
    let diff = build_diff(&previous, content);
    Ok(FileEditPreview {
        node_id,
        project_id,
        before_hash: blake3::hash(previous.as_bytes()).to_hex().to_string(),
        after_hash: blake3::hash(content.as_bytes()).to_hex().to_string(),
        added_lines: diff.added_lines,
        removed_lines: diff.removed_lines,
        hunks: diff.hunks,
        diff_truncated: diff.truncated,
        validation,
        git_context,
    })
}

pub(crate) fn preview_value_edit(
    state: &super::AppState,
    node_id: i64,
    request: &hangar_core::ValueEditRequest,
) -> Result<FileEditPreview, String> {
    let prepared = super::value_edit::prepare_value_edit(state, node_id, request)?;
    preview_file_edit(state, node_id, &prepared.content, Some(&prepared.source))
}

pub(crate) fn enforce_write_validation(
    path: &Path,
    previous: &str,
    proposed: &str,
    origin: &str,
) -> Result<(), String> {
    if origin == "restore" {
        return Ok(());
    }
    validation_summary(path, previous, proposed).map(|_| ())
}

fn validation_summary(
    path: &Path,
    previous: &str,
    proposed: &str,
) -> Result<EditValidationSummary, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let strict = matches!(extension.as_str(), "json" | "toml");
    let source = matches!(
        extension.as_str(),
        "js" | "jsx"
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
    if !strict && !source {
        return Ok(EditValidationSummary {
            status: "warning".to_string(),
            label: "No structure parser for this file".to_string(),
            note: "Code Hangar can still enforce text limits, whole-file change detection, a verified previous version and atomic replacement."
                .to_string(),
        });
    }
    let before = super::value_edit::validate_content_after_edit(&path.to_string_lossy(), previous);
    let after = super::value_edit::validate_content_after_edit(&path.to_string_lossy(), proposed);
    match after {
        Ok(()) => Ok(EditValidationSummary {
            status: "passed".to_string(),
            label: if strict {
                "File structure is valid".to_string()
            } else {
                "Basic structure check passed".to_string()
            },
            note: if strict {
                format!("The complete {extension} document parsed successfully.")
            } else {
                "Quotes, comments and bracket boundaries passed a lightweight local check. This is not a compiler or proof that the program works."
                    .to_string()
            },
        }),
        Err(error) if !strict && before.is_err() => Ok(EditValidationSummary {
            status: "warning".to_string(),
            label: "Existing structure issue remains".to_string(),
            note: format!(
                "The file already failed the lightweight structure check before this draft. Code Hangar will not claim the source is valid ({error})."
            ),
        }),
        Err(error) => Err(format!("Change review blocked: {error}")),
    }
}

fn git_context(state: &super::AppState, project_id: i64, relative_path: &str) -> EditGitContext {
    let change_set = match super::project_review::current_project_git_change_set(state, project_id) {
        Ok(change_set) => change_set,
        Err(error) => {
            return EditGitContext {
                state: "unavailable".to_string(),
                label: "Git context unavailable".to_string(),
                note: format!(
                    "The change can still use Code Hangar's verified previous version. Local Git inspection failed ({error})."
                ),
                other_changed_files: 0,
            }
        }
    };
    if change_set.coverage.level == "none" {
        return EditGitContext {
            state: "not_repository".to_string(),
            label: "This project is not using Git".to_string(),
            note: "Git history is unavailable, but Code Hangar will still create a verified previous version before applying the change."
                .to_string(),
            other_changed_files: 0,
        };
    }
    let current = change_set
        .files
        .iter()
        .find(|file| file.path.eq_ignore_ascii_case(relative_path));
    let other_changed_files = change_set
        .files
        .iter()
        .filter(|file| !file.path.eq_ignore_ascii_case(relative_path))
        .count() as u64;
    let Some(file) = current else {
        return EditGitContext {
            state: "clean".to_string(),
            label: "No pre-existing Git change in this file".to_string(),
            note: if other_changed_files == 0 {
                "Git reports no other local file changes in this project.".to_string()
            } else {
                format!(
                    "Git reports {other_changed_files} other locally changed file{} in this project; applying this draft will not touch them.",
                    if other_changed_files == 1 { "" } else { "s" }
                )
            },
            other_changed_files,
        };
    };
    let staged = file
        .edits
        .iter()
        .any(|edit| edit.source == "Git staged diff");
    let modified = file
        .edits
        .iter()
        .any(|edit| edit.source == "Git working tree diff");
    let untracked = file.edits.iter().any(|edit| edit.source == "Git status");
    let (git_state, label) = if untracked {
        ("untracked", "Git sees this as a new file")
    } else if staged && modified {
        (
            "staged_and_modified",
            "This file already has staged and unstaged changes",
        )
    } else if staged {
        ("staged", "This file already has staged changes")
    } else {
        ("modified", "This file already has local changes")
    };
    EditGitContext {
        state: git_state.to_string(),
        label: label.to_string(),
        note: "The review compares against the bytes currently on disk. Code Hangar will not stage, revert or overwrite unrelated Git changes."
            .to_string(),
        other_changed_files,
    }
}

pub(crate) fn build_diff(before: &str, after: &str) -> DiffPreview {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let raw = myers_ops(&before_lines, &after_lines)
        .unwrap_or_else(|| fallback_ops(&before_lines, &after_lines));
    hunks_from_ops(raw, before.ends_with('\n') != after.ends_with('\n'))
}

fn layer_value(layer: &[isize], distance: usize, diagonal: isize) -> isize {
    let index = diagonal + distance as isize;
    if index < 0 || index as usize >= layer.len() {
        -1
    } else {
        layer[index as usize]
    }
}

fn myers_ops(before: &[&str], after: &[&str]) -> Option<Vec<DiffOp>> {
    let max_distance = before
        .len()
        .saturating_add(after.len())
        .min(MAX_MYERS_DISTANCE);
    let mut layers: Vec<Vec<isize>> = Vec::with_capacity(max_distance + 1);
    for distance in 0..=max_distance {
        let mut layer = vec![-1; distance * 2 + 1];
        let mut diagonal = -(distance as isize);
        while diagonal <= distance as isize {
            let mut x = if distance == 0 {
                0
            } else {
                let previous = &layers[distance - 1];
                if diagonal == -(distance as isize)
                    || (diagonal != distance as isize
                        && layer_value(previous, distance - 1, diagonal - 1)
                            < layer_value(previous, distance - 1, diagonal + 1))
                {
                    layer_value(previous, distance - 1, diagonal + 1)
                } else {
                    layer_value(previous, distance - 1, diagonal - 1) + 1
                }
            };
            if x < 0 {
                diagonal += 2;
                continue;
            }
            let mut y = x - diagonal;
            while (x as usize) < before.len()
                && (y as usize) < after.len()
                && before[x as usize] == after[y as usize]
            {
                x += 1;
                y += 1;
            }
            layer[(diagonal + distance as isize) as usize] = x;
            if x as usize >= before.len() && y as usize >= after.len() {
                layers.push(layer);
                return Some(backtrack_ops(before, after, &layers));
            }
            diagonal += 2;
        }
        layers.push(layer);
    }
    None
}

fn backtrack_ops(before: &[&str], after: &[&str], layers: &[Vec<isize>]) -> Vec<DiffOp> {
    let mut x = before.len() as isize;
    let mut y = after.len() as isize;
    let mut reversed = Vec::new();
    for distance in (1..layers.len()).rev() {
        let diagonal = x - y;
        let previous = &layers[distance - 1];
        let previous_diagonal = if diagonal == -(distance as isize)
            || (diagonal != distance as isize
                && layer_value(previous, distance - 1, diagonal - 1)
                    < layer_value(previous, distance - 1, diagonal + 1))
        {
            diagonal + 1
        } else {
            diagonal - 1
        };
        let previous_x = layer_value(previous, distance - 1, previous_diagonal).max(0);
        let previous_y = previous_x - previous_diagonal;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            reversed.push((OpKind::Context, before[x as usize]));
        }
        if x == previous_x {
            y -= 1;
            reversed.push((OpKind::Added, after[y as usize]));
        } else {
            x -= 1;
            reversed.push((OpKind::Removed, before[x as usize]));
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        reversed.push((OpKind::Context, before[x as usize]));
    }
    while x > 0 {
        x -= 1;
        reversed.push((OpKind::Removed, before[x as usize]));
    }
    while y > 0 {
        y -= 1;
        reversed.push((OpKind::Added, after[y as usize]));
    }
    reversed.reverse();
    number_ops(reversed)
}

fn fallback_ops(before: &[&str], after: &[&str]) -> Vec<DiffOp> {
    let mut prefix = 0usize;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < before.len().saturating_sub(prefix)
        && suffix < after.len().saturating_sub(prefix)
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut raw = Vec::new();
    raw.extend(before[..prefix].iter().map(|line| (OpKind::Context, *line)));
    raw.extend(
        before[prefix..before.len().saturating_sub(suffix)]
            .iter()
            .map(|line| (OpKind::Removed, *line)),
    );
    raw.extend(
        after[prefix..after.len().saturating_sub(suffix)]
            .iter()
            .map(|line| (OpKind::Added, *line)),
    );
    raw.extend(
        before[before.len().saturating_sub(suffix)..]
            .iter()
            .map(|line| (OpKind::Context, *line)),
    );
    number_ops(raw)
}

fn number_ops(raw: Vec<(OpKind, &str)>) -> Vec<DiffOp> {
    let mut old_line = 1u64;
    let mut new_line = 1u64;
    raw.into_iter()
        .map(|(kind, content)| {
            let (old, new) = match kind {
                OpKind::Context => {
                    let result = (Some(old_line), Some(new_line));
                    old_line += 1;
                    new_line += 1;
                    result
                }
                OpKind::Removed => {
                    let result = (Some(old_line), None);
                    old_line += 1;
                    result
                }
                OpKind::Added => {
                    let result = (None, Some(new_line));
                    new_line += 1;
                    result
                }
            };
            DiffOp {
                kind,
                content: content.to_string(),
                old_line: old,
                new_line: new,
            }
        })
        .collect()
}

fn hunks_from_ops(ops: Vec<DiffOp>, newline_changed: bool) -> DiffPreview {
    let added_lines = ops
        .iter()
        .filter(|line| matches!(line.kind, OpKind::Added))
        .count() as u64;
    let removed_lines = ops
        .iter()
        .filter(|line| matches!(line.kind, OpKind::Removed))
        .count() as u64;
    let changed = ops
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (!matches!(line.kind, OpKind::Context)).then_some(index))
        .collect::<Vec<_>>();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let start = index.saturating_sub(DIFF_CONTEXT_LINES);
        let end = (index + DIFF_CONTEXT_LINES + 1).min(ops.len());
        if let Some(last) = ranges.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            ranges.push((start, end));
        }
    }
    let mut retained = 0usize;
    let mut truncated = false;
    let mut hunks = Vec::new();
    for (start, end) in ranges {
        if retained >= DIFF_OUTPUT_LINE_CAP {
            truncated = true;
            break;
        }
        let keep_end = end.min(start + DIFF_OUTPUT_LINE_CAP.saturating_sub(retained));
        truncated |= keep_end < end;
        let slice = &ops[start..keep_end];
        let old_start = slice.iter().find_map(|line| line.old_line).unwrap_or(1);
        let new_start = slice.iter().find_map(|line| line.new_line).unwrap_or(1);
        let old_count = slice.iter().filter(|line| line.old_line.is_some()).count();
        let new_count = slice.iter().filter(|line| line.new_line.is_some()).count();
        let lines = slice
            .iter()
            .map(|line| SessionDiffLine {
                kind: match line.kind {
                    OpKind::Context => "context",
                    OpKind::Added => "added",
                    OpKind::Removed => "removed",
                }
                .to_string(),
                content: line.content.clone(),
                old_line: line.old_line,
                new_line: line.new_line,
            })
            .collect();
        retained += slice.len();
        hunks.push(SessionDiffHunk {
            header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
            old_start: Some(old_start),
            new_start: Some(new_start),
            lines,
        });
        if truncated {
            break;
        }
    }
    if newline_changed || truncated {
        let note = if newline_changed && truncated {
            "The final-newline state changed. Additional diff lines were omitted by the local review limit."
        } else if newline_changed {
            "The final-newline state changed."
        } else {
            "Additional diff lines were omitted by the local review limit."
        };
        if let Some(last) = hunks.last_mut() {
            last.lines.push(SessionDiffLine {
                kind: "note".to_string(),
                content: note.to_string(),
                old_line: None,
                new_line: None,
            });
        } else {
            hunks.push(SessionDiffHunk {
                header: "File boundary change".to_string(),
                old_start: None,
                new_start: None,
                lines: vec![SessionDiffLine {
                    kind: "note".to_string(),
                    content: note.to_string(),
                    old_line: None,
                    new_line: None,
                }],
            });
        }
    }
    DiffPreview {
        hunks,
        added_lines: added_lines + u64::from(newline_changed),
        removed_lines: removed_lines + u64::from(newline_changed),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn myers_diff_keeps_separate_edits_and_context() {
        let diff = build_diff(
            "one\ntwo\nthree\nfour\nfive\n",
            "one\nTWO\nthree\nfour\nFIVE\n",
        );
        assert_eq!(diff.added_lines, 2);
        assert_eq!(diff.removed_lines, 2);
        let kinds = diff
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter().map(|line| line.kind.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(kinds.iter().filter(|kind| **kind == "added").count(), 2);
        assert_eq!(kinds.iter().filter(|kind| **kind == "removed").count(), 2);
    }

    #[test]
    fn final_newline_is_disclosed() {
        let diff = build_diff("same\n", "same");
        assert_eq!(diff.added_lines, 1);
        assert_eq!(diff.removed_lines, 1);
        assert!(diff.hunks[0].lines[0].content.contains("final-newline"));
    }

    #[test]
    fn myers_ops_reconstruct_both_sides_for_edge_cases() {
        let cases = [
            (vec![], vec!["one", "two"]),
            (vec!["one", "two"], vec![]),
            (vec!["two", "three"], vec!["one", "two", "three"]),
            (vec!["one", "two"], vec!["one", "two", "three"]),
            (vec!["one", "two", "three"], vec!["one", "three"]),
            (vec!["a", "b", "c", "d"], vec!["a", "B", "c", "D"]),
        ];
        for (before, after) in cases {
            let ops = myers_ops(&before, &after).expect("small changes should use Myers");
            let reconstructed_before = ops
                .iter()
                .filter(|op| !matches!(op.kind, OpKind::Added))
                .map(|op| op.content.as_str())
                .collect::<Vec<_>>();
            let reconstructed_after = ops
                .iter()
                .filter(|op| !matches!(op.kind, OpKind::Removed))
                .map(|op| op.content.as_str())
                .collect::<Vec<_>>();
            assert_eq!(reconstructed_before, before);
            assert_eq!(reconstructed_after, after);
        }
    }

    #[test]
    fn invalid_new_structure_is_blocked_but_existing_source_damage_is_disclosed() {
        let path = Path::new("screen.ts");
        assert!(
            validation_summary(path, "const ok = true;\n", "const bad = \"oops;\n")
                .unwrap_err()
                .contains("blocked")
        );
        let existing =
            validation_summary(path, "const bad = \"old;\n", "const bad = \"new;\n").unwrap();
        assert_eq!(existing.status, "warning");
    }
}
