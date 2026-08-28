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

The Connector edition is an explicit exception for one user-selected provider
client. Its `agent_automation` feature adds local MCP stdio, app-configuration
registration and a Windows named pipe plus the narrowly confined `hangar-ai`
provider edge documented under "Edition-aware network boundary" below. It still
forbids telemetry, updater, remote Git and implicit/background network activity.

Remote URLs found on disk are passive metadata.

## Database at rest

File-backed SQLite databases are encrypted at rest with SQLCipher.

`openssl-sys` and `openssl-src` are allowed only as transitive build/runtime dependencies of SQLCipher for local at-rest database encryption. The higher-level `openssl` crate remains forbidden in the base build, and this exception does not permit TLS clients, HTTP clients, outbound network calls or package fetching in application code.

The SQLCipher key is wrapped with Windows DPAPI bound to the current user account (never `CRYPTPROTECT_LOCAL_MACHINE`) and additionally with a fixed application-specific secondary entropy, so a generic same-user "unwrap every `.dpapi` blob" sweep cannot recover the key without also knowing the app's entropy value. Legacy key blobs written without entropy are still accepted on read and transparently re-wrapped with entropy. The re-wrap is written atomically (temp + fsync + rename over the original), so a crash during the one-time upgrade can never corrupt the sole key blob.

Plaintext database migration artifacts must be removed after a successful encrypted migration. A migration interrupted by a crash is reconciled at the next `Db::open` *before* the database is used: a completed-but-uncleaned plaintext copy is deleted, a complete-but-unpromoted encrypted temp is promoted, and otherwise the plaintext is restored so migration re-runs — so a crash can never leave a readable plaintext copy of the database on disk past startup, nor lose data. This is best-effort file deletion, not a secure wipe guarantee.

Startup snapshots that contain project names, paths or scan-root metadata are treated as index data. They must not be stored in browser `localStorage` or other plaintext UI caches; if cached for startup responsiveness, they must be protected with the same local-user boundary as the encrypted inventory key.

## Windows shell integration

File associations and File Explorer menu entries are optional, per-user (`HKCU`) and reversible. Code Hangar may register itself as an available handler for `.md`, `.markdown` and `.mdx`, but must never write or bypass Windows `UserChoice`; Windows Default Apps remains the user's authoritative choice.

Incoming shell-open requests must:

- accept only existing absolute paths on local drives, never URLs, protocols, UNC shares or mapped network drives;
- canonicalise and revalidate the path at the API boundary;
- quote the executable and Windows path placeholder independently in every registry command;
- inspect before registering: a path already contained by a registered root reuses that root; an unknown file may default only to the non-registering isolated Viewer, while an unknown folder requires an explicit Viewer, Automatic or Manual choice;
- keep Viewer roots ad-hoc, read-only and hidden from the project/session catalog; Automatic uses the nearest recognised ancestor (falling back to the selected folder or file parent), and Manual accepts only an existing local folder that contains the opened target;
- preserve Protected Zone, sensitive-file and indexing exclusions before preview;
- carry no path in broadcast events; the short-lived cross-instance request envelope is DPAPI-protected for the current Windows user, size-bounded, consumed once and deleted;
- start only the existing read-only scanner; project modes may refresh local AI-app/session correlation, while Viewer never does. Shell integration creates no mutation authority and no network capability.

Direct shell preview must revalidate the original requested entry, every ancestor,
project-root containment and local-drive boundary, then reuse the ordinary bounded
preview and Protected Zone policy. The first painted frame is capped independently
of the normal preview and its full bounded expansion remains DB-independent. It must
not wait for indexing, follow a reparse point,
hydrate a cloud placeholder, persist a file body or add a Recent item before the
scan resolves a real node.

Start-at-login is optional and per-user. Its `HKCU\...\Run` value must quote the
exact current executable and pass only the fixed `--background` argument. A
background launch may hide the window, create a local tray menu and start only
bounded read-only refreshes of registered roots. It must skip temporary Viewer
roots and must not add telemetry, update checks, network access or mutation.

The local file-change timeline is metadata-only and bounded. The first completed
scan establishes a silent baseline; later completed scans may record path,
created/modified/removed kind, node/project identity, mtime and size in the
encrypted catalog. It must not store file bodies or assert causal attribution to
an application, session or agent. App correlation displayed beside an event is
context only.

