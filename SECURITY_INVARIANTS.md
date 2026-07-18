# Security Invariants

These invariants are mandatory for Code Hangar.

## Base build

The base build must not contain:

- outbound HTTP client;
- DNS client;
- TLS client intended for outbound connections;
- telemetry;
- updater;
- remote Git operation;
- package registry client;
- external documentation fetcher;
- agent IPC server;
- mutation executor.

## Feature flags

`mutation` must be off by default.

`agent_automation` must be off by default.

Base build must compile and run without these features.

## Local-only policy

The strict `core` lane and the Local edition do not read online documentation, fetch remote repository previews, call GitHub APIs, call package registries, download adapters, upload telemetry or call provider APIs.

The connector edition has one narrow, opt-in exception: AI Assist may call the provider or loopback model server the user configures. That exception is isolated to `agent_automation` and documented under "Outbound AI Assist" below.

Remote URLs found on disk are passive metadata.

## Database at rest

File-backed SQLite databases are encrypted at rest with SQLCipher.

`openssl-sys` and `openssl-src` are allowed only as transitive build/runtime dependencies of SQLCipher for local at-rest database encryption. The higher-level `openssl` crate remains forbidden in the base build, and this exception does not permit TLS clients, HTTP clients, outbound network calls or package fetching in application code.

The SQLCipher key is wrapped with Windows DPAPI bound to the current user account (never `CRYPTPROTECT_LOCAL_MACHINE`) and additionally with a fixed application-specific secondary entropy, so a generic same-user "unwrap every `.dpapi` blob" sweep cannot recover the key without also knowing the app's entropy value. Legacy key blobs written without entropy are still accepted on read and transparently re-wrapped with entropy. The re-wrap is written atomically (temp + fsync + rename over the original), so a crash during the one-time upgrade can never corrupt the sole key blob.

Plaintext database migration artifacts must be removed after a successful encrypted migration. A migration interrupted by a crash is reconciled at the next `Db::open` *before* the database is used: a completed-but-uncleaned plaintext copy is deleted, a complete-but-unpromoted encrypted temp is promoted, and otherwise the plaintext is restored so migration re-runs — so a crash can never leave a readable plaintext copy of the database on disk past startup, nor lose data. This is best-effort file deletion, not a secure wipe guarantee.

Startup snapshots that contain project names, paths or scan-root metadata are treated as index data. They must not be stored in browser `localStorage` or other plaintext UI caches; if cached for startup responsiveness, they must be protected with the same local-user boundary as the encrypted inventory key.

## Markdown preview

Markdown preview must:

- sanitise HTML;
- execute no scripts;
- fetch no remote images;
- auto-open no links;
- block sensitive files by default;
- reveal sensitive text only after an explicit local user action;
- keep revealed sensitive content transient in memory only;
- obey Protected Zones.

Explicit reveal is allowed only for sensitive files that are already inside a registered local project and are not inside a strong Protected Zone. Revealed content must never be written to SQLite tables, FTS, persistent preview caches or logs.

## Adapters

Adapters are declarative data.

Adapters may not:

- execute shell commands;
- execute JavaScript;
- execute Python;
- execute PowerShell;
- perform network calls;
- create mutation hooks;
- auto-delete;
- auto-backup;
- hide mutation logic.

## Protected Zones

Protected Zones always govern preview, reveal, indexing and AI-send policy.
They are excluded from recoverable-space estimates and ordinary cleanup
recommendations.

The mutation-enabled folder-emptying path has a separate, deliberately narrow
rule: a protected or sensitive entry may participate only when it is inside the
selected target, is proven locally mutation-owned (not shared/referenced
elsewhere), is disclosed by the protected-item preview, and the user confirms a
complete backup-and-hold operation. Its bytes must be covered by the verified,
content-bound backup before the move. A protected entry that fails any of those
checks blocks the operation.

Reparse points are never followed and their targets are never read or changed.
For a confirmed folder-emptying operation, the link itself may be removed after
it is disclosed; revalidation must prove that it is still a reparse point at
execution time.

During early/read-only phases, Protected Zones block or limit preview and FTS.

Strong Protected Zones, including `.ssh` and app/system zones, cannot be revealed in the read-only extended inventory phase.

## Agents

Agents are not part of Phase -1, Phase 0, Phase 1, Phase 1.5, Phase 2 or Phase 3.

