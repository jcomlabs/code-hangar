use hangar_core::normalize_path;

const CONTEXT_NAMES: &[&str] = &[
    "readme.md",
    "agents.md",
    "claude.md",
    "gemini.md",
    ".cursorrules",
    ".clinerules",
    ".aider.conf.yml",
    "contributing.md",
    ".env.example",
    "docker-compose.yml",
    "makefile",
    "taskfile.yml",
    "justfile",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "cargo.toml",
    "go.mod",
];

const SENSITIVE_NAMES: &[&str] = &[
    ".env",
    "credentials.json",
    "credential.json",
    "token.json",
    "secrets.json",
    "secret.json",
    "id_rsa",
    "id_ed25519",
];

/// Narrow, project-local containers whose contents are conventionally derived
/// from source/manifests and can be inventoried for an explicit "clean
/// regenerables" review. This is deliberately *not* the same policy as
/// [`collapse_default_for_path`]: the normal tree also collapses protected,
/// shared and ambiguous heavy folders that must never become cleanup targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegenerableContainerKind {
    DependencyTree,
    BuildOutput,
    VirtualEnvironment,
    ToolCache,
    TestEnvironment,
    FrameworkOutput,
}

impl RegenerableContainerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependencyTree => "dependency_tree",
            Self::BuildOutput => "build_output",
            Self::VirtualEnvironment => "virtual_environment",
            Self::ToolCache => "tool_cache",
            Self::TestEnvironment => "test_environment",
            Self::FrameworkOutput => "framework_output",
        }
    }
}

/// Return the narrow regenerable kind for an exact directory path. Any
/// protected/shared/global-cache component makes the whole path ineligible,
/// even when its basename would otherwise be allowlisted. Generic `build`,
/// `dist` and `.cache` names are intentionally absent because their contents
/// are not reliably derived across arbitrary projects.
pub fn regenerable_container_kind(path: &str) -> Option<RegenerableContainerKind> {
    let components = path_components(path);
    let name = components.last()?.as_str();
    if components
        .iter()
        .any(|component| is_regenerable_cleanup_forbidden_component(component))
    {
        return None;
    }
    match name {
        "node_modules" | "bower_components" => Some(RegenerableContainerKind::DependencyTree),
        "target" => Some(RegenerableContainerKind::BuildOutput),
        ".venv" | "venv" => Some(RegenerableContainerKind::VirtualEnvironment),
        "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".hypothesis"
        | ".turbo" | ".vite" | ".parcel-cache" => Some(RegenerableContainerKind::ToolCache),
        ".tox" | ".nox" => Some(RegenerableContainerKind::TestEnvironment),
        ".next" | ".nuxt" | ".svelte-kit" => Some(RegenerableContainerKind::FrameworkOutput),
        _ => None,
    }
}

/// Strong exclusions used while explicitly expanding an already-allowlisted
/// regenerable target. Encountering one below the target is fail-closed: the
/// scanner records partial evidence and never descends into it.
pub fn is_regenerable_cleanup_forbidden_path(path: &str) -> bool {
    path_components(path)
        .iter()
        .any(|component| is_regenerable_cleanup_forbidden_component(component))
}

fn is_regenerable_cleanup_forbidden_component(component: &str) -> bool {
    matches!(
        component,
        ".git"
            | ".ssh"
            | "vendor"
            | "shared"
            | "global"
            | ".cache"
            | ".cargo"
            | ".rustup"
            | "registry"
            | "site-packages"
            | ".pnpm"
            | ".gradle"
    )
}

pub fn is_markdown_path(path: &str) -> bool {
    normalize_path(path).to_ascii_lowercase().ends_with(".md")
}