## Cloud-backed roots and zero implicit hydration

OneDrive and other Windows Cloud Files locations are valid project roots. They
must not be excluded merely because they are cloud-backed. The safety boundary is
content locality:

- Code Hangar enables exposed-placeholder compatibility for its process before
  filesystem work. It must not rely on Windows' legacy placeholder disguise.
- Enumeration may inspect names, directories, logical size, timestamps,
  attributes and placeholder state without opening file bodies.
- Every automatic content read (scanner, preview, search indexing, project
  summary, discovery transcript/database reader and resident refresh) must use a
  no-recall gate. Materialized Cloud Files may be read; online-only placeholders,
  ordinary links, junctions and unknown reparse points must fail closed.
- Registration, ad-hoc investigation, estimate and every scan worker must prove
  the scan root and any resumed subtree component-by-component before
  canonicalisation. Replacing a registered root/subtree with a link or junction
  must fail the scan rather than redirect it.
- The no-recall handle must be rechecked before the first byte read. A state race
  updates the stored identity to Cloud-only and yields no body.
- Online-only entries remain visible as `Cloud-only · content not indexed`; they
  contribute metadata but never FTS or preview body text.
- Global automatic discovery must not traverse the user's broad OneDrive root,
  Documents or Desktop. Registered roots and app/session-linked paths inside
  OneDrive remain supported. Explicit exploratory cloud discovery must have
  strict depth, directory-count and wall-clock budgets.
- A scan, global search, discovery pass or resident refresh must never change a
  provider pin or request hydration. The base build offers no hydration action,
  because initiating a OneDrive download would violate its no-outbound-network
  dependency boundary. Materialization remains an explicit Windows Explorer/user
  action; Code Hangar can read the file on a later pass once it is local.
- Reparse/cloud identity remains conservative for mutation even when content is
  materialized. Being locally readable is not permission to move, back up or
  delete a provider-managed placeholder.

Local and AI Connector editions share one registration owner. Installing or changing settings in one edition may deliberately transfer ownership to that executable; uninstalling the owner rebinds to a remaining valid Code Hangar installation when present, otherwise it removes the dedicated Code Hangar keys. The Local installer discovers that survivor by neutral enumeration of the shared installation registry and must not embed another edition's product name, executable path or capability vocabulary.

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
- expose final removal to agents only as a queued recommendation: no connected app can execute it
  directly, and approving the unscoped request still deletes nothing. The owner must separately
  use the immutable project/batch review in local Recover, with an object-complete recovery proof
  and fresh confirmation bound to that exact batch. The same durable, owner-controlled
  `final_remove_enabled` capability gates local preview, confirmation and batch start and is OFF
  by default; enabling it requires the exact in-app phrase `ENABLE PERMANENT REMOVAL`;
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

- `request_backup_protected` and `request_move_to_holding` queue Gate-3 backup /
  move-to-holding actions (scope `execute_plan`). `request_final_remove` may queue
  a recommendation to review a held entry, but approving that request cannot execute
  an unscoped single-entry delete: Code Hangar directs the owner to the immutable
  project/batch review in **Recovery & cleanup**. The **app** builds every
  `OperationPlan` — never the agent — so its
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
  tier is on (and `request_final_remove` only when the separate connected-app
  final-removal authorization is on too),
  comment-write tools only with the `comments_write` scope, each read only with its
  read scope; an unresolved (invalid/revoked) token is shown only the read-only set.
  This is a UX affordance, not the gate — every `tools/call` is still fully
  re-authenticated and scope/toggle-checked on the one policy path.
- A target outside the agent's grants is allowed but flagged **cross-scope**: the
  approval gate then requires an explicit, separate cross-project authorization.
- On approval the app runs the applicable Gate-3 backup/move executors **as the user**, which
  independently re-prove the verified-backup invariant, plan-fingerprint freshness
  and (for delete) the content-bound backup — the request layer adds no bypass. The
  mutation approval is a strengthened gate (recommendation-against, multiple
  warnings, a liability waiver, a required proceed-anyway acknowledgement, an easy
  human-picked backup, a read-only alternative, and a typed confirmation for final
  removal).
