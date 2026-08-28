use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use thiserror::Error;

const MAX_WORKFLOW_REFERENCES: usize = 512;
const MAX_REFERENCE_LENGTH: usize = 1_024;
pub const MAX_SAFETENSORS_HEADER_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum GraphParseError {
    #[error("workflow JSON is invalid: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModelHeaderError {
    #[error("header is too short")]
    TooShort,
    #[error("unsupported model header")]
    Unsupported,
    #[error("header length {0} exceeds the safety cap")]
    HeaderTooLarge(u64),
    #[error("header magic is invalid")]
    InvalidMagic,
    #[error("header JSON is invalid: {0}")]
    InvalidJson(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowReference {
    pub target: String,
    pub field: Option<String>,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelHeaderSummary {
    pub format: String,
    pub confidence: String,
    pub details: Vec<String>,
}

pub fn is_model_path(path: &str) -> bool {
    let normalized = normalize_path(path);
    extension(path).is_some_and(|extension| {
        if extension == "bin" {
            return [
                "/models/",
                "/model/",
                "/checkpoints/",
                "/huggingface/",
                "/transformers/",
                "/ollama/",
                "/diffusers/",
            ]
            .iter()
            .any(|signal| normalized.contains(signal));
        }
        matches!(
            extension.as_str(),
            "safetensors" | "ckpt" | "pt" | "pth" | "gguf" | "onnx" | "bin" | "engine" | "tflite"
        )
    })
}

pub fn model_category(path: &str) -> &'static str {
    let normalized = normalize_path(path);
    if normalized.contains("/loras/") || normalized.contains("/lora/") {
        "lora"
    } else if normalized.contains("/vae/") {
        "vae"
    } else if normalized.contains("/controlnet/") || normalized.contains("/control_net/") {
        "controlnet"
    } else if normalized.contains("/upscale_models/") || normalized.contains("/upscalers/") {
        "upscaler"
    } else if normalized.contains("/embeddings/") {
        "embedding"
    } else if normalized.contains("/text_encoders/") || normalized.contains("/text_encoder/") {
        "text_encoder"
    } else if normalized.contains("/clip/") || normalized.contains("/clip_vision/") {
        "clip"
    } else if normalized.contains("/diffusion_models/") || normalized.contains("/unet/") {
        "diffusion_model"
    } else if normalized.contains("/checkpoints/") || normalized.ends_with(".ckpt") {
        "checkpoint"
    } else if normalized.ends_with(".gguf") {
        "gguf_model"
    } else {
        "model"
    }
}

pub fn is_workflow_candidate_path(path: &str) -> bool {
    let normalized = normalize_path(path);
    extension(path).as_deref() == Some("json")
        && !is_vendored_workflow_path(&normalized)
        && (normalized.ends_with(".workflow.json")
            || normalized.ends_with("/workflow.json")
            || normalized.contains("/workflows/")
            || normalized.contains("/workflow/"))
}

fn is_vendored_workflow_path(normalized: &str) -> bool {
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.iter().any(|part| {
        matches!(
            *part,
            ".git"
                | ".github"
                | "node_modules"
                | ".pnpm"
                | ".venv"
                | "venv"
                | "site-packages"
                | "dist-packages"
                | "vendor"
                | "target"
                | "__pycache__"
        )
    }) {
        return true;
    }

    parts.windows(2).any(|pair| {
        matches!(
            pair,
            [".cargo", "registry"]
                | [".cargo", "git"]
                | ["cargo", "registry"]
                | ["cargo", "git"]
                | ["pkg", "mod"]
        )
    })
}

pub fn is_cache_path(path: &str, item_kind: &str) -> bool {
    cache_category(path, item_kind).is_some()
}

pub fn cache_category(path: &str, item_kind: &str) -> Option<&'static str> {
    if item_kind != "directory" {
        return None;
    }
    let normalized = normalize_path(path);
    if normalized.contains("/.cache/huggingface")
        || normalized.ends_with("/huggingface")
        || normalized.contains("/huggingface/hub")
    {
        Some("huggingface_cache")
    } else if normalized.contains("/.cache/transformers") || normalized.ends_with("/transformers") {
        Some("transformers_cache")
    } else if normalized.ends_with("/ollama/models")
        || normalized.contains("/ollama/models/")
        || normalized.ends_with("/.ollama/models")
        || normalized.contains("/.ollama/models/")
    {
        Some("ollama_model_cache")
    } else if normalized.contains("/.cache/torch")
        || normalized.contains("/torch/hub")
        || normalized.contains("/torch_extensions")
    {
        Some("torch_cache")
    } else if normalized.contains("/.cache/pip") || normalized.contains("/pip/cache") {
        Some("pip_cache")
    } else if normalized.contains("/.cache/uv") || normalized.ends_with("/uv/cache") {
        Some("uv_cache")
    } else if normalized.ends_with("/.npm")
        || normalized.contains("/npm-cache")
        || normalized.contains("/.npm/_cacache")
    {
        Some("npm_cache")
    } else if normalized.contains("/.cargo/registry") || normalized.contains("/.cargo/git") {
        Some("cargo_cache")
    } else if normalized.contains("/go/pkg/mod") {
        Some("go_module_cache")
    } else if normalized.contains("/.pnpm-store") || normalized.contains("/pnpm/store") {
        Some("pnpm_store")
    } else if normalized.ends_with("/.cache") || normalized.ends_with("/cache") {
        Some("generic_cache")
    } else {
        None
    }
}

pub fn cache_category_label(category: &str) -> &'static str {
    match category {
        "huggingface_cache" => "Hugging Face cache",
        "transformers_cache" => "Transformers cache",
        "ollama_model_cache" => "Ollama model cache",
        "torch_cache" => "PyTorch/torch hub cache",
        "pip_cache" => "pip download cache",
        "uv_cache" => "uv package cache",
        "npm_cache" => "npm package cache",
        "cargo_cache" => "Cargo registry cache",
        "go_module_cache" => "Go module cache",
        "pnpm_store" => "pnpm content store",
        "generic_cache" => "Generic local cache",
        _ => "Local cache",
    }
}

