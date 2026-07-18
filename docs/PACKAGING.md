# Packaging & release

Code Hangar ships as **two editions** built from the same source tree. This document is the release playbook.

## The two editions

| | Local | AI Connector |
|---|---|---|
| What it does | full local management **including safe delete** (backup → hold → final-remove) | Local + AI Assist + AI-app integration |
| Cargo features | `mutation` | `agent_automation` (⊇ `mutation`) |
| Tauri config | `tauri.conf.json` | `tauri.conf.json` + `tauri.connector.conf.json` |
| Network / AI | **none** (zero outbound, zero AI — CI-enforced) | only the provider the user explicitly configures |
| Connector crates linked | none | `hangar-agent`, `hangar-mcp`, `hangar-appconfig` |
| Ships `code-hangar-mcp.exe` | no | yes (Tauri sidecar, next to the app exe) |
| AI App Integration panel | absent from the compiled frontend bundle | shown |
| npm script | `package:local` (`package:base` kept as alias) | `package:connector` |

The Local edition can delete (it links `hangar-mutation`) but is **100% local**: it links none of the AI / connector code and makes no outbound network call. This is asserted in CI by `scripts/check-no-outbound-deps.mjs`, which runs `cargo tree` for **both** the `core` and the shipped `mutation` edition and fails if any connector/AI/outbound-network crate appears (the strictest `core` lane also forbids `hangar-mutation`). Frontend isolation is compile-time too: the base Tauri config runs `build:local` (Vite mode `offline`), the Connector override runs `build:connector`, and `scripts/check-frontend-edition.mjs` rejects Connector chunks, endpoints, AI/MCP IPC command names and Connector-only copy in the Local bundle. Runtime `active_features` remains a second gate in the Connector frontend.

**Naming & data.** The connector override sets a distinct `productName` ("Code Hangar AI Connector") so the two installers and Start-menu entries are unambiguous, but keeps the **same `identifier`** (`local.codehangar.desktop`) — so both editions read and write the **same** encrypted local catalog under `%APPDATA%`. A user can install whichever edition they want (or both); switching editions never loses the inventory.

## Build

From the repo root (Rust stable + Node 24 + Tauri Windows prerequisites installed):

```sh
npm ci

# Connector edition (builds the MCP server, stages it as the sidecar, then bundles)
npm --workspace apps/desktop run package:connector

# Local edition
# Build this last if you build both editions; it cleans any staged connector sidecar.
npm --workspace apps/desktop run package:local
```

Output (NSIS installers): `target/release/bundle/nsis/*-setup.exe`.
The loose `target/release/code-hangar-desktop.exe` is only a build byproduct and
reflects whichever edition was built last; the NSIS installers are the release
artifacts to test and publish.

Run the two package commands **sequentially**, not in parallel. Both builds reuse
`apps/desktop/dist`; parallel Tauri builds can race while embedding frontend
assets and fail with a missing asset path.

`package:connector` (via `scripts/package-connector.ps1`):
1. `cargo build -p code-hangar-mcp --release`
2. copies it to `apps/desktop/src-tauri/binaries/code-hangar-mcp-<target-triple>.exe` (the Tauri sidecar naming convention — gitignored)
3. `tauri build --features agent_automation --config src-tauri/tauri.connector.conf.json`

Tauri strips the triple and installs `code-hangar-mcp.exe` next to the desktop exe, which is exactly where `connected_app_server_path()` looks for it.

`package:local` removes any previously staged `code-hangar-mcp-*.exe` sidecar before bundling. This is defensive: the Local Tauri config does not include `externalBin`, and the cleanup keeps the workspace and installer contents clearly Local-only after alternating builds.

Both installers embed Microsoft's offline WebView2 runtime installer. This makes
the bundles substantially larger, but a clean Local install must not contact the
network or fail merely because WebView2 is absent. `check:security` rejects a
return to Tauri's online `downloadBootstrapper` default.

## Versioning

Bump `version` in `Cargo.toml` (`[workspace.package]`) and `apps/desktop/src-tauri/tauri.conf.json` together (keep them in sync). The NSIS installer filename embeds the version.

## Icons

The icon set under `apps/desktop/src-tauri/icons/` is generated from `apps/desktop/src-tauri/app-icon.svg`:

```sh
cd apps/desktop && npx tauri icon src-tauri/app-icon.svg
```

(Then delete the generated `ios/` and `android/` folders — this is a desktop-only app.)

## Code signing (TODO — requires a certificate)

Unsigned installers trigger a Windows SmartScreen / "unknown publisher" warning. To sign, the maintainer needs an Authenticode (OV/EV) code-signing certificate, then configures Tauri's Windows signing:

- set `bundle.windows.certificateThumbprint` (or use a signing command / `signCommand`) in the Tauri config, **or** sign the produced `.exe` with `signtool` post-build.

This is **not** wired up here because it needs a private certificate that must never live in the repo. Until signed, document the SmartScreen step for users (More info → Run anyway).

## Publishing (manual — maintainer action)

1. Build both editions (`package:connector` then `package:local`, sequentially), sign them (once a cert is available), and smoke-test each installer on a clean Windows user.
2. Generate release-safe asset copies and `SHA256SUMS` **after** signing: `pwsh scripts/checksums.ps1` (writes exactly two portable installer names plus the manifest under `target/release/bundle/nsis/release-assets/`). Upload those three files, not the space-containing Tauri build outputs.
3. Finalize the release notes from the draft at [`RELEASE_NOTES.md`](RELEASE_NOTES.md) (set the date, paste the `SHA256SUMS`, confirm the signed/unsigned line).
4. Create a GitHub Release for the tag and upload both `-setup.exe` files (clear edition labels) + `SHA256SUMS`.
5. Update the README Releases link if needed.

> Publishing a release is a deliberate maintainer action (it pushes binaries to the public). The build pipeline above gets you to verified, ready-to-upload installers; the upload itself stays in your hands.

## Pre-release checklist

- [ ] `pwsh scripts/local-ci.ps1 -AgentAutomation` is green (all lanes, fmt, clippy `-D warnings`, npm check, connector surface).
- [ ] Version bumped in `Cargo.toml` + `tauri.conf.json`.
- [ ] `package:local` produces a Local installer; AI App Integration panel is absent in it.
- [ ] `package:connector` produces a connector installer that ships `code-hangar-mcp.exe`; connecting a real AI app round-trips.
- [ ] Both editions installed on a **clean** Windows user and launched: Local shows no AI panel; connector connects a real AI app (register → MCP round-trip → remove config).
- [ ] Installers signed (or the SmartScreen note is documented for users).
- [ ] `release-assets/` generated (`scripts/checksums.ps1`), its two installers + `SHA256SUMS` uploaded, and release notes finalized from `RELEASE_NOTES.md`.
