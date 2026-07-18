# Phase -1, Phase 0 and Phase 1A Tickets

These tickets define the early implementation boundary. The first coding pass must stop at Phase 1A.

Do not implement mutation, backup, quarantine, restore, OperationPlan, Risk Report, local agent IPC or MCP in this ticket set.

## Strict first-run boundary

The first coding run is not allowed to complete all of Phase 1. It is limited to Phase -1, Phase 0 and Phase 1A.

Phase 1A is the small bridge from fixture data to the earliest real local data:

- navigation SQLite migrations;
- fixture loader into SQLite;
- IPC for navigation/preview;
- folder picker;
- read-only scanner skeleton for Markdown/context files only;
- basic Protected Zone markers for preview/index exclusion.

Everything else in Phase 1 remains future work. Git reader, full Windows filesystem correctness, disk accounting, Orphan Finder logic, model/workflow adapters, OperationPlan, Risk Report, mutation, backup/quarantine/restore, watchers, File Lock Inspector and agents/MCP are explicitly out of scope for the first run.


## Later-phase reference

`docs/engineering_details_by_phase.md` is intentionally included in the handoff, but it is not part of the Phase -1/0/1 ticket scope. Do not implement OperationPlan, backup, quarantine, mutation, model-depth logic or agent automation from that file during this ticket set.

## Epic 0: Repository guardrails

Goal: create a base that prevents architectural drift.

### Tickets

1. Create monorepo structure.
2. Initialise Tauri v2 + React + TypeScript.
3. Create Rust workspace.
4. Create empty crates:
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
5. Create feature flags:
   - `core`
   - `mutation`
   - `agent_automation`
6. Ensure `mutation` and `agent_automation` are not default.
7. Configure Rust format/lint/test.
8. Configure frontend lint/test.
9. Configure dependency audit.
10. Add outbound-network deny check for base build.
11. Add local fixtures.

### Definition of Done

The repository compiles. The app opens. No scanner real yet. No mutation. No agent IPC. No outbound network.

## Epic 1: UI shell

Goal: validate the application feel.

### Tickets

1. Implement three-pane layout.
2. Add left Project Navigator with fixture data.
3. Add centre File and Context Viewer.
4. Add right Inspector.
5. Add file tree with fixture data.
6. Add Context tab.
7. Add Markdown preview panel.
8. Add source/rendered toggle.
9. Add opened file tabs.
10. Add back/forward navigation.
11. Add recent items.
12. Add pinned projects/files.
13. Add quick open with fixture data.
14. Add minimal command palette.

### Definition of Done

The app looks and feels like Code Hangar with fixture data. It can open fixture projects, navigate files, render `README.md`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md` and toggle source/rendered view.

## Epic 2: Safe Markdown and context preview

Goal: secure reading before real files are connected.

### Tickets

1. Sanitise Markdown HTML.
2. Block scripts.
3. Block remote images.
4. Never auto-open links.
5. Allow local relative images only inside fixture/project root.
6. Add `PreviewBlocked` state.
7. Add tests for malicious Markdown.
8. Add tests for remote links/images.
9. Add cached preview performance target.
10. Add cold small Markdown performance target.

### Definition of Done

Preview does not execute scripts, does not fetch remote content and does not reveal blocked sensitive content.

## Epic 3: SQLite and navigation index

Goal: move from hardcoded fixtures to database-backed fixtures.

### Tickets

1. Add SQLite migration runner.
2. Add tables:
   - `node`
   - `edge`
   - `scan_root`
   - `protected_zone`
   - `adapter`
   - `nav_item`
   - `document_index`
   - `document_fts`
   - `recent_item`
   - `pinned_item`
   - `scan_cache`
3. Add Rust DB access layer.
4. Add fixture loader.
5. Add IPC commands:
   - `projects_list`
   - `project_get`
   - `project_nav_tree`
   - `project_context_files`
   - `file_preview`
   - `quick_open`
   - `search_documents`
   - `recent_items_list`
   - `pinned_items_list`
6. Connect UI to IPC instead of hardcoded state.

### Definition of Done

The UI is backed by local database fixture data. It remains read-only.

## Epic 4: Read-only scanner MVP

Goal: scan a real local folder without changing anything.

### Tickets

1. Implement folder picker.
2. Implement scan root management.
3. Implement read-only walker.
4. Detect Markdown/context files.
5. Detect sensitive files by name/pattern.
6. Create `nav_item` from real disk.
7. Create `document_index`.
8. Populate `document_fts` only for non-sensitive Markdown/context files.
9. Implement scan cancellation.
10. Emit scan progress events.
11. Keep UI responsive during scan.
12. Add tests with fixture directories.

### Definition of Done

The user selects a local folder, the app finds context files, displays them first, previews Markdown and supports quick open. There is still no Git reader, accounting, mutation or agent IPC.


---

# Phase 1B reference only, not part of the first coding run

The following epics are intentionally retained here as the immediate continuation after Phase 1A, but they are **not** part of the first coding run.

A coding agent working from `FIRST_AGENT_PROMPT.md` must stop before these epics. It may read them for context only. It must not implement them, create modules solely for them, or add hidden scaffolding for them beyond the empty crate boundaries already authorised in Epic 0.

## Phase 1B Epic 5: Windows filesystem correctness

**Status:** Reference only for the next coding run. Do not implement during the first run.

Goal: prevent the scanner from lying or crashing on Windows.

### Tickets

1. Support long paths internally.
2. Detect reparse points.
3. Detect symlinks.
4. Detect junctions.
5. Detect mount points where possible.
6. Prevent traversal loops.
7. Capture volume ID.
8. Capture inode/file index.
9. Capture hardlink count.
10. Capture apparent size.
11. Capture allocated size where possible.
12. Handle sharing violations.
13. Handle access denied.
14. Flag locked/inaccessible files.

### Definition of Done

The scanner handles long paths, locked files, access denied files and reparse points without crashing or looping.

## Phase 1B Epic 6: Local Git reader

**Status:** Reference only for the next coding run. Do not implement during the first run.

Goal: inspect local Git metadata without network.

### Tickets

1. Detect `.git`.
2. Read `.git/config`.
3. Show current branch.
4. Detect local branches where feasible.
5. Detect worktree state.
6. Show origin URL as passive text.
7. Detect uncommitted/untracked counts.
8. Detect submodules as metadata.
9. Add tests with local fixture repos.
10. Ensure no Git fetch/pull/push/clone command exists.

### Definition of Done

The app shows local Git metadata and never performs remote Git operations.

## Phase 1B Epic 7: Protected Zones baseline

**Status:** Reference only for the next coding run. Do not implement during the first run.

Goal: apply safety boundaries from the beginning.

### Tickets

1. Add default Protected Zones:
   - Code Hangar app data dir;
   - future quarantine dir;
   - future backups dir;
   - SSH folders;
   - credential folders;
   - OS folders.
2. Apply Protected Zones to scan.
3. Apply Protected Zones to preview.
4. Apply Protected Zones to FTS.
5. Show Protected Zone status in Inspector.
6. Add tests for blocked preview and blocked indexing.

### Definition of Done

Protected files are not indexed or previewed beyond their policy.

## Phase 1B Epic 8: First real dashboard

**Status:** Reference only for the next coding run. Do not implement during the first run.

Goal: show immediate value.

### Tickets

1. Total detected projects.
2. Projects with context files.
3. Projects with Git.
4. Projects with sensitive files.
5. Largest folders, approximate.
6. Current scan status.
7. Stale/dirty placeholder.
8. Adapters needing review placeholder.

### Definition of Done

The dashboard answers: what exists on this machine?