/// Caches that are shared machine-wide across tools and projects, so their bytes
/// must NOT be attributed as owned/recoverable by any single project (Gate 2).
pub fn cache_category_is_shared_by_default(category: &str) -> bool {
    matches!(
        category,
        "huggingface_cache"
            | "transformers_cache"
            | "ollama_model_cache"
            | "torch_cache"
            | "pip_cache"
            | "uv_cache"
            | "npm_cache"
            | "cargo_cache"
            | "go_module_cache"
            | "pnpm_store"
    )
}

pub fn extract_workflow_model_references(
    bytes: &[u8],
) -> Result<Vec<WorkflowReference>, GraphParseError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let mut references = Vec::new();
    let mut seen = HashSet::new();
    collect_references(&value, None, &mut references, &mut seen);
    Ok(references)
}

pub fn safetensors_header_len(prefix: &[u8]) -> Result<u64, ModelHeaderError> {
    let bytes: [u8; 8] = prefix
        .get(..8)
        .ok_or(ModelHeaderError::TooShort)?
        .try_into()
        .map_err(|_| ModelHeaderError::TooShort)?;
    let length = u64::from_le_bytes(bytes);
    if length > MAX_SAFETENSORS_HEADER_BYTES {
        return Err(ModelHeaderError::HeaderTooLarge(length));
    }
    Ok(length)
}

/// Bound + sanitize an untrusted model-header string value for display: drop control
/// characters, collapse whitespace, and cap length so a crafted header can't inject
/// control sequences or huge strings into the graph node details.
fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}

