//! "Explain this code" send-gate + orchestration (connector edition only).
//!
//! The stateful API resolves a webview-supplied node id through the inventory before this
//! module receives a path. This module then (a) refuses sensitive paths and files that
//! contain secrets BEFORE anything leaves the machine, (b) assembles the prompt, and (c)
//! hands the gated text to `hangar-ai` for the single outbound call. The API key lives only
//! in the OS keychain (resolved inside `hangar-ai`) and never crosses to JS. Compiled only
//! under `agent_automation`; absent from the strict core lane and Local edition.

use std::fs;
use std::io::Read;
use std::path::Path;

use hangar_core::{AiSendDisclosure, AiWalkthroughPreview, AiWalkthroughSection, SessionChangeSet};
use serde::Serialize;
use std::collections::HashSet;

const READ_CAP: u64 = 60 * 1024;
/// Extra bytes scanned beyond the send cap so a secret straddling the READ_CAP boundary is still
/// caught (only the first READ_CAP bytes are ever sent to a provider).
const SCAN_OVERLAP: u64 = 4 * 1024;
pub(crate) const MAX_TOKENS: u32 = 1200;
const FOLLOW_UP_MAX_TOKENS: u32 = 900;
const WALKTHROUGH_FILE_CAP: u64 = 2 * 1024 * 1024;
const WALKTHROUGH_SECTION_MAX_CHARS: usize = 12 * 1024;
const WALKTHROUGH_TARGET_SECTIONS: usize = 256;
const FOLLOW_UP_QUESTION_MAX_CHARS: usize = 600;
// A selected passage can still be substantial, so allow a bounded same-size response. Whole-file
// rewrite is intentionally not part of the retrospective product.
const REWRITE_MIN_TOKENS: u32 = 256;
const REWRITE_MAX_TOKENS: u32 = 4096;

#[derive(Debug, Clone)]
pub(crate) struct AiSelectionRewrite {
    pub source: String,
    pub replacement: String,
    pub language: String,
}

/// Read-only preview of an "explain" send: what (if anything) blocks it, and the size/cost.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiExplainPreview {
    /// Non-empty when the file must NOT be sent (sensitive path or secret content). Each
    /// entry is a human reason. When empty, the send is allowed.
    pub blocked: Vec<String>,
    /// Characters that would be sent.
    pub send_chars: usize,
    /// Rough token estimate (chars / 4) for a cost hint — no network call.
    pub est_tokens: u64,
    /// Detected language label from the extension.
    pub language: String,
}

/// A sensitive file by name/path that must never be sent, regardless of content.
fn sensitive_path_reason(path: &Path) -> Option<String> {
    let lower = path.to_string_lossy().to_lowercase();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let sensitive_names = [
        ".env",
        "id_rsa",
        "id_ed25519",
        "id_dsa",
        ".npmrc",
        ".pypirc",
        "credentials",
        ".netrc",
        ".pgpass",
    ];
    if sensitive_names
        .iter()
        .any(|s| name == *s || name.starts_with(&format!("{s}.")))
    {
        return Some(format!("'{name}' is a sensitive file and is never sent."));
    }
    let sensitive_exts = [
        ".pem",
        ".key",
        ".pfx",
        ".p12",
        ".keystore",
        ".der",
        ".asc",
        ".ppk",
    ];
    if sensitive_exts.iter().any(|e| name.ends_with(e)) {
        return Some(format!(
            "'{name}' looks like a key/credential file and is never sent."
        ));
    }
    let sensitive_segments = [
        "\\.ssh\\",
        "/.ssh/",
        "\\.gnupg\\",
        "/.gnupg/",
        "\\.aws\\",
        "/.aws/",
    ];
    if sensitive_segments.iter().any(|seg| lower.contains(seg)) {
        return Some(
            "This file is inside a credentials directory (.ssh/.aws/.gnupg) and is never sent."
                .to_string(),
        );
    }
    // Defer to the app-wide sensitivity classification — the SAME built-in rules that gate
    // every on-machine read/preview/FTS path (filenames containing token/secret/credential,
    // `.git/`, etc.). This guarantees the off-machine send boundary is never weaker than the
    // on-machine one, closing gaps the bespoke list above misses (e.g. `api-tokens.md`,
    // `.git/config`). The stateful API enforces the inventory's built-in-derived
    // `protected_level`/`is_sensitive` flags before calling this path/content gate; the
    // user-editable `protected_zone` globs are not yet reflected in those inventory flags
    // (they need a DB connection this helper does not carry), so this gate is their backstop.
    let path_str = path.to_string_lossy();
    if hangar_protect::is_sensitive_path(&path_str)
        || hangar_protect::protected_level_for_path(&path_str).is_some()
    {
        return Some(format!(
            "'{name}' is classified sensitive or Protected by Code Hangar and is never sent."
        ));
    }
    None
}

/// True for a run of `len` chars of secret-ish alphabet starting at `start`.
fn token_run_len(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
            i += 1;
        } else {
            break;
        }
    }
    i - start
}

fn contains_prefixed_token(content: &str, prefix: &str, min_run: usize) -> bool {
    let bytes = content.as_bytes();
    let mut from = 0;
    while let Some(rel) = content[from..].find(prefix) {
        let at = from + rel;
        let run = token_run_len(bytes, at + prefix.len());
        if run >= min_run {
            return true;
        }
        from = at + prefix.len();
    }
    false
}

/// A gitleaks-style scan for high-signal secrets. Hard-block on any hit. Conservative by
/// design: better to refuse a clean file than to leak a credential the user didn't notice.
fn secret_reasons(content: &str) -> Vec<String> {
    let mut reasons = Vec::new();
    let add = |label: &str, reasons: &mut Vec<String>| {
        let msg = format!("Looks like it contains a {label}.");
        if !reasons.contains(&msg) {
            reasons.push(msg);
        }
    };
    if content.contains("-----BEGIN") && content.contains("PRIVATE KEY") {
        add("private key block", &mut reasons);
    }
    if contains_prefixed_token(content, "sk-ant-", 20) {
        add("Anthropic API key", &mut reasons);
    }
    if contains_prefixed_token(content, "ghp_", 30)
        || contains_prefixed_token(content, "github_pat_", 30)
    {
        add("GitHub token", &mut reasons);
    }
    if contains_prefixed_token(content, "sk_live_", 16)
        || contains_prefixed_token(content, "rk_live_", 16)
    {
        add("Stripe secret key", &mut reasons);
    }
    if contains_prefixed_token(content, "xoxb-", 10)
        || contains_prefixed_token(content, "xoxp-", 10)
    {
        add("Slack token", &mut reasons);
    }
    if contains_prefixed_token(content, "AKIA", 16) {
        add("AWS access key", &mut reasons);
    }
    if contains_prefixed_token(content, "AIza", 30) {
        add("Google API key", &mut reasons);
    }
    // OpenAI-style: sk- followed by a long run, but not the Anthropic sk-ant- (handled).
    if !content.contains("sk-ant-") && contains_prefixed_token(content, "sk-", 40) {
        add("API key", &mut reasons);
    }
    // Generic: an assignment of a long token to a secret-named field.
    for raw in content.lines() {
        let line = raw.trim();
        let lower = line.to_lowercase();
        let looks_secret = [
            "secret",
            "password",
            "passwd",
            "api_key",
            "apikey",
            "api-key",
            "access_token",
            "private_key",
        ]
        .iter()
        .any(|k| lower.contains(k));
        if !looks_secret {
            continue;
        }
        // Check EVERY token on the line, not just the value after the first '='/':' — a
        // single-line JSON like `{"x":"y","api_key":"sk-..."}` hides the secret behind several
        // separators. Split on the usual separators/quotes/brackets and flag any long alnum run.
        let has_long_token = line
            .split(|c: char| {
                matches!(
                    c,
                    '=' | ':' | ',' | ';' | '"' | '\'' | '`' | '{' | '}' | '(' | ')' | '[' | ']'
                ) || c.is_whitespace()
            })
            .any(|token| {
                let token = token.trim_matches(['"', '\'', '`'].as_ref());
                let alnum = token.chars().filter(|c| c.is_ascii_alphanumeric()).count();
                token.len() >= 16 && alnum >= 12
            });
        if has_long_token {
            add("hard-coded secret value", &mut reasons);
            break;
        }
    }
    reasons
}

fn language_of(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "rb" => "Ruby",
        "php" => "PHP",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "cs" => "C#",
        "swift" => "Swift",
        "kt" | "kts" => "Kotlin",
        "sh" | "bash" => "Shell",
        "sql" => "SQL",
        "html" => "HTML",
        "css" => "CSS",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" => "Markdown",
        _ => "code",
    }
    .to_string()
}

/// Decode a (capped) byte buffer to text. Honors a UTF-16 BOM (LE/BE) by decoding to proper
/// UTF-8, so the scanned text is the SAME logical text the model reads. Otherwise treats it as
/// UTF-8 (lossy): a UTF-16 file WITHOUT a BOM, or a binary file, keeps its interleaved NUL bytes
/// here — the caller's NUL guard then refuses it. This closes the evasion where a UTF-16 secret
/// (s\0k\0-\0a\0n\0t…) hid from the ASCII substring scanner while still being transmitted.
fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Read a file ONCE and decode it. Returns `(bytes_to_send, bytes_to_scan)`: the send window is
/// the first `READ_CAP` bytes; the scan window reads a little further so a secret token straddling
/// the cap is still detected (only the send window ever leaves). Decoding is BOM-aware so the
/// scanned text equals the sent text — no second read and no lossy-encoding gap can diverge them.
fn read_capped_for_send(path: &Path) -> Result<(String, String), String> {
    let file = fs::File::open(path).map_err(|err| format!("Could not open the file: {err}"))?;
    let mut buffer = Vec::new();
    file.take(READ_CAP + SCAN_OVERLAP)
        .read_to_end(&mut buffer)
        .map_err(|err| format!("Could not read the file: {err}"))?;
    let send_len = buffer.len().min(READ_CAP as usize);
    let send = decode_text(&buffer[..send_len]);
    let scan = decode_text(&buffer);
    Ok((send, scan))
}

