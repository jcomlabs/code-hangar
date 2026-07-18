# Code Hangar Master Specification v20 Final

A local-first desktop control centre for discovering, navigating, inspecting, mapping, backing up and safely cleaning AI-assisted projects on a Windows machine.

Repository: `CodeHangar`

This document is written as the implementation contract for an AI coding tool. It is intentionally strict about phase order. Build the local read-only navigator first. Do not implement mutation or local agent automation until the inspection core, navigation layer and disk accounting are demonstrably correct.


## Companion implementation documents

This master specification is the source of truth for product direction, phase order and safety policy. The handoff also includes implementation documents that constrain how coding agents should proceed:

- `IMPLEMENTATION_PLAN.md` defines the operational phase sequence and gates.
- `PHASE_0_1_TICKETS.md` defines the first safe ticket set.
- `SECURITY_INVARIANTS.md` defines security rules that must not be bypassed.
- `docs/engineering_details_by_phase.md` preserves detailed algorithms, schemas, risk tiers, backup mechanics, error taxonomy and tests for later phases.

Do not implement later-phase detail before the relevant gate is passed.

## 1. Product identity

Code Hangar is a clean, fast project navigator and control centre for AI-assisted workspaces.

The user opens Code Hangar, sees detected AI-related projects, clicks a project, moves through its files immediately, and reads important Markdown and context files without friction. The feel should be closer to Obsidian plus a disk intelligence tool than to an IDE.

Code Hangar is not an IDE, not an agent, not a chat interface, not a GitHub client, not a model marketplace and not a generic disk cleaner. It is a local project map, context browser, dependency lens, archive tool and safety layer.

The primary first impression should be:

> I can finally see all my AI projects, their instructions, histories, models, caches and risks in one place.

The destructive features are important, but they must not dominate the interface. The application starts as a fast navigation and inspection tool. Cleanup comes only after the local map is trustworthy.

## 2. Scope and safety position

Code Hangar is local-only by design.

It does not read online documentation, fetch remote repository previews, call GitHub APIs, call package registries, download adapters, upload telemetry, call provider APIs, send project contents anywhere, or perform remote Git operations.

Remote URLs found in local files, such as Git remotes, package URLs, model IDs or documentation links, are passive metadata. They may be displayed or copied. They are not fetched by Code Hangar.

The only external input path allowed by the product model is local agent input through explicitly enabled local IPC/MCP/API in a later phase. That input is treated as untrusted or scoped unless it comes from a registered trusted local agent. Agent input never overrides safety policy.

The base build must have no outbound network capability. Local IPC for agents must be behind an explicit feature flag and must not exist in the base build.

## 3. Core promise

Code Hangar answers these questions:

1. What AI-assisted projects exist on this computer?
2. Which tools have touched each project?
3. Which Markdown/context files explain each project?
4. Which histories, memories, skills, workflows, models, caches and generated outputs are linked to each project?
5. How much disk space does each project truly occupy?
6. What is directly owned by a project, and what is only shared or referenced?
7. What is already orphaned and apparently unused?
8. What can be cleaned with low risk?
9. What should be backed up before deletion?
10. What would be left dangling if a model, workflow, cache, history or project folder were removed?
11. Which local files are sensitive and should not be indexed, archived, previewed or deleted casually?
12. Which relationships are inferred and with what confidence?

The central promise is not deletion. The central promise is confidence before deletion.

## 4. Core user experience

Code Hangar opens into a three-pane interface.

Left pane: Project Navigator.

It shows all detected projects, pinned projects, recent projects, Orphan Finder, adapters needing review, Protected Zones, global search and quick filters.

Useful quick filters include: Has README, Has AGENTS.md, Has Claude history, Has ChatGPT history, Has Cursor/Cline history, Large outputs, Uncommitted Git, Orphans, Sensitive files, Needs backup, Stale size.

Centre pane: File and Context Viewer.

It shows a file tree, Markdown preview by default, raw/source toggle, opened file tabs, back/forward navigation, breadcrumb path, search within file and context-file priority.

Right pane: Inspector.

It shows file metadata, project associations, tool associations, Git status, disk footprint, model/workflow references, sensitive-file warning, Protected Zone status, cleanup risk, backup/quarantine state and confidence level.

Status area.

It shows scan status, job progress, index status, mutation status, stale/dirty indicators and base build network status.

## 5. Navigation principles

Navigation should feel like Obsidian:

