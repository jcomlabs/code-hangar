# Contributing

Code Hangar is an Apache-2.0 local-first Windows project. Contributions are
welcome when they preserve its explicit privacy and safety boundaries.

## Before changing code

- Read [`AGENTS.md`](AGENTS.md), [`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md),
  and the relevant architecture/phase documents.
- Keep public source, comments, tests, fixtures, metadata, and documentation in
  English.
- Do not add telemetry, automatic updates, remote Git operations, package or
  documentation fetchers, or any outbound path to the Local edition.
- Do not commit credentials, personal paths, real conversations, private
  evidence, generated builds, caches, local databases, or acceptance output.
- Record incorporated third-party code or assets in [`SOURCES.md`](SOURCES.md)
  and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
- Keep event-specific material on its submission branch. General reliability,
  security, documentation, and packaging fixes belong on `main`.

## Local verification

From a clean Windows checkout with Node 24, Rust stable, and the Tauri Windows
prerequisites:

```powershell
npm ci
npm run check
npm run audit:publication
cargo fmt --all --check
cargo test --workspace --no-default-features --features core
cargo clippy --workspace --all-targets --no-default-features --features core -- -D warnings
npm --workspace apps/desktop run build:connector
npm --workspace apps/desktop run tauri:build
```

For release-level changes, also run:

```powershell
pwsh scripts/local-ci.ps1 -AgentAutomation
```

Add deterministic tests or fixture coverage for every changed invariant. State
which edition and security boundary a change affects.

## Pull requests

Keep changes small and reviewable. Describe the user-visible result, risk,
rollback path, tests run, and any remaining limitation. By contributing, you
agree that your contribution is licensed under Apache-2.0.