/// Read + decode a file once and run the full CONTENT gate. Returns `(block_reasons, text_to_send)`
/// — empty reasons mean the send is allowed. Both the preview (cost) and the real send call this,
/// so they can never disagree. A NUL byte in the decoded scan window means a binary or
/// non-UTF-8/UTF-16 file (a UTF-16 file without a BOM keeps its NULs): refuse it — the model
/// cannot explain binary, and a secret could otherwise hide between the NULs.
fn gate_file_content(path: &Path) -> Result<(Vec<String>, String), String> {
    let (send, scan) = read_capped_for_send(path)?;
    if scan.contains('\0') {
        return Ok((
            vec!["This file is binary or not UTF-8/UTF-16 text, so it is never sent.".to_string()],
            send,
        ));
    }
    Ok((secret_reasons(&scan), send))
}

/// Walkthroughs map the complete bounded file locally, then send only selected
/// sections. The complete text is scanned before any section can leave, so a
/// secret beyond the first provider batch cannot be bypassed by selecting an
/// earlier clean section.
fn gate_walkthrough_content(path: &Path) -> Result<(Vec<String>, String), String> {
    let file = fs::File::open(path).map_err(|err| format!("Could not open the file: {err}"))?;
    let mut buffer = Vec::new();
    file.take(WALKTHROUGH_FILE_CAP + 1)
        .read_to_end(&mut buffer)
        .map_err(|err| format!("Could not read the file: {err}"))?;
    if buffer.len() > WALKTHROUGH_FILE_CAP as usize {
        return Ok((
            vec![format!(
                "This file is larger than {} MiB, so Code Hangar will not build an incomplete walkthrough.",
                WALKTHROUGH_FILE_CAP / 1024 / 1024
            )],
            String::new(),
        ));
    }
    let content = decode_text(&buffer);
    if content.contains('\0') {
        return Ok((
            vec!["This file is binary or not UTF-8/UTF-16 text, so it is never sent.".to_string()],
            content,
        ));
    }
    Ok((secret_reasons(&content), content))
}

#[derive(Debug, Clone)]
struct WalkthroughSectionContent {
    summary: AiWalkthroughSection,
    content: String,
}

#[derive(Debug, Clone)]
struct WalkthroughMaterial {
    blocked: Vec<String>,
    language: String,
    sections: Vec<WalkthroughSectionContent>,
    source_chars: u64,
    truncated: bool,
}

/// Rust parity for the frontend's `hashSnippet`: djb2 over JavaScript UTF-16
/// code units. Keeping one algorithm on both sides makes an anchor stable for
/// non-ASCII selections too; the backend still derives the trusted hash.
pub(crate) fn hash_snippet(text: &str) -> String {
    let mut hash = 5_381_u32;
    for unit in text.encode_utf16() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(unit));
    }
    format!("{hash:x}")
}

fn compact_heading(line: &str) -> String {
    let cleaned = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches(['{', ':'])
        .trim();
    let title: String = cleaned.chars().take(72).collect();
    if title.is_empty() {
        "File section".to_string()
    } else {
        title
    }
}

fn section_heading(language: &str, line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
        return None;
    }
    if language == "Markdown" && trimmed.starts_with('#') {
        return Some(compact_heading(trimmed));
    }
    if matches!(language, "TOML" | "YAML")
        && ((trimmed.starts_with('[') && trimmed.ends_with(']'))
            || (!line.starts_with(char::is_whitespace) && trimmed.ends_with(':')))
    {
        return Some(compact_heading(trimmed));
    }
    if language == "JSON"
        && !line.starts_with(char::is_whitespace)
        && trimmed.starts_with('"')
        && trimmed.contains("\":")
    {
        return Some(compact_heading(trimmed.trim_matches([',', '"'])));
    }

    let lower = trimmed.to_ascii_lowercase();
    let is_heading = match language {
        "Rust" => [
            "fn ",
            "pub fn ",
            "pub(crate) fn ",
            "struct ",
            "pub struct ",
            "enum ",
            "pub enum ",
            "impl ",
            "trait ",
            "pub trait ",
            "mod ",
            "pub mod ",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix)),
        "TypeScript" | "JavaScript" => [
            "function ",
            "async function ",
            "export function ",
            "export async function ",
            "class ",
            "export class ",
            "interface ",
            "export interface ",
            "type ",
            "export type ",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix)),
        "Python" => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("class ")
        }
        "Go" => trimmed.starts_with("func ") || trimmed.starts_with("type "),
        "Java" | "C#" | "C++" | "C" | "Kotlin" | "Swift" | "PHP" => {
            [" class ", " interface ", " enum ", " struct "]
                .iter()
                .any(|needle| format!(" {lower} ").contains(needle))
                || (trimmed.ends_with('{') && trimmed.contains('(') && trimmed.contains(')'))
        }
        "HTML" => [
            "<head", "<body", "<main", "<section", "<article", "<nav", "<footer",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix)),
        "CSS" => trimmed.ends_with('{') && !trimmed.starts_with('@'),
        "SQL" => [
            "create ", "select ", "insert ", "update ", "delete ", "with ",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix)),
        _ => false,
    };
    is_heading.then(|| compact_heading(trimmed))
}

fn sectionize_content(language: &str, content: &str) -> Vec<WalkthroughSectionContent> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let total_chars = content.chars().count();
    let target_chars = total_chars
        .div_ceil(WALKTHROUGH_TARGET_SECTIONS)
        .clamp(1, WALKTHROUGH_SECTION_MAX_CHARS);
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0_usize;
    let mut start_line = 1_u64;
    let mut end_line = 1_u64;
    let mut title = "Start of file".to_string();
    let mut part = 1_usize;

    for (line_index, line) in lines.iter().enumerate() {
        let line_number = line_index as u64 + 1;
        if let Some(heading) = section_heading(language, line) {
            if !current.is_empty() && current_chars >= target_chars {
                push_walkthrough_section(
                    &mut sections,
                    std::mem::take(&mut current),
                    start_line,
                    end_line,
                    &title,
                    part,
                );
                current_chars = 0;
                start_line = line_number;
                title = heading;
                part = 1;
            } else if current.is_empty() {
                start_line = line_number;
                title = heading;
                part = 1;
            }
        }

        let mut remainder = *line;
        while !remainder.is_empty() {
            if current.is_empty() {
                start_line = line_number;
            }
            let available = WALKTHROUGH_SECTION_MAX_CHARS.saturating_sub(current_chars);
            if available == 0 {
                push_walkthrough_section(
                    &mut sections,
                    std::mem::take(&mut current),
                    start_line,
                    end_line,
                    &title,
                    part,
                );
                current_chars = 0;
                start_line = line_number;
                part += 1;
                continue;
            }
            let split = remainder
                .char_indices()
                .nth(available)
                .map(|(index, _)| index)
                .unwrap_or(remainder.len());
            let (piece, rest) = remainder.split_at(split);
            current.push_str(piece);
            current_chars += piece.chars().count();
            end_line = line_number;
            remainder = rest;
            if !remainder.is_empty() {
                push_walkthrough_section(
                    &mut sections,
                    std::mem::take(&mut current),
                    start_line,
                    end_line,
                    &title,
                    part,
                );
                current_chars = 0;
                start_line = line_number;
                part += 1;
            }
        }
    }
    if !current.is_empty() {
        push_walkthrough_section(&mut sections, current, start_line, end_line, &title, part);
    }
    sections
}

fn push_walkthrough_section(
    sections: &mut Vec<WalkthroughSectionContent>,
    content: String,
    line_start: u64,
    line_end: u64,
    title: &str,
    part: usize,
) {
    if content.is_empty() {
        return;
    }
    let snippet_hash = hash_snippet(&content);
    let display_title = if part == 1 {
        title.to_string()
    } else {
        format!("{title} — part {part}")
    };
    let id_hash =
        blake3::hash(format!("{line_start}:{line_end}:{snippet_hash}").as_bytes()).to_hex();
    let id = format!("s{}-{}", line_start, &id_hash[..10]);
    let send_chars = content.chars().count() as u64;
    let context_bytes = format!(
        "\n[section:{id}] {display_title} (lines {line_start}-{line_end})\n```\n{content}\n```\n"
    )
    .len() as u64;
    sections.push(WalkthroughSectionContent {
        summary: AiWalkthroughSection {
            id,
            title: display_title,
            start_line: line_start,
            end_line: line_end,
            snippet_hash,
            send_chars,
            context_bytes,
            est_tokens: send_chars.div_ceil(4),
        },
        content,
    });
}

fn default_walkthrough_selection(sections: &[WalkthroughSectionContent]) -> Vec<String> {
    let mut used = "Grounded file sections\n".len() as u64;
    sections
        .iter()
        .take_while(|section| {
            let next = used.saturating_add(section.summary.context_bytes);
            if next > READ_CAP {
                return false;
            }
            used = next;
            true
        })
        .map(|section| section.summary.id.clone())
        .collect()
}

fn compose_walkthrough_context(
    sections: &[WalkthroughSectionContent],
    selected_ids: &[String],
) -> Result<String, String> {
    let wanted: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
    if !wanted.is_empty()
        && wanted
            .iter()
            .any(|id| !sections.iter().any(|section| section.summary.id == *id))
    {
        return Err(
            "One or more walkthrough sections are stale. Reload the file sections.".to_string(),
        );
    }
    let selected: Vec<&WalkthroughSectionContent> = sections
        .iter()
        .filter(|section| wanted.is_empty() || wanted.contains(section.summary.id.as_str()))
        .collect();
    if selected.is_empty() {
        return Err("Select at least one file section.".to_string());
    }
    let mut output = String::from("Grounded file sections\n");
    for section in selected {
        output.push_str(&format!(
            "\n[section:{}] {} (lines {}-{})\n```\n{}\n```\n",
            section.summary.id,
            section.summary.title,
            section.summary.start_line,
            section.summary.end_line,
            section.content
        ));
        if output.len() > READ_CAP as usize {
            return Err(
                "Those sections exceed the 60 KiB walkthrough limit. Select fewer sections."
                    .to_string(),
            );
        }
    }
    Ok(output)
}