- click project, see its file tree immediately;
- click Markdown file, preview appears immediately;
- internal Markdown links are clickable when they resolve locally;
- backlinks are shown when other local files reference the current file;
- quick switcher opens projects and files by fuzzy search;
- command palette provides actions;
- recent items are remembered;
- projects and files can be pinned;
- keyboard-first navigation is supported.

Required shortcuts:

- Ctrl+P: quick open project or file;
- Ctrl+Shift+F: global search;
- Ctrl+K: command palette;
- Alt+Left / Alt+Right: navigation history;
- Ctrl+1 / Ctrl+2 / Ctrl+3: focus left, centre, right pane;
- Ctrl+R: rescan current project;
- Ctrl+E: explain current folder or file.

## 6. Markdown and context-file priority

High-priority context files include:

- `README.md`
- `AGENTS.md`
- `CLAUDE.md`
- `GEMINI.md`
- `.cursorrules`
- `.cursor/rules/*`
- `.clinerules`
- `.aider.conf.yml`
- `CONTRIBUTING.md`
- `docs/**/*.md`
- `prompts/**/*.md`
- `.env.example`
- `docker-compose.yml`
- `Makefile`
- `Taskfile.yml`
- `justfile`
- `package.json`
- `pyproject.toml`
- `requirements.txt`
- `Cargo.toml`
- `go.mod`

The project overview must surface these files before ordinary files. Default project landing tab: Context, not Cleanup.

Markdown preview rules:

- HTML is sanitised;
- scripts are not executed;
- remote images are not fetched;
- remote links are inert and never auto-opened;
- local relative images may be displayed only if inside allowed scan roots and not inside stricter Protected Zones;
- sensitive-file contents are not rendered by default.

Performance targets:

- cached Markdown preview: under 100 ms;
- cold small Markdown preview: under 300 ms.

## 7. Design principles

Read-only first.

Code Hangar boots in inspection mode. It can scan, classify, map, search and report without changing anything outside its own data directory.

Read the project before touching the project.

The first job is to let the user understand the project: Markdown context, instructions, histories, Git state and dependencies. Cleanup is a later consequence of understanding.

Heuristic honesty.

Every relationship carries a confidence level. Medium or Low confidence associations must never be phrased as fact.

Orphans before owners.

The safest cleanup surface is not "delete this project". It is "show me what nothing references any more". Orphan Finder is part of the MVP.

Disk numbers must be credible.

Code Hangar must handle symlinks, hardlinks, junctions, duplicate files, shared model folders, sparse files, compressed files and cross-volume operations.

The index is sensitive.

The database may contain project names, paths, histories, prompts, model names, workflow references and sensitive topology. It is encrypted by default.

Adapters are data, not code.

Adapters are declarative. They may describe paths, patterns and parsers. They may not execute scripts, call shell commands, perform network calls or mutate files.

Protected Zones override everything.

Files inside Protected Zones are never mutated. Content indexing and preview obey the protection level.

Confidence before deletion.

The UI should feel like it is preventing mistakes, not merely asking for permission after deciding what to destroy.

## 8. Operating phases

The phase order is mandatory.

### Phase -1: Repository foundation and guardrails

Goal: create a repository that prevents architectural drift.

Deliverables:

- repository skeleton for `CodeHangar`;
- Tauri v2 + React + TypeScript + Rust workspace;
- crate layout;
- CI for format, lint, tests, dependency audit and Tauri build smoke test;
- feature flags: `core`, `mutation`, `agent_automation`;
- base build with no mutation executor, no agent IPC server and no outbound network capability;
- local fixtures for UI and scanner.

Definition of done:

The app opens as an empty shell. CI passes. The base binary cannot perform outbound network operations and contains no mutation or agent automation code path.

### Phase 0: UI shell and Markdown navigation

Goal: validate the feel before heavy backend features.

Deliverables:

- three-pane layout;
- fixture-backed Project Navigator;
- fixture-backed file tree;
- Context tab;
- Markdown preview;
- source/rendered toggle;
- opened file tabs;
- back/forward navigation;
- breadcrumbs;
- quick open;
- recent items;
- pinned projects/files;
- minimal command palette.

Definition of done:

The UI already feels like Code Hangar with fixture data. It can open fictitious projects, browse files and read context Markdown safely.

### Phase 1: Read-only inventory core

Goal: detect and display real local projects without changing anything on disk.

Deliverables:

