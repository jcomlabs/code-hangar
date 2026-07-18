# Devpost submission draft

> Owner-reported status on 18 July 2026: the Devpost account exists. No project
> has been submitted and no external URL has been published by this file.

## Core fields

| Field | Draft value |
|---|---|
| Project name | **Code Hangar** |
| Category | **Developer Tools** |
| Tagline | **The local-first flight recorder for vibe coding.** |
| Repository | <https://github.com/jcomlabs/code-hangar> |
| Demo video | Pending owner-authorized public YouTube URL |
| Test build | Pending owner-authorized Connector installer URL |
| Supported platform | Windows 11 x64; WSL projects are catalogued from Windows |
| License | Apache-2.0 |
| Codex feedback session | `019f3315-12ff-7071-8534-04fe50ed534e` |

## Short description

Code Hangar reconstructs what AI coding tools changed from local session, Git,
current-file, and saved-review evidence. It shows coverage and unknowns first,
then lets GPT-5.6 in Codex inspect a curated project view through a scoped local
MCP server. Any correction remains small, reviewed, snapshotted, and reversible.

## Project story

### Inspiration

Vibe coding makes it easy to create many projects and hard to remember what an
AI actually changed. The evidence is scattered across tool sessions, Git state,
working files, caches, and project notes. People who most need an explanation
are often least equipped to reconstruct that history safely.

### What it does

Code Hangar is a Windows desktop control centre for retrospective AI-project
review. It discovers local projects and AI sessions, builds a best-supported
record of changes, labels incomplete evidence, and presents the source before
any model explanation. The Connector edition exposes a body-limited,
project-scoped MCP surface to Codex and offers an optional AI Assist for local
servers or a provider configured by the user. Secrets and Protected Zones are
blocked before transport. Corrections are deliberately limited to a reviewed
one-file surface with validation, snapshots, and restore.

### How we built it

The application uses Tauri v2, Rust, React, TypeScript, SQLCipher, Windows
DPAPI, and a feature-gated MCP sidecar over stdio. Codex was the engineering
collaborator during the eligible Build Week period: it audited the existing
product and local delta, implemented bounded review/navigation and graph-safety
improvements, added regressions, validated Local-versus-Connector isolation,
and prepared the judge evidence.

GPT-5.6 is demonstrated through **MCP out**: Codex, signed in with ChatGPT,
queries Code Hangar's scoped local tools. A synthetic acceptance run completed
through the sidecar installed by the final Connector with `gpt-5.6-sol`, two
audited Code Hangar reads, no API key, and no retained personal data. The
separate **AI in** path is the in-app provider adapter: its
OpenAI GPT-5.6 request contract, exact disclosure, secret gate, and compatible
provider behavior are tested. A future Codex app-server adapter could bring
ChatGPT-authenticated output into Code Hangar, but that adapter is not claimed
as shipped in this candidate.

### Challenges

The hardest problem was preserving epistemic and security boundaries. A model
answer cannot become evidence for a missing edit. A connected app cannot gain
cross-project visibility. The Local edition cannot merely hide network UI; the
provider and MCP code must be absent from its build. Mutation must remain behind
fresh review, content-bound backup, and human approval. We also had to keep the
Build Week delta honest because Code Hangar existed before the event.

### Accomplishments

- A review flow that separates recorded facts, current state, and unknowns.
- GPT-5.6 consuming Code Hangar context through authenticated, scoped MCP using
  ChatGPT subscription access.
- Compile-time Local-versus-Connector isolation with automated dependency and
  frontend gates.
- Secret and Protected Zone blocking before any model transport.
- A reversible correction path with exact diff, validation, snapshot, and
  restore.
- Two reproducible Windows installer artifacts with recorded hashes.
- Exact host install, native launch, edition-isolation inspection, and uninstall
  for both 0.1.2 artifacts, with the pre-existing catalog hash-verified unchanged.

### What we learned

The safest place for AI is after deterministic evidence, not instead of it.
MCP also creates a cleaner subscription story than copying credentials into an
app: Code Hangar owns curation and authorization, while Codex owns ChatGPT
authentication and model execution. Direct provider APIs remain valuable as an
optional inbound route, but they are a separate billing and trust boundary.

### What's next

- Sign the Windows installers and repeat the exact 0.1.2 clean-install journey
  in a disposable supported Windows environment.
- Productize the subscription-backed Codex app-server inbound bridge behind the
  Connector feature gate, without reading or storing Codex credentials.
- Expand evidence adapters and platform coverage while preserving the same
  project-scoped and reversible safety model.

## Build-period disclosure

Code Hangar is a pre-existing project. The declared baseline is commit
`843530c` from 12 July 2026. The eligible Build Week comparison is
`843530c..e831c14dfa15291dda152d7742766221438feaa3`. The reusable product work,
GPT-5.6 request integration, and removable submission-only package are separated
in local history and documented in `BUILD_PERIOD_DELTA.md`.

## Technologies

`Rust`, `Tauri v2`, `React`, `TypeScript`, `SQLite`, `SQLCipher`, `Windows
DPAPI`, `Model Context Protocol`, `Codex`, `GPT-5.6`, `PowerShell`, `Vitest`

## Known limitations to retain in the final form

- Windows x64 only; the installers are currently unsigned preview builds.
- The exact 0.1.2 bytes have not completed a clean disposable-Windows lifecycle:
  separate network-disabled Sandbox attempts were blocked before setup/product
  execution by guest Application Control because the installers are unsigned.
- The primary live proof is subscription-backed GPT-5.6 through Code Hangar MCP.
- The in-app direct OpenAI path is contract-tested but has no separately captured
  paid API round trip.
- The subscription-backed app-server inbound adapter is a documented extension
  path, not a shipped candidate feature.
- YouTube publication, `/feedback`, and final Devpost submission remain
  owner-gated external actions. Release URLs are filled after the public
  candidate rebuild and re-verification.