fn walkthrough_material(path: &Path) -> Result<WalkthroughMaterial, String> {
    let language = language_of(path);
    if let Some(reason) = sensitive_path_reason(path) {
        return Ok(WalkthroughMaterial {
            blocked: vec![reason],
            language,
            sections: Vec::new(),
            source_chars: 0,
            truncated: false,
        });
    }
    if !path.is_file() {
        return Ok(WalkthroughMaterial {
            blocked: vec!["That file no longer exists.".to_string()],
            language,
            sections: Vec::new(),
            source_chars: 0,
            truncated: false,
        });
    }
    let file_len = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let (blocked, content) = gate_walkthrough_content(path)?;
    if !blocked.is_empty() {
        return Ok(WalkthroughMaterial {
            blocked,
            language,
            sections: Vec::new(),
            source_chars: 0,
            truncated: file_len > WALKTHROUGH_FILE_CAP,
        });
    }
    let source_chars = content.chars().count() as u64;
    if content.trim().is_empty() {
        return Ok(WalkthroughMaterial {
            blocked: vec!["This file is empty, so there is nothing to walk through.".to_string()],
            language,
            sections: Vec::new(),
            source_chars,
            truncated: false,
        });
    }
    let sections = sectionize_content(&language, &content);
    Ok(WalkthroughMaterial {
        blocked: Vec::new(),
        language,
        source_chars,
        truncated: false,
        sections,
    })
}

pub(crate) fn ai_walkthrough_preview_for_path(path: &str) -> Result<AiWalkthroughPreview, String> {
    let material = walkthrough_material(Path::new(path))?;
    if !material.blocked.is_empty() {
        return Ok(AiWalkthroughPreview {
            blocked: material.blocked,
            language: material.language,
            sections: Vec::new(),
            default_section_ids: Vec::new(),
            send_chars: 0,
            est_tokens: 0,
            source_chars: material.source_chars,
            max_batch_bytes: READ_CAP,
            truncated: material.truncated,
        });
    }
    let default_section_ids = default_walkthrough_selection(&material.sections);
    let context = compose_walkthrough_context(&material.sections, &default_section_ids)?;
    let send_chars = context.chars().count() as u64;
    Ok(AiWalkthroughPreview {
        blocked: Vec::new(),
        language: material.language,
        sections: material
            .sections
            .into_iter()
            .map(|section| section.summary)
            .collect(),
        default_section_ids,
        send_chars,
        est_tokens: send_chars.div_ceil(4),
        source_chars: material.source_chars,
        max_batch_bytes: READ_CAP,
        truncated: material.truncated,
    })
}

fn walkthrough_system_prompt(level: &str) -> String {
    let base = "You are a read-only code-reading tutor. Explain only the supplied file sections, in their given order. For each section, copy its exact `[section:id]` marker onto its own line, then give: `purpose`, `how to read it`, and `terms` as short bullet lists. Ground every statement in visible code. Define jargon briefly. Do not propose edits, output replacement code or patches, execute anything, or invent missing project behaviour.";
    if level == "engineer" {
        format!("{base} Be concise and technically precise.")
    } else {
        format!("{base} The reader is a beginner who used AI to build the project; use plain, concrete language.")
    }
}

pub(crate) fn ai_walkthrough_file_for_path(
    path: &str,
    section_ids: &[String],
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let material = walkthrough_material(Path::new(path))?;
    if !material.blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            material.blocked.join(" ")
        ));
    }
    let context = compose_walkthrough_context(&material.sections, section_ids)?;
    let secrets = secret_reasons(&context);
    if !secrets.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            secrets.join(" ")
        ));
    }
    hangar_ai::explain(
        config,
        &walkthrough_system_prompt(level),
        &context,
        MAX_TOKENS,
    )
}

fn follow_up_context(
    path: &Path,
    section_id: &str,
    history: &[(String, String)],
    question: &str,
) -> Result<(String, String), String> {
    let question = question.trim();
    if question.is_empty() || question.chars().count() > FOLLOW_UP_QUESTION_MAX_CHARS {
        return Err("Ask one question of at most 600 characters.".to_string());
    }
    if history.len() >= 3 {
        return Err("This follow-up reached its three-turn limit.".to_string());
    }
    let material = walkthrough_material(path)?;
    if !material.blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            material.blocked.join(" ")
        ));
    }
    let selected = material
        .sections
        .iter()
        .find(|section| section.summary.id == section_id)
        .ok_or_else(|| "That walkthrough section is stale. Reload the sections.".to_string())?;
    let mut context = format!(
        "[section:{}] {} (lines {}-{})\n```\n{}\n```\n",
        selected.summary.id,
        selected.summary.title,
        selected.summary.start_line,
        selected.summary.end_line,
        selected.content
    );
    for (index, (prior_question, prior_answer)) in history.iter().enumerate() {
        context.push_str(&format!(
            "\nPrior turn {}\nQuestion: {}\nAnswer: {}\n",
            index + 1,
            prior_question,
            prior_answer
        ));
    }
    context.push_str(&format!("\nCurrent question: {question}"));
    if context.len() > READ_CAP as usize {
        return Err("This follow-up context exceeds the 60 KiB send limit.".to_string());
    }
    let secrets = secret_reasons(&context);
    if !secrets.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            secrets.join(" ")
        ));
    }
    Ok((context, material.language))
}

pub(crate) fn ai_follow_up_preview_for_path(
    path: &str,
    section_id: &str,
    history: &[(String, String)],
    question: &str,
) -> Result<AiExplainPreview, String> {
    let (context, language) = follow_up_context(Path::new(path), section_id, history, question)?;
    let send_chars = context.chars().count();
    Ok(AiExplainPreview {
        blocked: Vec::new(),
        send_chars,
        est_tokens: (send_chars as u64).div_ceil(4),
        language,
    })
}

fn follow_up_system_prompt(level: &str) -> String {
    let base = "You are answering a bounded follow-up about one supplied file section. Answer only the current question, using the section and prior turns as evidence. If the answer is not visible, say what is unknown. Do not propose or output edits, replacement code, commands, or patches, and do not claim to have executed anything.";
    if level == "engineer" {
        format!("{base} Use precise software terminology.")
    } else {
        format!("{base} Use short, plain language and define unavoidable jargon.")
    }
}

pub(crate) fn ai_follow_up_for_path(
    path: &str,
    section_id: &str,
    history: &[(String, String)],
    question: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let (context, _) = follow_up_context(Path::new(path), section_id, history, question)?;
    hangar_ai::explain(
        config,
        &follow_up_system_prompt(level),
        &context,
        FOLLOW_UP_MAX_TOKENS,
    )
}

