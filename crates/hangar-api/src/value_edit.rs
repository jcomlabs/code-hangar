use hangar_core::{EditableValue, EditableValueSet, ValueEditRequest, ValueEditResult};
use std::fs;
use std::path::Path;
use toml_edit::{Item as TomlItem, Value as TomlValue};

const MAX_VALUE_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_EDITABLE_VALUES: usize = 200;
const MINIFIED_LINE_BYTES: usize = 512;
const MINIFIED_SYNTAX_MARKERS: usize = 16;

pub(crate) struct PreparedValueEdit {
    pub source: String,
    pub content: String,
    pub changed: EditableValue,
}

pub(crate) fn editable_values(
    state: &super::AppState,
    node_id: i64,
) -> Result<EditableValueSet, String> {
    let (path, source) = read_authorized_source(state, node_id)?;
    extract_values(node_id, &path, &source)
}

pub(crate) fn apply_value_edit(
    state: &super::AppState,
    node_id: i64,
    request: &ValueEditRequest,
) -> Result<ValueEditResult, String> {
    let prepared = prepare_value_edit(state, node_id, request)?;
    apply_prepared_value_edit(state, node_id, request, prepared)
}

pub(crate) fn apply_prepared_value_edit(
    state: &super::AppState,
    node_id: i64,
    request: &ValueEditRequest,
    prepared: PreparedValueEdit,
) -> Result<ValueEditResult, String> {
    let outcome = super::edit_snapshot::write_file_with_snapshot(
        state,
        node_id,
        &prepared.content,
        "value",
        None,
        Some(&request.expected_source_hash),
    )?;
    let mut message = "Value saved. The previous file version is available in history.".to_string();
    if let Some(warning) = outcome.ledger_warning {
        message.push(' ');
        message.push_str(&warning);
    }
    Ok(ValueEditResult {
        node_id,
        snapshot_id: outcome.snapshot_id,
        source_hash: outcome.after_hash,
        value: prepared.changed,
        message,
    })
}

pub(crate) fn prepare_value_edit(
    state: &super::AppState,
    node_id: i64,
    request: &ValueEditRequest,
) -> Result<PreparedValueEdit, String> {
    let (path, source) = read_authorized_source(state, node_id)?;
    let values = extract_values(node_id, &path, &source)?;
    let (content, changed) = apply_value_to_source(&source, &values, request)?;
    validate_content_after_edit(&path, &content)?;
    Ok(PreparedValueEdit {
        source,
        content,
        changed,
    })
}

fn read_authorized_source(
    state: &super::AppState,
    node_id: i64,
) -> Result<(String, String), String> {
    let (path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&path, &project_paths)?;
    let metadata = fs::metadata(&path).map_err(|error| {
        format!("Values unavailable: the file could not be inspected ({error}).")
    })?;
    if metadata.len() > MAX_VALUE_FILE_BYTES as u64 {
        return Err(
            "Values unavailable: this file is above the safe editing size limit.".to_string(),
        );
    }
    let bytes = fs::read(&path)
        .map_err(|error| format!("Values unavailable: the file could not be read ({error})."))?;
    let source = String::from_utf8(bytes)
        .map_err(|_| "Values unavailable: this file is not UTF-8 text.".to_string())?;
    Ok((path, source))
}

fn extract_values(node_id: i64, path: &str, source: &str) -> Result<EditableValueSet, String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let supported = matches!(extension.as_str(), "json" | "toml")
        || SourceLanguage::from_extension(&extension).is_some();
    if !supported {
        return Err("Values are available for JSON, TOML and supported source files.".to_string());
    }
    if looks_minified(source) {
        return Err(
            "Values unavailable: minified files are not eligible for safe literal editing."
                .to_string(),
        );
    }
    let values = match extension.as_str() {
        "json" => extract_json_values(source)?,
        "toml" => extract_toml_values(source)?,
        _ => SourceScanner::new(
            source,
            SourceLanguage::from_extension(&extension).expect("supported source extension"),
        )
        .scan()?,
    };
    if values.is_empty() {
        return Err(
            "No safely editable string, number or toggle was found in this file.".to_string(),
        );
    }
    Ok(EditableValueSet {
        node_id,
        path: path.to_string(),
        format: extension,
        source_hash: source_hash(source),
        values,
    })
}

