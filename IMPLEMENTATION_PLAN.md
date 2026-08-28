# Code Hangar Implementation Plan

## Current status (2026-08-09)

This file records the phased implementation sequence that governed the original
build. It is not the current release scope or a claim that the product still
stops at Phase 1A. The first-run ceiling below was completed and later phases
were subsequently implemented behind their documented feature gates.

Current release readiness, direct test evidence and unresolved blockers are
tracked in [`docs/qa/v0.1.1-acceptance.md`](docs/qa/v0.1.1-acceptance.md). The
implemented post-Gate-3 state is summarized in
[`docs/roadmap_finishers.md`](docs/roadmap_finishers.md). New work must still
preserve the phase boundaries and security invariants described here; do not use
the historical first-run constraint as the current product inventory.

The owner-approved release acceptance contract is
[`docs/DEFINITION_OF_DONE.md`](docs/DEFINITION_OF_DONE.md). The current Safe
Manage product flow is
[`docs/safe-manage-assisted-cleanup.md`](docs/safe-manage-assisted-cleanup.md).
Together they add a read-only, portfolio-level decision layer between Discover
and the existing OperationPlan/Risk Report flow. They do not weaken or remove
permanent removal: final removal remains a primary, backup-gated outcome that is
disabled by default and always separately confirmed.

