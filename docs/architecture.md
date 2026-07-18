# Architecture Notes

## Stack

- Tauri v2
- React
- TypeScript
- Rust
- SQLite
- SQLite FTS5 for Markdown/context files only

## Layers

1. UI shell
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


## Phase-specific engineering detail

Detailed algorithms, schemas and risk-tier definitions for Phase 1.5 onward are kept in `docs/engineering_details_by_phase.md`. This keeps the first implementation pass focused while preserving the exact engineering detail needed later.

The file must not be used as permission to implement mutation, backup/quarantine or agent automation before their phase gates are passed.