/// Full, non-truncated bytes for anchored notes. A note is local-only, but the
/// same protected/sensitive/secret policy is used so a transient reveal cannot
/// become a durable code copy in the catalog.
pub(crate) fn annotation_source_for_path(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if let Some(reason) = sensitive_path_reason(path) {
        return Err(reason);
    }
    let metadata = path
        .metadata()
        .map_err(|error| format!("Could not inspect the file: {error}"))?;
    if metadata.len() > READ_CAP {
        return Err("Anchored notes are disabled on truncated previews.".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("Could not read the file: {error}"))?;
    let content = decode_text(&bytes);
    if content.contains('\0') {
        return Err("Anchored notes are available only for text files.".to_string());
    }
    let blocked = secret_reasons(&content);
    if !blocked.is_empty() {
        return Err("Anchored notes are disabled on revealed or sensitive previews.".to_string());
    }
    Ok(content)
}

pub(crate) fn unique_snippet_line_range(
    content: &str,
    snippet: &str,
) -> Result<(u64, u64), String> {
    if snippet.trim().is_empty() || snippet.len() > 16 * 1024 || snippet.contains('\0') {
        return Err("Select between 1 byte and 16 KiB of text for an anchored note.".to_string());
    }
    let matches: Vec<usize> = content
        .match_indices(snippet)
        .map(|(index, _)| index)
        .collect();
    if matches.is_empty() {
        return Err("The selected text is no longer present in the file.".to_string());
    }
    if matches.len() > 1 {
        return Err(
            "That exact selection appears more than once. Select a slightly larger unique block."
                .to_string(),
        );
    }
    let start = matches[0];
    let line_start = content[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u64
        + 1;
    let line_end = line_start + snippet.bytes().filter(|byte| *byte == b'\n').count() as u64;
    Ok((line_start, line_end))
}

/// Run the full send-gate for `path`. Never reads a sensitive file's bytes beyond the cap
/// needed to scan, and reports every block reason so the UI can show exactly why.
pub(crate) fn ai_explain_preview_for_path(path: &str) -> Result<AiExplainPreview, String> {
    let p = Path::new(path);
    let language = language_of(p);
    if let Some(reason) = sensitive_path_reason(p) {
        return Ok(AiExplainPreview {
            blocked: vec![reason],
            send_chars: 0,
            est_tokens: 0,
            language,
        });
    }
    if !p.is_file() {
        return Ok(AiExplainPreview {
            blocked: vec!["That file no longer exists.".to_string()],
            send_chars: 0,
            est_tokens: 0,
            language,
        });
    }
    // Use the SAME read+decode+scan path as the real send, so the preview's verdict and the
    // send's verdict can never diverge (same window, same encoding handling, same binary guard).
    let (blocked, send) = gate_file_content(p)?;
    let send_chars = if blocked.is_empty() {
        send.chars().count()
    } else {
        0
    };
    Ok(AiExplainPreview {
        blocked,
        send_chars,
        est_tokens: (send_chars as u64) / 4,
        language,
    })
}

/// The stable persona prompt. It is kept byte-identical per level and sent FIRST so providers'
/// automatic server-side prefix caching (OpenAI and Anthropic both do this) cuts the cost of
/// repeat explains/summaries — provider-agnostic and graceful, with no vendor-specific
/// `cache_control` that could break other Messages-API-compatible servers.
fn system_prompt(level: &str) -> String {
    let base = "You are explaining code to someone who built this project using AI tools and may not know the programming language. Explain in plain, neutral language: what this file does overall, the key pieces and how they fit together, and anything that looks risky, unfinished, or worth double-checking. Be matter-of-fact: no praise, no enthusiasm, no filler, no exclamation marks — do not editorialize about whether the code is good or exciting, just describe what it does. Avoid jargon; when a technical term is unavoidable, define it in one short clause. Never invent behaviour you cannot see in the code.";
    match level {
        "engineer" => format!(
            "{base} The reader is comfortable with software concepts, so be precise and concise."
        ),
        _ => format!("{base} The reader is a beginner; keep it simple and concrete."),
    }
}

fn review_system_prompt(level: &str) -> String {
    let base = "You are a code-review coach for someone who built this project with AI tools. Help the reader inspect the code instead of asking them to trust a verdict. Return short sections using ONLY this concern vocabulary, in this order: `be-careful`, `double-check`, `heads-up`, `what-looks-deliberate`, and `unknowns`. Omit an empty section. Under the first three sections, write concrete questions, cite the visible function, value, branch, or line pattern that prompted each question, and explain in one sentence what could happen if it is wrong. The labels mean: be-careful = credible data-loss/security/correctness harm; double-check = behaviour or failure handling worth verifying; heads-up = lower-impact maintainability or test uncertainty. Cover correctness, failure states, unsafe inputs, data loss, security boundaries, and missing tests only when the code provides evidence. Do not rewrite code, output a patch, claim you ran anything, use rule IDs, pronounce a verdict, or invent project behaviour that is not visible. Format every section as `[label]` on its own line followed by `- ` bullets. Example: `[double-check]` then `- Does parseConfig handle an empty file? Evidence: it calls JSON.parse without a visible fallback. If not, startup could stop on a blank config.`";
    match level {
        "engineer" => format!(
            "{base} Use precise software terminology and distinguish confirmed defects from review questions."
        ),
        _ => format!(
            "{base} Use plain language. Define any unavoidable technical term in a short clause, and phrase every risk as a question the reader can investigate."
        ),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AiReadLens {
    Explain,
    Review,
}

struct GatedPrompt {
    system: String,
    user: String,
}

fn gated_file_prompt(path: &str, level: &str, lens: AiReadLens) -> Result<GatedPrompt, String> {
    let p = Path::new(path);
    let language = language_of(p);
    if let Some(reason) = sensitive_path_reason(p) {
        return Err(format!("Not sent — {reason} Nothing left your machine."));
    }
    if !p.is_file() {
        return Err(
            "Not sent — that file no longer exists. Nothing left your machine.".to_string(),
        );
    }
    let (blocked, content) = gate_file_content(p)?;
    if !blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            blocked.join(" ")
        ));
    }
    let name = p
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    Ok(match lens {
        AiReadLens::Explain => GatedPrompt {
            system: system_prompt(level),
            user: format!("Explain this {language} file named `{name}`:\n\n```\n{content}\n```"),
        },
        AiReadLens::Review => GatedPrompt {
            system: review_system_prompt(level),
            user: format!(
                "Give me a read-only review lens for this {language} file named `{name}`. Ask what I should verify and ground every question in the visible code:\n\n```\n{content}\n```"
            ),
        },
    })
}

fn gated_text_prompt(
    snippet: &str,
    origin_path: &str,
    level: &str,
    lens: AiReadLens,
) -> Result<GatedPrompt, String> {
    let p = Path::new(origin_path);
    if let Some(reason) = sensitive_path_reason(p) {
        return Err(format!("Not sent — {reason} Nothing left your machine."));
    }
    if snippet.trim().is_empty() {
        return Err("Not sent — no text was selected. Nothing left your machine.".to_string());
    }
    if snippet.contains('\0') {
        return Err("Not sent — the selection is not text. Nothing left your machine.".to_string());
    }
    let secrets = secret_reasons(snippet);
    if !secrets.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            secrets.join(" ")
        ));
    }
    let language = language_of(p);
    Ok(match lens {
        AiReadLens::Explain => GatedPrompt {
            system: system_prompt(level),
            user: format!("Explain this {language} snippet:\n\n```\n{snippet}\n```"),
        },
        AiReadLens::Review => GatedPrompt {
            system: review_system_prompt(level),
            user: format!(
                "Give me a read-only review lens for this {language} snippet. Ask what I should verify and ground every question in the visible code:\n\n```\n{snippet}\n```"
            ),
        },
    })
}

fn send_gated_prompt(
    prompt: GatedPrompt,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    hangar_ai::explain(config, &prompt.system, &prompt.user, MAX_TOKENS)
}

fn stream_gated_prompt<F>(
    prompt: GatedPrompt,
    config: &hangar_ai::ProviderConfig,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    hangar_ai::explain_stream(config, &prompt.system, &prompt.user, MAX_TOKENS, on_delta)
}

/// Explain a file with the configured provider after a fresh complete send-gate.
pub(crate) fn ai_explain_file_for_path(
    path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    send_gated_prompt(gated_file_prompt(path, level, AiReadLens::Explain)?, config)
}

pub(crate) fn ai_explain_file_stream_for_path<F>(
    path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    stream_gated_prompt(
        gated_file_prompt(path, level, AiReadLens::Explain)?,
        config,
        on_delta,
    )
}

pub(crate) fn ai_explain_text_with_config(
    snippet: &str,
    origin_path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    send_gated_prompt(
        gated_text_prompt(snippet, origin_path, level, AiReadLens::Explain)?,
        config,
    )
}

pub(crate) fn ai_explain_text_stream_with_config<F>(
    snippet: &str,
    origin_path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    stream_gated_prompt(
        gated_text_prompt(snippet, origin_path, level, AiReadLens::Explain)?,
        config,
        on_delta,
    )
}

/// Review an inventoried file using the same read and secret gates as Explain.
pub(crate) fn ai_review_file_for_path(
    path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    send_gated_prompt(gated_file_prompt(path, level, AiReadLens::Review)?, config)
}

pub(crate) fn ai_review_file_stream_for_path<F>(
    path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    stream_gated_prompt(
        gated_file_prompt(path, level, AiReadLens::Review)?,
        config,
        on_delta,
    )
}

pub(crate) fn ai_review_text_with_config(
    snippet: &str,
    origin_path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    send_gated_prompt(
        gated_text_prompt(snippet, origin_path, level, AiReadLens::Review)?,
        config,
    )
}

pub(crate) fn ai_review_text_stream_with_config<F>(
    snippet: &str,
    origin_path: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    stream_gated_prompt(
        gated_text_prompt(snippet, origin_path, level, AiReadLens::Review)?,
        config,
        on_delta,
    )
}

pub(crate) fn ai_send_disclosure_for_path(
    path: &str,
    snippet: Option<&str>,
    lens: AiReadLens,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<AiSendDisclosure, String> {
    let prompt = match snippet {
        Some(snippet) => gated_text_prompt(snippet, path, level, lens)?,
        None => gated_file_prompt(path, level, lens)?,
    };
    let disclosure =
        hangar_ai::request_disclosure(config, &prompt.system, &prompt.user, MAX_TOKENS)?;
    let send_chars = disclosure.request_body.chars().count() as u64;
    Ok(AiSendDisclosure {
        method: disclosure.method,
        url: disclosure.url,
        request_body: disclosure.request_body,
        fallback_request_body: disclosure.fallback_request_body,
        transport: disclosure.transport,
        mode: if config.local { "local" } else { "api" }.to_string(),
        model: config.model.clone(),
        format: config.format.as_tag().to_string(),
        send_chars,
        est_tokens: send_chars.div_ceil(4),
    })
}

fn selected_change_context(
    change_set: &SessionChangeSet,
    file_path: Option<&str>,
    edit_index: Option<usize>,
) -> Result<String, String> {
    let selected_path = file_path.map(hangar_core::normalize_path);
    let mut output = format!(
        "Recorded local change evidence\nSource: {}\nCoverage: {}\nCoverage note: {}\nTotals: {} files, {} edits, +{} / -{} lines\n",
        change_set.source_kind,
        change_set.coverage.label,
        change_set.coverage.note,
        change_set.files.len(),
        change_set.edit_count,
        change_set.added_lines,
        change_set.removed_lines,
    );
    let mut included = 0usize;
    let mut omitted = 0usize;
    for file in &change_set.files {
        if let Some(expected) = selected_path.as_deref() {
            if !hangar_core::normalize_path(&file.path).eq_ignore_ascii_case(expected) {
                continue;
            }
        }
        for (index, edit) in file.edits.iter().enumerate() {
            if selected_path.is_some() && edit_index.is_some() && edit_index != Some(index) {
                continue;
            }
            let block = serde_json::to_string_pretty(&serde_json::json!({
                "file": file.path,
                "fileReality": file.reality,
                "editIndex": index,
                "edit": edit,
            }))
            .map_err(|error| format!("Could not prepare recorded changes: {error}"))?;
            let required = block.len().saturating_add(2);
            if output.len().saturating_add(required) > READ_CAP as usize {
                omitted += 1;
                continue;
            }
            output.push('\n');
            output.push_str(&block);
            output.push('\n');
            included += 1;
        }
    }
    if included == 0 {
        return Err(if selected_path.is_some() {
            "That recorded edit is no longer available in this bounded recap.".to_string()
        } else {
            "This recap has no deterministic edit evidence to explain. Code Hangar will not ask a model to invent a story.".to_string()
        });
    }
    if omitted > 0 {
        output.push_str(&format!(
            "\nBounded context: {omitted} additional recorded edit{} omitted before the 60 KiB send cap.\n",
            if omitted == 1 { " was" } else { "s were" }
        ));
    }
    Ok(output)
}

fn gate_change_context(
    change_set: &SessionChangeSet,
    file_path: Option<&str>,
    edit_index: Option<usize>,
) -> Result<String, String> {
    let context = selected_change_context(change_set, file_path, edit_index)?;
    if context.contains('\0') {
        return Err(
            "Not sent — the recorded change context is not text. Nothing left your machine."
                .to_string(),
        );
    }
    let secrets = secret_reasons(&context);
    if !secrets.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            secrets.join(" ")
        ));
    }
    Ok(context)
}

