# Total-control extension — generalized request → human-approve model

Status: **design approved, implementation in waves.** Source: design + 3-lens adversarial
review (workflow `wf_2890cf1a-da2`). This doc is the spec each wave re-audits against.

### Progress (branch `claude/total-control-extension`)
- ✅ **Wave 0** — `agent_request` generalized beyond comments (schema + `NewAgentRequest`
  + DTO + mapper + caller + TS types). Zero behaviour change.
- ✅ **Wave H** (complete):
  - ✅ H3 — killed the `node_project_id → unwrap_or(node_id)` scope-bypass fallback.
  - ✅ H1 — re-authorize the agent at resolve (revoked/disabled/narrowed → refused).
  - ✅ H6 — global read-only panic switch (`mcp_read_only_mode`), enforced at dispatch
    AND at resolve, wired to a Settings toggle.
  - ✅ H4 — **re-evaluated: not needed.** The review assumed `consume()` matched a
    token by action only; in fact `ConfirmTokenStore::consume` matches the EXACT token
    string (`tokens.get(token)`), and `agent_request_resolve` mints + consumes each
    token synchronously within one call (the value never persists or is shared), so
    there is no fungibility window. Keep the invariant by always minting+consuming in
    the same resolve call; no `ConfirmAction` rewrite.
  - ✅ H5 — recovery-state audit completed. Ambiguous disk outcomes remain
    `executing`/`verifying` and block new mutations; terminal `failed` operations are
    allowed only after their file/entry state is reconciled. Interrupted duplicate copies
    are exposed in Recover rather than orphaned, and regressions cover the status contract.
  - ✅ H7 — quarantine-entry project scope, concrete target identity and durable agent
    request/result attribution are enforced at filing and resolve.
- ✅ **Waves 1–4** — read_body + backup_protected + move_to_holding + final_remove
  request kinds: filing arms, generalized `agent_request_resolve` kind dispatch
  (re-auth, cross-scope gate, read-only check, Gate-3 executors as the user), the
  kind-aware six-friction approve UI, MCP tools (`request_*`), and gating tests.
  Final-remove is available but off by default (in-app opt-in, or forced on only by
  `CODEHANGAR_ENABLE_FINAL_REMOVE` during supervised QA); every removal still needs a
  verified backup + a fresh confirmation.
- ✅ **Implementation audit** (`wf_db793eec-d89`, 3 adversarial lenses): **GO verdict —
  no critical/high authorization bypass.** The core invariant held across all lenses
  (agent only queues; executors reached only via the human-only `agent_request_resolve`;
  re-auth, read-only, cross-scope, plan-fingerprint and verified-backup re-proof all
  verified). Four defense-in-depth findings; the two LOW are **fixed**:
  - ✅ LOW — `agent_request_resolve` is now single-shot atomic: it claims the row
    (`pending`→`processing`) before any executor and rolls back to `pending` on error,
    so concurrent approvals can't double-execute.
  - ✅ LOW — `backup_protected`'s "include protected files" opt-in is no longer
    pre-checked/locked; the human must actively tick it (like move-to-holding).
  - ✅ MED — quarantine-entry **scoping + identity**: a `final_remove` request now
    resolves the entry (`quarantine_entry_target`), **refuses a non-existent entry**,
    derives its owning project from `operation.target_node_id` and computes
    `cross_scope` like the other kinds (in-scope → normal; out-of-grant/project-less →
    explicit cross-project authorization), and stores the entry's `original_path` in
    `detail` so the approval card names the concrete target instead of an opaque id.
  - ✅ MED — **durable attribution**: the persisted `agent_request` row (agent id +
    kind + target + path + approved-at) is the durable, non-rotated attribution record,
    and `agent_request_set_result` now also records each action's outcome
    (`backupId`/`moved`/`removedEntry`/`grantedNode`) in `result_json`, linking the
    proposing agent forward to exactly what the app did on the user's behalf. (Done
    without touching the Gate-3 executor signatures, so zero regression risk.)
