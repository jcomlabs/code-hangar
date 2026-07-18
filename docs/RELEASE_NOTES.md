# Code Hangar v0.1.1-rc1 — release candidate notes

Release candidate updated 15 July 2026. **Do not publish until the external
signing/unsigned decision, clean-install lifecycle and remote hash round-trip in
the acceptance report are complete.**

**Code Hangar is a local-first control centre for the projects and chat sessions your AI coding tools leave on your machine.** It finds them, helps you understand them, and lets you clean up safely — on your PC, with no account and no telemetry. The Local edition has no outbound network capability; the AI Connector edition can call only the provider or local model server you explicitly configure.

This is an early **preview (alpha)**. The core navigator, discovery, safety layer and the optional AI connector are built and tested, but expect rough edges and breaking changes before 1.0. Windows only for now.

## What changed since v0.1.0

- A complete retrospective workflow now leads with **What changed -> understand it -> make one safe tweak**. Overview includes a cross-project **Review Inbox** built from local session records and per-project review checkpoints; Recap defaults to evidence since the last review checkpoint.
- Recap reconstructs supported ChatGPT, Claude and Cursor edits, current local Git changes, compare-only current-file reality and a bounded encrypted ledger. Coverage and unknowns appear before reconstructed diffs. In the Connector edition, the selected combined/session/Git evidence can be explained in plain language by the configured AI; the user must explicitly request the send.
- Beginner help is now available at the point of use through discreet `?` controls. What changed separates its overview, current project files, AI conversations by app and older reviews; Git, commits, branches, evidence strength, sessions, scans, recovery and local automation are explained without requiring programming knowledge. Technical source details remain available through explicit expansion.
- Recap can export a self-contained **Review Receipt** with aggregate evidence, coverage, observed-state and parser counts. Project identity, prompts, transcript text, diffs, file names and local paths are omitted, and the receipt explicitly avoids security/readiness certification.
- System & diagnostics can export a small redacted JSON support bundle. It omits project/file identity, paths, evidence content, logs, endpoints, credentials and model configuration by construction.
- Source previews now use safe deterministic syntax highlighting. Connector users can switch between plain-language **Explain** and question-led **What to check**, with exact request disclosure before sending.
- Optional, collapsed learning aids add bounded file walkthroughs, section-grounded follow-up, an opt-in term-only glossary and locally anchored notes without turning Code Hangar into a coding agent.
- Light correction now covers recognised source/config values, an advanced one-file text draft and one explicit AI-selected passage. Project-file controls start locked on each app run and project switch, and unlocking requires an acknowledgement plus the exact project name. Manual and value edits must pass a backend-generated line review with local validation and passive Git context; the reviewed hash is bound to the exact bytes applied. Previous versions must be compared before Restore becomes available. Every accepted change still uses whole-file CAS, a verified off-project snapshot, atomic replacement and durable undo; Code Hangar exposes no stage, commit, branch, push or whole-project/multi-file rewrite action.
- Controlled checks separate always-local static validation from explicitly approved project commands. The fixed allowlist runs without a shell, under a Windows Job Object, timeout, process/memory caps, offline package flags and redacted bounded output.
- Local AI is now the primary AI path: explicit fixed-port loopback discovery, model listing, exact final request disclosure, SSE streaming with a disclosed local-only fallback and no external paid retry. A session token meter and advisory soft cap cover every model operation without retaining prompts or inventing prices.
- Every selected-text explanation and project summary now stops at a literal request review before the separate send action. Model Markdown is rendered through the same safe, inert-link renderer as local documentation instead of exposing raw formatting markers.
- Connected AI apps require at least one explicitly selected project; an empty selection can never expand to the full catalog. Disconnect restores unchanged host configs byte-for-byte, preserves host edits made while connected, keeps token-bearing bytes out of backups/state, and revokes the old credential.
- The project sidebar now opens by default, the compact CH navigation keeps every section label readable, and Appearance offers three concise startup choices: Overview or the last workspace, plus open/remember/collapsed states for the project sidebar and details panel. Separate guided tours teach the Local workflow in seven steps and the Code Hangar AI Connector workflow in eight, with independent completion state for each edition.
- Clean, network-disabled Windows installs now work without a preinstalled WebView2 runtime; both installers embed the offline bootstrapper.
- Upgrade, repair, Local/Connector coexistence and cross-uninstall preserve the same encrypted catalog and installation key.
- Large sessions load progressively in-project and as independent sessions, with Load more and Open full session controls.
- Markdown now renders emphasis and tables instead of exposing raw markers and pipe syntax.
- Large Safe Manage dependency-cache groups are collapsed into bounded summaries without weakening mutation inputs.
- Project/session titles, byte formatting, run hints and file-tree ordering were hardened against real local data.
- The complete-folder safety path now discloses and backs up locally owned protected files, removes junction links without following their targets, and keeps shared/external protected data ineligible.
- Interrupted mutation recovery now exposes ambiguous held copies instead of orphaning them, keeps genuinely missing outcomes blocking, reports failed link/source removal truthfully, and counts recovered bytes only after the original is actually removed.
- Local and Connector frontends are now compiled as separate editions; the Local build gate rejects Connector chunks, provider endpoints and AI/MCP IPC commands before packaging.
- The frontend toolchain now resolves Vite 8 consistently, uses modern Rolldown vendor splitting and fails the edition gate if any JavaScript chunk exceeds 500 kB. Both editions build without deprecation or chunk-size warnings.
- Light/OLED, narrow layouts, 100/125/150% scaling and empty/loading/error/partial/saturated states received a full acceptance sweep and targeted regressions. The final narrow pass also fixed the first-run card collapsing to one letter per line, Recap actions being clipped by an intrinsic grid width, and scrolling content showing through sticky tool headers.

