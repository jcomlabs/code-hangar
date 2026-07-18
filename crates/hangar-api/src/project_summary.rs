//! Local, no-network project context summary: a heuristic "what this project does"
//! card from a project's README, top-level markdown and manifest files. Read-only and
//! available in EVERY edition — it touches only the project's own files and reaches no
//! network. The connector edition can later AI-enrich it using the SAME on-disk context.

use std::fs;
use std::io::Read;
use std::path::Path;

use hangar_core::ProjectContextSummary;
use pulldown_cmark::{Event, Options, Parser};

const FILE_READ_CAP: u64 = 64 * 1024;
const EXCERPT_MAX_CHARS: usize = 400;

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn read_capped(path: &Path) -> Option<String> {
    // Consistency with every other file-read surface in the app (file_preview, FTS, and the
    // connector send-gate): never read the bytes of a sensitive or Protected-Zone'd file,
    // even in this local no-network summary. Path-based classification only — this
    // summary path carries no DB connection, so user-defined Protected-Zone globs (which need
    // the protected_zone table) are enforced on the richer preview surfaces that do; the
    // built-in name/extension/credential-dir rules are applied here. Best-effort: a skipped
    // file simply yields fewer summary fields, never an error.
    // `protected_level_for_path` already subsumes `is_sensitive_path` today; the first term is
    // kept as intentional belt-and-suspenders so this gate stays correct even if that internal
    // coupling ever changes. (Do not "simplify" by dropping the wrong half.)
    let path_str = path.to_string_lossy();
    if hangar_protect::is_sensitive_path(&path_str)
        || hangar_protect::protected_level_for_path(&path_str).is_some()
    {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.take(FILE_READ_CAP).read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    // Strip a leading UTF-8 BOM (common on Windows) so it doesn't break heading detection
    // or leak into the excerpt.
    Some(
        text.strip_prefix('\u{FEFF}')
            .map(str::to_owned)
            .unwrap_or(text),
    )
}

/// Build a [`ProjectContextSummary`] for the project rooted at `project_path`. Best-effort:
/// an unreadable directory or file simply yields fewer fields, never an error.
pub fn project_context_summary(project_path: &str) -> ProjectContextSummary {
    let root = Path::new(project_path);
    let mut summary = ProjectContextSummary::default();
    if !root.is_dir() {
        return summary;
    }

    // Shallow listing of the project root (top level only — bounded, no recursion).
    let mut top_level: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    top_level.push(name.to_string());
                }
            }
        }
    }
    let has = |name: &str| top_level.iter().any(|file| file.eq_ignore_ascii_case(name));

    let mut kinds: Vec<String> = Vec::new();
    let mut manifests: Vec<String> = Vec::new();
    let mut runs: Vec<String> = Vec::new();

    if has("package.json") {
        push_unique(&mut manifests, "package.json");
        push_unique(&mut kinds, "Node.js");
        if let Some(text) = read_capped(&root.join("package.json")) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(scripts) = value.get("scripts").and_then(|s| s.as_object()) {
                    for key in ["dev", "start", "build", "test"] {
                        if scripts.contains_key(key) {
                            runs.push(format!("npm run {key}"));
                        }
                    }
                }
            }
        }
    }
    if has("Cargo.toml") {
        push_unique(&mut manifests, "Cargo.toml");
        push_unique(&mut kinds, "Rust");
        // A VIRTUAL workspace root (a `[workspace]` table with no `[package]`) has no default
        // binary, so a bare `cargo run` there errors "could not determine which binary to run".
        // Suggest `cargo run -p <member>` instead — naming a real member when one is cheaply
        // known from the manifest, else a clear placeholder — rather than a command that fails.
        match read_capped(&root.join("Cargo.toml"))
            .as_deref()
            .map(|manifest| cargo_run_hint(root, manifest))
        {
            Some(hint) => runs.push(hint),
            // Manifest unreadable (e.g. gated/oversized): fall back to the common single-package
            // case rather than omitting a hint entirely.
            None => runs.push("cargo run".to_string()),
        }
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        for manifest in ["pyproject.toml", "requirements.txt", "setup.py"] {
            if has(manifest) {
                push_unique(&mut manifests, manifest);
            }
        }
        push_unique(&mut kinds, "Python");
        // Python projects had NO run hint. Best-effort and clearly heuristic: prefer an obvious
        // top-level entrypoint (Django's `manage.py` has a distinct invocation), else a generic
        // module/script hint the reader can adapt.
        runs.push(python_run_hint(&has));
    }
    if has("go.mod") {
        push_unique(&mut manifests, "go.mod");
        push_unique(&mut kinds, "Go");
        runs.push("go run .".to_string());
    }
    if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") {
        push_unique(&mut kinds, "Java/JVM");
    }
    if has("Gemfile") {
        push_unique(&mut manifests, "Gemfile");
        push_unique(&mut kinds, "Ruby");
    }
    if has("composer.json") {
        push_unique(&mut manifests, "composer.json");
        push_unique(&mut kinds, "PHP");
    }
    if has("Makefile") {
        runs.push("make".to_string());
    }

    // Top-level markdown, README first.
    let mut markdown: Vec<String> = top_level
        .iter()
        .filter(|file| file.to_ascii_lowercase().ends_with(".md"))
        .cloned()
        .collect();
    markdown.sort_by_key(|file| {
        if file.to_ascii_lowercase().starts_with("readme") {
            0
        } else {
            1
        }
    });

    // README title + first meaningful paragraph.
    if let Some(readme) = markdown
        .iter()
        .find(|file| file.to_ascii_lowercase().starts_with("readme"))
    {
        if let Some(text) = read_capped(&root.join(readme)) {
            let (title, excerpt) = readme_title_and_excerpt(&text);
            summary.readme_title = title;
            summary.readme_excerpt = excerpt;
        }
    }

    summary.kinds = kinds;
    summary.run_commands = runs;
    summary.manifest_files = manifests;
    summary.markdown_files = markdown;
    summary
}