- ✅ H5 (recovery guard audit) — closed by the v0.1.1 RC hardening pass. Blindly adding
  all `failed` rows to the guard would permanently wedge coherent partial operations;
  instead, executors now reserve blocking states for genuinely unreconciled outcomes.

### Master-spec gaps (the "3 items") — resolution
- **Markdown backlinks: already implemented** (not missing). `node_relationships`
  returns the incoming `markdown_links_to` edges and the InspectorView "Referenced by"
  panel renders them, clickable. The spec's dedicated `markdown_backlinks(node_id)`
  IPC is just an IPC-naming difference from the general `node_relationships`.
- **Connector-links-mutation deviation: signed off** in SECURITY_INVARIANTS.md.
- **Phase-4 watchers (poll/fingerprint vs event-driven `notify`): accepted descope.**
  The truthful stale/dirty-badge intent is met on demand; live background change
  events are a deliberate non-goal for the read-only-first product (revisit only if
  real-time drift detection becomes a requirement).

## Goal

"Total control" means a trusted connected AI app may **request** any main app function it
otherwise can't reach — read a file body, build/preview a plan, run the Gate-3 mutations
(backup / move-to-holding / permanent-delete). The agent **never executes anything**: it
files a pending request; the app performs the action **as the user** (`actor="user"`) only
after an in-app human approval. This generalizes the shipped M7 `request_comment_change` →
`agent_request_resolve` flow.

## Friction model

- **Non-invariant-breaking reads** stay autonomous (no request): structure, graph,
  comments-read, redacted session search, plan *preview* (build only), and `read_body`
  *when the agent holds the standing scope*.
- **Invariant-breaking actions** (empty-with-secrets backup, move-to-holding,
  permanent-delete) require, on approval, the **six-friction gate**:
  1. paired **double confirmation** (the two confirmations must pair/match),
  2. **multiple warnings**,
  3. **liability waiver** ("desresponsabilização") — a required checkbox,
  4. an **explicit recommendation AGAINST** proceeding (danger styling + its own checkbox),
  5. an **easy backup** option (recommended-default checkbox + folder picker),
  6. a **read-only alternative** ("approve as read-only preview" → resolves without executing).
- **Cross-scope rule (user decision):** an action whose target belongs to a project the
  agent was NOT granted, or to no granted project at all (e.g. a project-less quarantine
  entry), is **not refused** — it is allowed with an **extra cross-project authorization**
  step on top of the gate above. In-scope actions use the normal gate.

## Request kinds

| kind | breaks invariant | autonomous | gate |
|---|---|---|---|
| `read_body` | no | yes (with `read_body` scope) | else: request → approval mints a short-lived per-(agent,node,project) read grant |
| `plan_preview` | no | yes (`build_plan`) | never queued; produces the OperationPlan + fingerprint mutations carry |
| `backup` (include_protected=false) | no | yes (`execute_plan`) — **but refused when global read-only is on** | reversible |
| `backup_protected` (include_protected=true) | yes | no | six-friction gate; protected paths surfaced server-side |
| `move_to_holding` | yes | no | six-friction gate; verified backup is a precondition |
| `permanent_delete` | yes | no | highest gate: typed-phrase confirm + the final-remove setting (in-app opt-in, default OFF, or the `CODEHANGAR_ENABLE_FINAL_REMOVE` supervised-QA env var) |
| `comment_edit` / `comment_delete` | no | no | existing M7 flow, unchanged |

## Wave-H security status

This section used to be the pre-implementation "must-fix" list. It is now a completed
status record; H5 was closed by the v0.1.1 RC recovery-state audit above.

- ✅ **Confirm-token fungibility: re-evaluated as not exploitable in this design.**
  `ConfirmTokenStore::consume` matches the exact random token string, and
  `agent_request_resolve` mints and consumes mutation tokens synchronously inside
  one approval path. Keep that invariant: do not persist or reuse approval tokens.