pub fn is_context_path(path: &str) -> bool {
    // Vendored, cache and dependency trees (node_modules, cargo registry, etc.)
    // are full of README.md and config files that are not project context.
    // They must never be surfaced as "Recommended reading" or indexed as context.
    if is_heavy_or_protected_container_path(path) {
        return false;
    }
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    (CONTEXT_NAMES.contains(&name) && path_depth(&normalized) <= 2)
        || normalized.starts_with("docs/")
        || normalized.contains("/docs/")
        || normalized.starts_with("prompts/")
        || normalized.contains("/prompts/")
        || normalized.starts_with(".cursor/rules/")
        || normalized.contains("/.cursor/rules/")
        || (is_markdown_path(&normalized)
            && (normalized.starts_with(".claude/") || normalized.contains("/.claude/")))
        || (is_markdown_path(&normalized)
            && (normalized.starts_with("commands/")
                || normalized.contains("/commands/")
                || normalized.starts_with("instructions/")
                || normalized.contains("/instructions/")))
        // Any top-level Markdown file is project context. A root-level .md is a place the
        // author deliberately put a document — README, SECURITY_INVARIANTS, a design note, a
        // prompt file. Surveying the real project corpus, root .md are overwhelmingly genuine
        // docs; an earlier attempt to gate on a naming convention (an uppercase letter in the
        // name) silently missed whole projects whose docs are lowercase and that have no
        // README. The heavy/vendored guard above already excludes cache/dependency roots.
        // (path_depth <= 1 == a file directly in the project root.)
        || (is_markdown_path(&normalized) && path_depth(&normalized) <= 1)
}

pub fn context_priority(path: &str) -> i64 {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    match name {
        "readme.md" => -100,
        "agents.md" => -95,
        "claude.md" => -90,
        "gemini.md" => -89,
        ".cursorrules" => -85,
        ".clinerules" => -84,
        ".env.example" => -75,
        "package.json" | "pyproject.toml" | "requirements.txt" | "cargo.toml" | "go.mod" => -55,
        _ if normalized.starts_with("docs/") || normalized.contains("/docs/") => -70,
        _ if normalized.starts_with("prompts/") || normalized.contains("/prompts/") => -65,
        _ if normalized.starts_with(".cursor/rules/") || normalized.contains("/.cursor/rules/") => {
            -80
        }
        _ if normalized.starts_with(".claude/") || normalized.contains("/.claude/") => -78,
        _ if normalized.starts_with("commands/")
            || normalized.contains("/commands/")
            || normalized.starts_with("instructions/")
            || normalized.contains("/instructions/") =>
        {
            -62
        }
        _ if is_markdown_path(path) => -40,
        _ => 0,
    }
}

pub fn is_sensitive_path(path: &str) -> bool {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    SENSITIVE_NAMES.contains(&name)
        || name.contains("credential")
        || name.contains("secret")
        || name.contains("token")
        || normalized.contains("/.ssh/")
}

pub fn protected_level_for_path(path: &str) -> Option<String> {
    let normalized = normalize_path(path).to_ascii_lowercase();
    if normalized.starts_with(".git/")
        || normalized.contains("/.git/")
        || normalized == ".git"
        || normalized.ends_with("/.git")
    {
        return Some("no_preview".to_string());
    }
    if normalized.starts_with(".ssh/")
        || normalized.contains("/.ssh/")
        || normalized == ".ssh"
        || normalized.ends_with("/.ssh")
    {
        return Some("no_preview".to_string());
    }
    if is_sensitive_path(path) {
        return Some("no_preview".to_string());
    }
    None
}

pub fn should_index_body(path: &str) -> bool {
    is_context_path(path)
        && !is_sensitive_path(path)
        && protected_level_for_path(path).is_none()
        && !is_heavy_or_protected_container_path(path)
}

pub fn collapse_default_for_path(path: &str) -> bool {
    path_components(path).iter().any(|part| {
        matches!(
            part.as_str(),
            ".git"
                | ".ssh"
                | "node_modules"
                | ".venv"
                | "venv"
                | "target"
                | "dist"
                | "build"
                | ".cache"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".ruff_cache"
                | ".hypothesis"
                | ".tox"
                | ".nox"
                | ".turbo"
                | ".vite"
                | ".parcel-cache"
                | ".next"
                | ".nuxt"
                | ".svelte-kit"
                | "__pycache__"
                | "registry"
                | "vendor"
                | ".cargo"
                | ".rustup"
                | "site-packages"
                | ".pnpm"
                | "bower_components"
                | ".gradle"
        )
    })
}

pub fn is_heavy_or_protected_container_path(path: &str) -> bool {
    collapse_default_for_path(path)
}

pub fn is_strong_protected_path(path: &str) -> bool {
    path_components(path).iter().any(|part| part == ".ssh")
}