/// Human-readable parameter count: 7.0B / 350M / 12K / 6.
fn format_param_count(count: u64) -> String {
    if count >= 1_000_000_000 {
        format!("{:.1}B", count as f64 / 1_000_000_000.0)
    } else if count >= 1_000_000 {
        format!("{:.0}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.0}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Map a GGUF `general.file_type` enum to a human label (common llama.cpp quantizations); unknown
/// values fall back to `type N` rather than guessing.
fn gguf_file_type_label(file_type: u32) -> String {
    match file_type {
        0 => "F32".to_string(),
        1 => "F16".to_string(),
        2 => "Q4_0".to_string(),
        3 => "Q4_1".to_string(),
        7 => "Q8_0".to_string(),
        8 => "Q5_0".to_string(),
        9 => "Q5_1".to_string(),
        10 => "Q2_K".to_string(),
        11 => "Q3_K_S".to_string(),
        12 => "Q3_K_M".to_string(),
        13 => "Q3_K_L".to_string(),
        14 => "Q4_K_S".to_string(),
        15 => "Q4_K_M".to_string(),
        16 => "Q5_K_S".to_string(),
        17 => "Q5_K_M".to_string(),
        18 => "Q6_K".to_string(),
        other => format!("type {other}"),
    }
}

pub fn summarize_safetensors_header(header: &[u8]) -> Result<ModelHeaderSummary, ModelHeaderError> {
    let value: Value = serde_json::from_slice(header)
        .map_err(|error| ModelHeaderError::InvalidJson(error.to_string()))?;
    let object = value.as_object().ok_or_else(|| {
        ModelHeaderError::InvalidJson("safetensors header is not a JSON object".to_string())
    })?;
    let tensor_count = object
        .keys()
        .filter(|key| key.as_str() != "__metadata__")
        .count();
    let mut dtypes = BTreeSet::new();
    let mut total_params: u64 = 0;
    let mut counted_any_shape = false;
    for (key, item) in object {
        if key == "__metadata__" {
            continue;
        }
        if let Some(dtype) = item.get("dtype").and_then(Value::as_str) {
            dtypes.insert(dtype.to_string());
        }
        // Total parameter count = sum over tensors of the product of their shape dims. The header
        // already carries every shape, so this needs no body bytes.
        if let Some(shape) = item.get("shape").and_then(Value::as_array) {
            if !shape.is_empty() {
                let mut params: u64 = 1;
                let mut ok = true;
                for dim in shape {
                    match dim.as_u64() {
                        Some(value) => params = params.saturating_mul(value),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    counted_any_shape = true;
                    total_params = total_params.saturating_add(params);
                }
            }
        }
    }
    let metadata = object.get("__metadata__").and_then(Value::as_object);
    let metadata_count = metadata.map_or(0, |metadata| metadata.len());
    let mut details = vec![
        "Safetensors header".to_string(),
        format_count(tensor_count as u64, "tensor", "tensors"),
    ];
    if counted_any_shape && total_params > 0 {
        details.push(format!("Parameters: {}", format_param_count(total_params)));
    }
    if !dtypes.is_empty() {
        details.push(format!(
            "Dtypes: {}",
            dtypes.into_iter().take(4).collect::<Vec<_>>().join(", ")
        ));
    }
    // Surface a couple of well-known, human-meaningful header keys (e.g. the model
    // architecture/title a LoRA or checkpoint records), bounded + sanitized since the
    // header is untrusted local file content.
    if let Some(metadata) = metadata {
        for (label, keys) in [
            (
                "Architecture",
                [
                    "modelspec.architecture",
                    "general.architecture",
                    "architecture",
                ]
                .as_slice(),
            ),
            (
                "Title",
                ["modelspec.title", "general.name", "title"].as_slice(),
            ),
            (
                "Base model",
                ["ss_base_model_version", "ss_sd_model_name"].as_slice(),
            ),
        ] {
            if let Some(value) = keys
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_str))
                .map(sanitize_header_value)
                .filter(|value| !value.is_empty())
            {
                details.push(format!("{label}: {value}"));
            }
        }
    }
    if metadata_count > 0 {
        details.push(format_count(
            metadata_count as u64,
            "metadata key",
            "metadata keys",
        ));
    }
    Ok(ModelHeaderSummary {
        format: "safetensors".to_string(),
        confidence: "High".to_string(),
        details,
    })
}

/// Number of leading GGUF key/value metadata entries scanned for the handful of
/// `general.*` string keys we surface. They appear first, so a small cap keeps the
/// walk bounded even on models with huge tokenizer arrays later in the header.
const GGUF_MAX_KV_ENTRIES: u64 = 64;

fn read_u32_le(buf: &[u8], pos: &mut usize) -> Option<u32> {
    let end = pos.checked_add(4)?;
    let value = u32::from_le_bytes(buf.get(*pos..end)?.try_into().ok()?);
    *pos = end;
    Some(value)
}

fn read_u64_le(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let end = pos.checked_add(8)?;
    let value = u64::from_le_bytes(buf.get(*pos..end)?.try_into().ok()?);
    *pos = end;
    Some(value)
}

/// A GGUF length-delimited string (length prefix + UTF-8 bytes), advancing `pos`. `wide` selects
/// the prefix width: GGUF v1 used u32 lengths; v2/v3 use u64.
fn read_gguf_string(buf: &[u8], pos: &mut usize, wide: bool) -> Option<String> {
    let len = if wide {
        read_u64_le(buf, pos)? as usize
    } else {
        read_u32_le(buf, pos)? as usize
    };
    let end = pos.checked_add(len)?;
    let bytes = buf.get(*pos..end)?;
    *pos = end;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Byte size of a fixed-width GGUF value type (not string=8 / array=9).
fn gguf_fixed_value_size(value_type: u32) -> Option<usize> {
    match value_type {
        0 | 1 | 7 => Some(1), // uint8 / int8 / bool
        2 | 3 => Some(2),     // uint16 / int16
        4..=6 => Some(4),     // uint32 / int32 / float32
        10..=12 => Some(8),   // uint64 / int64 / float64
        _ => None,
    }
}

/// The handful of `general.*` metadata values we surface for a model's identity, all read within
/// the bounded KV scan and without touching the model body.
#[derive(Default)]
struct GgufMetadata {
    architecture: Option<String>,
    name: Option<String>,
    /// `general.file_type` — the quantization enum (mapped to a label like Q4_K_M).
    file_type: Option<u32>,
    /// `general.block_count` — the transformer layer count.
    block_count: Option<u64>,
}

/// Best-effort, bounded scan of the GGUF kv metadata (after the header) for the few `general.*`
/// keys we surface (architecture/name strings + the file_type/block_count numbers). Stops at the
/// entry cap, the buffer end (the prefix is bounded), or the first array/unknown value it cannot
/// cheaply skip — by which point the `general.*` keys have already been seen.
fn extract_gguf_metadata(
    buf: &[u8],
    metadata_count: u64,
    kv_start: usize,
    wide: bool,
) -> GgufMetadata {
    let mut meta = GgufMetadata::default();
    let mut pos = kv_start;
    for _ in 0..metadata_count.min(GGUF_MAX_KV_ENTRIES) {
        let Some(key) = read_gguf_string(buf, &mut pos, wide) else {
            break;
        };
        let Some(value_type) = read_u32_le(buf, &mut pos) else {
            break;
        };
        if value_type == 8 {
            let Some(value) = read_gguf_string(buf, &mut pos, wide) else {
                break;
            };
            match key.as_str() {
                "general.architecture" => meta.architecture = Some(value),
                "general.name" => meta.name = Some(value),
                _ => {}
            }
        } else if let Some(size) = gguf_fixed_value_size(value_type) {
            // Read the few numeric general.* keys we care about; otherwise skip the value.
            match (key.as_str(), value_type) {
                ("general.file_type", 4) => match read_u32_le(buf, &mut pos) {
                    Some(value) => meta.file_type = Some(value),
                    None => break,
                },
                ("general.block_count", 4) => match read_u32_le(buf, &mut pos) {
                    Some(value) => meta.block_count = Some(value as u64),
                    None => break,
                },
                ("general.block_count", 10) => match read_u64_le(buf, &mut pos) {
                    Some(value) => meta.block_count = Some(value),
                    None => break,
                },
                _ => match pos.checked_add(size) {
                    Some(next) => pos = next,
                    None => break,
                },
            }
        } else {
            break; // array/unknown — cannot cheaply skip; general.* are already past.
        }
        if meta.architecture.is_some()
            && meta.name.is_some()
            && meta.file_type.is_some()
            && meta.block_count.is_some()
        {
            break;
        }
    }
    meta
}

pub fn summarize_gguf_header(prefix: &[u8]) -> Result<ModelHeaderSummary, ModelHeaderError> {
    if prefix.len() < 16 {
        return Err(ModelHeaderError::TooShort);
    }
    if prefix.get(..4) != Some(b"GGUF") {
        return Err(ModelHeaderError::InvalidMagic);
    }
    let version = u32::from_le_bytes(
        prefix[4..8]
            .try_into()
            .map_err(|_| ModelHeaderError::TooShort)?,
    );
    // GGUF v1 used u32 tensor/metadata counts + u32 string lengths and a 16-byte header; v2/v3
    // widened all of these to u64 with a 24-byte header. Reading the v2/v3 layout for a v1 file
    // garbles the architecture/name strings, so branch on the version.
    let wide = version >= 2;
    let (tensor_count, metadata_count, kv_start) = if wide {
        if prefix.len() < 24 {
            return Err(ModelHeaderError::TooShort);
        }
        let tc = u64::from_le_bytes(
            prefix[8..16]
                .try_into()
                .map_err(|_| ModelHeaderError::TooShort)?,
        );
        let mc = u64::from_le_bytes(
            prefix[16..24]
                .try_into()
                .map_err(|_| ModelHeaderError::TooShort)?,
        );
        (tc, mc, 24usize)
    } else {
        let tc = u32::from_le_bytes(
            prefix[8..12]
                .try_into()
                .map_err(|_| ModelHeaderError::TooShort)?,
        ) as u64;
        let mc = u32::from_le_bytes(
            prefix[12..16]
                .try_into()
                .map_err(|_| ModelHeaderError::TooShort)?,
        ) as u64;
        (tc, mc, 16usize)
    };
    let mut details = vec![format!("GGUF v{version}")];
    let meta = extract_gguf_metadata(prefix, metadata_count, kv_start, wide);
    if let Some(architecture) = meta.architecture.as_deref() {
        let value = sanitize_header_value(architecture);
        if !value.is_empty() {
            details.push(format!("Architecture: {value}"));
        }
    }
    if let Some(name) = meta.name.as_deref() {
        let value = sanitize_header_value(name);
        if !value.is_empty() {
            details.push(format!("Name: {value}"));
        }
    }
    if let Some(file_type) = meta.file_type {
        details.push(format!("Quantization: {}", gguf_file_type_label(file_type)));
    }
    if let Some(block_count) = meta.block_count {
        details.push(format!("Layers: {block_count}"));
    }
    details.push(format_count(tensor_count, "tensor", "tensors"));
    details.push(format_count(
        metadata_count,
        "metadata entry",
        "metadata entries",
    ));
    Ok(ModelHeaderSummary {
        format: "gguf".to_string(),
        confidence: "High".to_string(),
        details,
    })
}

pub fn model_header_probe_bytes(path: &str) -> Option<usize> {
    match extension(path).as_deref() {
        Some("safetensors") => Some(8),
        // Enough to cover the leading general.* kv metadata (architecture/name) that
        // appear before any large tokenizer arrays, without reading model body bytes.
        Some("gguf") => Some(256 * 1024),
        _ => None,
    }
}

fn format_count(count: u64, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn collect_references(
    value: &Value,
    field: Option<&str>,
    references: &mut Vec<WorkflowReference>,
    seen: &mut HashSet<String>,
) {
    if references.len() >= MAX_WORKFLOW_REFERENCES {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                collect_references(child, Some(key), references, seen);
                if references.len() >= MAX_WORKFLOW_REFERENCES {
                    break;
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_references(child, field, references, seen);
                if references.len() >= MAX_WORKFLOW_REFERENCES {
                    break;
                }
            }
        }
        Value::String(target) => {
            let field_is_model = field.is_some_and(is_model_reference_field);
            if !field_is_model && !is_model_path(target) {
                return;
            }
            let target = target.trim();
            if target.is_empty() || target.len() > MAX_REFERENCE_LENGTH {
                return;
            }
            let field = field.map(ToString::to_string);
            let key = format!("{}\0{}", field.as_deref().unwrap_or_default(), target);
            if seen.insert(key) {
                references.push(WorkflowReference {
                    target: target.to_string(),
                    evidence: field
                        .as_deref()
                        .map(|field| format!("{field}: {target}"))
                        .unwrap_or_else(|| target.to_string()),
                    field,
                });
            }
        }
        _ => {}
    }
}

fn is_model_reference_field(field: &str) -> bool {
    let normalized = field.to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "ckpt_name"
            | "checkpoint"
            | "checkpoint_name"
            | "lora_name"
            | "vae_name"
            | "control_net_name"
            | "controlnet_name"
            | "model_name"
            | "model_path"
            | "model_file"
            | "clip_name"
            | "clip_vision_name"
            | "unet_name"
            | "diffusion_model"
            | "text_encoder"
            | "upscale_model"
            | "upscaler_name"
            | "embedding_name"
            | "motion_model"
            | "motion_model_name"
            | "video_model"
            | "video_model_name"
            | "temporal_model"
            | "frame_interpolation_model"
            | "ipadapter_model"
            | "ipadapter_name"
    ) || normalized.ends_with("_model_name")
        || normalized.ends_with("_model_path")
}

fn extension(path: &str) -> Option<String> {
    Path::new(path.replace('\\', "/").as_str())
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

fn normalize_path(path: &str) -> String {
    format!("/{}", path.replace('\\', "/").trim_start_matches('/')).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_model_categories() {
        assert!(is_model_path("models/checkpoints/base.safetensors"));
        assert_eq!(model_category("models/loras/style.safetensors"), "lora");
        assert_eq!(model_category("models/vae/vae.pt"), "vae");
        assert_eq!(model_category("models/local.gguf"), "gguf_model");
        assert!(!is_model_path("src/model.ts"));
        assert!(!is_model_path("assets/firmware.bin"));
        assert!(is_model_path("models/weights.bin"));
    }

    #[test]
    fn recognises_only_likely_workflow_json_paths() {
        assert!(is_workflow_candidate_path(
            "user/default/workflows/video.json"
        ));
        assert!(is_workflow_candidate_path("render.workflow.json"));
        assert!(is_workflow_candidate_path("workflows/local-review.json"));
        assert!(!is_workflow_candidate_path("package.json"));
        assert!(!is_workflow_candidate_path(".github/workflows/ci.yml"));
        assert!(!is_workflow_candidate_path(
            ".github/workflows/release.yaml"
        ));
        assert!(!is_workflow_candidate_path("docs/workflows/README.md"));
        assert!(!is_workflow_candidate_path(
            ".local/cargo/registry/src/crate/workflows/test.json"
        ));
        assert!(!is_workflow_candidate_path(
            ".cargo/registry/src/crate/workflows/test.json"
        ));
        assert!(!is_workflow_candidate_path(
            ".cargo/git/checkouts/crate/workflows/test.json"
        ));
        assert!(!is_workflow_candidate_path(
            "node_modules/package/workflow/example.json"
        ));
        assert!(!is_workflow_candidate_path(
            ".venv/Lib/site-packages/tool/workflows/example.json"
        ));
        assert!(!is_workflow_candidate_path(
            ".github/workflows/release.json"
        ));
    }

    #[test]
    fn extracts_named_and_extension_model_references() {
        let json = br#"{
          "nodes": {
            "1": {"inputs": {"ckpt_name": "base.safetensors", "seed": 42}},
            "2": {"inputs": {"lora_name": "styles\\film.safetensors"}},
            "3": {"inputs": {"model_path": "local-model"}},
            "4": {"widgets_values": ["video_model.gguf", "ordinary text"]}
          }
        }"#;
        let references = extract_workflow_model_references(json).unwrap();
        assert_eq!(references.len(), 4);
        assert!(references
            .iter()
            .any(|item| item.target == "base.safetensors"));
        assert!(references
            .iter()
            .any(|item| item.target == "styles\\film.safetensors"));
        assert!(references.iter().any(|item| item.target == "local-model"));
        assert!(references
            .iter()
            .any(|item| item.target == "video_model.gguf"));
    }

    #[test]
    fn extracts_video_workflow_model_fields() {
        let references = extract_workflow_model_references(
            br#"{"inputs":{"motion_model_name":"motion-v3","video_model":"wan.gguf","clip_vision_name":"clip-vision.safetensors"}}"#,
        )
        .unwrap();
        assert_eq!(references.len(), 3);
        assert!(references.iter().any(|item| item.target == "motion-v3"));
        assert!(references.iter().any(|item| item.target == "wan.gguf"));
    }

    #[test]
    fn ignores_arbitrary_json_strings() {
        let references = extract_workflow_model_references(
            br#"{"name":"demo","description":"ordinary text","path":"README.md"}"#,
        )
        .unwrap();
        assert!(references.is_empty());
    }

    #[test]
    fn summarizes_safetensors_header_without_body_bytes() {
        let header = br#"{
          "__metadata__": {"format": "pt", "source": "fixture"},
          "layer.weight": {"dtype": "F16", "shape": [2, 2], "data_offsets": [0, 8]},
          "layer.bias": {"dtype": "F32", "shape": [2], "data_offsets": [8, 16]}
        }"#;
        let length = (header.len() as u64).to_le_bytes();
        assert_eq!(
            safetensors_header_len(&length).unwrap(),
            header.len() as u64
        );
        let summary = summarize_safetensors_header(header).unwrap();
        assert_eq!(summary.format, "safetensors");
        assert_eq!(summary.confidence, "High");
        assert!(summary.details.contains(&"2 tensors".to_string()));
        assert!(summary.details.contains(&"Dtypes: F16, F32".to_string()));
        assert!(summary.details.contains(&"2 metadata keys".to_string()));
    }

    #[test]
    fn safetensors_header_surfaces_known_metadata_keys() {
        // Well-known header keys (architecture/title/base model) surface for the node.
        let header = br#"{
          "__metadata__": {"modelspec.architecture": "stable-diffusion-xl-base-v1", "modelspec.title": "My LoRA", "ss_base_model_version": "sdxl_base_v1.0"},
          "w": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]}
        }"#;
        let summary = summarize_safetensors_header(header).unwrap();
        assert!(summary
            .details
            .iter()
            .any(|d| d == "Architecture: stable-diffusion-xl-base-v1"));
        assert!(summary.details.iter().any(|d| d == "Title: My LoRA"));
        assert!(summary
            .details
            .iter()
            .any(|d| d == "Base model: sdxl_base_v1.0"));
    }

    #[test]
    fn safetensors_header_cap_is_enforced() {
        let too_large = (MAX_SAFETENSORS_HEADER_BYTES + 1).to_le_bytes();
        assert_eq!(
            safetensors_header_len(&too_large).unwrap_err(),
            ModelHeaderError::HeaderTooLarge(MAX_SAFETENSORS_HEADER_BYTES + 1)
        );
    }

    #[test]
    fn summarizes_gguf_fixed_header() {
        let mut header = Vec::new();
        header.extend_from_slice(b"GGUF");
        header.extend_from_slice(&3u32.to_le_bytes());
        header.extend_from_slice(&42u64.to_le_bytes());
        header.extend_from_slice(&12u64.to_le_bytes());
        let summary = summarize_gguf_header(&header).unwrap();
        assert_eq!(summary.format, "gguf");
        assert_eq!(
            summary.details,
            vec![
                "GGUF v3".to_string(),
                "42 tensors".to_string(),
                "12 metadata entries".to_string()
            ]
        );
    }

    #[test]
    fn model_header_probe_is_limited_to_supported_headers() {
        assert_eq!(model_header_probe_bytes("models/base.safetensors"), Some(8));
        assert_eq!(
            model_header_probe_bytes("models/llm.gguf"),
            Some(256 * 1024)
        );
        assert_eq!(model_header_probe_bytes("models/base.ckpt"), None);
    }

    #[test]
    fn gguf_header_surfaces_architecture_and_name() {
        fn push_str(buf: &mut Vec<u8>, value: &str) {
            buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
            buf.extend_from_slice(value.as_bytes());
        }
        fn push_kv_string(buf: &mut Vec<u8>, key: &str, value: &str) {
            push_str(buf, key);
            buf.extend_from_slice(&8u32.to_le_bytes()); // value type 8 = string
            push_str(buf, value);
        }
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes()); // version
        buf.extend_from_slice(&1u64.to_le_bytes()); // tensor_count
        buf.extend_from_slice(&2u64.to_le_bytes()); // metadata_count
        push_kv_string(&mut buf, "general.architecture", "llama");
        push_kv_string(&mut buf, "general.name", "My Model");

        let summary = summarize_gguf_header(&buf).unwrap();
        assert_eq!(summary.format, "gguf");
        assert!(
            summary.details.iter().any(|d| d == "Architecture: llama"),
            "{:?}",
            summary.details
        );
        assert!(summary.details.iter().any(|d| d == "Name: My Model"));
        assert!(summary.details.contains(&"1 tensor".to_string()));
    }

    #[test]
    fn gguf_header_surfaces_quantization_and_layers() {
        fn push_str(buf: &mut Vec<u8>, value: &str) {
            buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
            buf.extend_from_slice(value.as_bytes());
        }
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes()); // version
        buf.extend_from_slice(&1u64.to_le_bytes()); // tensor_count
        buf.extend_from_slice(&2u64.to_le_bytes()); // metadata_count
                                                    // general.file_type = 15 (Q4_K_M), value type 4 = u32.
        push_str(&mut buf, "general.file_type");
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(&15u32.to_le_bytes());
        // general.block_count = 32, value type 10 = u64.
        push_str(&mut buf, "general.block_count");
        buf.extend_from_slice(&10u32.to_le_bytes());
        buf.extend_from_slice(&32u64.to_le_bytes());

        let summary = summarize_gguf_header(&buf).unwrap();
        assert!(
            summary.details.iter().any(|d| d == "Quantization: Q4_K_M"),
            "{:?}",
            summary.details
        );
        assert!(
            summary.details.iter().any(|d| d == "Layers: 32"),
            "{:?}",
            summary.details
        );
    }

    #[test]
    fn safetensors_header_reports_total_parameters() {
        // Two tensors of shape [2,3] (6) and [4] (4) → 10 total params, derived from the header
        // alone (no body bytes).
        let header = br#"{
          "a": {"dtype": "F16", "shape": [2, 3], "data_offsets": [0, 12]},
          "b": {"dtype": "F16", "shape": [4], "data_offsets": [12, 20]}
        }"#;
        let summary = summarize_safetensors_header(header).unwrap();
        assert!(
            summary.details.iter().any(|d| d == "Parameters: 10"),
            "{:?}",
            summary.details
        );
    }

    #[test]
    fn classifies_shared_cache_categories() {
        assert_eq!(
            cache_category(".cache/huggingface/hub", "directory"),
            Some("huggingface_cache")
        );
        assert_eq!(
            cache_category("C:/Users/me/.cache/transformers", "directory"),
            Some("transformers_cache")
        );
        assert_eq!(
            cache_category("C:/Users/me/.ollama/models", "directory"),
            Some("ollama_model_cache")
        );
        assert_eq!(
            cache_category("project/cache", "directory"),
            Some("generic_cache")
        );
        assert_eq!(cache_category("project/cache.bin", "file"), None);
        assert!(cache_category_is_shared_by_default("huggingface_cache"));
        assert!(!cache_category_is_shared_by_default("generic_cache"));

        // Widened global tool caches (Gate 2: never owned by one project).
        for (path, expected) in [
            ("C:/Users/me/.cache/torch", "torch_cache"),
            ("C:/Users/me/AppData/Local/torch/hub", "torch_cache"),
            ("C:/Users/me/.cache/pip", "pip_cache"),
            ("C:/Users/me/AppData/Local/pip/cache", "pip_cache"),
            ("C:/Users/me/.cache/uv", "uv_cache"),
            ("C:/Users/me/.npm", "npm_cache"),
            ("C:/Users/me/.cargo/registry", "cargo_cache"),
            ("C:/Users/me/go/pkg/mod", "go_module_cache"),
            ("C:/Users/me/.local/share/pnpm/store", "pnpm_store"),
        ] {
            assert_eq!(cache_category(path, "directory"), Some(expected), "{path}");
            assert!(
                cache_category_is_shared_by_default(expected),
                "{expected} must be shared-by-default"
            );
            assert_ne!(cache_category_label(expected), "Local cache");
        }
    }
}