- SQLite schema and migrations;
- encrypted database scaffold;
- `node`, `edge`, `nav_item`, `document_index`, `document_fts`, `recent_item`, `pinned_item`;
- scan root configuration;
- Windows known-folder discovery;
- read-only scanner;
- Protected Zones;
- Git reader, local metadata only;
- context file reader;
- Markdown/document indexing;
- Quick Open from real data;
- basic dashboard.

Definition of done:

The app discovers real projects, displays context files first, renders Markdown, detects Protected Zones and shows basic project metadata. It still cannot mutate anything.

### Phase 1.5: Disk accounting and relationship intelligence

Goal: make Code Hangar trustworthy as a disk and dependency intelligence tool before any OperationPlan, Risk Report or mutation layer exists.

Deliverables:

- apparent size, allocated size, volume ID, inode key, hardlink count;
- sparse/compressed file awareness where available;
- symlink, junction, mount point and reparse point detection;
- no double-counting of linked targets;
- ownership classification: owned, linked, shared, orphaned, protected, unknown;
- reference resolution with confidence;
- initial adapters: generic code/Git project, generic Markdown/context project, one workflow/model fixture adapter;
- disk footprint per project: direct, linked, shared, protected, likely recoverable, stale/unknown;
- duplicate candidates by size and partial hash;
- Explain This Folder with real data;
- Orphan Finder skeleton.

Definition of done:

The app can explain folders, show disk footprint with caveats, identify basic orphans and distinguish owned/shared/referenced/protected assets.

### Phase 2: OperationPlan and Risk Report preview-only

Goal: show what would happen without allowing anything to happen.

Deliverables:

- `OperationPlan` builder;
- Risk Report projection from the same `OperationPlan`;
- cleanup risk tiers;
- compact `recursive_dir` plan item for huge directories;
- mandatory dry-run traversal for compact plan items;
- `PlanTooLarge` safeguards;
- stale fingerprint detection;
- exportable Risk Report;
- no execute button.

Definition of done:

The user can build a plan, inspect risk, see recoverable space, see sensitive/protected warnings and cancel. No operation can be executed.

### Phase 3: Backup, quarantine and restore

Goal: introduce mutation only after the map and plan are trustworthy.

Deliverables:

- `mutation` feature flag;
- mutation mode UI;
- journaled operation executor;
- backup: minimal, standard, full;
- checksum verification;
- same-volume quarantine;
- cross-volume quarantine by verified copy/delete;
- restore with conflict handling;
- crash recovery;
- File Lock Inspector;
- safe recursive deletion;
- physical path locks based on OperationPlan paths/inodes;
- Operation Activity Log.

Definition of done:

Backup, quarantine, restore and crash recovery work on fixtures. The executor never mutates outside the plan, never follows reparse points and never mutates Protected Zones.

### Phase 4: Watchers and active project awareness

Goal: make the dashboard truthful while projects change in the background.

Deliverables:

- focused watcher for the open project;
- Markdown/context preview refresh for the current project;
- global low-resolution watcher for known project roots;
- dirty/stale badges;
- dashboard stale warnings;
- manual rescan affordances;
- watcher debounce and throttling.

Definition of done:

The user can see when another project changed significantly in the background without the app reindexing the whole machine continuously.

### Phase 5: Local agent input, MCP/API

Goal: allow local agents to feed Code Hangar structured events and request local operations without bypassing the safety model.

Deliverables:

- `agent_automation` feature flag;
- local-only IPC: named pipes or loopback only, never external listener;
- trusted agent registration;
- agent scopes;
- agent read gates;
- `agent_project_context`;
- `agent_plan_build`;
- `agent_plan_execute`;
- Agent Activity Log;
- cascade revoke;
- deep history search on demand.

Definition of done:

Agents can contribute useful local context and execute allowed local operations, but cannot read arbitrary file bodies, mutate outside scope or bypass OperationPlan, locks, journal, Protected Zones or audit.

### Phase 6: Full graph, models and workflow depth

Goal: turn Code Hangar into a real dependency map for AI projects.

Deliverables:

- full Hangar Map;
- project view;
- asset view;
- orphan view;
- risk view;
- workflow parsers;
- model reference resolution;
- missing reference detection;
- duplicate model detection;
- video workflow support;
- shared cache attribution;
- model dependency warnings.

Definition of done:

The user can click a model, workflow, cache or project and understand what uses it, what it owns, what is shared and what would be left dangling if removed.