fn path_components(path: &str) -> Vec<String> {
    normalize_path(path)
        .to_ascii_lowercase()
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn path_depth(normalized_path: &str) -> usize {
    normalized_path
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prioritises_context_files() {
        assert!(context_priority("README.md") < context_priority("src/main.ts"));
        assert!(is_context_path("docs/overview.md"));
        assert!(is_context_path(".cursor/rules/rule.md"));
        assert!(is_context_path(".claude/commands/review.md"));
        assert!(is_context_path("commands/summarise.md"));
        assert!(is_context_path("instructions/local.md"));
        // Top-level project docs are context even with non-whitelisted names.
        assert!(is_context_path("SECURITY_INVARIANTS.md"));
        assert!(is_context_path("ARCHITECTURE.md"));
        assert!(is_context_path("IMPLEMENTATION_PLAN.md"));
        // ...but a Markdown file nested below the root (and not in a context dir) is not.
        assert!(!is_context_path("notes/design.md"));
        assert!(!is_context_path("src/components/widget-readme.md"));
        // ANY top-level .md is context, regardless of case: the real project corpus shows root
        // .md are genuine docs (design notes, prompt files), and a case heuristic missed whole
        // projects whose docs are lowercase and that have no README.
        assert!(is_context_path("deep-notes.md"));
        assert!(is_context_path("todo.md"));
        assert!(is_context_path("prompt_claude_code_storyvideo.md"));
    }

    #[test]
    fn ignores_context_inside_vendored_trees() {
        // Real project context is still recognised.
        assert!(is_context_path("README.md"));
        assert!(is_context_path("crate/README.md"));
        assert!(!is_context_path("crates/a/README.md"));
        assert!(!should_index_body("crates/a/README.md"));
        assert!(!should_index_body("notes/design.md"));
        // Vendored, cache and dependency trees are not project context.
        assert!(!is_context_path(
            ".local/cargo/registry/src/foo-1.0/README.md"
        ));
        assert!(!is_context_path("node_modules/pkg/readme.md"));
        assert!(!is_context_path("project/vendor/lib/docs/guide.md"));
        assert!(!is_context_path(".venv/lib/site-packages/pkg/README.md"));
        assert!(!is_context_path("target/doc/crate/index.md"));
        assert!(!is_context_path(".pytest_cache/README.md"));
        assert!(!is_context_path(".mypy_cache/docs/README.md"));
        assert!(!is_context_path(".next/docs/README.md"));
        assert!(!should_index_body(
            ".local/cargo/registry/src/foo-1.0/README.md"
        ));
    }

    #[test]
    fn blocks_sensitive_files() {
        assert!(is_sensitive_path(".env"));
        assert!(is_sensitive_path("config/credentials.json"));
        assert!(!should_index_body(".env"));
    }

    #[test]
    fn cleanup_allowlist_is_narrower_than_opaque_container_policy() {
        assert_eq!(
            regenerable_container_kind("node_modules"),
            Some(RegenerableContainerKind::DependencyTree)
        );
        assert_eq!(
            regenerable_container_kind("crates/app/target"),
            Some(RegenerableContainerKind::BuildOutput)
        );
        assert_eq!(
            regenerable_container_kind("frontend/.next"),
            Some(RegenerableContainerKind::FrameworkOutput)
        );

        for ambiguous_or_protected in [
            ".git",
            ".ssh",
            "vendor",
            "build",
            "dist",
            ".cache",
            ".cargo/registry",
            "shared/node_modules",
            "vendor/pkg/target",
        ] {
            assert_eq!(
                regenerable_container_kind(ambiguous_or_protected),
                None,
                "{ambiguous_or_protected} must not become a cleanup target"
            );
        }

        // Normal inventory still collapses several paths that cleanup does not
        // authorize. Opaqueness is a performance/protection decision, not proof
        // that deletion is safe.
        assert!(collapse_default_for_path("vendor"));
        assert!(collapse_default_for_path("dist"));
        assert!(collapse_default_for_path(".git"));
    }

    #[test]
    fn nested_forbidden_components_fail_closed() {
        assert!(is_regenerable_cleanup_forbidden_path(
            "node_modules/pkg/.git/objects"
        ));
        assert!(is_regenerable_cleanup_forbidden_path(
            "target/shared/cache.bin"
        ));
        assert!(!is_regenerable_cleanup_forbidden_path(
            "node_modules/pkg/node_modules/child/index.js"
        ));
        assert!(!is_regenerable_cleanup_forbidden_path(
            ".next/cache/chunks/app.bin"
        ));
    }
}