/// The `cargo run` hint for a Cargo project, derived from its root `Cargo.toml` text. A VIRTUAL
/// workspace (a `[workspace]` table and NO `[package]` table) cannot be `cargo run` bare — cargo
/// errors "could not determine which binary to run" — so this returns `cargo run -p <member>`,
/// substituting the first workspace member when one is cheaply parseable from the manifest, else a
/// `<member>` placeholder the reader fills in. A normal single-package (or package-and-workspace)
/// manifest keeps the plain `cargo run`. Deliberately a light line-scan, not a full TOML parse:
/// this is a best-effort heuristic hint, and the manifest was already read (capped) for the summary.
fn cargo_run_hint(root: &Path, manifest: &str) -> String {
    let has_workspace = manifest_has_table(manifest, "workspace");
    let has_package = manifest_has_table(manifest, "package");
    if has_workspace && !has_package {
        match first_workspace_member(manifest) {
            Some(member) => {
                let package = workspace_member_package_name(root, &member)
                    .or_else(|| workspace_member_dir_name(&member));
                match package {
                    Some(package) => format!("cargo run -p {package}"),
                    None => "cargo run -p <member>".to_string(),
                }
            }
            None => "cargo run -p <member>".to_string(),
        }
    } else {
        "cargo run".to_string()
    }
}

/// Whether a TOML `[name]` (or `[name.sub]`) table header appears at the start of any line. A
/// light scan sufficient for the workspace/package distinction; comments and quoted strings that
/// merely mention the word are not matched because a table header must begin the (trimmed) line.
fn manifest_has_table(manifest: &str, name: &str) -> bool {
    let open = format!("[{name}]");
    let dotted = format!("[{name}.");
    manifest.lines().any(|line| {
        let line = line.trim();
        line == open || line.starts_with(&dotted)
    })
}