## 9. Architecture and crate layout

Recommended stack:

- Desktop shell: Tauri v2;
- Frontend: React + TypeScript;
- Backend: Rust;
- Storage: SQLite, SQLCipher-capable build;
- Search: SQLite FTS5 for Markdown/context files only;
- Graph UI: Cytoscape.js or React Flow;
- Hashing: blake3 for confirmed hashes, xxh3 for partial grouping;
- Walking: jwalk or walkdir with bounded parallelism;
- Watching: notify, after the read-only core is stable.

Recommended repository structure:

```text
CodeHangar/
  README.md
  AGENTS.md
  SECURITY_INVARIANTS.md
  IMPLEMENTATION_PLAN.md
  PHASE_0_1_TICKETS.md
  apps/
    desktop/
  crates/
    hangar-core/
    hangar-db/
    hangar-fs/
    hangar-nav/
    hangar-preview/
    hangar-accounting/
    hangar-adapters/
    hangar-graph/
    hangar-resolve/
    hangar-plan/
    hangar-mutation/
    hangar-protect/
    hangar-security/
    hangar-jobs/
    hangar-api/
    hangar-test-fixtures/
  docs/
    architecture.md
    ui_principles.md
    fixtures.md
```

Base crates allowed in Phase -1/0/1:

- `hangar-core`
- `hangar-db`
- `hangar-fs`
- `hangar-nav`
- `hangar-preview`
- `hangar-protect`
- `hangar-security`
- `hangar-jobs`
- `hangar-api`
- `hangar-test-fixtures`

Crates forbidden before later phases:

- `hangar-plan` before Phase 2;
- `hangar-mutation` before Phase 3;
- agent IPC/API before Phase 5.

## 10. Binding invariants

These must hold across every module and code path.

1. The base build is local-only and has no outbound network capability.
2. The base build has no agent IPC server.
3. The base build has no mutation executor.
4. Read-only inspection cannot damage user data.
5. Markdown preview sanitises HTML, executes no scripts, fetches no remote content and blocks sensitive files.
6. Protected Zones override scan, preview, FTS, accounting and mutation.
7. Adapters are declarative and cannot execute code.
8. Every detected relationship has confidence.
9. Medium or Low confidence relationships are not phrased as fact.
10. Disk accounting is physical and link-aware.
11. Same-volume quarantine is not space recovery.
12. Preview equals action for every destructive operation.
13. Every destructive operation is journaled before execution.
14. Restore never overwrites silently.
15. Agent automation is feature-gated and local-only.
16. Trusted agents cannot bypass OperationPlan, scopes, Protected Zones, locks, journal or audit.
17. No remote Git operation exists in the product.

## 11. Build invariants and CI

CI must fail the base build if it detects:

- outbound HTTP client;
- DNS client;
- TLS client;
- telemetry;
- updater;
- remote Git operation;
- package registry client;
- external documentation fetcher;
- any dependency whose only plausible purpose is external network communication in the base build.

Recommended checks:

- `cargo fmt --check`;
- `cargo clippy --all-targets -- -D warnings`;
- Rust tests;
- TypeScript lint;
- Tauri build smoke test;
- dependency audit;
- `cargo-deny`;
- custom dependency denylist for `reqwest`, `hyper`, `rustls`, `native-tls`, `openssl` when used for outbound clients, DNS clients, telemetry and auto-updaters in base build.

Local IPC is allowed only behind `agent_automation`.

Mutation is allowed only behind `mutation`.

## 12. Database schema

The graph lives in `node` and `edge`. Navigation uses dedicated tables for speed.

Minimum schema:

```sql
CREATE TABLE schema_migration (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE setting (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE scan_root (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_scanned_at TEXT
);

CREATE TABLE node (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  path TEXT,
  name TEXT,
  adapter_id INTEGER,
  classification TEXT,
  class_confidence TEXT,
  volume_id TEXT,
  inode_key TEXT,
  link_count INTEGER,
  is_reparse INTEGER NOT NULL DEFAULT 0,
  reparse_kind TEXT,
  link_target TEXT,
  size_apparent INTEGER,
  size_allocated INTEGER,
  mtime TEXT,
  ctime TEXT,
  hash_partial TEXT,
  hash_full TEXT,
  protected_level TEXT,
  fingerprint TEXT,
  attributes TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  present INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_node_kind ON node(kind);
CREATE INDEX idx_node_path ON node(path);
CREATE INDEX idx_node_inode ON node(volume_id, inode_key);
CREATE INDEX idx_node_present ON node(present);

CREATE TABLE edge (
  id INTEGER PRIMARY KEY,
  src INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  dst INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  confidence TEXT NOT NULL,
  adapter_id INTEGER,
  evidence TEXT,
  UNIQUE(src, dst, kind)
);

CREATE INDEX idx_edge_src ON edge(src, kind);
CREATE INDEX idx_edge_dst ON edge(dst, kind);

CREATE TABLE git_repo (
  node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  current_branch TEXT,
  origin_url TEXT,
  last_commit_at TEXT,
  last_commit_hash TEXT,
  uncommitted INTEGER NOT NULL DEFAULT 0,
  untracked_count INTEGER NOT NULL DEFAULT 0,
  is_worktree INTEGER NOT NULL DEFAULT 0,
  main_worktree_id INTEGER REFERENCES node(id)
);

CREATE TABLE protected_zone (
  id INTEGER PRIMARY KEY,
  pattern_type TEXT NOT NULL,
  pattern TEXT NOT NULL,
  level TEXT NOT NULL,
  source TEXT NOT NULL
);

CREATE TABLE adapter (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  type TEXT NOT NULL,
  platforms TEXT NOT NULL,
  verified_versions TEXT,
  definition_json TEXT NOT NULL,
  source TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  trust_ack_at TEXT,
  schema_version INTEGER NOT NULL,
  UNIQUE(name, version, source)
);

CREATE TABLE nav_item (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  node_id INTEGER REFERENCES node(id) ON DELETE CASCADE,
  parent_nav_id INTEGER REFERENCES nav_item(id),
  path TEXT NOT NULL,
  display_name TEXT NOT NULL,
  item_kind TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  sort_key TEXT NOT NULL,
  is_context INTEGER NOT NULL DEFAULT 0,
  is_markdown INTEGER NOT NULL DEFAULT 0,
  is_sensitive INTEGER NOT NULL DEFAULT 0,
  protected_level TEXT,
  last_opened_at TEXT,
  pinned INTEGER NOT NULL DEFAULT 0,
  UNIQUE(project_id, path)
);

CREATE INDEX idx_nav_project_parent ON nav_item(project_id, parent_nav_id, priority, sort_key);
CREATE INDEX idx_nav_context ON nav_item(project_id, is_context, priority);

CREATE TABLE document_index (
  node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  title TEXT,
  headings_json TEXT,
  links_json TEXT,
  backlinks_dirty INTEGER NOT NULL DEFAULT 1,
  preview_cache_key TEXT,
  preview_safe INTEGER NOT NULL DEFAULT 1,
  preview_blocked_reason TEXT,
  language TEXT,
  text_size INTEGER,
  indexed_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE document_fts USING fts5(
  node_id UNINDEXED,
  project_id UNINDEXED,
  path,
  title,
  headings,
  body
);

CREATE TABLE recent_item (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  item_kind TEXT NOT NULL,
  opened_at TEXT NOT NULL
);

CREATE TABLE pinned_item (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  project_id INTEGER REFERENCES node(id),
  item_kind TEXT NOT NULL,
  pinned_at TEXT NOT NULL,
  UNIQUE(node_id, item_kind)
);

CREATE TABLE scan_cache (
  path TEXT PRIMARY KEY,
  mtime TEXT NOT NULL,
  size INTEGER NOT NULL,
  dir_signature TEXT,
  node_id INTEGER REFERENCES node(id)
);
```

Mutation and agent tables must not be created or used until their phases.

## 13. IPC commands by phase

Phase 0/1 allowed commands:

```text
projects_list(filter?) -> ProjectSummary[]
project_get(node_id) -> ProjectDetail
project_nav_tree(project_id) -> NavTree
project_context_files(project_id) -> ContextFile[]
file_preview(node_id, mode:"rendered"|"source") -> FilePreview
file_preview_by_path(path, mode) -> FilePreview
quick_open(query, filters?) -> QuickOpenResult[]
search_documents(query, filters?) -> DocumentHit[]
markdown_links(node_id) -> Link[]
markdown_backlinks(node_id) -> Backlink[]
recent_items_list(limit?) -> RecentItem[]
pinned_items_list() -> PinnedItem[]
pin_item(node_id, item_kind) -> void
unpin_item(node_id, item_kind) -> void
roots_list() -> ScanRoot[]
roots_add(path) -> ScanRoot
roots_remove(id) -> void
scan_start(root_ids?, mode:"full"|"incremental") -> job_id
scan_cancel(job_id) -> void
scan_status(job_id) -> ScanStatus
zones_list() -> ProtectedZone[]
security_status() -> SecurityStatus
```

