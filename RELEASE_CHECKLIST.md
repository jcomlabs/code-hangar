# Release checklist (owner)

A single, ordered checklist for publishing a Code Hangar release. This complements
[`docs/PACKAGING.md`](docs/PACKAGING.md) — that document is the build playbook (edition
internals, scripts, naming); this is the **owner's publish checklist**.

**Signing and checksum generation are manual owner steps.** Nothing in this repo signs
binaries or uploads a release; a code-signing certificate must never live in the repo.

---

## 1. Edition matrix

Two editions are built from the same source tree. Confirm you know which is which before shipping both.

| | **Code Hangar (Local)** | **Code Hangar — AI Connector** |
|---|---|---|
| What it does | Full local management **including safe backup/delete** (backup → holding → final removal, off by default) | Local + AI Assist + AI-app integration (MCP) |
| Network / AI | **none** — zero outbound, zero AI (CI-enforced) | only the local model server / provider the user explicitly configures; MCP over a local channel |
| Connector / AI code | **physically absent** from the Rust graph and executable frontend modules (gate-enforced) | included |
| Ships `code-hangar-mcp.exe` | no | yes (Tauri sidecar, next to the app exe) |
| AI app integration panel | absent from the compiled frontend bundle | shown |
| Installer name | `Code Hangar_x.y.z_x64-setup.exe` | `Code Hangar AI Connector_x.y.z_x64-setup.exe` |
| npm build script | `package:local` | `package:connector` |
| Product name | `Code Hangar` | `Code Hangar AI Connector` |
| App data / `identifier` | `local.codehangar.desktop` (**shared** — both editions read/write the same encrypted catalog) | same |

Because the two editions share the same `identifier`, a user can install either or both, and switching editions never loses the local inventory.

---

## 2. Pre-publish verification

- [ ] **Version bumped and in sync** — `Cargo.toml` (`[workspace.package]`) and `apps/desktop/src-tauri/tauri.conf.json` carry the same `version` (the NSIS installer filename embeds it).
- [ ] **Canonical acceptance report is complete** — every non-`N/A` row in `docs/qa/v0.1.1-acceptance.md` is `PASS`, no HIGH/MEDIUM finding remains open, and every fixed finding names its regression evidence.
- [ ] **Full local gate is green** — `pwsh scripts/local-ci.ps1 -AgentAutomation` passes all lanes (fmt, clippy `-D warnings`, npm check, mutation + connector surface).
- [ ] **Public-history gate is green** — the sanitized public checkout passes `npm run audit:publication` and `npm run scan:secrets` with complete history.
- [ ] **Zero-network promise re-checked** — the guardrail (`scripts/check-no-outbound-deps.mjs`, run inside the gate) confirms no connector / AI / outbound-network crate appears in the `core` or shipped `mutation` dependency graph.
- [ ] **Build both editions, sequentially** — `package:connector` **then** `package:local` (never in parallel; they share `apps/desktop/dist`). Output: `target/release/bundle/nsis/*-setup.exe`.
- [ ] **Local installer verified on a clean Windows user** — installs per-user (no admin), launches, and the **AI app integration panel is absent**. Discovery finds projects/sessions; the backup/restore pipeline works.
- [ ] **Connector installer verified on a clean Windows user** — installs, ships `code-hangar-mcp.exe` next to the app exe, and a **real AI app round-trips**: Connect (registers into the app's config) → the app sees the `code-hangar` MCP server → a read works → Disconnect cleanly removes only Code Hangar's entry.
- [ ] **Safety gates spot-checked** — final removal is **off by default**; enabling it requires a deliberate confirmation; a removal still demands a verified backup + fresh confirmation. (The end-to-end backup → move-to-holding → final-remove → *backup survives* path is covered by automated tests; a one-time click-through on a throwaway folder is a good confidence check.)

---

## 3. Code signing (owner-performed — described only)

Unsigned installers trigger a Windows SmartScreen / "unknown publisher" warning. Signing is **not** wired into this repo because it requires a private certificate that must never be committed.

To sign, the owner:

1. Obtains an **Authenticode (OV or EV) code-signing certificate**.
2. Signs each produced `*-setup.exe`, either by:
   - configuring Tauri's Windows signing (`bundle.windows.certificateThumbprint`, or a `signCommand`) so `tauri build` signs during bundling, **or**
   - signing the produced `.exe` files post-build with `signtool` (e.g. `signtool sign /fd sha256 /tr <timestamp-url> /td sha256 ...`).
3. Confirms the signature (`signtool verify /pa <file>` or the file's Properties → Digital Signatures).

> **Order matters:** sign **before** generating checksums (step 4) — signing changes the file bytes, which changes the hash.

If a release ships **unsigned**, document the SmartScreen step for users (**More info → Run anyway**) in the release notes, and set the "signed vs unsigned" line in [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) accordingly.

---

## 4. Checksums (owner-performed)

Generate checksums **after** signing, so the published hashes match the signed binaries users download:

- [ ] Run `pwsh scripts/checksums.ps1` → stages exactly two portable-name installers plus `SHA256SUMS` under `target/release/bundle/nsis/release-assets/`; upload those staged copies so GitHub does not rewrite their names.
- [ ] Paste the `SHA256SUMS` block into [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) (replacing the placeholder), so users can verify with `Get-FileHash <installer> -Algorithm SHA256`.
- [ ] Download every staged release asset again into a fresh directory and verify its SHA-256 against `SHA256SUMS`; a local build hash alone does not prove the uploaded bytes.

---

## 5. Before you publish

- [ ] Both installers built (`package:connector` then `package:local`), **signed** (or the unsigned SmartScreen note is in the release notes), and smoke-tested on a clean Windows user.
- [ ] `SHA256SUMS` generated **after** signing and pasted into the release notes.
- [ ] [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) finalised — date set, checksum block pasted, signed/unsigned line correct, known limitations current.
- [ ] Create the GitHub Release for the tag and upload the **two staged installers** from `release-assets/` (clear edition names) **plus** its `SHA256SUMS`.
- [ ] Update the README Releases link if it changed.
- [ ] Confirm the public GitHub Actions run is green and the downloaded assets match the uploaded `SHA256SUMS` before announcing the release.

> Publishing pushes binaries to the public — it is a deliberate manual action. The steps above get you to verified, signed, checksummed installers; the upload stays in your hands.

---

_See also: [`docs/PACKAGING.md`](docs/PACKAGING.md) (build internals), [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) (the release-notes draft), [`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md) (security model)._
