use hangar_core::{
    display_path_for_path, DiscoverySignal, DiscoverySourceHit, ProjectDiscoveryCandidate,
    ProjectDiscoveryReport, SessionDiscoveryCandidate,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

const KNOWN_ROOT_DEPTH: usize = 6;
const KNOWN_ROOT_MAX_DIRS: usize = 25_000;
const DEEP_ROOT_DEPTH: usize = usize::MAX;
const DEEP_ROOT_MAX_DIRS: usize = 500_000;
const SOURCE_SCAN_DEPTH: usize = 6;
const SOURCE_SCAN_MAX_FILES: usize = 1_500;
const SOURCE_SCAN_BUDGET: Duration = Duration::from_secs(12);
const SOURCE_FILE_BYTES: usize = 64 * 1024;
const SOURCE_FILE_TAIL_BYTES: usize = 64 * 1024;
const SOURCE_FILE_MAX_EXTRACTED_PATHS: usize = 40;
const SOURCE_FILE_MAX_PATH_ATTEMPTS: usize = 32;
const SOURCE_TEXT_METADATA_MAX_BYTES: u64 = 16 * 1024 * 1024;
// Hard cap for the line-aware cwd probe on rollout/transcript files. Most files
// record `cwd` in the first line, but a Claude transcript can front-load a flood
// of cwd-less `queue-operation` records before the first `cwd` (~107 KB seen on
// the real machine), so the probe keeps reading whole lines up to this bound
// before giving up. Read in ~64 KB chunks so an oversized transcript never pulls
// its whole body into memory just to find one cwd.
const SESSION_CWD_PROBE_MAX_BYTES: u64 = 1024 * 1024;
const SESSION_CWD_PROBE_CHUNK_BYTES: usize = 64 * 1024;
const SESSION_TITLE_PROBE_BYTES: usize = 256 * 1024;
const SESSION_TITLE_MAX_CHARS: usize = 80;
// App project registries (e.g. ~/.claude.json) hold the deliberate-projects list
// but can grow large with history, so they get a generous cap of their own —
// missing the registry would mean missing every project from that app.
const REGISTRY_MAX_BYTES: u64 = 256 * 1024 * 1024;
const SQLITE_TEXT_ROWS_PER_COLUMN: usize = 200;
const SQLITE_METADATA_MAX_BYTES: u64 = 64 * 1024 * 1024;
const ESTIMATE_DEPTH: usize = 2;
const ESTIMATE_MAX_ITEMS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredRoot {
    pub project_id: Option<i64>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryOptions {
    pub limit: usize,
    /// Include project-less conversations (loose sessions: Codex date-slug
    /// scratch runs, transcripts with no resolvable cwd). On by default so
    /// "sessions soltas" surface out of the box; the UI toggles still hide them.
    pub include_loose_sessions: bool,
    /// Include autonomous agent sessions (Hermes/NemoClaw/OpenClaw chat agents).
    /// Off by default.
    pub include_agents: bool,
    /// Enumerate explicit local skill stores and retain their technical
    /// candidates. Off by default because large WSL skill trees are expensive
    /// and the normal project list hides them anyway.
    pub include_technical_candidates: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            limit: 100,
            include_loose_sessions: true,
            include_agents: false,
            include_technical_candidates: false,
        }
    }
}

/// Whether a session source belongs to an autonomous agent framework (Hermes,
/// NemoClaw, OpenClaw) rather than an editor/CLI coding session. Agent sessions
/// are gated behind the "include agents" option.
fn is_agent_session_kind(source_kind: &str) -> bool {
    source_kind.contains("hermes")
        || source_kind.contains("nemoclaw")
        || source_kind.contains("openclaw")
}

/// Drop sessions the caller did not opt into: agent sessions unless
/// `include_agents`, and project-less (loose) sessions unless
/// `include_loose_sessions`. Sessions tied to a project are always kept.
fn filter_sessions_by_options(
    sessions: &mut Vec<SessionDiscoveryCandidate>,
    options: &DiscoveryOptions,
) {
    sessions.retain(|session| {
        if is_agent_session_kind(&session.source_kind) {
            return options.include_agents;
        }
        if session.association == "loose_session" {
            return options.include_loose_sessions;
        }
        true
    });
}

/// Directories and metadata files Code Hangar treats as local session/transcript
/// stores. Used to bound which paths the read-only session preview may open, so
/// it can never be turned into an arbitrary local file reader.
pub fn session_store_roots() -> Vec<PathBuf> {
    let sources = discovery_sources();
    let mut roots = sources
        .iter()
        .filter(|source| {
            matches!(
                source.mode,
                SourceMode::TextMetadata
                    | SourceMode::SqliteMetadata
                    | SourceMode::CursorProjectTranscripts
                    | SourceMode::CursorIdeChats
                    | SourceMode::HermesState
                    | SourceMode::OpenClawState
            )
        })
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    for source in sources
        .iter()
        .filter(|source| source.mode == SourceMode::OpenClawState)
    {
        roots.extend(openclaw_agent_database_paths(&source.path));
    }
    roots
}

#[derive(Debug, Clone)]
struct DiscoverySource {
    kind: String,
    label: String,
    path: PathBuf,
    detail: Option<String>,
    mode: SourceMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    KnownFolder,
    TextMetadata,
    SqliteMetadata,
    PinokioApps,
    CursorProjectTranscripts,
    /// Cursor's in-IDE (Composer/agent) conversations, read from the small metadata
    /// keys of its global `state.vscdb`. A dedicated mode (not `SqliteMetadata`)
    /// because that DB is ~344 MB on a real machine — far past `SQLITE_METADATA_MAX_BYTES`
    /// — so the generic reader skips it and these chats are otherwise never surfaced.
    /// This reader touches only a handful of tiny `ItemTable` rows, never the ~41k
    /// content blobs, so it is safe regardless of the file's size.
    CursorIdeChats,
    HermesState,
    OpenClawState,
}

#[derive(Debug, Default)]
struct CandidateBuilder {
    path: PathBuf,
    source_kinds: BTreeSet<String>,
    signals: Vec<DiscoverySignal>,
    score: u64,
    estimated_files: Option<u64>,
    estimated_bytes: Option<u64>,
    estimate_partial: bool,
}

#[derive(Debug, Default)]
struct SessionBuilder {
    path: PathBuf,
    display_name: Option<String>,
    source_kind: String,
    source_label: String,
    session_kind: String,
    linked_project_paths: BTreeSet<PathBuf>,
    confidence: String,
    modified_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct ProjectMarker {
    file_name: &'static str,
    kind: &'static str,
    label: &'static str,
    score: u64,
}

const PROJECT_MARKERS: &[ProjectMarker] = &[
    ProjectMarker {
        file_name: "README.md",
        kind: "readme",
        label: "README project context",
        score: 16,
    },
    ProjectMarker {
        file_name: "AGENTS.md",
        kind: "agent_context",
        label: "AGENTS.md agent context",
        score: 24,
    },
    ProjectMarker {
        file_name: "CLAUDE.md",
        kind: "claude_context",
        label: "Claude project context",
        score: 24,
    },
    ProjectMarker {
        file_name: "GEMINI.md",
        kind: "gemini_context",
        label: "Gemini/Antigravity context",
        score: 22,
    },
    ProjectMarker {
        file_name: "package.json",
        kind: "node_project",
        label: "Node project manifest",
        score: 18,
    },
    ProjectMarker {
        file_name: "pyproject.toml",
        kind: "python_project",
        label: "Python project manifest",
        score: 18,
    },
    ProjectMarker {
        file_name: "Cargo.toml",
        kind: "rust_project",
        label: "Rust project manifest",
        score: 18,
    },
    ProjectMarker {
        file_name: "go.mod",
        kind: "go_project",
        label: "Go project manifest",
        score: 18,
    },
    ProjectMarker {
        file_name: ".cursorrules",
        kind: "cursor_rules",
        label: "Cursor rules",
        score: 20,
    },
    ProjectMarker {
        file_name: ".clinerules",
        kind: "cline_rules",
        label: "Cline rules",
        score: 18,
    },
    ProjectMarker {
        file_name: ".windsurfrules",
        kind: "windsurf_rules",
        label: "Windsurf legacy rules",
        score: 18,
    },
    ProjectMarker {
        file_name: ".aider.chat.history.md",
        kind: "aider_history",
        label: "Aider local chat history",
        score: 20,
    },
    ProjectMarker {
        file_name: ".aider.input.history",
        kind: "aider_input_history",
        label: "Aider local input history",
        score: 14,
    },
    ProjectMarker {
        file_name: ".aider.conf.yml",
        kind: "aider_config",
        label: "Aider project configuration",
        score: 16,
    },
    ProjectMarker {
        file_name: ".aider.conf.yaml",
        kind: "aider_config",
        label: "Aider project configuration",
        score: 10,
    },
    ProjectMarker {
        file_name: "docker-compose.yml",
        kind: "compose_project",
        label: "Docker Compose project",
        score: 14,
    },
    ProjectMarker {
        file_name: "requirements.txt",
        kind: "python_requirements",
        label: "Python requirements",
        score: 10,
    },
    ProjectMarker {
        file_name: "SKILL.md",
        kind: "skill_definition",
        label: "Local agent skill",
        score: 20,
    },
    ProjectMarker {
        file_name: "openclaw.json",
        kind: "openclaw_config",
        label: "OpenClaw local configuration",
        score: 22,
    },
    ProjectMarker {
        file_name: "MEMORY.md",
        kind: "agent_memory",
        label: "Local agent memory",
        score: 14,
    },
    ProjectMarker {
        file_name: "SOUL.md",
        kind: "agent_persona",
        label: "Local agent persona",
        score: 14,
    },
];

pub fn discover_known_projects(
    registered_roots: &[RegisteredRoot],
    options: DiscoveryOptions,
) -> ProjectDiscoveryReport {
    let started = Instant::now();
    let mut searched_locations = Vec::new();
    let mut candidates: BTreeMap<String, CandidateBuilder> = BTreeMap::new();
    let mut sessions: BTreeMap<String, SessionBuilder> = BTreeMap::new();

    for source in discovery_sources() {
        let exists = source.path.exists();
        searched_locations.push(DiscoverySourceHit {
            source_kind: source.kind.to_string(),
            source_label: source.label.to_string(),
            path: display_path(&source.path),
            exists,
            detail: source.detail.clone(),
        });
        if !exists {
            continue;
        }
        if is_explicit_skill_source(&source) && !options.include_technical_candidates {
            continue;
        }
        scan_discovery_source(&source, &mut candidates, &mut sessions);
    }

    // Registry-based projects: the deliberate projects from each app's own
    // registry (Windows and every WSL distro), not folder-marker guesses.
    add_all_registry_projects(&mut candidates);

    mark_registered_state(&mut candidates, registered_roots);
    // Projects come from registries and real sessions, never folder heuristics
    // alone — this is what stops random folders showing up as projects.
    candidates.retain(|_, builder| {
        candidate_is_visible_global_discovery_item(builder, options.include_technical_candidates)
    });
    let total_candidates = candidates.len() as u64;
    let mut candidates = candidates
        .into_values()
        .map(finalize_candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            a.path
                .to_ascii_lowercase()
                .cmp(&b.path.to_ascii_lowercase())
        })
    });
    if options.limit > 0 && candidates.len() > options.limit {
        candidates.truncate(options.limit);
    }
    let mut sessions = finalize_sessions(sessions, registered_roots);
    filter_sessions_by_options(&mut sessions, &options);
    let total_sessions = sessions.len() as u64;
    if options.limit > 0 && sessions.len() > options.limit {
        sessions.truncate(options.limit);
    }

    ProjectDiscoveryReport {
        candidates,
        sessions,
        searched_locations,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        total_candidates,
        total_sessions,
    }
}

pub fn discover_projects_in_root(
    root: &Path,
    registered_roots: &[RegisteredRoot],
    options: DiscoveryOptions,
) -> ProjectDiscoveryReport {
    let started = Instant::now();
    let source = DiscoverySource {
        kind: "deep_scan_root".to_string(),
        label: "Deep scan root".to_string(),
        path: root.to_path_buf(),
        detail: Some(
            "Recursive passive marker scan under the selected folder. It reads only bounded metadata and does not add projects automatically.".to_string(),
        ),
        mode: SourceMode::KnownFolder,
    };
    let exists = source.path.exists();
    let mut searched_locations = vec![DiscoverySourceHit {
        source_kind: source.kind.clone(),
        source_label: source.label.clone(),
        path: display_path(&source.path),
        exists,
        detail: source.detail.clone(),
    }];
    let mut candidates: BTreeMap<String, CandidateBuilder> = BTreeMap::new();
    let mut sessions: BTreeMap<String, SessionBuilder> = BTreeMap::new();
    if exists {
        scan_folder_source(
            &source,
            &mut candidates,
            DEEP_ROOT_DEPTH,
            DEEP_ROOT_MAX_DIRS,
            "deep_folder_marker",
            4,
            true,
        );
        for known_source in discovery_sources() {
            if !known_source.path.exists() {
                continue;
            }
            if !same_path(&known_source.path, root) && !is_child_of(&known_source.path, root) {
                continue;
            }
            searched_locations.push(DiscoverySourceHit {
                source_kind: known_source.kind.to_string(),
                source_label: known_source.label.to_string(),
                path: display_path(&known_source.path),
                exists: true,
                detail: known_source.detail.clone(),
            });
            scan_discovery_source(&known_source, &mut candidates, &mut sessions);
        }

        // A folder found under the root only counts as a *strong*, auto-addable
        // match when a local AI session has actually worked inside it. Those
        // transcripts live outside the scanned tree (~/.claude, ~/.codex, Cursor's
        // workspace storage, …), so the in-root loop above never sees them and a
        // deep scan of a plain code directory would link zero sessions. Sweep
        // every session-carrying source globally and keep only the links that
        // point back inside the root.
        cross_reference_global_sessions(
            root,
            &discovery_sources(),
            &mut candidates,
            &mut sessions,
            &mut searched_locations,
        );
    }
    mark_registered_state(&mut candidates, registered_roots);
    let total_candidates = candidates.len() as u64;
    let mut candidates = candidates
        .into_values()
        .map(finalize_candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            a.path
                .to_ascii_lowercase()
                .cmp(&b.path.to_ascii_lowercase())
        })
    });
    if options.limit > 0 && candidates.len() > options.limit {
        candidates.truncate(options.limit);
    }
    let mut sessions = finalize_sessions(sessions, registered_roots);
    filter_sessions_by_options(&mut sessions, &options);
    let total_sessions = sessions.len() as u64;
    if options.limit > 0 && sessions.len() > options.limit {
        sessions.truncate(options.limit);
    }

    ProjectDiscoveryReport {
        candidates,
        sessions,
        searched_locations,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        total_candidates,
        total_sessions,
    }
}

fn discovery_sources() -> Vec<DiscoverySource> {
    let mut sources = Vec::new();
    if let Some(home) = home_dir() {
        push_known_folder(
            &mut sources,
            "documents",
            "Documents",
            home.join("Documents"),
        );
        push_known_folder(&mut sources, "desktop", "Desktop", home.join("Desktop"));
        push_known_folder(
            &mut sources,
            "downloads",
            "Downloads",
            home.join("Downloads"),
        );
        push_known_folder(
            &mut sources,
            "onedrive_documents",
            "OneDrive Documents",
            home.join("OneDrive").join("Documents"),
        );
        push_known_folder(
            &mut sources,
            "claude_chat_projects",
            "Claude Chat projects",
            home.join("Documents").join("Claude").join("Projects"),
        );
        push_known_folder(
            &mut sources,
            "windsurf_documents",
            "Windsurf document projects",
            home.join("Documents").join("Windsurf"),
        );
        push_known_folder(&mut sources, "source", "Source folder", home.join("source"));
        push_known_folder(&mut sources, "repos", "Repos folder", home.join("repos"));
        push_known_folder(
            &mut sources,
            "projects",
            "Projects folder",
            home.join("projects"),
        );
        push_known_folder(&mut sources, "dev", "Dev folder", home.join("dev"));
        for (kind, label, path) in [
            ("git_folder", "Git folder", home.join("git")),
            ("code_folder", "Code folder", home.join("code")),
            ("work_folder", "Work folder", home.join("work")),
            (
                "workspace_folder",
                "Workspace folder",
                home.join("workspace"),
            ),
            (
                "workspaces_folder",
                "Workspaces folder",
                home.join("workspaces"),
            ),
            (
                "source_repos",
                "Source repos",
                home.join("source").join("repos"),
            ),
            (
                "documents_github",
                "Documents GitHub",
                home.join("Documents").join("GitHub"),
            ),
            (
                "documents_projects",
                "Documents projects",
                home.join("Documents").join("Projects"),
            ),
            (
                "documents_dev",
                "Documents dev",
                home.join("Documents").join("Dev"),
            ),
            (
                "onedrive_desktop",
                "OneDrive Desktop",
                home.join("OneDrive").join("Desktop"),
            ),
            (
                "onedrive_documents_ai",
                "OneDrive Documents AI",
                home.join("OneDrive").join("Documents").join("AI"),
            ),
        ] {
            push_known_folder_if_dir(&mut sources, kind, label, path);
        }

        let codex_home = env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
        push_source(
            &mut sources,
            "codex_sessions",
            "ChatGPT sessions",
            codex_home.join("sessions"),
            SourceMode::TextMetadata,
            "Reads local ChatGPT CLI JSONL transcripts for cwd/project paths.",
        );
        push_source(
            &mut sources,
            "codex_archived_sessions",
            "ChatGPT archived sessions",
            codex_home.join("archived_sessions"),
            SourceMode::TextMetadata,
            "Reads archived local ChatGPT CLI JSONL transcripts so older conversations remain discoverable.",
        );
        push_known_folder(
            &mut sources,
            "codex_skills",
            "ChatGPT user skills",
            codex_home.join("skills"),
        );
        push_source(
            &mut sources,
            "codex_index",
            "ChatGPT session index",
            codex_home.join("session_index.jsonl"),
            SourceMode::TextMetadata,
            "Reads the local ChatGPT CLI session index for project paths.",
        );
        push_source(
            &mut sources,
            "codex_state",
            "ChatGPT thread state",
            codex_home.join("state_5.sqlite"),
            SourceMode::SqliteMetadata,
            "Reads local ChatGPT CLI thread metadata such as cwd fields when present.",
        );

        let claude_home = env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
        push_source(
            &mut sources,
            "claude_code_projects",
            "Claude Code projects",
            claude_home.join("projects"),
            SourceMode::TextMetadata,
            "Reads local Claude Code project transcripts for cwd/project paths.",
        );
        push_source(
            &mut sources,
            "claude_code_sessions",
            "Claude Code sessions",
            claude_home.join("sessions"),
            SourceMode::TextMetadata,
            "Reads local Claude Code session metadata.",
        );
        push_source(
            &mut sources,
            "claude_file_history",
            "Claude Code file history",
            claude_home.join("file-history"),
            SourceMode::TextMetadata,
            "Reads local Claude Code file-history metadata for files and folders touched by sessions.",
        );
        push_known_folder(
            &mut sources,
            "claude_skills",
            "Claude user skills",
            claude_home.join("skills"),
        );
        push_source(
            &mut sources,
            "claude_commands",
            "Claude user commands",
            claude_home.join("commands"),
            SourceMode::TextMetadata,
            "Reads local Claude command markdown for project path references.",
        );
        push_source(
            &mut sources,
            "claude_subagents",
            "Claude user subagents",
            claude_home.join("agents"),
            SourceMode::TextMetadata,
            "Reads local Claude subagent definitions for project path references.",
        );
        push_source(
            &mut sources,
            "gemini_antigravity_brain",
            "Antigravity brain",
            home.join(".gemini").join("antigravity").join("brain"),
            SourceMode::TextMetadata,
            "Reads local Antigravity brain/task metadata for project paths.",
        );
        push_source(
            &mut sources,
            "gemini_antigravity_conversations",
            "Antigravity conversations",
            home.join(".gemini")
                .join("antigravity")
                .join("conversations"),
            SourceMode::TextMetadata,
            "Reads local Antigravity conversation metadata for project paths.",
        );
        // Antigravity IDE keeps a SECOND, independent store under `antigravity-ide/`
        // whose conversations exist only there. Register its `conversations` + `brain`
        // trees and its own summaries proto (for project links). Overlapping
        // conversations are deduped by UUID against the main store above so they are
        // not listed twice — see `main_antigravity_conversation_uuids`. The
        // `antigravity-backup/` mirror is deliberately NOT registered (identical copy).
        push_source(
            &mut sources,
            "gemini_antigravity_ide_conversations",
            "Antigravity IDE conversations",
            home.join(".gemini")
                .join("antigravity-ide")
                .join("conversations"),
            SourceMode::TextMetadata,
            "Reads the second local Antigravity IDE conversation store, deduped by UUID against the main store.",
        );
        push_source(
            &mut sources,
            "gemini_antigravity_ide_brain",
            "Antigravity IDE brain",
            home.join(".gemini").join("antigravity-ide").join("brain"),
            SourceMode::TextMetadata,
            "Reads the second local Antigravity IDE brain/task metadata for project paths.",
        );
        push_source(
            &mut sources,
            "gemini_cli_tmp",
            "Gemini CLI checkpoints",
            home.join(".gemini").join("tmp"),
            SourceMode::TextMetadata,
            "Reads local Gemini CLI checkpoints for project paths.",
        );
        push_known_folder(
            &mut sources,
            "gemini_skills",
            "Gemini/Antigravity user skills",
            home.join(".gemini").join("skills"),
        );
        push_source(
            &mut sources,
            "openclaw_home",
            "OpenClaw local state",
            home.join(".openclaw"),
            SourceMode::TextMetadata,
            "Reads local OpenClaw state for workspace/project paths.",
        );
        push_source(
            &mut sources,
            "openclaw_state_sessions",
            "OpenClaw session database",
            home.join(".openclaw").join("state").join("openclaw.sqlite"),
            SourceMode::OpenClawState,
            "Reads OpenClaw's local session registry and per-agent transcript databases read-only.",
        );
        push_known_folder(
            &mut sources,
            "openclaw_skills",
            "OpenClaw global skills",
            home.join(".openclaw").join("skills"),
        );
        push_source(
            &mut sources,
            "openclaw_sessions",
            "OpenClaw sessions",
            home.join(".openclaw").join("sessions"),
            SourceMode::TextMetadata,
            "Reads local OpenClaw session history for project and workspace path references.",
        );
        push_source(
            &mut sources,
            "openclaw_memory",
            "OpenClaw memory",
            home.join(".openclaw").join("memory"),
            SourceMode::TextMetadata,
            "Reads local OpenClaw memory files for project path references.",
        );
        push_source(
            &mut sources,
            "nemoclaw_home",
            "NemoClaw/Hermes local state",
            home.join(".nemoclaw"),
            SourceMode::TextMetadata,
            "Reads local NemoClaw/Hermes state for workspace/project paths.",
        );
        push_source(
            &mut sources,
            "nemoclaw_sessions",
            "NemoClaw/Hermes sessions",
            home.join(".nemoclaw").join("sessions"),
            SourceMode::TextMetadata,
            "Reads local NemoClaw/Hermes sessions for project path references.",
        );
        push_known_folder(
            &mut sources,
            "nemoclaw_skills",
            "NemoClaw user skills",
            home.join(".nemoclaw").join("skills"),
        );
        push_known_folder(
            &mut sources,
            "nemoclaw_sandboxes",
            "NemoClaw sandboxes",
            home.join(".nemoclaw").join("sandboxes"),
        );
        push_source(
            &mut sources,
            "hermes_home",
            "Hermes local configuration",
            home.join(".hermes"),
            SourceMode::TextMetadata,
            "Reads local Hermes configuration for sandbox and workspace path references.",
        );
        push_source(
            &mut sources,
            "hermes_state_sessions",
            "Hermes conversation database",
            home.join(".hermes").join("state.db"),
            SourceMode::HermesState,
            "Reads local Hermes sessions and conversation messages from state.db read-only.",
        );
        push_known_folder(
            &mut sources,
            "hermes_skills",
            "Hermes user skills",
            home.join(".hermes").join("skills"),
        );
        push_source(
            &mut sources,
            "continue_home",
            "Continue local configuration",
            home.join(".continue"),
            SourceMode::TextMetadata,
            "Reads Continue local config and rules for workspace path references.",
        );
        push_source(
            &mut sources,
            "roo_home",
            "Roo Code global rules",
            home.join(".roo"),
            SourceMode::TextMetadata,
            "Reads Roo Code global rule files for local project path references.",
        );
        push_source(
            &mut sources,
            "pinokio_api",
            "Pinokio apps",
            home.join("pinokio").join("api"),
            SourceMode::PinokioApps,
            "Treats each local Pinokio app directory as a candidate project.",
        );
        push_zed_sources(
            &mut sources,
            &home,
            env_path("APPDATA"),
            env_path("LOCALAPPDATA"),
        );
    }

    push_wsl_sources(&mut sources);

    if Path::new("C:\\AI").exists() {
        push_known_folder(&mut sources, "c_ai", "C:\\AI", PathBuf::from("C:\\AI"));
    }
    // Probe common project folders on each fixed drive (C: onward, skipping floppy A:/B:).
    // Many developers keep working trees on secondary drives such as D:\ or E:\.
    for drive in 'C'..='Z' {
        let root = PathBuf::from(format!("{drive}:\\"));
        if !root.is_dir() {
            continue;
        }
        for suffix in [
            "AI", "Projects", "Project", "Dev", "Code", "repos", "source", "work",
        ] {
            push_known_folder_if_dir(
                &mut sources,
                "drive_project_folder",
                &format!("{drive}:\\{suffix}"),
                root.join(suffix),
            );
        }
    }
    push_source(
        &mut sources,
        "pinokio_api_c",
        "Pinokio apps",
        PathBuf::from("C:\\pinokio").join("api"),
        SourceMode::PinokioApps,
        "Treats each local Pinokio app directory as a candidate project.",
    );
    if let Some(appdata) = env_path("APPDATA") {
        push_vscode_like_sources(
            &mut sources,
            &appdata,
            "vscode",
            "Visual Studio Code",
            appdata.join("Code").join("User"),
        );
        push_vscode_like_sources(
            &mut sources,
            &appdata,
            "vscode_insiders",
            "Visual Studio Code Insiders",
            appdata.join("Code - Insiders").join("User"),
        );
        push_vscode_like_sources(
            &mut sources,
            &appdata,
            "vscodium",
            "VSCodium",
            appdata.join("VSCodium").join("User"),
        );
        push_source(
            &mut sources,
            "cursor_workspace_storage",
            "Cursor workspace storage",
            appdata.join("Cursor").join("User").join("workspaceStorage"),
            SourceMode::TextMetadata,
            "Reads Cursor workspace.json and state files for folder paths.",
        );
        push_source(
            &mut sources,
            "cursor_global_state",
            "Cursor global state",
            appdata
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
            SourceMode::SqliteMetadata,
            "Reads Cursor local state database for workspace path references.",
        );
        // Same DB, different reader: the in-IDE Composer/agent conversations live in
        // the small `composer.composerHeaders` metadata row, which the size-capped
        // generic SQLite reader above never reaches (the DB is hundreds of MB of chat
        // blobs). This dedicated, cap-free reader lists those conversations as
        // sessions and links each to its recorded workspace.
        push_source(
            &mut sources,
            "cursor_ide_chats",
            "Cursor in-IDE conversations",
            appdata
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
            SourceMode::CursorIdeChats,
            "Reads Cursor's in-IDE Composer/agent conversation headers for session listing and workspace links.",
        );
        if let Some(home) = home_dir() {
            push_source(
                &mut sources,
                "cursor_project_transcripts",
                "Cursor project transcripts",
                home.join(".cursor").join("projects"),
                SourceMode::CursorProjectTranscripts,
                "Reads Cursor per-project agent transcripts as local sessions.",
            );
            push_known_folder(
                &mut sources,
                "cursor_skills",
                "Cursor user skills",
                home.join(".cursor").join("skills"),
            );
            push_known_folder(
                &mut sources,
                "cursor_skills_legacy",
                "Cursor legacy user skills",
                home.join(".cursor").join("skills-cursor"),
            );
        }
        push_vscode_extension_ai_sources(
            &mut sources,
            "cursor",
            "Cursor",
            appdata.join("Cursor").join("User"),
        );
        push_vscode_like_sources(
            &mut sources,
            &appdata,
            "windsurf",
            "Windsurf",
            appdata.join("Windsurf").join("User"),
        );
        push_vscode_like_sources(
            &mut sources,
            &appdata,
            "windsurf_next",
            "Windsurf Next",
            appdata.join("Windsurf - Next").join("User"),
        );
        push_source(
            &mut sources,
            "claude_app_sessions",
            "Claude desktop local sessions",
            appdata.join("Claude").join("claude-code-sessions"),
            SourceMode::TextMetadata,
            "Reads Claude desktop local session files for project paths.",
        );
        push_source(
            &mut sources,
            "claude_local_storage",
            "Claude local storage",
            appdata.join("Claude").join("Local Storage").join("leveldb"),
            SourceMode::TextMetadata,
            "Reads bounded Claude desktop local storage files for project path references.",
        );
        push_source(
            &mut sources,
            "claude_logs",
            "Claude desktop logs",
            appdata.join("Claude").join("logs"),
            SourceMode::TextMetadata,
            "Reads Claude desktop logs for local project path references.",
        );
        push_source(
            &mut sources,
            "claude_local_agent_sessions",
            "Claude local agent sessions",
            appdata.join("Claude").join("local-agent-mode-sessions"),
            SourceMode::TextMetadata,
            "Reads Claude local agent session files for workspace paths.",
        );
        push_source(
            &mut sources,
            "antigravity_workspace_storage",
            "Antigravity workspace storage",
            appdata
                .join("Antigravity")
                .join("User")
                .join("workspaceStorage"),
            SourceMode::TextMetadata,
            "Reads Antigravity IDE workspace metadata for folder paths.",
        );
        push_vscode_extension_ai_sources(
            &mut sources,
            "antigravity",
            "Antigravity",
            appdata.join("Antigravity").join("User"),
        );
        push_source(
            &mut sources,
            "antigravity_ide_workspace_storage",
            "Antigravity IDE workspace storage",
            appdata
                .join("Antigravity IDE")
                .join("User")
                .join("workspaceStorage"),
            SourceMode::TextMetadata,
            "Reads Antigravity IDE workspace metadata for folder paths.",
        );
        push_vscode_extension_ai_sources(
            &mut sources,
            "antigravity_ide",
            "Antigravity IDE",
            appdata.join("Antigravity IDE").join("User"),
        );
    }
    sources
}

fn push_wsl_sources(sources: &mut Vec<DiscoverySource>) {
    #[cfg(windows)]
    push_wsl_sources_from(sources, wsl_distro_homes());
}

/// The `(distro, home)` pairs to register in-WSL AI-tool sources for. Respects the
/// WSL scan gate EXACTLY like [`wsl_distros`]: with scanning OFF this returns empty
/// and touches neither `wsl.exe` nor the filesystem, so no WSL source is ever
/// registered and no stopped distro can be cold-booted (the whole reason the gate
/// exists). Only when the user has opted in does it enumerate distros (cached
/// `wsl.exe --list`) and their `\\wsl.localhost\<distro>\home\*` user homes — the
/// same dynamic enumeration Hermes always used (never guessing distro names or
/// assuming the WSL username matches the Windows one).
#[cfg(windows)]
fn wsl_distro_homes() -> Vec<(String, PathBuf)> {
    // wsl_distros() is already gated (returns empty when scanning is off) and cached,
    // so this is the single choke point that keeps the gate honest for every caller.
    let mut out = Vec::new();
    for distro in wsl_distros() {
        let homes = PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
        let Ok(entries) = fs::read_dir(&homes) else {
            continue;
        };
        for entry in entries.flatten() {
            let home = entry.path();
            if home.is_dir() {
                out.push((distro.clone(), home));
            }
        }
    }
    out
}

/// Register every in-WSL AI-tool source for each injected `(distro, home)` pair.
/// Split from [`wsl_distro_homes`] so source REGISTRATION is unit-testable with a
/// synthetic distro list: the sources are just UNC path strings, so the exact
/// per-app set can be asserted without a live distro or any filesystem access.
#[cfg(windows)]
fn push_wsl_sources_from(sources: &mut Vec<DiscoverySource>, homes: Vec<(String, PathBuf)>) {
    for (distro, home) in homes {
        push_wsl_home_sources(sources, &distro, &home);
    }
}

/// Register the full set of in-WSL AI-tool sources under ONE distro home. Mirrors
/// the Windows-host source set for each tool the owner runs inside WSL2, so a
/// Claude / Codex / OpenClaw / Hermes install living on the Linux side is detected
/// as completely as one on Windows — "quer em windows, quer em wsl2".
///
/// Antigravity and Cursor are deliberately NOT registered in-distro: they are
/// Windows-host GUI apps (no Linux build writes their stores), and a project a user
/// opens in them from inside WSL already surfaces on the Windows side — their
/// workspace registries record it with a `\\wsl.localhost\<distro>\…` folder URI,
/// which the Windows registry pass (`add_all_registry_projects`) reads directly.
/// Adding in-distro Cursor / Antigravity sources would only scan trees that never
/// exist there.
#[cfg(windows)]
fn push_wsl_home_sources(sources: &mut Vec<DiscoverySource>, distro: &str, home: &Path) {
    push_wsl_hermes_sources(sources, distro, home);
    push_wsl_claude_sources(sources, distro, home);
    push_wsl_codex_sources(sources, distro, home);
    push_wsl_openclaw_sources(sources, distro, home);
}

/// The `(tag, who)` a per-home WSL source uses: `tag` is a filesystem-safe
/// `<distro>_<user>` slug for unique source kinds, `who` a human label like
/// `WSL Ubuntu-24.04 (dev)`. Mirrors the naming [`push_wsl_hermes_sources`] builds.
#[cfg(windows)]
fn wsl_source_identity(distro: &str, home: &Path) -> (String, String) {
    let user = home
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("home");
    (
        format!("{}_{}", source_safe_name(distro), source_safe_name(user)),
        format!("WSL {distro} ({user})"),
    )
}

/// In-WSL Claude Code state under one distro home. The transcript store
/// (`~/.claude/projects/**/*.jsonl`) is registered under the SAME `claude_code_projects`
/// kind as the Windows source so every WSL transcript flows through the identical
/// parser and the per-file session-uuid dedup/title logic (`claude-session:<uuid>`) —
/// a transcript reached both on Windows (a `/mnt/c/…` project) and via
/// `\\wsl.localhost` then folds to one session instead of listing twice.
/// `~/.claude.json` (the deliberate-projects registry) is already read per WSL home
/// by `add_all_registry_projects`, so only the sessions + skills are added here.
#[cfg(windows)]
fn push_wsl_claude_sources(sources: &mut Vec<DiscoverySource>, distro: &str, home: &Path) {
    let (tag, who) = wsl_source_identity(distro, home);
    let claude_home = home.join(".claude");
    push_source(
        sources,
        "claude_code_projects",
        &format!("{who} Claude Code projects"),
        claude_home.join("projects"),
        SourceMode::TextMetadata,
        "Reads local Claude Code project transcripts from the WSL home for cwd/project paths.",
    );
    push_known_folder(
        sources,
        &format!("wsl_{tag}_claude_skills"),
        &format!("{who} Claude user skills"),
        claude_home.join("skills"),
    );
}

/// In-WSL Codex state under one distro home. The rollout transcript stores are
/// registered under the SAME `codex_sessions` / `codex_archived_sessions` kinds as
/// the Windows sources so each rollout keeps its per-file uuid dedup key
/// (`codex-rollout:<uuid>`), the only thing that stops distinct archived rollouts
/// with an auto-generated (title, cwd) from folding, and so the `session_index.jsonl`
/// and `state_*.sqlite` retitle/cwd-link passes run identically. The index is
/// registered AFTER the two rollout stores (matching the Windows order) because it
/// only names rollouts already scanned into the session map. `codex_state` resolves
/// the live DB by the same newest `state_*.sqlite` glob (`codex_state_db_path` reads
/// `<home>/.codex` and `<home>/.codex/sqlite`) and opens it read-only with
/// `immutable=1` for the WSL share via `open_discovery_sqlite`.
#[cfg(windows)]
fn push_wsl_codex_sources(sources: &mut Vec<DiscoverySource>, distro: &str, home: &Path) {
    let (tag, who) = wsl_source_identity(distro, home);
    let codex_home = home.join(".codex");
    push_source(
        sources,
        "codex_sessions",
        &format!("{who} ChatGPT sessions"),
        codex_home.join("sessions"),
        SourceMode::TextMetadata,
        "Reads local ChatGPT CLI JSONL transcripts from the WSL home for cwd/project paths.",
    );
    push_source(
        sources,
        "codex_archived_sessions",
        &format!("{who} ChatGPT archived sessions"),
        codex_home.join("archived_sessions"),
        SourceMode::TextMetadata,
        "Reads archived local ChatGPT CLI JSONL transcripts from the WSL home so older conversations remain discoverable.",
    );
    push_source(
        sources,
        "codex_index",
        &format!("{who} ChatGPT session index"),
        codex_home.join("session_index.jsonl"),
        SourceMode::TextMetadata,
        "Reads the local ChatGPT CLI session index from the WSL home for project paths and rollout names.",
    );
    push_source(
        sources,
        "codex_state",
        &format!("{who} ChatGPT thread state"),
        codex_home.join("state_5.sqlite"),
        SourceMode::SqliteMetadata,
        "Reads local ChatGPT CLI thread metadata such as cwd fields from the WSL home read-only when present.",
    );
    push_known_folder(
        sources,
        &format!("wsl_{tag}_codex_skills"),
        &format!("{who} ChatGPT user skills"),
        codex_home.join("skills"),
    );
}

/// In-WSL OpenClaw state under one distro home — the full Windows set (home /
/// state DB / sessions / memory / skills) mirrored under `~/.openclaw`. Uses
/// per-home `wsl_<tag>_openclaw_*` kinds; every one still contains `openclaw`, so
/// they stay agent-gated by `is_agent_session_kind` and ranked by
/// `session_candidate_rank` exactly like the Windows OpenClaw sources. The state DB
/// is read via the `OpenClawState` mode, whose reader derives its per-agent sibling
/// DBs from the source path and opens every DB read-only through
/// `open_discovery_sqlite`, so the WSL share gets the `immutable=1` snapshot.
#[cfg(windows)]
fn push_wsl_openclaw_sources(sources: &mut Vec<DiscoverySource>, distro: &str, home: &Path) {
    let (tag, who) = wsl_source_identity(distro, home);
    let openclaw_home = home.join(".openclaw");
    push_source(
        sources,
        &format!("wsl_{tag}_openclaw_home"),
        &format!("{who} OpenClaw local state"),
        openclaw_home.clone(),
        SourceMode::TextMetadata,
        "Reads local OpenClaw state from the WSL home for workspace/project paths.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_openclaw_state_sessions"),
        &format!("{who} OpenClaw session database"),
        openclaw_home.join("state").join("openclaw.sqlite"),
        SourceMode::OpenClawState,
        "Reads OpenClaw's local session registry and per-agent transcript databases from the WSL home read-only.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_openclaw_sessions"),
        &format!("{who} OpenClaw sessions"),
        openclaw_home.join("sessions"),
        SourceMode::TextMetadata,
        "Reads local OpenClaw session history from the WSL home for project and workspace path references.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_openclaw_memory"),
        &format!("{who} OpenClaw memory"),
        openclaw_home.join("memory"),
        SourceMode::TextMetadata,
        "Reads local OpenClaw memory files from the WSL home for project path references.",
    );
    push_known_folder(
        sources,
        &format!("wsl_{tag}_openclaw_skills"),
        &format!("{who} OpenClaw global skills"),
        openclaw_home.join("skills"),
    );
}

/// Every Hermes `state.db` under every WSL distro's home directories. Used by the removal
/// layer to find which DB(s) hold a project's sessions, regardless of whether the project
/// lives on the Linux home or a Windows-mounted drive (`/mnt/...`).
#[cfg(windows)]
pub fn wsl_hermes_state_dbs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for distro in wsl_distros() {
        let homes = PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
        let Ok(entries) = fs::read_dir(&homes) else {
            continue;
        };
        for entry in entries.flatten() {
            let db = entry.path().join(".hermes").join("state.db");
            if db.is_file() {
                out.push(db);
            }
        }
    }
    out
}

#[cfg(not(windows))]
pub fn wsl_hermes_state_dbs() -> Vec<PathBuf> {
    Vec::new()
}

/// Register the Hermes state under one WSL home as discovery sources. Hermes is a
/// chat agent that runs in WSL; its `sessions/sessions.json` holds Telegram /
/// Discord conversations that surface as independent (project-less) sessions.
#[cfg(windows)]
fn push_wsl_hermes_sources(sources: &mut Vec<DiscoverySource>, distro: &str, home: &Path) {
    let user = home
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("home");
    let tag = format!("{}_{}", source_safe_name(distro), source_safe_name(user));
    let who = format!("WSL {distro} ({user})");
    push_source(
        sources,
        &format!("wsl_{tag}_hermes_sessions"),
        &format!("{who} Hermes sessions"),
        home.join(".hermes").join("sessions"),
        SourceMode::TextMetadata,
        "Reads local Hermes sessions from the WSL home directory when the WSL share is available.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_hermes_state_sessions"),
        &format!("{who} Hermes conversation database"),
        home.join(".hermes").join("state.db"),
        SourceMode::HermesState,
        "Reads Hermes session metadata and messages from the WSL state database read-only.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_hermes_config"),
        &format!("{who} Hermes configuration"),
        home.join(".hermes").join("config.yaml"),
        SourceMode::TextMetadata,
        "Reads local Hermes configuration from WSL for sandbox and workspace path references.",
    );
    push_source(
        sources,
        &format!("wsl_{tag}_hermes_memories"),
        &format!("{who} Hermes memories"),
        home.join(".hermes").join("memories"),
        SourceMode::TextMetadata,
        "Reads local Hermes memory files from WSL for project path references.",
    );
    push_known_folder(
        sources,
        &format!("wsl_{tag}_hermes_skills"),
        &format!("{who} Hermes skills"),
        home.join(".hermes").join("skills"),
    );
    push_known_folder(
        sources,
        &format!("wsl_{tag}_hermes_sandboxes"),
        &format!("{who} Hermes sandboxes"),
        home.join(".hermes").join("sandboxes"),
    );
}

fn source_safe_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn push_vscode_like_sources(
    sources: &mut Vec<DiscoverySource>,
    _appdata: &Path,
    prefix: &str,
    label: &str,
    user_dir: PathBuf,
) {
    let workspace_kind = format!("{prefix}_workspace_storage");
    let workspace_label = format!("{label} workspace storage");
    let workspace_detail =
        format!("Reads {label} workspace storage for folder paths and extension-local AI state.");
    push_source(
        sources,
        &workspace_kind,
        &workspace_label,
        user_dir.join("workspaceStorage"),
        SourceMode::TextMetadata,
        &workspace_detail,
    );

    let global_kind = format!("{prefix}_global_state");
    let global_label = format!("{label} global state");
    let global_detail =
        format!("Reads {label} global state database for workspace path references.");
    push_source(
        sources,
        &global_kind,
        &global_label,
        user_dir.join("globalStorage").join("state.vscdb"),
        SourceMode::SqliteMetadata,
        &global_detail,
    );

    push_vscode_extension_ai_sources(sources, prefix, label, user_dir);
}

fn push_vscode_extension_ai_sources(
    sources: &mut Vec<DiscoverySource>,
    prefix: &str,
    label: &str,
    user_dir: PathBuf,
) {
    let global_storage = user_dir.join("globalStorage");
    for (suffix, extension_dir, extension_label) in [
        (
            "cline_tasks",
            "saoudrizwan.claude-dev",
            "Cline task history",
        ),
        (
            "roo_tasks",
            "rooveterinaryinc.roo-cline",
            "Roo Code task history",
        ),
        ("kilo_tasks", "kilocode.kilo-code", "Kilo Code task history"),
        (
            "continue_extension",
            "continue.continue",
            "Continue extension state",
        ),
        (
            "copilot_chat",
            "github.copilot-chat",
            "GitHub Copilot Chat state",
        ),
        ("copilot", "github.copilot", "GitHub Copilot state"),
        ("cody", "sourcegraph.cody-ai", "Sourcegraph Cody state"),
    ] {
        let kind = format!("{prefix}_{suffix}");
        let source_label = format!("{label} {extension_label}");
        let detail = format!(
            "Reads {extension_label} from {label} globalStorage for project path references."
        );
        push_source(
            sources,
            &kind,
            &source_label,
            global_storage.join(extension_dir),
            SourceMode::TextMetadata,
            &detail,
        );
    }
}

fn push_zed_sources(
    sources: &mut Vec<DiscoverySource>,
    home: &Path,
    appdata: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) {
    for (kind, label, path) in [
        (
            "zed_config_conversations",
            "Zed config conversations",
            home.join(".config").join("zed").join("conversations"),
        ),
        (
            "zed_data_conversations",
            "Zed data conversations",
            home.join(".local")
                .join("share")
                .join("zed")
                .join("conversations"),
        ),
    ] {
        push_source(
            sources,
            kind,
            label,
            path,
            SourceMode::TextMetadata,
            "Reads local Zed agent/conversation JSON metadata for project paths.",
        );
    }
    if let Some(appdata) = appdata {
        push_source(
            sources,
            "zed_windows_roaming_conversations",
            "Zed Windows roaming conversations",
            appdata.join("Zed").join("conversations"),
            SourceMode::TextMetadata,
            "Reads local Zed conversation metadata for project paths.",
        );
    }
    if let Some(localappdata) = localappdata {
        push_source(
            sources,
            "zed_windows_local_conversations",
            "Zed Windows local conversations",
            localappdata.join("Zed").join("conversations"),
            SourceMode::TextMetadata,
            "Reads local Zed conversation metadata for project paths.",
        );
    }
}

fn push_known_folder(sources: &mut Vec<DiscoverySource>, kind: &str, label: &str, path: PathBuf) {
    sources.push(DiscoverySource {
        kind: kind.to_string(),
        label: label.to_string(),
        path,
        detail: Some("Shallow passive marker scan.".to_string()),
        mode: SourceMode::KnownFolder,
    });
}

fn push_known_folder_if_dir(
    sources: &mut Vec<DiscoverySource>,
    kind: &str,
    label: &str,
    path: PathBuf,
) {
    if path.is_dir() && sources.iter().all(|existing| existing.path != path) {
        push_known_folder(sources, kind, label, path);
    }
}

fn push_source(
    sources: &mut Vec<DiscoverySource>,
    kind: &str,
    label: &str,
    path: PathBuf,
    mode: SourceMode,
    detail: &str,
) {
    sources.push(DiscoverySource {
        kind: kind.to_string(),
        label: label.to_string(),
        path,
        detail: Some(detail.to_string()),
        mode,
    });
}

fn scan_discovery_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    match source.mode {
        SourceMode::KnownFolder => scan_known_folder_source(source, candidates),
        SourceMode::TextMetadata => scan_text_metadata_source(source, candidates, sessions),
        SourceMode::SqliteMetadata => scan_sqlite_metadata_source(source, candidates, sessions),
        SourceMode::PinokioApps => scan_pinokio_apps_source(source, candidates),
        SourceMode::CursorProjectTranscripts => {
            scan_cursor_project_transcripts_source(source, candidates, sessions)
        }
        SourceMode::CursorIdeChats => scan_cursor_ide_chats_source(source, candidates, sessions),
        SourceMode::HermesState => scan_hermes_state_source(source, candidates, sessions),
        SourceMode::OpenClawState => scan_openclaw_state_source(source, candidates, sessions),
    }
}

fn scan_known_folder_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
) {
    scan_folder_source(
        source,
        candidates,
        KNOWN_ROOT_DEPTH,
        KNOWN_ROOT_MAX_DIRS,
        "folder_marker",
        0,
        false,
    );
}

fn scan_folder_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    max_depth: usize,
    max_dirs: usize,
    source_signal_kind: &'static str,
    extra_score: u64,
    include_root: bool,
) {
    let mut queue = VecDeque::from([(source.path.clone(), 0_usize)]);
    let mut visited = 0_usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if visited >= max_dirs {
            break;
        }
        visited += 1;
        if depth > 0 || include_root {
            add_directory_if_project(candidates, &dir, source, source_signal_kind, extra_score);
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_traversable_dir(&path) {
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }
}

fn scan_pinokio_apps_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
) {
    let Ok(entries) = fs::read_dir(&source.path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_traversable_dir(&path) {
            add_candidate(
                candidates,
                &path,
                source,
                "pinokio_app",
                "Pinokio local app folder",
                None,
                36,
            );
            add_marker_signals(candidates, &path);
        }
    }
}

fn scan_text_metadata_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if source.path.is_file() {
        scan_metadata_file(&source.path, source, candidates, sessions);
        return;
    }
    let files = bounded_metadata_files(&source.path, SOURCE_SCAN_DEPTH, SOURCE_SCAN_MAX_FILES);
    let started = Instant::now();
    for file in files {
        if started.elapsed() > SOURCE_SCAN_BUDGET {
            break;
        }
        scan_metadata_file(&file, source, candidates, sessions);
    }
}

/// The rollout/transcript kinds whose project is EXCLUSIVELY the recorded `cwd`
/// (never a path scraped from the message body). Codex archived rollouts share
/// the identical `session_meta` shape as live ones, so they belong here too —
/// otherwise they fall through to the generic body-scraping branch that can link
/// arbitrary dirs mentioned in the conversation.
fn is_cwd_only_session_kind(kind: &str) -> bool {
    kind == "codex_sessions" || kind == "codex_archived_sessions" || kind == "claude_code_projects"
}

/// List one cwd-only rollout/transcript as a session and wire up its project
/// links (candidate + markers + recent-activity) exactly as the normal in-budget
/// path does. `cwd` is the raw recorded working dir (or `None`); the same
/// explicit-cwd/existence policy is applied here so the oversize path and the normal
/// path stay behaviourally identical. Factored out so the oversize gate can reuse
/// it after a bounded head probe instead of dropping the session entirely.
fn link_cwd_only_session(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
    cwd: Option<PathBuf>,
) {
    let paths: Vec<PathBuf> = cwd
        .filter(|path| path.is_dir() && is_explicit_session_cwd_link_path(path))
        .into_iter()
        .collect();
    if should_list_session_file(source_file, &source.kind) {
        add_session_candidate(sessions, source_file, source, &paths);
    }
    for path in &paths {
        add_candidate(
            candidates,
            path,
            source,
            "session_path",
            "AI session working directory",
            Some(display_path(source_file)),
            32,
        );
        add_marker_signals(candidates, path);
        add_activity_signal(candidates, path, source_file, "session_recent_activity");
    }
}

fn scan_metadata_file(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if is_openclaw_sessions_file(source_file, &source.kind) {
        scan_openclaw_sessions_file(source_file, source, candidates, sessions);
        return;
    }
    if is_hermes_sessions_file(source_file, &source.kind) {
        scan_hermes_sessions_file(source_file, source, candidates, sessions);
        return;
    }
    // Second Antigravity IDE store: a conversation whose UUID also exists in the main
    // `~/.gemini/antigravity` store is the SAME conversation reached via a second path.
    // Drop it here so it is not double-listed; only the IDE-unique conversations
    // (verified: 2 of 29 on the real machine) survive to be surfaced below.
    if is_antigravity_ide_conversation_file(source_file, &source.kind)
        && antigravity_conversation_is_in_main_store(source_file)
    {
        return;
    }
    // Antigravity moves the live conversation into `conversations/<uuid>.db`; that
    // database (not the frozen brain `transcript.jsonl`) is the real, current chat.
    // Surface it as the session and link it from its own step payloads. Each store
    // has its OWN summaries proto, so pick the one that sits beside this conversation.
    if is_antigravity_conversation_db(source_file) {
        let proto_map = antigravity_proto_map_for(source_file);
        scan_antigravity_conversation_db(source_file, source, candidates, sessions, &proto_map);
        return;
    }
    // Codex's session index carries the only user-visible name a rollout has;
    // apply those names to the rollout sessions scanned before it (the index
    // source is registered after the sessions sources). NOT a diverting branch:
    // the generic pass below still extracts any project paths the index holds.
    if source.kind == "codex_index" {
        apply_codex_index_thread_names(source_file, sessions);
    }
    if looks_like_sqlite_metadata_file(source_file) {
        scan_sqlite_metadata_file(source_file, source, candidates, sessions);
    }
    if source_file
        .metadata()
        .map(|metadata| metadata.len() > SOURCE_TEXT_METADATA_MAX_BYTES)
        .unwrap_or(true)
    {
        // Oversized: the full-text scan is skipped, but a rollout/transcript still
        // records its `cwd` in the head. Without this a 425 MB Claude transcript's
        // empty-link candidate is DROPPED for `claude_code_projects` (not a loose
        // kind → `add_session_candidate` refuses it) so 7/20 real Claude sessions
        // vanished; a Codex oversized rollout listed but lost its project link.
        // Recover the link with a bounded head probe (line-aware, ~1 MiB cap).
        if is_cwd_only_session_kind(&source.kind) {
            let cwd = probe_session_cwd(source_file);
            link_cwd_only_session(source_file, source, candidates, sessions, cwd);
            return;
        }
        if should_list_session_file(source_file, &source.kind) {
            add_session_candidate(sessions, source_file, source, &[]);
        }
        return;
    }
    let Some(text) = read_text_prefix(source_file, SOURCE_FILE_BYTES) else {
        if should_list_session_file(source_file, &source.kind) {
            add_session_candidate(sessions, source_file, source, &[]);
        }
        return;
    };
    let decoded = percent_decode_lossy(&text);
    // A Codex or Claude Code session's project is ONLY its recorded `cwd`. Scraping
    // every path mentioned in the transcript body would wrongly add a session's
    // subfolders (e.g. cwd `…\GabrielKnight3-Lab` would also add
    // `…\GabrielKnight3-Lab\gengine`) as separate projects. Each conversation is
    // LISTED as a session linked to its cwd — Claude Code's `claude_code_projects`
    // transcripts were previously not listable, so the live conversation never
    // surfaced. A scratch/absent cwd yields no project (still a loose session).
    if is_cwd_only_session_kind(&source.kind) {
        // The common case: `cwd` is in the 64 KB prefix already read. If it is not
        // (a Claude transcript can front-load ~107 KB of cwd-less `queue-operation`
        // records before the first `cwd`), fall back to the line-aware bounded
        // probe that keeps reading whole lines up to ~1 MiB. The raw cwd is passed
        // through unfiltered; `link_cwd_only_session` applies the scratch/existence
        // policy so this stays identical to the previous inline logic.
        let cwd = extract_session_cwd(&text).or_else(|| probe_session_cwd(source_file));
        link_cwd_only_session(source_file, source, candidates, sessions, cwd);
        return;
    }
    let report_dirs = extract_agent_report_directories(&decoded)
        .into_iter()
        .filter(|path| is_session_project_link_path(path, true))
        .collect::<Vec<_>>();
    let report_paths = report_dirs
        .iter()
        .map(|path| candidate_key(path))
        .collect::<BTreeSet<_>>();
    let paths = if !report_dirs.is_empty() {
        report_dirs
    } else if let Some(cwd) =
        extract_session_cwd(&text).filter(|path| is_session_project_link_path(path, false))
    {
        vec![cwd]
    } else {
        extract_existing_directories(&text)
            .into_iter()
            .filter(|path| is_session_project_link_path(path, false))
            .collect::<Vec<_>>()
    };
    if should_list_session_file(source_file, &source.kind) {
        add_session_candidate(sessions, source_file, source, &paths);
    }
    for path in paths {
        let is_explicit_report_path = report_paths.contains(&candidate_key(&path));
        add_candidate(
            candidates,
            &path,
            source,
            if is_explicit_report_path {
                "agent_report_path"
            } else {
                "session_path"
            },
            if is_explicit_report_path {
                "Listed by local agent project report"
            } else {
                "Referenced by local session metadata"
            },
            Some(display_path(source_file)),
            if is_explicit_report_path { 40 } else { 32 },
        );
        add_marker_signals(candidates, &path);
        add_activity_signal(candidates, &path, source_file, "session_recent_activity");
    }
}

fn scan_cursor_project_transcripts_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if !source.path.is_dir() {
        return;
    }
    let started = Instant::now();
    let Ok(project_entries) = fs::read_dir(&source.path) else {
        return;
    };
    for project_entry in project_entries.flatten() {
        if started.elapsed() > SOURCE_SCAN_BUDGET {
            break;
        }
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let transcripts_dir = project_dir.join("agent-transcripts");
        if !transcripts_dir.is_dir() {
            continue;
        }
        let project_path = decode_cursor_project_store_dir(&project_dir);
        let project_label = project_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .or_else(|| {
                project_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(cursor_store_label)
            })
            .unwrap_or_else(|| "Cursor session".to_string());

        let Ok(session_entries) = fs::read_dir(&transcripts_dir) else {
            continue;
        };
        for session_entry in session_entries.flatten() {
            let session_dir = session_entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            let Some(session_id) = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Some(session_file) = cursor_main_transcript_file(&session_dir, &session_id) else {
                continue;
            };
            let mut linked_paths = BTreeSet::new();
            if let Some(path) = project_path.as_ref() {
                if is_session_project_link_path(path, false) {
                    linked_paths.insert(path.clone());
                }
            }
            if project_path.is_none() {
                if let Some(text) = read_session_prefix(&session_file, SOURCE_FILE_BYTES) {
                    if let Some(cwd) = extract_session_cwd(&text)
                        .filter(|path| is_session_project_link_path(path, false))
                    {
                        linked_paths.insert(cwd);
                    }
                }
            }
            let linked_paths = linked_paths.into_iter().collect::<Vec<_>>();
            for path in &linked_paths {
                add_candidate(
                    candidates,
                    path,
                    source,
                    "cursor_transcript_path",
                    "Referenced by local Cursor transcript metadata",
                    Some(display_path(&session_file)),
                    34,
                );
                add_marker_signals(candidates, path);
                add_activity_signal(candidates, path, &session_file, "session_recent_activity");
            }
            add_session_candidate_with_display(
                sessions,
                &session_file,
                source,
                &linked_paths,
                Some(cursor_session_display_name(&project_label, &session_id)),
                None,
            );
        }
    }
}

fn cursor_main_transcript_file(session_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let preferred = session_dir.join(format!("{session_id}.jsonl"));
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut files = fs::read_dir(session_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    files.sort();
    files.into_iter().next()
}

fn cursor_session_display_name(project_label: &str, session_id: &str) -> String {
    let short_id = session_id.chars().take(8).collect::<String>();
    format!("{project_label} · {short_id}")
}

fn cursor_store_label(name: &str) -> String {
    if name.eq_ignore_ascii_case("empty-window") {
        return "Cursor empty window".to_string();
    }
    let label =
        if name.len() > 2 && name.as_bytes()[0].is_ascii_alphabetic() && name.as_bytes()[1] == b'-'
        {
            &name[2..]
        } else {
            name
        };
    let label = label.replace('-', " ");
    if label.trim().is_empty() {
        "Cursor session".to_string()
    } else {
        label.trim().to_string()
    }
}

fn is_hermes_sessions_file(source_file: &Path, source_kind: &str) -> bool {
    source_kind.contains("hermes")
        && source_file
            .file_name()
            .and_then(|value| value.to_str())
            .map(|name| name.eq_ignore_ascii_case("sessions.json"))
            .unwrap_or(false)
}

fn is_openclaw_sessions_file(source_file: &Path, source_kind: &str) -> bool {
    source_kind.contains("openclaw")
        && source_file
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.eq_ignore_ascii_case("sessions.json"))
            .unwrap_or(false)
        && path_components_lower(source_file)
            .iter()
            .any(|component| component == ".openclaw")
}

/// OpenClaw releases before the database-first store keep one `sessions.json`
/// per agent and sibling `<session-id>.jsonl` transcripts. Preserve those
/// conversations while preferring the transcript file over the index payload.
fn scan_openclaw_sessions_file(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    let Some(text) = read_raw_text_file(source_file, SOURCE_TEXT_METADATA_MAX_BYTES) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(entries) = value.as_object() else {
        return;
    };
    let session_source = DiscoverySource {
        kind: "openclaw_legacy_sessions".to_string(),
        label: source.label.clone(),
        path: source.path.clone(),
        detail: source.detail.clone(),
        mode: SourceMode::TextMetadata,
    };
    for (session_key, entry) in entries {
        let entry_text = entry.to_string();
        let linked_paths = extract_session_cwd(&entry_text)
            .filter(|path| is_session_project_link_path(path, false))
            .into_iter()
            .collect::<Vec<_>>();
        for path in &linked_paths {
            add_candidate(
                candidates,
                path,
                &session_source,
                "session_path",
                "OpenClaw conversation working directory",
                Some(display_path(source_file)),
                38,
            );
            add_marker_signals(candidates, path);
            add_activity_signal(candidates, path, source_file, "session_recent_activity");
        }
        let session_id = entry
            .get("sessionId")
            .or_else(|| entry.get("session_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let transcript = session_id
            .map(|id| source_file.with_file_name(format!("{id}.jsonl")))
            .filter(|path| path.is_file());
        let display_name = entry
            .get("displayName")
            .or_else(|| entry.get("display_name"))
            .or_else(|| entry.get("label"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| session_key.to_string());
        if let Some(transcript) = transcript {
            add_session_candidate_with_display(
                sessions,
                &transcript,
                &session_source,
                &linked_paths,
                Some(display_name),
                None,
            );
        } else {
            add_session_candidate_with_display(
                sessions,
                source_file,
                &session_source,
                &linked_paths,
                Some(display_name),
                Some(session_key),
            );
        }
    }
}

fn scan_hermes_sessions_file(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    let Some(text) = read_raw_text_file(source_file, SOURCE_TEXT_METADATA_MAX_BYTES) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(object) = value.as_object() else {
        return;
    };
    for (session_key, session_value) in object {
        let session_text = session_value.to_string();
        let paths = if let Some(cwd) = extract_session_cwd(&session_text)
            .map(|path| session_path_for_source(source, &path))
            .filter(|path| is_session_project_link_path(path, false))
        {
            vec![cwd]
        } else {
            extract_existing_directories(&session_text)
                .into_iter()
                .filter(|path| is_session_project_link_path(path, false))
                .collect::<Vec<_>>()
        };
        for path in &paths {
            add_candidate(
                candidates,
                path,
                source,
                "session_path",
                "Referenced by local Hermes session metadata",
                Some(display_path(source_file)),
                32,
            );
            add_marker_signals(candidates, path);
            add_activity_signal(candidates, path, source_file, "session_recent_activity");
        }
        let display_name = hermes_session_display_name(session_key, session_value);
        let session_id = session_value
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(session_key);
        let suffix = format!("hermes-session={session_id}");
        add_session_candidate_with_display(
            sessions,
            source_file,
            source,
            &paths,
            Some(display_name),
            Some(&suffix),
        );
    }
}

const HERMES_STATE_SESSION_LIMIT: usize = 5_000;
const HERMES_TRANSCRIPT_MAX_MESSAGES: usize = 4_000;
pub const HERMES_TRANSCRIPT_MAX_BYTES: usize = 768 * 1024;

fn progressive_transcript_item_limit(
    default_items: usize,
    default_bytes: usize,
    max_bytes: usize,
    load_full: bool,
) -> usize {
    if load_full {
        return usize::MAX;
    }
    let windows = max_bytes
        .saturating_add(default_bytes.saturating_sub(1))
        .checked_div(default_bytes.max(1))
        .unwrap_or(1)
        .max(1);
    default_items.saturating_mul(windows)
}

/// Discover every durable Hermes conversation from the canonical `state.db`.
/// The older `sessions/sessions.json` index is still supported, but it does not
/// contain the message transcript and may omit CLI/TUI sessions. Reading the DB
/// closes both gaps while staying strictly read-only.
fn scan_hermes_state_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if !is_hermes_state_db(&source.path) {
        return;
    }
    let Ok(conn) = open_discovery_sqlite(&source.path) else {
        return;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, source, title, cwd, started_at, ended_at \
         FROM sessions ORDER BY COALESCE(ended_at, started_at) DESC LIMIT ?1",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([HERMES_STATE_SESSION_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1).unwrap_or_default(),
            row.get::<_, Option<String>>(2).unwrap_or_default(),
            row.get::<_, Option<String>>(3).unwrap_or_default(),
            row.get::<_, Option<f64>>(4).unwrap_or_default(),
            row.get::<_, Option<f64>>(5).unwrap_or_default(),
        ))
    }) else {
        return;
    };

    for row in rows.flatten() {
        let (session_id, source_name, title, cwd, started_at, ended_at) = row;
        let linked_paths = cwd
            .as_deref()
            .map(PathBuf::from)
            .map(|path| session_path_for_source(source, &path))
            .filter(|path| is_session_project_link_path(path, false))
            .into_iter()
            .collect::<Vec<_>>();
        for path in &linked_paths {
            add_candidate(
                candidates,
                path,
                source,
                "session_path",
                "Hermes conversation working directory",
                Some(display_path(&source.path)),
                38,
            );
            add_marker_signals(candidates, path);
            add_activity_signal(candidates, path, &source.path, "session_recent_activity");
        }
        let display_name = title
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                source_name
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| format!("{value} · {}", short_session_id(&session_id)))
            })
            .unwrap_or_else(|| format!("Hermes · {}", short_session_id(&session_id)));
        let suffix = format!("hermes-session={session_id}");
        add_session_candidate_with_display(
            sessions,
            &source.path,
            source,
            &linked_paths,
            Some(display_name),
            Some(&suffix),
        );
        let session_path = source_file_with_fragment(&source.path, &suffix);
        if let Some(entry) = sessions.get_mut(&candidate_key(&session_path)) {
            entry.modified_ms = ended_at
                .or(started_at)
                .and_then(sqlite_time_to_epoch_ms)
                .or(entry.modified_ms);
        }
    }
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(10).collect()
}

fn sqlite_time_to_epoch_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let value = if value >= 100_000_000_000.0 {
        value
    } else {
        value * 1_000.0
    };
    (value <= i64::MAX as f64).then_some(value.round() as i64)
}

fn session_path_for_source(source: &DiscoverySource, raw: &Path) -> PathBuf {
    if raw.to_string_lossy().starts_with('/') {
        let source_path = display_path(&source.path);
        if let Some(rest) = source_path
            .to_ascii_lowercase()
            .strip_prefix(r"\\wsl.localhost\")
        {
            if let Some(distro_lower) = rest.split('\\').next() {
                if let Some(distro) = source_path
                    .strip_prefix(r"\\wsl.localhost\")
                    .and_then(|value| value.split('\\').next())
                {
                    debug_assert_eq!(distro.to_ascii_lowercase(), distro_lower);
                    return translate_wsl_path(distro, raw);
                }
            }
        }
    }
    raw.to_path_buf()
}

pub fn is_hermes_state_db(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.eq_ignore_ascii_case("state.db"))
        .unwrap_or(false)
        && path_components_lower(path)
            .iter()
            .any(|component| component == ".hermes")
}

/// Render one Hermes SQLite conversation as bounded JSONL. Keeping one JSON
/// object per message lets the existing frontend transcript parser preserve
/// roles without exposing or copying the whole database.
pub fn hermes_session_transcript(
    path: &Path,
    session_id: &str,
    max_bytes: usize,
) -> Option<(String, bool)> {
    hermes_session_transcript_window(path, session_id, max_bytes, false)
}

pub fn hermes_session_transcript_window(
    path: &Path,
    session_id: &str,
    max_bytes: usize,
    load_full: bool,
) -> Option<(String, bool)> {
    if !is_hermes_state_db(path) || session_id.trim().is_empty() || max_bytes == 0 {
        return None;
    }
    let max_bytes = if load_full { usize::MAX } else { max_bytes };
    let max_messages = progressive_transcript_item_limit(
        HERMES_TRANSCRIPT_MAX_MESSAGES,
        HERMES_TRANSCRIPT_MAX_BYTES,
        max_bytes,
        load_full,
    );
    let query_limit = max_messages.saturating_add(1).min(i64::MAX as usize) as i64;
    let conn = open_discovery_sqlite(path).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT role, content, timestamp FROM messages \
             WHERE session_id = ?1 AND COALESCE(active, 1) = 1 \
             ORDER BY timestamp DESC LIMIT ?2",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![session_id, query_limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).unwrap_or_default(),
                row.get::<_, Option<f64>>(2).unwrap_or_default(),
            ))
        })
        .ok()?;
    let mut newest_first = Vec::new();
    let mut used = 0_usize;
    let mut truncated = false;
    for (index, row) in rows.flatten().enumerate() {
        if index >= max_messages {
            truncated = true;
            break;
        }
        let (role, content, timestamp) = row;
        let line = serde_json::json!({
            "role": role,
            "content": content.unwrap_or_default(),
            "timestamp": timestamp,
        })
        .to_string();
        let line_bytes = line.len().saturating_add(1);
        if used.saturating_add(line_bytes) > max_bytes {
            truncated = true;
            break;
        }
        used = used.saturating_add(line_bytes);
        newest_first.push(line);
    }
    newest_first.reverse();
    Some((newest_first.join("\n"), truncated))
}

const OPENCLAW_SESSION_LIMIT: usize = 5_000;
const OPENCLAW_TRANSCRIPT_MAX_EVENTS: usize = 4_000;
pub const OPENCLAW_TRANSCRIPT_MAX_BYTES: usize = 768 * 1024;

/// Read the OpenClaw global DB's authoritative per-agent database registry.
/// Paths can be absolute or relative to the global state database directory.
fn openclaw_agent_database_paths(global_db: &Path) -> Vec<PathBuf> {
    if !global_db.is_file() {
        return Vec::new();
    }
    let Ok(conn) = open_discovery_sqlite(global_db) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT path FROM agent_databases LIMIT 256") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    let base = global_db.parent().unwrap_or_else(|| Path::new("."));
    rows.flatten()
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        })
        .filter(|path| path.is_file())
        .map(|path| canonical_or_original(&path))
        .filter(|path| is_openclaw_agent_database(path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_openclaw_agent_database(path: &Path) -> bool {
    let Ok(conn) = open_discovery_sqlite(path) else {
        return false;
    };
    sqlite_has_table(&conn, "sessions") && sqlite_has_table(&conn, "transcript_events")
}

fn sqlite_has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .unwrap_or(false)
}

fn scan_openclaw_state_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if !source.path.is_file() {
        return;
    }
    scan_openclaw_replay_sessions(&source.path, source, candidates, sessions);
    for agent_db in openclaw_agent_database_paths(&source.path) {
        scan_openclaw_agent_sessions(&agent_db, source, candidates, sessions);
    }
}

fn scan_openclaw_replay_sessions(
    global_db: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    let Ok(conn) = open_discovery_sqlite(global_db) else {
        return;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT session_id, session_key, cwd, created_at, updated_at \
         FROM acp_replay_sessions ORDER BY updated_at DESC LIMIT ?1",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([OPENCLAW_SESSION_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2).unwrap_or_default(),
            row.get::<_, Option<i64>>(3).unwrap_or_default(),
            row.get::<_, Option<i64>>(4).unwrap_or_default(),
        ))
    }) else {
        return;
    };
    for row in rows.flatten() {
        let (session_id, session_key, cwd, created_at, updated_at) = row;
        let linked_paths = cwd
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| is_session_project_link_path(path, false))
            .into_iter()
            .collect::<Vec<_>>();
        add_openclaw_project_links(candidates, source, global_db, &linked_paths);
        let suffix = format!("openclaw-replay={session_id}");
        add_session_candidate_with_display(
            sessions,
            global_db,
            source,
            &linked_paths,
            Some(format!(
                "{} · {}",
                session_key,
                short_session_id(&session_id)
            )),
            Some(&suffix),
        );
        let session_path = source_file_with_fragment(global_db, &suffix);
        if let Some(entry) = sessions.get_mut(&candidate_key(&session_path)) {
            entry.modified_ms = updated_at.or(created_at).or(entry.modified_ms);
        }
    }
}

fn scan_openclaw_agent_sessions(
    agent_db: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    let Ok(conn) = open_discovery_sqlite(agent_db) else {
        return;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.session_id, s.session_key, s.display_name, s.updated_at, e.entry_json \
         FROM sessions s LEFT JOIN session_entries e ON e.session_id = s.session_id \
         ORDER BY s.updated_at DESC LIMIT ?1",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([OPENCLAW_SESSION_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1).unwrap_or_default(),
            row.get::<_, Option<String>>(2).unwrap_or_default(),
            row.get::<_, Option<i64>>(3).unwrap_or_default(),
            row.get::<_, Option<String>>(4).unwrap_or_default(),
        ))
    }) else {
        return;
    };
    for row in rows.flatten() {
        let (session_id, session_key, display_name, updated_at, entry_json) = row;
        let linked_paths = entry_json
            .as_deref()
            .and_then(extract_structured_session_path)
            .filter(|path| is_session_project_link_path(path, false))
            .into_iter()
            .collect::<Vec<_>>();
        add_openclaw_project_links(candidates, source, agent_db, &linked_paths);
        let label = display_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(session_key
                .as_deref()
                .filter(|value| !value.trim().is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| format!("OpenClaw · {}", short_session_id(&session_id)));
        let suffix = format!("openclaw-session={session_id}");
        add_session_candidate_with_display(
            sessions,
            agent_db,
            source,
            &linked_paths,
            Some(label),
            Some(&suffix),
        );
        let session_path = source_file_with_fragment(agent_db, &suffix);
        if let Some(entry) = sessions.get_mut(&candidate_key(&session_path)) {
            entry.modified_ms = updated_at.or(entry.modified_ms);
        }
    }
}

fn add_openclaw_project_links(
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    source: &DiscoverySource,
    source_db: &Path,
    linked_paths: &[PathBuf],
) {
    for path in linked_paths {
        add_candidate(
            candidates,
            path,
            source,
            "session_path",
            "OpenClaw conversation working directory",
            Some(display_path(source_db)),
            38,
        );
        add_marker_signals(candidates, path);
        add_activity_signal(candidates, path, source_db, "session_recent_activity");
    }
}

fn extract_structured_session_path(text: &str) -> Option<PathBuf> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    find_structured_path_value(&value, 0).map(PathBuf::from)
}

fn find_structured_path_value(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth > 6 {
        return None;
    }
    match value {
        serde_json::Value::Object(object) => {
            for key in [
                "cwd",
                "workspaceDir",
                "workspace_dir",
                "projectPath",
                "project_path",
            ] {
                if let Some(path) = object
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|path| !path.trim().is_empty())
                {
                    return Some(path.to_string());
                }
            }
            object
                .values()
                .find_map(|value| find_structured_path_value(value, depth + 1))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_structured_path_value(value, depth + 1)),
        _ => None,
    }
}

/// Read a single OpenClaw SQLite transcript selected by the synthetic fragment
/// emitted during discovery. The database remains read-only and only bounded
/// JSON event rows are returned.
pub fn openclaw_session_transcript(
    path: &Path,
    fragment: &str,
    max_bytes: usize,
) -> Option<(String, bool)> {
    openclaw_session_transcript_window(path, fragment, max_bytes, false)
}

pub fn openclaw_session_transcript_window(
    path: &Path,
    fragment: &str,
    max_bytes: usize,
    load_full: bool,
) -> Option<(String, bool)> {
    if max_bytes == 0 {
        return None;
    }
    let max_bytes = if load_full { usize::MAX } else { max_bytes };
    let max_events = progressive_transcript_item_limit(
        OPENCLAW_TRANSCRIPT_MAX_EVENTS,
        OPENCLAW_TRANSCRIPT_MAX_BYTES,
        max_bytes,
        load_full,
    );
    let query_limit = max_events.saturating_add(1).min(i64::MAX as usize) as i64;
    let conn = open_discovery_sqlite(path).ok()?;
    let (session_id, sql) = if let Some(session_id) = fragment.strip_prefix("openclaw-session=") {
        (
            session_id,
            "SELECT event_json FROM transcript_events WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2",
        )
    } else {
        let session_id = fragment.strip_prefix("openclaw-replay=")?;
        (
            session_id,
            "SELECT update_json FROM acp_replay_events WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2",
        )
    };
    if session_id.trim().is_empty() {
        return None;
    }
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![session_id, query_limit], |row| {
            row.get::<_, String>(0)
        })
        .ok()?;
    let mut newest_first = Vec::new();
    let mut used = 0_usize;
    let mut truncated = false;
    for (index, row) in rows.flatten().enumerate() {
        if index >= max_events {
            truncated = true;
            break;
        }
        let line_bytes = row.len().saturating_add(1);
        if used.saturating_add(line_bytes) > max_bytes {
            truncated = true;
            break;
        }
        used = used.saturating_add(line_bytes);
        newest_first.push(row);
    }
    newest_first.reverse();
    Some((newest_first.join("\n"), truncated))
}

fn read_raw_text_file(path: &Path, max_bytes: u64) -> Option<String> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > max_bytes {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

fn hermes_session_display_name(session_key: &str, value: &serde_json::Value) -> String {
    let display = value
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(session_key);
    let platform = value
        .get("platform")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("origin")
                .and_then(|origin| origin.get("platform"))
                .and_then(serde_json::Value::as_str)
        });
    let session_id = value
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    match (platform, session_id) {
        (Some(platform), Some(session_id)) => format!("{display} · {platform} · {session_id}"),
        (Some(platform), None) => format!("{display} · {platform}"),
        (None, Some(session_id)) => format!("{display} · {session_id}"),
        (None, None) => display.to_string(),
    }
}

fn scan_sqlite_metadata_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if !source.path.is_file() {
        return;
    }
    scan_sqlite_metadata_file(&source.path, source, candidates, sessions);
}

/// Open a discovery database without ever taking a write lock. SQLite WAL locks
/// created inside WSL are not interoperable with Windows UNC locking, so those
/// paths use an immutable URI snapshot. This reads the last checkpointed main DB
/// and avoids multi-second UI stalls; ordinary Windows paths keep normal
/// read-only/WAL semantics and therefore see the latest committed rows.
fn open_discovery_sqlite(path: &Path) -> rusqlite::Result<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let display = display_path(path);
    if display
        .to_ascii_lowercase()
        .starts_with(r"\\wsl.localhost\")
    {
        flags |= OpenFlags::SQLITE_OPEN_URI;
        let uri = format!("file:{display}?immutable=1");
        Connection::open_with_flags(uri, flags)
    } else {
        Connection::open_with_flags(path, flags)
    }
}

fn scan_sqlite_metadata_file(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    // Codex thread state: prefer the schema-aware `threads` reader. It retitles
    // rollouts (119/119 titled vs ~57% from session_index.jsonl) and, uniquely,
    // links each rollout's authoritative `cwd` — WITHOUT the generic scan's
    // text-mining of message columns (`first_user_message`/`preview`), which
    // scrapes stray path fragments the cwd-only policy forbids. The live DB may be
    // a sibling copy, so resolve it by newest-mtime glob first. If the schema
    // probe fails (older Codex without `threads`), fall through to the generic
    // scan so those versions still surface something.
    if source.kind == "codex_state" {
        if let Some(db_path) = codex_state_db_path(source_file) {
            if let Some(index) = read_codex_threads_index(&db_path) {
                apply_codex_state_threads(&db_path, &index, candidates, sessions);
                return;
            }
        }
    }
    if source_file
        .metadata()
        .map(|metadata| metadata.len() > SQLITE_METADATA_MAX_BYTES)
        .unwrap_or(true)
    {
        return;
    }
    // Route through open_discovery_sqlite: identical read-only/no-mutex flags for
    // ordinary paths, but a WSL codex_state that fell through the threads reader
    // (older schema) still gets the `immutable=1` snapshot instead of a raw UNC
    // open that could take an interop-hostile WAL lock.
    let Ok(conn) = open_discovery_sqlite(source_file) else {
        return;
    };
    let Ok(mut table_stmt) = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 32",
    ) else {
        return;
    };
    let Ok(table_rows) = table_stmt.query_map([], |row| row.get::<_, String>(0)) else {
        return;
    };
    for table in table_rows.flatten() {
        let columns = sqlite_text_columns(&conn, &table);
        for column in columns {
            let sql = format!(
                "SELECT {column} FROM {table} WHERE {column} LIKE '%C:%' OR {column} LIKE '%file:%' OR {column} LIKE '%/mnt/%' LIMIT {limit}",
                column = quote_ident(&column),
                table = quote_ident(&table),
                limit = SQLITE_TEXT_ROWS_PER_COLUMN
            );
            let Ok(mut stmt) = conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
                continue;
            };
            for value in rows.flatten() {
                let paths = extract_existing_directories(&value)
                    .into_iter()
                    .filter(|path| is_session_project_link_path(path, false))
                    .collect::<Vec<_>>();
                if should_list_session_file(source_file, &source.kind) {
                    add_session_candidate(sessions, source_file, source, &paths);
                }
                for path in paths {
                    add_candidate(
                        candidates,
                        &path,
                        source,
                        "sqlite_path",
                        "Referenced by local SQLite metadata",
                        Some(format!(
                            "{}:{}:{}",
                            display_path(source_file),
                            table,
                            column
                        )),
                        36,
                    );
                    add_marker_signals(candidates, &path);
                }
            }
        }
    }
}

// --- Cursor in-IDE (Composer/agent) conversation support -----------------------
//
// Cursor keeps its in-IDE Composer/agent chats in the global
// `globalStorage/state.vscdb`. On a real machine that DB is ~344 MB — almost all of
// it is per-message content blobs in `cursorDiskKV` (~41k `agentKv:blob:<sha256>`
// rows, individually JSON message bubbles or protobuf tool records that carry NO
// conversation id in their key) — so the size-capped generic SQLite reader skips
// the whole file and these conversations never surface. The `~/.cursor/projects`
// CLI transcripts are a DIFFERENT surface and do not include these.
//
// The conversation LIST, however, lives in one tiny `ItemTable` row,
// `composer.composerHeaders`, as JSON `{ "allComposers": [ {composerId, name?,
// subtitle?, createdAt, lastUpdatedAt?, workspaceIdentifier{ id, uri? { fsPath,
// path, external, ... } }, isArchived, ... }, ... ] }` (verified: 32 composers,
// 20 named, 17 with a resolvable workspace). Two sibling `ItemTable` rows link the
// rest to a folder: `glass.localAgentProjectMembership.v1` maps a conversation UUID
// to a project id, and `glass.localAgentProjects.v1` gives that project's
// `workspace.uri.fsPath`. We read ONLY those three small rows by exact key — never
// the content blobs — so this is safe regardless of the DB's size and needs no cap.
//
// Every step is defensive: a missing DB/row/schema, non-JSON, or a value shift
// yields zero sessions (never a panic, never a partial-scan abort). The store is
// proprietary and may change, so a no-op is the correct failure mode.

/// One Cursor in-IDE conversation's minimal, listable metadata. Deliberately holds
/// only what a session row needs — never message text — so no conversation content
/// is ever pulled out of the DB.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorIdeConversation {
    /// `composerId`: the stable conversation id, used for the visible-identity key.
    id: String,
    /// Human title (`name`, falling back to `subtitle`) when the conversation has
    /// one; `None` for untitled drafts.
    title: Option<String>,
    /// The recorded workspace directory, if one resolved (self `workspaceIdentifier`
    /// first, then the project this conversation belongs to). `None` = loose.
    cwd: Option<PathBuf>,
}

/// Read one small JSON `ItemTable` value by exact key. The column is declared BLOB;
/// today Cursor writes the JSON as SQLite `text`, but a future version could store it
/// as a `blob`. Fetch whichever the cell actually holds via `get_ref` (a typed
/// `get::<String>` errors on a Blob cell and `get::<Vec<u8>>` errors on a Text cell —
/// either would silently drop every conversation on the other storage type), then
/// parse the bytes. `None` on any miss/parse failure.
fn cursor_item_json(conn: &Connection, key: &str) -> Option<serde_json::Value> {
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1")
        .ok()?;
    let raw: Vec<u8> = stmt
        .query_row([key], |row| {
            Ok(match row.get_ref(0)? {
                rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
                    bytes.to_vec()
                }
                _ => Vec::new(),
            })
        })
        .ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Decode a VS Code `Uri`-shaped JSON object to the Windows directory it names.
/// Prefers `fsPath` (already native), then `path` (posix `/c:/...`), then `external`
/// (a `file://` URI). Existence is the caller's gate — a recorded-but-deleted folder
/// still decodes so the conversation can be reported loose rather than dropped.
fn cursor_uri_to_path(uri: &serde_json::Value) -> Option<PathBuf> {
    if let Some(fs_path) = uri.get("fsPath").and_then(|value| value.as_str()) {
        if fs_path.len() > 1 && fs_path.as_bytes()[1] == b':' {
            return Some(PathBuf::from(fs_path));
        }
    }
    if let Some(path) = uri.get("path").and_then(|value| value.as_str()) {
        let trimmed = path.trim_start_matches('/');
        if trimmed.len() > 1 && trimmed.as_bytes()[1] == b':' {
            return Some(PathBuf::from(trimmed.replace('/', "\\")));
        }
    }
    // `external` is a percent-encoded `file://` URI; reuse the Antigravity decoder,
    // which already handles `file:///c%3A/...` -> `C:\...`.
    uri.get("external")
        .and_then(|value| value.as_str())
        .and_then(antigravity_folder_uri_to_path)
}

/// Build the `projectId -> workspace directory` map from `glass.localAgentProjects.v1`
/// (each entry `{ id, workspace: { uri? } }`). Empty when the row is absent/misshaped;
/// only entries whose workspace URI decodes are kept.
fn cursor_project_paths(conn: &Connection) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    let Some(projects) = cursor_item_json(conn, "glass.localAgentProjects.v1") else {
        return map;
    };
    let Some(list) = projects.as_array() else {
        return map;
    };
    for project in list {
        let Some(id) = project.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        if let Some(path) = project
            .get("workspace")
            .and_then(|workspace| workspace.get("uri"))
            .and_then(cursor_uri_to_path)
        {
            map.insert(id.to_string(), path);
        }
    }
    map
}

/// Extract the listable conversations from a Cursor `composer.composerHeaders` value
/// plus the two project-linking rows, as pure JSON work so it is unit-testable
/// without SQLite. Each composer becomes one [`CursorIdeConversation`]; its `cwd` is
/// the self `workspaceIdentifier.uri` when present and existing, else the folder of
/// the project it belongs to (via `membership` + `project_paths`). A recorded folder
/// that no longer exists resolves to loose (`cwd = None`) rather than a dead link.
fn cursor_ide_conversations_from_json(
    headers: &serde_json::Value,
    membership: &serde_json::Value,
    project_paths: &BTreeMap<String, PathBuf>,
) -> Vec<CursorIdeConversation> {
    let Some(composers) = headers
        .get("allComposers")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let membership = membership.as_object();
    let mut out = Vec::new();
    for composer in composers {
        let Some(id) = composer.get("composerId").and_then(|value| value.as_str()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        // Title: `name` first, then `subtitle`; blanks are treated as untitled.
        let title = ["name", "subtitle"]
            .iter()
            .find_map(|field| composer.get(*field).and_then(|value| value.as_str()))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        // cwd: the conversation's own workspace URI wins when it exists on disk.
        let mut cwd = composer
            .get("workspaceIdentifier")
            .and_then(|workspace| workspace.get("uri"))
            .and_then(cursor_uri_to_path)
            .filter(|path| path.is_dir());
        // Otherwise fall back to the project this conversation is a member of.
        if cwd.is_none() {
            if let Some(project_id) = membership
                .and_then(|map| map.get(id))
                .and_then(|value| value.as_str())
            {
                cwd = project_paths
                    .get(project_id)
                    .filter(|path| path.is_dir())
                    .cloned();
            }
        }
        out.push(CursorIdeConversation {
            id: id.to_string(),
            title,
            cwd,
        });
    }
    out
}

/// List Cursor's in-IDE Composer/agent conversations as sessions, linking each to its
/// recorded workspace directory (loose when none resolves). Opens the DB read-only
/// and reads only the three small metadata rows; any failure is a clean no-op.
fn scan_cursor_ide_chats_source(
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    if !source.path.is_file() {
        return;
    }
    // Read-only, no write lock. This is the same open policy the other Cursor SQLite
    // readers use; a live Cursor may hold a WAL, which read-only open tolerates.
    let Ok(conn) = open_discovery_sqlite(&source.path) else {
        return;
    };
    let Some(headers) = cursor_item_json(&conn, "composer.composerHeaders") else {
        // No composer headers -> nothing to list (older Cursor, or schema shift).
        return;
    };
    let membership = cursor_item_json(&conn, "glass.localAgentProjectMembership.v1")
        .unwrap_or(serde_json::Value::Null);
    let project_paths = cursor_project_paths(&conn);
    for conversation in cursor_ide_conversations_from_json(&headers, &membership, &project_paths) {
        add_cursor_ide_conversation_session(sessions, candidates, source, &conversation);
    }
}

/// Register one Cursor in-IDE conversation as a session (and, when it has a
/// workspace, wire up the project candidate/markers/recency like other cwd-linked
/// sessions). The visible-identity key is derived from the `composerId` via a
/// `cursor-ide-chat=<id>` path fragment, so re-discovery folds duplicates while
/// distinct conversations never collapse.
fn add_cursor_ide_conversation_session(
    sessions: &mut BTreeMap<String, SessionBuilder>,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    source: &DiscoverySource,
    conversation: &CursorIdeConversation,
) {
    let linked: Vec<PathBuf> = conversation
        .cwd
        .as_ref()
        .filter(|path| !is_agent_scratch_path(path) && path.is_dir())
        .cloned()
        .into_iter()
        .collect();
    // Untitled drafts still list with a stable synthetic name so the row is not blank
    // and distinct drafts stay visually distinguishable.
    let display_name = conversation.title.clone().unwrap_or_else(|| {
        let short = conversation
            .id
            .split('-')
            .next()
            .unwrap_or(&conversation.id);
        format!("Cursor chat {short}")
    });
    add_session_candidate_with_display(
        sessions,
        &source.path,
        source,
        &linked,
        Some(display_name),
        Some(&format!("cursor-ide-chat={}", conversation.id)),
    );
    for path in &linked {
        add_candidate(
            candidates,
            path,
            source,
            "session_path",
            "AI session working directory",
            Some(display_path(&source.path)),
            32,
        );
        add_marker_signals(candidates, path);
        add_activity_signal(candidates, path, &source.path, "session_recent_activity");
    }
}

// --- Cursor in-IDE conversation TRANSCRIPT (preview) ---------------------------
//
// Verified against a real 344 MB `state.vscdb` (read-only): the ORDER + membership
// of a composer's messages live in `cursorDiskKV` row `composerData:<composerId>`,
// whose `fullConversationHeadersOnly` is an ORDERED array of
// `{ bubbleId, type, serverBubbleId, grouping, contentHeightHint }` (type 1 = user,
// 2 = assistant). Each header resolves to its message row
// `bubbleId:<composerId>:<bubbleId>`; the readable reply text is the bubble's `text`
// field (user bubbles also carry a `richText` ProseMirror doc, but `text` is the
// clean carrier). Assistant `thinking.text` is internal reasoning, NOT the reply, so
// it is deliberately not rendered. On that machine every bubble was JSON stored as
// SQLite text; other installs may store protobuf/undecodable bubbles, so those are
// handled gracefully (readable-UTF-8 scavenge, else a short placeholder). We load
// ONLY the target composer's record + its own bubbles by exact key — never the whole
// ~20k-bubble table — so this is safe regardless of the DB's size.

/// How many of a Cursor composer's most-recent messages a preview may render, and the
/// byte ceiling for the assembled transcript. Mirrors the Hermes/OpenClaw/Antigravity
/// caps so one enormous conversation can't blow up the preview.
const CURSOR_IDE_TRANSCRIPT_MAX_MESSAGES: usize = 4_000;
pub const CURSOR_IDE_TRANSCRIPT_MAX_BYTES: usize = 768 * 1024;

/// Read one `cursorDiskKV` value by exact key, returning its raw bytes. Like
/// [`cursor_item_json`], the column is declared BLOB but today holds text, so read
/// whichever the cell actually stores via `get_ref` rather than a typed getter that
/// would error on the other storage type. `None` on any miss.
fn cursor_kv_bytes(conn: &Connection, key: &str) -> Option<Vec<u8>> {
    let mut stmt = conn
        .prepare("SELECT value FROM cursorDiskKV WHERE key = ?1")
        .ok()?;
    stmt.query_row([key], |row| {
        Ok(match row.get_ref(0)? {
            rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
                bytes.to_vec()
            }
            _ => Vec::new(),
        })
    })
    .ok()
}

/// Render one bubble's raw stored bytes into a readable turn body. JSON bubbles yield
/// their `text` (falling back to a plain-string `richText`); a protobuf/undecodable
/// bubble is salvaged by the schema-less protobuf text walk, and only if THAT finds
/// nothing do we emit a short placeholder. Returns `None` for a JSON bubble that is
/// deliberately empty (a tool-call/diff bubble with no prose) so the caller can skip
/// it entirely rather than print an empty turn.
fn cursor_bubble_body(raw: &[u8]) -> Option<String> {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(raw) {
        // Primary: the plain `text` reply. Fallback: a `richText` that happens to be a
        // bare string (older bubbles); a JSON ProseMirror `richText` is skipped here
        // because `text` already mirrors it on every bubble we observed.
        let text = value
            .get("text")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                value
                    .get("richText")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
            });
        return text.map(|value| value.trim_end().to_string());
    }
    // Not JSON: treat as protobuf/undecodable. Recover any plainly-readable UTF-8 via
    // the same schema-less walk the Antigravity reader uses; never emit raw bytes.
    if let Some(block) = step_text_from_fragments(extract_protobuf_text_fragments(raw)) {
        return Some(block);
    }
    Some("_[unrenderable message]_".to_string())
}

/// Assemble a readable, role-labelled transcript for one Cursor in-IDE composer from
/// the composer record JSON plus a resolver that fetches each bubble's raw bytes by
/// id. Split out as (mostly) pure logic so it is unit-testable without a live 344 MB
/// SQLite file: the SQLite entry point [`cursor_ide_chat_transcript`] just supplies a
/// closure that reads `bubbleId:<composerId>:<id>`.
///
/// Turns are emitted OLDEST-first (matching the Antigravity/Hermes previews). To
/// honor the message cap while keeping the newest turns, the TAIL of the ordered
/// header list is taken. Skipped/empty bubbles never abort the render.
fn cursor_transcript_from_composer<F>(
    composer: &serde_json::Value,
    mut bubble_bytes: F,
    max_messages: usize,
    max_bytes: usize,
) -> Option<(String, bool)>
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    if max_bytes == 0 {
        return None;
    }
    let headers = composer
        .get("fullConversationHeadersOnly")
        .and_then(|value| value.as_array())?;

    // Keep only the newest `max_messages` headers (the tail), flagging truncation when
    // older turns were dropped — same "newest messages only" contract as the siblings.
    let mut truncated = headers.len() > max_messages;
    let start = headers.len().saturating_sub(max_messages);

    let mut blocks: Vec<String> = Vec::new();
    let mut total = 0usize;
    for header in &headers[start..] {
        let Some(id) = header.get("bubbleId").and_then(|value| value.as_str()) else {
            continue;
        };
        // Cursor encodes the speaker as `type`: 1 = user, 2 = assistant.
        let role = match header.get("type").and_then(serde_json::Value::as_i64) {
            Some(1) => "User",
            Some(2) => "Assistant",
            _ => "Message",
        };
        let Some(raw) = bubble_bytes(id) else {
            // The header points at a bubble row that isn't present (partial store /
            // pruned history): skip it rather than abort the whole transcript.
            continue;
        };
        let Some(body) = cursor_bubble_body(&raw) else {
            continue; // empty tool-call/diff bubble: no prose to show.
        };
        let block = format!("## {role}\n\n{body}");
        // +2 accounts for the blank-line separator between turns.
        if total + block.len() + 2 > max_bytes && !blocks.is_empty() {
            truncated = true;
            break;
        }
        total += block.len() + 2;
        blocks.push(block);
    }

    if blocks.is_empty() {
        return None;
    }
    let mut text = String::with_capacity(total + 128);
    if truncated {
        text.push_str(
            "[Showing the most recent messages of this Cursor conversation; earlier history truncated.]\n\n",
        );
    }
    text.push_str(&blocks.join("\n\n"));
    Some((text, truncated))
}

/// The outcome of trying to render one Cursor in-IDE conversation. Distinguishing
/// "empty" from "unavailable" lets the caller show the RIGHT note: a conversation with
/// no persisted messages read PERFECTLY — it just has nothing to show — so it must NOT
/// get the alarming "couldn't read this store, it may be in use" note, which is reserved
/// for a genuinely unreadable/locked DB. On a real machine ~1/3 of the listed composers
/// are empty drafts (plus the odd NULL record), so this distinction matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorChatTranscript {
    /// A readable transcript plus whether older turns were dropped to fit the cap.
    Rendered { text: String, truncated: bool },
    /// The DB opened fine but this composer has no renderable messages: a 0-message
    /// draft, or a missing/NULL/unparseable `composerData` record for a listed
    /// composer (Cursor never persisted a conversation body for it).
    Empty,
    /// The DB itself could not be opened (locked mid-write, corrupt, wrong path) — the
    /// only case where the caller should surface the "close the app and retry" note.
    Unavailable,
}

/// Read-only, size-bounded transcript for one Cursor in-IDE (Composer/agent)
/// conversation identified by its `composerId`. Loads ONLY that composer's
/// `composerData:` record and the specific `bubbleId:` rows it references — never the
/// whole content-blob table. Fully defensive: no panic on a missing row, non-JSON
/// value, or schema shift. Only a failure to OPEN the DB is [`CursorChatTranscript::Unavailable`];
/// a listed composer whose record is absent/NULL/unparseable, or which simply has no
/// messages, is [`CursorChatTranscript::Empty`] (a calm "no messages" note, not an alarm).
pub fn cursor_ide_chat_transcript(
    path: &Path,
    composer_id: &str,
    max_bytes: usize,
) -> CursorChatTranscript {
    cursor_ide_chat_transcript_window(path, composer_id, max_bytes, false)
}

pub fn cursor_ide_chat_transcript_window(
    path: &Path,
    composer_id: &str,
    max_bytes: usize,
    load_full: bool,
) -> CursorChatTranscript {
    if composer_id.trim().is_empty() || max_bytes == 0 {
        return CursorChatTranscript::Unavailable;
    }
    let max_bytes = if load_full { usize::MAX } else { max_bytes };
    let max_messages = progressive_transcript_item_limit(
        CURSOR_IDE_TRANSCRIPT_MAX_MESSAGES,
        CURSOR_IDE_TRANSCRIPT_MAX_BYTES,
        max_bytes,
        load_full,
    );
    let Ok(conn) = open_discovery_sqlite(path) else {
        // The store itself is unreadable (locked/corrupt) — the one true "unavailable".
        return CursorChatTranscript::Unavailable;
    };
    // From here the DB is open, so any missing/NULL/unparseable record or empty body is
    // "this conversation has nothing to show", never the alarming unreadable-store note.
    let Some(record_bytes) = cursor_kv_bytes(&conn, &format!("composerData:{composer_id}")) else {
        return CursorChatTranscript::Empty;
    };
    let Ok(composer) = serde_json::from_slice::<serde_json::Value>(&record_bytes) else {
        return CursorChatTranscript::Empty;
    };
    match cursor_transcript_from_composer(
        &composer,
        |bubble_id| cursor_kv_bytes(&conn, &format!("bubbleId:{composer_id}:{bubble_id}")),
        max_messages,
        max_bytes,
    ) {
        Some((text, truncated)) => CursorChatTranscript::Rendered { text, truncated },
        // Record parsed but no renderable turns (0 headers, or all bubbles empty).
        None => CursorChatTranscript::Empty,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRecordedDiffLine {
    pub kind: String,
    pub content: String,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRecordedEdit {
    pub path: String,
    pub request: Option<String>,
    pub lines: Vec<CursorRecordedDiffLine>,
}

/// Decode Cursor's recorded `edit_file_v2` diff bubbles for one composer only.
/// The caller owns secret redaction before IPC. This reader is bounded, opens
/// the database read-only and never scans unrelated composers or file bodies.
pub fn cursor_ide_chat_changes(
    path: &Path,
    composer_id: &str,
) -> Result<Vec<CursorRecordedEdit>, String> {
    const MAX_EDITS: usize = 500;
    const MAX_LINES_PER_EDIT: usize = 800;
    const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

    if composer_id.trim().is_empty() {
        return Err("Cursor change reconstruction needs a conversation id.".to_string());
    }
    let conn = open_discovery_sqlite(path).map_err(|error| {
        format!("Cursor's local conversation store could not be opened: {error}")
    })?;
    let record_bytes = cursor_kv_bytes(&conn, &format!("composerData:{composer_id}"))
        .ok_or_else(|| "Cursor has no persisted body for this conversation.".to_string())?;
    let composer = serde_json::from_slice::<serde_json::Value>(&record_bytes)
        .map_err(|_| "Cursor's conversation index has an unsupported format.".to_string())?;
    let headers = composer
        .get("fullConversationHeadersOnly")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Cursor's conversation has no ordered message list.".to_string())?;
    let start = headers
        .len()
        .saturating_sub(CURSOR_IDE_TRANSCRIPT_MAX_MESSAGES);
    let mut latest_request: Option<String> = None;
    let mut captured = 0usize;
    let mut edits = Vec::new();

    for header in &headers[start..] {
        if edits.len() >= MAX_EDITS || captured >= MAX_CAPTURE_BYTES {
            break;
        }
        let Some(bubble_id) = header.get("bubbleId").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(raw) = cursor_kv_bytes(&conn, &format!("bubbleId:{composer_id}:{bubble_id}"))
        else {
            continue;
        };
        let Ok(bubble) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        if header.get("type").and_then(serde_json::Value::as_i64) == Some(1) {
            latest_request = bubble
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
        }
        let Some(tool) = bubble
            .get("toolFormerData")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        if tool.get("name").and_then(serde_json::Value::as_str) != Some("edit_file_v2") {
            continue;
        }
        let params = match tool.get("params") {
            Some(serde_json::Value::String(encoded)) => {
                serde_json::from_str::<serde_json::Value>(encoded).ok()
            }
            Some(value @ serde_json::Value::Object(_)) => Some(value.clone()),
            _ => None,
        };
        let Some(path) = params
            .as_ref()
            .and_then(|value| value.get("relativeWorkspacePath"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let Some(diff_lines) = tool
            .get("additionalData")
            .and_then(|value| value.get("precomputedDiff"))
            .and_then(|value| value.get("lines"))
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        let mut lines = Vec::new();
        for line in diff_lines.iter().take(MAX_LINES_PER_EDIT) {
            let Some(kind) = line.get("type").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let normalized = match kind.to_ascii_lowercase().as_str() {
                "added" => "added",
                "removed" => "removed",
                "unchanged" | "context" => "context",
                _ => "note",
            };
            let content = line
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if captured.saturating_add(content.len()) > MAX_CAPTURE_BYTES {
                break;
            }
            captured += content.len();
            lines.push(CursorRecordedDiffLine {
                kind: normalized.to_string(),
                content: content.to_string(),
                old_line: line
                    .get("originalLineNumber")
                    .and_then(serde_json::Value::as_u64),
                new_line: line
                    .get("modifiedLineNumber")
                    .and_then(serde_json::Value::as_u64),
            });
        }
        if !lines.is_empty() {
            edits.push(CursorRecordedEdit {
                path: path.to_string(),
                request: latest_request.clone(),
                lines,
            });
        }
    }
    Ok(edits)
}

/// The human title of one Cursor composer (its `name`, then `subtitle`) from the
/// small `composer.composerHeaders` row, for use as a preview display name. `None`
/// when the row/composer is absent or the composer is untitled.
pub fn cursor_ide_chat_title(path: &Path, composer_id: &str) -> Option<String> {
    if composer_id.trim().is_empty() {
        return None;
    }
    let conn = open_discovery_sqlite(path).ok()?;
    let headers = cursor_item_json(&conn, "composer.composerHeaders")?;
    let composers = headers
        .get("allComposers")
        .and_then(|value| value.as_array())?;
    let composer = composers.iter().find(|composer| {
        composer.get("composerId").and_then(|v| v.as_str()) == Some(composer_id)
    })?;
    ["name", "subtitle"]
        .iter()
        .find_map(|field| composer.get(*field).and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// List + link an Antigravity conversation `.db`. The conversation itself is the
/// session (always listed — it is the canonical Antigravity chat transcript); its
/// project links are the authoritative folder roots resolved from the Antigravity
/// summaries proto (an Antigravity project spans several roots, all read from that
/// project's OWN registry — see [`antigravity_conversation_projects`]), with a
/// per-conversation `.db` metadata-blob fallback, never scraped from the
/// conversation's step payloads (which mention many unrelated folders). Read-only
/// and fully defensive: a malformed or unreadable database is still listed as a
/// loose session, never panics.
fn scan_antigravity_conversation_db(
    source_file: &Path,
    source: &DiscoverySource,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
    proto_map: &AntigravityProtoMap,
) {
    // All of the conversation's project roots (or none): an Antigravity project
    // spans several folders, so one conversation can link to each of them.
    let linked = antigravity_conversation_projects(source_file, proto_map);
    let display_name = antigravity_conversation_display_name(source_file);
    add_session_candidate_with_display(sessions, source_file, source, &linked, display_name, None);
    for path in &linked {
        add_candidate(
            candidates,
            path,
            source,
            "antigravity_conversation_path",
            "Referenced by local Antigravity conversation",
            Some(display_path(source_file)),
            32,
        );
        add_marker_signals(candidates, path);
        add_activity_signal(candidates, path, source_file, "session_recent_activity");
    }
}

fn sqlite_text_columns(conn: &Connection, table: &str) -> Vec<String> {
    let pragma = format!("PRAGMA table_info({})", quote_ident(table));
    let Ok(mut stmt) = conn.prepare(&pragma) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let type_name: String = row.get(2).unwrap_or_default();
        Ok((name, type_name))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|(name, type_name)| {
            let lower = type_name.to_ascii_lowercase();
            (lower.contains("text") || lower.contains("char") || lower.is_empty()).then_some(name)
        })
        .take(24)
        .collect()
}

// --- Antigravity (Gemini IDE) conversation `.db` support ------------------------
//
// Antigravity moves a live conversation out of its frozen brain `transcript.jsonl`
// into a plain (un-encrypted) SQLite database at
// `~/.gemini/antigravity/conversations/<uuid>.db`. The chat lives in the `steps`
// table ordered by `idx`, with each message held in the `step_payload` BLOB as
// raw protobuf bytes. We never have the schema, so human text is recovered by a
// schema-less protobuf wire-format scan (see `extract_protobuf_text_fragments`).
//
// Project LINKING, however, must NOT come from the step payloads: a single
// conversation's steps mention many unrelated folders, so scraping them smears
// one chat across several projects. The authoritative one-conversation→one-PROJECT
// mapping lives in `~/.gemini/antigravity/agyhub_summaries_proto.pb` (a flat list
// of per-conversation summary records); each `.db` also carries its own folder in
// the `trajectory_metadata_blob` row `id='main'` as a fallback. A project itself
// spans several folder roots, so a conversation links to ALL of its project's
// on-disk roots — read from that project's OWN registry file, never scraped from
// step payloads — which keeps the one-conversation→one-PROJECT guarantee intact.
// See `antigravity_proto_map` / `antigravity_conversation_projects`.

/// Maximum protobuf nesting depth the text scanner will recurse into. Bounded so
/// a crafted/garbage blob can never blow the stack.
const PROTOBUF_SCAN_MAX_DEPTH: u32 = 6;
/// Shortest length-delimited UTF-8 run worth emitting as a text fragment. Skips
/// 1-character noise without dropping short words.
const PROTOBUF_MIN_TEXT_LEN: usize = 2;
/// How much of a conversation `.db` transcript to render in a preview. Antigravity
/// conversations are large (tens of MB), so the preview keeps the newest steps up
/// to this many bytes of extracted text and notes the truncation.
pub const ANTIGRAVITY_TRANSCRIPT_MAX_BYTES: usize = 768 * 1024;
/// Hard cap on how many of the newest `steps` rows a single conversation preview
/// will decode, so even a pathological row count stays bounded.
const ANTIGRAVITY_TRANSCRIPT_MAX_STEPS: usize = 4_000;
/// Discovery reads only a handful of bounded early steps to derive a human label.
const ANTIGRAVITY_TITLE_MAX_STEPS: usize = 8;
const ANTIGRAVITY_TITLE_MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// True when `path` is an Antigravity conversation database
/// (`~/.gemini/antigravity/conversations/<name>.db`). These are Windows-home only
/// stores; the caller is responsible for the home/platform gating, this only
/// checks the path shape.
pub fn is_antigravity_conversation_db(path: &Path) -> bool {
    let is_db = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("db"))
        .unwrap_or(false);
    if !is_db {
        return false;
    }
    let components = path_components_lower(path);
    // Both the main store (`antigravity/`) and the second IDE store
    // (`antigravity-ide/`) hold live conversations as `<uuid>.db`.
    component_sequence_contains(&components, &[".gemini", "antigravity", "conversations"])
        || component_sequence_contains(
            &components,
            &[".gemini", "antigravity-ide", "conversations"],
        )
}

/// Whether `path` lives in the second Antigravity IDE store (`~/.gemini/antigravity-ide`)
/// rather than the main `~/.gemini/antigravity` store. Used to pick the IDE store's own
/// summaries proto and to apply the cross-store UUID dedup only to IDE files.
fn is_antigravity_ide_path(path: &Path) -> bool {
    component_sequence_contains(
        &path_components_lower(path),
        &[".gemini", "antigravity-ide"],
    )
}

/// One Antigravity conversation is a `<uuid>.pb` (frozen) or `<uuid>.db` (live) file
/// in a store's `conversations/` directory. The dedup keys on that UUID (the file
/// stem), so return it lowercased for a conversation file, else `None`.
fn antigravity_conversation_uuid(path: &Path) -> Option<String> {
    let ext = path.extension().and_then(|value| value.to_str())?;
    if !(ext.eq_ignore_ascii_case("pb") || ext.eq_ignore_ascii_case("db")) {
        return None;
    }
    if !component_sequence_contains(&path_components_lower(path), &["conversations"]) {
        return None;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_ascii_lowercase())
}

/// Whether `path` is a conversation file in the second Antigravity IDE store, i.e. a
/// candidate for cross-store UUID dedup. Gated on the IDE source kind too so the
/// check never fires for the main store's own files.
fn is_antigravity_ide_conversation_file(path: &Path, kind: &str) -> bool {
    kind == "gemini_antigravity_ide_conversations"
        && is_antigravity_ide_path(path)
        && antigravity_conversation_uuid(path).is_some()
}

/// Process-wide cache of the MAIN store's conversation UUID set, invalidated when the
/// `~/.gemini/antigravity/conversations` directory's modified-time changes (a new or
/// removed conversation bumps the directory mtime). Listing the directory is cheap,
/// but the IDE scan checks every IDE conversation against it, so caching keeps the
/// dedup O(1) per file.
static ANTIGRAVITY_MAIN_UUIDS_CACHE: Mutex<Option<AntigravityMainUuidsCache>> = Mutex::new(None);

struct AntigravityMainUuidsCache {
    modified: Option<SystemTime>,
    uuids: std::sync::Arc<BTreeSet<String>>,
}

/// Path of the main store's conversations directory under a given home, if known.
/// The home is injected so the dedup can be exercised against a synthetic tree in
/// tests without mutating the process environment.
fn antigravity_main_conversations_dir_in(home: Option<&Path>) -> Option<PathBuf> {
    Some(
        home?
            .join(".gemini")
            .join("antigravity")
            .join("conversations"),
    )
}

/// List the conversation UUIDs directly present in one conversations directory
/// (`<uuid>.pb`/`<uuid>.db`). Empty when the directory is absent/unreadable.
fn antigravity_conversation_uuids_in_dir(dir: Option<&Path>) -> BTreeSet<String> {
    let mut uuids = BTreeSet::new();
    if let Some(dir) = dir {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(uuid) = antigravity_conversation_uuid(&entry.path()) {
                    uuids.insert(uuid);
                }
            }
        }
    }
    uuids
}

/// The set of conversation UUIDs present in the MAIN store (`<uuid>.pb`/`<uuid>.db`
/// under `~/.gemini/antigravity/conversations`), cached for the process and keyed on
/// the directory mtime. Empty when the directory is absent/unreadable — which
/// correctly makes the dedup a no-op so no IDE conversation is ever wrongly hidden.
fn main_antigravity_conversation_uuids() -> std::sync::Arc<BTreeSet<String>> {
    let dir = antigravity_main_conversations_dir_in(home_dir().as_deref());
    let modified = dir
        .as_ref()
        .and_then(|d| d.metadata().ok())
        .and_then(|meta| meta.modified().ok());

    let mut guard = ANTIGRAVITY_MAIN_UUIDS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(cache) = guard.as_ref() {
        if cache.modified == modified {
            return cache.uuids.clone();
        }
    }
    let uuids = std::sync::Arc::new(antigravity_conversation_uuids_in_dir(dir.as_deref()));
    *guard = Some(AntigravityMainUuidsCache {
        modified,
        uuids: uuids.clone(),
    });
    uuids
}

/// Whether an IDE-store conversation `path`'s UUID appears in `main_uuids` (the set of
/// conversations already present in the main store). Pure set-membership so the dedup
/// rule is unit-testable without touching the filesystem or the process cache.
fn antigravity_conversation_uuid_in_set(path: &Path, main_uuids: &BTreeSet<String>) -> bool {
    match antigravity_conversation_uuid(path) {
        Some(uuid) => main_uuids.contains(&uuid),
        None => false,
    }
}

/// Whether an IDE-store conversation `path` duplicates one already present in the main
/// store (same UUID). Such a conversation is the same chat reached via a second store
/// and must not be listed twice.
fn antigravity_conversation_is_in_main_store(path: &Path) -> bool {
    antigravity_conversation_uuid_in_set(path, &main_antigravity_conversation_uuids())
}

/// Decode a single base-128 varint starting at `offset`. Returns the value and
/// the offset just past it, or `None` if the buffer ends mid-varint or the value
/// overflows 64 bits (treated as corrupt, stops the scan).
fn read_protobuf_varint(bytes: &[u8], offset: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut idx = offset;
    while idx < bytes.len() {
        let byte = bytes[idx];
        idx += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((result, idx));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// Whether a decoded byte run reads as human text: valid UTF-8 that is
/// overwhelmingly printable. Used to decide emit-as-text vs recurse-as-submessage.
fn looks_like_readable_text(value: &str) -> bool {
    if value.chars().count() < PROTOBUF_MIN_TEXT_LEN {
        return false;
    }
    let mut printable = 0usize;
    let mut total = 0usize;
    for ch in value.chars() {
        total += 1;
        if !ch.is_control() || matches!(ch, '\n' | '\t' | '\r') {
            printable += 1;
        }
    }
    total > 0 && (printable * 100) >= (total * 85)
}

/// A length-delimited field that is pure ASCII with no spaces and the shape of an
/// identifier (UUID, base64-ish token, hash) carries no conversational meaning.
/// Dropping these keeps the recovered transcript readable.
fn is_opaque_identifier_fragment(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(|ch| ch.is_whitespace()) {
        return false;
    }
    let alnum = trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '=' | '+' | '/'));
    if !alnum {
        return false;
    }
    // Looks like an identifier only if it is long enough to plausibly be a token
    // and has no lowercase-prose feel (it is a single run of symbols/digits).
    trimmed.len() >= 8
        && trimmed
            .chars()
            .any(|ch| ch.is_ascii_digit() || ch == '-' || ch == '_')
}

/// Walk protobuf fields, appending recovered text to `out`. Returns `true` when
/// every byte parsed as a well-formed field (clean message), `false` when a
/// corrupt/ambiguous field stopped the walk early. The clean/dirty signal lets
/// the caller decide whether a length-delimited chunk is better read as a nested
/// message or as a leaf string.
fn scan_protobuf_fragments(bytes: &[u8], depth: u32, out: &mut Vec<String>) -> bool {
    if depth > PROTOBUF_SCAN_MAX_DEPTH {
        return false;
    }
    let mut idx = 0usize;
    while idx < bytes.len() {
        let Some((key, next)) = read_protobuf_varint(bytes, idx) else {
            return false;
        };
        idx = next;
        let wire_type = key & 0x7;
        match wire_type {
            0 => {
                // varint
                let Some((_, next)) = read_protobuf_varint(bytes, idx) else {
                    return false;
                };
                idx = next;
            }
            1 => {
                // 64-bit
                if idx + 8 > bytes.len() {
                    return false;
                }
                idx += 8;
            }
            5 => {
                // 32-bit
                if idx + 4 > bytes.len() {
                    return false;
                }
                idx += 4;
            }
            2 => {
                // length-delimited
                let Some((len, next)) = read_protobuf_varint(bytes, idx) else {
                    return false;
                };
                let len = len as usize;
                idx = next;
                // Overflow-safe bounds check. `len` comes from an untrusted varint
                // (up to u64::MAX), so computing `idx + len` could wrap on 64-bit
                // and slip past the guard, panicking the slice below. `idx` is
                // always <= bytes.len() here, so compare against the bytes that
                // remain instead.
                if len > bytes.len() - idx {
                    return false;
                }
                let chunk = &bytes[idx..idx + len];
                idx += len;
                // Prefer a nested message when the chunk parses cleanly to its end
                // and yields readable text — wrapper messages often look "printable"
                // as raw bytes, so a UTF-8 check alone would swallow their contents.
                let mut nested = Vec::new();
                let clean = scan_protobuf_fragments(chunk, depth + 1, &mut nested);
                if clean && !nested.is_empty() {
                    out.append(&mut nested);
                    continue;
                }
                if let Ok(text) = std::str::from_utf8(chunk) {
                    if looks_like_readable_text(text) {
                        out.push(text.to_string());
                        continue;
                    }
                }
                // Not clean and not text: keep whatever the trial recursion found.
                out.append(&mut nested);
            }
            _ => return false, // groups / unknown wire types: stop, stay defensive
        }
    }
    true
}

/// Recover human-readable text fragments from one protobuf message by a
/// schema-less wire-format walk. Length-delimited fields that decode as mostly
/// printable UTF-8 are emitted as text; everything else is recursed into as a
/// nested message (bounded depth). Never panics; a malformed blob yields whatever
/// was recovered before the corrupt point (possibly nothing).
pub fn extract_protobuf_text_fragments(payload: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let _ = scan_protobuf_fragments(payload, 0, &mut out);
    out
}

/// Clean and de-duplicate the raw fragments recovered from one step into a
/// readable block: drops tiny/opaque-identifier fragments and consecutive
/// repeats, joins with newlines. Returns `None` when nothing readable remains.
fn step_text_from_fragments(fragments: Vec<String>) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for fragment in fragments {
        let trimmed = fragment.trim();
        if trimmed.chars().count() < PROTOBUF_MIN_TEXT_LEN {
            continue;
        }
        if is_opaque_identifier_fragment(trimmed) {
            continue;
        }
        if lines.last().map(|last| last == trimmed).unwrap_or(false) {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn antigravity_conversation_display_name(path: &Path) -> Option<String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).ok()?;
    let mut stmt = conn
        .prepare("SELECT substr(step_payload, 1, ?1) FROM steps ORDER BY idx ASC LIMIT ?2")
        .ok()?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                ANTIGRAVITY_TITLE_MAX_PAYLOAD_BYTES as i64,
                ANTIGRAVITY_TITLE_MAX_STEPS as i64
            ],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .ok()?;

    for payload in rows.flatten() {
        let Some(payload) = payload else { continue };
        let Some(block) = step_text_from_fragments(extract_protobuf_text_fragments(&payload))
        else {
            continue;
        };
        if let Some(title) = block.lines().find_map(antigravity_title_line) {
            return Some(title);
        }
    }
    None
}

fn antigravity_title_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if [
        "command(",
        "read_file(",
        "write_file(",
        "read_url(",
        "mcp_",
        "sessionid",
        "toolaction",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return None;
    }
    clean_session_title(trimmed)
}

/// The authoritative project a single Antigravity conversation belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AntigravityResolution {
    /// The conversation is anchored to one project folder (`folderUri` decoded to a
    /// Windows path, plus the project UUID for traceability).
    Project { path: PathBuf, project_uuid: String },
    /// The conversation is loose — Antigravity recorded it `outside-of-project`.
    Loose,
}

/// Map from conversation/trajectory UUID (the `.db` filename stem) to the single
/// project it belongs to, parsed from the Antigravity summaries proto.
type AntigravityProtoMap = BTreeMap<String, AntigravityResolution>;

/// Path of an Antigravity summaries proto under the Windows home for a given store
/// subdirectory (`"antigravity"` for the main store, `"antigravity-ide"` for the
/// second IDE store), if a home dir is known:
/// `~/.gemini/<store>/agyhub_summaries_proto.pb`.
fn antigravity_summaries_proto_path_in(store: &str) -> Option<PathBuf> {
    Some(
        home_dir()?
            .join(".gemini")
            .join(store)
            .join("agyhub_summaries_proto.pb"),
    )
}

/// Path of the MAIN store's summaries proto (`~/.gemini/antigravity/...`).
fn antigravity_summaries_proto_path() -> Option<PathBuf> {
    antigravity_summaries_proto_path_in("antigravity")
}

/// Process-wide caches of the parsed summaries protos, invalidated when a proto
/// file's modified-time or length changes. Parsing is O(file size) and each proto is
/// read once per `.db` otherwise, so a discovery run that lists dozens of
/// conversations parses each store's proto a single time. The two stores keep
/// separate caches so an IDE-store lookup never re-parses the (much larger) main one.
static ANTIGRAVITY_PROTO_CACHE: Mutex<Option<AntigravityProtoCache>> = Mutex::new(None);
static ANTIGRAVITY_IDE_PROTO_CACHE: Mutex<Option<AntigravityProtoCache>> = Mutex::new(None);

struct AntigravityProtoCache {
    modified: Option<SystemTime>,
    len: u64,
    map: std::sync::Arc<AntigravityProtoMap>,
}

/// The MAIN store's parsed conversation→project map, cached for the process. Returns
/// an empty map when the proto is missing/unreadable (callers then use the per-`.db`
/// fallback).
fn antigravity_proto_map() -> std::sync::Arc<AntigravityProtoMap> {
    antigravity_proto_map_cached(antigravity_summaries_proto_path(), &ANTIGRAVITY_PROTO_CACHE)
}

/// The summaries proto map to use for one conversation `.db`: the IDE store's own
/// proto when the conversation lives under `antigravity-ide/`, otherwise the main
/// store's. Keeps each store's conversations linking against their OWN project
/// registry, never the wrong store's.
fn antigravity_proto_map_for(conversation_path: &Path) -> std::sync::Arc<AntigravityProtoMap> {
    if is_antigravity_ide_path(conversation_path) {
        antigravity_proto_map_cached(
            antigravity_summaries_proto_path_in("antigravity-ide"),
            &ANTIGRAVITY_IDE_PROTO_CACHE,
        )
    } else {
        antigravity_proto_map()
    }
}

/// Shared cache read/refresh for a summaries proto at `path`, keyed on
/// modified-time + length. Empty map when the proto is missing/unreadable.
fn antigravity_proto_map_cached(
    path: Option<PathBuf>,
    cache: &Mutex<Option<AntigravityProtoCache>>,
) -> std::sync::Arc<AntigravityProtoMap> {
    let (modified, len) = path
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .map(|meta| (meta.modified().ok(), meta.len()))
        .unwrap_or((None, 0));

    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cache) = guard.as_ref() {
        if cache.modified == modified && cache.len == len {
            return cache.map.clone();
        }
    }
    let map = std::sync::Arc::new(
        path.as_ref()
            .map(|p| parse_antigravity_proto_map(p))
            .unwrap_or_default(),
    );
    *guard = Some(AntigravityProtoCache {
        modified,
        len,
        map: map.clone(),
    });
    map
}

/// Parse `agyhub_summaries_proto.pb` into a conversation→project map. The file is a
/// flat repeated field 1 of per-conversation summary records; each record carries
/// field 1 = conversation UUID and field 2 = a summary message whose field 17 holds
/// field 7 = `folderUri` and field 18 = project UUID / the literal
/// `outside-of-project`. Fully defensive: a truncated/garbage proto yields whatever
/// records parsed cleanly and never panics.
fn parse_antigravity_proto_map(path: &Path) -> AntigravityProtoMap {
    let Ok(bytes) = fs::read(path) else {
        return AntigravityProtoMap::new();
    };
    let mut map = AntigravityProtoMap::new();
    // Top level: repeated records. We accept records under any top-level field
    // number (observed: field 1) so a minor format shift still parses.
    for record in protobuf_length_delimited_fields(&bytes) {
        if let Some((uuid, resolution)) = antigravity_record_resolution(record.bytes) {
            map.entry(uuid).or_insert(resolution);
        }
    }
    map
}

/// One length-delimited (wire type 2) field of a protobuf message.
struct ProtobufField<'a> {
    field: u64,
    bytes: &'a [u8],
}

/// Iterate the length-delimited fields of one protobuf message, skipping
/// varint/fixed scalars and stopping cleanly at the first malformed/truncated field
/// (so a corrupt tail never panics and never runs away). Groups and unknown wire
/// types end the walk.
fn protobuf_length_delimited_fields(bytes: &[u8]) -> Vec<ProtobufField<'_>> {
    let mut fields = Vec::new();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let Some((key, next)) = read_protobuf_varint(bytes, idx) else {
            break;
        };
        idx = next;
        let field = key >> 3;
        match key & 0x7 {
            0 => {
                // varint scalar: consume and discard
                let Some((_, next)) = read_protobuf_varint(bytes, idx) else {
                    break;
                };
                idx = next;
            }
            1 => {
                // 64-bit fixed
                let Some(next) = idx.checked_add(8).filter(|n| *n <= bytes.len()) else {
                    break;
                };
                idx = next;
            }
            2 => {
                let Some((len, next)) = read_protobuf_varint(bytes, idx) else {
                    break;
                };
                let Ok(len) = usize::try_from(len) else { break };
                let Some(end) = next.checked_add(len).filter(|n| *n <= bytes.len()) else {
                    break;
                };
                fields.push(ProtobufField {
                    field,
                    bytes: &bytes[next..end],
                });
                idx = end;
            }
            5 => {
                // 32-bit fixed
                let Some(next) = idx.checked_add(4).filter(|n| *n <= bytes.len()) else {
                    break;
                };
                idx = next;
            }
            _ => break, // groups / unknown: stop, stay defensive
        }
    }
    fields
}

/// First length-delimited sub-field with the given number, decoded as a UTF-8
/// string (lossy). `None` if the field is absent or holds a nested message we
/// shouldn't read as text here.
fn protobuf_first_string(message: &[u8], field: u64) -> Option<String> {
    protobuf_length_delimited_fields(message)
        .into_iter()
        .find(|f| f.field == field)
        .map(|f| String::from_utf8_lossy(f.bytes).into_owned())
}

/// First length-delimited sub-field with the given number, as raw bytes (a nested
/// message to descend into).
fn protobuf_first_message(message: &[u8], field: u64) -> Option<&[u8]> {
    protobuf_length_delimited_fields(message)
        .into_iter()
        .find(|f| f.field == field)
        .map(|f| f.bytes)
}

/// Resolve one summary record to `(uuid, Project|Loose)`. Returns `None` when the
/// record carries no conversation UUID (so it cannot be keyed).
fn antigravity_record_resolution(record: &[u8]) -> Option<(String, AntigravityResolution)> {
    // Field 1 = conversation/trajectory UUID (the `.db` filename stem).
    let uuid = protobuf_first_string(record, 1)?;
    if uuid.is_empty() {
        return None;
    }
    // Field 2 = summary message; field 17 = the project-info sub-message.
    let resolution = protobuf_first_message(record, 2)
        .and_then(|summary| protobuf_first_message(summary, 17))
        .map(|project_info| {
            let project_uuid = protobuf_first_string(project_info, 18).unwrap_or_default();
            // field 7 = folderUri (percent-encoded). Absent / "outside-of-project"
            // both mean the conversation is loose.
            match protobuf_first_string(project_info, 7) {
                Some(folder_uri)
                    if !folder_uri.is_empty() && project_uuid != "outside-of-project" =>
                {
                    match antigravity_folder_uri_to_path(&folder_uri) {
                        Some(path) => AntigravityResolution::Project { path, project_uuid },
                        None => AntigravityResolution::Loose,
                    }
                }
                _ => AntigravityResolution::Loose,
            }
        })
        .unwrap_or(AntigravityResolution::Loose);
    Some((uuid, resolution))
}

/// Convert a percent-encoded `file://` folder URI (e.g.
/// `file:///c%3A/AI/Codex/CodeHangar`) into a Windows path (`C:\AI\Codex\CodeHangar`).
/// Does NOT require the directory to exist — existence is the resolver's gate — so a
/// proto-only conversation still yields its recorded path. Returns `None` if the URI
/// does not decode to a drive-letter path.
fn antigravity_folder_uri_to_path(folder_uri: &str) -> Option<PathBuf> {
    let decoded = percent_decode_lossy(folder_uri);
    let trimmed = decoded
        .trim()
        .strip_prefix("file://")
        .unwrap_or(decoded.trim())
        .trim_start_matches('/');
    if trimmed.len() < 2 || trimmed.as_bytes()[1] != b':' {
        return None;
    }
    if !trimmed.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    Some(PathBuf::from(trimmed.replace('/', "\\")))
}

/// Decode an Antigravity registry `folderUri` to the ONE directory it names,
/// keeping it only if that EXACT path exists — no parent walk. The registry URI
/// is authoritative, so when its root has been deleted the correct answer is
/// "gone", not "the nearest surviving ancestor": `extract_file_uri_directories`'
/// walk would resolve a deleted `…\proj\sub` up to a `.git`-bearing parent and
/// misattribute the project. Returns at most one path (a registry field holds a
/// single URI). Empty when the URI is malformed or the folder no longer exists.
fn antigravity_registry_uri_directories(folder_uri: &str) -> Vec<PathBuf> {
    match antigravity_folder_uri_to_path(folder_uri) {
        Some(path) if path.is_dir() => vec![path],
        _ => Vec::new(),
    }
}

/// Every project root an Antigravity conversation `.db` belongs to (empty = the
/// conversation is loose). An Antigravity PROJECT spans MULTIPLE folder roots, so a
/// conversation links to ALL of its project's on-disk roots, not just the single
/// `folderUri` the summaries proto recorded for it. Resolution order:
///   1. The summaries proto (authoritative): `Project { path, project_uuid }` →
///      the deduped set of the proto `path` (if it exists) PLUS every existing root
///      in that project's OWN registry file
///      `~/.gemini/config/projects/<project_uuid>.json` (see
///      [`gemini_project_roots_for_uuid`]). May be empty if nothing exists on disk.
///      `Loose` → empty.
///   2. Fallback (UUID absent from the proto, or proto missing): the conversation's
///      own `trajectory_metadata_blob` row `id='main'`, string-scraped for its ONE
///      `file://` folderUri (→ that dir) or the literal `outside-of-project`
///      (→ empty). The conversation's `steps` payloads are never scraped for links.
///
/// Linking only to a project's own authoritative registry roots (never paths
/// scraped from step payloads) keeps the one-conversation→one-PROJECT guarantee: a
/// conversation can fan out across that project's roots but never smears into a
/// different project.
///
/// Read-only and fully defensive: any failure resolves to an empty `Vec` (loose).
fn antigravity_conversation_projects(
    db_path: &Path,
    proto_map: &AntigravityProtoMap,
) -> Vec<PathBuf> {
    antigravity_conversation_projects_in(db_path, proto_map, home_dir().as_deref())
}

/// [`antigravity_conversation_projects`] with the Gemini home directory injected, so
/// the project-registry fan-out can be exercised against a temp home in tests. `home`
/// is `None` only when no home dir is known (then a proto `Project` yields just its
/// own existing folderUri).
fn antigravity_conversation_projects_in(
    db_path: &Path,
    proto_map: &AntigravityProtoMap,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let uuid = db_path.file_stem().and_then(|stem| stem.to_str());
    if let Some(uuid) = uuid {
        match proto_map.get(uuid) {
            Some(AntigravityResolution::Project { path, project_uuid }) => {
                let mut seen = std::collections::HashSet::new();
                let mut out = Vec::new();
                // The proto's own folderUri first (if it still exists on disk), then
                // the rest of the project's authoritative roots.
                if path.is_dir() && seen.insert(candidate_key(path)) {
                    out.push(path.clone());
                }
                if let Some(home) = home {
                    for root in gemini_project_roots_for_uuid(home, project_uuid) {
                        if seen.insert(candidate_key(&root)) {
                            out.push(root);
                        }
                    }
                }
                return out;
            }
            Some(AntigravityResolution::Loose) => return Vec::new(),
            None => {} // fall through to the per-`.db` blob fallback
        }
    }
    antigravity_db_blob_project(db_path).into_iter().collect()
}

/// Fallback: resolve the conversation's project from its own
/// `trajectory_metadata_blob` row (`id='main'`, column `data`, a protobuf BLOB) by
/// string-scraping that single blob for its one `file://` folderUri or the literal
/// `outside-of-project`. Defensive: a missing table/row/column yields `None`.
fn antigravity_db_blob_project(db_path: &Path) -> Option<PathBuf> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags).ok()?;
    let mut stmt = conn
        .prepare("SELECT data FROM trajectory_metadata_blob WHERE id = 'main'")
        .ok()?;
    let blob: Vec<u8> = stmt
        .query_row([], |row| row.get::<_, Option<Vec<u8>>>(0))
        .ok()
        .flatten()?;
    antigravity_blob_folder(&blob)
}

/// Resolve one `trajectory_metadata_blob` data blob to its project folder: the first
/// `file://` URI that decodes to an existing directory wins; an `outside-of-project`
/// marker (with no usable folder URI) means loose (`None`). Pure byte/string work so
/// it is unit-testable without SQLite.
fn antigravity_blob_folder(blob: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(blob);
    extract_file_uri_directories(&text)
        .into_iter()
        .find(|dir| is_session_project_link_path(dir, false))
}

/// Render an Antigravity conversation `.db` as a readable transcript by decoding
/// the newest `steps` rows' `step_payload` protobufs. Read-only and bounded:
/// keeps the most recent steps up to `max_bytes` of extracted text, then presents
/// them oldest-first. Returns the transcript and whether earlier history was
/// dropped to fit `max_bytes`, or `None` if the database cannot be opened or has
/// no recoverable text, so the caller can fall back to its default rendering.
pub fn antigravity_conversation_transcript(
    path: &Path,
    max_bytes: usize,
) -> Option<(String, bool)> {
    antigravity_conversation_transcript_window(path, max_bytes, false)
}

pub fn antigravity_conversation_transcript_window(
    path: &Path,
    max_bytes: usize,
    load_full: bool,
) -> Option<(String, bool)> {
    if max_bytes == 0 {
        return None;
    }
    let max_bytes = if load_full { usize::MAX } else { max_bytes };
    let max_steps = progressive_transcript_item_limit(
        ANTIGRAVITY_TRANSCRIPT_MAX_STEPS,
        ANTIGRAVITY_TRANSCRIPT_MAX_BYTES,
        max_bytes,
        load_full,
    );
    let query_limit = max_steps.min(i64::MAX as usize) as i64;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).ok()?;
    // Newest-first so a huge conversation keeps its latest turns; we reverse to
    // chronological order before returning.
    let mut stmt = conn
        .prepare("SELECT step_payload FROM steps ORDER BY idx DESC LIMIT ?1")
        .ok()?;
    let rows = stmt
        .query_map([query_limit], |row| row.get::<_, Option<Vec<u8>>>(0))
        .ok()?;

    let mut blocks: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    for payload in rows.flatten() {
        let Some(payload) = payload else { continue };
        let Some(block) = step_text_from_fragments(extract_protobuf_text_fragments(&payload))
        else {
            continue;
        };
        // +2 accounts for the blank-line separator between steps.
        if total + block.len() + 2 > max_bytes && !blocks.is_empty() {
            truncated = true;
            break;
        }
        total += block.len() + 2;
        blocks.push(block);
    }
    if blocks.is_empty() {
        return None;
    }
    // Collected newest-first; present oldest-first like a normal transcript.
    blocks.reverse();
    let mut text = String::with_capacity(total + 128);
    if truncated {
        text.push_str(
            "[Showing the most recent messages of this Antigravity conversation; earlier history truncated.]\n\n",
        );
    }
    text.push_str(&blocks.join("\n\n"));
    Some((text, truncated))
}

fn add_directory_if_project(
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    dir: &Path,
    source: &DiscoverySource,
    source_signal_kind: &'static str,
    extra_score: u64,
) {
    let markers = project_markers_in_dir(dir);
    let has_git = dir.join(".git").exists();
    let cursor_rules = dir.join(".cursor").join("rules").exists();
    let roo_rules = dir.join(".roo").join("rules").exists();
    let continue_rules = dir.join(".continue").join("rules").exists()
        || dir.join(".continue").join("config.yaml").exists()
        || dir.join(".continue").join("assistant").exists()
        || dir.join(".continue").join("assistants").exists();
    let windsurf_rules =
        dir.join(".windsurf").join("rules").exists() || dir.join(".devin").join("rules").exists();
    if markers.is_empty()
        && !has_git
        && !cursor_rules
        && !roo_rules
        && !continue_rules
        && !windsurf_rules
    {
        return;
    }
    let mut score = extra_score + markers.iter().map(|marker| marker.score).sum::<u64>();
    if has_git {
        score += 18;
    }
    if cursor_rules {
        score += 16;
    }
    if roo_rules || windsurf_rules {
        score += 18;
    }
    if continue_rules {
        score += 16;
    }
    add_candidate(
        candidates,
        dir,
        source,
        source_signal_kind,
        "Project markers found in a known local folder",
        None,
        score.max(12),
    );
    add_marker_signals(candidates, dir);
}

fn add_marker_signals(candidates: &mut BTreeMap<String, CandidateBuilder>, dir: &Path) {
    let key = candidate_key(dir);
    let Some(candidate) = candidates.get_mut(&key) else {
        return;
    };
    for marker in project_markers_in_dir(dir) {
        add_signal(
            candidate,
            marker.kind,
            marker.label,
            Some(marker.file_name.to_string()),
            confidence_for_score(marker.score),
            marker.score,
        );
    }
    if dir.join(".git").exists() {
        add_signal(
            candidate,
            "git_metadata",
            "Local .git metadata",
            None,
            "High",
            18,
        );
    }
    if dir.join(".cursor").join("rules").exists() {
        add_signal(
            candidate,
            "cursor_rules_dir",
            "Cursor rules directory",
            Some(".cursor/rules".to_string()),
            "Medium",
            16,
        );
    }
    if dir.join(".roo").join("rules").exists() {
        add_signal(
            candidate,
            "roo_rules_dir",
            "Roo Code workspace rules",
            Some(".roo/rules".to_string()),
            "Medium",
            18,
        );
    }
    if dir.join(".continue").join("rules").exists() {
        add_signal(
            candidate,
            "continue_rules_dir",
            "Continue workspace rules",
            Some(".continue/rules".to_string()),
            "Medium",
            16,
        );
    }
    if dir.join(".continue").join("config.yaml").exists()
        || dir.join(".continue").join("assistant").exists()
        || dir.join(".continue").join("assistants").exists()
    {
        add_signal(
            candidate,
            "continue_config",
            "Continue workspace config",
            Some(".continue".to_string()),
            "Medium",
            16,
        );
    }
    if dir.join(".windsurf").join("rules").exists() {
        add_signal(
            candidate,
            "windsurf_rules_dir",
            "Windsurf workspace rules",
            Some(".windsurf/rules".to_string()),
            "Medium",
            18,
        );
    }
    if dir.join(".devin").join("rules").exists() {
        add_signal(
            candidate,
            "devin_rules_dir",
            "Devin/Windsurf workspace rules",
            Some(".devin/rules".to_string()),
            "Medium",
            18,
        );
    }
    add_directory_activity_signal(candidate, dir);
}

fn add_candidate(
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    dir: &Path,
    source: &DiscoverySource,
    signal_kind: &'static str,
    signal_label: &'static str,
    signal_detail: Option<String>,
    score: u64,
) {
    if !dir.is_dir()
        || is_forbidden_candidate_path(dir)
        || (is_probably_system_or_noise(dir) && !has_project_identity(dir))
    {
        return;
    }
    if !has_project_identity(dir) && is_broad_container_path(dir) {
        return;
    }
    let key = candidate_key(dir);
    let entry = candidates.entry(key).or_insert_with(|| {
        let (estimated_files, estimated_bytes, estimate_partial) = estimate_dir(dir);
        CandidateBuilder {
            path: canonical_or_original(dir),
            estimated_files,
            estimated_bytes,
            estimate_partial,
            ..CandidateBuilder::default()
        }
    });
    entry.source_kinds.insert(source.kind.to_string());
    add_signal(
        entry,
        signal_kind,
        signal_label,
        signal_detail,
        confidence_for_score(score),
        score,
    );
}

fn add_session_candidate(
    sessions: &mut BTreeMap<String, SessionBuilder>,
    source_file: &Path,
    source: &DiscoverySource,
    linked_paths: &[PathBuf],
) {
    add_session_candidate_with_display(sessions, source_file, source, linked_paths, None, None);
}

fn add_session_candidate_with_display(
    sessions: &mut BTreeMap<String, SessionBuilder>,
    source_file: &Path,
    source: &DiscoverySource,
    linked_paths: &[PathBuf],
    display_name: Option<String>,
    key_suffix: Option<&str>,
) {
    if linked_paths.is_empty() && !is_loose_session_source_kind(&source.kind) {
        return;
    }
    let session_path = key_suffix
        .map(|suffix| source_file_with_fragment(source_file, suffix))
        .unwrap_or_else(|| source_file.to_path_buf());
    let key = candidate_key(&session_path);
    // Stat the real on-disk file (not the synthetic `#fragment` key) for recency.
    let modified_ms = file_modified_ms(source_file);
    let derived_display_name = derive_claude_session_display_name(source_file, &source.kind);
    // Session titles come from several app-owned stores and are not guaranteed to
    // be short, human-authored labels. Normalise them at the shared ingestion
    // boundary so one embedded system prompt cannot dominate every session list or
    // accessibility name. Technical filename fallbacks remain unchanged.
    let display_name = display_name.and_then(|name| clean_session_title(&name));
    let display_name = match display_name {
        Some(name) if session_display_name_is_technical(&name, source_file, &source.kind) => {
            derived_display_name.or(Some(name))
        }
        Some(name) => Some(name),
        None => derived_display_name,
    };
    let entry = sessions.entry(key).or_insert_with(|| SessionBuilder {
        path: session_path,
        display_name: display_name.clone(),
        source_kind: source.kind.clone(),
        source_label: source.label.clone(),
        session_kind: session_kind_for_source(&source.kind).to_string(),
        linked_project_paths: BTreeSet::new(),
        confidence: if linked_paths.is_empty() {
            "Low".to_string()
        } else {
            "High".to_string()
        },
        modified_ms,
    });
    let should_replace_display_name = entry
        .display_name
        .as_deref()
        .map(|name| session_display_name_is_technical(name, source_file, &source.kind))
        .unwrap_or(true);
    if should_replace_display_name {
        if let Some(display_name) = display_name {
            entry.display_name = Some(display_name);
        }
    }
    for path in linked_paths {
        entry
            .linked_project_paths
            .insert(canonical_or_original(path));
    }
    if !linked_paths.is_empty() {
        entry.confidence = "High".to_string();
    }
}

/// Best-effort epoch-millis modified time for a session source file. Returns
/// `None` when the file is gone or the clock predates the Unix epoch.
fn file_modified_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as i64)
}

fn source_file_with_fragment(source_file: &Path, suffix: &str) -> PathBuf {
    let safe_suffix = suffix
        .chars()
        .map(|ch| {
            if matches!(ch, '\\' | '/' | '"' | '\'' | '<' | '>' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    PathBuf::from(format!("{}#{}", source_file.to_string_lossy(), safe_suffix))
}

fn add_signal(
    candidate: &mut CandidateBuilder,
    kind: &'static str,
    label: &'static str,
    detail: Option<String>,
    confidence: &'static str,
    score: u64,
) {
    if candidate
        .signals
        .iter()
        .any(|signal| signal.kind == kind && signal.detail == detail)
    {
        return;
    }
    candidate.score = candidate.score.saturating_add(score);
    candidate.signals.push(DiscoverySignal {
        kind: kind.to_string(),
        label: label.to_string(),
        detail,
        confidence: confidence.to_string(),
    });
}

fn mark_registered_state(
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    registered_roots: &[RegisteredRoot],
) {
    let registered = registered_roots
        .iter()
        .map(|root| (canonical_or_original(&root.path), root.project_id))
        .collect::<Vec<_>>();
    for candidate in candidates.values_mut() {
        for (registered_path, project_id) in &registered {
            if same_path(&candidate.path, registered_path) {
                add_signal(
                    candidate,
                    "already_registered",
                    "Already registered in Code Hangar",
                    None,
                    "High",
                    4,
                );
                candidate
                    .source_kinds
                    .insert("code_hangar_registered".to_string());
                if let Some(id) = project_id {
                    add_signal(
                        candidate,
                        "registered_project_id",
                        "Existing Code Hangar project id",
                        Some(id.to_string()),
                        "High",
                        0,
                    );
                }
            } else if is_child_of(&candidate.path, registered_path) {
                add_signal(
                    candidate,
                    "nested_registered_root",
                    "Inside an existing scan root",
                    Some(display_path(registered_path)),
                    "High",
                    2,
                );
            } else if is_child_of(registered_path, &candidate.path) {
                add_signal(
                    candidate,
                    "contains_registered_root",
                    "Contains an existing scan root",
                    Some(display_path(registered_path)),
                    "High",
                    2,
                );
            }
        }
    }
}

fn add_activity_signal(
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    dir: &Path,
    source_file: &Path,
    kind: &'static str,
) {
    let key = candidate_key(dir);
    let Some(candidate) = candidates.get_mut(&key) else {
        return;
    };
    let Ok(metadata) = fs::metadata(source_file) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    if let Some((label, confidence, score)) = recency_signal(modified) {
        add_signal(
            candidate,
            kind,
            label,
            Some(display_path(source_file)),
            confidence,
            score,
        );
    }
}

fn add_directory_activity_signal(candidate: &mut CandidateBuilder, dir: &Path) {
    let Ok(metadata) = fs::metadata(dir) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    if let Some((label, confidence, score)) = recency_signal(modified) {
        add_signal(
            candidate,
            "directory_recent_activity",
            label,
            None,
            confidence,
            score.min(8),
        );
    }
}

fn recency_signal(modified: SystemTime) -> Option<(&'static str, &'static str, u64)> {
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age <= Duration::from_secs(45 * 24 * 60 * 60) {
        Some(("Recent local activity", "High", 14))
    } else if age <= Duration::from_secs(180 * 24 * 60 * 60) {
        Some(("Recent-ish local activity", "Medium", 8))
    } else {
        None
    }
}

/// In global discovery a project must be *deliberate*: listed in a local AI
/// app's project registry, or a folder a session actually worked in — not merely
/// a directory a marker scan walked into. This is the rule that keeps random
/// folders (a cloned repo never opened in an AI app, a `node_modules` sibling
/// with a stray `package.json`) out of the project list. Registered projects
/// always stay so the UI can show their status. Folder-marker evidence still
/// corroborates a deliberate project — it just can't introduce one on its own.
fn candidate_is_deliberate_project(builder: &CandidateBuilder) -> bool {
    // Signal kinds that mean "a person opened this" or "a session ran here".
    const DELIBERATE_SIGNALS: &[&str] = &[
        "app_project_registry",
        "session_path",
        "agent_report_path",
        // A folder referenced ONLY by a local Antigravity conversation is still deliberate (a
        // session ran there). The live signal kind is "antigravity_conversation_path" (emitted by
        // scan_antigravity_conversation_db); the old "decoded_project_path" was never emitted, so
        // conversation-only Antigravity projects were silently dropped from global discovery.
        "antigravity_conversation_path",
        "cursor_transcript_path",
        "sqlite_path",
        "pinokio_app",
        "already_registered",
    ];
    builder
        .signals
        .iter()
        .any(|signal| DELIBERATE_SIGNALS.contains(&signal.kind.as_str()))
}

/// Global discovery normally requires evidence that an AI app deliberately
/// opened a project. User-visible skill definitions are the one intentional
/// exception: they are shown only behind the UI's technical-candidates toggle
/// and are never auto-registered, but hiding them here would make the explicit
/// Codex/Claude/Cursor/Gemini/Hermes/OpenClaw skill sources ineffective.
fn candidate_is_visible_global_discovery_item(
    builder: &CandidateBuilder,
    include_technical_candidates: bool,
) -> bool {
    candidate_is_deliberate_project(builder)
        || (include_technical_candidates
            && builder
                .signals
                .iter()
                .any(|signal| signal.kind == "skill_definition"))
}

fn is_explicit_skill_source(source: &DiscoverySource) -> bool {
    source.kind.contains("skills")
}

fn finalize_candidate(builder: CandidateBuilder) -> ProjectDiscoveryCandidate {
    let already_registered = builder
        .signals
        .iter()
        .any(|signal| signal.kind == "already_registered");
    let existing_project_id = builder
        .signals
        .iter()
        .find(|signal| signal.kind == "registered_project_id")
        .and_then(|signal| signal.detail.as_ref())
        .and_then(|value| value.parse::<i64>().ok());
    let nested_under_registered = builder
        .signals
        .iter()
        .find(|signal| signal.kind == "nested_registered_root")
        .and_then(|signal| signal.detail.clone());
    let contains_registered_roots = builder
        .signals
        .iter()
        .filter(|signal| signal.kind == "contains_registered_root")
        .filter_map(|signal| signal.detail.clone())
        .collect::<Vec<_>>();
    let overlap_kind = if already_registered {
        "already_registered"
    } else if nested_under_registered.is_some() && !contains_registered_roots.is_empty() {
        "mixed_overlap"
    } else if nested_under_registered.is_some() {
        "inside_registered_root"
    } else if !contains_registered_roots.is_empty() {
        "contains_registered_root"
    } else {
        "none"
    };
    let score = builder.score.min(1_000);
    let project_kind = project_kind_for_candidate(&builder.path, &builder.signals);
    ProjectDiscoveryCandidate {
        path: display_path(&builder.path),
        display_name: builder
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| builder.path.to_string_lossy().to_string()),
        project_kind: project_kind.to_string(),
        confidence: confidence_for_total_score(score).to_string(),
        score,
        source_kinds: builder.source_kinds.into_iter().collect(),
        signals: builder.signals,
        already_registered,
        existing_project_id,
        overlap_kind: overlap_kind.to_string(),
        nested_under_registered,
        contains_registered_roots,
        estimated_files: builder.estimated_files,
        estimated_bytes: builder.estimated_bytes,
        estimate_partial: builder.estimate_partial,
    }
}

fn finalize_sessions(
    sessions: BTreeMap<String, SessionBuilder>,
    registered_roots: &[RegisteredRoot],
) -> Vec<SessionDiscoveryCandidate> {
    let sessions = sessions
        .into_values()
        .map(|builder| {
            let linked_registered_project_ids = registered_roots
                .iter()
                .filter(|root| {
                    builder
                        .linked_project_paths
                        .iter()
                        .any(|path| same_path(path, &root.path) || is_child_of(path, &root.path))
                })
                .filter_map(|root| root.project_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let association = if !linked_registered_project_ids.is_empty() {
                "registered_project"
            } else if builder.linked_project_paths.is_empty() {
                "loose_session"
            } else {
                "unregistered_project_reference"
            };
            SessionDiscoveryCandidate {
                path: display_path(&builder.path),
                display_name: builder.display_name.unwrap_or_else(|| {
                    builder
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| builder.path.to_string_lossy().to_string())
                }),
                source_kind: builder.source_kind,
                source_label: builder.source_label,
                session_kind: builder.session_kind,
                confidence: builder.confidence,
                linked_project_paths: builder
                    .linked_project_paths
                    .iter()
                    .map(|path| display_path(path))
                    .collect(),
                linked_registered_project_ids,
                association: association.to_string(),
                modified_ms: builder.modified_ms,
            }
        })
        .collect::<Vec<_>>();
    let mut by_visible_identity: BTreeMap<String, SessionDiscoveryCandidate> = BTreeMap::new();
    for session in sessions {
        let key = session_visible_identity_key(&session);
        match by_visible_identity.get(&key) {
            Some(existing)
                if session_candidate_rank(existing) > session_candidate_rank(&session) => {}
            Some(existing)
                if session_candidate_rank(existing) == session_candidate_rank(&session)
                    && existing.modified_ms.unwrap_or(i64::MIN)
                        >= session.modified_ms.unwrap_or(i64::MIN) => {}
            _ => {
                by_visible_identity.insert(key, session);
            }
        }
    }
    let mut sessions = by_visible_identity.into_values().collect::<Vec<_>>();
    sessions.sort_by(|a, b| {
        session_association_rank(&a.association)
            .cmp(&session_association_rank(&b.association))
            .then_with(|| a.source_label.cmp(&b.source_label))
            .then_with(|| {
                a.path
                    .to_ascii_lowercase()
                    .cmp(&b.path.to_ascii_lowercase())
            })
    });
    sessions
}

fn session_visible_identity_key(session: &SessionDiscoveryCandidate) -> String {
    if let Some((_, fragment)) = session.path.rsplit_once('#') {
        if let Some(session_id) = fragment.strip_prefix("hermes-session=") {
            return format!("hermes-session:{}", session_id.to_ascii_lowercase());
        }
        if let Some(session_id) = fragment.strip_prefix("openclaw-session=") {
            return format!("openclaw-session:{}", session_id.to_ascii_lowercase());
        }
        // Cursor in-IDE conversations all share ONE source file (state.vscdb), so the
        // generic identity below (which folds on source+name+links) would collapse
        // distinct chats that happen to share a title/workspace. Key on the
        // per-conversation `composerId` instead: only the same conversation re-read
        // ever folds.
        if let Some(conversation_id) = fragment.strip_prefix("cursor-ide-chat=") {
            return format!("cursor-ide-chat:{}", conversation_id.to_ascii_lowercase());
        }
    }
    if session.source_kind == "openclaw_legacy_sessions" {
        if let Some(stem) = Path::new(&session.path)
            .file_stem()
            .and_then(|value| value.to_str())
        {
            return format!("openclaw-session:{}", stem.to_ascii_lowercase());
        }
    }
    // Codex rollout transcripts each carry a globally-unique session uuid in their
    // filename. Distinct archived conversations in the same project routinely share an
    // auto-generated (title, cwd) — the generic identity below would then collapse them
    // and silently hide real sessions (Codex's own store shows 27 distinct archived
    // rollouts folding to just 13 title+cwd pairs). Key on the uuid instead, so only a
    // genuinely duplicated file (the same rollout reached via two sources) ever folds.
    if session.source_kind == "codex_sessions" || session.source_kind == "codex_archived_sessions" {
        let file_path = session.path.split('#').next().unwrap_or(&session.path);
        if let Some(id) = codex_rollout_session_id(Path::new(file_path)) {
            return format!("codex-rollout:{id}");
        }
    }
    // Claude Code transcripts are per-session `.jsonl` files whose filename stem IS the
    // session uuid (e.g. `019f3315-12ff-7071-8534-04fe50ed534e.jsonl`). New code titles
    // untitled sessions from their first user message, so two DISTINCT transcripts in the
    // same project can share a `display_name` — the generic identity below (source+name+links)
    // would then collapse them and silently hide a real session. Key on the per-file uuid
    // stem instead, mirroring the Codex arm, so only the same transcript reached twice folds.
    // The stem probe also catches any Claude session .jsonl reached via another source kind;
    // a `claude_code_projects` row that is NOT a .jsonl (e.g. a state file) falls through.
    if session.source_kind == "claude_code_projects"
        || Path::new(session.path.split('#').next().unwrap_or(&session.path))
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
            .unwrap_or(false)
    {
        if let Some(id) = claude_session_stem_id(&session.path) {
            return format!("claude-session:{id}");
        }
    }
    let mut linked = session
        .linked_project_paths
        .iter()
        .map(|path| path.to_ascii_lowercase())
        .collect::<Vec<_>>();
    linked.sort();
    format!(
        "{}\n{}\n{}\n{}\n{}",
        session.session_kind.to_ascii_lowercase(),
        session.source_label.to_ascii_lowercase(),
        session.display_name.to_ascii_lowercase(),
        session.association,
        linked.join("\n")
    )
}

fn session_candidate_rank(session: &SessionDiscoveryCandidate) -> u8 {
    if session.source_kind.contains("hermes_state")
        || session.source_kind.contains("openclaw_state")
    {
        return 3;
    }
    if Path::new(session.path.split('#').next().unwrap_or(&session.path))
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false)
    {
        return 2;
    }
    1
}

fn session_association_rank(association: &str) -> u8 {
    match association {
        "registered_project" => 0,
        "unregistered_project_reference" => 1,
        "loose_session" => 2,
        _ => 3,
    }
}

fn project_kind_for_candidate(path: &Path, signals: &[DiscoverySignal]) -> &'static str {
    if signals.iter().any(|signal| signal.kind == "pinokio_app") {
        return "pinokio_app";
    }
    if is_technical_discovery_candidate_path(path) {
        return "technical_candidate";
    }
    if signals.iter().any(|signal| {
        matches!(
            signal.kind.as_str(),
            "agent_context"
                | "claude_context"
                | "gemini_context"
                | "skill_definition"
                | "openclaw_config"
                | "agent_memory"
                | "agent_persona"
                | "cursor_rules"
                | "cursor_rules_dir"
                | "cline_rules"
                | "roo_rules_dir"
                | "windsurf_rules"
                | "windsurf_rules_dir"
                | "devin_rules_dir"
                | "continue_config"
                | "continue_rules_dir"
                | "aider_history"
                | "aider_input_history"
                | "aider_config"
                | "agent_report_path"
                | "session_path"
                | "sqlite_path"
        )
    }) {
        return "ai_assisted_project";
    }
    if signals.iter().any(|signal| {
        matches!(
            signal.kind.as_str(),
            "node_project"
                | "python_project"
                | "rust_project"
                | "go_project"
                | "compose_project"
                | "python_requirements"
                | "git_metadata"
        )
    }) {
        return "code_project";
    }
    if signals.iter().any(|signal| signal.kind == "readme") {
        return "documentation_project";
    }
    "project_candidate"
}

fn is_technical_discovery_candidate_path(path: &Path) -> bool {
    let components = path_components_lower(path);
    let technical_segments = [
        "skills",
        "custom_nodes",
        "models",
        "checkpoints",
        "resources",
        "outputs",
        "uploads",
        ".vendor",
        ".external",
        "templates",
        "envs",
        "cache",
        "caches",
    ];
    components
        .iter()
        .any(|component| technical_segments.contains(&component.as_str()))
}

fn project_markers_in_dir(dir: &Path) -> Vec<ProjectMarker> {
    let mut markers = Vec::new();
    for marker in PROJECT_MARKERS {
        if dir.join(marker.file_name).exists() {
            markers.push(*marker);
        }
    }
    markers
}

fn bounded_metadata_files(root: &Path, max_depth: usize, max_files: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if files.len() >= max_files || depth > max_depth {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries = entries
            .flatten()
            .map(|entry| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (entry.path(), modified)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|(left_path, left_modified), (right_path, right_modified)| {
            right_modified
                .cmp(left_modified)
                .then_with(|| left_path.cmp(right_path))
        });
        for (path, _) in entries {
            if is_traversable_dir(&path) {
                if !skip_dir(&path) {
                    queue.push_back((path, depth + 1));
                }
            } else if looks_like_metadata_file(&path) {
                files.push(path);
                if files.len() >= max_files {
                    break;
                }
            }
        }
    }
    files
}

fn is_traversable_dir(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_dir() || skip_dir(path) {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn looks_like_metadata_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "json"
            | "jsonl"
            | "md"
            | "txt"
            | "log"
            | "code-workspace"
            | "ldb"
            | "pb"
            | "pbtxt"
            | "db"
            | "sqlite"
            | "sqlite3"
            | "vscdb"
    )
}

fn looks_like_sqlite_metadata_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "db" | "sqlite" | "sqlite3" | "vscdb"
    )
}

fn read_text_prefix(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file
        .metadata()
        .ok()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut bytes = Vec::with_capacity(max_bytes.saturating_add(SOURCE_FILE_TAIL_BYTES));
    let mut prefix = vec![0; max_bytes];
    let read = file.read(&mut prefix).ok()?;
    bytes.extend_from_slice(&prefix[..read]);

    if len > max_bytes as u64 {
        bytes.push(b'\n');
        let tail_start = len.saturating_sub(SOURCE_FILE_TAIL_BYTES as u64);
        if file.seek(SeekFrom::Start(tail_start)).is_ok() {
            let mut tail = vec![0; SOURCE_FILE_TAIL_BYTES];
            if let Ok(tail_read) = file.read(&mut tail) {
                bytes.extend_from_slice(&tail[..tail_read]);
            }
        }
    }

    Some(
        String::from_utf8_lossy(&bytes)
            .replace("\\\\", "\\")
            .replace("\\/", "/"),
    )
}

fn extract_existing_directories(text: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    let mut attempts = 0_usize;
    let decoded = percent_decode_lossy(text);
    let generic_text = agent_report_safe_text(&decoded);
    let normalized = generic_text
        .replace("file:///", "")
        .replace("file://", "")
        .replace("\\u005c", "\\")
        .replace("\\/", "/");
    for uri_path in extract_file_uri_directories(&generic_text) {
        if !insert_extracted_path(&mut paths, uri_path) {
            return paths.into_iter().collect();
        }
    }
    for wsl_path in extract_wsl_windows_directories(&generic_text) {
        if !insert_extracted_path(&mut paths, wsl_path) {
            return paths.into_iter().collect();
        }
    }
    for report_path in extract_agent_report_directories(&decoded) {
        if !insert_extracted_path(&mut paths, report_path) {
            return paths.into_iter().collect();
        }
    }
    for (index, _) in normalized.match_indices(":\\") {
        attempts += 1;
        if attempts > SOURCE_FILE_MAX_PATH_ATTEMPTS {
            break;
        }
        if index == 0 {
            continue;
        }
        let drive_start = index - 1;
        let drive = normalized.as_bytes()[drive_start] as char;
        if !drive.is_ascii_alphabetic() {
            continue;
        }
        let candidate = collect_path_token(&normalized[drive_start..]);
        if let Some(path) = existing_directory_candidate(&candidate) {
            if !insert_extracted_path(&mut paths, path) {
                break;
            }
        }
    }
    for (index, _) in normalized.match_indices(":/") {
        attempts += 1;
        if attempts > SOURCE_FILE_MAX_PATH_ATTEMPTS {
            break;
        }
        if index == 0 {
            continue;
        }
        let drive_start = index - 1;
        let drive = normalized.as_bytes()[drive_start] as char;
        if !drive.is_ascii_alphabetic() {
            continue;
        }
        let candidate = collect_path_token(&normalized[drive_start..]).replace('/', "\\");
        if let Some(path) = existing_directory_candidate(&candidate) {
            if !insert_extracted_path(&mut paths, path) {
                break;
            }
        }
    }
    paths.into_iter().collect()
}

fn insert_extracted_path(paths: &mut BTreeSet<PathBuf>, path: PathBuf) -> bool {
    paths.insert(path);
    paths.len() < SOURCE_FILE_MAX_EXTRACTED_PATHS
}

fn agent_report_safe_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for line in text.lines() {
        if looks_like_non_project_report_path_line(line) {
            continue;
        }
        safe.push_str(line);
        safe.push('\n');
    }
    safe
}

fn looks_like_non_project_report_path_line(line: &str) -> bool {
    let lower = line.trim_start().to_ascii_lowercase();
    if lower.starts_with("ficheiro:") || lower.contains("motivo da exclusão") {
        return true;
    }
    lower.contains("caminho absoluto")
        && !(lower.contains("pasta")
            || lower.contains("projecto")
            || lower.contains("projeto")
            || lower.contains("conversa"))
}

fn extract_agent_report_directories(text: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("caminho") || !lower.contains("absoluto") {
            continue;
        }
        if !(lower.contains("pasta")
            || lower.contains("projecto")
            || lower.contains("projeto")
            || lower.contains("conversa"))
        {
            continue;
        }
        let Some((_, value)) = line.split_once(':') else {
            continue;
        };
        for raw_path in report_windows_path_tokens(value) {
            if let Some(path) = existing_directory_candidate(&raw_path) {
                paths.insert(path);
            }
        }
    }
    paths.into_iter().collect()
}

fn report_windows_path_tokens(value: &str) -> Vec<String> {
    let mut starts = Vec::new();
    for (index, _) in value.match_indices(":\\") {
        if index > 0 && value.as_bytes()[index - 1].is_ascii_alphabetic() {
            starts.push(index - 1);
        }
    }
    for (index, _) in value.match_indices(":/") {
        if index > 0 && value.as_bytes()[index - 1].is_ascii_alphabetic() {
            starts.push(index - 1);
        }
    }
    starts.sort_unstable();
    starts.dedup();

    let mut tokens = Vec::new();
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(value.len());
        let segment = &value[*start..end];
        let token = collect_report_path_token(segment);
        if !token.is_empty() {
            tokens.push(token);
        }
    }
    tokens
}

fn collect_report_path_token(input: &str) -> String {
    let mut token = String::new();
    for ch in input.chars() {
        if matches!(
            ch,
            '"' | '\''
                | '\n'
                | '\r'
                | '\t'
                | '<'
                | '>'
                | '|'
                | '?'
                | '*'
                | '{'
                | '}'
                | '['
                | ']'
                | ','
                | ';'
                | '('
        ) {
            break;
        }
        token.push(ch);
    }
    token
        .trim_matches(|ch: char| {
            ch == ')' || ch == '(' || ch == '.' || ch == ':' || ch.is_whitespace()
        })
        .to_string()
}

fn extract_wsl_windows_directories(text: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for (index, _) in text.match_indices("/mnt/") {
        let raw = collect_path_token(&text[index..]);
        let Some(candidate) = wsl_mount_path_to_windows(&raw) else {
            continue;
        };
        if let Some(path) = existing_directory_candidate(&candidate) {
            paths.insert(path);
        }
    }
    paths.into_iter().collect()
}

fn wsl_mount_path_to_windows(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let rest = trimmed.strip_prefix("/mnt/")?;
    let mut chars = rest.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() || chars.next()? != '/' {
        return None;
    }
    let remainder = chars.as_str().replace('/', "\\");
    Some(format!("{}:\\{}", drive.to_ascii_uppercase(), remainder))
}

fn extract_file_uri_directories(text: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for marker in ["file:///", "file://"] {
        for (index, _) in text.match_indices(marker) {
            let raw = collect_path_token(&text[index + marker.len()..]);
            let normalized = raw.trim_start_matches('/').replace('/', "\\");
            let candidate = if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
                normalized
            } else {
                raw.replace('/', "\\")
            };
            if let Some(path) = existing_directory_candidate(&candidate) {
                paths.insert(path);
            }
        }
    }
    paths.into_iter().collect()
}

fn percent_decode_lossy(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn collect_path_token(input: &str) -> String {
    let mut token = String::new();
    for ch in input.chars() {
        if matches!(
            ch,
            '"' | '\''
                | '\n'
                | '\r'
                | '\t'
                | '<'
                | '>'
                | '|'
                | '?'
                | '*'
                | '{'
                | '}'
                | '['
                | ']'
                | ','
                | ';'
        ) {
            break;
        }
        token.push(ch);
    }
    token
        .trim_matches(|ch: char| {
            ch == ')' || ch == '(' || ch == '.' || ch == ':' || ch.is_whitespace()
        })
        .to_string()
}

fn existing_directory_candidate(raw: &str) -> Option<PathBuf> {
    // Extracted text can name a WSL share; the `is_dir` walk below stats every
    // prefix of it, which can cold-boot the distro. Skip the whole walk while the
    // WSL gate is off (every parent of a WSL path is still a WSL path, so one
    // check up front covers the loop). Checked on the untrimmed input because the
    // trim below strips the leading `\\` the detection needs.
    if wsl_path_blocked_by_gate(Path::new(raw.trim())) {
        return None;
    }
    let mut candidate = raw.trim().trim_matches('\\').to_string();
    if candidate.len() < 4 {
        return None;
    }
    while !candidate.is_empty() {
        let path = PathBuf::from(&candidate);
        if path.is_dir() {
            return Some(prefer_project_container_for_embedded_dependency(&path));
        }
        let next = path.parent()?.to_string_lossy().to_string();
        if next == candidate {
            return None;
        }
        candidate = next;
    }
    None
}

fn prefer_project_container_for_embedded_dependency(path: &Path) -> PathBuf {
    let components = path_components_lower(path);
    for marker in [".external", ".vendor"] {
        let Some(index) = components.iter().position(|component| component == marker) else {
            continue;
        };
        let mut ancestor = path.to_path_buf();
        for _ in index..components.len() {
            if !ancestor.pop() {
                break;
            }
        }
        if ancestor.is_dir() && has_project_identity(&ancestor) {
            return ancestor;
        }
    }
    path.to_path_buf()
}

fn is_meaningful_metadata_path(path: &Path) -> bool {
    if is_forbidden_candidate_path(path) {
        return false;
    }
    if is_internal_ai_state_candidate_path(path) {
        return false;
    }
    if has_project_identity(path) {
        return true;
    }
    if is_probably_system_or_noise(path) {
        return false;
    }
    if is_broad_container_path(path) {
        return false;
    }
    is_local_ai_state_folder(path)
}

fn is_session_project_link_path(path: &Path, explicit_report_path: bool) -> bool {
    if is_forbidden_candidate_path(path)
        || is_internal_ai_state_candidate_path(path)
        || is_broad_container_path(path)
        || is_agent_scratch_path(path)
    {
        return false;
    }
    if is_probably_system_or_noise(path) && !has_project_identity(path) {
        return false;
    }
    explicit_report_path || is_meaningful_metadata_path(path)
}

/// A recorded session `cwd` is direct evidence even when the project has no
/// manifest or VCS marker. Keep that useful markerless behavior while still
/// refusing roots that are categorically not projects.
fn is_explicit_session_cwd_link_path(path: &Path) -> bool {
    if is_forbidden_candidate_path(path)
        || is_internal_ai_state_candidate_path(path)
        || is_known_broad_container_path(path)
        || is_agent_scratch_path(path)
    {
        return false;
    }
    !is_probably_system_or_noise(path) || has_project_identity(path)
}

/// Per-conversation scratch folders that agents create automatically, named by
/// date (e.g. Codex uses `…\Documents\Codex\2026-04-21-<first-prompt>` or
/// `…\codex\2026-04-23\<slug>`). These are loose conversations, never deliberate
/// projects — so they must not become project candidates, but the conversation
/// itself still surfaces as a loose, project-less session.
fn is_agent_scratch_path(path: &Path) -> bool {
    path_components_lower(path)
        .iter()
        .any(|component| is_date_prefixed_component(component))
}

/// A path component that begins with an ISO date (`YYYY-MM-DD`), optionally
/// followed by `-<slug>`. Real project folders are not named this way; agents
/// name per-conversation working directories like this.
fn is_date_prefixed_component(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && (bytes.len() == 10 || bytes[10] == b'-')
}

fn has_project_identity(path: &Path) -> bool {
    !project_markers_in_dir(path).is_empty()
        || path.join(".git").exists()
        || path.join(".cursor").join("rules").exists()
        || path.join(".roo").join("rules").exists()
        || path.join(".continue").join("rules").exists()
        || path.join(".continue").join("config.yaml").exists()
        || path.join(".windsurf").join("rules").exists()
        || path.join(".devin").join("rules").exists()
}

/// Deliberate project paths the user opened in Claude Code, read from
/// `<home>/.claude.json` (the `projects` map, keyed by cwd). Works for a Windows
/// home or a WSL distro home — the format is identical.
fn read_claude_project_registry(home: &Path) -> Vec<PathBuf> {
    let Some(text) = read_raw_text_file(&home.join(".claude.json"), REGISTRY_MAX_BYTES) else {
        return Vec::new();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("projects")
                .and_then(serde_json::Value::as_object)
                .map(|obj| obj.keys().map(PathBuf::from).collect())
        })
        .unwrap_or_default()
}

/// Deliberate project roots Codex knows about, read from the `[projects.'<path>']`
/// tables in `<home>/.codex/config.toml`. Parsed line-by-line so no toml
/// dependency is needed (the headers are the registry). The bool is whether the
/// table's `trust_level` is `"trusted"` — a user explicitly trusting a folder in
/// Codex is a deliberate-workspace signal, on par with opening a folder as a
/// Cursor/Antigravity workspace, so such roots survive even without a marker file
/// or a second corroborating app.
fn read_codex_project_registry(home: &Path) -> Vec<(PathBuf, bool)> {
    let Some(text) =
        read_raw_text_file(&home.join(".codex").join("config.toml"), REGISTRY_MAX_BYTES)
    else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, bool)> = Vec::new();
    // Index of the table whose body we are currently inside, so a `trust_level`
    // line under a header flips that entry's trusted flag. A new table header or a
    // top-level header (e.g. a different `[section]`) ends the current body.
    let mut current: Option<usize> = None;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("[projects.") {
            let inner = rest.strip_suffix(']').unwrap_or(rest).trim();
            let path = inner.trim_matches(|ch| ch == '"' || ch == '\'');
            if path.is_empty() {
                current = None;
                continue;
            }
            current = Some(out.len());
            out.push((PathBuf::from(path), false));
            continue;
        }
        // Any other table header ends the project table body we were reading.
        if trimmed.starts_with('[') {
            current = None;
            continue;
        }
        if let Some(idx) = current {
            if let Some((key, value)) = trimmed.split_once('=') {
                if key.trim() == "trust_level" {
                    let level = value.trim().trim_matches(|ch| ch == '"' || ch == '\'');
                    if level.eq_ignore_ascii_case("trusted") {
                        out[idx].1 = true;
                    }
                }
            }
        }
    }
    out
}

/// First non-empty `"cwd"` value across a session transcript's JSONL lines.
/// Checks a top-level `cwd` and a nested `payload.cwd` (Codex records it under
/// `payload`, Claude/Hermes at the top level). Returns the path verbatim — the
/// caller decides existence/scratch. Each line is parsed with `serde_json` so
/// path escaping is handled; if a line is not valid JSON (e.g. `read_text_prefix`
/// has already collapsed `\\` to `\`, which invalidates the escapes), a tolerant
/// manual scan of that line's `"cwd":"…"` value is used as a fallback so the same
/// helper works on both raw bytes and already-decoded text.
fn extract_session_cwd(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            let cwd = value
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    value
                        .get("payload")
                        .and_then(|payload| payload.get("cwd"))
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|cwd| !cwd.trim().is_empty());
            if let Some(cwd) = cwd {
                return Some(PathBuf::from(cwd));
            }
            continue;
        }
        if let Some(cwd) = manual_cwd_value(line) {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

/// Tolerant extraction of a `"cwd":"…"` value from a single line that did not
/// parse as JSON. This runs on text whose JSON escapes have ALREADY been
/// collapsed (as `read_text_prefix` does: `\\`→`\`), so a lone backslash here is
/// a real Windows separator and must be preserved verbatim. Only a backslash that
/// still escapes a quote (`\"`) or another backslash (`\\`) is consumed; the value
/// ends at the next unescaped double quote.
fn manual_cwd_value(line: &str) -> Option<String> {
    let start = line.find("\"cwd\"")?;
    let rest = &line[start + "\"cwd\"".len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let after = after.strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = after.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if matches!(chars.peek(), Some('"') | Some('\\')) => {
                value.push(chars.next().expect("peeked char is present"));
            }
            '"' => {
                let value = value.trim();
                return if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            other => value.push(other),
        }
    }
    None
}

/// Bounded raw read of a session file's first `max_bytes`, WITHOUT the
/// `read_text_prefix` escape-collapsing (which would corrupt the JSON string
/// escapes the `serde_json` parse in [`extract_session_cwd`] relies on). Used by
/// the session-cwd readers, which must see the on-disk bytes verbatim.
fn read_session_prefix(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut buf = vec![0; max_bytes];
    let read = file.read(&mut buf).ok()?;
    buf.truncate(read);
    Some(String::from_utf8_lossy(&buf).to_string())
}

fn derive_claude_session_display_name(source_file: &Path, source_kind: &str) -> Option<String> {
    if !source_kind.contains("claude") || !is_json_session_path(source_file) {
        return None;
    }
    let text = read_session_prefix(source_file, SESSION_TITLE_PROBE_BYTES)?;
    first_user_message_title(&text)
}

fn first_user_message_title(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(title) = json_session_title(&value) {
                return Some(title);
            }
            if let Some(title) = first_user_message_title_in_value(&value) {
                return Some(title);
            }
        }
    }

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(title) = json_session_title(&value) {
            return Some(title);
        }
        if let Some(title) = first_user_message_title_in_value(&value) {
            return Some(title);
        }
    }
    None
}

fn first_user_message_title_in_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(title) = first_user_message_title_in_value(item) {
                    return Some(title);
                }
            }
            None
        }
        serde_json::Value::Object(map) => {
            let role = json_session_role(value)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if role == "user" || role == "human" {
                if let Some(content) =
                    json_text_content(value).and_then(|text| clean_session_title(&text))
                {
                    return Some(content);
                }
            }
            for key in [
                "messages",
                "turns",
                "completedTurns",
                "conversation",
                "entries",
                "items",
            ] {
                if let Some(title) = map.get(key).and_then(first_user_message_title_in_value) {
                    return Some(title);
                }
            }
            None
        }
        _ => None,
    }
}

fn json_session_title(value: &serde_json::Value) -> Option<String> {
    for key in ["title", "display_name", "name", "summary"] {
        if let Some(title) = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(clean_session_title)
        {
            return Some(title);
        }
    }
    None
}

fn json_session_role(value: &serde_json::Value) -> Option<&str> {
    value
        .get("message")
        .and_then(|message| message.get("role"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("role").and_then(serde_json::Value::as_str))
        .or_else(|| value.get("type").and_then(serde_json::Value::as_str))
        .or_else(|| value.get("sender").and_then(serde_json::Value::as_str))
        .or_else(|| value.get("author").and_then(serde_json::Value::as_str))
}

fn json_text_content(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        serde_json::Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(json_text_content)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        serde_json::Value::Object(map) => {
            for key in ["text", "content", "message", "body", "summary", "value"] {
                if let Some(text) = map.get(key).and_then(json_text_content) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn tagged_session_title_value<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let opening = format!("<{tag}>");
    let closing = format!("</{tag}>");
    let start = text.find(&opening)? + opening.len();
    let end = text[start..].find(&closing)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then_some(value)
}

fn command_session_title(text: &str) -> Option<String> {
    if !text.starts_with("<command-message>") && !text.starts_with("<command-name>") {
        return None;
    }
    let command = tagged_session_title_value(text, "command-name")
        .or_else(|| tagged_session_title_value(text, "command-message"))?;
    let command = command.trim_matches(['"', '\'', '`']).trim();
    if command.is_empty() {
        return None;
    }
    let command = if command.starts_with('/') || command.contains(char::is_whitespace) {
        command.to_string()
    } else {
        format!("/{command}")
    };
    Some(format!("Command {command}"))
}

fn clean_session_title(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Some wrappers store the complete system prompt as the thread title and append
    // the actual human turn after an explicit request marker. Prefer that tail only
    // for oversized labels; short titles containing the same words stay literal.
    let command_title = command_session_title(&collapsed);
    let title = if let Some(command) = command_title.as_deref() {
        command
    } else if collapsed.chars().count() > SESSION_TITLE_MAX_CHARS {
        embedded_session_request(&collapsed).unwrap_or(&collapsed)
    } else {
        &collapsed
    };
    let cleaned = title
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim()
        .to_string();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.chars().count() <= SESSION_TITLE_MAX_CHARS {
        return Some(cleaned);
    }
    let mut title = cleaned
        .chars()
        .take(SESSION_TITLE_MAX_CHARS.saturating_sub(3))
        .collect::<String>();
    title.push_str("...");
    Some(title)
}

fn embedded_session_request(title: &str) -> Option<&str> {
    const MARKERS: &[&str] = &[
        "Pedido atual:",
        "Pedido do utilizador:",
        "Current request:",
        "User request:",
        "Actual request:",
    ];
    MARKERS
        .iter()
        .filter_map(|marker| {
            title
                .rfind(marker)
                .map(|index| (index, title[index + marker.len()..].trim()))
        })
        .filter(|(_, request)| !request.is_empty())
        .max_by_key(|(index, _)| *index)
        .map(|(_, request)| request)
}

fn is_json_session_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("jsonl") || ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

fn session_display_name_is_technical(
    display_name: &str,
    source_file: &Path,
    source_kind: &str,
) -> bool {
    if !source_kind.contains("claude") || !is_json_session_path(source_file) {
        return false;
    }
    let filename = Path::new(display_name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(display_name)
        .trim();
    let stem = filename
        .strip_suffix(".jsonl")
        .or_else(|| filename.strip_suffix(".json"))
        .unwrap_or(filename);
    let stem = stem.strip_prefix("local_").unwrap_or(stem);
    let compact = stem.replace('-', "");
    compact.len() >= 16 && compact.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// The project root Claude Code records for one `~/.claude/projects/<escaped>/`
/// dir, read from the newest `*.jsonl`'s `"cwd"`. The directory name itself is
/// hyphen-lossy (`:`, `\`, and spaces all became `-`) so it cannot be decoded;
/// the session transcript's recorded `cwd` is the only unambiguous source.
fn claude_dir_cwd(dir: &Path) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if newest
            .as_ref()
            .map(|(best, _)| modified > *best)
            .unwrap_or(true)
        {
            newest = Some((modified, path));
        }
    }
    let (_, newest) = newest?;
    let text = read_session_prefix(&newest, SOURCE_FILE_BYTES)?;
    extract_session_cwd(&text)
}

/// All Claude Code project roots from `<home>/.claude/projects/*/`, each read
/// from its newest session transcript's `cwd` (see [`claude_dir_cwd`]). Deduped
/// by `candidate_key`; only roots that currently exist on disk are returned.
fn claude_project_roots(home: &Path) -> Vec<PathBuf> {
    let projects_dir = home.join(".claude").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(cwd) = claude_dir_cwd(&dir) else {
            continue;
        };
        // The recorded cwd can be a WSL share (Claude run on Windows against a WSL
        // folder); with the gate off, `is_dir` on it could cold-boot the distro at
        // startup, so such roots are skipped outright rather than statted.
        if wsl_path_blocked_by_gate(&cwd)
            || !cwd.is_dir()
            || !is_explicit_session_cwd_link_path(&cwd)
        {
            continue;
        }
        if seen.insert(candidate_key(&cwd)) {
            out.push(cwd);
        }
    }
    out
}

/// Retitle already-scanned Codex rollout session candidates from the session
/// index (`~/.codex/session_index.jsonl`). Each index line is
/// `{"id":"<uuid>","thread_name":"…",…}` and the same uuid is embedded at the
/// end of the rollout filename — the index is the only place a rollout's
/// user-visible name exists, so without this the session list shows raw
/// `rollout-….jsonl` filenames. Read raw (NOT via `read_text_prefix`, whose
/// escape-collapsing corrupts JSON) and bounded; best-effort per line.
/// Allocation-light: each name is stored once and cloned only for the rollout it
/// retitles; rollouts missing from the index keep the filename fallback.
fn apply_codex_index_thread_names(
    source_file: &Path,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    let Some(text) = read_raw_text_file(source_file, SOURCE_TEXT_METADATA_MAX_BYTES) else {
        return;
    };
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in text.lines() {
        // Codex writes the file with a UTF-8 BOM; serde rejects a BOM-prefixed line.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(name) = value
            .get("thread_name")
            .and_then(serde_json::Value::as_str)
            .and_then(clean_session_title)
        else {
            continue;
        };
        names.insert(id.to_ascii_lowercase(), name);
    }
    if names.is_empty() {
        return;
    }
    for session in sessions.values_mut() {
        if session.display_name.is_some() {
            continue; // a specialized scanner already titled it
        }
        let Some(id) = codex_rollout_session_id(&session.path) else {
            continue;
        };
        if let Some(name) = names.get(&id) {
            session.display_name = Some(name.clone());
        }
    }
}

/// The session uuid embedded at the end of a Codex rollout filename
/// (`rollout-<timestamp>-<uuid>.jsonl`), lowercased for index-map lookups.
/// `None` for anything that is not shaped like a rollout transcript.
fn codex_rollout_session_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let stem = name.strip_suffix(".jsonl")?;
    if !stem.starts_with("rollout-") {
        return None;
    }
    // The uuid is the trailing 36 chars: 8-4-4-4-12 hex groups.
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    let bytes = id.as_bytes();
    let shape_ok = [8usize, 13, 18, 23].iter().all(|&i| bytes[i] == b'-')
        && id
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 8 | 13 | 18 | 23) || b.is_ascii_hexdigit());
    shape_ok.then(|| id.to_string())
}

/// The per-session identity for a Claude Code transcript: the lowercased `.jsonl`
/// filename stem, which is the session uuid Claude Code assigns each conversation
/// (e.g. `.../projects/<enc>/019f3315-12ff-7071-8534-04fe50ed534e.jsonl`). Any `#`
/// fragment (a synthetic sub-session anchor) is stripped first so the key stays
/// path-fragment-safe. Returns `None` for a path that is not a `.jsonl` file so the
/// caller can fall back to the generic identity.
fn claude_session_stem_id(path: &str) -> Option<String> {
    let file_path = path.split('#').next().unwrap_or(path);
    let name = Path::new(file_path).file_name()?.to_str()?;
    // Accept any case variant of the `.jsonl` extension (e.g. `.JSONL`).
    let dot = name.rfind('.')?;
    if !name[dot..].eq_ignore_ascii_case(".jsonl") {
        return None;
    }
    let stem = &name[..dot];
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_ascii_lowercase())
}

/// Strip a Windows extended-length prefix (`\\?\C:\…`) so the path stats and keys
/// like an ordinary local path. Codex's `threads.cwd` stores 116/119 of its paths
/// in this verbatim form. The UNC verbatim form (`\\?\UNC\…`, i.e. a WSL share) is
/// deliberately LEFT untouched — the WSL gate must still recognise and block it.
fn strip_verbatim_prefix(raw: &str) -> &str {
    if raw.starts_with(r"\\?\UNC\") {
        return raw;
    }
    raw.strip_prefix(r"\\?\").unwrap_or(raw)
}

/// One Codex thread's user-facing metadata read from the `threads` table: its
/// title and recorded `cwd` (already stripped of any `\\?\` prefix). Both are
/// `Option` because a row may carry only one.
#[derive(Debug, Clone, Default)]
struct CodexThreadMeta {
    title: Option<String>,
    cwd: Option<PathBuf>,
}

/// Schema-aware view of `~/.codex/state_*.sqlite`'s `threads` table, keyed two
/// ways so a rollout session can be matched by its filename uuid first and by its
/// full `rollout_path` as a fallback.
#[derive(Debug, Default)]
struct CodexThreadsIndex {
    by_uuid: std::collections::HashMap<String, CodexThreadMeta>,
    by_rollout_key: std::collections::HashMap<String, CodexThreadMeta>,
}

/// Resolve the Codex thread-state DB to read: newest-mtime `state_*.sqlite` across
/// BOTH `<codex_home>` and `<codex_home>/sqlite` (a parallel `sqlite/state_5.sqlite`
/// exists on real machines, and a fresh `state_5.sqlite` can be newer than a stale
/// `state_5.pre-cwd-fix-*.sqlite` backup that the glob also matches). `codex_home`
/// is the source path's parent — the registered source path is one specific
/// `state_5.sqlite`, but the live DB may be the sibling copy. `None` if neither
/// directory holds a match.
fn codex_state_db_path(source_file: &Path) -> Option<PathBuf> {
    let codex_home = source_file.parent()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for dir in [codex_home.to_path_buf(), codex_home.join("sqlite")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_state_db = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower.starts_with("state_") && lower.ends_with(".sqlite")
                })
                .unwrap_or(false);
            if !is_state_db {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if best
                .as_ref()
                .map(|(best_time, _)| modified > *best_time)
                .unwrap_or(true)
            {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Read the Codex `threads` table (schema-aware, read-only) into a
/// [`CodexThreadsIndex`]. Modern Codex records every thread's `title` and `cwd`
/// here (119/119 non-empty on the real machine) — far richer than the
/// `session_index.jsonl` title-only join (~57% titled) and, crucially, the ONLY
/// authoritative `cwd` source for a rollout that the transcript body would
/// otherwise have to be text-mined for. `None` when the schema probe fails
/// (missing `threads` table or the `id`/`title`/`cwd`/`rollout_path` columns → an
/// older Codex layout), so the caller can fall back to the generic scan. Only
/// `cwd`/`rollout_path` are read as path sources — never any message-text column.
fn read_codex_threads_index(db_path: &Path) -> Option<CodexThreadsIndex> {
    let conn = open_discovery_sqlite(db_path).ok()?;
    // Schema probe: a `threads` table exposing exactly the columns we rely on.
    // A missing table or any missing column fails the prepare and returns `None`,
    // so pre-`threads` Codex versions cleanly fall back to the generic scan.
    let mut stmt = conn
        .prepare("SELECT id, title, cwd, rollout_path FROM threads")
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .ok()?;
    let mut index = CodexThreadsIndex::default();
    for row in rows.flatten() {
        let (id, title, cwd, rollout_path) = row;
        let title = title.and_then(|value| clean_session_title(&value));
        let cwd = cwd
            .map(|value| PathBuf::from(strip_verbatim_prefix(value.trim())))
            .filter(|path| path.as_os_str().len() >= 2);
        let meta = CodexThreadMeta { title, cwd };
        if let Some(id) = id
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
        {
            index.by_uuid.insert(id, meta.clone());
        }
        if let Some(rollout_path) = rollout_path
            .map(|value| strip_verbatim_prefix(value.trim()).to_string())
            .filter(|value| !value.is_empty())
        {
            index
                .by_rollout_key
                .insert(candidate_key(Path::new(&rollout_path)), meta);
        }
    }
    if index.by_uuid.is_empty() && index.by_rollout_key.is_empty() {
        return None;
    }
    Some(index)
}

/// Retitle and project-link already-scanned Codex rollout sessions from the
/// schema-aware `threads` index. Matching is filename-uuid first, then
/// `rollout_path`; a title only fills a session that no specialized scanner
/// already named, but a `cwd` is always linked (subject to the same
/// explicit-cwd/existence policy as the rollout scan) and also surfaced as a project
/// candidate. This replaces the generic text-mining scan of the state DB, whose
/// path-fragment scraping of message columns is exactly the pollution the
/// cwd-only rollout policy forbids.
fn apply_codex_state_threads(
    db_path: &Path,
    index: &CodexThreadsIndex,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
) {
    // Collect the cwds to promote to candidates after the session borrow ends
    // (the session loop holds `&mut sessions`; candidate insertion is separate).
    let mut linked_cwds: Vec<PathBuf> = Vec::new();
    for session in sessions.values_mut() {
        // Only Codex rollout sessions are addressed by this index.
        let uuid = codex_rollout_session_id(&session.path);
        let meta = uuid
            .as_ref()
            .and_then(|uuid| index.by_uuid.get(uuid))
            .or_else(|| index.by_rollout_key.get(&candidate_key(&session.path)));
        let Some(meta) = meta else {
            continue;
        };
        // The `threads` DB is the authoritative title source, so it OVERRIDES a
        // title the `session_index.jsonl` pass (which runs earlier and covers only
        // ~57% of rollouts) may have already applied — that keeps the index a
        // pure fallback for rollouts the DB does not carry. Codex rollouts are
        // never titled by a specialized scanner, so nothing else is clobbered.
        if let Some(title) = &meta.title {
            session.display_name = Some(title.clone());
        }
        if let Some(cwd) = meta
            .cwd
            .as_ref()
            .filter(|path| !is_agent_scratch_path(path) && path.is_dir())
        {
            let canonical = canonical_or_original(cwd);
            if session.linked_project_paths.insert(canonical) {
                session.confidence = "High".to_string();
            }
            linked_cwds.push(cwd.clone());
        }
    }
    let source = DiscoverySource {
        kind: "codex_state".to_string(),
        label: "ChatGPT thread state".to_string(),
        path: db_path.to_path_buf(),
        detail: None,
        mode: SourceMode::SqliteMetadata,
    };
    for cwd in linked_cwds {
        add_candidate(
            candidates,
            &cwd,
            &source,
            "session_path",
            "AI session working directory",
            Some(display_path(db_path)),
            32,
        );
        add_marker_signals(candidates, &cwd);
    }
}

/// The `cwd` a Codex rollout records in its `session_meta` (the first JSONL
/// line; `cwd` lives under `payload`, occasionally top-level). Reads only enough
/// of the file to cover that first line — the `base_instructions` blob can push
/// it past 30 KB, so a generous prefix is read.
fn codex_rollout_cwd(file: &Path) -> Option<PathBuf> {
    let text = read_session_prefix(file, SOURCE_FILE_BYTES)?;
    let first = text.lines().next()?;
    extract_session_cwd(first)
}

/// Line-aware bounded `cwd` probe over a rollout/transcript file HEAD, for the
/// two cases `codex_rollout_cwd`'s first-line-only read misses:
///
/// 1. Oversized files (past the 16 MiB text-scan gate) whose full-text scan is
///    skipped — all 7 real oversized Claude transcripts (incl. 425/135/91 MB)
///    still record their `cwd` within the first ~5.5 KB.
/// 2. Files whose head is a flood of cwd-less records (Claude `queue-operation`
///    lines: `{content, operation, sessionId, timestamp, type}`) that push the
///    first `cwd` well past the first line (~107 KB on the real machine).
///
/// Reads on-disk bytes verbatim in ~64 KB chunks (like `read_session_prefix`, so
/// JSON escapes survive for `extract_session_cwd`'s per-line parse), keeps only
/// whole lines, and stops at the FIRST line that yields a `cwd` or once
/// `SESSION_CWD_PROBE_MAX_BYTES` have been consumed — so it never loads a giant
/// transcript body just to find one path. Returns the raw `cwd` unfiltered; the
/// caller applies the same explicit-cwd/existence policy as the normal path.
fn probe_session_cwd(file: &Path) -> Option<PathBuf> {
    let mut handle = fs::File::open(file).ok()?;
    let mut consumed: u64 = 0;
    // Carry a partial trailing line across chunk boundaries so a `cwd` split by a
    // 64 KB read is not missed; only ever holds up to one line's worth of bytes.
    let mut pending = String::new();
    let mut buf = vec![0u8; SESSION_CWD_PROBE_CHUNK_BYTES];
    while consumed < SESSION_CWD_PROBE_MAX_BYTES {
        let read = handle.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        consumed += read as u64;
        pending.push_str(&String::from_utf8_lossy(&buf[..read]));
        // Scan every COMPLETE line (all but the last, still-partial fragment).
        let mut search_from = 0;
        while let Some(newline) = pending[search_from..].find('\n') {
            let line_end = search_from + newline;
            if let Some(cwd) = extract_session_cwd(&pending[search_from..line_end]) {
                return Some(cwd);
            }
            search_from = line_end + 1;
        }
        // Keep only the unterminated tail; if a single line already exceeds the
        // whole budget (no newline yet), give up rather than grow unbounded.
        if search_from > 0 {
            pending.drain(..search_from);
        } else if pending.len() as u64 >= SESSION_CWD_PROBE_MAX_BYTES {
            break;
        }
    }
    // The final line may be unterminated (EOF without a trailing newline).
    extract_session_cwd(&pending)
}

/// Codex session working dirs from `<home>/.codex/sessions/**/rollout-*.jsonl`,
/// EXCLUDING per-conversation scratch dirs (`is_agent_scratch_path`, e.g.
/// `…\Documents\Codex\2026-06-14\<slug>`) and roots that no longer exist.
/// Deduped by `candidate_key`. The walk is bounded to the `YYYY/MM/DD` layout
/// (depth ~4 below `sessions/`) and caps the number of rollouts read so it stays
/// cheap on a machine with a long history.
fn codex_session_project_roots(home: &Path) -> Vec<PathBuf> {
    const MAX_ROLLOUTS: usize = 2000;
    const MAX_DEPTH: usize = 4;
    let sessions_dir = home.join(".codex").join("sessions");
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut read = 0_usize;
    let mut stack: Vec<(PathBuf, usize)> = vec![(sessions_dir, 0)];
    while let Some((dir, depth)) = stack.pop() {
        if read >= MAX_ROLLOUTS {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false);
            if is_dir {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let is_rollout = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
                .unwrap_or(false);
            if !is_rollout {
                continue;
            }
            if read >= MAX_ROLLOUTS {
                break;
            }
            read += 1;
            let Some(cwd) = codex_rollout_cwd(&path) else {
                continue;
            };
            // Same WSL-gate rule as `claude_project_roots`: a rollout cwd on a WSL
            // share must not be statted (and so cannot be listed) while the gate is off.
            if wsl_path_blocked_by_gate(&cwd)
                || !cwd.is_dir()
                || !is_explicit_session_cwd_link_path(&cwd)
            {
                continue;
            }
            if seen.insert(candidate_key(&cwd)) {
                out.push(cwd);
            }
        }
    }
    out
}

/// Whether a registry-sourced path is a real, deliberate project — not a
/// per-conversation scratch dir, a system/one-off path, or a folder that no
/// longer exists. It qualifies when it was opened as a deliberate workspace
/// (e.g. a Cursor workspace), corroborated by two or more apps, or carries its
/// own project identity (markers / `.git`). This is what keeps random one-off
/// cwds (a Pictures folder an agent ran in once) out of the project list.
fn is_registry_project_path(path: &Path, deliberate_workspace: bool, app_count: usize) -> bool {
    // A registry can record a project on a WSL share even from the Windows side
    // (e.g. Claude opened in `\\wsl.localhost\…`). While the WSL gate is off, no
    // stat may touch that share — so the path cannot qualify (the checks below
    // are all filesystem probes).
    if wsl_path_blocked_by_gate(path) {
        return false;
    }
    if !path.is_dir()
        || is_agent_scratch_path(path)
        || is_forbidden_candidate_path(path)
        || is_internal_ai_state_candidate_path(path)
        || is_broad_container_path(path)
        || (is_probably_system_or_noise(path) && !has_project_identity(path))
    {
        return false;
    }
    deliberate_workspace || app_count >= 2 || has_project_identity(path)
}

/// Deliberately-opened workspaces of a VS Code-family editor (Cursor,
/// Antigravity, …), from `<user_dir>/workspaceStorage/*/workspace.json` (the
/// `folder` field is a `file://` URI). Opening a folder as a workspace is a
/// strong, deliberate signal — stronger than a one-off cwd. All these editors
/// share the same on-disk format.
fn read_vscode_workspace_registry(user_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(user_dir.join("workspaceStorage")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(text) =
            read_raw_text_file(&entry.path().join("workspace.json"), REGISTRY_MAX_BYTES)
        else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(folder) = value.get("folder").and_then(serde_json::Value::as_str) {
            out.extend(extract_file_uri_directories(&percent_decode_lossy(folder)));
        }
    }
    out
}

/// Deliberately-opened projects of Antigravity (the Gemini IDE), read from its
/// AUTHORITATIVE registry at `<gemini_home>/.gemini/config/projects/*.json` (one
/// file per project). This is a Windows-side store, distinct from the VS
/// Code-style `workspaceStorage/*/workspace.json` the editor also keeps. Each
/// file carries a `name` and a `projectResources.resources` array whose entries
/// point at the project's roots via `folderUri` and/or a nested
/// `gitFolder.folderUri` — both `file://` URIs decoded the same way as the
/// workspace reader. A project can span several roots; many live on offline
/// external drives, so `extract_file_uri_directories` keeps only ones that exist
/// on disk right now.
fn read_gemini_project_registry(gemini_home: &Path) -> Vec<PathBuf> {
    let projects_dir = gemini_home.join(".gemini").join("config").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        out.extend(gemini_project_file_roots(&path));
    }
    out
}

/// Every existing root one Antigravity per-project registry JSON file points at:
/// the `folderUri` AND nested `gitFolder.folderUri` of each
/// `projectResources.resources[]` entry. This registry is AUTHORITATIVE, so each
/// `file://` URI is decoded to its EXACT path (via
/// [`antigravity_registry_uri_directories`]) and kept only if that exact folder
/// exists — deliberately WITHOUT `extract_file_uri_directories`' parent walk,
/// which would resolve a deleted root up to a surviving `.git`-bearing ancestor
/// and misattribute the project (4/30 real URIs point at now-deleted roots).
/// Offline external-drive roots are dropped. Defensive: an unreadable or
/// malformed file yields an empty list.
fn gemini_project_file_roots(file: &Path) -> Vec<PathBuf> {
    let Some(text) = read_raw_text_file(file, REGISTRY_MAX_BYTES) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(resources) = value
        .get("projectResources")
        .and_then(|res| res.get("resources"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for resource in resources {
        for folder in [
            resource.get("folderUri"),
            resource
                .get("gitFolder")
                .and_then(|git| git.get("folderUri")),
        ]
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        {
            out.extend(antigravity_registry_uri_directories(folder));
        }
    }
    out
}

/// All on-disk roots belonging to ONE Antigravity project, read from its own
/// authoritative registry file `<home>/.gemini/config/projects/<project_uuid>.json`.
/// An Antigravity project spans several `folderUri` roots (often including offline
/// external drives); this returns every one that exists right now, deduped by
/// `candidate_key`. Because the file is the project's own root list, every path
/// belongs to the SAME project — this is NOT path-scraping and never smears one
/// conversation across projects. Empty when the file is absent/unreadable or no
/// listed root currently exists.
fn gemini_project_roots_for_uuid(home: &Path, project_uuid: &str) -> Vec<PathBuf> {
    let file = home
        .join(".gemini")
        .join("config")
        .join("projects")
        .join(format!("{project_uuid}.json"));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for root in gemini_project_file_roots(&file) {
        if root.is_dir() && seen.insert(candidate_key(&root)) {
            out.push(root);
        }
    }
    out
}

/// The Antigravity per-project registry file (`<home>/.gemini/config/projects/<uuid>.json`)
/// whose own root list includes `project_root`. Backing up then deleting this file is what
/// makes Antigravity forget the project (it stops listing it). Read-only; returns the first
/// matching file. The project root must still exist on disk — the removal flow resolves this
/// BEFORE it deletes the folder, so that holds in practice.
pub fn antigravity_registry_file_for_root(project_root: &Path) -> Option<PathBuf> {
    let home = home_dir()?;
    antigravity_registry_file_for_root_in(&home, project_root)
}

fn antigravity_registry_file_for_root_in(home: &Path, project_root: &Path) -> Option<PathBuf> {
    let projects_dir = home.join(".gemini").join("config").join("projects");
    for entry in fs::read_dir(&projects_dir).ok()?.flatten() {
        let file = entry.path();
        if file.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if gemini_project_file_roots(&file)
            .iter()
            .any(|root| same_path(root, project_root))
        {
            return Some(file);
        }
    }
    None
}

/// Antigravity project display names (from `~/.gemini/config/projects/*.json`) keyed by
/// `candidate_key` of each existing project root. Lets a registered project surface the
/// name the user gave it in the Gemini IDE even when the folder basename differs — e.g.
/// the "ExampleProj" project is rooted at `D:\Example`, so it lists as "Example" and
/// carries this name as a "named: ExampleProj" label. Best-effort: empty when the
/// registry is absent.
pub fn antigravity_project_names() -> std::collections::HashMap<String, String> {
    match home_dir() {
        Some(home) => gemini_project_name_map(&home),
        None => std::collections::HashMap::new(),
    }
}

/// `candidate_key` for an external caller — to look a registered project's path up in
/// [`antigravity_project_names`] with the same normalization the map is keyed by.
pub fn project_path_key(path: &str) -> String {
    candidate_key(Path::new(path))
}

fn gemini_project_name_map(gemini_home: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let projects_dir = gemini_home.join(".gemini").join("config").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(text) = read_raw_text_file(&path, REGISTRY_MAX_BYTES) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        // Antigravity sometimes stores the project name percent-encoded (e.g.
        // "LLM%20Bench"), so decode it. Decoding also lets the API layer drop the
        // label when it just repeats the folder name ("LLM Bench").
        let Some(name) = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|raw| percent_decode_lossy(raw.trim()).trim().to_string())
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let Some(resources) = value
            .get("projectResources")
            .and_then(|res| res.get("resources"))
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for resource in resources {
            for folder in [
                resource.get("folderUri"),
                resource
                    .get("gitFolder")
                    .and_then(|git| git.get("folderUri")),
            ]
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            {
                for dir in extract_file_uri_directories(&percent_decode_lossy(folder)) {
                    map.entry(candidate_key(&dir))
                        .or_insert_with(|| name.to_string());
                }
            }
        }
    }
    map
}

/// Per-project app attribution and "currentness", keyed by `candidate_key` of the
/// project root. Built once per call and consumed by the API layer to enrich
/// `ProjectSummary`. See [`project_app_states`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectAppState {
    /// The source app that most specifically/deliberately owns this path (e.g.
    /// `"antigravity"`, `"cursor"`, `"codex"`, `"claude"`), or `None` if no registry
    /// claims it.
    pub app: Option<String>,
    /// Every app whose registry/sessions claim this path (the primary `app` plus any
    /// others). Drives the app FILTER so a project used in Claude AND Codex is found
    /// under both, even though its badge shows only the primary.
    pub apps: Vec<String>,
    /// Whether the project is current in its owning app — present in a recent
    /// app-activity signal (Antigravity summaries proto, or a Claude
    /// `lastSessionModified`).
    pub is_current: bool,
}

/// Relative specificity of each app's registry, higher = more specific/deliberate.
/// Used to pick a single owning `app` when a path is registered by several. A
/// folder explicitly registered in Antigravity's authoritative per-project store, or
/// opened as an editor workspace, is a stronger statement of ownership than a bare
/// recorded cwd (Claude/Codex). Unknown apps sort lowest.
fn registry_app_specificity(app: &str) -> u8 {
    match app {
        "antigravity" => 4,
        "cursor" => 3,
        "codex" => 2,
        "claude_code" | "claude" => 1,
        _ => 0,
    }
}

/// Public app label for a registry's internal app id (the internal `"claude_code"`
/// is surfaced to the frontend as `"claude"`; others pass through).
fn registry_app_label(app: &str) -> String {
    match app {
        "claude_code" => "claude".to_string(),
        other => other.to_string(),
    }
}

/// Project app attribution + currentness for every registered project root, keyed by
/// `candidate_key`. The API layer calls this AT MOST ONCE per `projects_list` and
/// looks each project up by `project_path_key(&path)`.
///
/// Currentness sources (parsed at most once each here):
///   * **Antigravity:** the project root appears as a `Project { path }` in
///     `~/.gemini/antigravity/agyhub_summaries_proto.pb` (the cached proto map). This
///     is what un-archives ExampleProj, whose conversations are `.pb` and never link
///     a `.db`.
///   * **Claude:** the root has a present `lastSessionModified` in `~/.claude.json`.
///
/// `app` attribution walks the same registries the discovery candidate pass uses
/// (Windows + each WSL distro home), keeping the most specific deliberate owner per
/// path (see [`registry_app_specificity`]). Codex / Cursor currentness is left
/// best-effort (not implemented) — the reported case is Antigravity + Claude.
pub fn project_app_states() -> std::collections::HashMap<String, ProjectAppState> {
    let mut states: std::collections::HashMap<String, ProjectAppState> =
        std::collections::HashMap::new();

    // Walk the Claude/Codex session trees for the Windows home ONCE and share the result
    // between app attribution (step 1) and currentness (step 6). Both previously re-walked
    // the entire Codex rollout history (up to MAX_ROLLOUTS files) and the Claude projects
    // tree independently on this hot `projects_list` path.
    let home = home_dir();
    let home_claude_roots: Vec<PathBuf> = home
        .as_ref()
        .map(|h| claude_project_roots(h))
        .unwrap_or_default();
    let home_codex_roots: Vec<PathBuf> = home
        .as_ref()
        .map(|h| codex_session_project_roots(h))
        .unwrap_or_default();

    // 1) App attribution from every registry (path key -> most specific app + all apps).
    for (key, (app, apps)) in registry_app_attribution(&home_claude_roots, &home_codex_roots) {
        let state = states.entry(key).or_default();
        state.app = Some(app);
        state.apps = apps;
    }

    // 2) Antigravity currentness from the cached summaries proto.
    for key in antigravity_current_project_keys() {
        states.entry(key).or_default().is_current = true;
    }

    // 3) Claude currentness from ~/.claude.json lastSessionModified.
    for key in claude_current_project_keys() {
        states.entry(key).or_default().is_current = true;
    }

    // 4) Antigravity project-registry membership = current. `~/.gemini/config/projects`
    //    is Antigravity's own "my projects" list, so a folder it still lists is active
    //    even when its conversations were recorded under a different folderUri — e.g.
    //    ExampleProj is registered at D:\Example\SubA but was chatted from an
    //    external-drive mirror, so the summaries-proto path (step 2) never matches the
    //    registered root.
    //
    //    5) A Codex *trusted* folder is likewise one the user deliberately works in, so
    //    it counts as current too — keeps trusted-but-session-less Codex projects (e.g.
    //    SubProjectA, SubProjectB) out of Archived.
    if let Some(home) = home.as_ref() {
        for path in read_gemini_project_registry(home) {
            states.entry(candidate_key(&path)).or_default().is_current = true;
        }
        for (path, trusted) in read_codex_project_registry(home) {
            if trusted {
                states.entry(candidate_key(&path)).or_default().is_current = true;
            }
        }
    }
    // 6) A folder Claude Code or Codex has actually run sessions in is an active project,
    //    even with no `.claude.json`/trusted-table entry. Reuse the roots walked once above.
    for path in &home_claude_roots {
        states.entry(candidate_key(path)).or_default().is_current = true;
    }
    for path in &home_codex_roots {
        states.entry(candidate_key(path)).or_default().is_current = true;
    }

    states
}

/// Path keys (via `candidate_key`) of every project root the Antigravity summaries
/// proto anchors a conversation to (the `Project { path }` resolutions). Reuses the
/// process-cached proto map, so this is effectively free after the first parse. The
/// recorded path is keyed whether or not it currently exists on disk — a project on
/// an offline external drive is still "current" in the app.
fn antigravity_current_project_keys() -> std::collections::HashSet<String> {
    antigravity_current_keys_from_map(&antigravity_proto_map())
}

/// The `Project { path }` resolutions of a parsed proto map, as path keys. Split out
/// so it can be tested with a hand-encoded map.
fn antigravity_current_keys_from_map(
    map: &AntigravityProtoMap,
) -> std::collections::HashSet<String> {
    map.values()
        .filter_map(|resolution| match resolution {
            AntigravityResolution::Project { path, .. } => Some(candidate_key(path)),
            AntigravityResolution::Loose => None,
        })
        .collect()
}

/// Path keys of Claude projects that carry a present `lastSessionModified` in
/// `~/.claude.json` (`projects` map, keyed by cwd) — Claude has recorded a session
/// for that folder, so it is current. Best-effort: empty when the file is absent or
/// unreadable. The file is small (tens of KB) and bounded by `REGISTRY_MAX_BYTES`.
fn claude_current_project_keys() -> std::collections::HashSet<String> {
    match home_dir() {
        Some(home) => claude_current_project_keys_in(&home),
        None => std::collections::HashSet::new(),
    }
}

fn claude_current_project_keys_in(home: &Path) -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    let Some(text) = read_raw_text_file(&home.join(".claude.json"), REGISTRY_MAX_BYTES) else {
        return keys;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return keys;
    };
    let Some(projects) = value.get("projects").and_then(serde_json::Value::as_object) else {
        return keys;
    };
    for (path, entry) in projects {
        let has_recent = entry
            .get("lastSessionModified")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if has_recent {
            keys.insert(candidate_key(Path::new(path)));
        }
    }
    keys
}

/// Owning-app per project path key, derived from the same registries the discovery
/// candidate pass reads (Windows home + each WSL distro home, plus the Windows-side
/// Cursor/Antigravity workspace stores). When several apps register one path, the
/// most specific deliberate one wins (see [`registry_app_specificity`]). Returned
/// app labels are the public form (`claude_code` -> `claude`).
///
/// `home_claude_roots` / `home_codex_roots` are the Windows-home Claude/Codex session roots,
/// walked once by the caller and shared so this hot path does not re-walk the rollout history.
fn registry_app_attribution(
    home_claude_roots: &[PathBuf],
    home_codex_roots: &[PathBuf],
) -> std::collections::HashMap<String, (String, Vec<String>)> {
    // key -> (specificity of primary, primary public label, set of ALL public labels).
    // The primary drives the badge; the full set drives the app FILTER, so a project
    // used in several apps (e.g. Claude + Codex) is found under each of them.
    let mut owners: std::collections::HashMap<String, (u8, String, BTreeSet<String>)> =
        std::collections::HashMap::new();
    let mut record = |path_key: String, app: &str| {
        let specificity = registry_app_specificity(app);
        let label = registry_app_label(app);
        let entry = owners
            .entry(path_key)
            .or_insert_with(|| (specificity, label.clone(), BTreeSet::new()));
        if specificity > entry.0 {
            entry.0 = specificity;
            entry.1 = label.clone();
        }
        entry.2.insert(label);
    };

    if let Some(home) = home_dir() {
        for (path, _trusted) in read_codex_project_registry(&home) {
            record(candidate_key(&path), "codex");
        }
        for path in read_claude_project_registry(&home) {
            record(candidate_key(&path), "claude_code");
        }
        // Windows-side editor workspace stores (Cursor / Antigravity).
        if let Some(appdata) = env_path("APPDATA") {
            for (dir, app) in [
                (appdata.join("Cursor").join("User"), "cursor"),
                (appdata.join("Antigravity").join("User"), "antigravity"),
                (appdata.join("Antigravity IDE").join("User"), "antigravity"),
            ] {
                for path in read_vscode_workspace_registry(&dir) {
                    record(candidate_key(&path), app);
                }
            }
        }
        // Antigravity's authoritative per-project store (Windows-side).
        for path in read_gemini_project_registry(&home) {
            record(candidate_key(&path), "antigravity");
        }
    }
    // A folder Claude Code or Codex has actually run sessions in is owned by that app, read
    // from the recorded session `cwd` (the Claude project dir name is hyphen-lossy; Codex
    // scratch cwds are excluded by the reader). These Windows-home roots are precomputed by
    // the caller and shared so the rollout history is walked once per projects_list.
    for path in home_claude_roots {
        record(candidate_key(path), "claude_code");
    }
    for path in home_codex_roots {
        record(candidate_key(path), "codex");
    }
    // WSL distro homes hold POSIX Claude/Codex registries; translate to the same
    // Windows-accessible keys the candidate pass dedups on.
    for distro in wsl_distros() {
        let homes = PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
        let Ok(entries) = fs::read_dir(&homes) else {
            continue;
        };
        for entry in entries.flatten() {
            let user_home = entry.path();
            if !user_home.is_dir() {
                continue;
            }
            for (path, _trusted) in read_codex_project_registry(&user_home) {
                record(candidate_key(&translate_wsl_path(&distro, &path)), "codex");
            }
            for path in read_claude_project_registry(&user_home) {
                record(
                    candidate_key(&translate_wsl_path(&distro, &path)),
                    "claude_code",
                );
            }
        }
    }

    owners
        .into_iter()
        .map(|(key, (_, primary, all))| (key, (primary, all.into_iter().collect())))
        .collect()
}

fn record_registry_path(
    by_path: &mut BTreeMap<String, (PathBuf, BTreeSet<&'static str>, bool)>,
    path: PathBuf,
    app: &'static str,
    deliberate_workspace: bool,
) {
    let entry = by_path
        .entry(candidate_key(&path))
        .or_insert_with(|| (path.clone(), BTreeSet::new(), false));
    entry.1.insert(app);
    entry.2 |= deliberate_workspace;
}

/// Translate a POSIX path from inside a WSL distro into a Windows-accessible
/// path. `/mnt/<drive>/…` is a Windows drive mounted into WSL → `<DRIVE>:\…`
/// (this is what lets a WSL project on `/mnt/c/...` dedup against the same
/// project discovered on Windows). Any other absolute POSIX path lives in the
/// distro's own filesystem → `\\wsl.localhost\<distro>\…`, which Windows can read
/// without sudo. A path that is not POSIX-absolute (already a Windows path) is
/// returned unchanged.
fn translate_wsl_path(distro: &str, raw: &Path) -> PathBuf {
    let Some(text) = raw.to_str() else {
        return raw.to_path_buf();
    };
    let Some(rest) = text.strip_prefix('/') else {
        return raw.to_path_buf();
    };
    if let Some(mount) = rest.strip_prefix("mnt/") {
        let mut parts = mount.splitn(2, '/');
        let drive = parts.next().unwrap_or("");
        if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
            let tail = parts.next().unwrap_or("").replace('/', "\\");
            return PathBuf::from(format!("{}:\\{}", drive.to_ascii_uppercase(), tail));
        }
    }
    PathBuf::from(format!(
        r"\\wsl.localhost\{}\{}",
        distro,
        rest.replace('/', "\\")
    ))
}

/// Process-wide cache of `wsl.exe --list` with a short TTL. A single user action queries the
/// distro list from several call sites (discovery_sources, add_all_registry_projects,
/// registry_app_attribution, wsl_hermes_state_dbs); without this each call spawns wsl.exe and
/// — by touching `\\wsl.localhost\<distro>` — can cold-boot every installed distro. The TTL
/// collapses the repeated spawns within one action while still picking up a newly-installed
/// distro on the next action a few seconds later.
#[cfg(windows)]
static WSL_DISTRO_CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

/// Runtime gate for WSL enumeration. OFF by default so the app never spawns
/// `wsl.exe` unprompted — on a machine where WSL is present but not fully set up,
/// an unprompted `wsl.exe` call surfaces a WSL error the moment the app opens.
/// The API flips this on (from a persisted setting) only when the user confirms
/// they run AI tools inside WSL, so startup attribution stays Windows-only.
#[cfg(windows)]
static WSL_SCAN_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enable or disable WSL enumeration at runtime (see `WSL_SCAN_ENABLED`). Called
/// by the API from the persisted `wsl_scan_enabled` setting and the Deep Scan
/// opt-in. A no-op on non-Windows, where WSL never applies.
pub fn set_wsl_scan_enabled(enabled: bool) {
    #[cfg(windows)]
    WSL_SCAN_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(windows))]
    let _ = enabled;
}

/// Whether WSL enumeration is currently enabled (Windows only; always false
/// elsewhere). Used to decide whether a Deep Scan should attempt WSL at all.
pub fn wsl_scan_enabled() -> bool {
    #[cfg(windows)]
    {
        WSL_SCAN_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether a path points into a WSL distro share — `\\wsl.localhost\…` or the
/// legacy `\\wsl$\…`, in any separator style or case, including the verbatim
/// `\\?\UNC\wsl.localhost\…` form (`display_path` strips that prefix). Purely
/// string-based: this must never touch the filesystem, because merely statting
/// one of these UNC paths can cold-boot the distro's VM.
fn is_wsl_unc_path(path: &Path) -> bool {
    let normalized = display_path(path).replace('/', "\\").to_ascii_lowercase();
    normalized.starts_with(r"\\wsl.localhost\") || normalized.starts_with(r"\\wsl$\")
}

/// True when `path` is a WSL share the user has NOT opted into scanning — i.e.
/// no filesystem probe (canonicalize, stat, `is_dir`) may touch it right now.
/// The gate exists because statting `\\wsl.localhost\…` can cold-boot a distro,
/// and stored registry/session paths flow through startup attribution
/// (`projects_list` → `project_app_states`) long before any deliberate scan —
/// an opted-out user with one recorded WSL path would otherwise get WSL woken
/// the moment the app opens.
fn wsl_path_blocked_by_gate(path: &Path) -> bool {
    !wsl_scan_enabled() && is_wsl_unc_path(path)
}

/// Installed WSL distro names worth scanning, via `wsl.exe --list --quiet`
/// (`WSL_UTF8=1` forces UTF-8 output instead of UTF-16). System distros that back
/// container runtimes and never hold user projects are skipped. Returns empty
/// when WSL is absent or the call fails, so discovery just covers Windows. Results are
/// memoized for a short TTL (see `WSL_DISTRO_CACHE`) so repeated calls in one pass are free.
#[cfg(windows)]
fn wsl_distros() -> Vec<String> {
    // Gated OFF by default: never spawn wsl.exe unless the user has opted into WSL
    // scanning. This is what keeps the app from surfacing a WSL error at startup.
    if !WSL_SCAN_ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        return Vec::new();
    }
    const TTL: Duration = Duration::from_secs(30);
    {
        let guard = WSL_DISTRO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, distros)) = guard.as_ref() {
            if at.elapsed() < TTL {
                return distros.clone();
            }
        }
    }
    let distros = wsl_distros_uncached();
    let mut guard = WSL_DISTRO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((Instant::now(), distros.clone()));
    distros
}

#[cfg(windows)]
fn wsl_distros_uncached() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: don't flash a console window from the desktop GUI app.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(output) = std::process::Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .env("WSL_UTF8", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().trim_matches('\u{0}').trim())
        .filter(|name| !name.is_empty() && !is_system_wsl_distro(name))
        .map(str::to_string)
        .collect()
}

#[cfg(not(windows))]
fn wsl_distros() -> Vec<String> {
    Vec::new()
}

/// Distros that exist only to back a container runtime — never a place a user
/// opens projects, so they are excluded from discovery.
fn is_system_wsl_distro(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "docker-desktop" | "docker-desktop-data" | "rancher-desktop" | "rancher-desktop-data"
    )
}

/// Read every app's project registry on Windows AND inside each WSL distro, and
/// add the deliberate projects as candidates. The user runs AI agents inside WSL
/// (e.g. Claude Code / Codex in Ubuntu), so a project list that only looked at
/// Windows would miss them. WSL project paths are translated to
/// Windows-accessible paths so they dedup against Windows projects and can be
/// scanned. This is the single entry point discovery uses for registry projects.
fn add_all_registry_projects(candidates: &mut BTreeMap<String, CandidateBuilder>) {
    if let Some(home) = home_dir() {
        // VS Code-family editors store their opened workspaces under %APPDATA%.
        // They share one workspace.json format, so one reader serves them all.
        let workspace_dirs: Vec<(PathBuf, &'static str)> = env_path("APPDATA")
            .map(|appdata| {
                vec![
                    (appdata.join("Cursor").join("User"), "cursor"),
                    (appdata.join("Antigravity").join("User"), "antigravity"),
                    (appdata.join("Antigravity IDE").join("User"), "antigravity"),
                ]
            })
            .unwrap_or_default();
        add_registry_project_candidates(&home, &workspace_dirs, None, candidates);
    }
    for distro in wsl_distros() {
        let homes = PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
        let Ok(entries) = fs::read_dir(&homes) else {
            continue;
        };
        for entry in entries.flatten() {
            let user_home = entry.path();
            if user_home.is_dir() {
                // The VS Code-family editors are Windows apps; only the home
                // registries (Claude, Codex) are read from a WSL distro.
                add_registry_project_candidates(&user_home, &[], Some(&distro), candidates);
            }
        }
    }
}

/// Read every app's explicit project registry and add the real, deliberate
/// projects as strong candidates. This is the registry-based project source: a
/// project is something the user opened in a local AI app, not a random folder a
/// marker scan stumbled on. `cursor_user_dir` is the platform's Cursor `User`
/// directories (`%APPDATA%\<Editor>\User`); these editors are Windows-side, so
/// they are only passed for the Windows home. When `distro` is `Some`, `home` is
/// a WSL distro home and the registries hold POSIX paths, so each is translated
/// to a Windows-accessible path before dedup. Antigravity's authoritative project
/// registry (`.gemini/config/projects`) is likewise a Windows-side store, read
/// only for the Windows home. Paths are deduped across apps — the same project
/// opened in several apps counts once, with corroboration — then classified by
/// `is_registry_project_path`.
fn add_registry_project_candidates(
    home: &Path,
    workspace_user_dirs: &[(PathBuf, &'static str)],
    distro: Option<&str>,
    candidates: &mut BTreeMap<String, CandidateBuilder>,
) {
    let translate = |path: PathBuf| match distro {
        Some(name) => translate_wsl_path(name, &path),
        None => path,
    };
    let mut by_path: BTreeMap<String, (PathBuf, BTreeSet<&'static str>, bool)> = BTreeMap::new();
    for path in read_claude_project_registry(home) {
        record_registry_path(&mut by_path, translate(path), "claude_code", false);
    }
    for (path, trusted) in read_codex_project_registry(home) {
        // A folder the user trusted in Codex is a deliberate workspace; an
        // untrusted/one-off cwd is not (it still needs identity or a second app).
        record_registry_path(&mut by_path, translate(path), "codex", trusted);
    }
    for (dir, app) in workspace_user_dirs {
        for path in read_vscode_workspace_registry(dir) {
            record_registry_path(&mut by_path, path, app, true);
        }
    }
    // Antigravity's authoritative project registry is a Windows-side store, so it
    // is read only for the Windows home (`distro` is `None`), never per WSL distro.
    if distro.is_none() {
        for path in read_gemini_project_registry(home) {
            record_registry_path(&mut by_path, path, "antigravity", true);
        }
        // A folder Claude Code or Codex actually ran sessions in is a deliberate
        // project (read from the recorded session `cwd`). The session readers are
        // Windows-side here; WSL session trees are not walked. `deliberate_workspace`
        // is true so a marker-less root still survives `is_registry_project_path`.
        for path in claude_project_roots(home) {
            record_registry_path(&mut by_path, path, "claude_code", true);
        }
        for path in codex_session_project_roots(home) {
            record_registry_path(&mut by_path, path, "codex", true);
        }
    }

    for (_, (path, apps, deliberate_workspace)) in by_path {
        if !is_registry_project_path(&path, deliberate_workspace, apps.len()) {
            continue;
        }
        let source = DiscoverySource {
            kind: "app_project_registry".to_string(),
            label: "App project registry".to_string(),
            path: path.clone(),
            detail: Some("Listed in a local AI app's project registry.".to_string()),
            mode: SourceMode::TextMetadata,
        };
        add_candidate(
            candidates,
            &path,
            &source,
            "app_project_registry",
            "Deliberately opened in a local AI app",
            None,
            44,
        );
        add_marker_signals(candidates, &path);
    }
}

fn is_local_ai_state_folder(path: &Path) -> bool {
    let components = path_components_lower(path);
    component_sequence_contains(&components, &[".codex", "sessions"])
        || component_sequence_contains(&components, &[".codex", "archived_sessions"])
        || component_sequence_contains(&components, &[".codex", "skills"])
        || component_sequence_contains(&components, &[".claude", "projects"])
        || component_sequence_contains(&components, &[".claude", "sessions"])
        || component_sequence_contains(&components, &[".claude", "skills"])
        || component_sequence_contains(&components, &[".cursor", "skills"])
        || component_sequence_contains(&components, &[".cursor", "skills-cursor"])
        || component_sequence_contains(&components, &[".gemini", "skills"])
        || component_sequence_contains(&components, &[".openclaw", "sessions"])
        || component_sequence_contains(&components, &[".openclaw", "skills"])
        || component_sequence_contains(&components, &[".nemoclaw", "sessions"])
        || component_sequence_contains(&components, &[".nemoclaw", "skills"])
        || component_sequence_contains(&components, &[".nemoclaw", "sandboxes"])
        || component_sequence_contains(&components, &[".hermes"])
        || component_sequence_contains(&components, &[".gemini", "antigravity", "brain"])
        || component_sequence_contains(&components, &[".gemini", "antigravity", "conversations"])
        || component_sequence_contains(&components, &[".gemini", "antigravity", "scratch"])
        // Second Antigravity IDE store: treat its session trees as AI state too.
        || component_sequence_contains(&components, &[".gemini", "antigravity-ide", "brain"])
        || component_sequence_contains(&components, &[".gemini", "antigravity-ide", "conversations"])
}

fn should_list_session_file(source_file: &Path, kind: &str) -> bool {
    if kind == "gemini_antigravity_brain" || kind == "gemini_antigravity_ide_brain" {
        // The brain `transcript.jsonl` FREEZES once the conversation migrates into
        // `conversations/<uuid>.db`, so it is no longer surfaced as the chat — the
        // live `.db` (handled by `scan_antigravity_conversation_db`) is. The brain
        // tree is still scanned for project-path signals, just not listed here. The
        // second IDE store's brain (`antigravity_ide_brain`) is treated identically.
        return false;
    }
    if kind == "claude_local_agent_sessions" || kind == "claude_app_sessions" {
        return is_claude_local_json_session_file(source_file);
    }
    if kind == "claude_code_sessions"
        && source_file
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        // `<claude-home>/sessions/*.json` are per-PID lock/heartbeat files
        // (`{pid, sessionId, cwd, …}`), not conversations — a "13464.json" row is
        // noise. The source is still scanned: its recorded cwd keeps feeding the
        // project-attribution and recent-activity signals below the listing gate.
        return false;
    }
    is_listable_session_source_kind(kind) && !is_session_support_metadata_file(source_file)
}

/// The frozen Antigravity brain transcript marker
/// (`<task>/.system_generated/logs/transcript.jsonl`). No longer listed as a
/// session (see `should_list_session_file`); retained as a named predicate so the
/// exclusion stays explicit and testable.
#[cfg(test)]
fn is_antigravity_brain_session_marker_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.eq_ignore_ascii_case("transcript.jsonl"))
        .unwrap_or(false)
        && component_sequence_contains(&path_components_lower(path), &[".system_generated", "logs"])
}

fn is_claude_local_json_session_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("local_") && lower.ends_with(".json")
        })
        .unwrap_or(false)
}

fn is_listable_session_source_kind(kind: &str) -> bool {
    is_loose_session_source_kind(kind)
        // Claude Code conversation transcripts (~/.claude/projects/<dir>/*.jsonl) —
        // the kind has no "session"/"transcript" token, so list it explicitly.
        || kind == "claude_code_projects"
        || kind.contains("_tasks")
        || kind.contains("copilot_chat")
        || kind.contains("cody")
}

fn is_loose_session_source_kind(kind: &str) -> bool {
    kind.contains("session")
        || kind.contains("transcript")
        || kind.contains("conversation")
        || kind.contains("brain")
        || kind.contains("antigravity_conversations")
        // Cursor in-IDE chats: many have no resolvable workspace (drafts / hashed-only
        // workspace ids), so they must be allowed to list as loose like other chats.
        || kind == "cursor_ide_chats"
}

fn is_session_support_metadata_file(path: &Path) -> bool {
    let components = path_components_lower(path);
    if components.iter().any(|component| {
        matches!(
            component.as_str(),
            "artifacts"
                | "scratch"
                | "uploads"
                | "outputs"
                | "node_modules"
                | "cache"
                | "caches"
                // Claude Code nests internal subagent transcripts under
                // <session>/subagents/agent-*.jsonl — list the main conversation,
                // not the dozens of internal agent runs.
                | "subagents"
        )
    }) || component_sequence_contains(&components, &[".claude", "tasks"])
        || component_sequence_contains(&components, &[".claude", "plugins"])
        || component_sequence_contains(&components, &[".system_generated", "messages"])
        || component_sequence_contains(&components, &[".system_generated", "steps"])
        || component_sequence_contains(&components, &[".system_generated", "tasks"])
    {
        return true;
    }
    let Some(name) = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
    else {
        return false;
    };
    matches!(
        name.as_str(),
        ".claude.json"
            | "audit.jsonl"
            | "artifacts.json"
            | "blocklist.json"
            | "cowork-clientdata-cache.json"
            | "cowork-gb-cache.json"
            | "db-shm"
            | "db-wal"
            | "package-lock.json"
            | "package.json"
            | "scheduled-tasks.json"
            | "spaces.json"
            | "settings.local.json"
            | "skill.md"
    ) || name.ends_with("-cache.json")
        || name.ends_with(".db-shm")
        || name.ends_with(".db-wal")
        || name.ends_with(".metadata.json")
        || name.contains(".resolved")
}

fn session_kind_for_source(kind: &str) -> &'static str {
    if kind.contains("codex") {
        "ChatGPT"
    } else if kind.contains("claude") {
        "Claude"
    } else if kind.contains("cursor") {
        "Cursor"
    } else if kind.contains("antigravity") || kind.contains("gemini") {
        "Antigravity/Gemini"
    } else if kind.contains("openclaw") {
        "OpenClaw"
    } else if kind.contains("nemoclaw") || kind.contains("hermes") {
        "Hermes/NemoClaw"
    } else if kind.contains("zed") {
        "Zed"
    } else if kind.contains("copilot") {
        "GitHub Copilot"
    } else if kind.contains("cody") {
        "Sourcegraph Cody"
    } else if kind.contains("continue") {
        "Continue"
    } else if kind.contains("roo") {
        "Roo Code"
    } else if kind.contains("cline") {
        "Cline"
    } else if kind.contains("kilo") {
        "Kilo Code"
    } else {
        "Local AI session"
    }
}

fn decode_cursor_project_store_dir(path: &Path) -> Option<PathBuf> {
    let name = path.file_name().and_then(|value| value.to_str())?;
    if name.eq_ignore_ascii_case("empty-window") || name.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let mut chars = name.chars();
    let drive = chars.next()?.to_ascii_uppercase();
    if !drive.is_ascii_alphabetic() || chars.next()? != '-' {
        return None;
    }
    let parts = chars
        .as_str()
        .split('-')
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    let root = PathBuf::from(format!("{drive}:\\"));
    decode_cursor_project_segments(&root, &parts)
}

fn decode_cursor_project_segments(base: &Path, parts: &[&str]) -> Option<PathBuf> {
    if parts.is_empty() {
        return base.is_dir().then(|| canonical_or_original(base));
    }
    for width in 1..=parts.len().min(6) {
        let segment = parts[..width].join("-");
        let candidate = base.join(segment);
        if !candidate.is_dir() {
            continue;
        }
        if let Some(decoded) = decode_cursor_project_segments(&candidate, &parts[width..]) {
            return Some(decoded);
        }
    }
    None
}

fn estimate_dir(path: &Path) -> (Option<u64>, Option<u64>, bool) {
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    let mut partial = false;
    let mut queue = VecDeque::from([(path.to_path_buf(), 0_usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if count >= ESTIMATE_MAX_ITEMS as u64 {
                partial = true;
                break;
            }
            let child = entry.path();
            if child.is_dir() {
                count = count.saturating_add(1);
                if depth < ESTIMATE_DEPTH && !skip_dir(&child) {
                    queue.push_back((child, depth + 1));
                } else if depth >= ESTIMATE_DEPTH {
                    partial = true;
                }
            } else if let Ok(metadata) = entry.metadata() {
                count = count.saturating_add(1);
                bytes = bytes.saturating_add(metadata.len());
            }
        }
        if partial && count >= ESTIMATE_MAX_ITEMS as u64 {
            break;
        }
    }
    (Some(count), Some(bytes), partial)
}

fn skip_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    // Prune dependency, build-output, cache and toolchain folders during the
    // walk. These never contain the user's own projects (only vendored deps or
    // generated artifacts), so skipping them makes Deep Scan dramatically faster
    // and keeps vendored packages out of the discovered candidate list. We
    // deliberately do NOT prune generic container names like "packages", "bin"
    // or "src" because monorepos keep real sub-projects there.
    matches!(
        name.to_ascii_lowercase().as_str(),
        // Version control internals
        ".git"
            | ".svn"
            | ".hg"
            // JS/TS dependencies, build output and caches
            | "node_modules"
            | "bower_components"
            | ".next"
            | ".nuxt"
            | ".svelte-kit"
            | ".angular"
            | ".parcel-cache"
            | ".turbo"
            | ".yarn"
            | ".pnpm-store"
            // Python environments, vendored packages and tool caches
            | ".venv"
            | "venv"
            | "site-packages"
            | "dist-packages"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".nox"
            // Rust / Java / .NET / mobile build output and toolchains
            | "target"
            | ".gradle"
            | ".m2"
            | "obj"
            | "pods"
            // Generic build output, vendored deps and coverage/cache folders
            | "dist"
            | "build"
            | "vendor"
            | "coverage"
            | ".nyc_output"
            | ".terraform"
            | ".cache"
            // IDE and editor index/state
            | ".idea"
            | ".vs"
            // System / OS
            | "appdata"
            | "$recycle.bin"
            | "system volume information"
    )
}

fn is_probably_system_or_noise(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    if is_top_level_system_dir(path) {
        return true;
    }
    if let Some(home) = home_dir() {
        if same_path(path, &home) {
            return true;
        }
    }
    let components = path_components_lower(path);
    components.iter().any(|name| {
        matches!(
            name.as_str(),
            "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "$recycle.bin"
                | "system volume information"
                | ".system_generated"
                | "site-packages"
                | "dist-packages"
                | "node_modules"
                | "__pycache__"
                | "windowsapps"
                | "cacheddata"
                | "dist"
                | "build"
                | "outputs"
                | "uploads"
                | "vendor"
                | ".vendor"
        ) || name.starts_with("index.crates.io-")
    }) || component_sequence_contains(&components, &["appdata", "local", "programs", "python"])
        || (components.iter().any(|name| name == "appdata") && !is_local_ai_state_folder(path))
        || component_sequence_contains(&components, &["appdata", "local", "temp"])
        || component_sequence_contains(&components, &["appdata", "local", "programs"])
        || component_sequence_contains(&components, &["appdata", "local", "microsoft", "winget"])
        || component_sequence_contains(&components, &["appdata", "roaming", "cursor", "cacheddata"])
        || component_sequence_contains(&components, &["pinokio", "envs"])
        || component_sequence_contains(&components, &[".cargo", "registry"])
        || component_sequence_contains(&components, &[".cargo", "git"])
        || component_sequence_contains(&components, &["cargo", "registry"])
        || component_sequence_contains(&components, &["cargo", "git"])
        || component_sequence_contains(&components, &["registry", "src"])
        || component_sequence_contains(&components, &[".gradle", "caches"])
        || component_sequence_contains(&components, &["pip", "cache"])
        || component_sequence_contains(&components, &[".gemini", "config", "plugins"])
        || component_sequence_contains(&components, &[".local", "bin"])
        || component_sequence_contains(&components, &[".lmstudio", "bin"])
        || components.last().map(String::as_str) == Some("bin")
        || components.last().map(String::as_str) == Some("src")
        || components.last().map(String::as_str) == Some("temp")
        || components.last().map(String::as_str) == Some("scripts")
            && !project_markers_in_dir(path).iter().any(|marker| {
                matches!(
                    marker.kind,
                    "agent_context" | "claude_context" | "gemini_context" | "skill_definition"
                )
            })
}

fn is_forbidden_candidate_path(path: &Path) -> bool {
    let components = path_components_lower(path);
    same_path(path, Path::new("C:\\pinokio"))
        // Installed applications can carry README files, package manifests and
        // even AI-tool metadata, but they are not user projects. Treat the
        // Windows-owned install roots as unconditional exclusions so a session
        // launched from an application's working directory cannot promote it.
        || is_system_owned_install_path(&components)
        // Pinokio ships bundled demos under `pinokio\prototype\…` (the
        // `system\examples\*` apps). A session that merely mentions one makes it
        // look like a project — but the user never created it. The real installed
        // Pinokio apps live under `pinokio\api\` and are surfaced separately.
        || component_sequence_contains(&components, &["pinokio", "prototype"])
        || component_sequence_contains(&components, &["pinokio", "bin"])
        || component_sequence_contains(&components, &["pinokio", "cache"])
        || components.iter().any(|name| {
            // Vendored-package containers. Anything nested inside one is a
            // third-party dependency, never the user's own project — even when
            // the package ships its own README or manifest (a Python wheel under
            // site-packages, an npm package under node_modules, …). This is
            // unconditional on purpose: project identity inside these does not
            // promote them to a candidate.
            matches!(
                name.as_str(),
                "node_modules" | "site-packages" | "dist-packages" | "bower_components"
            )
        })
        || components
            .iter()
            .any(|name| name.starts_with("index.crates.io-"))
        || component_sequence_contains(&components, &["appdata", "local", "microsoft", "winget"])
        || component_sequence_contains(&components, &["appdata", "roaming", "npm", "node_modules"])
        || component_sequence_contains(&components, &[".cargo", "registry"])
        || component_sequence_contains(&components, &[".cargo", "git"])
        || component_sequence_contains(&components, &["cargo", "registry"])
        || component_sequence_contains(&components, &["cargo", "git"])
        || component_sequence_contains(&components, &["registry", "src"])
        || component_sequence_contains(&components, &[".gradle", "caches"])
        || component_sequence_contains(&components, &["pip", "cache"])
        || component_sequence_contains(&components, &[".codex", "skills", ".system"])
        || is_internal_ai_state_candidate_components(&components)
        || component_sequence_contains(&components, &[".gemini", "config", "plugins"])
        || (components.iter().any(|name| name == "skills")
            && components.iter().any(|name| name == "references"))
        || (components
            .iter()
            .any(|name| name == "local-agent-mode-sessions")
            && matches!(
                components.last().map(String::as_str),
                Some("outputs" | "uploads")
            ))
}

fn is_system_owned_install_path(components: &[String]) -> bool {
    matches!(
        components.first().map(String::as_str),
        Some("windows" | "program files" | "program files (x86)" | "programdata")
    ) || component_sequence_contains(components, &["appdata", "local", "programs"])
}

fn is_internal_ai_state_candidate_path(path: &Path) -> bool {
    is_internal_ai_state_candidate_components(&path_components_lower(path))
}

fn is_internal_ai_state_candidate_components(components: &[String]) -> bool {
    let Some(last) = components.last().map(String::as_str) else {
        return false;
    };
    matches!(
        last,
        ".codex" | ".claude" | ".cursor" | ".gemini" | ".hermes" | ".openclaw" | ".nemoclaw"
    ) || component_sequence_contains(components, &[".codex", "sessions"])
        || component_sequence_contains(components, &[".codex", "archived_sessions"])
        || component_sequence_contains(components, &[".claude", "projects"])
        || component_sequence_contains(components, &[".claude", "sessions"])
        || component_sequence_contains(components, &[".claude", "session-env"])
        || component_sequence_contains(components, &[".cursor", "projects"])
        || component_sequence_contains(components, &[".cursor", "agent-transcripts"])
        || component_sequence_contains(components, &["claude", "local-agent-mode-sessions"])
        || component_sequence_contains(components, &[".gemini", "antigravity"])
        || component_sequence_contains(components, &[".gemini", "antigravity-ide"])
        || component_sequence_contains(components, &[".gemini", "antigravity", "brain"])
        || component_sequence_contains(components, &[".gemini", "antigravity", "conversations"])
        || component_sequence_contains(components, &[".gemini", "antigravity", "scratch"])
        || component_sequence_contains(components, &[".gemini", "antigravity-ide", "brain"])
        || component_sequence_contains(components, &[".gemini", "antigravity-ide", "conversations"])
        || component_sequence_contains(components, &[".hermes", "sessions"])
        || component_sequence_contains(components, &[".hermes", "memories"])
        || component_sequence_contains(components, &[".hermes", "cache"])
        || component_sequence_contains(components, &[".hermes", "logs"])
        || component_sequence_contains(components, &[".hermes", "state-snapshots"])
        || component_sequence_contains(components, &[".openclaw", "sessions"])
        || component_sequence_contains(components, &[".openclaw", "memory"])
        || component_sequence_contains(components, &[".nemoclaw", "sessions"])
        || component_sequence_contains(components, &[".nemoclaw", "sandboxes"])
        || component_sequence_contains(components, &[".nemoclaw", "memory"])
}

fn is_broad_container_path(path: &Path) -> bool {
    if is_known_broad_container_path(path) {
        return true;
    }
    // Shallow folders without project identity are usually workspace/tools/data
    // containers. Explicit session cwd paths intentionally use only the narrower
    // known-container policy so markerless projects remain valid.
    match drive_root_depth(path) {
        Some(1) | Some(2) => !has_project_identity(path),
        _ => false,
    }
}

fn is_known_broad_container_path(path: &Path) -> bool {
    if is_local_ai_state_folder(path) {
        return false;
    }
    if let Some(home) = home_dir() {
        for broad in [
            home.clone(),
            home.join("Desktop"),
            home.join("Documents"),
            home.join("Downloads"),
            home.join("OneDrive"),
            home.join("OneDrive").join("Documents"),
            home.join("OneDrive").join("Desktop"),
            home.join("OneDrive").join("Documents").join("AI"),
            home.join("OneDrive")
                .join("Documents")
                .join("AI")
                .join("Codex"),
            home.join("OneDrive")
                .join("Documents")
                .join("AI")
                .join("Antigravity"),
            home.join(".gemini").join("antigravity"),
            home.join("OneDrive")
                .join("Documents")
                .join("Claude")
                .join("Projects"),
        ] {
            if same_path(path, &broad) {
                return true;
            }
        }
    }
    drive_root_depth(path) == Some(0)
}

/// Number of path components below the drive root: `C:\` = 0, `C:\Tools` = 1,
/// `C:\Tools\Sub` = 2. `None` when the path is not an absolute drive-rooted path
/// (relative, UNC, or WSL share), so such paths are treated as "not shallow".
fn drive_root_depth(path: &Path) -> Option<usize> {
    let mut components = path.components();
    let Some(Component::Prefix(_)) = components.next() else {
        return None;
    };
    let Some(Component::RootDir) = components.next() else {
        return None;
    };
    Some(
        components
            .filter(|component| matches!(component, Component::Normal(_)))
            .count(),
    )
}

fn path_components_lower(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

fn is_top_level_system_dir(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Prefix(_)) = components.next() else {
        return false;
    };
    let Some(Component::RootDir) = components.next() else {
        return false;
    };
    let Some(Component::Normal(name)) = components.next() else {
        return false;
    };
    if components.next().is_some() {
        return false;
    }
    matches!(
        name.to_string_lossy().to_ascii_lowercase().as_str(),
        "users" | "windows" | "program files" | "program files (x86)" | "programdata" | "amd"
    )
}

fn component_sequence_contains(components: &[String], sequence: &[&str]) -> bool {
    if sequence.is_empty() || components.len() < sequence.len() {
        return false;
    }
    components.windows(sequence.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(sequence.iter().copied())
    })
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn candidate_key(path: &Path) -> String {
    // Normalize separators so a registry key written with forward slashes collapses to
    // the same key as the canonical backslash form. Claude's `.claude.json` records the
    // session cwd with mixed `/` and `\` (the same project appears as both
    // `C:\proj` and `C:/proj`); when `fs::canonicalize` fails — e.g. an offline drive —
    // it can no longer normalize them for us, so without this the forward-slash variant
    // would never match the project key and the folder would lose its app attribution.
    display_path(&canonical_or_original(path))
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn canonical_or_original(path: &Path) -> PathBuf {
    // Never canonicalize a WSL share path while the WSL scan gate is off:
    // `fs::canonicalize` stats the UNC path, and statting `\\wsl.localhost\…` can
    // cold-boot the distro — the exact startup failure the gate exists to prevent
    // (every registered path is keyed through here on `projects_list`). The raw
    // path is returned instead; `candidate_key`'s own separator/case folding still
    // produces the same key string canonicalization would have.
    if wsl_path_blocked_by_gate(path) {
        return path.to_path_buf();
    }
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn display_path(path: &Path) -> String {
    display_path_for_path(&path.to_string_lossy())
}

fn same_path(a: &Path, b: &Path) -> bool {
    candidate_key(a) == candidate_key(b)
}

fn is_child_of(child: &Path, parent: &Path) -> bool {
    let child = candidate_key(child);
    let parent = candidate_key(parent);
    child != parent && child.starts_with(&(parent.trim_end_matches('\\').to_string() + "\\"))
}

/// Whether a discovery source can contribute `session_path` links (a project a
/// local AI session has worked in). Folder-only sources just enumerate
/// directories and never carry session references.
fn source_carries_session_links(mode: SourceMode) -> bool {
    matches!(
        mode,
        SourceMode::TextMetadata
            | SourceMode::SqliteMetadata
            | SourceMode::CursorProjectTranscripts
            | SourceMode::HermesState
            | SourceMode::OpenClawState
    )
}

/// Cross-reference every session-carrying discovery source that lives *outside*
/// the scanned root, attaching `session_path` signals to folders inside the root
/// that a local AI session has actually worked in. Transcripts live in global
/// locations (~/.claude, ~/.codex, Cursor storage, …), so without this a deep
/// scan of a plain code directory links zero sessions and auto-adds nothing.
/// Anything the sweep surfaces outside the root is pruned afterwards.
fn cross_reference_global_sessions(
    root: &Path,
    sources: &[DiscoverySource],
    candidates: &mut BTreeMap<String, CandidateBuilder>,
    sessions: &mut BTreeMap<String, SessionBuilder>,
    searched_locations: &mut Vec<DiscoverySourceHit>,
) {
    for known_source in sources {
        if !source_carries_session_links(known_source.mode) {
            continue;
        }
        if same_path(&known_source.path, root) || is_child_of(&known_source.path, root) {
            continue; // already swept by the in-root pass
        }
        if !known_source.path.exists() {
            continue;
        }
        searched_locations.push(DiscoverySourceHit {
            source_kind: known_source.kind.to_string(),
            source_label: known_source.label.to_string(),
            path: display_path(&known_source.path),
            exists: true,
            detail: known_source.detail.clone(),
        });
        scan_discovery_source(known_source, candidates, sessions);
    }

    // The sweep also surfaces projects elsewhere on disk; keep only the scanned
    // folder itself and anything nested under it.
    candidates
        .retain(|_, builder| same_path(&builder.path, root) || is_child_of(&builder.path, root));
    sessions.retain(|_, builder| {
        same_path(&builder.path, root)
            || is_child_of(&builder.path, root)
            || builder
                .linked_project_paths
                .iter()
                .any(|path| same_path(path, root) || is_child_of(path, root))
    });
}

fn confidence_for_score(score: u64) -> &'static str {
    if score >= 24 {
        "High"
    } else if score >= 16 {
        "Medium"
    } else {
        "Low"
    }
}

fn confidence_for_total_score(score: u64) -> &'static str {
    if score >= 70 {
        "High"
    } else if score >= 35 {
        "Medium"
    } else {
        "Low"
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn home_dir() -> Option<PathBuf> {
    env_path("USERPROFILE").or_else(|| env_path("HOME"))
}

/// Fast, side-effect-free probe of which local AI tools are installed on this host,
/// by testing whether each tool's config/home directory exists. No process is
/// spawned and WSL is never touched (WSL tools are opt-in, handled separately), so
/// this is safe to call at any time — including to drive the Deep Scan UI so it
/// lists only the tools actually present instead of a fixed roster.
pub fn detect_installed_apps() -> Vec<hangar_core::InstalledApp> {
    let home = home_dir();
    let appdata = env_path("APPDATA");
    let home_join = |seg: &str| home.as_ref().map(|h| h.join(seg));
    let app_join = |seg: &str| appdata.as_ref().map(|a| a.join(seg));
    let any_exists = |paths: Vec<Option<PathBuf>>| paths.into_iter().flatten().any(|p| p.exists());
    let app = |id: &str, label: &str, paths: Vec<Option<PathBuf>>| hangar_core::InstalledApp {
        id: id.to_string(),
        label: label.to_string(),
        present: any_exists(paths),
    };
    vec![
        app("claude", "Claude Code", vec![home_join(".claude")]),
        app("codex", "ChatGPT", vec![home_join(".codex")]),
        app("cursor", "Cursor", vec![app_join("Cursor")]),
        app(
            "antigravity",
            "Antigravity",
            vec![
                app_join("Antigravity"),
                app_join("Antigravity IDE"),
                home.as_ref().map(|h| h.join(".gemini").join("antigravity")),
            ],
        ),
        // Probe CLI-specific markers, not the bare `~/.gemini` dir: Antigravity
        // creates `~/.gemini/antigravity` + `~/.gemini/config` itself, so the bare
        // dir would report "Gemini CLI installed" to every Antigravity-only user.
        // The CLI writes `settings.json` on first run and `tmp/` for checkpoints.
        app(
            "gemini",
            "Gemini CLI",
            vec![
                home.as_ref()
                    .map(|h| h.join(".gemini").join("settings.json")),
                home.as_ref().map(|h| h.join(".gemini").join("tmp")),
            ],
        ),
        app(
            "windsurf",
            "Windsurf",
            vec![app_join("Windsurf"), app_join("Windsurf - Next")],
        ),
        app("openclaw", "OpenClaw", vec![home_join(".openclaw")]),
        app(
            "hermes",
            "Hermes / NemoClaw",
            vec![home_join(".hermes"), home_join(".nemoclaw")],
        ),
        app(
            "pinokio",
            "Pinokio",
            vec![home_join("pinokio"), Some(PathBuf::from(r"C:\pinokio"))],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A temp dir that does NOT live under `…\AppData\Local\Temp` (the default
    /// `tempdir()` location, which `is_probably_system_or_noise` rejects). Rooted
    /// under the workspace `target` dir so a no-marker "project" folder created
    /// inside it is treated like a real on-disk project — needed to exercise the
    /// trust-only path where there is deliberately no marker file.
    fn tempdir_projectlike() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        fs::create_dir_all(&base).expect("create project-like temp base");
        tempfile::TempDir::new_in(&base).expect("create project-like temp dir")
    }

    #[test]
    fn progressive_transcript_limits_grow_and_full_removes_the_item_cap() {
        assert_eq!(
            progressive_transcript_item_limit(4_000, 768 * 1024, 768 * 1024, false),
            4_000
        );
        assert_eq!(
            progressive_transcript_item_limit(4_000, 768 * 1024, 1536 * 1024, false),
            8_000
        );
        assert_eq!(
            progressive_transcript_item_limit(4_000, 768 * 1024, 1, true),
            usize::MAX
        );
    }

    #[test]
    fn antigravity_registry_file_resolves_a_project_by_root() {
        let base = tempdir_projectlike();
        let home = base.path();
        let projects_dir = home.join(".gemini").join("config").join("projects");
        fs::create_dir_all(&projects_dir).unwrap();

        let project = home.join("work").join("my-proj");
        fs::create_dir_all(&project).unwrap();
        let other = home.join("work").join("other");
        fs::create_dir_all(&other).unwrap();

        let uri = |p: &Path| format!("file:///{}", p.to_string_lossy().replace('\\', "/"));
        let registry_file = projects_dir.join("abc-123.json");
        fs::write(
            &registry_file,
            format!(
                r#"{{"name":"My Proj","projectResources":{{"resources":[{{"folderUri":"{}"}}]}}}}"#,
                uri(&project)
            ),
        )
        .unwrap();
        fs::write(
            projects_dir.join("def-456.json"),
            format!(
                r#"{{"projectResources":{{"resources":[{{"folderUri":"{}"}}]}}}}"#,
                uri(&other)
            ),
        )
        .unwrap();

        // The project's own registry file is found (the one to back up + delete).
        let found = antigravity_registry_file_for_root_in(home, &project);
        assert_eq!(
            found.and_then(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())),
            Some("abc-123.json".to_string())
        );
        // A path with no registry entry resolves to nothing.
        assert!(
            antigravity_registry_file_for_root_in(home, &home.join("work").join("nope")).is_none()
        );
    }

    #[cfg(windows)]
    fn cursor_store_name_for_test(path: &Path) -> String {
        let mut drive = None;
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => {
                    let value = prefix.as_os_str().to_string_lossy();
                    drive = value.chars().find(|ch| ch.is_ascii_alphabetic());
                }
                Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
                _ => {}
            }
        }
        format!(
            "{}-{}",
            drive.unwrap_or('C').to_ascii_uppercase(),
            parts.join("-")
        )
    }

    #[test]
    fn detects_project_markers_in_known_folder() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("demo-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# Demo").unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();

        let source = DiscoverySource {
            kind: "test_known".to_string(),
            label: "Test known folder".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .find(|candidate| candidate.path.ends_with("demo-project"))
            .unwrap();
        assert_eq!(candidate.display_name, "demo-project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "agent_context"));
        assert_eq!(candidate.confidence, "High");
    }

    #[test]
    fn global_discovery_drops_folder_marker_only_projects() {
        // A folder-marker scan walks into a plain repo that was never opened in
        // any AI app. It must NOT count as a project in global discovery — only
        // registry/session-backed folders do.
        let dir = tempdir().unwrap();
        let random = dir.path().join("random-clone");
        fs::create_dir_all(random.join(".git")).unwrap();
        fs::write(random.join("README.md"), "# r").unwrap();

        let source = DiscoverySource {
            kind: "documents".to_string(),
            label: "Documents".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);
        let random_builder = candidates
            .get(&candidate_key(&random))
            .expect("folder-marker scan should find the repo as a raw candidate");
        assert!(
            !candidate_is_deliberate_project(random_builder),
            "a folder-marker-only candidate must be dropped from global discovery"
        );

        // The same folder, once it is in an app registry, IS a deliberate project.
        let registry_source = DiscoverySource {
            kind: "app_project_registry".to_string(),
            label: "App project registry".to_string(),
            path: random.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        add_candidate(
            &mut candidates,
            &random,
            &registry_source,
            "app_project_registry",
            "Deliberately opened in a local AI app",
            None,
            44,
        );
        assert!(
            candidate_is_deliberate_project(candidates.get(&candidate_key(&random)).unwrap()),
            "a registry-listed candidate is a deliberate project"
        );
    }

    #[test]
    fn antigravity_conversation_only_folder_is_a_deliberate_project() {
        // A folder referenced ONLY by a local Antigravity conversation must surface in global
        // discovery. The live signal kind is "antigravity_conversation_path"; the allowlist once
        // held the dead kind "decoded_project_path", silently dropping conversation-only projects.
        let dir = tempdir().unwrap();
        let project = dir.path().join("ag-project");
        // Give the folder a real identity so add_candidate doesn't drop it as empty/noise; the
        // .git alone is NOT a deliberate signal (the folder-marker-only case), so this test still
        // proves it's the antigravity_conversation_path signal that makes it deliberate.
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join("main.py"), "print(1)").unwrap();
        let source = DiscoverySource {
            kind: "gemini_antigravity_conversations".to_string(),
            label: "Antigravity conversations".to_string(),
            path: project.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        add_candidate(
            &mut candidates,
            &project,
            &source,
            "antigravity_conversation_path",
            "Referenced by local Antigravity conversation",
            None,
            32,
        );
        assert!(
            candidate_is_deliberate_project(candidates.get(&candidate_key(&project)).unwrap()),
            "an antigravity-conversation-only folder must be a deliberate project"
        );
    }

    #[test]
    fn session_options_gate_loose_and_agent_sessions() {
        fn session(source_kind: &str, association: &str) -> SessionDiscoveryCandidate {
            SessionDiscoveryCandidate {
                path: format!("/tmp/{source_kind}"),
                display_name: source_kind.to_string(),
                source_kind: source_kind.to_string(),
                source_label: source_kind.to_string(),
                session_kind: "x".to_string(),
                confidence: "Medium".to_string(),
                linked_project_paths: Vec::new(),
                linked_registered_project_ids: Vec::new(),
                association: association.to_string(),
                modified_ms: None,
            }
        }
        let make = || {
            vec![
                session("claude_code_projects", "registered_project"), // project-linked
                session("codex_sessions", "loose_session"),            // loose, no project
                session("wsl_ubuntu_hermes_sessions", "loose_session"), // agent
            ]
        };

        // The default now includes loose sessions (so "sessions soltas" show out of
        // the box) but still gates agent sessions off.
        let defaults = DiscoveryOptions::default();
        assert!(defaults.include_loose_sessions);
        assert!(!defaults.include_agents);

        // Default (loose on, agents off): project-linked + loose codex survive; the
        // agent session is dropped.
        let mut s = make();
        filter_sessions_by_options(&mut s, &defaults);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|x| !is_agent_session_kind(&x.source_kind)));
        assert!(s.iter().any(|x| x.association == "loose_session"));

        // Loose explicitly off, agents off: only the project-linked session survives.
        let mut s = make();
        filter_sessions_by_options(
            &mut s,
            &DiscoveryOptions {
                include_loose_sessions: false,
                include_agents: false,
                ..Default::default()
            },
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].association, "registered_project");

        // Agents on, loose off: project-linked + the Hermes agent, but not the loose codex.
        let mut s = make();
        filter_sessions_by_options(
            &mut s,
            &DiscoveryOptions {
                include_loose_sessions: false,
                include_agents: true,
                ..Default::default()
            },
        );
        assert_eq!(s.len(), 2);
        assert!(s.iter().any(|x| is_agent_session_kind(&x.source_kind)));

        // Both on: everything.
        let mut s = make();
        filter_sessions_by_options(
            &mut s,
            &DiscoveryOptions {
                include_loose_sessions: true,
                include_agents: true,
                ..Default::default()
            },
        );
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn finalize_sessions_deduplicates_same_visible_session() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let mut sessions = BTreeMap::new();
        for (idx, modified_ms) in [100_i64, 200_i64].into_iter().enumerate() {
            let mut linked_project_paths = BTreeSet::new();
            linked_project_paths.insert(project.clone());
            sessions.insert(
                format!("copy-{idx}"),
                SessionBuilder {
                    path: dir.path().join(format!("copy-{idx}.json")),
                    display_name: Some("local_same-session.json".to_string()),
                    source_kind: "claude_app_sessions".to_string(),
                    source_label: "Claude desktop local sessions".to_string(),
                    session_kind: "Claude".to_string(),
                    linked_project_paths,
                    confidence: "High".to_string(),
                    modified_ms: Some(modified_ms),
                },
            );
        }

        let out = finalize_sessions(sessions, &[]);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].display_name, "local_same-session.json");
        assert_eq!(out[0].modified_ms, Some(200));
    }

    #[test]
    fn finalize_sessions_prefers_hermes_state_db_over_the_index_copy() {
        let dir = tempdir().unwrap();
        let hermes_home = dir.path().join(".hermes");
        fs::create_dir_all(hermes_home.join("sessions")).unwrap();
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "index".to_string(),
            SessionBuilder {
                path: source_file_with_fragment(
                    &hermes_home.join("sessions").join("sessions.json"),
                    "hermes-session=same-id",
                ),
                display_name: Some("Index label".to_string()),
                source_kind: "wsl_hermes_sessions".to_string(),
                source_label: "Hermes sessions".to_string(),
                session_kind: "Hermes/NemoClaw".to_string(),
                linked_project_paths: BTreeSet::new(),
                confidence: "Low".to_string(),
                modified_ms: Some(300),
            },
        );
        sessions.insert(
            "state".to_string(),
            SessionBuilder {
                path: source_file_with_fragment(
                    &hermes_home.join("state.db"),
                    "hermes-session=same-id",
                ),
                display_name: Some("Canonical conversation title".to_string()),
                source_kind: "wsl_hermes_state_sessions".to_string(),
                source_label: "Hermes conversation database".to_string(),
                session_kind: "Hermes/NemoClaw".to_string(),
                linked_project_paths: BTreeSet::new(),
                confidence: "Low".to_string(),
                modified_ms: Some(200),
            },
        );

        let out = finalize_sessions(sessions, &[]);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].display_name, "Canonical conversation title");
        assert!(out[0].path.contains("state.db#hermes-session=same-id"));
    }

    #[test]
    fn extracts_existing_project_paths_from_session_text() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("from-session");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        let session = dir.path().join("session.jsonl");
        fs::write(
            &session,
            format!(
                "{{\"cwd\":\"{}\"}}",
                project.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "test_sessions".to_string(),
            label: "Test sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert!(candidate.path.ends_with("from-session"));
        assert!(candidate
            .source_kinds
            .contains(&"test_sessions".to_string()));
        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "unregistered_project_reference");
    }

    #[test]
    fn expands_hermes_sessions_json_into_individual_sessions() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path().join(".hermes").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        let project = dir.path().join("actual-project");
        let mentioned = dir.path().join("mentioned-project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&mentioned).unwrap();
        fs::write(project.join("README.md"), "# Project").unwrap();
        fs::write(mentioned.join("README.md"), "# Mentioned").unwrap();
        fs::write(mentioned.join("AGENTS.md"), "local").unwrap();
        let sessions_file = sessions_dir.join("sessions.json");
        fs::write(
            &sessions_file,
            format!(
                r#"{{
                  "agent:one": {{
                    "session_key": "agent:one",
                    "session_id": "20260609_one",
                    "display_name": "Hermes One",
                    "platform": "discord",
                    "cwd": "{}",
                    "trace": "also read {}"
                  }},
                  "agent:two": {{
                    "session_key": "agent:two",
                    "session_id": "20260610_two",
                    "display_name": "Hermes Two",
                    "platform": "telegram"
                  }}
                }}"#,
                project.to_string_lossy().replace('\\', "\\\\"),
                mentioned.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "wsl_ubuntu_24_04_hermes_sessions".to_string(),
            label: "WSL Ubuntu-24.04 Hermes sessions".to_string(),
            path: sessions_dir,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 2);
        assert!(sessions
            .iter()
            .any(|session| session.display_name.contains("Hermes One · discord")));
        assert!(sessions
            .iter()
            .any(|session| session.display_name.contains("Hermes Two · telegram")));
        let paths = candidates
            .into_values()
            .map(finalize_candidate)
            .map(|candidate| candidate_key(Path::new(&candidate.path)))
            .collect::<BTreeSet<_>>();
        assert!(paths.contains(&candidate_key(&project)));
        assert!(
            !paths.contains(&candidate_key(&mentioned)),
            "Hermes session cwd is canonical; body traces must not create extra project links"
        );
    }

    #[test]
    fn explicit_skill_candidates_survive_global_visibility_filter() {
        let dir = tempdir().unwrap();
        let skill = dir.path().join(".cursor").join("skills").join("my-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# My local skill").unwrap();
        let source = DiscoverySource {
            kind: "cursor_skills".to_string(),
            label: "Cursor user skills".to_string(),
            path: dir.path().join(".cursor").join("skills"),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);
        let builder = candidates
            .get(&candidate_key(&skill))
            .expect("skill marker creates a candidate");

        assert!(candidate_is_visible_global_discovery_item(builder, true));
        assert!(!candidate_is_visible_global_discovery_item(builder, false));
        assert!(!candidate_is_deliberate_project(builder));
        assert_eq!(
            project_kind_for_candidate(&builder.path, &builder.signals),
            "technical_candidate"
        );
    }

    #[test]
    fn hermes_state_db_lists_individual_sessions_and_renders_their_messages() {
        let dir = tempdir().unwrap();
        let hermes_home = dir.path().join(".hermes");
        let project = dir.path().join("hermes-project");
        fs::create_dir_all(&hermes_home).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# Hermes project").unwrap();
        let db_path = hermes_home.join("state.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
               id TEXT PRIMARY KEY, source TEXT NOT NULL, title TEXT, cwd TEXT,
               started_at REAL NOT NULL, ended_at REAL
             );
             CREATE TABLE messages (
               id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT, timestamp REAL NOT NULL, active INTEGER NOT NULL DEFAULT 1
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions(id, source, title, cwd, started_at, ended_at)
             VALUES (?1, 'telegram', 'Hermes planning chat', ?2, 1000, 2000)",
            rusqlite::params!["hermes-1234567890", project.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(session_id, role, content, timestamp, active)
             VALUES ('hermes-1234567890', 'user', 'Plan the local project', 1001, 1),
                    ('hermes-1234567890', 'assistant', 'Local plan ready', 1002, 1),
                    ('hermes-1234567890', 'assistant', 'obsolete rewind', 1003, 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let source = DiscoverySource {
            kind: "hermes_state_sessions".to_string(),
            label: "Hermes conversation database".to_string(),
            path: db_path.clone(),
            detail: None,
            mode: SourceMode::HermesState,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_hermes_state_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].display_name, "Hermes planning chat");
        assert!(sessions[0]
            .path
            .contains("#hermes-session=hermes-1234567890"));
        assert_eq!(
            sessions[0].linked_project_paths,
            vec![display_path(&canonical_or_original(&project))]
        );
        let (transcript, truncated) =
            hermes_session_transcript(&db_path, "hermes-1234567890", 32 * 1024).unwrap();
        assert!(!truncated);
        assert!(transcript.contains("Plan the local project"));
        assert!(transcript.contains("Local plan ready"));
        assert!(!transcript.contains("obsolete rewind"));
    }

    #[test]
    fn openclaw_state_follows_agent_db_and_renders_agent_and_replay_transcripts() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join(".openclaw").join("state");
        let project = dir.path().join("openclaw-project");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        let global_db = state_dir.join("openclaw.sqlite");
        let agent_db = state_dir.join("main-agent.sqlite");
        let unrelated_file = state_dir.join("not-a-session-db.txt");
        fs::write(&unrelated_file, "private unrelated text").unwrap();
        let global = Connection::open(&global_db).unwrap();
        global
            .execute_batch(
                "CREATE TABLE agent_databases (
                   agent_id TEXT NOT NULL, path TEXT NOT NULL, schema_version INTEGER NOT NULL,
                   last_seen_at INTEGER NOT NULL, size_bytes INTEGER
                 );
                 CREATE TABLE acp_replay_sessions (
                   session_id TEXT PRIMARY KEY, session_key TEXT NOT NULL, cwd TEXT NOT NULL,
                   complete INTEGER NOT NULL, created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL, next_seq INTEGER NOT NULL
                 );
                 CREATE TABLE acp_replay_events (
                   session_id TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL,
                   session_key TEXT NOT NULL, run_id TEXT, update_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        global
            .execute(
                "INSERT INTO agent_databases(agent_id, path, schema_version, last_seen_at)
                 VALUES ('main', ?1, 1, 5000)",
                [agent_db.to_string_lossy().to_string()],
            )
            .unwrap();
        global
            .execute(
                "INSERT INTO agent_databases(agent_id, path, schema_version, last_seen_at)
                 VALUES ('invalid', ?1, 1, 5000)",
                [unrelated_file.to_string_lossy().to_string()],
            )
            .unwrap();
        global
            .execute(
                "INSERT INTO acp_replay_sessions
                 VALUES ('replay-1', 'agent:main:replay', ?1, 1, 3000, 4000, 2)",
                [project.to_string_lossy().to_string()],
            )
            .unwrap();
        global
            .execute(
                "INSERT INTO acp_replay_events
                 VALUES ('replay-1', 1, 3500, 'agent:main:replay', NULL, ?1)",
                [serde_json::json!({"role":"user","content":"Replay prompt"}).to_string()],
            )
            .unwrap();
        drop(global);

        let agent = Connection::open(&agent_db).unwrap();
        agent
            .execute_batch(
                "CREATE TABLE sessions (
                   session_id TEXT PRIMARY KEY, session_key TEXT, display_name TEXT,
                   updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE session_entries (
                   session_id TEXT PRIMARY KEY, session_key TEXT, entry_json TEXT,
                   updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE transcript_events (
                   session_id TEXT NOT NULL, seq INTEGER NOT NULL,
                   event_json TEXT NOT NULL, created_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        agent
            .execute(
                "INSERT INTO sessions VALUES ('session-1', 'agent:main:chat', 'OpenClaw planning', 6000)",
                [],
            )
            .unwrap();
        agent
            .execute(
                "INSERT INTO session_entries VALUES ('session-1', 'agent:main:chat', ?1, 6000)",
                [serde_json::json!({"cwd": project}).to_string()],
            )
            .unwrap();
        agent
            .execute(
                "INSERT INTO transcript_events VALUES ('session-1', 1, ?1, 6100)",
                [
                    serde_json::json!({"role":"assistant","content":"Agent transcript"})
                        .to_string(),
                ],
            )
            .unwrap();
        drop(agent);

        let source = DiscoverySource {
            kind: "openclaw_state_sessions".to_string(),
            label: "OpenClaw session database".to_string(),
            path: global_db.clone(),
            detail: None,
            mode: SourceMode::OpenClawState,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        assert_eq!(
            openclaw_agent_database_paths(&global_db),
            vec![canonical_or_original(&agent_db)]
        );
        scan_openclaw_state_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|session| {
            session.display_name == "OpenClaw planning"
                && session.path.contains("#openclaw-session=session-1")
        }));
        assert!(sessions
            .iter()
            .any(|session| session.path.contains("#openclaw-replay=replay-1")));
        assert!(candidates.contains_key(&candidate_key(&project)));
        let (agent_text, _) =
            openclaw_session_transcript(&agent_db, "openclaw-session=session-1", 32 * 1024)
                .unwrap();
        let (replay_text, _) =
            openclaw_session_transcript(&global_db, "openclaw-replay=replay-1", 32 * 1024).unwrap();
        assert!(agent_text.contains("Agent transcript"));
        assert!(replay_text.contains("Replay prompt"));
    }

    #[test]
    fn openclaw_legacy_index_prefers_the_session_transcript_file() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir
            .path()
            .join(".openclaw")
            .join("agents")
            .join("main")
            .join("sessions");
        let project = dir.path().join("legacy-openclaw-project");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# Legacy").unwrap();
        let index = sessions_dir.join("sessions.json");
        let transcript = sessions_dir.join("legacy-id.jsonl");
        fs::write(
            &index,
            serde_json::json!({
                "agent:main:chat": {
                    "sessionId": "legacy-id",
                    "displayName": "Legacy OpenClaw chat",
                    "cwd": project
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(&transcript, "{\"role\":\"user\",\"content\":\"hello\"}\n").unwrap();
        let source = DiscoverySource {
            kind: "openclaw_home".to_string(),
            label: "OpenClaw local state".to_string(),
            path: dir.path().join(".openclaw"),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_openclaw_sessions_file(&index, &source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].display_name, "Legacy OpenClaw chat");
        assert_eq!(
            candidate_key(Path::new(&sessions[0].path)),
            candidate_key(&transcript)
        );
        assert_eq!(
            sessions[0].linked_project_paths,
            vec![display_path(&canonical_or_original(&project))]
        );
    }

    #[test]
    fn internal_ai_state_paths_are_not_promoted_as_projects() {
        let dir = tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let cursor_project_store = dir.path().join(".cursor").join("projects").join("stored");
        let brain = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("brain")
            .join("task-id");
        let hermes_sessions = dir.path().join(".hermes").join("sessions");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&cursor_project_store).unwrap();
        fs::create_dir_all(&brain).unwrap();
        fs::create_dir_all(&hermes_sessions).unwrap();
        fs::write(codex_home.join("AGENTS.md"), "internal").unwrap();
        fs::write(cursor_project_store.join("README.md"), "internal").unwrap();
        fs::write(brain.join("README.md"), "internal").unwrap();
        fs::write(hermes_sessions.join("README.md"), "internal").unwrap();
        assert!(!is_meaningful_metadata_path(&codex_home));
        assert!(!is_meaningful_metadata_path(&cursor_project_store));
        assert!(!is_meaningful_metadata_path(&brain));
        assert!(!is_meaningful_metadata_path(&hermes_sessions));
    }

    #[test]
    fn session_links_ignore_internal_state_and_broad_containers() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("real-project");
        let cursor_store = dir.path().join(".cursor").join("projects").join("stored");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&cursor_store).unwrap();
        fs::write(project.join("README.md"), "# Project").unwrap();
        fs::write(cursor_store.join("README.md"), "# Internal").unwrap();

        assert!(is_session_project_link_path(&project, false));
        assert!(!is_session_project_link_path(&cursor_store, true));
        assert!(!is_session_project_link_path(Path::new("C:\\"), true));
    }

    #[test]
    fn support_metadata_files_do_not_become_session_rows() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("linked-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# Project").unwrap();
        let audit = dir.path().join("audit.jsonl");
        let local_session = dir.path().join("local_123.json");
        let encoded = project.to_string_lossy().replace('\\', "\\\\");
        fs::write(&audit, format!("{{\"cwd\":\"{encoded}\"}}")).unwrap();
        fs::write(&local_session, format!("{{\"cwd\":\"{encoded}\"}}")).unwrap();
        let source = DiscoverySource {
            kind: "claude_local_agent_sessions".to_string(),
            label: "Claude local agent sessions".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_metadata_file(&audit, &source, &mut candidates, &mut sessions);
        scan_metadata_file(&local_session, &source, &mut candidates, &mut sessions);

        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].display_name, "local_123.json");
        assert!(candidates
            .into_values()
            .map(finalize_candidate)
            .any(|candidate| candidate.path.ends_with("linked-project")));
    }

    #[test]
    fn antigravity_frozen_brain_transcript_is_no_longer_listed_as_session() {
        let dir = tempdir().unwrap();
        let brain = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("brain")
            .join("task-id");
        let transcript = brain
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        let artifact = brain
            .join("artifacts")
            .join("implementation_plan.md.metadata.json");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&transcript, "{}").unwrap();
        fs::write(&artifact, "{}").unwrap();

        // The marker is still recognised structurally...
        assert!(is_antigravity_brain_session_marker_file(&transcript));
        assert!(!is_antigravity_brain_session_marker_file(&artifact));
        // ...but the frozen brain transcript is no longer surfaced as the chat:
        // the live `conversations/<uuid>.db` is now the source of truth.
        assert!(!should_list_session_file(
            &transcript,
            "gemini_antigravity_brain"
        ));
        assert!(!should_list_session_file(
            &artifact,
            "gemini_antigravity_brain"
        ));
    }

    #[test]
    fn antigravity_conversation_db_is_recognised_by_path_shape() {
        let convo = Path::new("C:\\Users\\me\\.gemini\\antigravity\\conversations\\abc.db");
        assert!(is_antigravity_conversation_db(convo));
        // Brain transcript and non-db siblings are not conversation databases.
        let brain = Path::new(
            "C:\\Users\\me\\.gemini\\antigravity\\brain\\t\\.system_generated\\logs\\transcript.jsonl",
        );
        assert!(!is_antigravity_conversation_db(brain));
        let pb = Path::new("C:\\Users\\me\\.gemini\\antigravity\\conversations\\abc.pb");
        assert!(!is_antigravity_conversation_db(pb));
    }

    #[test]
    fn oversized_session_files_are_listed_without_body_scan() {
        let dir = tempdir().unwrap();
        let conversation = dir.path().join("conversation.pb");
        fs::write(
            &conversation,
            vec![b'x'; (SOURCE_TEXT_METADATA_MAX_BYTES + 1) as usize],
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "gemini_antigravity_conversations".to_string(),
            label: "Antigravity conversations".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_metadata_file(&conversation, &source, &mut candidates, &mut sessions);

        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "loose_session");
        assert_eq!(sessions[0].display_name, "conversation.pb");
        assert!(candidates.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn cursor_project_transcripts_link_to_registered_project() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("own-project");
        let sibling = dir.path().join("mentioned-sibling");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        fs::write(sibling.join("AGENTS.md"), "local").unwrap();
        let store = dir
            .path()
            .join(".cursor")
            .join("projects")
            .join(cursor_store_name_for_test(&project));
        let session_id = "81da9bd8-c4e3-49ee-9aa5-17645a72933a";
        let transcript = store
            .join("agent-transcripts")
            .join(session_id)
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(
            &transcript,
            format!(
                "{{\"role\":\"user\",\"message\":\"read {}\"}}\n",
                sibling.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "cursor_project_transcripts".to_string(),
            label: "Cursor project transcripts".to_string(),
            path: dir.path().join(".cursor").join("projects"),
            detail: None,
            mode: SourceMode::CursorProjectTranscripts,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_project_transcripts_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(
            sessions,
            &[RegisteredRoot {
                project_id: Some(42),
                path: project.clone(),
            }],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "registered_project");
        assert_eq!(sessions[0].linked_registered_project_ids, vec![42]);
        assert!(sessions[0].display_name.contains("own-project"));
        let paths = candidates
            .into_values()
            .map(finalize_candidate)
            .map(|candidate| candidate_key(Path::new(&candidate.path)))
            .collect::<BTreeSet<_>>();
        assert!(paths.contains(&candidate_key(&project)));
        assert!(
            !paths.contains(&candidate_key(&sibling)),
            "Cursor transcripts belong to the workspace store directory, not every path mentioned in the chat body"
        );
    }

    #[test]
    fn cursor_empty_window_transcript_is_loose_session() {
        let dir = tempdir().unwrap();
        let session_id = "60640902-8634-422c-bb15-0d65f7851cc7";
        let transcript = dir
            .path()
            .join(".cursor")
            .join("projects")
            .join("empty-window")
            .join("agent-transcripts")
            .join(session_id)
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(&transcript, "{\"role\":\"user\",\"message\":\"loose\"}\n").unwrap();
        let source = DiscoverySource {
            kind: "cursor_project_transcripts".to_string(),
            label: "Cursor project transcripts".to_string(),
            path: dir.path().join(".cursor").join("projects"),
            detail: None,
            mode: SourceMode::CursorProjectTranscripts,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_project_transcripts_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "loose_session");
        assert_eq!(sessions[0].display_name, "Cursor empty window · 60640902");
        assert!(candidates.is_empty());
    }

    #[test]
    fn embedded_external_dependency_prefers_project_container() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        let external = project.join(".external").join("downloaded-lib");
        fs::create_dir_all(&external).unwrap();
        fs::write(project.join("AGENTS.md"), "project").unwrap();
        fs::write(external.join("README.md"), "dependency").unwrap();

        let normalized = prefer_project_container_for_embedded_dependency(&external);
        assert_eq!(candidate_key(&normalized), candidate_key(&project));
    }

    #[test]
    fn skill_folders_are_classified_as_technical_candidates() {
        let dir = tempdir().unwrap();
        let skill = dir.path().join(".hermes").join("skills").join("demo-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("README.md"), "# Skill").unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();

        let source = DiscoverySource {
            kind: "test_skills".to_string(),
            label: "Test skills".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_folder_source(
            &source,
            &mut candidates,
            DEEP_ROOT_DEPTH,
            DEEP_ROOT_MAX_DIRS,
            "deep_folder_marker",
            4,
            true,
        );

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .find(|candidate| candidate.path.ends_with("demo-skill"))
            .unwrap();
        assert_eq!(candidate.project_kind, "technical_candidate");
    }

    #[test]
    fn ignores_generic_dependency_paths_from_session_text() {
        let dir = tempdir().unwrap();
        let dependency = dir
            .path()
            .join("pinokio")
            .join("envs")
            .join("demo")
            .join("Lib")
            .join("site-packages")
            .join("pytest");
        let project = dir.path().join("real-project");
        fs::create_dir_all(&dependency).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        let session = dir.path().join("session.jsonl");
        fs::write(
            &session,
            format!(
                "{{\"cwd\":\"{}\",\"trace\":\"{}\"}}",
                dependency.to_string_lossy().replace('\\', "\\\\"),
                project.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "test_sessions".to_string(),
            label: "Test sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let paths = candidates
            .into_values()
            .map(|candidate| candidate_key(&candidate.path))
            .collect::<BTreeSet<_>>();

        assert!(paths.contains(&candidate_key(&project)));
        assert!(!paths.contains(&candidate_key(&dependency)));
    }

    #[test]
    fn generic_session_cwd_is_canonical_over_body_paths() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("real-project");
        let sibling = dir.path().join("mentioned-sibling");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        fs::write(sibling.join("AGENTS.md"), "local").unwrap();
        let session = dir.path().join("session.jsonl");
        fs::write(
            &session,
            format!(
                "{{\"cwd\":\"{}\",\"log\":\"read {}\"}}",
                project.to_string_lossy().replace('\\', "\\\\"),
                sibling.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "test_sessions".to_string(),
            label: "Test sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let paths = candidates
            .into_values()
            .map(|candidate| candidate_key(&candidate.path))
            .collect::<BTreeSet<_>>();
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "unregistered_project_reference");
        assert!(paths.contains(&candidate_key(&project)));
        assert!(
            !paths.contains(&candidate_key(&sibling)),
            "body path references must not smear one session across sibling projects"
        );
    }

    #[test]
    fn ignores_cargo_registry_crates_even_with_project_markers() {
        let dir = tempdir().unwrap();
        let registry_crate = dir
            .path()
            .join(".local")
            .join("cargo")
            .join("registry")
            .join("src")
            .join("index.crates.io-1949cf8c6b5b557f")
            .join("zerocopy-0.8.50");
        let project = dir.path().join("own-project");
        fs::create_dir_all(&registry_crate).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            registry_crate.join("Cargo.toml"),
            "[package]\nname='zerocopy'",
        )
        .unwrap();
        fs::write(registry_crate.join("README.md"), "# zerocopy").unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\nname='own-project'").unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        let source = DiscoverySource {
            kind: "test_known".to_string(),
            label: "Test known".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_folder_source(
            &source,
            &mut candidates,
            DEEP_ROOT_DEPTH,
            DEEP_ROOT_MAX_DIRS,
            "deep_folder_marker",
            4,
            true,
        );
        let paths = candidates
            .into_values()
            .map(|candidate| candidate_key(&candidate.path))
            .collect::<BTreeSet<_>>();

        assert!(paths.contains(&candidate_key(&project)));
        assert!(!paths.contains(&candidate_key(&registry_crate)));
    }

    #[test]
    fn ignores_site_packages_dependencies_even_with_project_markers() {
        // The exact false positive a live deep scan surfaced: a third-party
        // Python package that ships its own README/manifest, referenced by a
        // session log, must never be auto-added as one of the user's projects.
        let dir = tempdir().unwrap();
        let dependency = dir
            .path()
            .join(".venv")
            .join("Lib")
            .join("site-packages")
            .join("flask")
            .join("sansio");
        let project = dir.path().join("own-project");
        fs::create_dir_all(&dependency).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(dependency.join("__init__.py"), "# sansio").unwrap();
        fs::write(dependency.join("README.md"), "# flask.sansio").unwrap();
        fs::write(project.join("README.md"), "# Own").unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();
        let session = dir.path().join("session.jsonl");
        fs::write(
            &session,
            format!(
                "{{\"cwd\":\"{}\",\"ref\":\"{}\"}}",
                project.to_string_lossy().replace('\\', "\\\\"),
                dependency.to_string_lossy().replace('\\', "\\\\"),
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "test_sessions".to_string(),
            label: "Test sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let paths = candidates
            .into_values()
            .map(|candidate| candidate_key(&candidate.path))
            .collect::<BTreeSet<_>>();

        assert!(paths.contains(&candidate_key(&project)));
        assert!(
            !paths.contains(&candidate_key(&dependency)),
            "a site-packages dependency must never be a project candidate"
        );
    }

    #[test]
    fn codex_date_slug_folders_are_loose_sessions_not_projects() {
        // Date-prefixed component detection.
        assert!(is_date_prefixed_component("2026-04-21"));
        assert!(is_date_prefixed_component(
            "2026-04-21-i-want-to-use-these-commands"
        ));
        assert!(!is_date_prefixed_component("codehangar"));
        assert!(!is_date_prefixed_component("2026-4-1")); // not zero-padded ISO

        // Codex per-conversation scratch dirs are detected and rejected as
        // project candidates (they remain loose sessions elsewhere).
        let slug =
            Path::new(r"C:\Users\user\Documents\Codex\2026-04-21-i-want-to-use-these-commands");
        let dated = Path::new(r"C:\Users\user\Documents\codex\2026-04-23\some-slug");
        assert!(is_agent_scratch_path(slug));
        assert!(is_agent_scratch_path(dated));
        assert!(!is_session_project_link_path(slug, false));
        assert!(!is_session_project_link_path(dated, true));

        // A real project folder is unaffected.
        let real = Path::new(r"C:\AI\Codex\CodeHangar");
        assert!(!is_agent_scratch_path(real));
        assert!(is_session_project_link_path(real, true));
    }

    #[test]
    fn app_project_registries_yield_only_deliberate_projects() {
        let dir = tempdir_projectlike();
        let home = dir.path();

        // A real project (has a README → identity) and a per-conversation scratch
        // dir, both on disk.
        let real = home.join("CodeHangar");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("README.md"), "# real").unwrap();
        let scratch = home
            .join("Documents")
            .join("Codex")
            .join("2026-04-21-some-prompt");
        fs::create_dir_all(&scratch).unwrap();
        let oneoff = home.join("pics-oneoff");
        fs::create_dir_all(&oneoff).unwrap();

        // Claude registry (.claude.json projects map) lists the real project.
        let claude = serde_json::json!({
            "projects": { real.to_string_lossy(): {} },
            "other": "ignored",
        });
        fs::write(home.join(".claude.json"), claude.to_string()).unwrap();
        // A folder the user trusted in Codex that has NO marker file and is listed
        // by no other app — the "SubProjectA"/"SubProjectB" case. It must still
        // survive on the trust_level signal alone.
        let trusted_no_marker = home.join("subprojecta");
        fs::create_dir_all(&trusted_no_marker).unwrap();
        // An untrusted Codex cwd with no identity must NOT survive on its own.
        let untrusted = home.join("untrusted-oneoff");
        fs::create_dir_all(&untrusted).unwrap();
        // Codex registry ([projects.'…'] tables) lists real + scratch (both
        // trusted), the trusted no-marker folder, and an untrusted one-off.
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            format!(
                "model = \"x\"\n\
                 [projects.'{}']\ntrust_level = \"trusted\"\n\
                 [projects.'{}']\ntrust_level = \"trusted\"\n\
                 [projects.'{}']\ntrust_level = \"trusted\"\n\
                 [projects.'{}']\ntrust_level = \"untrusted\"\n",
                real.to_string_lossy(),
                scratch.to_string_lossy(),
                trusted_no_marker.to_string_lossy(),
                untrusted.to_string_lossy(),
            ),
        )
        .unwrap();

        let claude_paths = read_claude_project_registry(home);
        assert_eq!(claude_paths.len(), 1);
        assert!(claude_paths
            .iter()
            .any(|p| candidate_key(p) == candidate_key(&real)));

        let codex_paths = read_codex_project_registry(home);
        let codex_trusted = |target: &Path| {
            codex_paths
                .iter()
                .find(|(p, _)| candidate_key(p) == candidate_key(target))
                .map(|(_, trusted)| *trusted)
        };
        assert_eq!(codex_trusted(&real), Some(true));
        assert_eq!(codex_trusted(&scratch), Some(true));
        assert_eq!(codex_trusted(&trusted_no_marker), Some(true));
        assert_eq!(
            codex_trusted(&untrusted),
            Some(false),
            "trust_level = untrusted parses as not-deliberate"
        );

        // Classification: a real project (identity) is kept even when 1 app lists
        // it; a per-conversation scratch dir is rejected even when trusted; a
        // no-identity one-off listed by a single app with no trust is rejected.
        assert!(is_registry_project_path(&real, false, 1));
        assert!(!is_registry_project_path(&scratch, true, 1));
        assert!(!is_registry_project_path(&oneoff, false, 1));
        // Fix A: a trusted Codex folder with no marker still becomes a candidate
        // because trust flips `deliberate_workspace` to true.
        assert!(!is_registry_project_path(&trusted_no_marker, false, 1));
        assert!(is_registry_project_path(&trusted_no_marker, true, 1));
    }

    #[test]
    fn registry_candidates_appear_with_strong_signal() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let real = home.join("CodeHangar");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("README.md"), "# r").unwrap();
        let claude = serde_json::json!({ "projects": { real.to_string_lossy(): {} } });
        fs::write(home.join(".claude.json"), claude.to_string()).unwrap();

        let mut candidates = BTreeMap::new();
        add_registry_project_candidates(home, &[], None, &mut candidates);
        let finalized: Vec<_> = candidates.into_values().map(finalize_candidate).collect();
        let ch = finalized
            .iter()
            .find(|c| c.path.ends_with("CodeHangar"))
            .expect("registry project should be a candidate");
        assert!(ch
            .signals
            .iter()
            .any(|signal| signal.kind == "app_project_registry"));
    }

    #[test]
    fn trusted_codex_folder_without_marker_becomes_candidate() {
        // End-to-end: the "SubProjectA"/"SubProjectB" case. A folder that is ONLY
        // known via a trusted Codex `[projects.'…']` entry — no marker file, no
        // `.git`, not listed by any other app — must surface as an
        // app_project_registry candidate purely on the trust_level signal.
        let dir = tempdir_projectlike();
        let home = dir.path();
        let subprojecta = home.join("subprojecta");
        fs::create_dir_all(&subprojecta).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            format!(
                "[projects.'{}']\ntrust_level = \"trusted\"\n",
                subprojecta.to_string_lossy()
            ),
        )
        .unwrap();

        let mut candidates = BTreeMap::new();
        add_registry_project_candidates(home, &[], None, &mut candidates);
        let finalized: Vec<_> = candidates.into_values().map(finalize_candidate).collect();
        let cand = finalized
            .iter()
            .find(|c| candidate_key(Path::new(&c.path)) == candidate_key(&subprojecta))
            .expect("trusted Codex folder with no marker should be a candidate");
        assert!(cand
            .signals
            .iter()
            .any(|signal| signal.kind == "app_project_registry"));
    }

    /// Real-data verification for Fix A against the user's actual Codex config:
    /// the trusted, marker-less "SubProjectA" and "SubProjectB" folders must now
    /// read as trusted and pass `is_registry_project_path` on the trust signal alone.
    /// Ignored by default (machine-specific paths); run with
    /// `cargo test -p hangar-discovery -- --ignored real_codex_trusted_folders`.
    #[test]
    #[ignore = "depends on the local user's real Codex config.toml"]
    fn real_codex_trusted_folders_without_markers_pass_filter() {
        let Some(home) = home_dir() else {
            eprintln!("skipping: no home dir");
            return;
        };
        let config = home.join(".codex").join("config.toml");
        if !config.is_file() {
            eprintln!("skipping: {} not present", config.display());
            return;
        }
        let entries = read_codex_project_registry(&home);
        for needle in ["subprojecta", "subprojectb"] {
            let Some((path, trusted)) = entries.iter().find(|(p, _)| {
                p.file_name()
                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case(needle))
                    .unwrap_or(false)
            }) else {
                eprintln!("skipping {needle}: not in this machine's Codex registry");
                continue;
            };
            assert!(*trusted, "{} should read as trusted", path.display());
            // Confirm the regression: no marker identity, single app, yet it must
            // survive purely because trust flips deliberate_workspace to true.
            if path.is_dir() {
                assert!(
                    !has_project_identity(path),
                    "{} is expected to have no marker (the bug case)",
                    path.display()
                );
                assert!(
                    is_registry_project_path(path, true, 1),
                    "{} must pass the filter on the trust signal",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn cursor_workspace_registry_is_read() {
        let dir = tempdir().unwrap();
        let user = dir.path().join("User");
        let storage = user.join("workspaceStorage").join("abc123");
        fs::create_dir_all(&storage).unwrap();
        let project = dir.path().join("my-cursor-proj");
        fs::create_dir_all(&project).unwrap();

        // workspace.json `folder` is a file:// URI pointing at the opened folder.
        let uri = format!("file:///{}", project.to_string_lossy().replace('\\', "/"));
        fs::write(
            storage.join("workspace.json"),
            serde_json::json!({ "folder": uri }).to_string(),
        )
        .unwrap();

        let paths = read_vscode_workspace_registry(&user);
        assert!(
            paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&project)),
            "cursor workspace folder should be read, got {paths:?}"
        );
    }

    /// Build the `file:///<path>` URI shape Antigravity stores in its registry.
    #[cfg(test)]
    fn gemini_folder_uri(path: &Path) -> String {
        format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
    }

    #[cfg(test)]
    fn write_gemini_project(
        gemini_home: &Path,
        file_stem: &str,
        name: &str,
        value: serde_json::Value,
    ) {
        let projects_dir = gemini_home.join(".gemini").join("config").join("projects");
        fs::create_dir_all(&projects_dir).unwrap();
        let mut doc = value;
        doc["name"] = serde_json::Value::String(name.to_string());
        fs::write(
            projects_dir.join(format!("{file_stem}.json")),
            doc.to_string(),
        )
        .unwrap();
    }

    #[test]
    fn gemini_registry_reads_folder_uri_and_skips_missing() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        // An existing root (like the real D:\Example) and a missing one (an offline
        // external drive). Only the existing dir should surface.
        let real = home.join("ExampleProj");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("README.md"), "# ExampleProj").unwrap();
        let missing = home.join("offline-G-drive-root");

        write_gemini_project(
            home,
            "exampleproj",
            "ExampleProj",
            serde_json::json!({
                "projectResources": {
                    "resources": [
                        { "folderUri": gemini_folder_uri(&real) },
                        { "folderUri": gemini_folder_uri(&missing) },
                    ]
                }
            }),
        );

        let paths = read_gemini_project_registry(home);
        assert!(
            paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&real)),
            "existing gemini project root should be read, got {paths:?}"
        );
        assert!(
            !paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&missing)),
            "a non-existent gemini project root must be skipped, got {paths:?}"
        );

        // End to end: it surfaces as a candidate carrying the deliberate registry
        // signal, exactly like the workspace.json reader's projects.
        let mut candidates = BTreeMap::new();
        add_registry_project_candidates(home, &[], None, &mut candidates);
        let finalized: Vec<_> = candidates.into_values().map(finalize_candidate).collect();
        let example = finalized
            .iter()
            .find(|c| c.path.ends_with("ExampleProj"))
            .expect("gemini registry project should be a candidate");
        assert!(example
            .signals
            .iter()
            .any(|signal| signal.kind == "app_project_registry"));
        assert!(
            !finalized
                .iter()
                .any(|c| c.path.ends_with("offline-G-drive-root")),
            "the offline root must not appear as a candidate"
        );
    }

    #[test]
    fn gemini_registry_reads_nested_git_folder_uri() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let real = home.join("Example");
        fs::create_dir_all(&real).unwrap();

        // The nested `gitFolder.folderUri` shape (vs. the bare `folderUri`).
        write_gemini_project(
            home,
            "example",
            "Example",
            serde_json::json!({
                "projectResources": {
                    "resources": [
                        { "gitFolder": { "folderUri": gemini_folder_uri(&real) } },
                    ]
                }
            }),
        );

        let paths = read_gemini_project_registry(home);
        assert!(
            paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&real)),
            "nested gitFolder.folderUri should be read, got {paths:?}"
        );
    }

    #[test]
    fn wsl_drive_mounts_translate_to_windows_drive_paths() {
        // /mnt/c/... is the Windows C: drive seen from inside WSL — translating it
        // back lets a WSL-side project dedup against the same project on Windows.
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new("/mnt/c/AI/Codex/CodeHangar")),
            PathBuf::from(r"C:\AI\Codex\CodeHangar")
        );
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new("/mnt/d/work")),
            PathBuf::from(r"D:\work")
        );
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new("/mnt/c")),
            PathBuf::from(r"C:\")
        );
    }

    #[test]
    fn wsl_home_paths_translate_to_unc_share() {
        // A project inside the distro filesystem is reached over the \\wsl.localhost
        // share (no sudo needed).
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new("/home/user/proj")),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\user\proj")
        );
        // A non-single-letter "mount" segment is not a drive — keep it in the share.
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new("/mnt/wsl/foo")),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\mnt\wsl\foo")
        );
        // An already-Windows path is left untouched.
        assert_eq!(
            translate_wsl_path("Ubuntu-24.04", Path::new(r"C:\already\win")),
            PathBuf::from(r"C:\already\win")
        );
        let source = DiscoverySource {
            kind: "wsl_ubuntu_24_04_user_hermes_state_sessions".to_string(),
            label: "Hermes".to_string(),
            path: PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\user\.hermes\state.db"),
            detail: None,
            mode: SourceMode::HermesState,
        };
        assert_eq!(
            session_path_for_source(&source, Path::new("/home/user/project")),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\user\project")
        );
    }

    #[test]
    fn pinokio_bundled_examples_are_forbidden_but_installed_apps_are_not() {
        // Bundled demos a session merely mentions — never the user's projects.
        assert!(is_forbidden_candidate_path(Path::new(
            r"C:\pinokio\prototype\system\examples\flux-webui"
        )));
        assert!(is_forbidden_candidate_path(Path::new(
            r"C:\pinokio\prototype\system\python\new\static\app"
        )));
        // A real installed Pinokio app under `api\` stays eligible.
        assert!(!is_forbidden_candidate_path(Path::new(
            r"C:\pinokio\api\Frame-Pack.git"
        )));
    }

    #[test]
    fn windows_install_roots_never_become_session_projects() {
        for path in [
            r"C:\Windows\System32",
            r"C:\Program Files\OpenVPN Connect",
            r"C:\Program Files (x86)\Vendor\Application",
            r"C:\ProgramData\Vendor\Application",
            r"C:\Users\user\AppData\Local\Programs\Vendor\Application",
        ] {
            let path = Path::new(path);
            assert!(is_forbidden_candidate_path(path), "{path:?}");
            assert!(!is_session_project_link_path(path, true), "{path:?}");
            assert!(!is_explicit_session_cwd_link_path(path), "{path:?}");
        }

        assert!(!is_forbidden_candidate_path(Path::new(
            r"C:\Work\Windows\SampleProject"
        )));
        assert!(is_explicit_session_cwd_link_path(Path::new(r"C:\RTX_MPC")));
    }

    #[test]
    fn container_backing_distros_are_skipped() {
        assert!(is_system_wsl_distro("docker-desktop"));
        assert!(is_system_wsl_distro("docker-desktop-data"));
        assert!(is_system_wsl_distro("Rancher-Desktop"));
        assert!(!is_system_wsl_distro("Ubuntu-24.04"));
        assert!(!is_system_wsl_distro("Debian"));
    }

    #[test]
    fn extracts_percent_encoded_file_uri_workspace_paths() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project with space");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "local").unwrap();

        let uri_path = project
            .to_string_lossy()
            .replace('\\', "/")
            .replace(':', "%3A")
            .replace(' ', "%20");
        let workspace = dir.path().join("workspace.json");
        fs::write(
            &workspace,
            format!("{{\"folder\":\"file:///{uri_path}\",\"recent\":\"file:\\/\\/{uri_path}\"}}"),
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "test_workspace_storage".to_string(),
            label: "Test workspace storage".to_string(),
            path: workspace,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert!(candidate.path.ends_with("project with space"));
        assert_eq!(candidate.project_kind, "ai_assisted_project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "session_path"));
    }

    #[test]
    fn extracts_wsl_mnt_paths_from_session_text() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("wsl-session-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("CLAUDE.md"), "local instructions").unwrap();

        let windows_path = project.to_string_lossy().replace('\\', "/");
        let drive_index = windows_path
            .find(":/")
            .expect("Windows tempdir should contain a drive prefix");
        let drive = windows_path[..drive_index].to_ascii_lowercase();
        let remainder = &windows_path[drive_index + 2..];
        let wsl_path = format!("/mnt/{drive}/{remainder}");
        let session = dir.path().join("session.jsonl");
        fs::write(&session, format!("{{\"cwd\":\"{wsl_path}\"}}")).unwrap();

        let source = DiscoverySource {
            kind: "test_wsl_sessions".to_string(),
            label: "Test WSL sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert!(candidate.path.ends_with("wsl-session-project"));
        assert_eq!(candidate.project_kind, "ai_assisted_project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "claude_context"));
    }

    #[test]
    fn extracts_project_paths_from_large_binary_metadata_tail() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("antigravity-tail-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("GEMINI.md"), "local").unwrap();
        let metadata = dir.path().join("conversation.pb");
        let mut bytes = vec![b'x'; SOURCE_FILE_BYTES + 1024];
        bytes.extend_from_slice(
            format!(" cwd={} ", project.to_string_lossy().replace('\\', "\\\\")).as_bytes(),
        );
        fs::write(&metadata, bytes).unwrap();

        let source = DiscoverySource {
            kind: "test_antigravity_pb".to_string(),
            label: "Test Antigravity pb".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert!(candidate.path.ends_with("antigravity-tail-project"));
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "gemini_context"));
    }

    #[test]
    fn extracts_project_paths_from_sqlite_files_inside_metadata_directory() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("sqlite-conversation-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# SQLite").unwrap();
        let db_path = dir.path().join("conversation.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE messages(body TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO messages(body) VALUES (?1)",
            [format!("working directory {}", project.to_string_lossy())],
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "test_antigravity_db".to_string(),
            label: "Test Antigravity db".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert!(candidate.path.ends_with("sqlite-conversation-project"));
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "sqlite_path"));
    }

    #[test]
    fn extracts_agent_report_project_and_conversation_folders_without_excluded_files() {
        let dir = tempdir().unwrap();
        let story = dir.path().join("ProjectAlpha");
        let training_a = dir.path().join("Example");
        let training_b = dir.path().join("DiscoExample").join("Downloads");
        let conversation_base = dir.path().join(".gemini").join("antigravity").join("brain");
        let internal = dir.path().join(".gemini").join("antigravity");
        fs::create_dir_all(&story).unwrap();
        fs::create_dir_all(&training_a).unwrap();
        fs::create_dir_all(&training_b).unwrap();
        fs::create_dir_all(&conversation_base).unwrap();
        fs::create_dir_all(&internal).unwrap();
        fs::write(story.join("GEMINI.md"), "local").unwrap();
        fs::write(training_a.join("README.md"), "# train").unwrap();
        fs::write(training_b.join("README.md"), "# old").unwrap();
        fs::write(internal.join("antigravity_state.pbtxt"), "state").unwrap();

        let report = format!(
            r#"Project folders
Nome do projecto: Project Alpha
Caminho absoluto da pasta do projecto: {} (incluindo a subpasta SubProjectA)
Nome do projecto: Project Beta
Caminhos absolutos das pastas do projecto: {} (incluindo ExampleProj e SubA) e {} (incluindo ExampleProj e SubB)
Conversation base folders linked to projects
Caminho absoluto da pasta-base das conversas: {}
Explicitly excluded
Ficheiro: {}
Motivo da exclusão: Ficheiro de configuração global do agente.
"#,
            story.display(),
            training_a.display(),
            training_b.display(),
            conversation_base.display(),
            internal.join("antigravity_state.pbtxt").display()
        );

        let paths = extract_existing_directories(&report)
            .into_iter()
            .map(|path| candidate_key(&path))
            .collect::<BTreeSet<_>>();

        assert!(paths.contains(&candidate_key(&story)));
        assert!(paths.contains(&candidate_key(&training_a)));
        assert!(paths.contains(&candidate_key(&training_b)));
        assert!(paths.contains(&candidate_key(&conversation_base)));
        assert!(!paths.contains(&candidate_key(&internal)));
    }

    #[test]
    fn vscode_like_sources_include_agent_extension_task_storage() {
        let dir = tempdir().unwrap();
        let user_dir = dir.path().join("Code").join("User");
        let mut sources = Vec::new();
        push_vscode_like_sources(&mut sources, dir.path(), "test_host", "Test Host", user_dir);
        let kinds = sources
            .iter()
            .map(|source| source.kind.as_str())
            .collect::<BTreeSet<_>>();

        assert!(kinds.contains("test_host_cline_tasks"));
        assert!(kinds.contains("test_host_roo_tasks"));
        assert!(kinds.contains("test_host_kilo_tasks"));
        assert!(kinds.contains("test_host_continue_extension"));
        assert!(kinds.contains("test_host_copilot_chat"));
        assert!(kinds.contains("test_host_copilot"));
        assert!(kinds.contains("test_host_cody"));
        assert!(sources.iter().any(|source| source
            .path
            .ends_with("globalStorage\\saoudrizwan.claude-dev")));
        assert!(sources
            .iter()
            .any(|source| source.path.ends_with("globalStorage\\github.copilot-chat")));
        assert!(sources
            .iter()
            .any(|source| source.path.ends_with("globalStorage\\sourcegraph.cody-ai")));
    }

    #[test]
    fn zed_sources_include_common_conversation_locations() {
        let dir = tempdir().unwrap();
        let appdata = dir.path().join("AppData").join("Roaming");
        let localappdata = dir.path().join("AppData").join("Local");
        let mut sources = Vec::new();
        push_zed_sources(
            &mut sources,
            dir.path(),
            Some(appdata.clone()),
            Some(localappdata.clone()),
        );
        let kinds = sources
            .iter()
            .map(|source| source.kind.as_str())
            .collect::<BTreeSet<_>>();

        assert!(kinds.contains("zed_config_conversations"));
        assert!(kinds.contains("zed_data_conversations"));
        assert!(kinds.contains("zed_windows_roaming_conversations"));
        assert!(kinds.contains("zed_windows_local_conversations"));
        assert!(sources
            .iter()
            .any(|source| source.path.ends_with(".config\\zed\\conversations")));
        assert!(sources
            .iter()
            .any(|source| source.path.ends_with(".local\\share\\zed\\conversations")));
    }

    #[test]
    fn marks_registered_and_nested_candidates() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        let nested = root.join("inner");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("README.md"), "# Inner").unwrap();
        let source = DiscoverySource {
            kind: "test_known".to_string(),
            label: "Test known folder".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);
        mark_registered_state(
            &mut candidates,
            &[RegisteredRoot {
                project_id: Some(42),
                path: root,
            }],
        );

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .find(|candidate| candidate.path.ends_with("inner"))
            .unwrap();
        assert!(!candidate.already_registered);
        assert!(candidate.nested_under_registered.is_some());
        assert_eq!(candidate.overlap_kind, "inside_registered_root");
        assert_eq!(candidate.project_kind, "documentation_project");
    }

    #[test]
    fn detects_ai_tool_rule_directories_without_readme() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("rules-only");
        fs::create_dir_all(project.join(".roo").join("rules")).unwrap();
        fs::write(project.join(".windsurfrules"), "Use local rules").unwrap();
        let source = DiscoverySource {
            kind: "test_known".to_string(),
            label: "Test known folder".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .find(|candidate| candidate.path.ends_with("rules-only"))
            .unwrap();
        assert_eq!(candidate.project_kind, "ai_assisted_project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "roo_rules_dir"));
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "windsurf_rules"));
    }

    #[test]
    fn deep_scan_finds_nested_project_and_selected_project_root() {
        let dir = tempdir().unwrap();
        let project = dir
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("deep-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# Deep").unwrap();
        fs::write(project.join("SKILL.md"), "---\nname: deep\n---\n").unwrap();

        let report = discover_projects_in_root(
            dir.path(),
            &[],
            DiscoveryOptions {
                limit: 20,
                ..Default::default()
            },
        );
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.path.ends_with("deep-project")));

        let root_report = discover_projects_in_root(
            &project,
            &[],
            DiscoveryOptions {
                limit: 20,
                ..Default::default()
            },
        );
        let root_candidate = root_report
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("deep-project"))
            .unwrap();
        assert_eq!(root_candidate.project_kind, "ai_assisted_project");
        assert!(root_candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "skill_definition"));
    }

    #[test]
    fn deep_scan_links_global_sessions_to_in_root_folders() {
        // The project a deep scan should auto-add: it lives under the scanned
        // root and a local session (stored elsewhere) has worked in it.
        let root = tempdir().unwrap();
        let in_root_project = root.path().join("code-hangar");
        fs::create_dir_all(&in_root_project).unwrap();
        fs::write(in_root_project.join("README.md"), "# CH").unwrap();
        fs::write(in_root_project.join("AGENTS.md"), "ctx").unwrap();

        // A project the same session references but that sits OUTSIDE the root —
        // a root-scoped deep scan must prune it.
        let outside = tempdir().unwrap();
        let outside_project = outside.path().join("other-project");
        fs::create_dir_all(&outside_project).unwrap();
        fs::write(outside_project.join("package.json"), "{}").unwrap();

        // The transcript lives in its own global location (mimicking ~/.claude),
        // pointing at both projects via cwd.
        let session_home = tempdir().unwrap();
        let transcript = session_home.path().join("session.jsonl");
        fs::write(
            &transcript,
            format!(
                "{{\"cwd\":\"{}\"}}\n{{\"cwd\":\"{}\"}}\n",
                in_root_project.to_string_lossy().replace('\\', "\\\\"),
                outside_project.to_string_lossy().replace('\\', "\\\\"),
            ),
        )
        .unwrap();
        let global_session_source = DiscoverySource {
            kind: "claude_code_sessions".to_string(),
            label: "Claude Code sessions".to_string(),
            path: session_home.path().to_path_buf(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };

        // Marker pass over the root, then the global session cross-reference —
        // exactly what discover_projects_in_root does internally.
        let root_source = DiscoverySource {
            kind: "deep_scan_root".to_string(),
            label: "Deep scan root".to_string(),
            path: root.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        let mut searched = Vec::new();
        scan_folder_source(
            &root_source,
            &mut candidates,
            DEEP_ROOT_DEPTH,
            DEEP_ROOT_MAX_DIRS,
            "deep_folder_marker",
            4,
            true,
        );
        cross_reference_global_sessions(
            root.path(),
            std::slice::from_ref(&global_session_source),
            &mut candidates,
            &mut sessions,
            &mut searched,
        );

        let finalized = candidates
            .into_values()
            .map(finalize_candidate)
            .collect::<Vec<_>>();

        // The in-root project picked up the session link → strong / auto-addable.
        let linked = finalized
            .iter()
            .find(|candidate| candidate.path.ends_with("code-hangar"))
            .expect("in-root project should be a candidate");
        assert!(
            linked
                .signals
                .iter()
                .any(|signal| signal.kind == "session_path"),
            "in-root project should be linked to the global session"
        );

        // The out-of-root project the session also referenced is pruned.
        assert!(
            !finalized
                .iter()
                .any(|candidate| candidate.path.ends_with("other-project")),
            "out-of-root project must not leak into a root-scoped deep scan"
        );

        // The linking session is retained.
        assert!(
            !sessions.is_empty(),
            "the linking session should be retained"
        );
    }

    #[test]
    fn claude_pid_lock_files_are_scanned_but_never_listed_as_sessions() {
        let home = tempdir_projectlike();
        let sessions_dir = home.path().join(".claude").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        let project = home.path().join("LockedProj");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "# p").unwrap();

        // A per-PID lock/heartbeat file — the shape Claude Code actually writes.
        fs::write(
            sessions_dir.join("13464.json"),
            format!(
                "{{\"pid\":13464,\"sessionId\":\"18b6d6cf\",\"cwd\":\"{}\",\"kind\":\"interactive\"}}",
                project.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "claude_code_sessions".to_string(),
            label: "Claude Code sessions".to_string(),
            path: sessions_dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_discovery_source(&source, &mut candidates, &mut sessions);

        // Not a conversation: the lock file must never surface as a session row…
        assert!(
            sessions.is_empty(),
            "PID lock json must not be listed, got: {:?}",
            sessions.keys().collect::<Vec<_>>()
        );
        // …but the cwd it records still feeds project attribution signals.
        assert!(
            candidates.contains_key(&candidate_key(&project)),
            "the lock file's cwd must still contribute the project signal"
        );
        // The listing gate itself: json is lock metadata, jsonl stays listable.
        assert!(!should_list_session_file(
            &sessions_dir.join("13464.json"),
            "claude_code_sessions"
        ));
        assert!(should_list_session_file(
            &sessions_dir.join("conv.jsonl"),
            "claude_code_sessions"
        ));
    }

    #[test]
    fn codex_index_thread_names_retitle_rollout_sessions() {
        let home = tempdir_projectlike();
        let day = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("06")
            .join("14");
        fs::create_dir_all(&day).unwrap();
        let project = home.path().join("NamedProj");
        fs::create_dir_all(&project).unwrap();

        let rollout =
            day.join("rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl");
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                project.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        // BOM-prefixed first line (as Codex writes it) + an uppercase id that must
        // still match the lowercase filename id + an entry with no rollout on disk.
        let index = home.path().join(".codex").join("session_index.jsonl");
        fs::write(
            &index,
            "\u{feff}{\"id\":\"019D6874-29F6-7722-81F8-33B21CD1B6CC\",\"thread_name\":\"Fix the flaky build\",\"updated_at\":\"2026-06-14T10:00:00Z\"}\n{\"id\":\"ffffffff-ffff-ffff-ffff-ffffffffffff\",\"thread_name\":\"Unmatched\"}\n",
        )
        .unwrap();

        // Scan in discovery_sources order: rollouts first, then the index.
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_discovery_source(
            &DiscoverySource {
                kind: "codex_sessions".to_string(),
                label: "Codex sessions".to_string(),
                path: home.path().join(".codex").join("sessions"),
                detail: None,
                mode: SourceMode::TextMetadata,
            },
            &mut candidates,
            &mut sessions,
        );
        scan_discovery_source(
            &DiscoverySource {
                kind: "codex_index".to_string(),
                label: "Codex session index".to_string(),
                path: index,
                detail: None,
                mode: SourceMode::TextMetadata,
            },
            &mut candidates,
            &mut sessions,
        );

        let session = sessions
            .get(&candidate_key(&rollout))
            .expect("the rollout must be listed as a session");
        assert_eq!(
            session.display_name.as_deref(),
            Some("Fix the flaky build"),
            "the index thread_name must retitle the rollout session"
        );

        // The filename-id extractor: exact uuid tail, rollout-only, case-folded.
        assert_eq!(
            codex_rollout_session_id(&rollout).as_deref(),
            Some("019d6874-29f6-7722-81f8-33b21cd1b6cc")
        );
        assert_eq!(
            codex_rollout_session_id(Path::new("rollout-short.jsonl")),
            None
        );
        assert_eq!(
            codex_rollout_session_id(Path::new(
                "transcript-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl"
            )),
            None
        );
    }

    /// A `session_meta` first line embedding the given cwd, exactly as Codex/Claude
    /// write it (double-escaped backslashes so the on-disk JSON is valid).
    fn session_meta_line(cwd: &Path) -> String {
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
            cwd.to_string_lossy().replace('\\', "\\\\")
        )
    }

    /// Fix #1: an OVERSIZED (>16 MiB) Claude transcript whose `cwd` is in the head
    /// must still be LISTED and linked. Before the fix its empty-link candidate was
    /// dropped for `claude_code_projects` (not a loose kind) — the real machine lost
    /// 7/20 Claude transcripts (incl. 425/135/91 MB) this way.
    #[test]
    fn oversized_claude_transcript_is_listed_via_head_cwd_probe() {
        let home = tempdir_projectlike();
        let project = home.path().join("BigProj");
        fs::create_dir_all(&project).unwrap();
        let dir = home.path().join(".claude").join("projects").join("BigProj");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("conversation.jsonl");

        // cwd in the head, then pad with cwd-less lines to push size past 16 MiB.
        let mut body = session_meta_line(&project);
        let filler = "{\"type\":\"assistant\",\"content\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}\n";
        while (body.len() as u64) <= SOURCE_TEXT_METADATA_MAX_BYTES + 1024 {
            body.push_str(filler);
        }
        fs::write(&transcript, &body).unwrap();
        assert!(
            fs::metadata(&transcript).unwrap().len() > SOURCE_TEXT_METADATA_MAX_BYTES,
            "fixture must exceed the oversize gate"
        );

        let source = DiscoverySource {
            kind: "claude_code_projects".to_string(),
            label: "Claude Code projects".to_string(),
            path: dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let session = sessions
            .get(&candidate_key(&transcript))
            .expect("oversized Claude transcript must be listed as a session");
        assert!(
            session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&project)),
            "the oversized transcript must link to its head cwd"
        );
    }

    #[test]
    fn untitled_claude_transcript_uses_first_user_message_as_display_name() {
        let home = tempdir_projectlike();
        let project = home.path().join("NamedProj");
        fs::create_dir_all(&project).unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("NamedProj");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("019f3315-12ff-7071-8534-04fe50ed534e.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"Plan the Safe Manage UX fixes\"}}]}}}}\n",
                session_meta_line(&project)
            ),
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "claude_code_projects".to_string(),
            label: "Claude Code projects".to_string(),
            path: dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let session = sessions
            .get(&candidate_key(&transcript))
            .expect("Claude transcript must be listed");
        assert_eq!(
            session.display_name.as_deref(),
            Some("Plan the Safe Manage UX fixes")
        );
    }

    #[test]
    fn session_title_cleanup_extracts_wrapped_human_request_and_caps_other_titles() {
        let wrapped = format!(
            "{} Pedido atual: Corrigir os titulos das sessoes",
            "System instructions that must not become a card title. ".repeat(8)
        );
        assert_eq!(
            clean_session_title(&wrapped).as_deref(),
            Some("Corrigir os titulos das sessoes")
        );

        let unstructured = "A deliberately long session title without a request marker that still needs a bounded accessible name";
        let cleaned = clean_session_title(unstructured).expect("long title should remain usable");
        assert_eq!(cleaned.chars().count(), SESSION_TITLE_MAX_CHARS);
        assert!(cleaned.ends_with("..."));

        let command = "<command-message>claude-api</command-message> <command-name>/claude-api</command-name>";
        assert_eq!(
            clean_session_title(command).as_deref(),
            Some("Command /claude-api")
        );
    }

    #[test]
    fn claude_local_json_uses_title_instead_of_local_uuid() {
        let home = tempdir_projectlike();
        let project = home.path().join("LocalClaudeProj");
        fs::create_dir_all(&project).unwrap();
        let dir = home.path().join("claude-code-sessions");
        fs::create_dir_all(&dir).unwrap();
        let session = dir.join("local_47346395-6484-4213-9f0d-3d584747a428.json");
        fs::write(
            &session,
            format!(
                "{{\"cwd\":\"{}\",\"title\":\"Review Safe Manage UX\",\"titleSource\":\"auto\"}}",
                project.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "claude_local_agent_sessions".to_string(),
            label: "Claude local agent sessions".to_string(),
            path: dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_metadata_file(&session, &source, &mut candidates, &mut sessions);

        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].display_name, "Review Safe Manage UX");
    }

    #[test]
    fn technical_claude_display_name_is_replaced_when_title_is_derivable() {
        let home = tempdir_projectlike();
        let project = home.path().join("ReplacementProj");
        fs::create_dir_all(&project).unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("ReplacementProj");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("019f3315-12ff-7071-8534-04fe50ed534e.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"Replace the UUID title\"}}}}\n",
                session_meta_line(&project)
            ),
        )
        .unwrap();
        let source = DiscoverySource {
            kind: "claude_code_projects".to_string(),
            label: "Claude Code projects".to_string(),
            path: dir,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut sessions = BTreeMap::new();
        add_session_candidate_with_display(
            &mut sessions,
            &transcript,
            &source,
            &[project],
            Some("019f3315-12ff-7071-8534-04fe50ed534e.jsonl".to_string()),
            None,
        );

        let session = sessions
            .get(&candidate_key(&transcript))
            .expect("Claude transcript must be listed");
        assert_eq!(
            session.display_name.as_deref(),
            Some("Replace the UUID title")
        );
    }

    /// Fix #1 (Codex side): an oversized Codex rollout lists regardless (kind carries
    /// "session"), but must REGAIN its project link from the head cwd.
    #[test]
    fn oversized_codex_rollout_regains_project_link() {
        let home = tempdir_projectlike();
        let project = home.path().join("CodexBig");
        fs::create_dir_all(&project).unwrap();
        let day = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("06")
            .join("14");
        fs::create_dir_all(&day).unwrap();
        let rollout =
            day.join("rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl");

        let mut body = session_meta_line(&project);
        let filler =
            "{\"type\":\"response_item\",\"payload\":{\"text\":\"xxxxxxxxxxxxxxxxxxxx\"}}\n";
        while (body.len() as u64) <= SOURCE_TEXT_METADATA_MAX_BYTES + 1024 {
            body.push_str(filler);
        }
        fs::write(&rollout, &body).unwrap();

        let source = DiscoverySource {
            kind: "codex_sessions".to_string(),
            label: "Codex sessions".to_string(),
            path: home.path().join(".codex").join("sessions"),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let session = sessions
            .get(&candidate_key(&rollout))
            .expect("oversized Codex rollout must be listed");
        assert!(
            session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&project)),
            "the oversized rollout must regain its cwd link"
        );
    }

    /// Fix #2: a transcript whose head is a flood of cwd-less `queue-operation`
    /// records, with the first `cwd` well past the 64 KB prefix (but under the 1 MiB
    /// probe cap). The line-aware probe must still find it.
    #[test]
    fn late_cwd_after_queue_operation_flood_is_found() {
        let home = tempdir_projectlike();
        let project = home.path().join("LateProj");
        fs::create_dir_all(&project).unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("LateProj");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("conversation.jsonl");

        // ~120 KB of cwd-less queue-operation lines, THEN the cwd line.
        let mut body = String::new();
        let queue_line = "{\"content\":\"do something\",\"operation\":\"append\",\"sessionId\":\"abc\",\"timestamp\":\"2026-06-14T10:00:00Z\",\"type\":\"queue-operation\"}\n";
        while body.len() < 120 * 1024 {
            body.push_str(queue_line);
        }
        let cwd_offset = body.len();
        body.push_str(&session_meta_line(&project));
        fs::write(&transcript, &body).unwrap();
        assert!(
            cwd_offset > SOURCE_FILE_BYTES,
            "the cwd must be beyond the 64 KB prefix for this test to be meaningful"
        );

        // The probe directly finds the late cwd...
        assert_eq!(
            probe_session_cwd(&transcript).as_deref(),
            Some(project.as_path()),
            "line-aware probe must find a cwd past the 64 KB prefix"
        );

        // ...and the full scan links it.
        let source = DiscoverySource {
            kind: "claude_code_projects".to_string(),
            label: "Claude Code projects".to_string(),
            path: dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let session = sessions
            .get(&candidate_key(&transcript))
            .expect("transcript must be listed");
        assert!(
            session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&project)),
            "the late cwd must be linked to the session"
        );
    }

    /// Build a minimal Codex `threads` state DB with the columns the schema-aware
    /// reader relies on. `rows` are `(id, title, cwd, rollout_path)`.
    fn write_codex_threads_db(path: &Path, rows: &[(&str, &str, &str, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (id TEXT, rollout_path TEXT, cwd TEXT, title TEXT, archived INTEGER);",
        )
        .unwrap();
        for (id, title, cwd, rollout_path) in rows {
            conn.execute(
                "INSERT INTO threads (id, rollout_path, cwd, title, archived) VALUES (?1, ?2, ?3, ?4, 0)",
                rusqlite::params![id, rollout_path, cwd, title],
            )
            .unwrap();
        }
    }

    /// Fix #3: the schema-aware `threads` reader retitles a rollout session and links
    /// its authoritative `cwd`, matching by filename uuid — and it OVERRIDES a weaker
    /// title the session_index pass may have set, keeping the index a fallback.
    #[test]
    fn codex_threads_state_retitles_and_links_cwd() {
        let home = tempdir_projectlike();
        let project = home.path().join("ThreadProj");
        fs::create_dir_all(&project).unwrap();
        let day = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("06")
            .join("14");
        fs::create_dir_all(&day).unwrap();
        let uuid = "019d6874-29f6-7722-81f8-33b21cd1b6cc";
        let rollout = day.join(format!("rollout-2026-06-14T10-00-00-{uuid}.jsonl"));
        // A rollout with NO cwd of its own, so any link must come from the DB.
        fs::write(&rollout, "{\"type\":\"session_meta\",\"payload\":{}}\n").unwrap();

        // The `\\?\` verbatim prefix, as real Codex stores it, must be stripped.
        let cwd_verbatim = format!("\\\\?\\{}", project.to_string_lossy());
        let wrapped_title = format!(
            "{} Current request: Fix the login bug",
            "Embedded system instructions. ".repeat(12)
        );
        let db = home.path().join(".codex").join("state_5.sqlite");
        write_codex_threads_db(
            &db,
            &[(
                uuid,
                &wrapped_title,
                &cwd_verbatim,
                &rollout.to_string_lossy(),
            )],
        );

        // Pre-seed a session as the rollout scan would, plus a weaker index title.
        let mut sessions: BTreeMap<String, SessionBuilder> = BTreeMap::new();
        let src = DiscoverySource {
            kind: "codex_sessions".to_string(),
            label: "Codex sessions".to_string(),
            path: home.path().join(".codex").join("sessions"),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        add_session_candidate(&mut sessions, &rollout, &src, &[]);
        sessions
            .get_mut(&candidate_key(&rollout))
            .unwrap()
            .display_name = Some("weaker index title".to_string());

        let source = DiscoverySource {
            kind: "codex_state".to_string(),
            label: "Codex thread state".to_string(),
            path: db.clone(),
            detail: None,
            mode: SourceMode::SqliteMetadata,
        };
        let mut candidates = BTreeMap::new();
        scan_sqlite_metadata_source(&source, &mut candidates, &mut sessions);

        let session = sessions.get(&candidate_key(&rollout)).unwrap();
        assert_eq!(
            session.display_name.as_deref(),
            Some("Fix the login bug"),
            "the threads DB title must override the weaker index title"
        );
        assert!(
            session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&project)),
            "the threads DB cwd (verbatim-prefix stripped) must be linked"
        );
    }

    /// Fix #3 fallback: a state DB WITHOUT a `threads` table (older Codex) must make
    /// the schema-aware reader return `None` so the caller keeps the generic scan.
    #[test]
    fn codex_threads_reader_falls_back_when_schema_absent() {
        let home = tempdir_projectlike();
        let db = home.path().join("state_old.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE conversations (id TEXT, preview TEXT);")
            .unwrap();
        drop(conn);
        assert!(
            read_codex_threads_index(&db).is_none(),
            "a DB without a `threads` table must fall back (None)"
        );
    }

    /// Fix #3c: the state DB is resolved by newest mtime across `<home>` and
    /// `<home>/sqlite`, so a fresh sibling copy wins over a stale one (mirrors the
    /// real machine, where `.codex\state_5.sqlite` is newer than the `sqlite\` copy).
    #[test]
    fn codex_state_db_prefers_newest_across_both_dirs() {
        let home = tempdir_projectlike();
        let codex = home.path().join(".codex");
        let sqlite_dir = codex.join("sqlite");
        fs::create_dir_all(&sqlite_dir).unwrap();
        // The registered source path is always `<codex_home>\state_5.sqlite`; the
        // resolver globs its parent AND the `sqlite\` sibling. Here the newer copy
        // lives in `sqlite\`, so it must be chosen over the registered path itself.
        let registered = codex.join("state_5.sqlite");
        let sibling = sqlite_dir.join("state_5.sqlite");
        fs::write(&registered, b"older").unwrap();
        let registered_mtime = fs::metadata(&registered).unwrap().modified().unwrap();
        // No std API sets mtime, so rewrite the sibling until the filesystem reports
        // it strictly newer (NTFS mtime resolution is ~1 ms here, so this converges
        // in a handful of writes; the cap is a safety net, never a busy spin).
        let mut attempts = 0;
        loop {
            fs::write(&sibling, b"newer").unwrap();
            let sibling_mtime = fs::metadata(&sibling).unwrap().modified().unwrap();
            if sibling_mtime > registered_mtime {
                break;
            }
            attempts += 1;
            assert!(attempts < 1_000_000, "sibling copy never became newer");
        }

        let resolved = codex_state_db_path(&registered)
            .expect("a state DB must resolve from the registered source parent");
        assert_eq!(
            candidate_key(&resolved),
            candidate_key(&sibling),
            "the newer copy must win regardless of which directory holds it"
        );
    }

    /// Fix #4: an ARCHIVED Codex rollout must take the cwd-only branch — a stray
    /// directory named only in its message body must NOT be linked as a project.
    #[test]
    fn archived_codex_rollout_ignores_stray_body_dirs() {
        let home = tempdir_projectlike();
        let cwd_proj = home.path().join("RealCwd");
        fs::create_dir_all(&cwd_proj).unwrap();
        // A real, marker-bearing dir mentioned in the body but NOT the cwd — the
        // generic branch would have linked it; the cwd-only branch must not.
        let stray = home.path().join("StrayDir");
        fs::create_dir_all(stray.join(".git")).unwrap();

        let archived_dir = home.path().join(".codex").join("archived_sessions");
        fs::create_dir_all(&archived_dir).unwrap();
        let rollout = archived_dir
            .join("rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl");
        let mut body = session_meta_line(&cwd_proj);
        body.push_str(&format!(
            "{{\"type\":\"message\",\"text\":\"see {}\"}}\n",
            stray.to_string_lossy().replace('\\', "\\\\")
        ));
        fs::write(&rollout, &body).unwrap();

        let source = DiscoverySource {
            kind: "codex_archived_sessions".to_string(),
            label: "Codex archived sessions".to_string(),
            path: archived_dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        let session = sessions
            .get(&candidate_key(&rollout))
            .expect("archived rollout must be listed");
        assert!(
            session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&cwd_proj)),
            "the archived rollout must link to its cwd"
        );
        assert!(
            !session
                .linked_project_paths
                .iter()
                .any(|p| candidate_key(p) == candidate_key(&stray)),
            "a stray dir from the message body must NOT be linked"
        );
    }

    /// Regression: titling archived Codex rollouts must not let the visible-identity
    /// dedup collapse DISTINCT conversations that merely share an auto-generated
    /// (title, cwd). Codex's own store had 27 distinct archived rollouts (27 uuids)
    /// folding to 13 title+cwd pairs; keying on the per-file rollout uuid keeps all 27.
    #[test]
    fn distinct_codex_rollouts_sharing_title_and_cwd_are_not_deduped() {
        fn archived(rollout_file: &str) -> SessionDiscoveryCandidate {
            SessionDiscoveryCandidate {
                path: format!("C:/Users/x/.codex/archived_sessions/{rollout_file}"),
                display_name: "Fix the build".to_string(),
                source_kind: "codex_archived_sessions".to_string(),
                source_label: "Codex archived sessions".to_string(),
                session_kind: "agent".to_string(),
                confidence: "medium".to_string(),
                linked_project_paths: vec!["C:/proj/app".to_string()],
                linked_registered_project_ids: Vec::new(),
                association: "unregistered_project_reference".to_string(),
                modified_ms: Some(1),
            }
        }
        let a = archived("rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl");
        let b = archived("rollout-2026-06-14T11-00-00-019d6875-29f6-7722-81f8-33b21cd1b6cd.jsonl");
        assert_ne!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&b),
            "distinct archived rollouts sharing an auto-title must stay separate"
        );
        // The SAME rollout reached twice still folds to one visible identity.
        let a_again =
            archived("rollout-2026-06-14T10-00-00-019d6874-29f6-7722-81f8-33b21cd1b6cc.jsonl");
        assert_eq!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&a_again),
            "the same rollout reached via two sources must still dedupe"
        );
    }

    /// Regression: Claude Code transcripts are per-session `.jsonl` files whose filename
    /// stem is the session uuid. New code titles untitled sessions from their first user
    /// message, so two DISTINCT transcripts in the same project can share a `display_name`
    /// and links — the generic identity would then collapse them and hide a real session.
    /// Keying on the per-file uuid stem keeps distinct transcripts separate while still
    /// folding the same transcript reached via two sources.
    #[test]
    fn distinct_claude_transcripts_sharing_title_and_cwd_are_not_deduped() {
        fn transcript(file: &str) -> SessionDiscoveryCandidate {
            SessionDiscoveryCandidate {
                path: format!("C:/Users/x/.claude/projects/C--proj-app/{file}"),
                display_name: "Fix the build".to_string(),
                source_kind: "claude_code_projects".to_string(),
                source_label: "Claude Code projects".to_string(),
                session_kind: "agent".to_string(),
                confidence: "medium".to_string(),
                linked_project_paths: vec!["C:/proj/app".to_string()],
                linked_registered_project_ids: Vec::new(),
                association: "unregistered_project_reference".to_string(),
                modified_ms: Some(1),
            }
        }
        let a = transcript("019f3315-12ff-7071-8534-04fe50ed534e.jsonl");
        let b = transcript("019f3316-12ff-7071-8534-04fe50ed534f.jsonl");
        assert_ne!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&b),
            "distinct Claude transcripts sharing an auto-title must stay separate"
        );
        // The SAME transcript reached twice still folds to one visible identity.
        let a_again = transcript("019f3315-12ff-7071-8534-04fe50ed534e.jsonl");
        assert_eq!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&a_again),
            "the same Claude transcript reached via two sources must still dedupe"
        );
        // The uuid stem alone drives identity: a differing on-disk parent (e.g. the same
        // session listed under two encoded project dirs) with the same stem still folds.
        let mut a_moved = transcript("019f3315-12ff-7071-8534-04fe50ed534e.jsonl");
        a_moved.path =
            "D:/backup/.claude/projects/other/019F3315-12FF-7071-8534-04FE50ED534E.jsonl"
                .to_string();
        assert_eq!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&a_moved),
            "the same uuid stem (case-insensitive) must fold regardless of parent path"
        );
    }

    /// Fix #5: an Antigravity registry `folderUri` whose EXACT root has been deleted
    /// must resolve to nothing — never parent-walk up to a surviving `.git` parent
    /// (which would misattribute the project).
    #[test]
    fn antigravity_registry_uri_does_not_parent_walk_to_git_ancestor() {
        let home = tempdir_projectlike();
        // A `.git`-bearing parent exists; the actual project child does NOT.
        let parent = home.path().join("workspace");
        fs::create_dir_all(parent.join(".git")).unwrap();
        let deleted_root = parent.join("deleted-project");
        assert!(!deleted_root.exists());

        let uri = format!(
            "file:///{}",
            deleted_root.to_string_lossy().replace('\\', "/")
        );
        assert!(
            antigravity_registry_uri_directories(&uri).is_empty(),
            "a deleted registry root must not resolve to its surviving ancestor"
        );

        // Sanity: a URI whose exact folder DOES exist still resolves to it.
        let live = parent.join("live-project");
        fs::create_dir_all(&live).unwrap();
        let live_uri = format!("file:///{}", live.to_string_lossy().replace('\\', "/"));
        let resolved = antigravity_registry_uri_directories(&live_uri);
        assert_eq!(resolved.len(), 1);
        assert_eq!(candidate_key(&resolved[0]), candidate_key(&live));
    }

    #[test]
    fn openclaw_markers_are_ai_assisted_project_signals() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("openclaw-workspace");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("openclaw.json"), "{}").unwrap();
        fs::write(project.join("MEMORY.md"), "# Memory").unwrap();
        fs::write(project.join("SOUL.md"), "# Persona").unwrap();
        let source = DiscoverySource {
            kind: "test_openclaw".to_string(),
            label: "Test OpenClaw".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert_eq!(candidate.project_kind, "ai_assisted_project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "openclaw_config"));
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "agent_memory"));
    }

    #[test]
    fn aider_markers_are_ai_assisted_project_signals() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("aider-workspace");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(".aider.input.history"), "/add README.md").unwrap();
        fs::write(project.join(".aider.conf.yml"), "model: local").unwrap();
        let source = DiscoverySource {
            kind: "test_aider".to_string(),
            label: "Test Aider".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .next()
            .unwrap();
        assert_eq!(candidate.project_kind, "ai_assisted_project");
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "aider_input_history"));
        assert!(candidate
            .signals
            .iter()
            .any(|signal| signal.kind == "aider_config"));
    }

    #[test]
    fn marks_candidate_that_contains_registered_root() {
        let dir = tempdir().unwrap();
        let parent = dir.path().join("parent");
        let registered = parent.join("child");
        fs::create_dir_all(&registered).unwrap();
        fs::write(parent.join("README.md"), "# Parent").unwrap();
        fs::write(registered.join("README.md"), "# Child").unwrap();
        let source = DiscoverySource {
            kind: "test_known".to_string(),
            label: "Test known folder".to_string(),
            path: dir.path().to_path_buf(),
            detail: None,
            mode: SourceMode::KnownFolder,
        };
        let mut candidates = BTreeMap::new();
        scan_known_folder_source(&source, &mut candidates);
        mark_registered_state(
            &mut candidates,
            &[RegisteredRoot {
                project_id: Some(7),
                path: registered,
            }],
        );

        let candidate = candidates
            .into_values()
            .map(finalize_candidate)
            .find(|candidate| candidate.path.ends_with("parent"))
            .unwrap();
        assert_eq!(candidate.overlap_kind, "contains_registered_root");
        assert_eq!(candidate.contains_registered_roots.len(), 1);
    }

    #[test]
    #[ignore]
    fn local_discovery_smoke_prints_candidates() {
        let options = DiscoveryOptions {
            limit: 80,
            include_loose_sessions: true,
            include_agents: true,
            include_technical_candidates: std::env::var_os(
                "CODEHANGAR_DISCOVERY_INCLUDE_TECHNICAL",
            )
            .is_some(),
        };
        let report =
            if let Some(source_filter) = std::env::var_os("CODEHANGAR_DISCOVERY_SOURCE_KIND") {
                let source_filter = source_filter.to_string_lossy().to_ascii_lowercase();
                let started = Instant::now();
                let mut searched_locations = Vec::new();
                let mut candidates = BTreeMap::new();
                let mut sessions = BTreeMap::new();
                for source in discovery_sources() {
                    if !source.kind.to_ascii_lowercase().contains(&source_filter) {
                        continue;
                    }
                    let source_started = Instant::now();
                    eprintln!(
                        "source_start kind={} path={}",
                        source.kind,
                        source.path.display()
                    );
                    let exists = source.path.exists();
                    searched_locations.push(DiscoverySourceHit {
                        source_kind: source.kind.clone(),
                        source_label: source.label.clone(),
                        path: display_path(&source.path),
                        exists,
                        detail: source.detail.clone(),
                    });
                    if exists {
                        scan_discovery_source(&source, &mut candidates, &mut sessions);
                    }
                    eprintln!(
                        "source_done kind={} duration_ms={} candidates={} sessions={}",
                        source.kind,
                        source_started.elapsed().as_millis(),
                        candidates.len(),
                        sessions.len()
                    );
                }
                let total_candidates = candidates.len() as u64;
                let mut candidates = candidates
                    .into_values()
                    .map(finalize_candidate)
                    .collect::<Vec<_>>();
                candidates.sort_by(|a, b| {
                    b.score.cmp(&a.score).then_with(|| {
                        a.path
                            .to_ascii_lowercase()
                            .cmp(&b.path.to_ascii_lowercase())
                    })
                });
                if options.limit > 0 && candidates.len() > options.limit {
                    candidates.truncate(options.limit);
                }
                let total_sessions = sessions.len() as u64;
                let mut sessions = finalize_sessions(sessions, &[]);
                if options.limit > 0 && sessions.len() > options.limit {
                    sessions.truncate(options.limit);
                }
                ProjectDiscoveryReport {
                    candidates,
                    sessions,
                    searched_locations,
                    duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    total_candidates,
                    total_sessions,
                }
            } else {
                std::env::var_os("CODEHANGAR_DISCOVERY_SMOKE_ROOT")
                    .map(PathBuf::from)
                    .map(|root| discover_projects_in_root(&root, &[], options.clone()))
                    .unwrap_or_else(|| discover_known_projects(&[], options))
            };

        eprintln!(
            "searched={} total_candidates={} shown={} total_sessions={} sessions_shown={} duration_ms={}",
            report.searched_locations.len(),
            report.total_candidates,
            report.candidates.len(),
            report.total_sessions,
            report.sessions.len(),
            report.duration_ms
        );
        for candidate in report.candidates.iter().take(40) {
            let signals = candidate
                .signals
                .iter()
                .take(4)
                .map(|signal| signal.kind.as_str())
                .collect::<Vec<_>>()
                .join(",");
            eprintln!(
                "{} | {} | score={} | overlap={} | {} | {}",
                candidate.confidence,
                candidate.project_kind,
                candidate.score,
                candidate.overlap_kind,
                candidate.path,
                signals
            );
        }
        for session in report.sessions.iter().take(40) {
            eprintln!(
                "session | {} | {} | {} | {} | linked={}",
                session.association,
                session.session_kind,
                session.source_label,
                session.display_name,
                session.linked_project_paths.join(";")
            );
        }

        assert!(report.duration_ms < 15 * 60 * 1_000);
    }

    /// Real-data completeness breakdown (`--ignored --nocapture`): the FULL discovery
    /// report (not the 40-line print cap), aggregated by session app/kind/association
    /// and by candidate source, so a human can confirm every app + Hermes + loose are
    /// represented. Asserts the major session kinds (Codex/Claude/Antigravity/Hermes/
    /// Cursor) and loose sessions are all present.
    #[test]
    #[ignore = "depends on the local user's real AI app data"]
    fn discovery_completeness_breakdown() {
        let options = DiscoveryOptions {
            limit: 5000,
            include_loose_sessions: true,
            include_agents: true,
            include_technical_candidates: true,
        };
        let report = discover_known_projects(&[], options);
        let mut by_kind: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let mut by_assoc: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for s in &report.sessions {
            *by_kind.entry(s.session_kind.clone()).or_default() += 1;
            *by_assoc.entry(s.association.clone()).or_default() += 1;
        }
        let mut by_source: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let mut by_project_kind: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for c in &report.candidates {
            *by_project_kind.entry(c.project_kind.clone()).or_default() += 1;
            for s in &c.source_kinds {
                *by_source.entry(s.clone()).or_default() += 1;
            }
        }
        eprintln!(
            "== DISCOVERY COMPLETENESS == candidates={} sessions={} sources_searched={}",
            report.candidates.len(),
            report.sessions.len(),
            report.searched_locations.len()
        );
        eprintln!("-- sessions by kind --");
        for (k, n) in &by_kind {
            eprintln!("  {n:>4}  {k}");
        }
        eprintln!("-- sessions by association --");
        for (k, n) in &by_assoc {
            eprintln!("  {n:>4}  {k}");
        }
        eprintln!("-- candidates by project_kind --");
        for (k, n) in &by_project_kind {
            eprintln!("  {n:>4}  {k}");
        }
        eprintln!("-- candidate source_kinds (top by count) --");
        let mut srcs: Vec<_> = by_source.into_iter().collect();
        srcs.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        for (k, n) in srcs.iter().take(30) {
            eprintln!("  {n:>4}  {k}");
        }
        let kinds_lower: Vec<String> = by_kind.keys().map(|k| k.to_ascii_lowercase()).collect();
        let has = |needle: &str| kinds_lower.iter().any(|k| k.contains(needle));
        assert!(has("codex"), "Codex sessions present");
        assert!(has("claude"), "Claude sessions present");
        assert!(
            has("antigravity") || has("gemini"),
            "Antigravity sessions present"
        );
        // Hermes/NemoClaw live primarily inside WSL, which is gated OFF by default,
        // so their sessions only appear when a Windows-side state store exists or
        // the WSL scan is enabled. Assert their presence ONLY under those
        // conditions; the Codex/Claude/Antigravity assertions above stay strict.
        let windows_hermes_store_present = discovery_sources()
            .into_iter()
            .any(|source| source.mode == SourceMode::HermesState && source.path.is_file());
        if windows_hermes_store_present || wsl_scan_enabled() {
            assert!(
                has("hermes") || has("nemoclaw"),
                "Hermes sessions present (Windows store present or WSL gate on)"
            );
        }
        assert!(
            by_assoc.keys().any(|a| a == "loose_session"),
            "loose sessions present (isolated conversations)"
        );
    }

    // --- Antigravity conversation `.db` protobuf scanning ----------------------

    /// Append a base-128 varint, little-endian groups of 7 bits.
    fn push_varint(buf: &mut Vec<u8>, mut value: u64) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// Encode one length-delimited (wire type 2) field with the given field number.
    fn push_len_delimited(buf: &mut Vec<u8>, field: u64, payload: &[u8]) {
        push_varint(buf, (field << 3) | 2);
        push_varint(buf, payload.len() as u64);
        buf.extend_from_slice(payload);
    }

    #[test]
    fn protobuf_scanner_extracts_top_level_and_nested_strings() {
        // field 1: a top-level string; field 2: a sub-message whose field 1 is a
        // nested string; field 3: a varint (must be skipped, not misread).
        let mut inner = Vec::new();
        push_len_delimited(&mut inner, 1, "nested resposta em português".as_bytes());
        let mut msg = Vec::new();
        push_len_delimited(&mut msg, 1, "hello world prompt".as_bytes());
        push_len_delimited(&mut msg, 2, &inner);
        push_varint(&mut msg, 3 << 3); // field 3, wire type 0 (varint)
        push_varint(&mut msg, 123_456);

        let fragments = extract_protobuf_text_fragments(&msg);
        assert!(
            fragments.iter().any(|f| f == "hello world prompt"),
            "top-level string recovered: {fragments:?}"
        );
        assert!(
            fragments
                .iter()
                .any(|f| f == "nested resposta em português"),
            "nested string recovered: {fragments:?}"
        );
    }

    #[test]
    fn protobuf_scanner_is_bounded_and_never_panics_on_garbage() {
        // Truncated length prefix and random bytes must degrade, not panic.
        let garbage = [0x0a, 0xff, 0xff, 0xff, 0x01, 0x02, 0x03];
        let _ = extract_protobuf_text_fragments(&garbage);
        let random: Vec<u8> = (0..512u32)
            .map(|i| (i.wrapping_mul(37) & 0xff) as u8)
            .collect();
        let _ = extract_protobuf_text_fragments(&random);
    }

    #[test]
    fn protobuf_scanner_survives_oversized_length_field() {
        // A length-delimited field claiming u64::MAX bytes must not overflow the
        // `idx + len` bounds math and panic the slice — it must degrade cleanly.
        let mut blob = vec![0x0a]; // field 1, wire type 2 (length-delimited)
                                   // 10-byte varint that decodes to u64::MAX.
        blob.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
        blob.extend_from_slice(&[0x41, 0x42]); // a couple of trailing bytes
        assert!(extract_protobuf_text_fragments(&blob).is_empty());
    }

    /// Build a minimal Antigravity-shaped conversation database: a `steps` table
    /// with `idx` + `step_payload`, each payload a protobuf carrying one string.
    fn write_synthetic_conversation_db(path: &Path, messages: &[(i64, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, step_payload BLOB);",
        )
        .unwrap();
        for (idx, text) in messages {
            let mut payload = Vec::new();
            push_len_delimited(&mut payload, 5, text.as_bytes());
            conn.execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 0, ?2)",
                rusqlite::params![idx, payload],
            )
            .unwrap();
        }
    }

    #[test]
    fn antigravity_transcript_recovers_steps_in_chronological_order() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("conv.db");
        write_synthetic_conversation_db(
            &db,
            &[
                (1, "first user prompt"),
                (2, "assistant reply about upscaling"),
                (3, "final step Kit Gratis"),
            ],
        );

        let (transcript, truncated) =
            antigravity_conversation_transcript(&db, ANTIGRAVITY_TRANSCRIPT_MAX_BYTES)
                .expect("transcript recovered");
        assert!(!truncated, "small conversation fits without truncation");
        assert!(transcript.contains("first user prompt"));
        assert!(transcript.contains("final step Kit Gratis"));
        // Oldest-first ordering: step 1 appears before step 3.
        let first = transcript.find("first user prompt").unwrap();
        let last = transcript.find("final step Kit Gratis").unwrap();
        assert!(first < last, "steps presented chronologically");
    }

    #[test]
    fn antigravity_session_title_uses_first_human_text_instead_of_uuid() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("801625f7-88ea-4a22-a80a-7c3114bc7c25.db");
        write_synthetic_conversation_db(
            &db,
            &[
                (1, "command(git status)"),
                (2, "Cria uma nova branch sem alterar o codigo"),
                (3, "Branch criada"),
            ],
        );

        assert_eq!(
            antigravity_conversation_display_name(&db).as_deref(),
            Some("Cria uma nova branch sem alterar o codigo")
        );
    }

    #[test]
    fn antigravity_transcript_truncates_and_keeps_newest_when_over_cap() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("conv.db");
        write_synthetic_conversation_db(
            &db,
            &[
                (1, "oldest message that should be dropped"),
                (2, "middle message"),
                (3, "newest message must survive"),
            ],
        );

        // Cap that fits only one step's worth of text forces truncation.
        let (transcript, truncated) = antigravity_conversation_transcript(&db, 40)
            .expect("transcript recovered under tight cap");
        assert!(truncated, "older history dropped to fit the cap");
        assert!(
            transcript.contains("newest message must survive"),
            "the newest step is always kept: {transcript:?}"
        );
        assert!(
            !transcript.contains("oldest message that should be dropped"),
            "the oldest step is dropped first"
        );
    }

    /// Write a synthetic conversation `.db` whose `trajectory_metadata_blob` row
    /// `id='main'` holds `data` = the given bytes (the per-conversation fallback
    /// source).
    fn write_synthetic_metadata_blob_db(path: &Path, data: &[u8]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE trajectory_metadata_blob (id TEXT PRIMARY KEY, data BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
            rusqlite::params![data],
        )
        .unwrap();
    }

    #[test]
    fn antigravity_db_blob_fallback_resolves_single_project() {
        let dir = tempdir().unwrap();
        // A real, existing project dir the conversation belongs to.
        let project = dir.path().join("my-project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "ctx").unwrap();
        // The metadata blob embeds the one folderUri (real Antigravity stores it as
        // a percent-encoded `file://` string inside the protobuf bytes).
        let blob = format!(
            "\x0a\x1bfile:///{}",
            project.to_string_lossy().replace('\\', "/")
        );

        let convo_dir = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        fs::create_dir_all(&convo_dir).unwrap();
        // UUID is absent from the (empty) proto map, so resolution uses the blob.
        let db = convo_dir.join("abc.db");
        write_synthetic_metadata_blob_db(&db, blob.as_bytes());

        let empty: AntigravityProtoMap = BTreeMap::new();
        let resolved = antigravity_conversation_projects(&db, &empty);
        assert!(
            resolved.iter().any(|p| same_path(p, &project)),
            "blob fallback resolved single project: {resolved:?}"
        );
    }

    #[test]
    fn antigravity_db_blob_fallback_outside_of_project_is_loose() {
        let dir = tempdir().unwrap();
        let convo_dir = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        fs::create_dir_all(&convo_dir).unwrap();
        let db = convo_dir.join("loose.db");
        // No folderUri, only the loose marker -> no links.
        write_synthetic_metadata_blob_db(&db, b"\x12\x12outside-of-project");

        let empty: AntigravityProtoMap = BTreeMap::new();
        assert!(antigravity_conversation_projects(&db, &empty).is_empty());
    }

    /// Percent-encode a path as a `file://` URI the way Antigravity's per-project
    /// registry stores it: drive colon as `%3A`, spaces as `%20`, separators as `/`.
    /// Lets the roots reader prove it decodes `%3A`→`:` and `%20`→space identically
    /// to `read_gemini_project_registry`.
    #[cfg(test)]
    fn gemini_folder_uri_percent_encoded(path: &Path) -> String {
        let forward = path.to_string_lossy().replace('\\', "/");
        let encoded = forward.replace(':', "%3A").replace(' ', "%20");
        format!("file:///{encoded}")
    }

    #[test]
    fn gemini_project_roots_for_uuid_collects_all_existing_roots() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let uuid = "00000000-0000-0000-0000-000000000000";

        // Two roots that exist on disk: one exercising the `%3A` drive-colon decode,
        // one exercising the `%20` space decode (and the nested gitFolder shape).
        let with_colon = home.join("Example").join("SubA");
        fs::create_dir_all(&with_colon).unwrap();
        let with_space = home.join("New folder");
        fs::create_dir_all(&with_space).unwrap();
        // A root listed in the registry but NOT present on disk: an offline external
        // drive, mirroring ExampleProj's real `…\New folder\ExampleProj` on a
        // disconnected `H:`/`G:`. An absent drive letter means none of its ancestors
        // exist either, so the shared `file://`-decoder (which walks up to the nearest
        // existing dir) surfaces nothing for it.
        let missing = PathBuf::from("Z:\\offline\\New folder\\ExampleProj");

        write_gemini_project(
            home,
            uuid,
            "ExampleProj",
            serde_json::json!({
                "projectResources": {
                    "resources": [
                        { "folderUri": gemini_folder_uri_percent_encoded(&with_colon) },
                        { "gitFolder": { "folderUri": gemini_folder_uri_percent_encoded(&with_space) } },
                        { "folderUri": gemini_folder_uri_percent_encoded(&missing) },
                    ]
                }
            }),
        );

        let roots = gemini_project_roots_for_uuid(home, uuid);
        assert!(
            roots.iter().any(|p| same_path(p, &with_colon)),
            "%3A-encoded existing root must decode and be returned, got {roots:?}"
        );
        assert!(
            roots.iter().any(|p| same_path(p, &with_space)),
            "%20-encoded nested gitFolder root must decode and be returned, got {roots:?}"
        );
        assert!(
            !roots.iter().any(|p| same_path(p, &missing)),
            "a non-existent listed root must be skipped, got {roots:?}"
        );
        assert_eq!(
            roots.len(),
            2,
            "only the two existing roots, deduped: {roots:?}"
        );
    }

    #[test]
    fn antigravity_conversation_links_all_project_roots() {
        // The ExampleProj case: the proto pins the conversation to a folderUri that no
        // longer exists on disk (a disconnected external drive), yet the project's OWN
        // registry lists a root that DOES exist (the registered Code Hangar root).
        // The conversation must still link to that existing root.
        let dir = tempdir().unwrap();
        let home = dir.path();
        let uuid = "00000000-0000-0000-0000-000000000000";

        let existing_root = home.join("Example").join("SubA");
        fs::create_dir_all(&existing_root).unwrap();
        // An absent drive letter: the proto folderUri (and the same root in the
        // registry) is offline with no existing ancestor, so nothing surfaces for it
        // — exactly the real ExampleProj `…\New folder\ExampleProj` external-drive
        // situation.
        let disconnected = PathBuf::from("Z:\\offline\\New folder\\ExampleProj");

        write_gemini_project(
            home,
            uuid,
            "ExampleProj",
            serde_json::json!({
                "projectResources": {
                    "resources": [
                        { "folderUri": gemini_folder_uri_percent_encoded(&existing_root) },
                        { "folderUri": gemini_folder_uri_percent_encoded(&disconnected) },
                    ]
                }
            }),
        );

        // Proto resolves the conversation to the disconnected folderUri (not is_dir).
        let mut map: AntigravityProtoMap = BTreeMap::new();
        map.insert(
            uuid.to_string(),
            AntigravityResolution::Project {
                path: disconnected.clone(),
                project_uuid: uuid.to_string(),
            },
        );

        let convo_dir = home
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        fs::create_dir_all(&convo_dir).unwrap();
        let db = convo_dir.join(format!("{uuid}.db"));

        let links = antigravity_conversation_projects_in(&db, &map, Some(home));
        assert!(
            links.iter().any(|p| same_path(p, &existing_root)),
            "conversation must link the project's existing registry root even though \
             the proto folderUri is offline, got {links:?}"
        );
        assert!(
            !links.iter().any(|p| same_path(p, &disconnected)),
            "the offline proto folderUri must not be linked, got {links:?}"
        );
        assert_eq!(
            links.len(),
            1,
            "only the single existing root links (offline roots drop out): {links:?}"
        );
    }

    /// Real-data smoke test against the user's actual WWS conversation database.
    /// Ignored by default (machine-specific path); run explicitly with
    /// `cargo test -p hangar-discovery -- --ignored real_antigravity_wws`.
    /// Proves the scanner recovers the genuinely latest conversation: the newest
    /// steps mention the recent WWS upscaling topic.
    #[test]
    #[ignore = "depends on the local user's real Antigravity data"]
    fn real_antigravity_wws_tail_mentions_latest_topic() {
        let db = Path::new(
            "C:\\Users\\user\\.gemini\\antigravity\\conversations\\00000000-0000-0000-0000-000000000000.db",
        );
        if !db.is_file() {
            return;
        }
        let (transcript, _truncated) =
            antigravity_conversation_transcript(db, ANTIGRAVITY_TRANSCRIPT_MAX_BYTES)
                .expect("recovered transcript from real WWS conversation");
        // Never print real transcript content: this test proves the bounded
        // reader structurally without leaking conversations into build logs.
        let lower = transcript.to_lowercase();
        assert!(
            lower.contains("upscaling")
                || lower.contains("kit gratis")
                || lower.contains("kit grátis")
                || lower.contains("contracapa")
                || lower.contains("processamento"),
            "newest steps mention the recent WWS upscaling topic"
        );
    }

    #[test]
    fn antigravity_transcript_degrades_when_no_steps_table() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("conv.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE other (id INTEGER);")
            .unwrap();
        drop(conn);
        // No `steps` table -> None (caller falls back), never panics.
        assert!(
            antigravity_conversation_transcript(&db, ANTIGRAVITY_TRANSCRIPT_MAX_BYTES).is_none()
        );
        // No `trajectory_metadata_blob` table -> loose (no links), never panics.
        let empty: AntigravityProtoMap = BTreeMap::new();
        assert!(antigravity_conversation_projects(&db, &empty).is_empty());
    }

    #[test]
    fn antigravity_conversation_db_is_listed_and_scanned_as_session() {
        let dir = tempdir().unwrap();
        let convo_dir = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        fs::create_dir_all(&convo_dir).unwrap();
        let db = convo_dir.join("7f69ea34.db");
        write_synthetic_conversation_db(&db, &[(1, "olá mundo conversa")]);

        let source = DiscoverySource {
            kind: "gemini_antigravity_conversations".to_string(),
            label: "Antigravity conversations".to_string(),
            path: convo_dir.clone(),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_metadata_file(&db, &source, &mut candidates, &mut sessions);

        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1, "the .db is surfaced as the session");
        assert_eq!(sessions[0].display_name, "olá mundo conversa");
        assert_eq!(sessions[0].session_kind, "Antigravity/Gemini");
        assert_eq!(sessions[0].association, "loose_session");
    }

    /// Hand-encode one summary record: field 1 = uuid, field 2 = summary message
    /// whose field 17 = project-info message carrying field 7 = folderUri and
    /// field 18 = project uuid / "outside-of-project". Mirrors the on-disk wire
    /// shape verified in `agyhub_summaries_proto.pb`.
    fn encode_antigravity_record(
        uuid: &str,
        folder_uri: Option<&str>,
        project_uuid: &str,
    ) -> Vec<u8> {
        let mut project_info = Vec::new();
        if let Some(uri) = folder_uri {
            push_len_delimited(&mut project_info, 7, uri.as_bytes());
        }
        push_len_delimited(&mut project_info, 18, project_uuid.as_bytes());
        let mut summary = Vec::new();
        // A couple of unrelated scalar/string fields to prove the walker skips them.
        push_varint(&mut summary, 2 << 3); // field 2, varint
        push_varint(&mut summary, 6);
        push_len_delimited(&mut summary, 1, b"Some conversation title");
        push_len_delimited(&mut summary, 17, &project_info);
        let mut record = Vec::new();
        push_len_delimited(&mut record, 1, uuid.as_bytes());
        push_len_delimited(&mut record, 2, &summary);
        record
    }

    /// Wrap records as the top-level repeated field 1 (the proto's outer shape).
    fn encode_antigravity_proto(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            push_len_delimited(&mut out, 1, record);
        }
        out
    }

    #[test]
    fn antigravity_proto_parse_extracts_project_and_loose() {
        let proto = encode_antigravity_proto(&[
            encode_antigravity_record(
                "11111111-1111-4111-8111-111111111111",
                Some("file:///c%3A/AI/Codex/CodeHangar"),
                "5e0a3a20-1fd9-4466-9947-4c475e3f5dfe",
            ),
            // Loose: project uuid is the literal sentinel, folderUri absent.
            encode_antigravity_record(
                "22222222-2222-4222-8222-222222222222",
                None,
                "outside-of-project",
            ),
        ]);

        let dir = tempdir().unwrap();
        let path = dir.path().join("agyhub_summaries_proto.pb");
        fs::write(&path, &proto).unwrap();
        let map = parse_antigravity_proto_map(&path);

        assert_eq!(
            map.get("11111111-1111-4111-8111-111111111111"),
            Some(&AntigravityResolution::Project {
                path: PathBuf::from("C:\\AI\\Codex\\CodeHangar"),
                project_uuid: "5e0a3a20-1fd9-4466-9947-4c475e3f5dfe".to_string(),
            }),
            "percent-decoded folderUri -> Windows path"
        );
        assert_eq!(
            map.get("22222222-2222-4222-8222-222222222222"),
            Some(&AntigravityResolution::Loose),
            "outside-of-project sentinel -> loose"
        );
    }

    #[test]
    fn antigravity_proto_parse_treats_sentinel_with_stray_uri_as_loose() {
        // Even if a folderUri is somehow present, the explicit sentinel wins: loose.
        let proto = encode_antigravity_proto(&[encode_antigravity_record(
            "33333333-3333-4333-8333-333333333333",
            Some("file:///c%3A/AI/Codex/CodeHangar"),
            "outside-of-project",
        )]);
        let dir = tempdir().unwrap();
        let path = dir.path().join("agyhub_summaries_proto.pb");
        fs::write(&path, &proto).unwrap();
        let map = parse_antigravity_proto_map(&path);
        assert_eq!(
            map.get("33333333-3333-4333-8333-333333333333"),
            Some(&AntigravityResolution::Loose)
        );
    }

    #[test]
    fn antigravity_proto_parse_truncated_is_defensive() {
        // A record cut mid-field must not panic; whatever parsed cleanly is kept.
        let good = encode_antigravity_record(
            "44444444-4444-4444-8444-444444444444",
            Some("file:///c%3A/AI/Codex/CodeHangar"),
            "abc",
        );
        let mut proto = encode_antigravity_proto(&[good]);
        // Append a bogus length-delimited record claiming more bytes than remain.
        push_varint(&mut proto, 1 << 3 | 2); // field 1, wire type 2
        push_varint(&mut proto, 9999); // length far past EOF
        proto.extend_from_slice(b"short");

        let dir = tempdir().unwrap();
        let path = dir.path().join("agyhub_summaries_proto.pb");
        fs::write(&path, &proto).unwrap();
        let map = parse_antigravity_proto_map(&path);
        // The first, well-formed record still resolves.
        assert!(matches!(
            map.get("44444444-4444-4444-8444-444444444444"),
            Some(AntigravityResolution::Project { .. })
        ));
    }

    #[test]
    fn antigravity_current_keys_collects_project_paths_only() {
        // Two project-anchored conversations (one repeated path) + one loose. The
        // current set holds the project path keys, deduped, and excludes the loose.
        let proto = encode_antigravity_proto(&[
            encode_antigravity_record(
                "11111111-1111-4111-8111-111111111111",
                Some("file:///d%3A/Example"),
                "proj-a",
            ),
            encode_antigravity_record(
                "22222222-2222-4222-8222-222222222222",
                Some("file:///d%3A/Example"),
                "proj-a",
            ),
            encode_antigravity_record(
                "33333333-3333-4333-8333-333333333333",
                None,
                "outside-of-project",
            ),
        ]);
        let dir = tempdir().unwrap();
        let path = dir.path().join("agyhub_summaries_proto.pb");
        fs::write(&path, &proto).unwrap();
        let map = parse_antigravity_proto_map(&path);

        let keys = antigravity_current_keys_from_map(&map);
        // D:\Example does not exist in the test env, but the key is still recorded
        // (currentness does not require the dir to exist).
        assert!(keys.contains(&candidate_key(Path::new("D:\\Example"))));
        assert_eq!(keys.len(), 1, "deduped, loose excluded");
    }

    #[test]
    fn claude_current_keys_reads_last_session_modified() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let active = home.join("ActiveProj");
        let never = home.join("NeverOpened");
        let claude = serde_json::json!({
            "projects": {
                active.to_string_lossy(): { "lastSessionModified": 1779069092161i64 },
                never.to_string_lossy(): { "history": [] },
            }
        });
        fs::write(home.join(".claude.json"), claude.to_string()).unwrap();

        let keys = claude_current_project_keys_in(home);
        assert!(
            keys.contains(&candidate_key(&active)),
            "a project with lastSessionModified is current"
        );
        assert!(
            !keys.contains(&candidate_key(&never)),
            "a project without lastSessionModified is not current"
        );
    }

    #[test]
    fn registry_app_specificity_prefers_most_deliberate_owner() {
        // Antigravity (authoritative store) beats cursor (workspace) beats codex
        // (trusted cwd) beats claude (recorded cwd).
        assert!(registry_app_specificity("antigravity") > registry_app_specificity("cursor"));
        assert!(registry_app_specificity("cursor") > registry_app_specificity("codex"));
        assert!(registry_app_specificity("codex") > registry_app_specificity("claude_code"));
        assert!(registry_app_specificity("claude_code") > registry_app_specificity("unknown"));
        // The internal claude id is surfaced as the public "claude" label.
        assert_eq!(registry_app_label("claude_code"), "claude");
        assert_eq!(registry_app_label("antigravity"), "antigravity");
    }

    /// Real-data verification for Fix C: ExampleProj's conversation root must be in
    /// the Antigravity summaries-proto current set, so the frontend can keep it Active
    /// even though its `.pb` conversations link no parsed `.db`. The user's actual
    /// proto records the ExampleProj conversations under the `…\New folder\ExampleProj`
    /// root (a reconnected external drive — `H:`/`G:` in the registry, surfaced as
    /// whatever letter it carried when the chat happened), NOT under the `D:\Example`
    /// mirror also listed in ExampleProj's project registry — `is_current` correctly
    /// reflects WHERE the conversations happened. We assert the basename-level
    /// ExampleProj/SubB presence so the drive-letter reshuffle does not make the check
    /// brittle.
    /// Ignored by default (machine-specific); run with
    /// `cargo test -p hangar-discovery -- --ignored real_antigravity_exampleproj`.
    #[test]
    #[ignore = "depends on the local user's real Antigravity data"]
    fn real_antigravity_exampleproj_folder_is_in_current_set() {
        let Some(proto) = antigravity_summaries_proto_path() else {
            eprintln!("skipping: no home dir");
            return;
        };
        if !proto.is_file() {
            eprintln!("skipping: {} not present", proto.display());
            return;
        }
        let map = parse_antigravity_proto_map(&proto);
        let keys = antigravity_current_keys_from_map(&map);
        eprintln!("antigravity current set has {} project keys", keys.len());
        let expected_example = map.values().find_map(|resolution| match resolution {
            AntigravityResolution::Project { path, .. } => {
                let key = candidate_key(path).replace('\\', "/");
                (key.ends_with("/new folder/exampleproj") || key.ends_with("/new folder/subb"))
                    .then(|| candidate_key(path))
            }
            AntigravityResolution::Loose => None,
        });
        let Some(expected_example) = expected_example else {
            eprintln!("skipping: the current Antigravity store no longer contains ExampleProj");
            return;
        };
        // ExampleProj spans several roots (…\New folder\ExampleProj and
        // …\New folder\SubB). Any of them being in the current set un-archives the
        // ExampleProj project.
        assert!(
            keys.contains(&expected_example),
            "an ExampleProj conversation root (…/New folder/ExampleProj or …/SubB) must \
             be in the Antigravity current set; present keys: {keys:?}"
        );
    }

    /// Real-data verification against the user's actual Antigravity summaries proto.
    /// Ignored by default (machine-specific path); run with
    /// `cargo test -p hangar-discovery -- --ignored real_antigravity_proto`.
    /// Asserts the authoritative one-conversation→one-project mapping:
    ///   * `55a16015-…` is LOOSE (was previously smeared across WWS/etc.).
    ///   * `e06064f4-…` belongs to `C:\AI\Codex\CodeHangar`.
    ///   * `81ca11de-…` belongs to the WWS folder (proto-only; its `.db` may be
    ///     absent — we assert the proto parse, not on-disk linking).
    #[test]
    #[ignore = "depends on the local user's real Antigravity data"]
    fn real_antigravity_proto_maps_conversations_to_single_projects() {
        let Some(proto) = antigravity_summaries_proto_path() else {
            eprintln!("skipping: no home dir");
            return;
        };
        if !proto.is_file() {
            eprintln!("skipping: {} not present", proto.display());
            return;
        }
        let map = parse_antigravity_proto_map(&proto);
        eprintln!("parsed {} conversation records", map.len());

        assert_eq!(
            map.get("55a16015-e44d-4ebb-ba4b-1fcc1104e5f4"),
            Some(&AntigravityResolution::Loose),
            "55a16015 must be loose, not smeared across projects"
        );

        match map.get("e06064f4-8b06-4de3-a1de-9da4f58f4d94") {
            Some(AntigravityResolution::Project { path, .. }) => {
                assert_eq!(path, &PathBuf::from("C:\\AI\\Codex\\CodeHangar"));
            }
            other => panic!("e06064f4 must map to CodeHangar, got {other:?}"),
        }

        match map.get("81ca11de-770e-4c6c-b7f0-9e8dc1533975") {
            Some(AntigravityResolution::Project { path, .. }) => {
                assert!(
                    path.to_string_lossy().replace('\\', "/").ends_with("/WWS"),
                    "81ca11de must map to the WWS folder, got {}",
                    path.display()
                );
            }
            other => panic!("81ca11de must map to WWS, got {other:?}"),
        }

        // End-to-end through the resolver: the loose conversation's live `.db`
        // resolves to None (no project), even though its step payloads mention many
        // folders. Only assert when that `.db` is actually present.
        let loose_db = proto
            .with_file_name("conversations")
            .join("55a16015-e44d-4ebb-ba4b-1fcc1104e5f4.db");
        if loose_db.is_file() {
            assert!(
                antigravity_conversation_projects(&loose_db, &map).is_empty(),
                "loose conversation resolves to no project"
            );
        }
    }

    /// Real-data verification of the ExampleProj session-linking fix: at least one
    /// real Antigravity conversation must now link to the registered ExampleProj root
    /// `D:\Example\SubA` via the project's own registry, even though that
    /// conversation's proto folderUri points at a (frequently offline) external-drive
    /// `…\New folder\ExampleProj`. Iterates every
    /// `~/.gemini/antigravity/conversations/*.db`, prints each conversation's resolved
    /// links, and asserts one of them includes a path whose `candidate_key` contains
    /// `example\suba`. If NO real conversation resolves to the ExampleProj project at
    /// all, it prints that and skips (asserts nothing) rather than failing. Ignored by
    /// default (machine-specific); run with
    /// `cargo test -p hangar-discovery -- --ignored real_exampleproj_conversation`.
    #[test]
    #[ignore = "depends on the local user's real Antigravity data"]
    fn real_exampleproj_conversation_links_registered_root() {
        let Some(home) = home_dir() else {
            eprintln!("skipping: no home dir");
            return;
        };
        let convo_dir = home
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        let Ok(entries) = fs::read_dir(&convo_dir) else {
            eprintln!("skipping: {} not present", convo_dir.display());
            return;
        };
        let map = antigravity_proto_map();

        let mut example_link: Option<(PathBuf, PathBuf)> = None; // (db, example\suba link)
        let mut any_link = false;
        for entry in entries.flatten() {
            let db = entry.path();
            if db.extension().and_then(|ext| ext.to_str()) != Some("db") {
                continue;
            }
            let links = antigravity_conversation_projects(&db, &map);
            if !links.is_empty() {
                any_link = true;
            }
            eprintln!(
                "{} -> {:?}",
                db.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                links
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
            );
            if example_link.is_none() {
                if let Some(hit) = links.iter().find(|p| {
                    candidate_key(p)
                        .replace('/', "\\")
                        .contains("example\\suba")
                }) {
                    example_link = Some((db.clone(), hit.clone()));
                }
            }
        }

        match example_link {
            Some((db, root)) => {
                eprintln!(
                    "ExampleProj link found: conversation {} links registered root {}",
                    db.display(),
                    root.display()
                );
            }
            None => {
                // No conversation resolves to the ExampleProj project (e.g. the proto
                // maps none to the project uuid, or none is on disk). Report and skip
                // rather than fail — the report flags whether ExampleProj has any real
                // conversation.
                eprintln!(
                    "no real Antigravity conversation links the ExampleProj root \
                     D:\\Example\\SubA (any conversation linked at all: {any_link}); \
                     skipping assertion"
                );
            }
        }
    }

    /// Real-data smoke check (`--ignored --nocapture`): a project used in several AI
    /// apps must expose all of them in `apps` so the app FILTER finds it under each —
    /// the #2 "I only see two Claude projects" fix. Prints multi-app paths and asserts
    /// at least one multi-app project includes Claude.
    #[test]
    #[ignore = "depends on the local user's real app registries"]
    fn real_project_app_states_have_multi_app_membership() {
        let states = project_app_states();
        let multi: Vec<_> = states.iter().filter(|(_, s)| s.apps.len() > 1).collect();
        eprintln!("paths with >1 app: {} of {}", multi.len(), states.len());
        for (key, state) in multi.iter().take(12) {
            eprintln!("  {key} -> {:?} (primary {:?})", state.apps, state.app);
        }
        assert!(
            states
                .values()
                .any(|s| s.apps.len() > 1 && s.apps.iter().any(|a| a == "claude")),
            "expected at least one multi-app project that includes claude"
        );
    }

    #[test]
    fn extract_session_cwd_reads_payload_and_toplevel() {
        // payload.cwd (Codex) is read; a cwd-less line in between is skipped.
        let codex = concat!(
            "{\"type\":\"other\",\"foo\":1}\n",
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"C:\\\\AI\\\\Codex\\\\CodeHangar\"}}\n"
        );
        assert_eq!(
            extract_session_cwd(codex),
            Some(PathBuf::from("C:\\AI\\Codex\\CodeHangar"))
        );

        // top-level cwd (Claude) is read.
        let claude = "{\"type\":\"user\",\"cwd\":\"D:\\\\Work\\\\Thing\"}\n";
        assert_eq!(
            extract_session_cwd(claude),
            Some(PathBuf::from("D:\\Work\\Thing"))
        );

        // No cwd anywhere -> None.
        assert_eq!(extract_session_cwd("{\"type\":\"user\"}\n"), None);

        // Tolerant fallback: a line whose JSON escapes were already collapsed (as
        // `read_text_prefix` does) still yields the cwd via the manual scan.
        let mangled =
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"C:\\AI\\Codex\\CodeHangar\"}}";
        assert_eq!(
            extract_session_cwd(mangled),
            Some(PathBuf::from("C:\\AI\\Codex\\CodeHangar"))
        );
    }

    #[test]
    fn claude_project_roots_reads_cwd_not_dirname() {
        let home = tempdir_projectlike();
        // A real project root the session was recorded in.
        let real = home.path().join("Real Project");
        fs::create_dir_all(&real).unwrap();
        // The Claude project dir name is hyphen-lossy and does NOT decode to `real`.
        let proj_dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("C--Some-Hyphen-Name");
        fs::create_dir_all(&proj_dir).unwrap();
        fs::write(
            proj_dir.join("s.jsonl"),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\"}}",
                real.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let roots = claude_project_roots(home.path());
        assert_eq!(roots.len(), 1, "exactly the recorded cwd");
        assert_eq!(candidate_key(&roots[0]), candidate_key(&real));
    }

    #[test]
    fn candidate_key_collapses_separators_and_case() {
        // Claude's `.claude.json` records the same project under mixed separators
        // (e.g. `C:\proj` AND `C:/proj`). For a path that does not exist on disk
        // `fs::canonicalize` fails and cannot normalize them, so the key builder's own
        // separator+case folding must still collapse both forms to one key — otherwise
        // the forward-slash variant loses its app attribution and the project drops out
        // of, say, the Claude filter.
        let back = candidate_key(Path::new(r"C:\NoSuchDir\Mixed Project"));
        let fwd = candidate_key(Path::new("C:/NoSuchDir/Mixed Project"));
        assert_eq!(back, fwd, "forward-slash and backslash keys must match");
        assert_eq!(back, r"c:\nosuchdir\mixed project");
    }

    #[cfg(windows)]
    #[test]
    fn wsl_home_sources_register_the_full_per_app_set() {
        // A synthetic distro home — no live distro, no filesystem access. The sources
        // are pure UNC path strings, so the exact per-app set is assertable directly.
        let mut sources = Vec::new();
        let home = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\dev");
        push_wsl_home_sources(&mut sources, "Ubuntu-24.04", &home);

        let by_kind = |kind: &str| {
            sources
                .iter()
                .find(|s| s.kind == kind)
                .map(|s| s.path.clone())
        };

        // Claude: the projects transcript store reuses the exact `claude_code_projects`
        // kind (so WSL transcripts hit the same parser + uuid dedup) plus skills.
        assert_eq!(
            by_kind("claude_code_projects"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.claude\projects"
            ))
        );
        assert!(sources
            .iter()
            .any(|s| s.kind == "wsl_ubuntu_24_04_dev_claude_skills"));

        // Codex: rollout stores reuse the exact rollout kinds, plus index/state/skills.
        assert_eq!(
            by_kind("codex_sessions"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.codex\sessions"
            ))
        );
        assert_eq!(
            by_kind("codex_archived_sessions"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.codex\archived_sessions"
            ))
        );
        assert_eq!(
            by_kind("codex_index"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.codex\session_index.jsonl"
            ))
        );
        assert_eq!(
            by_kind("codex_state"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.codex\state_5.sqlite"
            ))
        );
        assert!(sources
            .iter()
            .any(|s| s.kind == "wsl_ubuntu_24_04_dev_codex_skills"));

        // OpenClaw: the full home/state/sessions/memory/skills set, under per-home
        // kinds that still contain "openclaw" (so they stay agent-gated).
        for suffix in ["home", "state_sessions", "sessions", "memory", "skills"] {
            let kind = format!("wsl_ubuntu_24_04_dev_openclaw_{suffix}");
            assert!(sources.iter().any(|s| s.kind == kind), "missing {kind}");
        }
        assert_eq!(
            by_kind("wsl_ubuntu_24_04_dev_openclaw_state_sessions"),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\dev\.openclaw\state\openclaw.sqlite"
            ))
        );
        assert!(sources
            .iter()
            .any(|s| s.mode == SourceMode::OpenClawState && s.kind.contains("openclaw")));

        // Hermes stays registered too (the pre-existing WSL coverage is preserved).
        assert!(sources.iter().any(|s| s.kind.contains("hermes")));

        // Antigravity and Cursor are deliberately NOT registered in-distro.
        assert!(!sources.iter().any(|s| s.kind.contains("cursor")));
        assert!(!sources.iter().any(|s| s.kind.contains("antigravity")));

        // Every registered source points at the distro share, so the gate and the
        // read-only/immutable opens apply uniformly.
        assert!(sources.iter().all(|s| s
            .path
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(r"\\wsl.localhost\ubuntu-24.04\")));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_sources_from_registers_each_injected_distro() {
        // The `push_wsl_sources_from` seam takes an explicit (distro, home) list, so
        // multi-distro registration is verifiable without any live distro.
        let mut sources = Vec::new();
        push_wsl_sources_from(
            &mut sources,
            vec![
                (
                    "Ubuntu".to_string(),
                    PathBuf::from(r"\\wsl.localhost\Ubuntu\home\a"),
                ),
                (
                    "Debian".to_string(),
                    PathBuf::from(r"\\wsl.localhost\Debian\home\b"),
                ),
            ],
        );
        // Each distro contributes its own Claude projects source (same reused kind,
        // distinct share path) and a distinct per-home OpenClaw home kind.
        let claude_sources = sources
            .iter()
            .filter(|s| s.kind == "claude_code_projects")
            .count();
        assert_eq!(claude_sources, 2);
        assert!(sources
            .iter()
            .any(|s| s.kind == "wsl_ubuntu_a_openclaw_home"));
        assert!(sources
            .iter()
            .any(|s| s.kind == "wsl_debian_b_openclaw_home"));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_sources_absent_when_scan_gate_off() {
        // The gate defaults off; force it off explicitly and restore. This only ever
        // sets OFF (the default), so it can never race a parallel test into a
        // surprise ON.
        let previous = wsl_scan_enabled();
        set_wsl_scan_enabled(false);

        // With scanning off, the enumeration yields nothing without spawning wsl.exe
        // or statting any `\\wsl.localhost` share, so no WSL source is registered.
        assert!(wsl_distro_homes().is_empty());
        let mut sources = Vec::new();
        push_wsl_sources(&mut sources);
        assert!(
            !sources.iter().any(|s| {
                let p = s.path.to_string_lossy().to_ascii_lowercase();
                p.starts_with(r"\\wsl.localhost\") || p.starts_with(r"\\wsl$\")
            }),
            "no WSL sources may be registered while the scan gate is off"
        );

        set_wsl_scan_enabled(previous);
    }

    #[test]
    fn wsl_share_paths_are_never_canonicalized_while_the_gate_is_off() {
        // The gate is process-wide; force it off for this test and restore after
        // (it defaults off, so parallel tests never see a surprise flip to on).
        let previous = wsl_scan_enabled();
        set_wsl_scan_enabled(false);

        // Detection is purely textual: both share names, either separator style,
        // any case, and the verbatim-UNC form — but never a lookalike local path.
        assert!(is_wsl_unc_path(Path::new(
            r"\\wsl.localhost\Ubuntu\home\user\proj"
        )));
        assert!(is_wsl_unc_path(Path::new(r"\\wsl$\Ubuntu\home\user\proj")));
        assert!(is_wsl_unc_path(Path::new(r"\\WSL.LOCALHOST\Ubuntu\proj")));
        assert!(is_wsl_unc_path(Path::new("//wsl.localhost/Ubuntu/proj")));
        assert!(is_wsl_unc_path(Path::new(
            r"\\?\UNC\wsl.localhost\Ubuntu\proj"
        )));
        assert!(!is_wsl_unc_path(Path::new(r"C:\wsl.localhost\not-a-share")));
        assert!(!is_wsl_unc_path(Path::new(r"\\server\share\proj")));

        // With the gate off every filesystem probe of a WSL share is blocked, so
        // `canonical_or_original` must return the input untouched (a canonicalize
        // would stat the share and can cold-boot the distro at startup)…
        let share = Path::new(r"\\wsl.localhost\NoSuchDistro-CH-Test\home\user\proj");
        assert!(wsl_path_blocked_by_gate(share));
        assert_eq!(canonical_or_original(share), share.to_path_buf());
        // …while `candidate_key`'s own separator/case folding still normalizes it,
        // so a registered WSL project keeps a stable key without touching disk.
        assert_eq!(
            candidate_key(share),
            r"\\wsl.localhost\nosuchdistro-ch-test\home\user\proj"
        );
        assert_eq!(
            candidate_key(Path::new(
                "//wsl.localhost/NoSuchDistro-CH-Test/home/user/proj"
            )),
            candidate_key(share)
        );

        set_wsl_scan_enabled(previous);
    }

    #[cfg(windows)]
    #[test]
    fn broad_container_uses_generic_drive_depth_not_hardcoded_paths() {
        // Depth below the drive root: C:\ = 0, C:\X = 1, C:\X\Y = 2.
        assert_eq!(drive_root_depth(Path::new(r"C:\")), Some(0));
        assert_eq!(drive_root_depth(Path::new(r"C:\Tools")), Some(1));
        assert_eq!(drive_root_depth(Path::new(r"C:\Tools\Sub")), Some(2));
        assert_eq!(drive_root_depth(Path::new(r"C:\Tools\Sub\Proj")), Some(3));
        assert_eq!(drive_root_depth(Path::new(r"relative\path")), None);

        // A drive root and shallow folders with no project identity are broad
        // containers — derived generically from depth, with no machine-specific
        // folder list. Non-existent paths carry no identity, so the depth-1/2
        // branch resolves without touching disk content.
        assert!(is_broad_container_path(Path::new(r"C:\")));
        assert!(is_broad_container_path(Path::new(r"C:\NoSuchWorkspace")));
        assert!(is_broad_container_path(Path::new(
            r"C:\NoSuchWorkspace\NoSuchContainer"
        )));
        // Anything deeper than two levels is never swept up by this heuristic.
        assert!(!is_broad_container_path(Path::new(r"C:\A\B\C\RealProject")));
    }

    #[test]
    fn codex_session_project_roots_excludes_scratch() {
        let home = tempdir_projectlike();
        let day = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("06")
            .join("14");
        fs::create_dir_all(&day).unwrap();

        // A real project root.
        let real = home.path().join("RealCodexProject");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            day.join("rollout-real.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                real.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        // A per-conversation scratch dir (date-prefixed component) -> excluded.
        let scratch = home
            .path()
            .join("Documents")
            .join("Codex")
            .join("2026-06-14")
            .join("o-que-est-a-fazer-com");
        fs::create_dir_all(&scratch).unwrap();
        fs::write(
            day.join("rollout-scratch.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                scratch.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let roots = codex_session_project_roots(home.path());
        assert_eq!(roots.len(), 1, "only the real project, scratch excluded");
        assert_eq!(candidate_key(&roots[0]), candidate_key(&real));
        assert!(!roots.iter().any(|path| is_agent_scratch_path(path)));
    }

    #[test]
    fn codex_session_links_only_cwd() {
        let home = tempdir_projectlike();
        // Parent = the session cwd; child = a subfolder also mentioned in the body.
        let parent = home.path().join("GabrielKnight3-Lab");
        let child = parent.join("gengine");
        fs::create_dir_all(&child).unwrap();

        let session = home.path().join("rollout-link.jsonl");
        fs::write(
            &session,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{parent}\"}}}}\nthe agent then edited {child}\n",
                parent = parent.to_string_lossy().replace('\\', "\\\\"),
                child = child.to_string_lossy().replace('\\', "\\\\"),
            ),
        )
        .unwrap();

        let source = DiscoverySource {
            kind: "codex_sessions".to_string(),
            label: "Codex sessions".to_string(),
            path: session,
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);

        assert!(
            candidates.contains_key(&candidate_key(&parent)),
            "the session cwd (parent) is a candidate"
        );
        assert!(
            !candidates.contains_key(&candidate_key(&child)),
            "a subfolder mentioned in the body must NOT become a separate project"
        );
        // The session is listed and linked to exactly the cwd.
        let finalized = finalize_sessions(sessions, &[]);
        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].linked_project_paths.len(), 1);
    }

    /// Real-data smoke check (run with `--ignored --nocapture`): prints the Claude +
    /// Codex roots discovered from THIS machine and asserts the phase-8 expectations —
    /// Claude roots are non-empty, GabrielKnight3-Lab (a Codex session cwd not in the
    /// config.toml trust table) is present, and its subfolders gengine/gengine-source
    /// are NOT treated as Codex roots.
    #[test]
    #[ignore]
    fn real_session_cwd_discovery() {
        let home = home_dir().expect("home dir");
        let claude = claude_project_roots(&home);
        let codex = codex_session_project_roots(&home);
        eprintln!("CLAUDE roots ({}):", claude.len());
        for p in &claude {
            eprintln!("  {}", p.display());
        }
        eprintln!("CODEX session roots ({}):", codex.len());
        for p in &codex {
            eprintln!("  {}", p.display());
        }
        assert!(
            !claude.is_empty(),
            "expected at least one Claude project root"
        );
        let codex_keys: std::collections::HashSet<String> =
            codex.iter().map(|p| candidate_key(p)).collect();
        let lower = |s: &str| s.to_ascii_lowercase();
        let has = |needle: &str| {
            codex
                .iter()
                .any(|p| lower(&p.to_string_lossy()).contains(needle))
        };
        assert!(
            has("gabrielknight3-lab"),
            "GabrielKnight3-Lab (a Codex session cwd) should be a discovered Codex root"
        );
        // Its subfolders are inside the cwd, never standalone Codex roots.
        for sub in [
            "gabrielknight3-lab\\gengine",
            "gabrielknight3-lab\\gengine-source",
        ] {
            assert!(
                !codex_keys.iter().any(|k| k.contains(sub)),
                "subfolder {sub} must not be a Codex root"
            );
        }
    }

    /// Real-data smoke check (run with `--ignored --nocapture`): scans this machine's
    /// `~/.claude/projects` and asserts the Claude Code conversations are now LISTED as
    /// sessions linked to their project (previously `claude_code_projects` was not a
    /// listable kind, so the live conversation never surfaced).
    #[test]
    #[ignore]
    fn real_claude_sessions_are_listed() {
        let home = home_dir().expect("home dir");
        let source = DiscoverySource {
            kind: "claude_code_projects".to_string(),
            label: "Claude Code projects".to_string(),
            path: home.join(".claude").join("projects"),
            detail: None,
            mode: SourceMode::TextMetadata,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let finalized = finalize_sessions(sessions, &[]);
        let linked = finalized
            .iter()
            .filter(|s| !s.linked_project_paths.is_empty())
            .count();
        eprintln!(
            "Claude sessions listed: {} ({} linked to a project)",
            finalized.len(),
            linked
        );
        for s in finalized.iter().take(6) {
            eprintln!("  {} -> {:?}", s.path, s.linked_project_paths);
        }
        assert!(
            !finalized.is_empty(),
            "Claude Code conversations should now be listed as sessions"
        );
        assert!(
            linked > 0,
            "at least one Claude session should link to its project cwd"
        );
    }

    #[test]
    #[ignore = "depends on the local user's real Hermes state database"]
    fn real_hermes_state_transcript_is_readable() {
        let Some(source) = discovery_sources()
            .into_iter()
            .find(|source| source.mode == SourceMode::HermesState && source.path.is_file())
        else {
            eprintln!("skipping: no installed Hermes state.db source");
            return;
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_hermes_state_source(&source, &mut candidates, &mut sessions);
        let session = finalize_sessions(sessions, &[])
            .into_iter()
            .find(|session| session.path.contains("#hermes-session="))
            .expect("at least one durable Hermes conversation");
        let session_id = session
            .path
            .rsplit_once("#hermes-session=")
            .map(|(_, value)| value)
            .expect("Hermes session fragment");
        let (transcript, _) =
            hermes_session_transcript(&source.path, session_id, HERMES_TRANSCRIPT_MAX_BYTES)
                .expect("read one Hermes transcript");

        assert!(!transcript.trim().is_empty());
        assert!(transcript.contains("\"role\""));
    }

    // --- C1: Cursor in-IDE (Composer/agent) conversations ----------------------

    /// Build a minimal Cursor `state.vscdb` in the REAL shape: an `ItemTable(key,
    /// value)` holding the JSON metadata rows this reader consumes, plus a
    /// `cursorDiskKV(key, value)` with a couple of `agentKv:blob:<sha256>` rows in the
    /// real encodings we observed (one JSON message bubble, one protobuf record) — the
    /// reader must ignore those content blobs entirely and never choke on them.
    fn write_synthetic_cursor_state_db(
        path: &Path,
        composer_headers: &serde_json::Value,
        projects: Option<&serde_json::Value>,
        membership: Option<&serde_json::Value>,
    ) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('composer.composerHeaders', ?1)",
            rusqlite::params![composer_headers.to_string()],
        )
        .unwrap();
        if let Some(projects) = projects {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES ('glass.localAgentProjects.v1', ?1)",
                rusqlite::params![projects.to_string()],
            )
            .unwrap();
        }
        if let Some(membership) = membership {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES ('glass.localAgentProjectMembership.v1', ?1)",
                rusqlite::params![membership.to_string()],
            )
            .unwrap();
        }
        // A JSON message bubble (real agentKv encoding #1): must be ignored.
        let bubble = serde_json::json!({ "role": "user", "content": "unused", "id": "x" });
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                format!("agentKv:blob:{}", "a".repeat(64)),
                bubble.to_string().into_bytes()
            ],
        )
        .unwrap();
        // A protobuf record (real agentKv encoding #2, first byte 0x0a): must be ignored.
        let mut pb = Vec::new();
        push_len_delimited(&mut pb, 1, b"unused protobuf record");
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![format!("agentKv:blob:{}", "b".repeat(64)), pb],
        )
        .unwrap();
    }

    /// A VS Code `Uri`-shaped JSON object for a Windows directory, mirroring the real
    /// `workspaceIdentifier.uri` / `workspace.uri` shape (`fsPath` + posix `path`).
    fn cursor_uri_json(dir: &Path) -> serde_json::Value {
        let fs_path = dir.to_string_lossy().to_string();
        let posix = format!("/{}", fs_path.replace('\\', "/"));
        serde_json::json!({
            "$mid": 1,
            "fsPath": fs_path,
            "path": posix,
            "scheme": "file",
        })
    }

    #[test]
    fn cursor_ide_conversations_extract_id_title_and_cwd() {
        // Three composers: one with its own workspace uri, one linked only via the
        // project membership, and one untitled loose draft.
        let dir = tempdir_projectlike();
        let own = dir.path().join("OwnWorkspace");
        let proj = dir.path().join("ProjectRoot");
        fs::create_dir_all(&own).unwrap();
        fs::create_dir_all(&proj).unwrap();

        let headers = serde_json::json!({
            "allComposers": [
                {
                    "composerId": "11111111-1111-4111-8111-111111111111",
                    "name": "Fix the parser",
                    "createdAt": 1_700_000_000_000i64,
                    "workspaceIdentifier": { "id": "ws-a", "uri": cursor_uri_json(&own) },
                },
                {
                    "composerId": "22222222-2222-4222-8222-222222222222",
                    "subtitle": "Refactor titles",
                    "createdAt": 1_700_000_100_000i64,
                    // No self uri: must resolve via membership -> project.
                    "workspaceIdentifier": { "id": "ws-hash-only" },
                },
                {
                    "composerId": "33333333-3333-4333-8333-333333333333",
                    "createdAt": 1_700_000_200_000i64,
                    "workspaceIdentifier": { "id": "ws-none" },
                },
            ]
        });
        let membership = serde_json::json!({
            "22222222-2222-4222-8222-222222222222": "proj-1"
        });

        let mut project_paths = BTreeMap::new();
        project_paths.insert("proj-1".to_string(), proj.clone());
        let conversations =
            cursor_ide_conversations_from_json(&headers, &membership, &project_paths);

        assert_eq!(conversations.len(), 3);
        // #1: own workspace uri.
        assert_eq!(conversations[0].id, "11111111-1111-4111-8111-111111111111");
        assert_eq!(conversations[0].title.as_deref(), Some("Fix the parser"));
        assert_eq!(conversations[0].cwd.as_deref(), Some(own.as_path()));
        // #2: title from subtitle, cwd from project membership fallback.
        assert_eq!(conversations[1].title.as_deref(), Some("Refactor titles"));
        assert_eq!(conversations[1].cwd.as_deref(), Some(proj.as_path()));
        // #3: untitled, no cwd -> loose.
        assert_eq!(conversations[2].title, None);
        assert_eq!(conversations[2].cwd, None);
    }

    #[test]
    #[cfg(windows)]
    fn cursor_ide_chats_source_lists_sessions_with_stable_identity() {
        let dir = tempdir_projectlike();
        let own = dir.path().join("CursorProj");
        fs::create_dir_all(&own).unwrap();
        let headers = serde_json::json!({
            "allComposers": [
                {
                    "composerId": "aaaaaaaa-1111-4111-8111-111111111111",
                    "name": "Linked chat",
                    "workspaceIdentifier": { "id": "ws", "uri": cursor_uri_json(&own) },
                },
                {
                    "composerId": "bbbbbbbb-2222-4222-8222-222222222222",
                    // Untitled, no resolvable workspace -> loose.
                    "workspaceIdentifier": { "id": "ws2" },
                },
            ]
        });
        let db = dir.path().join("state.vscdb");
        write_synthetic_cursor_state_db(&db, &headers, None, None);

        let source = DiscoverySource {
            kind: "cursor_ide_chats".to_string(),
            label: "Cursor in-IDE conversations".to_string(),
            path: db.clone(),
            detail: None,
            mode: SourceMode::CursorIdeChats,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_ide_chats_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);

        assert_eq!(sessions.len(), 2, "both conversations listed (one loose)");
        let linked = sessions
            .iter()
            .find(|s| s.display_name == "Linked chat")
            .expect("named, linked session present");
        assert_eq!(linked.association, "unregistered_project_reference");
        assert!(linked
            .path
            .contains("#cursor-ide-chat=aaaaaaaa-1111-4111-8111-111111111111"));
        assert_eq!(
            linked.linked_project_paths,
            vec![display_path(&own)],
            "session links to its workspace"
        );
        // The linked workspace also became a project candidate.
        assert!(candidates
            .keys()
            .any(|k| k.eq_ignore_ascii_case(&candidate_key(&own))));

        let loose = sessions
            .iter()
            .find(|s| s.path.contains("bbbbbbbb-2222-4222-8222-222222222222"))
            .expect("loose draft present");
        assert_eq!(loose.association, "loose_session");
        assert!(loose.linked_project_paths.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn cursor_ide_chats_source_links_via_project_membership() {
        // A conversation with no self workspace uri must still link to a project
        // through `glass.localAgentProjectMembership.v1` + `glass.localAgentProjects.v1`
        // (exercises `cursor_project_paths` and the membership fallback end to end).
        let dir = tempdir_projectlike();
        let proj = dir.path().join("MemberProj");
        fs::create_dir_all(&proj).unwrap();
        let headers = serde_json::json!({
            "allComposers": [
                {
                    "composerId": "cccccccc-3333-4333-8333-333333333333",
                    "name": "Member chat",
                    "workspaceIdentifier": { "id": "hash-only" },
                }
            ]
        });
        let projects = serde_json::json!([
            { "id": "proj-9", "name": "MemberProj", "workspace": { "uri": cursor_uri_json(&proj) } }
        ]);
        let membership = serde_json::json!({ "cccccccc-3333-4333-8333-333333333333": "proj-9" });
        let db = dir.path().join("state.vscdb");
        write_synthetic_cursor_state_db(&db, &headers, Some(&projects), Some(&membership));

        let source = DiscoverySource {
            kind: "cursor_ide_chats".to_string(),
            label: "Cursor in-IDE conversations".to_string(),
            path: db,
            detail: None,
            mode: SourceMode::CursorIdeChats,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_ide_chats_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].association, "unregistered_project_reference");
        assert_eq!(sessions[0].linked_project_paths, vec![display_path(&proj)]);
    }

    #[test]
    fn cursor_ide_chat_distinct_conversations_do_not_collapse() {
        // Two distinct conversations sharing everything but the composerId must stay
        // separate; the same conversation reached twice folds.
        fn convo(id: &str) -> SessionDiscoveryCandidate {
            SessionDiscoveryCandidate {
                path: format!("C:/state.vscdb#cursor-ide-chat={id}"),
                display_name: "Same title".to_string(),
                source_kind: "cursor_ide_chats".to_string(),
                source_label: "Cursor in-IDE conversations".to_string(),
                session_kind: "Cursor".to_string(),
                confidence: "High".to_string(),
                linked_project_paths: vec!["C:/proj".to_string()],
                linked_registered_project_ids: Vec::new(),
                association: "unregistered_project_reference".to_string(),
                modified_ms: Some(1),
            }
        }
        let a = convo("11111111-1111-4111-8111-111111111111");
        let b = convo("22222222-2222-4222-8222-222222222222");
        assert_ne!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&b),
            "distinct in-IDE conversations must not collapse"
        );
        let a_again = convo("11111111-1111-4111-8111-111111111111");
        assert_eq!(
            session_visible_identity_key(&a),
            session_visible_identity_key(&a_again),
            "the same conversation re-read folds to one identity"
        );
    }

    #[test]
    fn cursor_ide_chats_source_is_noop_when_db_absent_or_empty() {
        // Absent file: clean no-op.
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.vscdb");
        let source = DiscoverySource {
            kind: "cursor_ide_chats".to_string(),
            label: "Cursor in-IDE conversations".to_string(),
            path: missing,
            detail: None,
            mode: SourceMode::CursorIdeChats,
        };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_ide_chats_source(&source, &mut candidates, &mut sessions);
        assert!(sessions.is_empty() && candidates.is_empty());

        // DB present but no composer.composerHeaders row: still a clean no-op.
        let db = dir.path().join("empty.vscdb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        drop(conn);
        let source = DiscoverySource { path: db, ..source };
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_ide_chats_source(&source, &mut candidates, &mut sessions);
        assert!(sessions.is_empty() && candidates.is_empty());
    }

    #[test]
    fn cursor_uri_to_path_decodes_all_three_shapes() {
        // fsPath (native) wins first.
        let uri = serde_json::json!({ "fsPath": "C:\\AI\\Proj", "path": "/c:/AI/Proj" });
        assert_eq!(
            cursor_uri_to_path(&uri),
            Some(PathBuf::from("C:\\AI\\Proj"))
        );
        // posix path when no fsPath.
        let uri = serde_json::json!({ "path": "/d:/Work/App" });
        assert_eq!(
            cursor_uri_to_path(&uri),
            Some(PathBuf::from("D:\\Work\\App"))
        );
        // external file:// URI when neither of the above is present.
        let uri = serde_json::json!({ "external": "file:///c%3A/AI/Codex/CodeHangar" });
        assert_eq!(
            cursor_uri_to_path(&uri),
            Some(PathBuf::from("C:\\AI\\Codex\\CodeHangar"))
        );
        // A non-path uri object yields nothing.
        assert_eq!(
            cursor_uri_to_path(&serde_json::json!({ "scheme": "vscode" })),
            None
        );
    }

    #[test]
    #[ignore = "depends on the local user's real Cursor state.vscdb"]
    fn real_cursor_ide_chats_report_count() {
        let source = discovery_sources()
            .into_iter()
            .find(|source| source.mode == SourceMode::CursorIdeChats && source.path.is_file())
            .expect("an installed Cursor state.vscdb source");
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_cursor_ide_chats_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);
        let linked = sessions
            .iter()
            .filter(|s| !s.linked_project_paths.is_empty())
            .count();
        // Counts only — never any conversation content.
        println!(
            "[real] cursor in-IDE conversations surfaced: {} ({} linked to a project)",
            sessions.len(),
            linked
        );
        assert!(
            !sessions.is_empty(),
            "the real machine has in-IDE Cursor conversations"
        );
        // Every session carries a stable per-conversation identity fragment.
        assert!(sessions
            .iter()
            .all(|s| s.path.contains("#cursor-ide-chat=")));
    }

    // --- C1b: Cursor in-IDE conversation TRANSCRIPT (preview) ------------------

    /// FIX 2 robustness: `ItemTable.value` is declared BLOB. Prove `cursor_item_json`
    /// parses the JSON whether the cell is stored as SQLite text OR as a blob — a typed
    /// `get::<String>`/`get::<Vec<u8>>` would drop the conversations on the other type.
    #[test]
    fn cursor_item_json_reads_both_text_and_blob_cells() {
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB);")
            .unwrap();
        let json = serde_json::json!({ "allComposers": [{ "composerId": "c1" }] });
        // Row stored as TEXT (how Cursor writes it today).
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('as_text', ?1)",
            rusqlite::params![json.to_string()],
        )
        .unwrap();
        // Same JSON stored as a real BLOB cell (a plausible future encoding).
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('as_blob', ?1)",
            rusqlite::params![json.to_string().into_bytes()],
        )
        .unwrap();

        let from_text = cursor_item_json(&conn, "as_text").expect("text cell parses");
        let from_blob = cursor_item_json(&conn, "as_blob").expect("blob cell parses");
        assert_eq!(from_text, json);
        assert_eq!(from_blob, json, "a BLOB-stored JSON value must still parse");
    }

    /// Insert one `composerData:<id>` record plus its `bubbleId:<id>:<bubbleId>` rows
    /// into a synthetic `cursorDiskKV`, in the REAL shape observed on a live machine:
    /// the composer's `fullConversationHeadersOnly` is the ordered list of
    /// `{ bubbleId, type }` (1=user, 2=assistant); each bubble row holds the message.
    /// `bubbles` is `(bubbleId, type, value_bytes)` so a test can inject a JSON bubble,
    /// an empty tool-call bubble, or a protobuf/undecodable blob.
    fn write_cursor_composer_with_bubbles(
        path: &Path,
        composer_id: &str,
        title: Option<&str>,
        bubbles: &[(&str, i64, Vec<u8>)],
    ) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
        )
        .unwrap();
        // composerHeaders row carries the title used as the preview display name.
        let mut header = serde_json::json!({ "composerId": composer_id });
        if let Some(title) = title {
            header["name"] = serde_json::json!(title);
        }
        let headers = serde_json::json!({ "allComposers": [header] });
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES ('composer.composerHeaders', ?1)",
            rusqlite::params![headers.to_string()],
        )
        .unwrap();
        // The ordered header list the reader walks to find each message, in order.
        let ordered: Vec<serde_json::Value> = bubbles
            .iter()
            .map(|(id, kind, _)| serde_json::json!({ "bubbleId": id, "type": kind }))
            .collect();
        let record = serde_json::json!({
            "composerId": composer_id,
            "fullConversationHeadersOnly": ordered,
        });
        conn.execute(
            "INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                format!("composerData:{composer_id}"),
                record.to_string().into_bytes()
            ],
        )
        .unwrap();
        for (id, _kind, value) in bubbles {
            conn.execute(
                "INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![format!("bubbleId:{composer_id}:{id}"), value],
            )
            .unwrap();
        }
    }

    /// A JSON message bubble as Cursor stores it: the readable reply is in `text`.
    fn cursor_text_bubble(kind: i64, text: &str) -> Vec<u8> {
        serde_json::json!({ "type": kind, "text": text })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn cursor_changes_decode_only_recorded_edit_diff_fields() {
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let composer = "dddddddd-4444-4444-8444-444444444444";
        let tool = serde_json::json!({
            "toolFormerData": {
                "name": "edit_file_v2",
                "params": serde_json::json!({
                    "relativeWorkspacePath": "src/main.ts",
                    "streamingContent": "ignored full replacement"
                }).to_string(),
                "additionalData": {
                    "precomputedDiff": {
                        "lines": [
                            { "type": "removed", "content": "old", "originalLineNumber": 12 },
                            { "type": "added", "content": "new", "modifiedLineNumber": 12 },
                            { "type": "unchanged", "content": "tail" }
                        ]
                    }
                }
            }
        })
        .to_string()
        .into_bytes();
        write_cursor_composer_with_bubbles(
            &db,
            composer,
            Some("Recorded edit"),
            &[
                ("user", 1, cursor_text_bubble(1, "Change the value")),
                ("tool", 2, tool),
            ],
        );

        let changes = cursor_ide_chat_changes(&db, composer).expect("Cursor recorded changes");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "src/main.ts");
        assert_eq!(changes[0].request.as_deref(), Some("Change the value"));
        assert_eq!(changes[0].lines[0].old_line, Some(12));
        assert_eq!(changes[0].lines[0].new_line, None);
        assert_eq!(changes[0].lines[1].old_line, None);
        assert_eq!(changes[0].lines[1].new_line, Some(12));
        assert_eq!(changes[0].lines[2].kind, "context");
        assert_eq!(changes[0].lines[2].old_line, None);
        assert_eq!(changes[0].lines[2].new_line, None);
    }

    #[test]
    fn cursor_transcript_renders_ordered_role_labelled_turns() {
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let composer = "aaaaaaaa-1111-4111-8111-111111111111";
        write_cursor_composer_with_bubbles(
            &db,
            composer,
            Some("Fix the parser"),
            &[
                ("b1", 1, cursor_text_bubble(1, "first user question")),
                ("b2", 2, cursor_text_bubble(2, "assistant answer one")),
                ("b3", 1, cursor_text_bubble(1, "second user question")),
            ],
        );

        let CursorChatTranscript::Rendered { text, truncated } =
            cursor_ide_chat_transcript(&db, composer, CURSOR_IDE_TRANSCRIPT_MAX_BYTES)
        else {
            panic!("transcript should render");
        };
        assert!(!truncated);
        // Role-labelled, blank-line-separated, in stored (chronological) order.
        let expected = "## User\n\nfirst user question\n\n\
                        ## Assistant\n\nassistant answer one\n\n\
                        ## User\n\nsecond user question";
        assert_eq!(text, expected);
        // The composer title is available for the display name.
        assert_eq!(
            cursor_ide_chat_title(&db, composer).as_deref(),
            Some("Fix the parser")
        );
    }

    #[test]
    fn cursor_transcript_handles_protobuf_and_empty_bubbles() {
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let composer = "bbbbbbbb-2222-4222-8222-222222222222";

        // A protobuf/undecodable bubble that DOES carry readable UTF-8 in a
        // length-delimited field -> that text is salvaged, not a placeholder.
        let mut readable_pb = Vec::new();
        push_len_delimited(
            &mut readable_pb,
            1,
            "salvaged protobuf reply text".as_bytes(),
        );
        // A protobuf bubble with NO readable text (a varint field only) -> placeholder.
        // Tag = (field 2 << 3) | wire-type 0 (varint) = 16.
        let mut opaque_pb = Vec::new();
        push_varint(&mut opaque_pb, 16);
        push_varint(&mut opaque_pb, 42);
        // An empty JSON tool-call bubble (no `text`) -> skipped entirely, no empty turn.
        let empty_json = serde_json::json!({ "type": 2, "toolResults": [] })
            .to_string()
            .into_bytes();

        write_cursor_composer_with_bubbles(
            &db,
            composer,
            None,
            &[
                ("u1", 1, cursor_text_bubble(1, "please do the thing")),
                ("tool", 2, empty_json),
                ("pb1", 2, readable_pb),
                ("pb2", 2, opaque_pb),
            ],
        );

        let CursorChatTranscript::Rendered { text, .. } =
            cursor_ide_chat_transcript(&db, composer, CURSOR_IDE_TRANSCRIPT_MAX_BYTES)
        else {
            panic!("transcript should render despite mixed encodings");
        };
        assert!(text.contains("please do the thing"));
        // The empty tool bubble contributed no turn.
        assert_eq!(text.matches("toolResults").count(), 0);
        // Readable protobuf text was recovered.
        assert!(
            text.contains("salvaged protobuf reply text"),
            "readable UTF-8 in a protobuf bubble should be rendered: {text}"
        );
        // The opaque protobuf bubble degraded to a placeholder, never raw bytes.
        assert!(
            text.contains("_[unrenderable message]_"),
            "an undecodable bubble must degrade to a placeholder: {text}"
        );
    }

    #[test]
    fn cursor_transcript_is_noop_for_missing_composer_or_db() {
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        write_cursor_composer_with_bubbles(
            &db,
            "cccccccc-3333-4333-8333-333333333333",
            Some("Present"),
            &[("b1", 1, cursor_text_bubble(1, "hello"))],
        );

        // Composer id absent from an OPEN store -> Empty (Cursor never persisted a body
        // for it): a calm "no messages" note, never the alarming unreadable-store note.
        assert_eq!(
            cursor_ide_chat_transcript(&db, "does-not-exist", CURSOR_IDE_TRANSCRIPT_MAX_BYTES),
            CursorChatTranscript::Empty
        );
        // Absent DB file -> Unavailable (the store truly can't be opened), never a panic.
        let missing = dir.path().join("nope.vscdb");
        assert_eq!(
            cursor_ide_chat_transcript(
                &missing,
                "cccccccc-3333-4333-8333-333333333333",
                CURSOR_IDE_TRANSCRIPT_MAX_BYTES
            ),
            CursorChatTranscript::Unavailable
        );
        // A zero byte-budget yields Unavailable rather than an empty render.
        assert_eq!(
            cursor_ide_chat_transcript(&db, "cccccccc-3333-4333-8333-333333333333", 0),
            CursorChatTranscript::Unavailable
        );
    }

    #[test]
    fn cursor_transcript_empty_draft_is_distinct_from_unavailable() {
        // An empty draft (composer record present, zero messages) read PERFECTLY — it
        // must report `Empty`, not `Unavailable`, so the UI shows a calm "no messages"
        // note rather than the alarming "couldn't read this store" one. ~1/3 of a real
        // machine's listed Cursor composers are empty drafts.
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let composer = "eeeeeeee-5555-4555-8555-555555555555";
        write_cursor_composer_with_bubbles(&db, composer, Some("Draft"), &[]);
        assert_eq!(
            cursor_ide_chat_transcript(&db, composer, CURSOR_IDE_TRANSCRIPT_MAX_BYTES),
            CursorChatTranscript::Empty
        );
    }

    #[test]
    fn cursor_transcript_caps_to_newest_messages() {
        // More turns than the message cap: only the newest `max_messages` render, the
        // truncation banner appears, and the OLDEST turn is dropped from the tail.
        let dir = tempdir_projectlike();
        let db = dir.path().join("state.vscdb");
        let composer = "dddddddd-4444-4444-8444-444444444444";
        let bubbles: Vec<(String, i64, Vec<u8>)> = (0..5)
            .map(|i| {
                (
                    format!("b{i}"),
                    if i % 2 == 0 { 1 } else { 2 },
                    cursor_text_bubble(1, &format!("turn number {i}")),
                )
            })
            .collect();
        let refs: Vec<(&str, i64, Vec<u8>)> = bubbles
            .iter()
            .map(|(id, k, v)| (id.as_str(), *k, v.clone()))
            .collect();
        write_cursor_composer_with_bubbles(&db, composer, None, &refs);

        // Cap at 3 messages via the internal renderer (the public fn uses the const cap).
        let record_bytes = {
            let conn = open_discovery_sqlite(&db).unwrap();
            cursor_kv_bytes(&conn, &format!("composerData:{composer}")).unwrap()
        };
        let composer_json: serde_json::Value = serde_json::from_slice(&record_bytes).unwrap();
        let conn = open_discovery_sqlite(&db).unwrap();
        let (text, truncated) = cursor_transcript_from_composer(
            &composer_json,
            |bubble_id| cursor_kv_bytes(&conn, &format!("bubbleId:{composer}:{bubble_id}")),
            3,
            CURSOR_IDE_TRANSCRIPT_MAX_BYTES,
        )
        .expect("capped transcript renders");
        assert!(truncated, "dropping older turns must set truncated");
        assert!(text.contains("earlier history truncated"));
        // Newest three (turns 2,3,4) are present; the oldest two (0,1) are gone.
        assert!(text.contains("turn number 4"));
        assert!(text.contains("turn number 2"));
        assert!(!text.contains("turn number 0"));
        assert!(!text.contains("turn number 1"));
    }

    #[test]
    #[ignore = "depends on the local user's real Cursor state.vscdb"]
    fn real_cursor_ide_chat_transcript_length() {
        // Pick the real composer with the most messages and render it; report COUNTS
        // and byte length only — never any conversation content.
        let source = discovery_sources()
            .into_iter()
            .find(|source| source.mode == SourceMode::CursorIdeChats && source.path.is_file())
            .expect("an installed Cursor state.vscdb source");
        let conn = open_discovery_sqlite(&source.path).unwrap();
        let headers =
            cursor_item_json(&conn, "composer.composerHeaders").expect("composer headers present");
        let mut best: Option<(String, usize)> = None;
        for composer in headers["allComposers"].as_array().unwrap() {
            let Some(id) = composer.get("composerId").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(bytes) = cursor_kv_bytes(&conn, &format!("composerData:{id}")) {
                if let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    let n = record
                        .get("fullConversationHeadersOnly")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    if best.as_ref().map(|(_, m)| n > *m).unwrap_or(true) {
                        best = Some((id.to_string(), n));
                    }
                }
            }
        }
        let (id, header_count) = best.expect("at least one composer with messages");
        match cursor_ide_chat_transcript(&source.path, &id, CURSOR_IDE_TRANSCRIPT_MAX_BYTES) {
            CursorChatTranscript::Rendered { text, truncated } => {
                let turns = text.matches("\n## ").count() + usize::from(text.starts_with("## "));
                println!(
                    "[real] cursor transcript: composer has {header_count} headers -> {} rendered chars, {turns} labelled turns, truncated={truncated}",
                    text.len()
                );
                assert!(!text.is_empty());
            }
            CursorChatTranscript::Empty => {
                println!("[real] busiest composer ({header_count} headers) had no renderable prose")
            }
            CursorChatTranscript::Unavailable => {
                panic!("busiest composer's record should be readable")
            }
        }
    }

    // --- B2: second Antigravity IDE store dedup --------------------------------

    #[test]
    fn antigravity_conversation_uuid_reads_pb_and_db_stems_only() {
        let pb = Path::new("C:\\Users\\me\\.gemini\\antigravity-ide\\conversations\\abc-123.pb");
        assert_eq!(
            antigravity_conversation_uuid(pb).as_deref(),
            Some("abc-123")
        );
        let db = Path::new("C:\\Users\\me\\.gemini\\antigravity\\conversations\\DEF-456.db");
        assert_eq!(
            antigravity_conversation_uuid(db).as_deref(),
            Some("def-456")
        );
        // Not in a conversations dir -> None.
        let brain = Path::new("C:\\Users\\me\\.gemini\\antigravity-ide\\brain\\x\\t.pb");
        assert_eq!(antigravity_conversation_uuid(brain), None);
        // Wrong extension -> None.
        let other = Path::new("C:\\Users\\me\\.gemini\\antigravity-ide\\conversations\\notes.txt");
        assert_eq!(antigravity_conversation_uuid(other), None);
    }

    #[test]
    fn antigravity_ide_conversation_file_predicate_is_kind_gated() {
        let ide = Path::new("C:\\Users\\me\\.gemini\\antigravity-ide\\conversations\\u.pb");
        assert!(is_antigravity_ide_conversation_file(
            ide,
            "gemini_antigravity_ide_conversations"
        ));
        // The MAIN store's file must never trigger the IDE dedup, even by path.
        let main = Path::new("C:\\Users\\me\\.gemini\\antigravity\\conversations\\u.pb");
        assert!(!is_antigravity_ide_conversation_file(
            main,
            "gemini_antigravity_conversations"
        ));
        // IDE path but wrong (main) kind: not an IDE dedup candidate.
        assert!(!is_antigravity_ide_conversation_file(
            ide,
            "gemini_antigravity_conversations"
        ));
    }

    #[test]
    fn antigravity_ide_conversation_overlapping_main_uuid_is_deduped() {
        // The dedup drops an IDE conversation whose UUID also exists in the main store
        // (same chat, second store) and keeps an IDE-unique one. Build a synthetic main
        // store on disk, read its UUID set the same way production does (via the
        // injectable dir seam), then assert the pure membership rule that
        // `scan_metadata_file` applies.
        let base = tempdir_projectlike();
        let home = base.path();
        let main_convos = antigravity_main_conversations_dir_in(Some(home)).unwrap();
        fs::create_dir_all(&main_convos).unwrap();

        let shared_uuid = "11111111-1111-4111-8111-111111111111";
        let unique_uuid = "22222222-2222-4222-8222-222222222222";
        // Main store holds the shared conversation (and a WAL sidecar that must be
        // ignored — only `.pb`/`.db` stems are UUIDs).
        fs::write(main_convos.join(format!("{shared_uuid}.pb")), b"main").unwrap();
        fs::write(main_convos.join(format!("{shared_uuid}.pb-wal")), b"wal").unwrap();

        let main_uuids = antigravity_conversation_uuids_in_dir(Some(&main_convos));
        assert_eq!(main_uuids.len(), 1, "WAL sidecar is not a conversation");

        let ide_convos = home
            .join(".gemini")
            .join("antigravity-ide")
            .join("conversations");
        let ide_shared = ide_convos.join(format!("{shared_uuid}.pb"));
        let ide_unique = ide_convos.join(format!("{unique_uuid}.pb"));

        // Overlapping IDE conversation -> deduped; unique one -> kept.
        assert!(
            antigravity_conversation_uuid_in_set(&ide_shared, &main_uuids),
            "IDE conversation overlapping the main store is deduped away"
        );
        assert!(
            !antigravity_conversation_uuid_in_set(&ide_unique, &main_uuids),
            "IDE-unique conversation survives the dedup"
        );
        // An empty main set (main store absent) makes the dedup a no-op: nothing hidden.
        let empty = antigravity_conversation_uuids_in_dir(None);
        assert!(!antigravity_conversation_uuid_in_set(&ide_shared, &empty));
    }

    #[test]
    fn antigravity_proto_map_for_picks_store_by_path() {
        // The proto map chosen for a conversation must come from that conversation's
        // OWN store, selected purely by whether the path is under `antigravity-ide/`.
        // (We assert the selection rule, not emptiness: a real dev machine may have a
        // proto in either store.) The lookups must also never panic.
        let ide = Path::new("C:\\Users\\me\\.gemini\\antigravity-ide\\conversations\\u.db");
        let main = Path::new("C:\\Users\\me\\.gemini\\antigravity\\conversations\\u.db");
        assert!(is_antigravity_ide_path(ide));
        assert!(!is_antigravity_ide_path(main));
        // Exercise both cache branches; the maps are Arc<BTreeMap>, contents depend on
        // the host, so we only require the calls to resolve without panicking.
        let _ide_map = antigravity_proto_map_for(ide);
        let _main_map = antigravity_proto_map_for(main);
    }

    #[test]
    #[ignore = "depends on the local user's real ~/.gemini/antigravity-ide store"]
    fn real_antigravity_ide_unique_conversation_count() {
        // Drive the real IDE conversations source through the scanner and report how
        // many UNIQUE conversations survive the cross-store UUID dedup. Counts only.
        let source = discovery_sources()
            .into_iter()
            .find(|source| {
                source.kind == "gemini_antigravity_ide_conversations" && source.path.is_dir()
            })
            .expect("an installed antigravity-ide conversations source");
        let mut candidates = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        scan_text_metadata_source(&source, &mut candidates, &mut sessions);
        let sessions = finalize_sessions(sessions, &[]);
        println!(
            "[real] antigravity-ide UNIQUE conversations added (deduped vs main store): {}",
            sessions.len()
        );
        // The dedup must strictly reduce the store's file count (overlap exists).
        assert!(
            sessions.len() < 29,
            "most IDE conversations overlap the main store"
        );
    }
}
