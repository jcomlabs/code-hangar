# First Agent Prompt, Strict Initial Scope

Use this prompt inside the `CodeHangar` repository.

You are implementing Code Hangar.

Read these files first, in this order:

1. `AGENTS.md`
2. `README.md`
3. `IMPLEMENTATION_PLAN.md`
4. `PHASE_0_1_TICKETS.md`
5. `SECURITY_INVARIANTS.md`
6. `docs/architecture.md`
7. `docs/ui_principles.md`
8. `docs/fixtures.md`
9. `CodeHangar_Master_Spec_v20_Final.md`, only as the source-of-truth reference when a ticket needs clarification
10. `docs/engineering_details_by_phase.md`, awareness only, do not implement its later-phase details yet

## Absolute scope boundary for this run

Implement only:

- Phase -1: repository foundation and guardrails;
- Phase 0: UI shell and Markdown/context navigation using fixture data;
- Phase 1A only: the very beginning of Phase 1, limited to SQLite/navigation migrations, fixture-backed DB wiring, and a read-only local folder scanner skeleton for Markdown/context files.

Do not implement full Phase 1 unless explicitly requested later.

## Allowed work

You may create:

- repository skeleton;
- Tauri v2 + React + TypeScript app shell;
- Rust workspace and empty crates listed in the plan;
- feature flags `core`, `mutation`, `agent_automation` with only `core` enabled by default;
- CI/lint/test scaffolding;
- outbound-network dependency deny checks for the base build;
- fixture-backed three-pane UI;
- safe Markdown preview;
- source/rendered toggle;
- quick open using fixture/local DB data;
- recent and pinned items;
- SQLite migrations for navigation tables only;
- fixture loader into SQLite;
- read-only folder picker and scanner skeleton limited to Markdown/context files;
- basic Protected Zone markers for preview/index exclusion.

## Forbidden in this run

Do not implement:

- mutation executor;
- backup;
- quarantine;
- restore;
- delete/permanent delete;
- OperationPlan;
- Risk Report;
- disk accounting beyond basic file size display;
- orphan cleanup logic beyond UI placeholders;
- File Lock Inspector;
- watchers;
- local agent IPC;
- MCP server;
- trusted agents;
- agent scopes/read grants;
- outbound network;
- online documentation fetch;
- adapter online updates;
- remote clone;
- Git fetch/pull/push;
- telemetry;
- auto-update.

## Implementation rule

If a file or module would only be needed for mutation, agent automation, Reference Layer, remote access, backup/quarantine/restore, or full disk accounting, do not create it yet unless it is an empty placeholder crate required by the planned workspace layout.

## End-of-run report

At the end, report:

1. files created or changed;
2. commands needed to run locally;
3. tests added;
4. feature flags and default state;
5. proof that no mutation, no agent IPC and no outbound network were added;
6. what remains for the next ticket.