/// The first entry of a `[workspace]` `members = [ ... ]` array, as a runnable `-p` name. Cargo's
/// `-p` takes a package name, but a member is a path (often `crates/foo`); its LAST path segment is
/// the package name in the overwhelmingly common case where the directory matches the crate name, so
/// that is used as a good-enough hint. Best-effort: a `members` that spans lines awkwardly, uses a
/// glob (`crates/*`), or is absent yields `None`, and the caller falls back to a placeholder.
fn first_workspace_member(manifest: &str) -> Option<String> {
    // Find the `members` key and take everything up to the closing bracket of its array (which may
    // be on a later line), so a multi-line `members = [\n  "a",\n  "b",\n]` is handled.
    let start = manifest.find("members")?;
    let after_eq = manifest[start..].find('=')? + start + 1;
    let open = manifest[after_eq..].find('[')? + after_eq + 1;
    let close = manifest[open..].find(']')? + open;
    let first = manifest[open..close]
        .split(',')
        .map(str::trim)
        .find(|entry| !entry.is_empty())?
        .trim_matches(['"', '\'']);
    // A globbed member (`crates/*`) is not a concrete package name — skip to the placeholder.
    if first.is_empty() || first.contains('*') {
        return None;
    }
    Some(first.to_string())
}

fn workspace_member_dir_name(member: &str) -> Option<String> {
    member
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

fn workspace_member_package_name(root: &Path, member: &str) -> Option<String> {
    let manifest = read_capped(&root.join(member).join("Cargo.toml"))?;
    package_name_from_manifest(&manifest)
}

fn package_name_from_manifest(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        let name = value
            .split('#')
            .next()
            .unwrap_or(value)
            .trim()
            .trim_matches(['"', '\'']);
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// A best-effort, clearly-heuristic run hint for a Python project. Prefers a recognizable top-level
/// entrypoint (`manage.py` is Django, with its own subcommand form; `app.py`/`main.py` run directly)
/// so the suggestion is concrete when it can be, and otherwise gives a generic module/script line the
/// reader adapts. `has` is the caller's case-insensitive top-level file check.
fn python_run_hint(has: &dyn Fn(&str) -> bool) -> String {
    if has("manage.py") {
        // Django's entrypoint is not run bare; `runserver` is its dev-server subcommand.
        "python manage.py runserver".to_string()
    } else if has("app.py") {
        "python app.py".to_string()
    } else if has("main.py") {
        "python main.py".to_string()
    } else {
        // No obvious entrypoint at the top level: a generic, adapt-me hint.
        "python -m <module>".to_string()
    }
}

/// A markdown ordered-list line like `1. ` / `2) `.
fn is_ordered_list_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    index > 0
        && bytes
            .get(index)
            .map(|b| *b == b'.' || *b == b')')
            .unwrap_or(false)
        && bytes.get(index + 1).map(|b| *b == b' ').unwrap_or(false)
}

/// A markdown line that is structure (heading/badge/html/list/table/rule), not prose.
fn is_structural_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("![")
        || line.starts_with("<!--")
        || line.starts_with('<')
        || line.starts_with('|')
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("+ ")
        || line == "-"
        || line == "*"
        || line == "+"
        || is_ordered_list_line(line)
        || (!line.is_empty() && line.chars().all(|c| c == '=' || c == '-')) // setext underline / hr
}

