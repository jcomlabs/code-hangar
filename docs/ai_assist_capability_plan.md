# AI Assist capability — execution-ready plan (connector edition)

Status: **implemented and locally release-gated in the working tree (2026-07-17).** The opt-in "AI Assist" capability lets the **connector edition** explain files and reconstructed changes in plain language for vibe coders. The primary path is a user-configured local loopback model; Off remains the default and an explicitly labelled external API remains secondary. This is the app's only outbound-network capability, isolated to the add-on edition and never sending sensitive files. Provider/model entries are quick-fill presets, never stored defaults or privileged runtime paths; the OpenAI preset now fills the GPT-5.6 alias while the model field remains editable. Remaining release blockers are external distribution evidence: signing, a clean environment that permits the chosen signed/unsigned installer, and remote upload/download hash verification.

> **Current retrospective layer (2026-07-13):** a Connector-only `RecapAiLayer` adds Story, Learn-from-one-edit and What-to-check over the deterministic Recap. Rust reconstructs the Recap again from authorized local session/Git/ledger/file-state evidence, excludes sensitive/Protected paths, applies a 60 KiB edit-boundary cap, re-runs the secret gate, and only then calls `hangar_ai::explain`. The webview cannot supply a diff body. Each reality label is compare-only and timestamped. A pre-send character/token estimate is shown before Generate. The Local frontend contains no component, CSS chunk, endpoint or IPC marker for this layer; `check-frontend-edition.mjs` proves both absence and Connector completeness.

> **Current guided-reading layer (2026-07-14):** the Connector-only `AiLearningTools` is closed by default and keeps four aids independent: a language-aware section walkthrough, a three-turn section-scoped follow-up held only in memory, an opt-in canonical seed glossary, and local content-hash anchored notes. Rust maps every section in a freshly authorized file up to 2 MiB, while each selected provider batch remains below 60 KiB and is rebuilt from exact section ids. Walkthrough and follow-up repeat the sensitive/secret gate and show their estimated input size and destination before send. Notes are disabled for truncated/revealed/sensitive previews. The Local bundle contains no learning component, CSS, command name or endpoint marker.

> **Current light-correction layer (2026-07-13):** deterministic Values now recognises exact-span strings, plain numbers, booleans and CSS hex colours in common JS/TS, Python, Rust, Go, C-family and CSS source files as well as JSON/TOML. Every value save uses full-file CAS, a source/format validity re-check and a verified off-project snapshot. Connector suggestions are selection-only: the full file is capped and gated, the selection must be unique, provider output is an opaque in-memory proposal, and Rust alone performs the confirmed byte splice after another CAS/validity check. Whole-file rewrite was removed. Suggestions carry a durable edit-session id, and the first verified snapshot can undo that session after the dialog closes.

> **Current correction-check layer (2026-07-14):** deterministic static analysis re-parses the complete authorized file and checks local Markdown links plus indexed relationships without executing code. Optional project checks are detected from bounded manifests and expose only fixed `npm`, Cargo, Go or pytest commands. They require explicit approval bound to the exact project/check/manifest fingerprint, re-detect before every run, and execute directly under a Windows Job Object with a 120-second timeout, 2048 MiB job-memory cap, 32-process cap, below-normal priority, filtered environment, offline package-manager flags, capped/redacted output and one concurrent run. The UI discloses that this is not a sandbox and that restoring the verified correction snapshot cannot undo arbitrary side effects from project code.

> **Current local-model layer (2026-07-14):** Settings can explicitly probe four fixed numeric `127.0.0.1` OpenAI-compatible ports with a two-second timeout, no proxy and no DNS discovery; a result remains a draft until the user chooses and saves it. Explain/What-to-check streams bounded SSE deltas from local providers, with one disclosed non-stream fallback only when the local server cannot stream. External APIs retain one complete-response request so compatibility handling cannot duplicate a paid call. "Exactly what is sent" reconstructs the current gated prompt and displays the literal URL and compact JSON bodies without credentials. The response ceiling is 2 MiB. A malformed small-model review stays visible as plain text rather than being discarded.

> **Local ship phase complete; external evidence remains:** the cross-operation usage meter, advisory soft cap, final journey/accessibility pass, release documentation and sequential Local/Connector packaging are implemented and exercised. The current installers remain unsigned, the active Windows Sandbox Application Control policy refuses unsigned setup before launch, and these exact bytes have not been uploaded/downloaded through the remote draft. A direct installed Claude/Codex process engine is deliberately not part of this release: it is optional in the roadmap, would be cloud-capable, and requires a separate product/security decision rather than a disabled teaser or hidden subprocess path.