fn apply_value_to_source(
    source: &str,
    set: &EditableValueSet,
    request: &ValueEditRequest,
) -> Result<(String, EditableValue), String> {
    if source_hash(source) != request.expected_source_hash
        || set.source_hash != request.expected_source_hash
    {
        return Err(
            "Not saved: the file changed on disk. Reload Values before trying again.".to_string(),
        );
    }
    let current = set
        .values
        .iter()
        .find(|value| value.id == request.value_id)
        .ok_or_else(|| "Not saved: that value is no longer present. Reload Values.".to_string())?;
    let start = usize::try_from(current.start_byte)
        .map_err(|_| "Not saved: invalid value position.".to_string())?;
    let end = usize::try_from(current.end_byte)
        .map_err(|_| "Not saved: invalid value position.".to_string())?;
    let raw = source
        .get(start..end)
        .ok_or_else(|| "Not saved: the value position is no longer valid.".to_string())?;
    if raw != current.raw_value || raw != request.expected_raw_value {
        return Err(
            "Not saved: this value changed on disk. Reload Values before trying again.".to_string(),
        );
    }
    let replacement = encode_value(
        &set.format,
        &current.kind,
        &request.new_value,
        &current.raw_value,
    )?;
    if replacement == raw {
        return Err("Not saved: the value is unchanged.".to_string());
    }
    let mut content = String::with_capacity(source.len() - raw.len() + replacement.len());
    content.push_str(&source[..start]);
    content.push_str(&replacement);
    content.push_str(&source[end..]);
    validate_format(&set.format, &content)?;
    let refreshed = extract_values(set.node_id, &set.path, &content)?;
    let changed = refreshed
        .values
        .iter()
        .find(|value| value.path == current.path)
        .cloned()
        .ok_or_else(|| "Not saved: the edited value could not be verified.".to_string())?;
    Ok((content, changed))
}

fn encode_value(format: &str, kind: &str, value: &str, raw: &str) -> Result<String, String> {
    match (format, kind) {
        ("json", "string") => serde_json::to_string(value)
            .map_err(|error| format!("Not saved: invalid text value ({error}).")),
        ("json", "number") => match serde_json::from_str::<serde_json::Value>(value.trim()) {
            Ok(serde_json::Value::Number(number)) => Ok(number.to_string()),
            _ => Err("Not saved: enter a valid JSON number.".to_string()),
        },
        ("json", "boolean") | ("toml", "boolean") => match value.trim() {
            "true" => Ok("true".to_string()),
            "false" => Ok("false".to_string()),
            _ => Err("Not saved: a toggle must be true or false.".to_string()),
        },
        ("toml", "string") => Ok(TomlValue::from(value.to_string()).to_string()),
        ("toml", "number") => {
            let trimmed = value.trim();
            if let Ok(integer) = trimmed.parse::<i64>() {
                return Ok(TomlValue::from(integer).to_string());
            }
            let float = trimmed
                .parse::<f64>()
                .map_err(|_| "Not saved: enter a valid TOML number.".to_string())?;
            if !float.is_finite() {
                return Err("Not saved: enter a finite TOML number.".to_string());
            }
            Ok(TomlValue::from(float).to_string())
        }
        (_, "string") if SourceLanguage::from_extension(format).is_some() => {
            encode_source_string(raw, value)
        }
        (_, "number") if SourceLanguage::from_extension(format).is_some() => {
            let trimmed = value.trim();
            if safe_source_number(trimmed) {
                Ok(trimmed.to_string())
            } else {
                Err("Not saved: enter a plain decimal or hexadecimal number.".to_string())
            }
        }
        (_, "boolean") if SourceLanguage::from_extension(format).is_some() => {
            let enabled = match value.trim().to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => return Err("Not saved: a toggle must be true or false.".to_string()),
            };
            if matches!(format, "py" | "pyw") {
                Ok(if enabled { "True" } else { "False" }.to_string())
            } else {
                Ok(if enabled { "true" } else { "false" }.to_string())
            }
        }
        (_, "color") if SourceLanguage::from_extension(format).is_some() => {
            let trimmed = value.trim();
            if safe_hex_color(trimmed) {
                Ok(trimmed.to_string())
            } else {
                Err("Not saved: enter a hex colour such as #0f8 or #00ff88.".to_string())
            }
        }
        _ => Err("Not saved: this value type is not supported.".to_string()),
    }
}

fn validate_format(format: &str, source: &str) -> Result<(), String> {
    if looks_minified(source) {
        return Err(
            "Not saved: minified files are not eligible for safe literal editing.".to_string(),
        );
    }
    match format {
        "json" => serde_json::from_str::<serde_json::Value>(source)
            .map(|_| ())
            .map_err(|error| format!("Not saved: the result would not be valid JSON ({error}).")),
        "toml" => toml_edit::ImDocument::parse(source)
            .map(|_| ())
            .map_err(|error| format!("Not saved: the result would not be valid TOML ({error}).")),
        _ if SourceLanguage::from_extension(format).is_some() => SourceScanner::new(
            source,
            SourceLanguage::from_extension(format).expect("supported source extension"),
        )
        .scan()
        .map(|_| ())
        .map_err(|error| format!("Not saved: the source lexical boundary check failed ({error}).")),
        _ => Err("Not saved: unsupported value format.".to_string()),
    }
}