The product now has two compile-time and packaging-separated editions. Local is
100% local and contains no AI/MCP/provider code or user-facing traces. Connector
adds the optional, default-off and explicitly experimental AI Assist/provider and
local MCP surfaces. Connector-only provider requests are user-selected and
payload-previewed; no other outbound functionality is permitted. See
[`AGENTS.md`](AGENTS.md) and [`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md)
for the binding edition boundaries.

The current product entry point is the Windows cross-project explorer: optional
Markdown `Open with` registration and text-file/folder context menus feed the
existing read-only root resolution, scanner, project navigation and local
AI-app/session correlation. A known file reuses its registered project root; an
unknown file opens directly in a temporary isolated Viewer without registration
or session aggregation. Unknown folders must explicitly choose Viewer, automatic
root detection, or a user-selected containing root before registration/scanning.
Direct file preview is the foreground critical path; it is deliberately independent
of SQLCipher startup, project registration and scan creation. The one explicitly
requested local file is inspected through the bounded no-recall preview policy and
painted in an isolated provisional Viewer first. Inventory attachment, indexing and
correlation run behind that first paint. This is navigation infrastructure, not a new phase gate and not
permission to weaken preview, mutation, agent or network policy.

Cloud-backed roots are first-class project locations, not excluded locations.
The implemented default is cloud-safe: Windows placeholder visibility is enabled
for the process, content opens use no-recall handles, materialized Cloud Files are
indexed, and online-only files remain metadata-only and visibly unindexed. Global
discovery does not traverse broad OneDrive Documents/Desktop roots; explicit
OneDrive discovery is depth-, directory- and time-bounded. The base build never
changes OneDrive pins or hydrates content because that would violate its outbound
network boundary.

## Historical first coding run constraint

The first coding run was required to stop at Phase 1A. It could implement Phase -1, Phase 0 and the beginning of Phase 1 only.

Phase 1A includes SQLite/navigation migrations, fixture loading, navigation IPC, folder picker and a read-only scanner skeleton limited to Markdown/context files.

Git reader, full Windows filesystem correctness, disk accounting, Orphan Finder logic, OperationPlan, Risk Report, mutation, backup/quarantine/restore, watchers, File Lock Inspector and agent IPC/MCP were forbidden in that first run.


This plan is the operational sequence for building Code Hangar.

The key rule is simple: build the local read-only navigator first. Do not build mutation, backup/quarantine or local agent automation until the scanner, navigation index and disk intelligence are trustworthy.

## Phase -1: Repository foundation and guardrails

Goal: create a repository that prevents architectural drift.

Deliverables:

- Initialise the `CodeHangar` monorepo.
- Create Tauri v2 + React + TypeScript shell.
- Create Rust workspace/crates.
- Add feature flags:
  - `core`
  - `mutation`
  - `agent_automation`
- Ensure `mutation` and `agent_automation` are not enabled by default.
- Configure CI:
  - Rust formatting;
  - TypeScript lint;
  - Rust tests;
  - dependency audit;
  - Tauri build smoke test;
  - base-build outbound-network deny gate.
- Create local fixtures.

Definition of done:

The repository compiles, the app opens, CI passes, and there is no mutation, no agent IPC and no outbound network capability in the base build.

## Phase 0: UI shell and Markdown navigation

Goal: validate the user experience using fixture data.

Deliverables:

- Three-pane layout.
- Project Navigator.
- File Tree.
- Context tab.
- Markdown preview.
- Raw/source toggle.
- Opened file tabs.
- Back/forward navigation.
- Breadcrumb.
- Quick open.
- Recent/pinned items.
- Minimal command palette.

Definition of done:

The UI feels like Code Hangar. It can browse fixture projects, open Markdown/context files, preview safely and navigate quickly.

## Phase 1: Read-only inventory core

Goal: read real local folders without changing anything.

Deliverables:

- SQLite schema and migrations.
- Encrypted database scaffold.
- Read-only scanner.
- Folder picker.
- Scan roots.
- Context file detection.
- Sensitive file detection.
- Protected Zones.
- `nav_item`, `document_index`, `document_fts`.
- Safe Markdown preview from real files.
- Quick open from real data.
- Basic dashboard.
- Local Git reader, metadata only.

Definition of done:

The app can scan a real folder, show context files first, render Markdown, detect sensitive/protected files and remain read-only.

## Gate 1: Read-only boundary

Before moving on:

- No destructive UI exists.
- No mutation executor exists.
- No agent IPC server exists.
- No outbound network capability exists.
- Scanner is cancellable.
- Markdown preview is safe.
- Sensitive files are blocked from preview and FTS.
- Protected Zones are respected.
- UI remains responsive during scans.
- CI proves the base build has no outbound-network dependency path.

## Phase 1.5: Disk accounting and relationship intelligence

Goal: make disk numbers and relationships trustworthy before cleanup planning.

When entering this phase, consult `docs/engineering_details_by_phase.md` for the confidence catalogue, owned/shared/orphan definitions, reference-resolution algorithm, duplicate tables, model classification lists and the full adapter example.

Deliverables:

- Physical size accounting.
- Volume ID and inode key.
- Hardlink count.
- Reparse point detection.
- Symlink/junction/mount point handling.
- Owned/linked/shared/protected/orphan classification.
- Reference resolution with confidence.
- First adapters.
- Explain This Folder.
- Orphan Finder skeleton.
- Duplicate candidate detection.

Definition of done:

The app can explain folders, show disk footprint with caveats, identify basic orphans and distinguish owned/shared/referenced/protected assets.

## Gate 2: Accounting correctness

Before OperationPlan or Risk Report:

- hardlinks are counted once;
- deleting one hardlink path does not falsely report recovered space;
- symlinks and junctions are not followed for ownership;
- Protected Zones never contribute to recoverable space;
- shared model assets are not treated as owned by one project;
- missing references are reported;
- ambiguous references are Low confidence;
- Orphan Finder matches fixture expectations;
- stale disk numbers are marked stale.

## Phase 2: OperationPlan and Risk Report preview-only

Goal: show what would happen without allowing anything to happen.

When entering this phase, consult `docs/engineering_details_by_phase.md` for cleanup tier definitions and the OperationPlan field reference. The compact `recursive_dir` design remains mandatory; do not enumerate huge directories file-by-file.

Deliverables:

- OperationPlan builder.
- Risk Report projection.
- Cleanup risk tiers.
- Compact `recursive_dir` item.
- Dry-run traversal for compact items.
- `PlanTooLarge`.
- Stale fingerprint detection.
- Exportable Risk Report.
- No execute button.

Definition of done:

The user can build a plan, see risk and cancel. No operation can be executed.

## Phase 3: Backup, quarantine and restore

Goal: introduce mutation only after map and preview are trustworthy.

When entering this phase, consult `docs/engineering_details_by_phase.md` for backup levels, checksum verification, quarantine/restore mechanics, the destructive-action state machine, journal table schemas, error taxonomy and invariant tests.

Deliverables:

- Mutation feature flag.
- Mutation mode UI.
- Journaled executor.
- Backup.
- Quarantine.
- Restore.
- Crash recovery.
- File Lock Inspector.
- No-follow recursive execution.
- Physical path locks.
- Operation Activity Log.

Definition of done:

Mutations work only through OperationPlan, never affect Protected Zones, never follow reparse points and recover after interruption.

## Phase 4: Watchers and active project awareness

Goal: keep the dashboard honest while projects change.

Deliverables:

- Focused watcher.
- Global low-resolution watcher.
- Dirty/stale badges.
- Dashboard stale warnings.
- Manual rescan affordances.
- Debounce and throttling.
- Optional per-user start-at-login plus close-to-tray residency.
- Bounded quick refreshes plus a one-root-at-a-time periodic rotation through registered roots.
- Encrypted metadata-only created/modified/removed timeline, silently baselined on the first completed scan.

Implemented resident-awareness boundary (2026-08-09): the base app may remain
running in the Windows notification area and reuse the ordinary read-only scan
pipeline. Quick passes rotate metadata probes across at most four roots; actual
resident scans admit one root and one below-normal-priority worker, and a
six-hour safety pass rotates the least-recent root. Repeated inventories are
delta-reconciled and skip unchanged writes/derived rebuilds. It performs no
separate filesystem walk for the timeline, stores no
file bodies in change events and makes no causal claim about which application
or agent changed a file. This belongs to Phase 4 and does not enable Phase 5
local-agent input, MCP, mutation, telemetry, update checks or outbound network.

## Phase 5: Local agent input / MCP

Goal: allow local agents to provide structured input and request scoped local operations without bypassing safety.

Deliverables:

- Agent automation feature flag.
- Local-only IPC.
- Trusted agent registration.
- Agent scopes.
- Agent read gates.
- Agent Activity Log.
- Cascade revoke.
- Deep history search on demand.

## Phase 6: Full graph, models and workflow depth

Goal: full dependency map.

Deliverables:

- Hangar Map.
- Project view.
- Asset view.
- Orphan view.
- Risk view.
- Workflow parsers.
- Model reference resolution.
- Missing reference detection.
- Duplicate model detection.
- Video workflow support.