> **Built state (provider-agnostic, 2026-07-14):** `crates/hangar-ai` stores any API key in the Windows Credential Manager (via `keyring`) and calls only the **configured** provider via two adapters — **Chat Completions–compatible** `/v1/chat/completions` and **Messages-API–compatible** `/v1/messages`. The mode/base_url/model/format live in encrypted settings (default mode `off`); the key is never stored there. A `local` provider is loopback-enforced and keyless; an `api` provider sends the saved key. `crates/hangar-api/src/ai_assist.rs` owns the provider-independent send-gate: sensitive-path block + secret hard-block + operation-specific cap, re-run server-side before every send and disclosure. `cargo tree --features mutation` confirms the **Local edition links none of `hangar-ai`/`reqwest`/`keyring`**. Provider plumbing, Explain/review streaming, literal payload disclosure, local discovery/model listing, project summary, retrospective Recap, guided reading and the process-session usage meter/advisory soft cap are implemented.

> **⚠️ Note for the owner:** the connector edition therefore CAN reach the network once an API key is added and "Explain this" is used. The Local edition cannot (CI-enforced). This matches the plan's design: network only in the opt-in add-on.

## Two independent modes (owner's framing)

The **AI Connector edition** has **two modes that can be used independently**:

- **MCP-out** — the app *teaches the agent* what it can do with the catalog (the existing connector: read structure, comments, request→approve, etc.). Knowledge flows **out** to the user's AI apps.
- **API-in** — the app *fetches intelligence from outside* (the user's own API key) and folds it into its **own** features: "Explain this" on a selected snippet, "Explain this file" in context, project summaries. Intelligence flows **in**. The data does not leave to be useful to someone else — it comes back as understanding inside the app.

Both modes are opt-in and toggled separately. A user may run MCP-out only, API-in only, or both.

### Zero-trace isolation (hard requirement — "no FOMO")

Whoever installs the **Local edition** (no add-on) must see **no trace whatsoever** of either mode — no AI panels, no "Explain this" menu items, no disabled/greyed buttons, no "upgrade to unlock" teasers, no settings stubs. The feature must be **invisible**, not merely disabled. This is enforced structurally (the code is physically absent from the Local build) AND in the UI (every entry point is gated on `activeFeatures`, and gating means *not rendered*, never *rendered-disabled*). The "Explain this" context-menu items only exist when the API-in capability is compiled in and enabled.

## Market read — does this already exist?

**No** — not at Code Hangar's exact intersection. The pain is real and timely (2025–26 "I vibe-coded an app and I'm scared to touch it" discourse). The neighbours each miss:

- **IDE `/explain` / `@codebase`** — Copilot Chat, Cursor, Cody, Windsurf, Tabnine, **Aider** (free, BYO-key, local — the spiritual closest): all editor/one-repo bound, developer-framed, assume you know which repo to open.
- **Repo comprehension** — **DeepWiki** (great whole-codebase explain + an MCP precedent, but **public GitHub only**, cloud), CodeViz, Greptile, bloop, Swimm, CodeSee: single-repo, Git-remote-centric, engineer/team-framed.
- **Pedagogical tone** — *Tutorial-Codebase-Knowledge* (beginner tutorial from a repo; reviewers found it too childish, single GitHub repo), **Workik** (plain-language but a cloud copy-paste pasteboard, no project memory).
- **BYO-key desktop chat** — LM Studio, Cherry Studio, Jan, Chatbox: store keys in **plaintext** app config; no notion of a project catalog or sensitive zones.

**The unoccupied intersection (our opportunity):**
1. **Cross-project, not single-repo** — we start from a local cross-tool catalog of the dozens of half-finished AI projects scattered across the disk + WSL, and can explain *any* file from *any* of them. Nobody else has this inventory (our discovery graph is a moat).
2. **Local-first + never-send-sensitive as a hard, CI-enforced guarantee** — privacy-credible tools aren't pedagogical; pedagogical ones are cloud.
3. **Vibe-coder audience stated explicitly**, with a level slider (vibe-coder ↔ engineer) to avoid the "too childish" failure.
4. **README → one-paragraph "what this project does"** at the catalog level for never-pushed local folders — no direct competitor.

**Positioning line:** *"DeepWiki for the private, half-finished AI projects rotting on your own disk — explained in plain language, with your own key, nothing sensitive leaving your machine."*

**Honest wedge risk:** a vibe coder may just ask Cursor/Claude already open in their editor. Our defensible wedge = the projects they **forgot** + the no-editor, plain-language, privacy-guaranteed framing.

## Capabilities it unlocks

| Capability | What | Notes |
|---|---|---|
| **Local project summary** | README + top-level `*.md` + manifest (package.json/pyproject/Cargo) + shallow file tree → a 3-sentence "what / stack / how to run" card | Realizes feature **#3**; works **with no network** in the Local edition; optionally AI-enriched in the connector |
| **Explain-this-file (the headline)** | Per-file plain-language explanation pinned to a "explain to someone who built this with AI and doesn't know the language" persona; streamed; **level slider** | The category nobody ships, tied to our catalog |
| **Usage preview + advisory soft cap** | A bounded character-based estimate before each send, plus process-session input/output estimates and a configurable warning threshold | Makes BYO-key usage visible without pretending to know provider tokenization or price |
| **Model picker** | a cheaper model for summaries / a stronger model for deep explain — whatever your provider offers, BYO key | Match spend to need |
| **Pre-send secret scan + Protected-Zone gate** | Reuse the existing sensitive/Protected-Zone classifier as the first gate, then a gitleaks-style regex set + `.gitignore`-aware exclusion; **hard-block** on a hit | A guarantee no generic BYO-key chat client can make |
| **OS-keychain key storage** | Key only in Windows Credential Manager (DPAPI) via the `keyring` crate; resolved Rust-side; **never crosses to the webview**, never in SQLite/logs | Stronger than the plaintext-config incumbents |

## Architecture (mirror the existing two-edition split)

1. **New connector-only crate `hangar-ai`** — the outbound provider client (Chat Completions– and Messages-API–compatible). Added to the `agent_automation` feature graph alongside `hangar-mcp`; **never** to the `core` or Local-edition graph. Ships only in the add-on edition.
2. **Context-building is shared and network-free** — the cheap local context (README + `*.md` + manifests + shallow tree, and for explain just the one target file) is built by existing local code both editions have. The Local edition uses it for a **non-network heuristic summary** (#3); only `hangar-ai` turns it into an outbound call. Never dump the whole repo.
3. **Base stays network-free** — no member of the core tree depends on `hangar-ai`; asserted by `scripts/check-no-outbound-deps.mjs` (`cargo tree --features core`).
4. **The CI no-network guards get a scoped, feature-gated exemption** (exactly like the connector got, narrower):
   - `check-no-outbound-deps.mjs`: allow the HTTP/TLS client to be declared **only** in `crates/hangar-ai/Cargo.toml`; keep the `--features core` tree check proving `hangar-ai` + its HTTP/TLS crate are **absent** from the base graph; add `hangar-ai` to `forbiddenBasePackages` as a backstop.
   - `check-no-forbidden-code.mjs`: add `crates/hangar-ai` to a narrow allow-list dir (like `agentSurfaceDirs`), lifting **only** the outbound-HTTP ban there; telemetry/mutation/remote-Git/updater stay forbidden even there.
5. **`SECURITY_INVARIANTS.md`** gets an "Outbound AI Assist (connector-only, opt-in)" section modeled on the MCP one.
6. **IPC boundary** — the key is resolved Rust-side (keyring) and never crosses to JS; the webview sends a node/file id + options, Rust assembles context, runs the send-gate, calls the configured provider, streams deltas back. Same "Rust owns the secret" discipline as the MCP token.

## Privacy model (the load-bearing differentiator)

1. **Edition isolation** — `hangar-ai` only in the connector edition; base has no outbound capability (CI-enforced); the AI Assist UI is gated on `activeFeatures` like the AI App Integration panel.
2. **Per-use opt-in, not a global switch** — each Summarize/Explain click is an explicit "send this file to the configured provider" with a **literal byte-level preview** of exactly what leaves the machine. A first-run consent gate enables the capability; each send still shows the preview.
3. **Never send sensitive / Protected-Zone files — enforced, not trust-based** — reuse the existing classifier as the first gate, then a gitleaks-style secret scan (AWS/GitHub/Stripe and AI-provider API keys, private-key blocks, JWTs, raw `.env`), `.gitignore`-aware exclusion. **Hard-block** on a hit; offer redact-and-resend. Threat model: the vibe coder least recognizes a credential in their own AI-generated code.
4. **Per-call transparency + usage** — bounded character-based input estimates, a process-session usage meter and an advisory soft cap. No prompt/response body is persisted and Code Hangar does not invent provider prices or exact token counts.
5. **Key storage** — only in Windows Credential Manager (DPAPI) via `keyring`; test-key + remove-key; masked in the UI.

## Phasing

0. **Phase 0 (base, no network) — ✅ DONE (shipped as feature #3):** local heuristic project summary from README/`*.md`/manifests/tree (`crates/hangar-api/src/project_summary.rs` → `ProjectContextSummary`, surfaced by `ProjectSummaryCard`). It is the shared, network-free context layer Phases 1–3 reuse. Ships in both editions.
1. **Phase 1 (connector plumbing):** `hangar-ai` crate behind `agent_automation`; OS-keychain BYO-key storage (key never crosses IPC); extend both CI guards + add the SECURITY_INVARIANTS section. No user feature yet — just the gated, CI-proven foundation.
2. **Phase 2 (single-file Explain, opt-in):** pedagogical explanation with the pinned persona, streamed; first-run consent + per-call byte preview; Protected-Zone + secret hard-block; bounded usage estimate + advisory soft cap; metadata-only usage logging.
3. **Phase 3 (AI-enriched catalog summary):** upgrade the Phase-0 local summary to an optional one-call provider summary per project, same context + gates.
4. **Phase 4 (depth + polish):** level slider, prompt caching on the stable persona once it exceeds the cacheable-prefix minimum, model picker, optional "what would break if I deleted this" framing.

## Key risks

- **Privacy perception (highest)** — this is the first outbound capability and the brand is "100% local". Mitigate: hard-block (not warn) on secrets, literal send preview, metadata-only logging, CI-proven edition isolation, keep base literally network-free.
- **Bill shock** — cost-sensitive audience. Mitigate with bounded pre-send estimates, a configurable advisory soft cap, explicit local/API labelling and user-selected models. No provider or model is privileged by default.
- **Output quality** — too childish vs too jargony, hallucinated terms. Mitigate: level slider, tight rubric, one few-shot, "flag risky lines" framing.
- **Scope creep** — "explain a file" → "chat with my whole codebase" → "let it edit" re-opens all the mutation risk. Mitigate: keep AI Assist strictly **read-only / explanation-only**; assert it never routes into plan-build/execute/mutation (like the connector tool-list test).
- **CI-exemption risk** — widening the network allow-list is the most dangerous guard edit. Mitigate: directory-scope it to `hangar-ai` only; keep the base-tree assertion as the backstop.

## Turn-key execution checklist (Phase 1, when greenlit)

Concrete edits so the first build is mechanical, not exploratory:

1. **`crates/hangar-ai/`** (new): `Cargo.toml` declaring the lone HTTP/TLS dep (e.g. `reqwest` with `rustls-tls`, no `openssl`); `src/lib.rs` with `pub fn explain(...)`/`pub fn summarize(...)` taking already-assembled context text + options, returning a stream. **No file/DB/path access in this crate** — it only takes text in and returns text out (keeps the send-gate upstream, in `hangar-api`).
2. **`crates/hangar-ai/Cargo.toml` membership**: add to the workspace; pull it into `hangar-api` **only** under `agent_automation` (`hangar-ai = { ..., optional = true }`, `agent_automation = ["dep:hangar-ai", ...]`). Never under `core`/`mutation`.
3. **Key storage** (`hangar-api`, `agent_automation`): `keyring`-crate wrapper `ai_key_set/ai_key_status/ai_key_remove` → Windows Credential Manager; never returns the key to JS. Tauri commands mirror the MCP token pattern (registered only in the `agent_automation` invoke_handler block).
4. **Send-gate** (`hangar-api`, `agent_automation`): `ai_assist_preview(node_id, opts)` assembles the one-file context, runs the existing sensitive/Protected-Zone classifier **then** the secret-pattern gate, and returns either block reasons or the literal bounded request with a character-based input estimate. `ai_assist_explain` re-runs the gate server-side before calling `hangar-ai`.
5. **CI guards**: in `scripts/check-no-outbound-deps.mjs` allow the HTTP dep only when its manifest path is `crates/hangar-ai/Cargo.toml`, keep the `--features core` tree assertion that `hangar-ai` + the HTTP crate are **absent**, add `hangar-ai` to `forbiddenBasePackages`. In `scripts/check-no-forbidden-code.mjs` add `crates/hangar-ai` to a narrow allow-list lifting **only** the outbound-HTTP ban (telemetry/mutation/remote-Git/updater stay banned even there).
6. **Frontend** (gated on `activeFeatures` / `connectorBuild`, **not rendered** otherwise): an "Explain this" item in the file/selection context menu and a per-call dialog showing the literal send preview, bounded usage estimate and block reasons; key management in the existing AI-App-Integration settings panel. Zero entry points compile/render in the Local edition.
7. **Tests**: a `hangar-api` test that the send-gate hard-blocks a synthetic `.env`/key; a tool-surface test asserting AI Assist never routes into plan-build/execute/mutation; `cargo tree --features core` proves no `hangar-ai`/HTTP crate; `scripts/local-ci.ps1` green.
8. **Docs**: add the "Outbound AI Assist (connector-only, opt-in)" section to `SECURITY_INVARIANTS.md`.

Acceptance: the base installer is network-free by dependency/code-surface gates, the connector edition can Explain one file with the user's key behind literal request disclosure, a secret hard-block and an advisory usage warning, and the key never crosses the IPC boundary.