- **Permanent removal is a central capability, deliberately OFF by default.** The durable
  `final_remove_enabled` owner setting gates both the Local preview/confirmation/batch APIs and
  whether an authorized connected app may file the corresponding review recommendation. There
  is no environment-variable or test bypass in the distributed app. Enabling requires the exact
  in-app phrase `ENABLE PERMANENT REMOVAL`; disabling signals an active batch to stop after its
  current topology group and is serialized with preview, confirmation and the execution worker.
  Once the setting reports OFF, a batch cannot progress past its authoritative
  worker-side recheck even if a fresh confirmation was just issued. A DB read error fails closed. Enabling is availability,
  never execution authority: actual removal still requires the local object-complete,
  immutable-batch flow and its fresh digest-bound confirmation.

**Accepted architecture deviation (signed off).** Because the total-control request
model performs the approved Gate-3 actions, the connector binary (`code-hangar-mcp`)
links `hangar-api` with `agent_automation`, which pulls in `hangar-mutation` — the
mutation executor is therefore physically present in that binary, not merely
runtime-gated. This is a deliberate, documented departure from the earlier
"mutation-free MCP binary" sketch; it is acceptable because: (a) the strict
`core` lane links neither mutation nor connector crates, and the Local desktop
edition links mutation only for direct in-app user actions, not for connected-app
automation — `check-no-outbound-deps.mjs` fails if Connector crates enter the
core or Local dependency tree; (b) every connected-app executor path is
runtime-gated behind the default-OFF total-control toggle, the read-only switch
and per-request human approval. Final removal additionally requires the same
fresh, immutable object/topology batch review and digest-bound confirmation as
local Recover; no environment variable or persistent local-user switch grants
that authority; and (c) the connector is shipped only as the opt-in add-on
edition, never in the local-only edition.

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

## Gate 3 — mutation (feature-gated; final removal is explicit per batch)

- **An object is never permanently deleted without a verified recovery copy that covers
  its complete supported representation.** This is enforced in the backend, not the UI.
  The migration-era `permanent_delete_entry` command is retained only as an explicit
  fail-closed compatibility symbol: action-only permanent-delete grants are deliberately
  impossible, so no production caller can reach its historical body. A
  `backup_manifest/1` cannot authorize any shipping final deletion. Project/batch final removal
  requires an `object_archive/2` proof for the exact file/directory/topology group,
  including allowlisted streams and security metadata, plus a successful replay and
  recapture. A move into the holding area also refuses unless its applicable recovery
  proof covers every concrete plan object.
- Mutation acts only through a fresh Operation Plan: execution aborts if the target's
  `target_fingerprint` changed since the preview was built.
- Protected and sensitive files are excluded from recoverable-space figures. A complete
  folder-emptying operation may back up and move only those protected entries that are
  locally mutation-owned, inside the selected target, explicitly disclosed and confirmed;
  the verified backup must content-cover every such file. Shared/external protected entries
  remain ineligible and block the operation.
- Reparse points are never followed and their targets are never touched. A disclosed link
  object may become eligible only through the separately tested no-follow link profile that
  captures and round-trips its exact reparse payload. Until that profile is present, the link
  and any required ancestor directory remain held with a stable blocked reason; a regular
  file or directory can never be routed through a link-removal path.
- Every destructive action is journaled before it runs. Batch/item intent, exact identity,
  archive proof and disposition outcome have explicit states. A crash after physical
  deletion is reconciled from FileId/proof/absence evidence and commits the item, held-entry,
  operation and per-volume space effect coherently; recovery never automatically resumes an
  irreversible delete. If both the original and held copies exist after an interrupted
  cross-volume move, neither is deleted: the held copy is exposed in Recover. Genuinely
  ambiguous outcomes remain non-terminal and block conflicting mutation.
- Windows final disposition is armed only after an authenticated, separately signed guardian
  process has duplicated the exact no-sharing parent handle and that binding is durable. Parent
  and guardian independently prove the mode-specific delete-pending link/stream profile; close
  authorization is durable before either handle is intentionally closed. Before that authority,
  a failed cancellation makes the guardian retain and retry rather than close an armed duplicate.
  Recovery never treats guardian state before `close_authorized` as proof of deletion. This guards
  a parent-process crash while the guardian remains alive; it is not a guarantee against
  simultaneous process termination, session teardown, restart or power loss.