pub(crate) fn ai_change_set_preview(
    change_set: &SessionChangeSet,
    file_path: Option<&str>,
    edit_index: Option<usize>,
) -> Result<AiExplainPreview, String> {
    let context = selected_change_context(change_set, file_path, edit_index)?;
    let blocked = secret_reasons(&context);
    let send_chars = if blocked.is_empty() {
        context.chars().count()
    } else {
        0
    };
    Ok(AiExplainPreview {
        blocked,
        send_chars,
        est_tokens: (send_chars as u64) / 4,
        language: "Recorded changes".to_string(),
    })
}

fn change_narration_system_prompt(level: &str) -> String {
    let base = "You are helping a non-programmer understand a retrospective record of changes made to their project by AI tools. Use ONLY the supplied local diff evidence, requests, provenance, confidence, coverage and compare-only reality labels. Return short sections in this exact order: `[story]`, `[why]`, `[learn]`, `[unknowns]`; omit only an empty section. Story describes the sequence and files in plain language. Why may connect an edit to its recorded user request, but must not invent intent. Learn teaches one or two concrete code-reading ideas visible in the diff. Unknowns states gaps caused by partial coverage, omitted records, shell activity or drift. Never output code changes, patches, commands, verdicts or claims that anything was executed. Never treat `Applied`, `Reverted`, `Drifted` or `FileMissing` as more than timestamped compare-only evidence.";
    match level {
        "engineer" => format!(
            "{base} Be precise and concise; preserve uncertainty and technical distinctions."
        ),
        _ => format!(
            "{base} Use short sentences and define any unavoidable technical term in one clause."
        ),
    }
}

fn change_learning_system_prompt(level: &str) -> String {
    let base = "You are teaching a non-programmer how to read ONE recorded code change after an AI tool made it. Use only the supplied diff, request, provenance and compare-only reality label. Return short sections in this exact order: `[what-changed]`, `[how-to-read]`, `[why-it-matters]`, `[unknowns]`. Explain added lines, removed lines and surrounding context without inventing missing line numbers or project behaviour. Teach at most two concepts grounded in visible text. Never propose a rewrite, output replacement code, run anything, or claim the recorded request proves intent.";
    match level {
        "engineer" => format!(
            "{base} Use precise software terminology and distinguish evidence from inference."
        ),
        _ => {
            format!("{base} Use simple, concrete language and define each technical term briefly.")
        }
    }
}

fn change_review_system_prompt(level: &str) -> String {
    let base = "You are a read-only review coach for a non-programmer inspecting changes made by AI tools. Use only the supplied local change evidence. Return short sections using ONLY this concern vocabulary, in this order: `[be-careful]`, `[double-check]`, `[heads-up]`, `[what-looks-deliberate]`, `[unknowns]`; omit an empty section. Under the first three, ask grounded questions and cite the visible file, request, added/removed text, provenance, coverage gap or drift label that prompted each question. Be-careful is credible data-loss/security/correctness harm; double-check is behaviour or failure handling worth verifying; heads-up is lower-impact maintenance or test uncertainty. Do not output fixes, patches, commands, rule IDs, verdicts, or claims that tests ran. Preserve uncertainty and never infer unseen code.";
    match level {
        "engineer" => format!("{base} Use precise terminology and separate confirmed evidence from review questions."),
        _ => format!("{base} Use plain language and explain in one sentence what could happen if each concern is real."),
    }
}

pub(crate) fn ai_narrate_change_set(
    change_set: &SessionChangeSet,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let context = gate_change_context(change_set, None, None)?;
    hangar_ai::explain(
        config,
        &change_narration_system_prompt(level),
        &format!("Tell the evidence-led story of these recorded changes:\n\n{context}"),
        MAX_TOKENS,
    )
}

pub(crate) fn ai_explain_recorded_change(
    change_set: &SessionChangeSet,
    file_path: &str,
    edit_index: usize,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let context = gate_change_context(change_set, Some(file_path), Some(edit_index))?;
    hangar_ai::explain(
        config,
        &change_learning_system_prompt(level),
        &format!("Teach me how to read this one recorded change:\n\n{context}"),
        900,
    )
}

pub(crate) fn ai_review_change_set_with_config(
    change_set: &SessionChangeSet,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let context = gate_change_context(change_set, None, None)?;
    hangar_ai::explain(
        config,
        &change_review_system_prompt(level),
        &format!("Ask what I should check in these recorded changes:\n\n{context}"),
        MAX_TOKENS,
    )
}

/// The rewrite persona. The model must return ONLY the rewritten content (no commentary, no code
/// fences) so it can be applied verbatim. Kept byte-identical per level for prefix caching.
#[cfg(feature = "agent_automation")]
fn rewrite_system_prompt(level: &str) -> String {
    let base = "You are proposing one small correction to a selected passage for someone who built this project using AI tools. Output ONLY the replacement for the supplied selection: no explanation, no commentary, and no Markdown code fences. Preserve the language, surrounding assumptions and behaviour outside the user's stated intent. Make the smallest change that satisfies the request. Never broaden the change beyond the selected passage.";
    match level {
        "engineer" => {
            format!("{base} The reader is technical; idiomatic, precise edits are appropriate.")
        }
        _ => format!("{base} Keep the changes conservative and easy to follow."),
    }
}

pub(crate) fn rewrite_output_allowance(snippet: &str) -> u32 {
    estimate_tokens(snippet)
        .saturating_add(128)
        .clamp(u64::from(REWRITE_MIN_TOKENS), u64::from(REWRITE_MAX_TOKENS)) as u32
}

/// Drop a single leading+trailing Markdown code fence the model may wrap the output in, so the
/// applied content is the raw file body. Only strips when BOTH a leading ``` line and a trailing
/// ``` are present, so a file that legitimately starts with a fence is left untouched.
#[cfg(feature = "agent_automation")]
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(first_newline) = rest.find('\n') {
            let body = &rest[first_newline + 1..];
            if let Some(end) = body.rfind("```") {
                return body[..end].trim_end_matches(['\n', '\r']).to_string();
            }
        }
    }
    text.to_string()
}

/// Rewrite a free-text selection with the configured provider and RETURN the rewrite (no write).
/// The whole source file is freshly read and hard-gated first, capped at 60 KiB, and the supplied
/// selection must occur exactly once. This keeps the later local splice unambiguous; the provider
/// still receives only the selected bytes and the plain-language intent.
#[cfg(feature = "agent_automation")]
pub(crate) fn ai_rewrite_text_with_config(
    snippet: &str,
    origin_path: &str,
    instruction: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<AiSelectionRewrite, String> {
    let p = Path::new(origin_path);
    if let Some(reason) = sensitive_path_reason(p) {
        return Err(format!("Not sent — {reason} Nothing left your machine."));
    }
    if snippet.trim().is_empty() {
        return Err("Not sent — no text was selected. Nothing left your machine.".to_string());
    }
    if snippet.contains('\0') {
        return Err("Not sent — the selection is not text. Nothing left your machine.".to_string());
    }
    if !p.is_file() {
        return Err(
            "Not sent — that file no longer exists. Nothing left your machine.".to_string(),
        );
    }
    let file_len = p
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(u64::MAX);
    if file_len > READ_CAP {
        return Err(format!(
            "Not sent — this file is larger than {} KB. A safe exact replacement requires one complete fresh read, so nothing left your machine.",
            READ_CAP / 1024
        ));
    }
    let (blocked, source) = gate_file_content(p)?;
    if !blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            blocked.join(" ")
        ));
    }
    let mut matches = source.match_indices(snippet);
    if matches.next().is_none() {
        return Err("Not sent — the selected text is no longer present in the file. Reload it and select again.".to_string());
    }
    if matches.next().is_some() {
        return Err("Not sent — that text appears more than once. Select a slightly larger unique passage so Code Hangar cannot change the wrong place.".to_string());
    }
    let secrets = secret_reasons(snippet);
    if !secrets.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            secrets.join(" ")
        ));
    }
    let language = language_of(p);
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return Err("Describe in plain language what should change in the selection.".to_string());
    }
    if instruction.chars().count() > 1000 {
        return Err("Keep the requested change under 1,000 characters.".to_string());
    }
    let user = format!("Change only this selected {language} passage according to this intent: {instruction}\n\nOutput only the replacement for the selection:\n\n```\n{snippet}\n```");
    let rewritten = hangar_ai::explain(
        config,
        &rewrite_system_prompt(level),
        &user,
        rewrite_output_allowance(snippet),
    )?;
    let replacement = strip_code_fences(&rewritten);
    if replacement.len() > READ_CAP as usize || replacement.contains('\0') {
        return Err("The proposed replacement is not safe to stage because it is too large or is not plain text.".to_string());
    }
    Ok(AiSelectionRewrite {
        source,
        replacement,
        language,
    })
}

