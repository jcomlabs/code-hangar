# Architecture Notes

## Stack

- Tauri v2
- React
- TypeScript
- Rust
- SQLite
- SQLite FTS5 for Markdown/context files only

## Layers

1. Windows shell ingress and UI shell
2. Navigation and preview
3. Read-only inspection core
4. Disk accounting
5. OperationPlan/Risk Report preview
6. Mutation
7. Local agents

Only the first three layers are in scope for the initial implementation.

## Crates

- `hangar-core`: shared types and orchestration
- `hangar-db`: SQLite migrations and queries
- `hangar-fs`: filesystem walk/stat identity
- `hangar-nav`: nav tree, recent, pinned, quick open
- `hangar-preview`: safe file preview
- `hangar-protect`: Protected Zones
- `hangar-security`: encryption and redaction
- `hangar-jobs`: background job runner
- `hangar-api`: typed Tauri commands
- `hangar-test-fixtures`: local fixtures

Later:

- `hangar-accounting`
- `hangar-adapters`
- `hangar-graph`
- `hangar-resolve`
- `hangar-plan`
- `hangar-mutation`

## Rule

Read-only core must compile and run without mutation and without agent automation.

## Windows shell ingress

The installed Windows app can receive an existing local file or folder from a
file association, `Open with`, or a File Explorer context menu. This ingress is
navigation-only. A file request first takes a DB-independent path: it validates
the raw absolute local path, inspects reparse/cloud identity before canonicalisation,
and applies the bounded no-recall preview policy without waiting for SQLCipher,
migrations, project registration or scan creation. That provisional Viewer is
painted from at most 256 KiB before catalog work begins; a DB-independent full
bounded preview replaces it after that frame. Multiple Explorer requests all take
this immediate lane, while only the newest untouched destination proceeds to
project attachment, preventing stale ad-hoc roots and scans. Once the inventory is ready, a file inside a
registered project is promoted into that project; an unknown file is attached to
a hidden ad-hoc Viewer root for its parent. Promotion replaces the provisional
route/tab rather than leaving a false project in navigation. An unknown folder pauses at
a UI choice: Viewer uses only that folder; Automatic finds the nearest
marker-bearing ancestor (with a local fallback); Manual validates a user-selected
containing root. The normal read-only scan then updates navigation in the
background and project modes may refresh local AI-app correlation. Viewer never
registers a project or aggregates sessions. Direct preview uses the same bounded
read, Protected Zone, reparse/cloud and Markdown sanitisation policy as indexed
preview, but persists neither a preview body nor a Recent item until a real node
exists. The ingress cannot accept a URL/UNC/mapped-network path, mutate a project
or add network access.

Local and Connector builds share a named single-instance lease because they
share the same encrypted catalog. A secondary launch leaves a size-bounded,
one-shot DPAPI-protected path envelope for the primary instance; the Tauri event
contains no path and only tells the frontend to drain the managed queue.

## Resident awareness and local change timeline

The Windows base application can optionally register its exact executable in the
current user's `Run` key with a `--background` argument. That launch destroys the
bootstrap WebView and keeps only the native notification-area menu plus resident
service; the WebView and UI hydration (project lists, dashboard summaries and
global session discovery) are created on first explicit open. Closing `main`
destroys its WebView to stop deferred hydration but keeps the native resident
process; the next activation recreates it. Only the tray's explicit Exit action
ends the resident process.
Context-classification policy is keyed by an explicit version, but upgrades are
lazy: startup records the marker in constant time and each project's next normal
scan applies the current policy. A large encrypted catalog is never swept merely
because Windows started the resident process.

One bounded loop reuses Phase 4 watcher signals and the existing cancellable,
read-only scanner. Every 30 seconds it checks only registered-root metadata.
This quick pass is strictly detection-only: it neither queries the large
encrypted navigation catalog nor lets changing Markdown, build output or AI
session files promote it to a whole-root scan. Focused context fingerprints run
while the relevant project is open. A six-hour safety pass (or the explicit
tray refresh) selects only the least-recently scanned root to close gaps such as
newly-created nested files. Reconciliation admits one root at a time, uses one
below-normal-priority worker with small batches and explicit yields, yields
whenever any other scan is running, and skips disabled, missing, empty and
ad-hoc Viewer roots. There is no updater, network listener, telemetry or second
scan engine.

Registered-root inventory is delta-reconciled. A repeat scan keeps stable
navigation/node identities, performs no write for unchanged items, inserts or
updates only changed paths, and removes disappeared paths only after the walk
completes. Expensive aggregate/link rebuilding is skipped for a no-op scan; an
interrupted scan keeps the last complete navigation visible and marks the root
as needing another pass. Resident finalization also defers whole-project
footprint, Markdown-relationship and workflow/model projections; foreground
scans refresh those derived views, while navigation, indexed Markdown and the
change timeline remain current in the resident path. The scanner carries the
last completed body fingerprint into the one-worker resident pass: unchanged
Markdown/context files are never reopened, while new/changed bodies still use
the no-recall content gate and refresh FTS. Generated/protected containers are
opaque during resident scans, so their recursive footprint is measured only by
an explicit foreground scan.

After a completed registered-root scan, `hangar-db` compares the new navigation
metadata with the previous baseline inside the encrypted catalog. It retains a
bounded created/modified/removed event timeline containing project/node/path,
mtime and size metadata only. The first scan is a silent baseline. App names are
joined later from existing project correlation and mean only that an app knows
the project; they are not attribution evidence.

## Cloud-backed project roots

OneDrive projects use the same registered-root, navigation, correlation and
resident pipelines as projects elsewhere. At process start, `hangar-fs` switches
Windows from placeholder-disguise compatibility to exposed-placeholder mode.
Identity inspection can then distinguish an online-only `cloud_placeholder` from
a materialized `cloud_local` Cloud Files reparse point. Both retain their physical
reparse identity so mutation code remains conservative.

Directory and filename enumeration is metadata-only. A materialized Cloud File
may be indexed or previewed through a shared `OPEN_NO_RECALL` reader; a symlink,
junction, unknown reparse point or online-only placeholder is refused before body
bytes are read. The opened handle is checked again to close the metadata/open race.
The proof includes every ancestor, and scan roots/subtrees are revalidated before
canonicalisation at both registration and worker start, so replacing a known root
with a junction cannot redirect a resident or interactive scan.
SQLite discovery also holds no-recall handles for the database and existing WAL,
SHM or journal sidecars while the read-only connection is alive. If a provider
dehydrates an item between inspection and open, the read fails closed and the
scanner refreshes the stored identity to Cloud-only.

Global discovery deliberately has no broad OneDrive Documents/Desktop source.
Projects inside OneDrive still arrive from registered roots and app/session
registries. An explicit Deep Scan of a cloud-backed root is capped at depth 8,
10,000 directories and 10 seconds for its exploratory marker walk. No code path
changes a OneDrive pin or requests hydration; the user materializes a file through
Windows Explorer when desired.


## Phase-specific engineering detail

Detailed algorithms, schemas and risk-tier definitions for Phase 1.5 onward are kept in `docs/engineering_details_by_phase.md`. This keeps the first implementation pass focused while preserving the exact engineering detail needed later.

The file must not be used as permission to implement mutation, backup/quarantine or agent automation before their phase gates are passed.
