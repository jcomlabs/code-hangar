# Phase 5: Local Automation

Phase 5 lets explicitly registered local tools request scoped Code Hangar data
and safe operations. It is optional and is not compiled into the default `core`
or `mutation` builds.

## Build boundary

- Default: `core`, no named pipe and no mutation executor.
- Mutation: `mutation`, disk actions behind review and confirmation, no named
  pipe.
- Local automation: `agent_automation`; includes the mutation executor because
  safe action requests reuse the same plan, lock, journal and confirmation
  checks.

Build the most capable executable locally with:

```powershell
npm.cmd --workspace apps/desktop run tauri:build:agent
```

Run every local gate with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\local-ci.ps1 -AgentAutomation
```

## Transport

The server uses a Windows named pipe whose name changes each launch. It sets
`PIPE_REJECT_REMOTE_CLIENTS`; there is no TCP, HTTP, DNS, remote listener or
outbound client. The current endpoint is shown in Settings > Advanced > Local
automation.

Each connection sends one bounded JSON request and receives one bounded JSON
response using protocol `codehangar-agent/1`.

An unauthenticated `status` request returns capabilities only. Every other
request needs a random 32-byte token registered in the UI. Code Hangar displays
the token once and stores only its BLAKE3 hash inside the encrypted database.

## Scopes

- `read_structure`: project summary and context-file metadata, no bodies.
- `read_body`: non-sensitive body preview inside allowed projects.
- `build_plan`: transient OperationPlan/Risk Report preview.
- `execute_plan`: verified backup or holding-area move only, still requiring a
  fresh human confirmation token from the Code Hangar UI.
- `history_search`: bounded and redacted on-demand search for one explicit
  allowed project. No permanent history FTS is created.

Project IDs are always explicit. A credential with access to project A cannot
read, search or build a plan for project B. Loose sessions are not returned to
agents because they cannot be proven to belong to an allowed project.

## Read gates and revocation

File bodies require either permanent `read_body` scope or a ten-minute grant
created in the UI for the currently open file. Neither mechanism overrides
sensitive-file blocking or Protected Zones.

Revocation disables the token and revokes all active grants in one transaction.
Plans are transient and disk actions are synchronous, so no persisted agent plan
survives revocation.

## Audit

The encrypted activity log records the agent, method, allowed/denied status,
short diagnostic detail and timestamp. It never stores request/response bodies,
transcript snippets, file bodies or raw tokens.

## Protocol methods

- `status`
- `agent_project_context`
- `agent_read_body`
- `agent_plan_build`
- `agent_plan_execute`
- `deep_history_search`

`agent_plan_execute` accepts only `backup` and `move_to_holding`. It cannot
request final removal. The existing mutation layer revalidates the plan and
consumes the fresh human confirmation token.