Forbidden in Phase 0/1:

- `plan_build`;
- `plan_execute`;
- `backup_run`;
- `quarantine_*`;
- `trusted_agent_*`;
- `agent_*`;
- remote fetch/preview/update commands.

Phase 1.5 adds:

```text
explain_folder(path) -> FolderExplanation
footprint_get(node_id) -> Footprint
duplicates_list(min_size?) -> DuplicateGroup[]
graph_orphans(filter?) -> GraphData
```

Phase 2 adds preview-only:

```text
plan_build(target_node_id, action) -> OperationPlan
risk_report(plan_id) -> RiskReport
```

Phase 3 adds mutation behind feature flag.

Phase 5 adds local agent commands behind feature flag.

## 14. Adapter engine

Adapters are JSON documents validated against a fixed schema. Built-in parsers are the only executable behaviour, selected by name.

Allowed parser registry:

- `json_field_map`
- `toml_field_map`
- `yaml_field_map`
- `sqlite_query`, allowlisted SELECT only, no writes
- `git_metadata`, local only and never network-capable Git subcommands
- `gguf_header`
- `safetensors_header`, header bytes only
- `dotenv_signature`, detect by name and shape, never read values
- `markdown_outline`
- `markdown_links`
- `package_json_summary`
- `pyproject_summary`
- `requirements_summary`
- `workflow_json_model_refs`

Adapters may not run scripts, shell commands, network calls, post-scan hooks, auto-delete logic or hidden mutation.

## 15. Scanning pipeline

`scan_start` runs as a cancellable job.

Phases:

1. Resolve roots.
2. Walk filesystem read-only.
3. Collect path, apparent size, allocated size where available, mtime, ctime, volume id, inode key, hardlink count, reparse flag and reparse kind.
4. Skip Invisible Protected Zones.
5. Incremental short-circuit with `scan_cache`.
6. For critical context files, do not rely only on mtime/size. Compute blake3 on scan.
7. Classify through adapters.
8. Build `nav_item`.
9. Parse Markdown/context documents.
10. Populate `document_index` and `document_fts` only for allowed non-sensitive Markdown/context files.
11. Parse local Git metadata.
12. Resolve local references.
13. Persist and reconcile.
14. Mark vanished roots as re-anchor candidates.

The scanner never writes outside Code Hangar's own data directory.

## 16. Windows filesystem specifics

Windows support is mandatory for 1.0.

Requirements:

- long paths: internally support `\\?\`;
- detect reparse points via Windows metadata;
- distinguish symlink, junction and mount point where possible;
- track visited `(volume_id, inode_key)` to prevent loops;
- collect volume id and file index;
- collect hardlink count;
- collect allocated size where possible;
- handle `ERROR_SHARING_VIOLATION` without failing scan;
- handle `ERROR_ACCESS_DENIED` without failing scan;
- surface locked/inaccessible files as flagged nodes.

## 17. Disk accounting

Physical bytes:

```text
physical_bytes(node) = size_allocated if available, else size_apparent
```

Hardlink rule:

A hardlink group is counted once. Deleting one visible path frees 0 if other hardlinks remain. If link_count exceeds visible links, other links exist outside scanned roots and deletion is conservatively reported as freeing 0.

Symlink/junction rule:

Symlinks and junctions contribute only the reparse point. Targets are accounted at their real location, never through the link.

Recoverable space:

```text
recoverable_bytes = physical_sum(owned_nodes union orphaned_on_removal)
```

It is not:

```text
project folder size + all linked models + all shared caches
```

Protected nodes are excluded from recoverable figures.

## 18. OperationPlan and compact recursive_dir

An `OperationPlan` is the single serialised source of truth for preview and execution.

For very large directories, a plan may contain a compact `recursive_dir` item instead of listing every file.

Mandatory dry-run for `recursive_dir`:

Before a compact plan can be marked REVIEWED, the builder must run a read-only traversal of the target directory and store an aggregate summary in `plan_json`:

- estimated item count;
- total bytes;
- recoverable bytes;
- sensitive file hits;
- Protected Zone hits;
- Git/worktree warnings;
- reparse point hits;
- hardlink caveats;
- locked/inaccessible item counts;
- stale fingerprint state;
- confidence caveats.

The dry-run may not serialise all paths. It must produce enough summary data for the Risk Report to be honest.

If the item count or serialised plan exceeds configured limits, return `PlanTooLarge` and require parent path aggregation or batching.

Execution of `recursive_dir`, when mutation exists, must be no-follow:

- never follow symlinks;
- never follow junctions;
- never follow mount points;
- move/delete only the link itself;
- execute in journaled chunks;
- abort chunk on concurrent parent directory drift.

## 19. Risk Report

Risk Report is generated from the same OperationPlan that would execute.

It shows:

- risk level;
- highest-risk factor;
- actually recoverable space;
- direct footprint;
- linked footprint;
- shared dependencies;
- orphaned-on-removal;
- confidence summary;
- sensitive files;
- Protected Zones;
- Git/worktree warnings;
- backup status;
- quarantine option;
- references left dangling;
- recommended action.

Phase 2 must not include execution. The Risk Report has no execute button until Phase 3 exists.

## 20. Local agents and MCP, later phase

Local agents are not part of Phase 0, Phase 1, Phase 1.5, Phase 2 or Phase 3.

When eventually enabled:

- local IPC only;
- no external listener;
- agents have explicit scopes;
- guest agents receive structure-only responses;
- file body reads require `read_body` scope or temporary UI grant;
- agents cannot bypass OperationPlan;
- agents cannot mutate outside scope;
- agents cannot mutate Protected Zones;
- agent revocation kills active read grants, pending plans and held locks;
- all agent activity is audited.

## 21. Performance targets

Pass/fail targets:

- app shell visible: under 1 second after launch;
- project list from warm cache: under 100 ms;
- project tree from warm cache: under 150 ms;
- cached Markdown preview: under 100 ms;
- cold small Markdown preview: under 300 ms;
- quick open results: under 100 ms;
- Markdown/context search: under 300 ms for normal workspaces;
- UI remains responsive during scans;
- full scan of a large workspace is measured in minutes, not hours;
- incremental rescan is measured in seconds when dominated by stat calls;
- memory use remains bounded during scan, plan build and preview.

## 22. Acceptance criteria

A serious release candidate must prove:

1. App opens quickly.
2. UI uses three panes.
3. Projects are discovered from real local folders.
4. Context files appear before ordinary files.
5. Markdown renders safely.
6. Sensitive files are blocked from preview and FTS.
7. Protected Zones are respected.
8. Quick open works from real indexed data.
9. Back/forward navigation works.
10. Recent and pinned files work.
11. Full-text search works for Markdown/context files.
12. Agent histories are metadata-only by default.
13. Deep history search is on demand and adapter-structured.
14. Disk accounting does not double-count hardlinks.
15. Symlinks and junctions are not followed during destructive execution.
16. Same-volume quarantine reports zero recovered bytes.
17. Cross-volume quarantine verifies before deleting source.
18. Restore is byte-exact.
19. Restore never overwrites silently.
20. Orphan Finder matches fixture expectations.
21. Worktrees are detected and protected from raw deletion.
22. Risk Report is generated from the same OperationPlan that would execute.
23. Compact `recursive_dir` plans include dry-run summaries.
24. OperationPlan size limits prevent OOM.
25. Protected Zones never mutate.
26. File Lock Inspector reports blocking processes where possible.
27. Revoking an agent kills active grants and pending plans.
28. Agent scopes prevent cross-project mutation.
29. Agent read gates block file bodies unless authorised.
30. Global low-resolution watcher marks stale projects.
31. Focused watcher refreshes the current project.
32. Crash recovery resumes or rolls back safely.
33. Build gates prove no outbound network in the base build.
34. The base build contains no agent IPC server.
35. The base build contains no mutation executor.

## 23. Practical first sprint

The first sprint must not touch mutation, agents, backup, quarantine or OperationPlan.

Build only:

1. repository skeleton;
2. Tauri shell;
3. three-pane UI;
4. fixture-backed Project Navigator;
5. fixture-backed file tree;
6. Markdown preview;
7. source/rendered toggle;
8. quick open;
9. recent/pinned files;
10. SQLite migrations for navigation tables;
11. real local folder picker;
12. basic read-only scanner for Markdown/context files.

The first sprint succeeds when the app can open a local folder, show its context files first, render Markdown safely and move around the project smoothly.