- Recovered-space accounting is based on completed source removal, not on a successful copy.
  If a verified cross-volume held copy is created but the source unlink fails, the operation
  reports a failure, records zero recovered bytes and keeps the held entry recoverable.
- Recovery exposes the permanent-removal capability and its current state instead of silently
  hiding it. The capability is OFF by default and its durable backend gate is rechecked by preview,
  confirmation and batch start. Enabling it with `ENABLE PERMANENT REMOVAL` never authorizes an
  operation by itself. The backend still rebuilds an immutable project/batch preview, requires an
  object-complete `object_archive/2` proof for every eligible topology group, and consumes a
  short-lived OS-random confirmation bound to the exact preview digest and selected groups.
  Disabling is linearized against the execution worker; once it returns, no newly admitted worker
  can reach helper resolution, token consumption or deletion. Unsupported objects fail closed individually and stay held. The retired
  one-entry command remains fail-closed and cannot authorize the batch operation.
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
  `status`, `rev-parse`, and exact `git remote` name enumeration. The latter reads
  configured names only: no URL query, DNS resolution or remote contact is possible.
  No fetch, pull, push, clone, hook, credential, commit, branch, checkout, reset or
  restore command can pass through the production Git runner.
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

## Edition-aware network boundary

The shipped Local edition is zero-outbound. `agent_automation` is a separate,
opt-in Connector build: MCP remains local, while AI Assist may make only the
provider request the user explicitly configured and reviewed. No inference,
discovery probe or credential use may run merely because the app started.

- Local's supported Windows graph must contain no `hangar-ai`, `reqwest` or
  `keyring`, no `hangar-api -> hangar-ai` active edge, and no provider source,
  command, chunk, CSS or installed MCP artifact. Tauri already brings the `url`
  parser transitively, so the invariant deliberately does **not** claim that the
  lockfile or Local graph contains no `url`; it rejects provider declarations and
  provider reachability instead.
- The sole Connector outbound crate is `crates/hangar-ai`. The only activation
  edge is optional `hangar-api -> hangar-ai` inside `agent_automation`. Its only
  direct provider dependencies are `reqwest 0.12` with default features disabled
  and `blocking`, `json`, `native-tls`; `url 2`; and `keyring 3` with default
  features disabled and `windows-native`. Renamed-package/alias forms do not widen
  this allowlist.
- Production HTTP-client code is confined to `crates/hangar-ai/src/lib.rs`;
  provider prompt/request construction is confined to
  `crates/hangar-api/src/ai_assist.rs`, while the Connector-only Safe Manage
  selector/receipt boundary is isolated in
  `crates/hangar-api/src/connector_advisory.rs`. The client disables proxy and
  redirects. Local-provider URLs are rechecked as loopback; remote provider URLs
  require HTTPS (plain HTTP is accepted only for loopback). Raw sockets are
  permitted only in exact `cfg(test)` loopback/redirect fixtures.
- Optional Safe Manage recommendation enrichment accepts only random,
  process-local context selectors issued for the exact project/run/evidence
  revision. It retains the deterministic baseline, may return a different typed
  recommendation for any current non-`DoNotTouch` assessment, and labels AI
  confidence separately. The backend
  resolves inventory nodes or allowed session-store paths, applies ordinary
  preview membership/Protected/reparse/cloud gates, removes absolute paths,
  blocks remaining high-signal secrets, and then shows the exact provider body.
  SQLite receipts contain request/result hashes and character counts plus typed
  opaque source hashes/redaction counts; their schema cannot store request/result
  bodies, keys, provider/model config, display names or source paths.
  Provider prose is parsed only through exact allowlisted recommendation and
  confidence tokens; malformed output cannot become an inferred action. Even a
  valid changed recommendation is display-only: it cannot record a user
  decision, create an OperationPlan or cross any mutation, protection, backup,
  holding, restore or final-removal gate.
- `crates/hangar-mcp`, `apps/mcp-server`, `crates/hangar-appconfig` and
  `crates/hangar-agent` remain the local Connector integration surface. The MCP
  executable reuses `hangar-api` authentication, project scopes, audit records and
  mutation gates; it does not expose raw database or filesystem access.