/// Reachability check for a configured provider — sends a fixed "ping" (no file/user content, so
/// the send-gate is not involved). Used by the Settings "Test provider" button.
pub(crate) fn ai_provider_test_with_config(
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    hangar_ai::provider_test(config)
}

/// Best-effort token estimate (~4 chars/token), provider-agnostic and local (no network call), so
/// the UI can show an approximate input size before sending. Matches the explain preview's hint.
pub(crate) fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn summary_context_blocked(context: &str) -> Vec<String> {
    if context.trim().is_empty() {
        return vec!["This project has no README, manifest or text to summarize.".to_string()];
    }
    if context.contains('\0') {
        return vec!["The assembled project context is not text.".to_string()];
    }
    secret_reasons(context)
}

pub(crate) fn ai_summarize_project_preview(context: &str, level: &str) -> AiExplainPreview {
    let blocked = summary_context_blocked(context);
    let system = summary_system_prompt(level);
    let user = format!("Summarize this project from its local context:\n\n{context}");
    let send_chars = if blocked.is_empty() {
        system.chars().count().saturating_add(user.chars().count())
    } else {
        0
    };
    AiExplainPreview {
        blocked,
        send_chars,
        est_tokens: estimate_tokens(&system)
            .saturating_add(estimate_tokens(&user))
            .saturating_add(8),
        language: "Project context".to_string(),
    }
}

pub(crate) fn ai_summarize_project_disclosure_with_config(
    context: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<AiSendDisclosure, String> {
    let blocked = summary_context_blocked(context);
    if !blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            blocked.join(" ")
        ));
    }
    let system = summary_system_prompt(level);
    let user = format!("Summarize this project from its local context:\n\n{context}");
    let disclosure = hangar_ai::complete_request_disclosure(config, &system, &user, MAX_TOKENS)?;
    let send_chars = disclosure.request_body.chars().count() as u64;
    Ok(AiSendDisclosure {
        method: disclosure.method,
        url: disclosure.url,
        request_body: disclosure.request_body,
        fallback_request_body: disclosure.fallback_request_body,
        transport: disclosure.transport,
        mode: if config.local { "local" } else { "api" }.to_string(),
        model: config.model.clone(),
        format: config.format.as_tag().to_string(),
        send_chars,
        est_tokens: send_chars.div_ceil(4),
    })
}

fn summary_system_prompt(level: &str) -> String {
    let base = "You are summarizing a local software project for the person who built it — possibly with AI tools and without deep programming knowledge. From the provided README excerpt, detected stack, run commands and file list, write a short, plain-language summary covering: what the project is and does, its tech stack, and how to run it when that is clear. Be concrete, neutral and honest: no praise, no enthusiasm, no filler, no exclamation marks — do not editorialize about whether the project is good or exciting. Never invent features, files, or instructions you cannot see in the provided context.";
    match level {
        "engineer" => format!("{base} The reader is comfortable with software concepts; be precise and concise (4-6 sentences)."),
        _ => format!("{base} The reader is a beginner; keep it simple and concrete (3-5 short sentences)."),
    }
}

/// Summarize a project from its already-assembled local context (README excerpt / manifests / file
/// list). Re-runs the secret scan on the EXACT text that will be sent and refuses if a secret is
/// found — defense in depth on top of the sensitive-path guard the caller already applied when it
/// read those files. The provider (local loopback or external API) is resolved by the caller.
pub(crate) fn ai_summarize_project_with_config(
    context: &str,
    level: &str,
    config: &hangar_ai::ProviderConfig,
) -> Result<String, String> {
    let blocked = summary_context_blocked(context);
    if !blocked.is_empty() {
        return Err(format!(
            "Not sent — {} Nothing left your machine.",
            blocked.join(" ")
        ));
    }
    let user = format!("Summarize this project from its local context:\n\n{context}");
    hangar_ai::explain(config, &summary_system_prompt(level), &user, MAX_TOKENS)
}

/// Max characters of any single curated-context excerpt folded into a project summary. Small on
/// purpose: the excerpt is a hint about the file, not its full contents, and keeps total prompt
/// size (and cost) modest across several context files.
#[cfg(feature = "agent_automation")]
const CONTEXT_EXCERPT_MAX_CHARS: usize = 800;