- ✅ **Re-authorize at resolve.** `agent_request_resolve` reloads the live agent,
  re-checks scope/project grants, and refuses revoked, disabled or narrowed agents
  before claiming a request.
- ✅ **No `node_project_id → unwrap_or(node_id)` fallback.** Targets are resolved
  through real navigation membership; cross-scope/no-project cases are explicit.
- ✅ **Final-remove request scoping and identity.** A final-remove request resolves
  the held entry before approval, records the concrete original path in the
  request detail, and applies the same in-scope/cross-scope approval model as the
  other privileged requests.
- ✅ **Human-picked destinations.** Agents queue requests only; backup and holding
  destinations are supplied by the human approval UI, not by the agent payload.
- ✅ **Global read-only panic switch.** `mcp_read_only_mode` is persisted and
  enforced at filing and resolve for mutation-capable request kinds.
- ✅ **Protected-content opt-in is server-validated.** `resolve` rebuilds and
  revalidates the plan; including protected/sensitive files requires the human's
  explicit opt-in and the same Gate-3 executor checks as direct in-app actions.
- ✅ **Durable attribution.** The `agent_request` row records the proposing agent,
  target, approval status and result JSON, linking a human-approved action back
  to the app that requested it without rotating Gate-3 executor signatures.
- ✅ **Recovery guard contract.** `ensure_no_pending_recovery` blocks the three
  running/interrupted journal states used by crash recovery. Terminal `failed` rows do
  not block only because each executor first reconciles physical truth: moved entries stay
  visible, post-move restore warnings mark the entry restored, failed source unlinks claim
  zero recovered bytes, and any outcome with neither copy visible stays `verifying`.
- ✅ **Move-time backup coverage.** Move-to-holding reloads the verified backup and
  recomputes the concrete plan items before moving, so a stale or incomplete
  backup cannot cover a different set.
- ✅ **Atomic single-shot resolve.** Requests are claimed with a pending →
  processing transition and released back to pending on executor error, preventing
  concurrent approvals from double-running an action.
- ✅ **Read-body grants are bounded.** Temporary file-body grants are scoped to the
  requesting app, node and project, and are rechecked before use.

## Data model

Single `agent_request` table, widened additively (nullable columns; comment columns retained
for back-compat): `target_kind`, `target_id`, `payload_json`, `result_json`, plus the resolved
`project_id` and a `cross_scope` flag. Reviewer-context enrichment moves from a fixed comment
JOIN to kind-dispatched enrichment in Rust. `payload_json` is a serde-tagged per-kind struct
carrying the app-built `OperationPlan` (with `target_fingerprint`) — never agent-supplied.

## Rollout waves

0. **Storage refactor** (dark, no behaviour change): generic columns + generalized
   `agent_request_create` + kind-dispatched enrichment; comment round-trip test + UI unchanged.
- **H. Hardening**: authorization, scoping, read-only, attribution and recovery-state
  fixes above. Complete for this extension.
1. **`read_body` request** (read-only) + global `mcp_read_only_mode` switch + kind-aware
   request card / "approve as read-only preview" / `previewed` status.
2. **`backup_protected`** + the `StrengthenedApproveDialog` (six frictions) + cross-scope auth.
3. **`move_to_holding`** (reversible mutation; verified-backup precondition).
4. **`permanent_delete`** (irreversible; typed-phrase confirm; runtime opt-in).
5. **Hardening/QA**: audit coverage, per-kind create→resolve contract tests + negative tests
   (stale fingerprint rejected, move refused without verified backup, read-only refuses queued
   mutation at resolve, agent can't set include_protected without server-proven preview,
   cross-scope requires extra auth, revoked agent refused at resolve).

Each wave: implement → `local-ci.ps1 -AgentAutomation -SkipTauriBuild` green → adversarial
re-audit against this doc → commit.