/// Collect the first prose paragraph, skipping code fences. When `skip_structure` is true,
/// list/table/heading lines are skipped too (preferred); a lenient second pass drops that so
/// a list-only README still yields something.
fn collect_excerpt(lines: &[&str], skip_structure: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut collecting = false;
    let mut in_fence = false;
    for raw in lines {
        let line = raw.trim();
        if in_fence {
            if line.starts_with("```") || line.starts_with("~~~") {
                in_fence = false;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            if collecting {
                break;
            }
            in_fence = true;
            continue;
        }
        if line.is_empty() {
            if collecting {
                break;
            }
            continue;
        }
        let skip = if skip_structure {
            is_structural_line(line)
        } else {
            // Lenient: still skip headings/badges/html/underlines, but allow lists/tables.
            line.starts_with('#')
                || line.starts_with("![")
                || line.starts_with("<!--")
                || line.starts_with('<')
                || (line.chars().all(|c| c == '=' || c == '-'))
        };
        if skip {
            if collecting {
                break;
            }
            continue;
        }
        collecting = true;
        out.push(line.to_string());
        if out.join(" ").chars().count() >= EXCERPT_MAX_CHARS {
            break;
        }
    }
    out
}

fn markdown_inline_text(input: &str) -> String {
    let mut plain = String::new();
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_SMART_PUNCTUATION;
    for event in Parser::new_ext(input, options) {
        match event {
            Event::Text(text) | Event::Code(text) => plain.push_str(&text),
            Event::SoftBreak | Event::HardBreak => plain.push(' '),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The README's first heading (title) and first meaningful paragraph (excerpt). Handles ATX
/// (`# Title`) and Setext (underlined) headings, skips code fences / lists / tables / badges,
/// and strips a leading BOM. Length-bounded.
fn readme_title_and_excerpt(text: &str) -> (Option<String>, Option<String>) {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();

    // Title: first non-empty line that is an ATX heading, or a line followed by a Setext
    // underline (==== / ----). `body_start` is where the excerpt search begins so the title
    // (and its underline) are never re-collected as the excerpt.
    // Skip a leading YAML/TOML front-matter block (`---` … `---` or `+++` … `+++`) so its keys
    // (e.g. `title: x`) are never mistaken for the title or the excerpt.
    let mut scan_start = 0;
    while scan_start < lines.len() && lines[scan_start].trim().is_empty() {
        scan_start += 1;
    }
    if scan_start < lines.len() {
        let fence = lines[scan_start].trim();
        if fence == "---" || fence == "+++" {
            if let Some(close) = (scan_start + 1..lines.len()).find(|&i| lines[i].trim() == fence) {
                scan_start = close + 1;
            }
        }
    }

    let mut title: Option<String> = None;
    let mut body_start = scan_start;
    let mut in_fence = false;
    let mut fence_marker = "";
    let mut index = scan_start;
    while index < lines.len() {
        let line = lines[index].trim();
        // Code-fence aware (like `collect_excerpt`): never treat a fence marker as a heading,
        // and never mistake a fenced code line of all `=`/`-` for a Setext underline.
        if in_fence {
            if line.starts_with(fence_marker) {
                in_fence = false;
            }
            index += 1;
            body_start = index;
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = true;
            fence_marker = if line.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            index += 1;
            body_start = index;
            continue;
        }
        if line.is_empty() {
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim();
            if !heading.is_empty() {
                title = Some(markdown_inline_text(heading));
            }
            body_start = index + 1;
            break;
        }
        if let Some(next) = lines.get(index + 1) {
            let underline = next.trim();
            if !underline.is_empty()
                && (underline.chars().all(|c| c == '=') || underline.chars().all(|c| c == '-'))
            {
                title = Some(markdown_inline_text(line));
                body_start = index + 2;
                break;
            }
        }
        // First content line is prose, no title — start the excerpt right here.
        body_start = index;
        break;
    }

    let body = &lines[body_start.min(lines.len())..];
    let mut excerpt_lines = collect_excerpt(body, true);
    if excerpt_lines.is_empty() {
        excerpt_lines = collect_excerpt(body, false);
    }

    let excerpt = if excerpt_lines.is_empty() {
        None
    } else {
        let mut joined = excerpt_lines.join(" ");
        if joined.chars().count() > EXCERPT_MAX_CHARS {
            joined = joined.chars().take(EXCERPT_MAX_CHARS).collect::<String>();
            joined.push('…');
        }
        let plain = markdown_inline_text(&joined);
        (!plain.is_empty()).then_some(plain)
    };
    (title, excerpt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn readme_title_skips_leading_fence_and_front_matter() {
        // A leading fenced block whose inner line is all '-' must NOT be read as a Setext
        // heading (which would make the fence marker the title); the real ATX heading wins.
        let (title, _) = readme_title_and_excerpt("```\n---\n```\n# Real Title\n\nProse here.");
        assert_eq!(title.as_deref(), Some("Real Title"));

        let (title2, _) = readme_title_and_excerpt("~~~\n====\nx = 1\n~~~\n# After Fence\n");
        assert_eq!(title2.as_deref(), Some("After Fence"));

        // YAML front matter is skipped, not surfaced as the title/excerpt.
        let (title3, excerpt3) =
            readme_title_and_excerpt("---\ntitle: meta\n---\n# Heading\n\nThe real description.");
        assert_eq!(title3.as_deref(), Some("Heading"));
        assert_eq!(excerpt3.as_deref(), Some("The real description."));
        assert!(!excerpt3.unwrap_or_default().contains("title: meta"));
    }

    #[test]
    fn summarizes_a_node_project_with_readme() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","scripts":{"dev":"vite","build":"vite build","test":"vitest"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("README.md"),
            "# My Web App\n\n![badge](x)\n\nIt does a useful thing for users.\n\nMore details below.",
        )
        .unwrap();

        let summary = project_context_summary(&dir.path().to_string_lossy());
        assert_eq!(summary.kinds, vec!["Node.js"]);
        assert_eq!(summary.readme_title.as_deref(), Some("My Web App"));
        assert_eq!(
            summary.readme_excerpt.as_deref(),
            Some("It does a useful thing for users.")
        );
        assert!(summary.run_commands.contains(&"npm run dev".to_string()));
        assert!(summary.run_commands.contains(&"npm run test".to_string()));
        assert_eq!(summary.manifest_files, vec!["package.json"]);
        assert_eq!(summary.markdown_files, vec!["README.md"]);
    }

    #[test]
    fn detects_rust_and_a_missing_dir_is_empty() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"y\"\n").unwrap();
        let summary = project_context_summary(&dir.path().to_string_lossy());
        assert_eq!(summary.kinds, vec!["Rust"]);
        assert!(summary.run_commands.contains(&"cargo run".to_string()));
        assert!(summary.readme_title.is_none());

        let empty = project_context_summary("Z:\\does\\not\\exist");
        assert_eq!(empty, ProjectContextSummary::default());
    }

    #[test]
    fn virtual_cargo_workspace_suggests_run_dash_p_not_bare_cargo_run() {
        // A `[workspace]` root with NO `[package]` cannot be `cargo run` bare (cargo errors
        // "could not determine which binary to run"): suggest a `-p <member>` form, naming the
        // first real member, and never emit the failing bare command.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\n  \"apps/desktop/src-tauri\",\n  \"crates/lib\",\n]\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("apps/desktop/src-tauri")).unwrap();
        fs::write(
            dir.path().join("apps/desktop/src-tauri/Cargo.toml"),
            "[package]\nname = \"code-hangar-desktop\"\n",
        )
        .unwrap();
        let summary = project_context_summary(&dir.path().to_string_lossy());
        assert_eq!(summary.kinds, vec!["Rust"]);
        assert!(
            summary
                .run_commands
                .contains(&"cargo run -p code-hangar-desktop".to_string()),
            "run_commands: {:?}",
            summary.run_commands
        );
        assert!(
            !summary.run_commands.contains(&"cargo run".to_string()),
            "the bare (failing) cargo run must not be suggested for a virtual workspace"
        );

        // A workspace whose members are a glob (no concrete package name) falls back to a
        // placeholder rather than guessing wrong.
        let glob_dir = tempdir().unwrap();
        fs::write(
            glob_dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        let glob_summary = project_context_summary(&glob_dir.path().to_string_lossy());
        assert!(glob_summary
            .run_commands
            .contains(&"cargo run -p <member>".to_string()));

        // A manifest that is BOTH a package and a workspace root (a common single-crate-with-members
        // layout) is runnable bare — keep the plain command.
        let hybrid_dir = tempdir().unwrap();
        fs::write(
            hybrid_dir.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\n\n[workspace]\nmembers = [\"crates/x\"]\n",
        )
        .unwrap();
        let hybrid_summary = project_context_summary(&hybrid_dir.path().to_string_lossy());
        assert!(hybrid_summary
            .run_commands
            .contains(&"cargo run".to_string()));
    }

    #[test]
    fn python_projects_get_a_heuristic_run_hint() {
        // Django's `manage.py` gets its subcommand form.
        let django = tempdir().unwrap();
        fs::write(django.path().join("requirements.txt"), "django\n").unwrap();
        fs::write(django.path().join("manage.py"), "# entrypoint\n").unwrap();
        let django_summary = project_context_summary(&django.path().to_string_lossy());
        assert_eq!(django_summary.kinds, vec!["Python"]);
        assert!(
            django_summary
                .run_commands
                .contains(&"python manage.py runserver".to_string()),
            "run_commands: {:?}",
            django_summary.run_commands
        );

        // A pyproject-only project with no obvious entrypoint gets the generic module hint.
        let generic = tempdir().unwrap();
        fs::write(
            generic.path().join("pyproject.toml"),
            "[project]\nname = \"pkg\"\n",
        )
        .unwrap();
        let generic_summary = project_context_summary(&generic.path().to_string_lossy());
        assert_eq!(generic_summary.kinds, vec!["Python"]);
        assert!(generic_summary
            .run_commands
            .contains(&"python -m <module>".to_string()));
        assert_eq!(generic_summary.manifest_files, vec!["pyproject.toml"]);

        // A `main.py` script entrypoint is run directly.
        let script = tempdir().unwrap();
        fs::write(
            script.path().join("setup.py"),
            "from setuptools import setup\n",
        )
        .unwrap();
        fs::write(script.path().join("main.py"), "print('hi')\n").unwrap();
        let script_summary = project_context_summary(&script.path().to_string_lossy());
        assert!(script_summary
            .run_commands
            .contains(&"python main.py".to_string()));
    }

    #[test]
    fn readme_parsing_handles_bom_fences_setext_and_lists() {
        // BOM + ATX heading + a leading code fence: title detected, fence not used as excerpt.
        let (title, excerpt) = readme_title_and_excerpt(
            "\u{FEFF}# My Title\n\n```bash\nnpm install\n```\n\nReal description here.",
        );
        assert_eq!(title.as_deref(), Some("My Title"));
        assert_eq!(excerpt.as_deref(), Some("Real description here."));

        // Setext underline heading.
        let (title, excerpt) = readme_title_and_excerpt("My Project\n==========\n\nWhat it does.");
        assert_eq!(title.as_deref(), Some("My Project"));
        assert_eq!(excerpt.as_deref(), Some("What it does."));

        // Leading bullet list, then prose: prose preferred.
        let (_title, excerpt) =
            readme_title_and_excerpt("# T\n\n- one\n- two\n\nThe actual summary.");
        assert_eq!(excerpt.as_deref(), Some("The actual summary."));

        // List-only README: falls back to the list rather than empty.
        let (_title, excerpt) = readme_title_and_excerpt("# T\n\n- only a list item");
        assert_eq!(excerpt.as_deref(), Some("only a list item"));
    }

    #[test]
    fn readme_excerpt_is_plain_text_not_markdown_source() {
        let (title, excerpt) = readme_title_and_excerpt(
            "# **Code Hangar**\n\n**A local-first control centre** with [safe links](docs/safety.md) and `fast` navigation.",
        );

        assert_eq!(title.as_deref(), Some("Code Hangar"));
        assert_eq!(
            excerpt.as_deref(),
            Some("A local-first control centre with safe links and fast navigation.")
        );
    }
}