Phase 5 local automation is compiled only with `agent_automation`. The strict
`core` lane and the Local executable contain no local agent server.

The Phase 5 server must:

- use a Windows named pipe with remote clients rejected, never TCP/HTTP, and bound
  the number of concurrently-served clients so a same-user connection flood cannot
  exhaust threads or memory (excess connections are refused, not queued);
- expose only capability status to unauthenticated clients;
- authenticate every data request with a one-time-shown random token whose hash,
  not plaintext, is stored in the encrypted database;
- restrict every request to explicit scopes and registered project IDs;
- require `read_body` or a short-lived UI grant before returning a file body;
- keep sensitive and Protected Zone policy authoritative after a read grant;
- require the existing fresh human confirmation token before a scoped agent can
  request a verified backup or holding-area move;
- expose final removal to agents only as a queued request: no connected app can execute it directly,
  and approval still goes through the total-control gate, explicit final-removal opt-in, verified
  backup proof and a fresh human confirmation;
- revoke the token and every active read grant together;
- record method, agent, result and timestamp without storing response bodies or
  file content in the activity log.

Agents remain incapable of bypassing OperationPlan, Protected Zones, locks,
read gates, project scopes or the mutation journal.

## Connected AI apps (MCP)

The connected-AI-app surface lets the AI apps Code Hangar catalogs read — and,
when the user opts in, annotate — the curated knowledge (comments + a no-bodies
project context) over the Model Context Protocol. It is compiled only into the
dedicated `code-hangar-mcp` binary and the `hangar-mcp` / `hangar-appconfig`
crates. The strict `core` lane and the Local desktop executable contain none of
it; no member of the core or Local dependency graph links these crates
(CI-asserted via the targeted `code-hangar-desktop` cargo trees).

The server must:

- speak JSON-RPC 2.0 over **stdio only** — one short-lived child process, one
  peer, no socket, port, listener, TCP or HTTP. stdout carries only framed
  JSON-RPC; every diagnostic goes to stderr;
- require its per-app token in `CODEHANGAR_MCP_TOKEN` and fail closed without it;
- open the same DPAPI-wrapped encrypted database as the desktop app, binding it
  to this Windows user (another user or machine fails to open it);
- translate every tool call into an agent request dispatched through the SAME
  authenticated, scope- and project-gated, audited path as the named-pipe
  server. The runtime holds no database handle and re-implements no policy;
- gate reads on the `comments_read` scope and writes on the `comments_write`
  scope **and** the global `comment_write_enabled` toggle (default OFF); with the
  toggle off every write is refused while reads still work;
- assign a written comment's author/source from the authenticated agent name,
  never from the client, so an app can never forge a human (`user`) record; the
  name `user` is reserved at registration;
- let an agent add and manage only its OWN comments — never edit or delete a
  human comment or another agent's (`guard_comment_actor`);
- expose no comment-deletion tool (deletion stays human-UI-only) and no
  file-body tool or resource (the `read_body` path is never linked here).