- `scripts/check-no-outbound-deps.mjs` validates exact manifests and the resolved
  Windows `core`, `mutation`, Connector desktop and sidecar graphs. The all-target
  audit separately documents Tauri 2.11.2's `reqwest 0.13.4` edge for unsupported
  Android/non-macOS-Apple targets; it is not part of a shipped Windows graph.
  SQLCipher's `openssl-sys`/`openssl-src` transitive exception remains solely for
  database encryption at rest.
- Frontend editions remain compile-time separate. `tauri.conf.json` invokes
  `build:local`; `tauri.connector.conf.json` invokes `build:connector`.
  `scripts/check-frontend-edition.mjs` rejects all Connector/provider/MCP chunks,
  copy, CSS and IPC from Local, requires the complete gated surface in Connector,
  and rejects browser `fetch`, XHR, beacon, socket, Tauri HTTP, telemetry, updater
  and remote-Git primitives in **both** bundles. Provider transport stays in native
  Rust rather than the webview.
- Connect and Disconnect support Claude, Codex and Cursor by backing up, atomically
  updating and verifying only Code Hangar's own local MCP registration. A malformed
  app configuration is never overwritten.
- Connected-app reads remain per-agent and per-project scoped. Comment writes,
  read-only mode and the advanced request tier remain explicit, default-off gates.
  A connected app can only recommend permanent removal while the owner has enabled the same
  default-off capability; the local owner must still complete Code Hangar's immutable preview
  and fresh confirmation flow.

### Reviewed one-file correction

Manual text changes and recognised-value changes are local correction surfaces, not an IDE or a
Git client.

- Project-file changes start locked on every app run and whenever the selected project changes.
  Unlocking requires an explicit acknowledgement and the exact project name. The unlock only
  exposes the correction controls for that project and never changes a file by itself. Opening a
  project, file, session, Recap or Git review does not unlock or enter a write path.
- The UI lock is an additional human-safety barrier, not an authorization boundary. Every write
  remains independently protected by backend project/path authorization, fresh-byte comparison,
  exact reviewed-output hash, verified off-project snapshot and atomic replacement. A frontend
  defect therefore cannot turn project navigation into a write or bypass the backend checks.
- A previously approved controlled project check still requires a fresh, unchecked confirmation
  for every run, with the exact detected command visible. Its warning states that project-code side
  effects outside the checked file may not be reversible. Deleting encrypted local comments also
  requires a separate permanent-delete acknowledgement.
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
  arbitrary files or other side effects produced by that project's test/build process. Offline
  package-manager policy is not socket isolation: arbitrary project code can still open sockets.
- On Windows, Rust starts the fixed executable directly with no free shell surface, null stdin,
  below-normal priority and a cleared/rebuilt environment. Cargo and npm receive fail-closed
  offline flags; Go receives `GOPROXY=off`, `GOSUMDB=off`, `GONOSUMDB=*`, `GOVCS=*:off`,
  `GOTOOLCHAIN=local`, `GOTELEMETRY=off` and `GOENV=off`; pip/uv index access is disabled;
  Git child transports are restricted to local `file`. Package audit, funding and update checks
  are disabled. The exact fixed environment is part of the check fingerprint, so policy drift
  invalidates earlier approvals.
- The child is created with `CREATE_SUSPENDED`, assigned to a Windows Job Object, and resumed only
  after assignment succeeds. There is therefore no pre-assignment execution gap. The job kills on
  close, caps the active process count at 32 and caps job memory at 2048 MiB. A 120-second wall
  timeout terminates the whole job. Only 64 KiB per output stream is retained, and retained output
  passes through the local secret redactor before reaching the webview.
- Only one project check may run at a time. The correction node is re-verified as belonging to the
  approved project, and the check is freshly re-detected before every run. The latest verified edit
  snapshot is exposed as a one-click correction restore after the result.
- These commands are compiled under `mutation`, not strict `core`. The deterministic static report
  also uses the Mutation edition's complete-file validity guard. Code Hangar adds no app-owned
  outbound client here; the approved project subprocess remains user code and is not network-
  sandboxed.
