# Source and provenance record

Review date: 2026-07-18.

Code Hangar's application code, tests, documentation, fixtures, screenshots,
and branding are original project work unless identified below. The screenshots
in `docs/assets/` were captured from Code Hangar with synthetic example data.
No private conversation, project, account, or filesystem evidence is part of
the public distribution.

## Incorporated dependencies

Exact versions and transitive graphs are pinned by `Cargo.lock` and
`package-lock.json`. The principal directly used dependency families are:

| Component | Role | License metadata |
|---|---|---|
| Tauri and Tauri plugins | Windows desktop shell, bundling, dialogs | Apache-2.0 OR MIT |
| React and React DOM | desktop UI | MIT |
| Lucide React | interface icons | ISC |
| Vite, Vitest, TypeScript, ESLint | build, tests, type checking, lint | MIT / Apache-2.0 as declared by each package |
| rusqlite with bundled SQLCipher | encrypted local SQLite storage | MIT for rusqlite; bundled SQLCipher remains under its own license |
| serde / serde_json | serialization | MIT OR Apache-2.0 |
| pulldown-cmark | Markdown parsing | MIT |
| blake3 | content hashing | CC0-1.0 OR Apache-2.0 variants |
| chrono, tempfile, thiserror, toml_edit, windows-sys | time, tests, errors, config and Windows APIs | permissive licenses declared in `Cargo.lock` metadata |
| walkdir | bounded filesystem traversal | Unlicense OR MIT |

`NOTICE` and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) identify runtime
material that retains separate terms. The repository's Apache-2.0 license does
not relicense dependencies, Windows, WebView2 or external AI apps.

## Interoperability, not incorporation

Code Hangar reads local, user-owned records produced by supported AI coding
applications and can register its own MCP server in their documented local
configuration formats. Those applications, their branding, services, models,
and conversation data are not distributed by this repository.

The Connector registers a local MCP executable in supported AI-app configuration
formats. It does not bundle those apps, their services or their models, and does
not imply sponsorship by their vendors.
