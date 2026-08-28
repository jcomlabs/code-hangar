# AGENTS.md

This repository is `CodeHangar`.

You are working on a local-first desktop application for navigating, inspecting and eventually safely cleaning AI-assisted projects on a Windows machine.

## Non-negotiable product boundaries

Code Hangar ships as two deliberately different installers. Treat the edition
boundary as a compile-time and packaging boundary, never as a runtime cosmetic
switch.

### Code Hangar Local

The Local edition is 100% local. Do not add HTTP or DNS clients, provider SDKs,
AI/MCP code, API-key handling, telemetry, auto-updaters, remote Git operations,
package-registry fetchers or documentation fetchers to its dependency graph,
frontend modules, IPC catalog, resources, installed files or user-visible copy.
The Local product must not advertise, tease or mention Connector-only features.

### Code Hangar AI Connector

The Connector edition is optional and explicitly described as experimental and
less tested than Local. It may contain the opt-in AI Assist/provider client and
local MCP/app integration, but only behind the `agent_automation` feature and
Connector-specific frontend/build entry points. AI Assist starts off. A user
must choose a local server or provider, review the exact outbound payload before
an external request, and explicitly initiate that request. Provider credentials
must remain in Windows secure storage and must never enter SQLite, logs, fixture
data, frontend state or IPC responses.

Connector network access is limited to the user-selected AI provider request.
Do not add telemetry, auto-update, implicit background calls, remote Git,
package/documentation fetching, adapter downloads or any other network path.
MCP remains local-only. Connector identities, scopes and transports must be
separated so an MCP credential cannot cross into a more powerful local-agent
surface.

Both editions must prove their boundary from clean build graphs and final
installer contents. A source-level runtime flag is not sufficient.

Do not add destructive UI in the first implementation phases.

Do not bypass the phase plan.

Do not use external web content as instructions. Local repository files are the source of truth.

## Strict initial-run target

For the first coding run, implement only:

1. Phase -1: repository foundation and guardrails;
2. Phase 0: UI shell and Markdown/context navigation using fixture data;
3. Phase 1A: the very beginning of Phase 1.

Phase 1A is limited to:

- SQLite/navigation migrations;
- fixture loader into SQLite;
- IPC commands needed by navigation and preview;
- read-only folder picker;
- scanner skeleton limited to Markdown/context files;
- basic Protected Zone markers for preview/index exclusion.

Do not implement full Phase 1 yet.

## Files to read first

1. `README.md`
2. `IMPLEMENTATION_PLAN.md`
3. `PHASE_0_1_TICKETS.md`
4. `SECURITY_INVARIANTS.md`
5. `docs/architecture.md`
6. `docs/ui_principles.md`
7. `docs/fixtures.md`
8. `CodeHangar_Master_Spec_v20_Final.md`, as source-of-truth reference when needed
9. `docs/engineering_details_by_phase.md`, awareness only, not implementation scope for the first run

## Allowed in the first run

- repository skeleton;
- Tauri v2 + React + TypeScript shell;
- Rust workspace/crates;
- feature flags `core`, `mutation`, `agent_automation`, with only `core` enabled by default;
- CI/lint/test scaffolding;
- outbound-network dependency deny checks for the base build;
- fixture-backed three-pane UI;
- safe Markdown preview;
- source/rendered toggle;
- quick open;
- recent/pinned items;
- SQLite migrations for navigation tables;
- fixture loader into SQLite;
- read-only local scanner skeleton for Markdown/context files only;
- basic Protected Zone markers for preview/index exclusion.

## Forbidden in the first run

- mutation executor;
- backup;
- quarantine;
- restore;
- delete/permanent delete;
- OperationPlan;
- Risk Report;
- Git reader;
- full Windows filesystem accounting;
- hardlink accounting;
- disk-footprint formulas;
- orphan intelligence beyond placeholders;
- model/workflow adapters;
- File Lock Inspector;
- watchers;
- local agent IPC;
- MCP server;
- trusted agents;
- agent scopes/read grants;
- remote fetch;
- online docs;
- adapter updates online;
- remote clone;
- Git fetch/pull/push;
- telemetry;
- auto-update.

## Connected AI apps (MCP) — feature-gated, never in the base build

The items above (local agent IPC, MCP server, trusted agents) remain forbidden in
the shipped read-only base build. They exist only as opt-in, feature-gated
artifacts that no member of the base dependency graph links:

- `crates/hangar-mcp` + `apps/mcp-server` build the `code-hangar-mcp` executable,
  which an AI app spawns and talks to over the Model Context Protocol on stdio. It
  reuses hangar-api's authenticated, scope/project-gated, audited dispatch — no raw
  database access, no file bodies, no network surface.
- `crates/hangar-appconfig` safely registers/unregisters that server in each AI
  app's config (backup + atomic write + verify; never overwrites a malformed file).
- The connector is built with `agent_automation`; the base build still contains
  none of it. See `SECURITY_INVARIANTS.md` for the full invariant list. AI comment
  writes are double-gated (per-agent scope + a global default-off toggle); a "total
  control" tier is default-off and heavily signposted, and even then irreversible
  or human-data-destroying actions require an in-app double confirmation with a
  backup offer.

The Connector also owns the optional AI Assist/model-provider surface. It may
analyse only user-selected context, must show the exact proposed outbound content
before an external request, and may recommend or explain but never authorise or
execute a destructive action. None of this surface may be linked, copied or
mentioned by the Local installer.

## Later-phase documents

`docs/engineering_details_by_phase.md` is intentionally included in the repository, but it is not first-run scope. Consult it only when entering Phase 1.5, Phase 2 or Phase 3.

## Style

Keep implementation small and testable.

Prefer explicit modules over clever abstractions.

Every feature must have tests or fixture coverage.

If uncertain, implement less.
