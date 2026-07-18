# Technical proof matrix

Code Hangar's core technical decision is a strict ordering:

```text
local deterministic evidence
        ↓
coverage and unknowns
        ├── scoped MCP grant → GPT-5.6 in Codex → audited explanation
        └── literal request → secret gate → optional AI-in provider
                                                   ↓
                                      small reviewed correction
                                                   ↓
                                   snapshot, validation, restore
```

GPT-5.6 is not allowed to become the record of what happened. It explains a
bounded record that Code Hangar has already assembled and disclosed.

## Claim-to-evidence matrix

Status vocabulary:

- **Repository-verifiable:** implementation and tests can be inspected locally.
- **Locally verified:** regenerated from the frozen product candidate and
  recorded in the evidence manifest.
- **Pending external proof:** requires a public submission action or the final
  native recorded journey.

| Claim | Status | Repository evidence | Final evidence required |
|---|---|---|---|
| Review starts from local session, Git, current-file, and saved-review evidence | Locally verified | Review/recap code and regression suites | [Final manifest](EVIDENCE_MANIFEST.md) |
| Coverage and unknowns precede AI explanation | Locally verified | Recap UI/tests and synthetic What changed capture | Public demo segment still pending |
| AI Assist starts Off and requires explicit configuration/send | Locally verified | Connector defaults, UI tests, backend preview/send split, installed Connector UI inspection | Final manifest; public demo pending |
| Code Hangar exposes only project-scoped, body-limited MCP context to Codex | Locally verified | MCP tool surface, scope filtering, token/auth audit tests | Subscription smoke plus final native demo |
| GPT-5.6 Sol consumed Code Hangar context through the installed candidate MCP sidecar under ChatGPT sign-in | **Locally verified** | `scripts/submission/codex-gpt56-mcp-smoke.ps1 -ServerPath <installed-sidecar>` | Sanitized report, installed sidecar hash, and matching audited `list_catalog` / `project_context` reads |
| Direct OpenAI GPT-5.6 requests use the current Chat Completions token field | Locally verified | `crates/hangar-ai` serialization and contract tests | Final manifest |
| Other Chat-Completions-compatible endpoints retain their existing contract | Locally verified | Provider-compatibility regressions | Final manifest |
| The in-app direct OpenAI API route returned a separately billed live answer | **Not run; optional** | Contract tests are not called live proof | Reviewer-owned API key only if this secondary route is demonstrated |
| Code Hangar ships a ChatGPT-authenticated app-server inbound adapter | **Not claimed** | Official interface and architecture plan only | Future general-product implementation and tests |
| Secret-like content and Protected Zones are blocked before send | Locally verified | AI preview/send gate and synthetic secret regressions | Final manifest; public demo pending |
| Local edition contains no AI/provider/MCP frontend surface | Locally verified | Edition build checker, dependency/forbidden-code gates, installed Local sidecar absence, and native UI inspection | Final manifest |
| Corrections are small, exact-diff reviewed, snapshotted, and restorable | Repository-verifiable | Edit preview/CAS/snapshot/restore code and tests | Native disposable-data demo pending |
| Final Connector and Local installers match recorded hashes | Locally verified | Packaging/checksum scripts | Final manifest and local `SHA256SUMS` |
| Exact 0.1.2 host install, native launch, edition UI, and uninstall lifecycle succeeds | **Locally verified with disclosed limitations** | Final installers plus isolated application profiles; pre-existing catalog verified unchanged | Sanitized lifecycle summary in the final manifest |
| Candidate installs on a clean supported Windows system | **Blocked before product execution; not verified for 0.1.2** | Exact Local and Connector hashes attempted separately with `scripts/submission/sandbox-candidate-lifecycle.ps1` from empty, network-disabled guests | Guest Application Control blocks both unsigned setups; use signed bytes or another disposable VM |

## GPT-5.6 path A — subscription-backed MCP out

Code Hangar's MCP server is a shipped Connector sidecar. It runs over stdio,
uses a per-app token, reuses the same scope/project/audit policy as local agent
automation, and does not receive the host application's model credential.
Codex owns ChatGPT OAuth, subscription entitlements, model execution, and the
model response. Code Hangar owns curation, project grants, and audit.

The acceptance proof completed on 18 July 2026:

- Codex CLI `0.144.1`, already signed in with ChatGPT;
- explicit model `gpt-5.6-sol`;
- final installed Connector sidecar with SHA-256
  `6e8c2bd602977a456d24c094972a816a9f48bb4f19110e1eb7535e0401b4e97d`;
- disposable Code Hangar catalog and temporary Codex client grant;
- successful `list_catalog` and `get_project_context` calls;
- exact synthetic project returned in the final model answer;
- both reads present in Code Hangar's audit log;
- `--ephemeral` execution from a system temporary directory with zero new Codex
  rollout files; and
- disconnect/revocation completed before the fixture was removed.

Only the sanitized result under
`.local/acceptance/gpt56-mcp-installed-20260718-01/` was retained. The path is
gitignored and the result declares that it contains neither a credential nor
personal project data. The tracked reproduction script is
`scripts/submission/codex-gpt56-mcp-smoke.ps1`; `-ServerPath` makes the selected
installed executable and its hash part of the proof.

The local CLI's `gpt-5.6` alias produced a stale-model-cache warning and was
rejected by `codex exec` with ChatGPT auth, while explicit `gpt-5.6-sol`
succeeded. The proof pins the explicit model and records the CLI version instead
of hiding that compatibility finding.

## GPT-5.6 path B — optional AI in transport contract

The Connector remains provider-compatible, but treats the direct official
OpenAI GPT-5.6 path explicitly:

- OpenAI base origin: `https://api.openai.com`
- model selection: `gpt-5.6` or an explicit GPT-5.6 snapshot/variant
- API family: Chat Completions
- output-token field: `max_completion_tokens`
- generic compatible endpoints: retain their established `max_tokens` request
  field unless their own configuration says otherwise
- system instruction role: preserved for compatibility with the existing
  provider abstraction

This contract follows the official
[GPT-5.6 model page](https://developers.openai.com/api/docs/models/gpt-5.6-sol),
[latest-model guide](https://developers.openai.com/api/docs/guides/latest-model),
and [Chat Completions reference](https://platform.openai.com/docs/api-reference/chat/create).
The `gpt-5.6` alias routes to GPT-5.6 Sol; the repository still treats the
configured model string literally and does not rewrite a user-selected explicit
variant.

The request-body disclosure shown to the user must be generated from the same
serialized request contract used by transport. A UI-only string is not proof.

Any optional direct-API evidence bundle must record:

1. the exact candidate commit;
2. the focused request-construction tests;
3. the redacted request shape (never the API key or project-sensitive content);
4. HTTP success metadata and returned model identifier where available;
5. a screenshot or short recording of the real response in Code Hangar;
6. a clear distinction between contract proof, subscription-backed MCP proof,
   and the separately billed direct API call.

Direct-API live proof status: not run and no longer required for the primary
submission path. Explicit owner authorization, key, and spending scope are
still required if this secondary route is recorded.

## Subscription-backed AI in extension boundary

OpenAI documents Codex app-server as a local JSONL/stdio interface for embedding
authentication, threads, approvals, and streamed agent events in another
product. Code Hangar does not yet spawn or speak app-server, so this remains a
future Connector-only adapter, not candidate functionality. Code Hangar must
never read or copy Codex's cached OAuth credential to implement it.

## Privacy and safety boundary

### Before model transport

- The user selects the evidence; Code Hangar does not upload the project.
- Strong Protected Zones remain ineligible.
- Secret-pattern detection fails closed.
- The literal bounded request is displayed before a separate send action.
- Credentials are stored through the native credential path and never returned
  to the frontend as plaintext.

### After model response

- The answer is treated as advisory explanation, not evidence.
- No automatic project mutation follows from a response.
- A correction must target the bounded supported surface and show an exact local
  preview.
- The applied bytes are bound to the reviewed state, snapshotted, and
  restorable.
- Code Hangar exposes no stage, commit, branch, push, or whole-project AI rewrite
  operation.

## Edition-isolation proof

Run from a clean candidate checkout:

```powershell
npm run check
npm --workspace apps/desktop run build:connector
npm --workspace apps/desktop run build:local
powershell -ExecutionPolicy Bypass -File scripts/local-ci.ps1 -AgentAutomation
```

The proof is stronger than checking whether a button is hidden:

- Connector-only strings and commands must exist in the Connector build.
- The Local edition checker must reject those same markers.
- The strict core dependency graph must exclude the outbound AI crate and HTTP
  client.
- Security checks must keep telemetry, updater, remote Git, and other outbound
  surfaces forbidden in both editions.

Final gate output: [candidate evidence manifest](EVIDENCE_MANIFEST.md).

## Visual fixture proof

The prepared [What changed](assets/01-what-changed.jpg),
[Hangar Map](assets/02-hangar-map.jpg), and
[GPT-5.6 preset](assets/03-gpt56-preset.jpg) screenshots use synthetic
`fixture://` data. They validate deterministic UI states only. Their
[provenance record](assets/README.md) explicitly excludes a live-model claim.

## Build and artifact manifest

Do not copy historical hashes into the final submission. Generate both packages
from the final candidate commit, Connector first and Local last, then record:

| Evidence | Final value |
|---|---|
| Candidate commit | `e831c14dfa15291dda152d7742766221438feaa3` |
| Working tree | Clean at candidate before packaging/evidence generation |
| Full local gate | [PASS summary](EVIDENCE_MANIFEST.md) |
| Connector filename | `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` |
| Connector SHA-256 | `ffa66b3033ac4cd51e017bb2592f9e37dcbc8f688faff9f82f10a065d926d241` |
| Local filename | `Code-Hangar_0.1.2_x64-setup.exe` |
| Local SHA-256 | `52288762d0de48403cd545852374178bf6cb72815f0c1c7c08d14fb0ee521a47` |
| Authenticode state | `NotSigned` for both installers |
| Exact host lifecycle | Passed for both 0.1.2 editions with isolated profiles; real catalog unchanged; generated Connector WebView2 residue disclosed |
| Clean disposable-machine install | Exact Local and Connector candidates attempted separately on 18 July; both blocked before setup/product execution by guest Application Control; not verified |
| Demo video | Pending recording and owner-authorized publication |

Any unsigned status or uncompleted clean-machine check must be disclosed plainly;
it must not be converted into a passing claim by documentation.

## Evaluation criteria mapping

The [official Build Week rules](https://openai.devpost.com/rules) weight all
four criteria equally.

| Criterion | What to inspect |
|---|---|
| Technological Implementation | Evidence reconstruction, scoped/audited MCP with subscription-backed GPT-5.6 proof, optional provider contract, secret gate, edition isolation, reversible edit pipeline |
| Design | Evidence-first hierarchy, explicit uncertainty, literal send review, small correction scope |
| Potential impact | Makes AI-assisted development reviewable for non-experts without requiring a new IDE or hosted account |
| Quality of the Idea | A flight recorder for vibe coding: retrospective accountability instead of another autonomous generator |