/// Produce a bounded, send-SAFE excerpt of a curated context file, or `None` if it must not be sent.
///
/// This is the SAME send-gate the file explain/summary use, applied to each curated context file
/// before ANY of its bytes go into the AI-summary prompt: the sensitive/Protected-Zone path gate
/// runs first, then the single read whose scanned bytes ARE the candidate bytes (binary refused,
/// `secret_reasons` refused). Only a fully-clean file yields text, and only its first
/// `CONTEXT_EXCERPT_MAX_CHARS` characters. The whole assembled prompt is re-scanned by
/// `ai_summarize_project_with_config` before it leaves the machine, so this is defense in depth, not
/// the sole barrier. Best-effort: a missing/gated/secret-bearing file simply contributes no excerpt
/// (its name is still safe to list), never an error.
#[cfg(feature = "agent_automation")]
pub(crate) fn gated_context_excerpt(path: &str) -> Option<String> {
    let p = Path::new(path);
    if sensitive_path_reason(p).is_some() || !p.is_file() {
        return None;
    }
    let (blocked, content) = gate_file_content(p).ok()?;
    if !blocked.is_empty() {
        return None;
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut excerpt: String = trimmed.chars().take(CONTEXT_EXCERPT_MAX_CHARS).collect();
    if trimmed.chars().count() > CONTEXT_EXCERPT_MAX_CHARS {
        excerpt.push('…');
    }
    Some(excerpt)
}

/// Best-effort model list for a configured provider (empty unless it's an OpenAI-compatible
/// endpoint exposing `/models`). Drives an optional dropdown; the UI falls back to free text.
pub(crate) fn ai_provider_models_with_config(
    config: &hangar_ai::ProviderConfig,
) -> Result<Vec<String>, String> {
    hangar_ai::provider_models(config)
}

pub fn ai_key_set(key: &str) -> Result<(), String> {
    hangar_ai::key_set(key)
}

pub fn ai_key_status() -> bool {
    hangar_ai::key_status()
}

pub fn ai_key_clear() -> Result<(), String> {
    hangar_ai::key_clear()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded_change_set(content: &str) -> SessionChangeSet {
        serde_json::from_value(serde_json::json!({
            "path": "project:1",
            "sourceKind": "Combined local evidence",
            "coverage": {
                "level": "full",
                "label": "Fused local evidence",
                "note": "Observed from local records only."
            },
            "files": [{
                "path": "src/app.ts",
                "edits": [{
                    "source": "Cursor edit_file_v2",
                    "summary": "Recorded Cursor file edit",
                    "provenance": "Recorded Cursor diff bubble",
                    "confidence": "observed",
                    "reality": {
                        "status": "applied",
                        "label": "Appears applied",
                        "note": "The recorded after-block was found.",
                        "observedMs": 1700000000000i64
                    },
                    "request": "Change the label",
                    "hunks": [{
                        "header": "Recorded Cursor edit",
                        "oldStart": 1,
                        "newStart": 1,
                        "lines": [
                            { "kind": "removed", "content": "old", "oldLine": 1 },
                            { "kind": "added", "content": content, "newLine": 1 }
                        ]
                    }],
                    "addedLines": 1,
                    "removedLines": 1
                }],
                "addedLines": 1,
                "removedLines": 1,
                "reality": {
                    "status": "applied",
                    "label": "Appears applied",
                    "note": "The recorded after-block was found.",
                    "observedMs": 1700000000000i64
                }
            }],
            "editCount": 1,
            "addedLines": 1,
            "removedLines": 1,
            "redactedCount": 0,
            "parsedRecords": 1,
            "omittedRecords": 0
        }))
        .unwrap()
    }

    #[test]
    fn blocks_sensitive_paths() {
        let p = ai_explain_preview_for_path("C:\\Users\\me\\.ssh\\id_rsa").unwrap();
        assert!(!p.blocked.is_empty());
        let p2 = ai_explain_preview_for_path("project\\.env").unwrap();
        assert!(!p2.blocked.is_empty());
        let p3 = ai_explain_preview_for_path("project\\server.pem").unwrap();
        assert!(!p3.blocked.is_empty());
    }

    #[test]
    fn project_summary_blocks_a_secret_and_empty_context() {
        let config = hangar_ai::ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "test".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };
        // A secret in the assembled context blocks the summary BEFORE any network call — the same
        // send-gate that protects file explains protects the summary. A keyword-assignment form
        // (realistic for a README/manifest excerpt) and a known-prefix token both trip it.
        let leaky = "Title: My App\n\napi_key = abcdef0123456789abcdef0123456789abcdef01";
        let err = ai_summarize_project_with_config(leaky, "vibe", &config).unwrap_err();
        assert!(err.contains("Not sent"), "{err}");
        let prefixed = "Title: App\n\nDeploy with AKIAIOSFODNN7EXAMPLE today";
        assert!(ai_summarize_project_with_config(prefixed, "vibe", &config).is_err());
        // Empty context is refused too — nothing to summarize, no network call.
        assert!(ai_summarize_project_with_config("   ", "vibe", &config).is_err());
        // estimate_tokens is a local ~chars/4 heuristic (no network).
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn project_summary_preview_matches_the_exact_send_and_blocks_before_send() {
        let context = "Title: Local App\nStack: Rust and React\nRun: cargo run";
        let preview = ai_summarize_project_preview(context, "vibe");
        let system = summary_system_prompt("vibe");
        let user = format!("Summarize this project from its local context:\n\n{context}");

        assert!(preview.blocked.is_empty());
        assert_eq!(
            preview.send_chars,
            system.chars().count() + user.chars().count()
        );
        assert_eq!(
            preview.est_tokens,
            estimate_tokens(&system) + estimate_tokens(&user) + 8
        );

        let leaky = ai_summarize_project_preview(
            "Title: App\napi_key = abcdef0123456789abcdef0123456789abcdef01",
            "vibe",
        );
        assert!(!leaky.blocked.is_empty());
        assert_eq!(leaky.send_chars, 0);
    }

    #[test]
    fn project_summary_disclosure_contains_the_literal_non_streaming_request() {
        let config = hangar_ai::ProviderConfig {
            base_url: "http://127.0.0.1:8080/v1".to_string(),
            model: "summary-model".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };
        let context = "Title: Local App\nStack: Rust and React\nRun: cargo run";
        let disclosure =
            ai_summarize_project_disclosure_with_config(context, "vibe", &config).unwrap();
        let body: serde_json::Value = serde_json::from_str(&disclosure.request_body).unwrap();

        assert_eq!(disclosure.url, "http://127.0.0.1:8080/v1/chat/completions");
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "summary-model");
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .ends_with(context));
        assert!(disclosure.fallback_request_body.is_none());

        let error = ai_summarize_project_disclosure_with_config(
            "api_key = abcdef0123456789abcdef0123456789abcdef01",
            "vibe",
            &config,
        )
        .unwrap_err();
        assert!(error.contains("Not sent"), "{error}");
    }

    #[test]
    fn rewrite_output_budget_is_bounded_and_scales_with_the_selection() {
        assert_eq!(rewrite_output_allowance("short"), REWRITE_MIN_TOKENS);
        assert_eq!(rewrite_output_allowance(&"x".repeat(4_000)), 1_128);
        assert_eq!(
            rewrite_output_allowance(&"x".repeat(100_000)),
            REWRITE_MAX_TOKENS
        );
    }

    #[test]
    fn explain_text_blocks_secret_empty_and_sensitive_origin() {
        let config = hangar_ai::ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "test".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };
        let ok_origin = "project/src/main.rs";
        // The same secret send-gate guards a free-text selection: a secret in the snippet is
        // refused BEFORE any network call, exactly like a file explain.
        let leaky = "let token = ghp_abcdefghijklmnopqrstuvwxyz0123456789;";
        let err = ai_explain_text_with_config(leaky, ok_origin, "vibe", &config).unwrap_err();
        assert!(err.contains("Not sent"), "{err}");
        // An empty selection is refused — nothing to explain, no network call.
        assert!(ai_explain_text_with_config("   ", ok_origin, "vibe", &config).is_err());
        // A NUL byte means the selection is not text — refused.
        assert!(ai_explain_text_with_config("fn main() {}\0", ok_origin, "vibe", &config).is_err());
        // Parity with file explain: a sensitive/protected ORIGIN refuses even an innocuous snippet,
        // so a transiently-revealed sensitive file's text can never be selected and sent.
        assert!(ai_explain_text_with_config(
            "fn main() {}",
            "C:\\Users\\me\\.ssh\\id_rsa",
            "vibe",
            &config
        )
        .is_err());
        assert!(ai_explain_text_with_config("x = 1", "project/.env", "vibe", &config).is_err());
    }

    #[test]
    fn review_lens_is_question_led_read_only_and_uses_the_same_send_gate() {
        let prompt = review_system_prompt("vibe");
        assert!(prompt.contains("be-careful"));
        assert!(prompt.contains("double-check"));
        assert!(prompt.contains("heads-up"));
        assert!(prompt.contains("Example:"));
        assert!(prompt.contains("Do not rewrite code"));
        assert!(prompt.contains("phrase every risk as a question"));

        let config = hangar_ai::ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "test".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };
        let secret = "const token = ghp_abcdefghijklmnopqrstuvwxyz0123456789;";
        let error =
            ai_review_text_with_config(secret, "project/src/main.ts", "vibe", &config).unwrap_err();
        assert!(error.contains("Not sent"), "{error}");
        assert!(ai_review_text_with_config("x = 1", "project/.env", "vibe", &config).is_err());
    }

    #[test]
    fn exact_send_disclosure_uses_fresh_gated_bytes_and_never_exposes_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn answer() -> u8 { 42 }\n").unwrap();
        let config = hangar_ai::ProviderConfig {
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            model: "small-local".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };

        let disclosure = ai_send_disclosure_for_path(
            &path.to_string_lossy(),
            None,
            AiReadLens::Explain,
            "vibe",
            &config,
        )
        .unwrap();
        assert_eq!(disclosure.method, "POST");
        assert_eq!(disclosure.url, "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(disclosure.mode, "local");
        assert_eq!(disclosure.model, "small-local");
        assert!(disclosure.request_body.contains("fn answer() -> u8 { 42 }"));
        assert!(disclosure.request_body.contains("\"stream\":true"));
        assert!(disclosure
            .fallback_request_body
            .as_deref()
            .is_some_and(|body| body.contains("\"stream\":false")));
        for forbidden in ["authorization", "bearer", "api-key", "x-api-key"] {
            assert!(!disclosure
                .request_body
                .to_ascii_lowercase()
                .contains(forbidden));
        }

        std::fs::write(
            &path,
            "const token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\";\n",
        )
        .unwrap();
        let error = ai_send_disclosure_for_path(
            &path.to_string_lossy(),
            None,
            AiReadLens::Explain,
            "vibe",
            &config,
        )
        .unwrap_err();
        assert!(error.contains("Not sent"), "{error}");
    }

    #[test]
    fn disclosure_and_streaming_helpers_remain_read_only() {
        let source = include_str!("ai_assist.rs");
        for (start, end, required) in [
            (
                "fn send_gated_prompt(",
                "fn stream_gated_prompt",
                "hangar_ai::explain",
            ),
            (
                "fn stream_gated_prompt",
                "/// Explain a file",
                "hangar_ai::explain_stream",
            ),
            (
                "pub(crate) fn ai_send_disclosure_for_path",
                "fn selected_change_context",
                "hangar_ai::request_disclosure",
            ),
        ] {
            let body = source
                .split_once(start)
                .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
                .expect("AI read helper body");
            assert!(body.contains(required), "{start} does not call {required}");
            for forbidden in [
                "std::fs::write",
                "File::create",
                "write_file_content",
                "apply_value_edit",
                "hangar_mutation",
                "Command::new",
            ] {
                assert!(!body.contains(forbidden), "{start} contains {forbidden}");
            }
        }
    }

    #[test]
    fn every_provider_backed_ai_surface_remains_read_only() {
        let source = include_str!("ai_assist.rs");
        let marker = "pub(crate) fn ai_";
        let line_marker = "\npub(crate) fn ai_";
        let starts: Vec<usize> = source
            .match_indices(line_marker)
            .map(|(index, _)| index + 1)
            .collect();
        let mut checked = Vec::new();
        for (position, start) in starts.iter().copied().enumerate() {
            let mut end = starts.get(position + 1).copied().unwrap_or(source.len());
            let body_start = start + marker.len();
            for boundary in ["\nfn ", "\n#[cfg("] {
                if let Some(offset) = source[body_start..end].find(boundary) {
                    end = end.min(body_start + offset);
                }
            }
            let segment = &source[start..end];
            let name = format!(
                "ai_{}",
                segment[marker.len()..]
                    .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .next()
                    .unwrap_or_default()
            );
            let provider_backed = ["hangar_ai::", "send_gated_prompt(", "stream_gated_prompt("]
                .iter()
                .any(|needle| segment.contains(needle));
            if !provider_backed {
                continue;
            }
            checked.push(name.clone());
            for forbidden in [
                "std::fs::write",
                "fs::write",
                "File::create",
                "write_file_content",
                "write_file_with_snapshot",
                "apply_value_edit",
                "edit_snapshot_restore",
                "hangar_mutation",
                "Command::new",
            ] {
                assert!(!segment.contains(forbidden), "{name} contains {forbidden}");
            }
        }

        for required in [
            "ai_walkthrough_file_for_path",
            "ai_follow_up_for_path",
            "ai_explain_file_for_path",
            "ai_review_file_for_path",
            "ai_narrate_change_set",
            "ai_explain_recorded_change",
            "ai_review_change_set_with_config",
            "ai_rewrite_text_with_config",
            "ai_summarize_project_with_config",
        ] {
            assert!(
                checked.iter().any(|name| name == required),
                "did not audit {required}"
            );
        }
        assert!(checked.len() >= 15, "only audited {checked:?}");
    }

    #[test]
    fn change_ai_context_is_bounded_grounded_and_secret_gated() {
        let clean = recorded_change_set("new");
        let preview = ai_change_set_preview(&clean, None, None).unwrap();
        assert!(preview.blocked.is_empty());
        assert!(preview.send_chars > 0);
        assert_eq!(preview.language, "Recorded changes");
        let one = selected_change_context(&clean, Some("src\\app.ts"), Some(0)).unwrap();
        assert!(one.contains("Recorded Cursor diff bubble"));
        assert!(one.contains("observedMs"));
        assert!(selected_change_context(&clean, Some("src/missing.ts"), Some(0)).is_err());

        let leaky = recorded_change_set("ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        let blocked = ai_change_set_preview(&leaky, None, None).unwrap();
        assert!(!blocked.blocked.is_empty());
        let config = hangar_ai::ProviderConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "test".to_string(),
            format: hangar_ai::ProviderFormat::ChatCompletions,
            local: true,
        };
        let error = ai_narrate_change_set(&leaky, "vibe", &config).unwrap_err();
        assert!(error.contains("Not sent"), "{error}");
    }

    #[test]
    fn change_personas_are_structured_honest_and_read_only() {
        let narration = change_narration_system_prompt("vibe");
        for label in ["[story]", "[why]", "[learn]", "[unknowns]"] {
            assert!(narration.contains(label));
        }
        assert!(narration.contains("must not invent intent"));
        let learning = change_learning_system_prompt("vibe");
        assert!(learning.contains("[how-to-read]"));
        assert!(learning.contains("without inventing missing line numbers"));
        let review = change_review_system_prompt("vibe");
        assert!(review.contains("[be-careful]"));
        assert!(review.contains("Do not output fixes"));

        let source = include_str!("ai_assist.rs");
        for (start, end) in [
            (
                "pub(crate) fn ai_narrate_change_set",
                "pub(crate) fn ai_explain_recorded_change",
            ),
            (
                "pub(crate) fn ai_explain_recorded_change",
                "pub(crate) fn ai_review_change_set_with_config",
            ),
            (
                "pub(crate) fn ai_review_change_set_with_config",
                "/// The rewrite persona",
            ),
        ] {
            let body = source
                .split_once(start)
                .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
                .expect("AI change function body");
            assert!(body.contains("hangar_ai::explain"));
            for forbidden in [
                "write_file_content",
                "apply_value_edit",
                "edit_snapshot_restore",
                "hangar_mutation",
                "Command::new",
            ] {
                assert!(!body.contains(forbidden), "{start} contains {forbidden}");
            }
        }
    }

    #[test]
    fn secret_scanner_flags_high_signal_tokens() {
        assert!(
            !secret_reasons("const k = \"sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123\";")
                .is_empty()
        );
        assert!(!secret_reasons("token = ghp_abcdefghijklmnopqrstuvwxyz0123456789").is_empty());
        assert!(!secret_reasons("-----BEGIN RSA PRIVATE KEY-----\n...").is_empty());
        assert!(!secret_reasons("AWS=AKIAIOSFODNN7EXAMPLE").is_empty());
        assert!(!secret_reasons("api_key = \"abcdef0123456789abcdef\"").is_empty());
        // Single-line JSON where the secret is NOT the value after the first separator (the prior
        // first-`:`-only parser missed this because the first field's value held a space).
        assert!(!secret_reasons(
            "{\"description\":\"my service\",\"api_key\":\"abcdefABCDEF0123456789mnop\"}"
        )
        .is_empty());
        // Ordinary code is not flagged.
        assert!(secret_reasons("fn main() { println!(\"hello\"); }").is_empty());
        assert!(secret_reasons("const apiBase = \"https://example.com/api\";").is_empty());
    }

    #[test]
    fn walkthrough_sectionizer_is_language_aware_bounded_and_stable() {
        let source = "use std::path::Path;\n\npub struct App {\n    ready: bool,\n}\n\nimpl App {\n    pub fn new() -> Self { Self { ready: true } }\n}\n\npub fn run() {\n    println!(\"ready\");\n}\n";
        let sections = sectionize_content("Rust", source);
        assert!(sections.len() >= 4, "{sections:#?}");
        assert!(sections
            .iter()
            .any(|section| section.summary.title.contains("pub struct App")));
        assert!(sections
            .iter()
            .any(|section| section.summary.title.contains("impl App")));
        assert!(sections
            .iter()
            .any(|section| section.summary.title.contains("pub fn run")));
        let ids: HashSet<&str> = sections
            .iter()
            .map(|section| section.summary.id.as_str())
            .collect();
        assert_eq!(ids.len(), sections.len());
        let context = compose_walkthrough_context(&sections, &[]).unwrap();
        assert!(context.len() <= READ_CAP as usize);
        assert!(context.contains("[section:"));
    }

    #[test]
    fn walkthrough_maps_the_complete_bounded_file_in_safe_batches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.ts");
        let mut source = (0..7_000)
            .map(|index| format!("const value{index} = {index};\n"))
            .collect::<String>();
        source.push_str("const finalSentinel = 'walkthrough-end';\n");
        assert!(source.len() > READ_CAP as usize);
        std::fs::write(&path, &source).unwrap();

        let material = walkthrough_material(&path).unwrap();
        assert!(material.blocked.is_empty(), "{:?}", material.blocked);
        assert!(!material.truncated);
        assert_eq!(
            material
                .sections
                .iter()
                .map(|section| section.content.as_str())
                .collect::<String>(),
            source
        );
        assert!(material
            .sections
            .iter()
            .all(|section| section.summary.context_bytes <= READ_CAP));

        let preview = ai_walkthrough_preview_for_path(&path.to_string_lossy()).unwrap();
        assert!(!preview.default_section_ids.is_empty());
        assert!(preview.default_section_ids.len() < preview.sections.len());
        let initial =
            compose_walkthrough_context(&material.sections, &preview.default_section_ids).unwrap();
        assert!(initial.len() <= READ_CAP as usize);
        let last_id = material.sections.last().unwrap().summary.id.clone();
        let final_batch = compose_walkthrough_context(&material.sections, &[last_id]).unwrap();
        assert!(final_batch.contains("finalSentinel"));
    }

    #[test]
    fn walkthrough_scans_for_secrets_beyond_the_first_provider_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("late-secret.ts");
        let mut source = "const harmless = true;\n".repeat(4_000);
        assert!(source.len() > READ_CAP as usize);
        source.push_str("const token = 'ghp_abcdefghijklmnopqrstuvwxyz0123456789';\n");
        std::fs::write(&path, source).unwrap();

        let preview = ai_walkthrough_preview_for_path(&path.to_string_lossy()).unwrap();
        assert!(!preview.blocked.is_empty());
        assert!(preview.sections.is_empty());
    }

    #[test]
    fn hash_snippet_matches_frontend_utf16_djb2() {
        assert_eq!(hash_snippet("abc"), "b885c8b");
        assert_eq!(hash_snippet("café"), "7c9503b8");
        assert_eq!(hash_snippet("🙂"), "762864");
    }

    #[test]
    fn follow_up_preview_is_section_scoped_and_secret_gated() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ch-ai-followup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.rs");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"pub fn answer() -> u32 {\n    42\n}\n")
            .unwrap();
        let preview = ai_walkthrough_preview_for_path(&path.to_string_lossy()).unwrap();
        let section_id = preview.sections.first().unwrap().id.clone();
        let follow = ai_follow_up_preview_for_path(
            &path.to_string_lossy(),
            &section_id,
            &[],
            "What does this return?",
        )
        .unwrap();
        assert!(follow.send_chars > 0);
        assert!(ai_follow_up_preview_for_path(
            &path.to_string_lossy(),
            &section_id,
            &[],
            "Is ghp_abcdefghijklmnopqrstuvwxyz0123456789 used?",
        )
        .is_err());
        assert!(follow_up_system_prompt("vibe").contains("Do not propose or output edits"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anchored_snippets_require_one_exact_match() {
        assert_eq!(
            unique_snippet_line_range("one\ntwo\nthree", "two").unwrap(),
            (2, 2)
        );
        assert!(unique_snippet_line_range("same\nsame", "same").is_err());
        assert!(unique_snippet_line_range("one", "missing").is_err());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn gated_context_excerpt_includes_clean_but_excludes_sensitive_and_secret() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ch-ai-ctx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // A clean doc yields a bounded excerpt.
        let clean = dir.join("overview.md");
        std::fs::File::create(&clean)
            .unwrap()
            .write_all(b"# Overview\n\nThis project does a specific, describable thing.")
            .unwrap();
        let excerpt = gated_context_excerpt(&clean.to_string_lossy())
            .expect("a clean context file must yield an excerpt");
        assert!(excerpt.contains("Overview"));

        // A file whose bytes contain a secret is refused outright — no excerpt, no bytes.
        let secret_doc = dir.join("notes.md");
        std::fs::File::create(&secret_doc)
            .unwrap()
            .write_all(b"deploy key: ghp_abcdefghijklmnopqrstuvwxyz0123456789")
            .unwrap();
        assert!(
            gated_context_excerpt(&secret_doc.to_string_lossy()).is_none(),
            "a secret-bearing context file must contribute no excerpt"
        );

        // A sensitive PATH (a dotenv) is refused on the path gate before any read.
        let env = dir.join(".env");
        std::fs::File::create(&env)
            .unwrap()
            .write_all(b"HARMLESS=1")
            .unwrap();
        assert!(
            gated_context_excerpt(&env.to_string_lossy()).is_none(),
            "a sensitive-path context file must contribute no excerpt"
        );

        // A file that does not exist contributes nothing (best-effort), never an error.
        assert!(gated_context_excerpt(&dir.join("missing.md").to_string_lossy()).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utf16_secret_is_caught_or_refused_never_silently_sent() {
        use std::io::Write;
        let secret = "export ANTHROPIC_API_KEY=\"sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123\";";
        let dir = std::env::temp_dir().join(format!("ch-ai-utf16-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // (1) UTF-16LE WITH BOM: decoded to clean UTF-8, so the secret scanner CATCHES it.
        let mut le_bom = vec![0xFF, 0xFE];
        for u in secret.encode_utf16() {
            le_bom.extend_from_slice(&u.to_le_bytes());
        }
        let f1 = dir.join("config_le.ts");
        std::fs::File::create(&f1)
            .unwrap()
            .write_all(&le_bom)
            .unwrap();
        let (blocked1, _) = gate_file_content(&f1).unwrap();
        assert!(!blocked1.is_empty(), "UTF-16LE+BOM secret must be blocked");

        // (2) UTF-16LE WITHOUT BOM keeps NUL bytes -> refused as binary/non-UTF-8, and the literal
        // secret never appears contiguously in the send text. (The round-1 lossy decode used to
        // pass this through, letting the recipient recover the secret by stripping NULs.)
        let mut le_nobom = Vec::new();
        for u in secret.encode_utf16() {
            le_nobom.extend_from_slice(&u.to_le_bytes());
        }
        let f2 = dir.join("config_nobom.ts");
        std::fs::File::create(&f2)
            .unwrap()
            .write_all(&le_nobom)
            .unwrap();
        let (blocked2, send2) = gate_file_content(&f2).unwrap();
        assert!(!blocked2.is_empty(), "NUL-interleaved file must be refused");
        assert!(
            !send2.contains("sk-ant-"),
            "raw secret must not be in the send text"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
