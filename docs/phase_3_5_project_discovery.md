# Phase 3.5 Project Discovery

Phase 3.5 adds passive local project and session discovery. It does not replace
explicit scan roots; it proposes root project folders that the user can add to
Code Hangar and separately lists local conversation/session records that may
belong to registered or unregistered projects.

## Scope

- Read-only and local-only.
- No network, telemetry, remote Git or package registry access.
- Conservative automatic import is limited to strong, non-overlapping project
  candidates already named by an app registry or a local session. Technical,
  weak and overlapping candidates always require explicit user review.
- No deep inventory scan during discovery. The normal scanner still owns full
  inventory, protected-zone handling and indexing.
- Sessions are not treated as projects. They are shown separately and classified
  as linked to a registered project, referencing an unregistered project, or
  loose/unlinked.

## Sources

The first implementation checks:

- Known local folders: Documents, Desktop, Downloads, OneDrive Documents,
  `C:\AI`, `source`, `repos`, `projects`, `dev`.
- Claude Chat/Desktop project folder under Documents when present.
- Codex active and archived local sessions, session index, local thread SQLite
  metadata and user skills (technical candidates only).
- Claude Code local projects/sessions, file-history, user skills, commands,
  subagents and Claude desktop local session/log metadata.
- Cursor workspace storage, per-project transcripts, local state database and
  user skill folders (technical candidates only).
- VS Code, VS Code Insiders and VSCodium workspace/global storage, which is
  where many extension-based agents keep workspace-local state.
- Cline, Roo Code, Kilo Code and Continue extension `globalStorage` folders
  under VS Code-like hosts when present. These are read as bounded local
  metadata because they often contain task/conversation path references.
- GitHub Copilot/Copilot Chat and Sourcegraph Cody extension storage under
  VS Code-like hosts when present, including workspace/global metadata that may
  point back to local workspaces.
- Zed local conversation folders when present, including common
  `.config/zed/conversations`, `.local/share/zed/conversations` and Windows
  roaming/local variants.
- Windsurf workspace/global storage and Windsurf document project folders when
  present.
- Antigravity/Gemini local brain, conversation and workspace metadata, plus
  user skill folders (technical candidates only).
- Antigravity conversation databases/protobuf-like state files when present
  (`.db`, `.sqlite`, `.pb`, `.pbtxt`), sampled with bounded local reads.
- Pinokio local app folders under user `pinokio\api` and `C:\pinokio\api`
  when present.
- Continue and Roo Code local state folders when present.
- OpenClaw local state, legacy `sessions.json`/JSONL conversations, the current
  global SQLite registry and per-agent transcript databases, memory and global
  skills such as `~/.openclaw/skills/<name>/SKILL.md`.
- NemoClaw/Hermes local state, sessions, skills, sandboxes and Hermes
  configuration folders when present, including WSL Hermes homes exposed through
  `\\wsl.localhost\<distro>\home\<user>\.hermes` when that local share is
  available. Hermes `state.db` is the canonical conversation source; each
  discovered session opens only its own bounded messages.

## Signals

Project candidates are scored by conservative local signals:

- session metadata path references;
- README or project context files;
- `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`;
- `.git`;
- common manifests such as `package.json`, `pyproject.toml`, `Cargo.toml`,
  `go.mod`;
- local tool markers such as `.cursorrules`, `.cursor/rules`,
  `.clinerules`, `.roo/rules`, `.continue`, `.windsurf/rules`,
  `.devin/rules`, `.windsurfrules`, `.aider.chat.history.md`,
  `.aider.input.history` and `.aider.conf.yml`;
- Pinokio app-folder shape.
- local agent skill/config files such as `SKILL.md`, `openclaw.json`,
  `MEMORY.md` and `SOUL.md`.
- Windows paths saved in session metadata as `C:\...`, `C:/...`,
  `file:///C:/...` or WSL-style `/mnt/c/...`.
- SQLite text columns and bounded text/string samples from binary-ish local
  metadata files.
- structured text reports produced by local agents, including Portuguese-style
  `Caminho absoluto da pasta do projecto` and `pasta-base das conversas` lines,
  while ignoring explicitly excluded file/internal-state lines.
- recent session/directory activity, when the local metadata timestamp is
  recent enough to be meaningful.

Discovery intentionally avoids promoting dependency/cache folders as projects,
even when they contain their own manifests. Examples include `node_modules`,
Cargo registry sources such as `.cargo/registry/src/index.crates.io-*`,
Python site-packages, build folders, model/vendor folders and app cache trees.
Those can still be audited as technical material, but they are classified as
technical candidates and kept behind the UI toggle so they do not pollute the
root project workflow.

## Sessions

Local sessions are displayed in their own section.

- Linked sessions contain a path that falls inside a registered Code Hangar
  project.
- Unregistered-project sessions contain a local project path that has not been
  added yet.
- Loose sessions were found in a known local session store but no project path
  could be linked from bounded metadata.

The session list is searchable by app, path, association and linked project
path. `Find local sessions` includes project-linked, loose and autonomous-agent
sessions; the narrower project-discovery run may omit loose/agent sessions.
Session previews are bounded, read-only and secret-redacted by default. They do
not add projects automatically.

Explicit skill stores are enumerated only when the user enables `Show skills
and technical candidates`. This avoids traversing large WSL skill trees during
normal project discovery while keeping user-created skills available for audit.

## Add Projects

The main toolbar uses `Add Projects` instead of a direct `Add Folder` action.

- `Add Project` keeps the old direct path: the user chooses one folder, and
  Code Hangar registers it as a read-only scan root.
- `Deep Scan` lets the user choose a folder or drive such as `C:\`. Code Hangar
  recursively searches that location for passive project signals. Strong,
  non-overlapping project candidates are registered and scanned automatically;
  weaker, technical and overlapping candidates remain in Discover for review.

## Overlap Handling

Every candidate is compared with existing Code Hangar scan roots.

- Exact matches are marked as already registered.
- Candidates inside a registered root are marked as nested and cannot be added
  again from the discovery list. The normal project list hides these by default.
- Candidates that contain a registered root are marked as parent overlaps and
  cannot be added until the user resolves the overlap deliberately. The normal
  project list hides these by default.

This keeps discovery sensitive without making overlapping roots look like
independent duplicate projects.

## Safety Caps

- Known-folder scans are shallow and capped by directory count.
- Session/metadata scans read only bounded prefixes of a capped number of files.
- Candidate size/file counts are estimates, bounded to depth and item caps.
- Heavy folders such as `.git`, `node_modules`, `.venv`, `target`, `dist`,
  `build` and caches are not traversed during discovery.

## Follow-Up

Later discovery passes should add user-editable source locations, stronger
tool-specific parsers, ranking by recent activity, and a dedicated overlapping
root resolver. Those are still passive review features, not cleanup actions.