---

## Two editions — pick one

| | **Code Hangar** (recommended) | **Code Hangar AI Connector** (advanced) |
|---|---|---|
| For | Everyone. Full local management (including safe delete) with **zero** external access. | People who also want AI Assist and their AI apps to read (and, with approval, act on) the catalog. |
| AI connector | not present (physically absent from the build) | included |
| Start here if… | you're not sure | you specifically want MCP integration |

Most people should start with the **Local** edition. The **AI Connector** is an *advanced preview*: more capable, more surface area, and best used by people comfortable supervising an AI tool. Both editions read and write the **same** local catalog, so you can switch later without losing anything.

## What it does

- **Catalogs your AI work** — discovers projects and chat sessions across your installed AI apps (Claude, ChatGPT, Cursor, Antigravity, Hermes, OpenClaw, …) on Windows and WSL, grouped by app, with real transcripts.
- **Reads fast** — a three-pane navigator with safe Markdown/context preview (scripts and remote images blocked).
- **Explains each folder** — references, dependencies, models, workflows and disk footprint.
- **Finds waste** — duplicate model files and orphaned assets, with an on-demand complete-file comparison.
- **Cleans up reversibly** — nothing is destroyed without a verified backup and a holding area; strong multi-step confirmation; permanent delete is off by default, must be explicitly enabled in Recover, and always needs a verified backup plus a fresh confirmation.
- **Annotates** — comments on any project, folder or file.

## Privacy & security

- **No account and no telemetry.** The Local edition has no outbound network capability. The AI Connector edition adds only opt-in AI Assist calls to the provider or local model server you configure.
- **Encrypted at rest** — SQLCipher database, key bound to your Windows user account (DPAPI).
- **Sensitive files stay protected** — secrets, `.env`, key material are excluded from preview and search.
- **Destructive actions are gated** — backup-before-delete is enforced; permanent delete always needs a verified backup and a fresh confirmation, and can be turned off in Recover.

### About the AI Connector edition

When you connect an AI app, it can read curated **metadata** (not file contents by default). If you turn on the optional **"total control" (advanced)** tier, a trusted app may *file requests* it cannot perform directly — edit/delete a comment, a temporary file-content read, a protected backup, a move to the holding area, or a final removal of a held item. **It never executes anything itself**: every request waits for your in-app approval, where Code Hangar re-checks scopes and safety gates and acts on your behalf. Final removal is off by default, requires explicit opt-in in Recover, and still requires a verified backup plus a fresh confirmation for each item. The connector speaks only over a local channel (stdio / named pipe) — never the internet.

The connector edition also adds the optional **"Explain this code"** helper, which is **provider-agnostic and off by default**. You choose where it runs: a **local model server** on your machine (stays entirely local), or **your own API** endpoint — any Chat Completions–compatible or Messages-API–compatible provider, with your key. Provider presets are quick-fill shortcuts only; no provider is required or privileged. Any API key is stored only in the Windows Credential Manager, never in the app's database or logs; local mode needs no key and is restricted to this machine (loopback). Sensitive files and files that contain secrets are hard-blocked before anything is sent. The **Local** edition has no outbound capability at all.

Local model setup is documented in [`connect-your-local-model.md`](connect-your-local-model.md).

## Install

1. Download the installer for your edition (see assets below).
2. Run it — a per-user install, no admin required.

> **Unsigned build:** these installers are not yet code-signed, so Windows SmartScreen may warn about an "unknown publisher". Choose **More info → Run anyway**. (Signed builds are planned.)

## Verify your download

Compare the SHA-256 of your download against `SHA256SUMS`:

```powershell
Get-FileHash .\Code-Hangar_0.1.1_x64-setup.exe -Algorithm SHA256
```

```
579bae11e67570fd6135ff2f5720d6bb2779e2acba708de409f48f168fd841c0  Code-Hangar_0.1.1_x64-setup.exe
d9a1b6d5374a49f8513b9f5edfb24ce8da32e77f39a24abed545f11bd77e4bef  Code-Hangar-AI-Connector_0.1.1_x64-setup.exe
```

## Known limitations

- Windows only (WSL projects are catalogued from Windows; no native Linux/macOS build yet).
- Early preview — expect bugs and breaking changes before 1.0.
- The AI Connector edition is an advanced preview; the round-trip with each AI app is best verified on your own setup.
- Installers are unsigned for now.

## Thanks

Built in the open. Issues and feedback welcome on the tracker.

---

_Full technical detail: [`SECURITY_INVARIANTS.md`](../SECURITY_INVARIANTS.md), [`docs/PACKAGING.md`](PACKAGING.md), [`docs/total_control_extension.md`](total_control_extension.md)._
