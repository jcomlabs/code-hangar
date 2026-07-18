use hangar_core::{NavItem, QuickOpenResult};

pub fn build_tree(mut items: Vec<NavItem>) -> Vec<NavItem> {
    items.sort_by(|left, right| {
        left.parent_nav_id
            .cmp(&right.parent_nav_id)
            .then(left.priority.cmp(&right.priority))
            .then(
                left.display_name
                    .to_ascii_lowercase()
                    .cmp(&right.display_name.to_ascii_lowercase()),
            )
    });

    let mut roots = Vec::new();
    let mut pending = items;
    let mut changed = true;

    while changed && !pending.is_empty() {
        changed = false;
        let mut next = Vec::new();
        for mut item in pending {
            if let Some(parent_id) = item.parent_nav_id {
                if attach_child(&mut roots, parent_id, item.clone()) {
                    changed = true;
                } else {
                    next.push(item);
                }
            } else {
                item.children = Vec::new();
                roots.push(item);
                changed = true;
            }
        }
        pending = next;
    }

    roots
}

fn attach_child(items: &mut [NavItem], parent_id: i64, mut child: NavItem) -> bool {
    for item in items {
        if item.id == parent_id {
            child.children = Vec::new();
            item.children.push(child);
            item.children.sort_by(|left, right| {
                left.priority.cmp(&right.priority).then(
                    left.display_name
                        .to_ascii_lowercase()
                        .cmp(&right.display_name.to_ascii_lowercase()),
                )
            });
            return true;
        }
        if attach_child(&mut item.children, parent_id, child.clone()) {
            return true;
        }
    }
    false
}

pub fn score_quick_open(label: &str, path: &str, query: &str) -> Option<i64> {
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() {
        return Some(1);
    }
    let label_lower = label.to_ascii_lowercase();
    let path_lower = path.to_ascii_lowercase();
    let label_stem = label_match_stem(&label_lower);
    let base_score = if label_lower == normalized_query || label_stem == normalized_query {
        Some(100)
    } else if label_lower.starts_with(&normalized_query) {
        Some(85)
    } else if label_lower.contains(&normalized_query) {
        Some(75)
    } else if path_lower.contains(&normalized_query) {
        Some(50)
    } else {
        fuzzy_contains(&path_lower, &normalized_query).then_some(25)
    }?;
    let score = base_score + quick_open_context_bonus(&path_lower)
        - quick_open_path_penalty(&path_lower)
        - quick_open_depth_penalty(&path_lower);
    Some(score.max(1))
}

pub fn score_quick_open_with_project(
    label: &str,
    path: &str,
    project_name: &str,
    project_path: &str,
    query: &str,
) -> Option<i64> {
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() {
        return Some(1);
    }
    let tokens = normalized_query.split_whitespace().collect::<Vec<_>>();
    if tokens.len() == 1 {
        return score_quick_open(label, path, query);
    }

    let label_lower = label.to_ascii_lowercase();
    let path_lower = path.to_ascii_lowercase();
    let project_name_lower = project_name.to_ascii_lowercase();
    let project_path_lower = project_path.to_ascii_lowercase();
    let matches_file = |token: &str| label_lower.contains(token) || path_lower.contains(token);
    let matches_context = |token: &str| {
        matches_file(token)
            || project_name_lower.contains(token)
            || project_path_lower.contains(token)
    };
    if !tokens.iter().all(|token| matches_context(token)) {
        return None;
    }

    let file_tokens = tokens
        .iter()
        .copied()
        .filter(|token| matches_file(token))
        .collect::<Vec<_>>();
    if file_tokens.is_empty() {
        return None;
    }
    let best_file_score = file_tokens
        .iter()
        .filter_map(|token| score_quick_open(label, path, token))
        .max()?;
    let additional_file_terms = file_tokens.len().saturating_sub(1) as i64;
    let project_terms = tokens.len().saturating_sub(file_tokens.len()) as i64;
    Some(best_file_score + additional_file_terms * 4 + project_terms * 8)
}

pub fn sort_quick_open(mut results: Vec<QuickOpenResult>) -> Vec<QuickOpenResult> {
    results.sort_by(|left, right| {
        right.score.cmp(&left.score).then(
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase()),
        )
    });
    results
}

fn fuzzy_contains(value: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut chars = query.chars();
    let Some(mut needle) = chars.next() else {
        return true;
    };
    for ch in value.chars() {
        if ch == needle {
            if let Some(next) = chars.next() {
                needle = next;
            } else {
                return true;
            }
        }
    }
    false
}

fn label_match_stem(label_lower: &str) -> &str {
    [".markdown", ".md", ".mdx", ".txt"]
        .iter()
        .find_map(|suffix| label_lower.strip_suffix(suffix))
        .unwrap_or(label_lower)
}