Beyond comments, the connector exposes a fixed set of pre-cooked, **read-only,
body-free** discovery tools so an app can learn a project's main functionalities
without ever seeing file or session contents. They are gated by the
`read_structure` scope (catalog, context files, navigation tree, folder
explanations, Git status, adapter list, and the app's own request list —
`list_my_requests`, which returns only the CALLING app's queued/approved/denied
requests and their status, never another app's data), the granular `read_graph`
scope (project graph map, node relationships, orphan/duplicate candidates), or the
existing `history_search` scope (redacted session search — never the full
transcript). Each is project-scoped (or, for `list_my_requests`, own-app-scoped),
and the surface is fail-closed against cross-project leaks:

- `list_catalog` intersects the full project list with the agent's grants, so an
  app never learns of a project it was not scoped to;
- `explain_folder` resolves the folder's owning project and enforces membership
  **before** returning (the underlying lookup is keyed by nav id and does not
  check membership itself);
- `node_relationships`, `list_duplicate_candidates` and `confirm_duplicate_group`
  drop any row owned by an un-granted project (a duplicate group's members can
  span projects), recomputing counts and reclaimable bytes after the filter;
- `get_project_graph` drops any node, edge endpoint, issue, `shared_project_ids`
  entry or "inventoried by N projects" detail belonging to a project outside the
  grant — the graph can otherwise pull in cross-project duplicate/workflow edges —
  and scrubs the machine-wide counts (shared-cache / duplicate-model tallies) out of
  the surviving issue and edge evidence/target text;
- `search_sessions` sanitizes every hit, clearing the other-project absolute paths
  and retaining only granted project ids, so a multi-project session never reveals
  the existence, ids or on-disk locations of un-granted projects;
- the global "lost project" scan, the global orphan scan, the dashboard rollup,
  cross-project quick-open, and any tool returning a file/session body
  (`file_preview`, `file_reveal`, `search_documents`, `session_preview`) are
  deliberately **not** exposed — they cannot be meaningfully project-scoped or
  they leak bodies. No advertised tool maps to the body, plan-build, plan-execute
  or mutation methods (asserted in the connector tool-list test).

The total-control tier toggle (`mcp_full_control_enabled`, default OFF, heavily
signposted behind a double confirmation) gates a strict *request* model — the
agent never executes a privileged action itself:

- With the toggle OFF, the `request_comment_change` tool is refused outright; the
  connector exposes only the read/own-comment-write tools above.
- With it ON, a trusted agent may only FILE a pending request to edit or delete a
  comment it could not otherwise touch (e.g. a human's). Filing changes nothing.
- A human reviews each pending request in Code Hangar and approves or rejects it.
  On approval the app performs the action AS the user (`actor = "user"`) — only
  then — after offering a prior backup of the comment to a safe, user-chosen
  folder (written and verified before the change). The agent's identity never
  reaches `guard_comment_actor`; the user's explicit in-app decision does.

The request model is generalized beyond comments to the rest of the main surface,
always queue-only and human-approved, each kind behind its own scope:

- `request_backup_protected`, `request_move_to_holding` and `request_final_remove`
  queue the Gate-3 backup / move-to-holding / permanent-remove actions (scope
  `execute_plan`). The **app** builds the `OperationPlan` — never the agent — so its
  target fingerprint cannot be forged, and the agent supplies **no destination**: the
  human picks every folder at approval, so an app can never choose where backed-up
  secret bytes land.
- The connector does **not** advertise or dispatch a `request_file_access` tool over
  MCP: its approval would mint a per-node read grant that no MCP tool can redeem (no
  file-body tool is exposed here), so it was a dead end. The backend `RequestReadBody`
  method remains only for the in-app surface. Because filing any request writes a
  pending-request row, `RequestReadBody` — like every other `Request*` method — is
  classified as a write, so the read-only "panic switch" refuses to queue it too.
- Filing any write/mutation request is refused while the global read-only "panic
  switch" (`mcp_read_only_mode`) is on, and at approval the agent's queued authority
  is re-checked — a revoked, disabled or narrowed agent's request is dropped, never
  executed.
- `tools/list` is computed **per app token**: the advertised catalog is filtered to
  what that app could actually invoke — `request_*` tools only when the total-control
  tier is on (and `request_final_remove` only when the final-removal opt-in is on too),
  comment-write tools only with the `comments_write` scope, each read only with its
  read scope; an unresolved (invalid/revoked) token is shown only the read-only set.
  This is a UX affordance, not the gate — every `tools/call` is still fully
  re-authenticated and scope/toggle-checked on the one policy path.
- A target outside the agent's grants is allowed but flagged **cross-scope**: the
  approval gate then requires an explicit, separate cross-project authorization.
- On approval the app runs the unchanged Gate-3 executors **as the user**, which
  independently re-prove the verified-backup invariant, plan-fingerprint freshness
  and (for delete) the content-bound backup — the request layer adds no bypass. The
  mutation approval is a strengthened gate (recommendation-against, multiple
  warnings, a liability waiver, a required proceed-anyway acknowledgement, an easy
  human-picked backup, a read-only alternative, and a typed confirmation for final
  removal).
- **Final removal is OFF by default, and the user must opt in**: it is governed by an
  encrypted setting (enabled only when the user explicitly enables it in Recover) OR the
  supervised-QA `CODEHANGAR_ENABLE_FINAL_REMOVE=1` env var; a DB read error resolves to OFF
  (fail-closed). When it is off, the confirm token cannot even be minted at either enforcement
  point. Enabling only makes the action available — a verified backup that covers the file and
  a fresh confirmation are still required for every removal.

**Accepted architecture deviation (signed off).** Because the total-control request
model performs the approved Gate-3 actions, the connector binary (`code-hangar-mcp`)
links `hangar-api` with `agent_automation`, which pulls in `hangar-mutation` — the
mutation executor is therefore physically present in that binary, not merely
runtime-gated. This is a deliberate, documented departure from the earlier
"mutation-free MCP binary" sketch; it is acceptable because: (a) the strict
`core` lane links neither mutation nor connector crates, and the Local desktop
edition links mutation only for direct in-app user actions, not for connected-app
automation — `check-no-outbound-deps.mjs` fails if connector/AI crates enter the
core or Local dependency tree; (b) every executor path is runtime-gated behind
the default-OFF total-control toggle, the read-only switch, per-request human
approval, and (for final removal) explicit final-removal opt-in or the
supervised-QA env override; and (c) the connector is shipped only as the opt-in
add-on edition, never in the local-only edition.

Auto-registration into a host's config (`hangar-appconfig`) backs up the
existing file and verifies the copy, refuses to overwrite an unparseable config,
round-trip merges only the `code-hangar` entry (preserving every other key and
JSON order / TOML formatting), and writes atomically (temp + fsync + rename +
verify). The token lives in the host config `env` in plaintext, so it is a
same-Windows-user secret; each host gets its own token, and revoking removes both
the database credential and the config entry.

## Local graph parsing

Phase 6 workflow parsing is local-only, bounded to explicit candidate paths and
a strict per-file byte limit.

- Sensitive files, Protected Zones, reparse points and cloud placeholders are
  never opened by the graph parser.
- Workflow JSON is treated as data. Code Hangar never runs workflow nodes,
  scripts, plugins or shell commands.
- Model files are classified from local path and extension metadata. GGUF and
  safetensors may read only bounded header bytes for local summaries; model
  tensor payload/body bytes are not read during graph construction.
- Duplicate model warnings may read only the bounded first 64 KiB hash already
  used for duplicate candidates. Full model hashes are never computed
  automatically while opening the graph.
- Shared cache warnings are conservative diagnostics based on local path shape
  or existing project membership. They never mark bytes recoverable by
  themselves.
- Derived graph edges and issues are advisory index metadata and never grant
  mutation permission.

## Gate 3 — mutation (feature-gated; final removal off by default)

- **A file is never permanently deleted without a verified backup that covers it.**
  This is enforced in the backend, not the UI: `permanent_delete_entry` refuses unless
  the held entry links a `backup` row with `verified = 1` whose on-disk manifest
  (re-read, `verified: true`) contains the file's source path, and a move into the
  holding area refuses unless a verified backup covers every concrete plan file.
- Mutation acts only through a fresh Operation Plan: execution aborts if the target's
  `target_fingerprint` changed since the preview was built.
- Protected and sensitive files are excluded from recoverable-space figures. A complete
  folder-emptying operation may back up and move only those protected entries that are
  locally mutation-owned, inside the selected target, explicitly disclosed and confirmed;
  the verified backup must content-cover every such file. Shared/external protected entries
  remain ineligible and block the operation.
- Reparse points are never followed or backed up, and their targets are never touched. The
  disclosed link itself may be removed only during the confirmed complete-folder operation,
  after execution-time identity revalidation proves that it is still a reparse point.
- Every destructive action is journaled before it runs; a crash mid-delete is reconciled
  on next launch by the on-disk truth of the held copy, and an interrupted move rolls back.
  If both the original and held copies exist after an interrupted cross-volume move, neither
  is deleted: the held copy is exposed in Recover. A mutation may enter terminal `failed`
  only after its file and entry state is coherent; genuinely ambiguous outcomes remain
  `executing`/`verifying` and block every subsequent mutation.
- Recovered-space accounting is based on completed source removal, not on a successful copy.
  If a verified cross-volume held copy is created but the source unlink fails, the operation
  reports a failure, records zero recovered bytes and keeps the held entry recoverable.
- Final removal is OFF by default (mutation builds). The command and its token are governed by
  an encrypted setting (enabled only after explicit opt-in) or the supervised-QA
  `CODEHANGAR_ENABLE_FINAL_REMOVE=1` env var; a DB read error fails closed to OFF. With the
  setting off, the token is refused at both enforcement points; a verified backup + fresh
  confirmation are always required regardless.
- See `docs/gates/gate3_mutation_hardening.md` for the full checklist and the mandatory
  local release gate (`scripts/local-ci.ps1`).

## Reversible removal from AI apps (mutation feature)

"Remove project everywhere" un-registers a project from the AI apps that track it
(`crates/hangar-api/src/app_removal.rs`). It never touches the project's own files on disk;
it only edits the apps' OWN registries, and every change is recorded before it is made so it
is always reversible (the in-session Undo and the durable `removals.json` manifest behind the
Recover view).

- **Surgical edits to SHARED configs never clobber a concurrently-running app.** Cursor
  `storage.json`, Claude `~/.claude.json` and Codex `config.toml` are edited with
  `atomic_write_cas` (re-read-and-compare before the atomic rename); on a detected concurrent
  change it aborts having changed nothing. The same CAS guards the RESTORE path. This is a
  best-effort guard (a write landing strictly between the compare and the rename can still be
  lost on Windows), not an absolute no-clobber guarantee.
- **Removing one project never silently de-registers another.** An Antigravity per-project
  file can bundle several folder roots; if it lists siblings, only the target's
  `projectResources.resources[]` entries are surgically removed (a `json_array_item` record,
  restore re-appends exactly them). Cursor/Codex/Claude removals match a project root by
  normalized path and edit only that project's key/table. A blank project root is refused
  before it can match degenerate empty entries.
- **A partial failure is never silently unrecoverable.** Each app runs independently
  (best-effort); a failure in one is a warning, not an abort, and every change actually made
  on disk is persisted to `removals.json` before the warning is surfaced — so the Recover view
  can always reverse it.
- **Restore is idempotent and non-clobbering.** Hermes `db_rows` re-insert with
  `INSERT OR IGNORE`; a `file`/`dir` restore skips an original the app has re-created;
  progress is persisted so a retry never double-applies.
- **Containment.** A restore only writes under a managed registry location (the user config
  roots, with the dotted-segment escape hatch anchored to WSL UNC paths), never follows a
  reparse point, and keeps `file`/`dir` backups strictly inside the managed backup folder.

## Local retrospective review evidence

The What changed/Recap surface is read-only and retrospective. It combines only
bounded evidence already present on the machine: supported local AI-session edit
records, the local Git object database/index/working tree, the current authorized
project file, and normalized entries previously retained in the encrypted catalog.

- Git is invoked directly as `git` without a shell, with `--no-ext-diff`,
  `--no-textconv`, `--no-pager`, bounded output and an eight-second timeout. Each
  command disables `core.fsmonitor`, points `core.hooksPath` at `/dev/null`, disables
  system/global Git configuration and removes inherited Git/diff configuration
  variables. A runtime allowlist refuses every subcommand except local `diff`,
  `status`, and `rev-parse`; no fetch, pull, push, clone, remote, hook, credential,
  commit, branch, checkout, reset or restore command can pass through the production
  Git runner.
- A reviewed Git baseline is a validated full 40- or 64-hex-character local object
  id. It is passed as a single process argument, never interpolated into a shell.
- Current-file comparison canonicalizes existing targets, refuses reparse points,
  and proves the canonical target remains inside the registered project before a
  body is read. Sensitive/Protected files, non-UTF-8 files and files above the
  bounded read limit are labelled unverified instead of being opened.
- Review-ledger payloads are secret-redacted before storage. Sensitive paths,
  Protected Zones and paths that cannot be proven inside the project are removed
  from the persisted copy. The ledger is in the SQLCipher catalog, has per-entry
  and per-project retention caps, and never stores an unredacted session body.
  Each entry carries a Blake3 content hash, the preceding retained entry hash and
  its own chained entry hash. Reads validate payload and chain integrity and omit
  tampered entries instead of treating them as evidence. Ledger rows are evidence,
  not session-body cache entries, and remain governed by their own bounded retention.
- Cursor reconstruction reads only the selected composer's ordered local records
  and only accepts `edit_file_v2` bubbles with a persisted precomputed diff. Missing
  line numbers remain missing; prose and unsupported tool activity are never
  converted into invented edits.

## Outbound AI Assist (connector edition only, opt-in)

The optional AI Assist layer is the app's only outbound-network capability. It is
compiled solely into the AI Connector edition (`agent_automation`) and is invisible and
physically absent from the strict `core` lane and Local edition. The provider is **never hardcoded**: the
user chooses it — Off, a local model server, or their own API endpoint.

- The only network crate is `reqwest`, declared only in `crates/hangar-ai/Cargo.toml`.
  `scripts/check-no-outbound-deps.mjs` allows that single manifest exemption and proves via
  `cargo tree --features core` that neither `reqwest` nor `hangar-ai` is in the base graph;
  `hangar-ai` is also in `forbiddenBasePackages`. On Windows reqwest uses the OS TLS stack
  (SChannel); no OpenSSL.
- Frontend editions are also compile-time separate. `tauri.conf.json` always invokes
  `build:local` (Vite mode `offline`), while `tauri.connector.conf.json` invokes
  `build:connector`. `scripts/check-frontend-edition.mjs` fails packaging if Local contains
  Connector chunks, provider endpoints, AI/MCP command names or Connector-only UI copy, and
  requires those capability assets in the Connector build. The runtime `activeFeatures` check
  is defense in depth, not the mechanism that keeps dormant Connector code out of Local.
- **Provider-agnostic, default Off.** The provider is configured in the encrypted settings
  store as one of three modes (`crates/hangar-db` keys `ai_provider_mode|base_url|model|format`,
  default `off`). `off` means nothing is ever contacted — `hangar-api::resolve_ai_provider_config`
  hard-errors before any request is built, so a fresh install makes zero outbound calls until
  the user explicitly configures a provider. No provider/endpoint is baked in as a default;
  the UI presets (example local servers and API providers) are quick-fill **data**,
  not stored defaults.
- **Local mode is loopback-enforced.** A `local` provider's endpoint must resolve to loopback
  (127.0.0.0/8, `::1`, or the `localhost` domain). The check parses the URL with the **same
  `url` crate reqwest uses**, so the host validated is byte-for-byte the host reqwest will dial —
  closing parser-divergence bypasses (e.g. WHATWG treats `\` as `/`, so `http://evil.com\@localhost`
  actually dials `evil.com`). It runs at persist time (`ai_provider_set` refuses to store a
  non-loopback local URL) and again at send time against the **exact request URL** inside each
  adapter in `hangar-ai`. Local mode is keyless — any saved key is ignored so a cloud key can
  never leak to a local server.
- **Discovery is explicit and finite.** Nothing probes on mount or at startup. "Find local
  models" checks only the fixed numeric endpoints `127.0.0.1:11434`, `:1234`, `:8000` and
  `:8080`, sequentially, with a two-second timeout, no proxy and the same final-URL loopback
  guard. It performs no DNS, LAN scan or registry/process inspection. A discovered server is
  returned as a draft and is not persisted until the user chooses it and presses Save.
- The provider API key (used only in `api` mode) lives only in the Windows Credential Manager
  (via `keyring`), is read inside `hangar-ai`, and never crosses the IPC boundary to the
  webview, the app database, or logs. It is sent only as the provider's auth header
  (`Authorization: Bearer …` for the Chat Completions format, `x-api-key` for the Messages-API format).
- Every send runs a hard send-gate first (`crates/hangar-api/src/ai_assist.rs`), unchanged and
  provider-independent: a file with a sensitive path (`.env`, `.pem`, `.ssh`/`.aws`/`.gnupg`,
  key/credential files) or content that matches a secret pattern (private-key blocks, provider
  API keys, hard-coded secret assignments) is refused before any byte leaves the machine. The
  gate is re-run server-side on the actual send, never trusting the preview, so a misconfigured
  external provider can never receive secret-bearing content.
- **The literal request disclosure uses the real request builders.** Rust freshly resolves the
  inventory node/provider and re-runs the same file/selection and secret gates used by the send,
  then returns the exact final URL and compact JSON body. Authentication headers are never part
  of that value. Local mode also discloses the possible `stream: false` fallback body; changing
  target, lens, level or model clears the previous disclosure in the webview.
- **Streaming cannot create a paid retry.** SSE is used only for loopback providers. A local
  server that explicitly rejects streaming or returns no readable deltas may receive one
  disclosed non-streaming retry. External API mode always makes exactly one complete-response
  request and emits the result as one UI delta. Every response reader is capped at 2 MiB at the
  underlying byte stream, and the UI preserves any partial text if a stream then fails.
- **Provider redirects are never followed.** Every AI HTTP client uses
  `redirect::Policy::none()`. A 3xx response is returned as an error and cannot replay a body or
  credential to a different origin; regression tests cover 307 and 308 redirects to non-loopback
  targets.
- Provider-facing AI commands are read-only: they never route into plan-build/execute/mutation.
  The optional selected-passage suggestion returns an opaque, in-memory proposal only; a separate
  explicit local command applies it after confirmation under the light-correction rules below.
- Every UI entry point (the "Explain this code" menu item, the provider settings card) is
  gated on `activeFeatures` including `agent_automation` — not rendered at all otherwise.

### AI session usage and soft cap

Every call that can reach a model passes through `hangar-ai::explain`,
`hangar-ai::explain_stream` or `hangar-ai::provider_test`. Those three functions update one
process-local usage store after success or failure, including partial streamed output. UI
previews, exact-request disclosure, local discovery and model listing do not infer and are not
counted as model calls.

- The meter retains only session start time, call count and estimated input/output token totals.
  It never stores a prompt, response, path, model output or request body.
- Input and output counts are provider-agnostic estimates based on bounded character counts; they
  are not a provider bill. Code Hangar does not invent a currency or price because model prices
  and tokenizers differ. Local calls are labelled as having no per-token API charge.
- The default 50,000-token cap is a warning threshold, configurable from 10,000 to 250,000 tokens
  or disabled. It projects the next bounded output allowance before a send but never blocks the
  user's explicit choice to continue.
- Reset clears only the in-memory process-session counters. Closing the app also clears them; no
  usage history is written to the encrypted catalog, browser storage or logs.
- Output allowances remain modest and command-specific. The usage warning cannot raise an output
  limit, trigger a retry or bypass the exact-request/secret gates.

### Optional guided learning

The Connector-only reading aids remain subordinate to Explain and are closed by
default. They add no execution or project-write path.

- Walkthrough section ids, line ranges and hashes are reconstructed from freshly
  authorized bytes in Rust. The complete language-aware map may cover a file up to
  2 MiB, but every selected provider batch and its framing remains below 60 KiB;
  stale or invented section ids are refused before a send.
- Follow-up conversations are scoped to one inventory node and one reconstructed
  section, live only in process memory, allow at most three completed/reserved
  turns, and refuse concurrent turns. The usage preview includes the bounded prior
  turns; the real send re-resolves the node, section and secret gate again.
- Personal glossary persistence is off by default. The webview can submit only a
  term name; Rust resolves it against the built-in seed dictionary. The encrypted
  catalog stores only canonical term, definition and count (plus timestamps),
  never model output, code, snippets or paths.
- Anchored notes are local-only. Rust accepts an exact unique selection only on a
  complete, non-revealed, non-sensitive preview, derives the UTF-16 djb2
  `hashSnippet` anchor itself, and stores the note plus selection in the encrypted
  catalog. Listing re-anchors against fresh authorized bytes and labels an anchor
  `current`, `moved`, `ambiguous` or `stale`; the selection is never sent to a
  provider by the annotation commands.

### Reviewed one-file correction

Manual text changes and recognised-value changes are local correction surfaces, not an IDE or a
Git client.

- Project-file changes start locked on every app run and whenever the selected project changes.
  Unlocking requires an explicit acknowledgement and the exact project name. The unlock only
  exposes the correction controls for that project and never changes a file by itself. Opening a
  project, file, session, Recap or Git explanation does not unlock or enter a write path.
- The UI lock is an additional human-safety barrier, not an authorization boundary. Every write
  remains independently protected by backend project/path authorization, fresh-byte comparison,
  exact reviewed-output hash, verified off-project snapshot and atomic replacement. A frontend
  defect therefore cannot turn project navigation into a write or bypass the backend checks.
- A previously approved controlled project check still requires a fresh, unchecked confirmation
  for every run, with the exact detected command visible. Its warning states that project-code side
  effects outside the checked file may not be reversible. Deleting encrypted local comments or
  anchored notes also requires a separate permanent-delete acknowledgement.
- The backend freshly re-authorizes and reads the complete inventory file before producing a
  bounded line diff. A review is tied to the blake3 hash of the exact proposed bytes; applying a
  different draft is refused. The final write re-reads the file and refuses stale on-disk bytes.
- JSON and TOML must parse completely. Supported source files receive a deliberately lightweight
  local quote/comment/bracket check, clearly labelled as not a compiler; introducing a new detected
  structure error is blocked. Unsupported text formats carry an explicit warning instead of a
  false validity claim.
- Git context is passive evidence from the already local repository/index/working tree. It follows
  the read-only Git restrictions above and exposes no stage, commit, branch, revert, fetch, push or
  arbitrary command action. Other changed files are counted but never touched.
- Before every accepted write, Rust creates and verifies an off-project snapshot and then replaces
  the one file atomically. Previous versions are compared against freshly authorized current bytes
  through the same bounded diff before Restore becomes available; Restore snapshots the current
  version first, so it is itself reversible. Apply, Restore and Undo require a separate explicit
  confirmation after the relevant comparison; destructive confirmation controls are unchecked by
  default.
- The desktop text-write IPC accepts only a reviewed manual apply or a whole-file-CAS immediate
  undo. Recognised values are also bound to the reviewed proposed-file hash. Protected, sensitive,
  revealed, truncated, non-UTF-8, oversized, reparse and out-of-project targets remain refused.
- Large diffs have a fixed local output bound and disclose truncation. A truncated review never
  weakens whole-file CAS, validation, snapshot verification or the atomic write.

### Selected-passage correction

The Connector edition may ask the configured provider for a replacement for one explicit text
selection. This is retrospective light correction, not an implementation agent.

- Whole-file and multi-file rewrite commands do not exist. The complete source must be at most
  60 KiB and is freshly read and hard-gated before the provider receives only the selected bytes.
- The selection must occur exactly once. Duplicate or stale anchors are refused rather than
  choosing the first match.
- Provider output is staged in a bounded, expiring in-memory proposal store. It cannot write a
  path, byte range or replacement supplied by the webview.
- After the user reviews a plain-language summary and the exact before/after passage, a separate
  local command re-reads the file, verifies whole-file blake3 CAS, re-checks the unique span,
  splices by byte in Rust, and runs the format/source validity guard.
- Every accepted suggestion first creates and verifies the same durable off-project snapshot used
  by manual/value edits. It is recorded as `ai_suggestion` with a `session_id`; undo restores the
  snapshot from before that session's first edit through the verified restore path.

### Controlled correction checks

Correction validation has two deliberately separate tiers. Static checks parse the complete
authorized file and inspect local links/indexed relationships without executing project code.
Project-code checks are optional, Mutation-edition actions and are never a terminal or arbitrary
command facility.

- The backend detects a fixed allowlist from bounded local manifests only: `npm test`,
  `npm run build`, `cargo check`, `cargo test`, `go test ./...`, and `python -m pytest`.
  The webview supplies only a project id, correction node id, detected check id and fingerprint;
  it cannot supply an executable, argument or shell fragment.
- Approval is explicit per project and exact check fingerprint. The fingerprint covers the check
  identity, displayed command, manifest path, fixed executable/arguments and complete bounded
  manifest bytes. Any manifest change makes the stored approval inactive before execution.
- Approval copy states plainly that an allowlisted command is not a sandbox and runs project code.
  A correction snapshot can restore the checked file, but Code Hangar does not claim to undo
  arbitrary files or other side effects produced by that project's test/build process.
- On Windows, Rust starts the fixed executable directly with no free shell surface, null stdin,
  below-normal priority and a cleared/rebuilt environment. Cargo and npm receive offline flags;
  package audit, funding and update checks are disabled.
- The child is created with `CREATE_SUSPENDED`, assigned to a Windows Job Object, and resumed only
  after assignment succeeds. There is therefore no pre-assignment execution gap. The job kills on
  close, caps the active process count at 32 and caps job memory at 2048 MiB. A 120-second wall
  timeout terminates the whole job. Only 64 KiB per output stream is retained, and retained output
  passes through the local secret redactor before reaching the webview.
- Only one project check may run at a time. The correction node is re-verified as belonging to the
  approved project, and the check is freshly re-detected before every run. The latest verified edit
  snapshot is exposed as a one-click correction restore after the result.
- These commands are compiled under `mutation`, not strict `core`. The deterministic static report
  also uses the Mutation edition's complete-file validity guard; neither tier adds an outbound call.