pub(crate) fn validate_content_after_edit(path: &str, source: &str) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "json" | "toml" => validate_format(&extension, source),
        _ if SourceLanguage::from_extension(&extension).is_some() => {
            validate_format(&extension, source)
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceLanguage {
    JavaScript,
    Python,
    Rust,
    Go,
    CFamily,
    Css,
}

impl SourceLanguage {
    fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => Some(Self::JavaScript),
            "py" | "pyw" => Some(Self::Python),
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "java" | "kt" | "kts" | "cs" => {
                Some(Self::CFamily)
            }
            "css" | "scss" => Some(Self::Css),
            _ => None,
        }
    }

    fn hash_comments(self) -> bool {
        self == Self::Python
    }

    fn slash_comments(self) -> bool {
        self != Self::Python
    }

    fn editable_single_quotes(self) -> bool {
        matches!(self, Self::JavaScript | Self::Python | Self::Css)
    }

    fn editable_backticks(self) -> bool {
        self == Self::JavaScript
    }
}

struct SourceScanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    cursor: usize,
    language: SourceLanguage,
    values: Vec<EditableValue>,
}

impl<'a> SourceScanner<'a> {
    fn new(source: &'a str, language: SourceLanguage) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            cursor: 0,
            language,
            values: Vec::new(),
        }
    }

    fn scan(mut self) -> Result<Vec<EditableValue>, String> {
        while self.cursor < self.bytes.len() {
            if self.language.hash_comments() && self.bytes[self.cursor] == b'#' {
                self.skip_line_comment();
                continue;
            }
            if self.language.slash_comments()
                && self.bytes.get(self.cursor..self.cursor + 2) == Some(b"//")
            {
                self.skip_line_comment();
                continue;
            }
            if self.language.slash_comments()
                && self.bytes.get(self.cursor..self.cursor + 2) == Some(b"/*")
            {
                self.skip_block_comment()?;
                continue;
            }
            let byte = self.bytes[self.cursor];
            if self.language == SourceLanguage::Rust
                && byte == b'\''
                && self.rust_lifetime_at_cursor()
            {
                self.cursor += 1;
                self.scan_identifier();
                continue;
            }
            if matches!(byte, b'\'' | b'"' | b'`') {
                self.scan_string(byte)?;
                continue;
            }
            if self.language == SourceLanguage::Css && byte == b'#' && self.scan_hex_color() {
                continue;
            }
            if is_ident_start(byte) {
                self.scan_identifier();
                continue;
            }
            if (byte.is_ascii_digit()
                || (byte == b'-'
                    && self
                        .bytes
                        .get(self.cursor + 1)
                        .is_some_and(u8::is_ascii_digit)))
                && self.scan_number()
            {
                continue;
            }
            self.advance_char();
        }
        Ok(self.values)
    }

    fn skip_line_comment(&mut self) {
        while self.cursor < self.bytes.len() && self.bytes[self.cursor] != b'\n' {
            self.cursor += 1;
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), String> {
        self.cursor += 2;
        while self.cursor + 1 < self.bytes.len() {
            if self.bytes.get(self.cursor..self.cursor + 2) == Some(b"*/") {
                self.cursor += 2;
                return Ok(());
            }
            self.advance_char();
        }
        Err("an existing block comment is not terminated".to_string())
    }

    fn scan_string(&mut self, quote: u8) -> Result<(), String> {
        let start = self.cursor;
        if self.language == SourceLanguage::Rust && quote == b'"' {
            if let Some(hashes) = self.rust_raw_string_hashes(start) {
                return self.skip_rust_raw_string(hashes);
            }
        }
        if self.language == SourceLanguage::Python
            && self.bytes.get(start..start + 3) == Some(&[quote, quote, quote])
        {
            self.cursor += 3;
            while self.cursor + 2 < self.bytes.len() {
                if self.bytes.get(self.cursor..self.cursor + 3) == Some(&[quote, quote, quote]) {
                    self.cursor += 3;
                    return Ok(());
                }
                if self.bytes[self.cursor] == b'\\' {
                    self.cursor = (self.cursor + 2).min(self.bytes.len());
                } else {
                    self.advance_char();
                }
            }
            return Err("an existing triple-quoted string is not terminated".to_string());
        }
        self.cursor += 1;
        let mut has_template_expression = false;
        while self.cursor < self.bytes.len() {
            let byte = self.bytes[self.cursor];
            if byte == b'\\' {
                self.cursor = (self.cursor + 2).min(self.bytes.len());
                continue;
            }
            if quote == b'`' && byte == b'$' && self.bytes.get(self.cursor + 1) == Some(&b'{') {
                has_template_expression = true;
            }
            self.advance_char();
            if byte == quote {
                let end = self.cursor;
                let prefixed_python_string = self.language == SourceLanguage::Python
                    && start > 0
                    && self.bytes[start - 1].is_ascii_alphabetic();
                let editable = !prefixed_python_string
                    && (quote == b'"'
                        || (quote == b'\'' && self.language.editable_single_quotes())
                        || (quote == b'`'
                            && self.language.editable_backticks()
                            && !has_template_expression));
                if editable && self.user_facing_string(start, end) {
                    let raw = &self.source[start..end];
                    let display = decode_source_string(raw);
                    self.push("string", display, start, end);
                }
                return Ok(());
            }
        }
        Err("an existing string is not terminated".to_string())
    }

    fn rust_lifetime_at_cursor(&self) -> bool {
        let Some(next) = self.bytes.get(self.cursor + 1).copied() else {
            return false;
        };
        if !is_ident_start(next) {
            return false;
        }
        let mut end = self.cursor + 2;
        while self
            .bytes
            .get(end)
            .is_some_and(|byte| is_ident_continue(*byte))
        {
            end += 1;
        }
        self.bytes.get(end) != Some(&b'\'')
    }

    fn rust_raw_string_hashes(&self, quote_start: usize) -> Option<usize> {
        let mut prefix = quote_start;
        while prefix > 0 && self.bytes[prefix - 1] == b'#' {
            prefix -= 1;
        }
        if prefix > 0 && self.bytes[prefix - 1] == b'r' {
            Some(quote_start - prefix)
        } else {
            None
        }
    }

    fn skip_rust_raw_string(&mut self, hashes: usize) -> Result<(), String> {
        self.cursor += 1;
        while self.cursor < self.bytes.len() {
            if self.bytes[self.cursor] == b'"' {
                let hash_end = self.cursor + 1 + hashes;
                if hash_end <= self.bytes.len()
                    && self.bytes[self.cursor + 1..hash_end]
                        .iter()
                        .all(|byte| *byte == b'#')
                {
                    self.cursor = hash_end;
                    return Ok(());
                }
            }
            self.advance_char();
        }
        Err("an existing raw string is not terminated".to_string())
    }

    fn user_facing_string(&self, start: usize, end: usize) -> bool {
        if end <= start + 2 {
            return false;
        }
        let line_start = self.source[..start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let prefix = self.source[line_start..start].trim().to_ascii_lowercase();
        !prefix.starts_with("import ")
            && !prefix.starts_with("from ")
            && !prefix.starts_with("use ")
            && !prefix.starts_with("mod ")
            && !prefix.contains("require(")
            && !prefix.contains("include(")
            && !prefix.ends_with("url(")
    }

    fn scan_identifier(&mut self) {
        let start = self.cursor;
        self.cursor += 1;
        while self
            .bytes
            .get(self.cursor)
            .is_some_and(|byte| is_ident_continue(*byte))
        {
            self.cursor += 1;
        }
        let word = &self.source[start..self.cursor];
        let is_boolean = if self.language == SourceLanguage::Python {
            matches!(word, "True" | "False")
        } else {
            matches!(word, "true" | "false")
        };
        if is_boolean {
            self.push(
                "boolean",
                word.eq_ignore_ascii_case("true").to_string(),
                start,
                self.cursor,
            );
        }
    }

    fn scan_number(&mut self) -> bool {
        let start = self.cursor;
        if start > 0 && is_ident_continue(self.bytes[start - 1]) {
            self.cursor += 1;
            return true;
        }
        if self.bytes[self.cursor] == b'-' {
            self.cursor += 1;
        }
        if self.bytes.get(self.cursor..self.cursor + 2) == Some(b"0x")
            || self.bytes.get(self.cursor..self.cursor + 2) == Some(b"0X")
        {
            self.cursor += 2;
            while self
                .bytes
                .get(self.cursor)
                .is_some_and(|byte| byte.is_ascii_hexdigit() || *byte == b'_')
            {
                self.cursor += 1;
            }
        } else {
            while self
                .bytes
                .get(self.cursor)
                .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
            {
                self.cursor += 1;
            }
            if self.bytes.get(self.cursor) == Some(&b'.')
                && self
                    .bytes
                    .get(self.cursor + 1)
                    .is_some_and(u8::is_ascii_digit)
            {
                self.cursor += 1;
                while self
                    .bytes
                    .get(self.cursor)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
                {
                    self.cursor += 1;
                }
            }
            if matches!(self.bytes.get(self.cursor), Some(b'e' | b'E')) {
                let exponent = self.cursor;
                self.cursor += 1;
                if matches!(self.bytes.get(self.cursor), Some(b'+' | b'-')) {
                    self.cursor += 1;
                }
                let digits = self.cursor;
                while self
                    .bytes
                    .get(self.cursor)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
                {
                    self.cursor += 1;
                }
                if self.cursor == digits {
                    self.cursor = exponent;
                }
            }
        }
        let raw = &self.source[start..self.cursor];
        if safe_source_number(raw)
            && !self
                .bytes
                .get(self.cursor)
                .is_some_and(|byte| is_ident_continue(*byte))
        {
            self.push("number", raw.to_string(), start, self.cursor);
        }
        true
    }

    fn scan_hex_color(&mut self) -> bool {
        let start = self.cursor;
        let mut end = start + 1;
        while self
            .bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_hexdigit())
            && end - start <= 8
        {
            end += 1;
        }
        let raw = &self.source[start..end];
        if safe_hex_color(raw)
            && self.css_colour_context(start)
            && !self
                .bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_hexdigit())
        {
            self.cursor = end;
            self.push("color", raw.to_string(), start, end);
            true
        } else {
            false
        }
    }

    fn css_colour_context(&self, start: usize) -> bool {
        let line_start = self.source[..start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let prefix = &self.source[line_start..start];
        let last_colon = prefix.rfind(':');
        let last_open_brace = prefix.rfind('{');
        last_colon.is_some_and(|colon| last_open_brace.is_none_or(|brace| colon > brace))
    }

    fn push(&mut self, kind: &str, display: String, start: usize, end: usize) {
        if self.values.len() >= MAX_EDITABLE_VALUES {
            return;
        }
        let (line, column) = source_position(self.source, start);
        let label = match kind {
            "string" => format!("Text on line {line}"),
            "number" => format!("Number on line {line}"),
            "boolean" => format!("Toggle on line {line}"),
            "color" => format!("Colour on line {line}"),
            _ => format!("Value on line {line}"),
        };
        self.values.push(EditableValue {
            id: format!("source:{start}:{end}"),
            path: format!("Line {line}, column {column}"),
            label,
            kind: kind.to_string(),
            display_value: display,
            raw_value: self.source[start..end].to_string(),
            start_byte: start as u64,
            end_byte: end as u64,
        });
    }

    fn advance_char(&mut self) {
        let width = self.source[self.cursor..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        self.cursor = (self.cursor + width).min(self.bytes.len());
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn source_position(source: &str, byte_index: usize) -> (usize, usize) {
    let before = &source[..byte_index];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before.rsplit_once('\n').map_or_else(
        || before.chars().count() + 1,
        |(_, tail)| tail.chars().count() + 1,
    );
    (line, column)
}

fn decode_source_string(raw: &str) -> String {
    if raw.len() < 2 {
        return raw.to_string();
    }
    let mut output = String::new();
    let mut chars = raw[1..raw.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('\\') => output.push('\\'),
            Some('\'') => output.push('\''),
            Some('"') => output.push('"'),
            Some('`') => output.push('`'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => output.push('\\'),
        }
    }
    output
}

fn encode_source_string(raw: &str, value: &str) -> Result<String, String> {
    let quote = raw
        .as_bytes()
        .first()
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"' | b'`'))
        .ok_or_else(|| "Not saved: the original text delimiter is unsupported.".to_string())?;
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push(quote as char);
    for ch in value.chars() {
        match ch {
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            ch if ch as u32 == u32::from(quote) => {
                encoded.push('\\');
                encoded.push(ch);
            }
            '$' if quote == b'`' => encoded.push_str("\\$"),
            _ => encoded.push(ch),
        }
    }
    encoded.push(quote as char);
    Ok(encoded)
}

fn safe_source_number(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    if value.is_empty() {
        return false;
    }
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return separated_digits(hex, u8::is_ascii_hexdigit);
    }

    let (mantissa, exponent) = value.find(['e', 'E']).map_or((value, None), |index| {
        (&value[..index], Some(&value[index + 1..]))
    });
    if let Some(exponent) = exponent {
        let exponent = exponent
            .strip_prefix('+')
            .or_else(|| exponent.strip_prefix('-'))
            .unwrap_or(exponent);
        if !separated_digits(exponent, u8::is_ascii_digit) {
            return false;
        }
    }

    let (integer, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    separated_digits(integer, u8::is_ascii_digit)
        && fraction.is_none_or(|fraction| separated_digits(fraction, u8::is_ascii_digit))
}

fn separated_digits(value: &str, is_digit: fn(&u8) -> bool) -> bool {
    let mut previous_was_digit = false;
    let mut saw_digit = false;
    for byte in value.as_bytes() {
        if is_digit(byte) {
            previous_was_digit = true;
            saw_digit = true;
        } else if *byte == b'_' && previous_was_digit {
            previous_was_digit = false;
        } else {
            return false;
        }
    }
    saw_digit && previous_was_digit
}

fn looks_minified(source: &str) -> bool {
    source.lines().any(|line| {
        if line.len() < MINIFIED_LINE_BYTES {
            return false;
        }
        let whitespace = line
            .bytes()
            .filter(|byte| byte.is_ascii_whitespace())
            .count();
        let syntax_markers = line
            .bytes()
            .filter(|byte| {
                matches!(
                    byte,
                    b'{' | b'}' | b'[' | b']' | b'(' | b')' | b',' | b':' | b';' | b'='
                )
            })
            .count();
        whitespace * 8 <= line.len() && syntax_markers >= MINIFIED_SYNTAX_MARKERS
    })
}

fn safe_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn source_hash(source: &str) -> String {
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

struct JsonScanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    cursor: usize,
    values: Vec<EditableValue>,
}

impl<'a> JsonScanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            cursor: 0,
            values: Vec::new(),
        }
    }

    fn scan(mut self) -> Result<Vec<EditableValue>, String> {
        self.skip_ws();
        self.value("$", "Root")?;
        Ok(self.values)
    }

    fn value(&mut self, path: &str, label: &str) -> Result<(), String> {
        self.skip_ws();
        let start = self.cursor;
        match self.bytes.get(self.cursor).copied() {
            Some(b'{') => self.object(path),
            Some(b'[') => self.array(path),
            Some(b'"') => {
                let end = self.string_end()?;
                let raw = &self.source[start..end];
                let display = serde_json::from_str::<String>(raw).map_err(|error| {
                    format!("Values unavailable: invalid JSON string ({error}).")
                })?;
                self.push(path, label, "string", display, start, end);
                Ok(())
            }
            Some(b't') | Some(b'f') => {
                while matches!(self.bytes.get(self.cursor), Some(b'a'..=b'z')) {
                    self.cursor += 1;
                }
                let raw = &self.source[start..self.cursor];
                self.push(path, label, "boolean", raw.to_string(), start, self.cursor);
                Ok(())
            }
            Some(b'n') => {
                self.cursor = (self.cursor + 4).min(self.bytes.len());
                Ok(())
            }
            Some(b'-' | b'0'..=b'9') => {
                while matches!(
                    self.bytes.get(self.cursor),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.cursor += 1;
                }
                let raw = &self.source[start..self.cursor];
                self.push(path, label, "number", raw.to_string(), start, self.cursor);
                Ok(())
            }
            _ => Err("Values unavailable: JSON positions could not be reconstructed.".to_string()),
        }
    }

    fn object(&mut self, path: &str) -> Result<(), String> {
        self.cursor += 1;
        self.skip_ws();
        if self.take(b'}') {
            return Ok(());
        }
        loop {
            self.skip_ws();
            let key_start = self.cursor;
            let key_end = self.string_end()?;
            let key = serde_json::from_str::<String>(&self.source[key_start..key_end])
                .map_err(|error| format!("Values unavailable: invalid JSON key ({error})."))?;
            self.skip_ws();
            if !self.take(b':') {
                return Err("Values unavailable: JSON key has no value.".to_string());
            }
            let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
            self.value(&child, &key)?;
            self.skip_ws();
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err("Values unavailable: JSON object positions are invalid.".to_string());
            }
        }
        Ok(())
    }

    fn array(&mut self, path: &str) -> Result<(), String> {
        self.cursor += 1;
        self.skip_ws();
        if self.take(b']') {
            return Ok(());
        }
        let mut index = 0usize;
        loop {
            let child = format!("{path}/{index}");
            self.value(&child, &format!("Item {}", index + 1))?;
            index += 1;
            self.skip_ws();
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err("Values unavailable: JSON array positions are invalid.".to_string());
            }
        }
        Ok(())
    }

    fn string_end(&mut self) -> Result<usize, String> {
        if !self.take(b'"') {
            return Err("Values unavailable: expected a JSON string.".to_string());
        }
        let mut escaped = false;
        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            self.cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(self.cursor);
            }
        }
        Err("Values unavailable: unterminated JSON string.".to_string())
    }

    fn push(
        &mut self,
        path: &str,
        label: &str,
        kind: &str,
        display: String,
        start: usize,
        end: usize,
    ) {
        if self.values.len() >= MAX_EDITABLE_VALUES {
            return;
        }
        self.values.push(EditableValue {
            id: format!("json:{start}:{end}"),
            path: path.to_string(),
            label: label.to_string(),
            kind: kind.to_string(),
            display_value: display,
            raw_value: self.source[start..end].to_string(),
            start_byte: start as u64,
            end_byte: end as u64,
        });
    }

    fn skip_ws(&mut self) {
        while matches!(
            self.bytes.get(self.cursor),
            Some(b' ' | b'\n' | b'\r' | b'\t')
        ) {
            self.cursor += 1;
        }
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.bytes.get(self.cursor) == Some(&expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
}

fn extract_json_values(source: &str) -> Result<Vec<EditableValue>, String> {
    serde_json::from_str::<serde_json::Value>(source)
        .map_err(|error| format!("Values unavailable: invalid JSON ({error})."))?;
    JsonScanner::new(source).scan()
}

fn extract_toml_values(source: &str) -> Result<Vec<EditableValue>, String> {
    let document = toml_edit::ImDocument::parse(source)
        .map_err(|error| format!("Values unavailable: invalid TOML ({error})."))?;
    let mut values = Vec::new();
    collect_toml_item(document.as_item(), source, "$", "Root", &mut values);
    Ok(values)
}

fn collect_toml_item(
    item: &TomlItem,
    source: &str,
    path: &str,
    label: &str,
    values: &mut Vec<EditableValue>,
) {
    if values.len() >= MAX_EDITABLE_VALUES {
        return;
    }
    match item {
        TomlItem::Value(value) => collect_toml_value(value, source, path, label, values),
        TomlItem::Table(table) => {
            for (key, child) in table.iter() {
                collect_toml_item(child, source, &format!("{path}.{key}"), key, values);
            }
        }
        TomlItem::ArrayOfTables(tables) => {
            for (index, table) in tables.iter().enumerate() {
                collect_toml_item(
                    &TomlItem::Table(table.clone()),
                    source,
                    &format!("{path}[{index}]"),
                    &format!("Item {}", index + 1),
                    values,
                );
            }
        }
        TomlItem::None => {}
    }
}

fn collect_toml_value(
    value: &TomlValue,
    source: &str,
    path: &str,
    label: &str,
    values: &mut Vec<EditableValue>,
) {
    match value {
        TomlValue::Array(array) => {
            for (index, child) in array.iter().enumerate() {
                collect_toml_value(
                    child,
                    source,
                    &format!("{path}[{index}]"),
                    &format!("Item {}", index + 1),
                    values,
                );
            }
        }
        TomlValue::InlineTable(table) => {
            for (key, child) in table.iter() {
                collect_toml_value(child, source, &format!("{path}.{key}"), key, values);
            }
        }
        TomlValue::String(_)
        | TomlValue::Integer(_)
        | TomlValue::Float(_)
        | TomlValue::Boolean(_) => {
            let Some(span) = value.span() else { return };
            let Some(raw) = source.get(span.clone()) else {
                return;
            };
            let (kind, display) = match value {
                TomlValue::String(value) => ("string", value.value().clone()),
                TomlValue::Integer(value) => ("number", value.value().to_string()),
                TomlValue::Float(value) => ("number", value.value().to_string()),
                TomlValue::Boolean(value) => ("boolean", value.value().to_string()),
                _ => return,
            };
            if values.len() < MAX_EDITABLE_VALUES {
                values.push(EditableValue {
                    id: format!("toml:{}:{}", span.start, span.end),
                    path: path.to_string(),
                    label: label.to_string(),
                    kind: kind.to_string(),
                    display_value: display,
                    raw_value: raw.to_string(),
                    start_byte: span.start as u64,
                    end_byte: span.end as u64,
                });
            }
        }
        TomlValue::Datetime(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(value: &EditableValue, hash: &str, new_value: &str) -> ValueEditRequest {
        ValueEditRequest {
            value_id: value.id.clone(),
            expected_source_hash: hash.to_string(),
            expected_raw_value: value.raw_value.clone(),
            new_value: new_value.to_string(),
        }
    }

    #[test]
    fn json_edit_changes_only_the_selected_scalar_and_remains_valid() {
        let source =
            "{\n  \"title\": \"Vibe \\\"lab\\\"\",\n  \"enabled\": false,\n  \"count\": 3\n}\n";
        let set = extract_values(4, "settings.json", source).unwrap();
        let value = set
            .values
            .iter()
            .find(|value| value.path == "$/title")
            .unwrap();
        let (edited, changed) = apply_value_to_source(
            source,
            &set,
            &request(value, &set.source_hash, "Code Hangar"),
        )
        .unwrap();
        assert!(edited.contains("\"title\": \"Code Hangar\""));
        assert!(edited.contains("\"enabled\": false"));
        assert_eq!(changed.display_value, "Code Hangar");
        serde_json::from_str::<serde_json::Value>(&edited).unwrap();
    }

    #[test]
    fn minified_json_is_refused_but_short_one_line_config_is_accepted() {
        let compact = r#"{"enabled":true,"retries":3}"#;
        let values = extract_values(5, "settings.json", compact).unwrap();
        assert_eq!(values.values.len(), 2);

        let entries = (0..80)
            .map(|index| format!(r#""key_{index}":{index}"#))
            .collect::<Vec<_>>()
            .join(",");
        let minified = format!("{{{entries}}}");
        assert!(minified.len() >= MINIFIED_LINE_BYTES);
        let error = extract_values(5, "bundle.json", &minified).unwrap_err();
        assert!(error.contains("minified"), "{error}");
        let error = validate_content_after_edit("bundle.json", &minified).unwrap_err();
        assert!(error.contains("minified"), "{error}");
    }

    #[test]
    fn toml_edit_preserves_comments_and_changes_only_the_selected_value() {
        let source = "# keep me\ntitle = \"Demo\" # inline\nenabled = false\ncount = 3\n";
        let set = extract_values(9, "settings.toml", source).unwrap();
        let value = set
            .values
            .iter()
            .find(|value| value.path == "$.enabled")
            .unwrap();
        let (edited, _) =
            apply_value_to_source(source, &set, &request(value, &set.source_hash, "true")).unwrap();
        assert_eq!(
            edited,
            "# keep me\ntitle = \"Demo\" # inline\nenabled = true\ncount = 3\n"
        );
        toml_edit::ImDocument::parse(edited).unwrap();
    }

    #[test]
    fn stale_hash_and_stale_raw_value_are_refused() {
        let source = "{\"count\": 3}";
        let set = extract_values(1, "config.json", source).unwrap();
        let value = &set.values[0];
        let mut stale_hash = request(value, "old", "4");
        assert!(apply_value_to_source(source, &set, &stale_hash)
            .unwrap_err()
            .contains("changed on disk"));
        stale_hash.expected_source_hash = set.source_hash.clone();
        stale_hash.expected_raw_value = "2".to_string();
        assert!(apply_value_to_source(source, &set, &stale_hash)
            .unwrap_err()
            .contains("value changed"));
    }

    #[test]
    fn source_values_are_exact_and_skip_module_specifiers() {
        let source = "import x from 'library';\nconst title = \"Hello\";\nconst enabled = true;\nconst delta = -12.5;\n";
        let set = extract_values(7, "screen.ts", source).unwrap();
        assert!(!set
            .values
            .iter()
            .any(|value| value.display_value == "library"));
        assert!(set
            .values
            .iter()
            .any(|value| value.display_value == "Hello"));
        assert!(set.values.iter().any(|value| value.display_value == "true"));
        let number = set
            .values
            .iter()
            .find(|value| value.display_value == "-12.5")
            .unwrap();
        let (edited, _) =
            apply_value_to_source(source, &set, &request(number, &set.source_hash, "-3.25"))
                .unwrap();
        assert!(edited.contains("const delta = -3.25;"));
        assert!(edited.contains("const title = \"Hello\";"));
    }

    #[test]
    fn minified_source_is_refused() {
        let source = (0..80)
            .map(|index| format!("const value{index}={index};"))
            .collect::<String>();
        assert!(source.len() >= MINIFIED_LINE_BYTES);
        let error = extract_values(7, "bundle.js", &source).unwrap_err();
        assert!(error.contains("minified"), "{error}");
        let error = validate_content_after_edit("bundle.js", &source).unwrap_err();
        assert!(error.contains("minified"), "{error}");
    }

    #[test]
    fn source_numbers_require_well_formed_digit_separators() {
        for valid in ["0", "-12.5", "1_000", "0xFF_A0", "6.02e23", "1e-3"] {
            assert!(safe_source_number(valid), "expected {valid} to be valid");
        }
        for invalid in [
            "1__2", "1_", "1_.2", "1._2", "1e_2", "1e+_2", "0x_FF", "0xFF__A0",
        ] {
            assert!(
                !safe_source_number(invalid),
                "expected {invalid} to be invalid"
            );
        }

        let source = "const count = 12;\n";
        let set = extract_values(11, "counter.ts", source).unwrap();
        let number = set
            .values
            .iter()
            .find(|value| value.display_value == "12")
            .unwrap();
        let error = apply_value_to_source(source, &set, &request(number, &set.source_hash, "1__2"))
            .unwrap_err();
        assert!(error.contains("plain decimal or hexadecimal"), "{error}");
    }

    #[test]
    fn css_hex_colour_is_exposed_and_validated() {
        let source = "#face { color: #00ff88; border-color: #fff; }\n";
        let set = extract_values(8, "theme.css", source).unwrap();
        let colour = set
            .values
            .iter()
            .find(|value| value.display_value == "#00ff88")
            .unwrap();
        let (edited, _) =
            apply_value_to_source(source, &set, &request(colour, &set.source_hash, "#123abc"))
                .unwrap();
        assert_eq!(edited, "#face { color: #123abc; border-color: #fff; }\n");
        assert!(!set
            .values
            .iter()
            .any(|value| value.display_value == "#face"));
        assert!(
            apply_value_to_source(source, &set, &request(colour, &set.source_hash, "red"))
                .unwrap_err()
                .contains("hex colour")
        );
    }

    #[test]
    fn source_validity_guard_refuses_unterminated_result() {
        assert!(
            validate_content_after_edit("broken.ts", "const label = \"broken;\n")
                .unwrap_err()
                .contains("not terminated")
        );
    }

    #[test]
    fn source_validity_guard_is_lexical_not_a_language_compiler() {
        validate_content_after_edit(
            "incomplete.ts",
            "const enabled = true;\nfunction pending(\n",
        )
        .unwrap();
    }

    #[test]
    fn rust_lifetimes_and_raw_strings_are_not_mistaken_for_editable_text() {
        let source = "fn borrow<'a>(value: &'a str) -> &'a str {\n    let raw = r#\"not \\\" UI\"#;\n    let label = \"Visible\";\n    label\n}\n";
        let set = extract_values(10, "screen.rs", source).unwrap();
        assert_eq!(
            set.values
                .iter()
                .filter(|value| value.kind == "string")
                .map(|value| value.display_value.as_str())
                .collect::<Vec<_>>(),
            vec!["Visible"]
        );
    }
}