fn quick_open_context_bonus(path_lower: &str) -> i64 {
    let file_name = path_segments(path_lower).last().copied().unwrap_or("");
    match file_name {
        "readme.md" | "readme" | "agents.md" | "claude.md" | "gemini.md" => 12,
        _ => 0,
    }
}

fn quick_open_depth_penalty(path_lower: &str) -> i64 {
    let depth = path_segments(path_lower).len().saturating_sub(1) as i64;
    (depth * 4).min(32)
}

fn quick_open_path_penalty(path_lower: &str) -> i64 {
    let segments = path_segments(path_lower);
    let has_segment = |needle: &str| segments.contains(&needle);
    if [
        ".git",
        ".ssh",
        "node_modules",
        ".venv",
        "venv",
        "env",
        "target",
        "dist",
        "build",
        ".cache",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".hypothesis",
        ".tox",
        ".nox",
        ".turbo",
        ".vite",
        ".parcel-cache",
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".browser-profile",
        "__pycache__",
        "site-packages",
    ]
    .iter()
    .any(|segment| has_segment(segment))
    {
        return 110;
    }
    if has_segment_sequence(&segments, &[".local", "cargo", "registry"])
        || [
            "third_party",
            "third-party",
            "vendor",
            "vendors",
            "external",
            "deps",
        ]
        .iter()
        .any(|segment| has_segment(segment))
    {
        return 70;
    }
    0
}

fn path_segments(path_lower: &str) -> Vec<&str> {
    path_lower
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn has_segment_sequence(segments: &[&str], needle: &[&str]) -> bool {
    !needle.is_empty()
        && segments
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_exact_above_path_match() {
        assert!(
            score_quick_open("README.md", "docs/README.md", "README.md").unwrap()
                > score_quick_open("other", "docs/README.md", "readme").unwrap()
        );
    }

    #[test]
    fn combines_file_and_project_terms_in_any_order() {
        let forward = score_quick_open_with_project(
            "README.md",
            "README.md",
            "CodeHangar",
            r"C:\AI\Codex\CodeHangar",
            "README CodeHangar",
        );
        let reverse = score_quick_open_with_project(
            "README.md",
            "README.md",
            "CodeHangar",
            r"C:\AI\Codex\CodeHangar",
            "CodeHangar README",
        );

        assert!(forward.is_some());
        assert_eq!(forward, reverse);
        assert!(score_quick_open_with_project(
            "README.md",
            "README.md",
            "AnotherProject",
            r"C:\Work\AnotherProject",
            "README CodeHangar",
        )
        .is_none());
        assert!(score_quick_open_with_project(
            "README.md",
            "README.md",
            "CodeHangar",
            r"C:\AI\Codex\CodeHangar",
            "CodeHangar",
        )
        .is_none());
    }

    #[test]
    fn dependency_readmes_rank_below_project_context() {
        let project_readme = QuickOpenResult {
            node_id: 1,
            project_id: 1,
            label: "README.md".to_string(),
            path: "README.md".to_string(),
            item_kind: "file".to_string(),
            score: score_quick_open("README.md", "README.md", "README").unwrap(),
        };
        let dependency_readme = QuickOpenResult {
            node_id: 2,
            project_id: 1,
            label: "README".to_string(),
            path: ".browser-profile/Default/README".to_string(),
            item_kind: "file".to_string(),
            score: score_quick_open("README", ".browser-profile/Default/README", "README").unwrap(),
        };

        assert!(project_readme.score > dependency_readme.score);
        let sorted = sort_quick_open(vec![dependency_readme, project_readme]);
        assert_eq!(sorted[0].path, "README.md");
    }

    #[test]
    fn nested_vendor_readmes_rank_below_root_markdown() {
        let root_readme = score_quick_open("README.md", "README.md", "README").unwrap();
        let vendor_readme =
            score_quick_open("README", "third_party/openvr/src/README", "README").unwrap();
        let nested_tool_readme = score_quick_open(
            "README-dev.md",
            "servers/llama.cpp/tools/server/README-dev.md",
            "README",
        )
        .unwrap();

        assert!(root_readme > vendor_readme);
        assert!(root_readme > nested_tool_readme);
    }

    #[test]
    fn generated_cache_readmes_rank_below_root_markdown() {
        let root_readme = score_quick_open("README.md", "README.md", "README").unwrap();
        for cache_dir in [
            ".pytest_cache",
            ".mypy_cache",
            ".ruff_cache",
            ".hypothesis",
            ".tox",
            ".nox",
            ".turbo",
            ".vite",
            ".parcel-cache",
            ".next",
            ".nuxt",
            ".svelte-kit",
        ] {
            let cache_readme =
                score_quick_open("README.md", &format!("{cache_dir}/README.md"), "README").unwrap();
            assert!(root_readme > cache_readme, "cache directory: {cache_dir}");
        }
    }
}
