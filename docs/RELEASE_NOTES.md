# Code Hangar v0.1.3 — release-candidate notes

Status: **not published**. Final safety acceptance, the signing decision, the
clean-install lifecycle and the upload/download hash round-trip are still
release gates. Nothing in this draft authorizes publication.

Code Hangar is a local-first Windows desktop application for finding, reviewing
and safely managing AI-assisted projects and the local conversation records that
help explain them. Local has no account, telemetry, AI/MCP surface or
outbound-network client. The optional AI Connector is experimental and less
tested than Local. It adds local MCP stdio, AI-app configuration and Windows
named-pipe integration plus one experimental,
feature-gated client for an explicit request to the loopback/HTTPS provider the
user configures. It adds no telemetry, updater, remote Git or background network.

This remains an early preview. Keep backups of important work and review every
Safe Manage plan before confirming it.

## What is new since v0.1.1

- A calmer, more directed shell reduces duplicate navigation, secondary controls
  and idle status noise while keeping advanced tools available on demand.
- The Review Inbox and What changed flow use durable project checkpoints. Undated
  local session records are acknowledged by a content-free set fingerprint, so
  reviewed records clear and newly discovered identities reappear.
- Direct Viewer shell-open remains isolated from the project catalog. Switching
  between Rendered and Source before catalog attachment no longer sends an
  invalid project identifier to the backend.
- The scanner uses a fixed-size authoritative Windows handle query for file
  identity/reparse state. The normal path avoids a per-entry reparse buffer and
  IOCTL; the prior IOCTL remains a bounded fallback when the fixed query fails.
- File-tab close controls meet the 24 × 24 CSS-pixel target contract and have a
  visible keyboard focus state. Reduced-motion and OLED token coverage received
  additional regression checks.
- Deep Scan and mutation safety are undergoing a new adversarial release pass.
  `docs/qa/v0.1.3-acceptance.md` is the stable 50-gate specification, not a
  mutable candidate report. Exact results stay private outside Git as 50
  canonical schema-2 gate envelopes, typed manual/owner attestations, shared
  local-CI/release/lifecycle evidence and
  schema-3 `ACCEPTANCE-EVIDENCE.private.json`; arbitrary `PASS` files cannot seal
  a gate, and any gate not proven `PASS` blocks the
  seal. Canonical staging later exports a closed, path/claim-free public
  projection as `ACCEPTANCE-EVIDENCE.json`.
- The Windows Sandbox lifecycle runner is version-parameterized. Its v0.1.3
  default tests upgrade from v0.1.1. The canonical run requires a new evidence
  directory, records the clean Git commit and tree, hashes every shared input and
  exports the edition/role/path identity/length/hash of the binaries actually
  installed by each exact setup inside the network-disabled guest. The final
  proof rejects substituted snapshots from another build or installation.
  Failed historical attempts are never promoted to current release evidence.
- Windows packaging now requires one explicit offline WebView2 installer bound
  to a tracked name/length/hash/version/signature manifest. A generated
  preinstall hook re-hashes the extracted runtime before execution and has no
  download, updater or automatic cache-selection fallback. Real no-WebView,
  no-network VM acceptance remains a release gate.

## Two editions

| | Code Hangar Local | Code Hangar AI Connector |
|---|---|---|
| Recommended for | Local project navigation, review and guarded cleanup | Users who deliberately want scoped MCP access and/or opt-in AI Assist |
| Network surface | None | local MCP; only explicit AI Assist requests to the configured loopback or HTTPS provider |
| MCP sidecar | Not compiled or packaged | Feature-gated `code-hangar-mcp.exe` |
| Local catalog | Shared, encrypted, current-Windows-user boundary | Same catalog and boundary |
| Safe Manage | Same explicit mutation gates | Same explicit mutation gates; an AI app cannot bypass them |

Both editions are designed to use the same application identity and catalog.
Catalog preservation while installing, switching and uninstalling v0.1.3 remains
a mandatory lifecycle gate, not a conclusion inferred from the shared identifier.
The product must make the active edition clear and must never interpret a
capability-check error as proof that Connector features were not compiled.

## Safety model

- Read-only navigation is the default.
- Safe Manage remains explicit, while final removal is a primary local-user outcome rather than
  a hidden feature. Its durable capability starts OFF and requires the exact in-app phrase
  `ENABLE PERMANENT REMOVAL`; Local preview, confirmation and batch start all recheck that gate.
- Enabling the capability is not deletion authority. A removal still starts with an immutable
  one-project or multi-project review.
- A permanent removal requires an object/topology-complete archive, a held source, a short-lived
  single-use confirmation bound to the exact preview and a final handle-bound revalidation.
  Ineligible objects remain held while independently eligible objects can complete. Backend checks
  remain authoritative even if the UI is bypassed.
- Reparse points are never followed. Cloud Files — online-only or materialized —
  are not mutation-owned merely because their contents are locally readable.
- The application performs no Git fetch, pull, push, clone, commit, branch,
  checkout, reset or restore operation.
- The supported Windows x64 Local graph rejects `hangar-ai`, `reqwest`,
  `keyring` and provider reachability; its bundle rejects provider/MCP chunks,
  IPC, copy and CSS. Connector permits only the reviewed `hangar-ai` →
  `reqwest 0.12` provider closure and local MCP, while both editions reject telemetry,
  updater, remote Git and browser/implicit network. Tauri's existing transitive
  `url` parser is not falsely described as absent. Cargo's all-target metadata retains Tauri 2.11.2's exact
  mobile-only `reqwest 0.13.4` edge for unsupported Android/non-macOS-Apple
  targets; it is guarded but neither compiled nor shipped, so this is not a
  lockfile-wide zero-`reqwest` claim.

## Installation

Final installer names are expected to be:

- `Code Hangar_0.1.3_x64-setup.exe`
- `Code Hangar AI Connector_0.1.3_x64-setup.exe`

The signing state is **PENDING OWNER DECISION**. If the final installers remain
unsigned, these notes must explicitly retain the Windows SmartScreen instructions
before publication. Do not infer the final signing state from any private build.

## Verify your download

Finalize and commit these tracked notes before creating receipts or final
artifacts; do not edit them after provenance begins. Generate `SHA256SUMS` only
after the signing/unsigned decision and only from the final clean candidate
whose exact bytes passed the offline lifecycle, private final-byte release proof
and private 50-gate acceptance seal. Final staging also contains the closed
public `ACCEPTANCE-EVIDENCE.json` projection and `RELEASE-MANIFEST.json`, binding
the private-proof/report hashes, both installer hashes, receipts, identities,
Connector MCP and lifecycle to the Git commit/tree and structured signing
decision. Then
download every uploaded installer into a fresh directory and reproduce the
published hashes. This records the source/artifact association and exact tested
bytes; it is not a claim that the build is independently reproducible.

```powershell
Get-FileHash '.\Code Hangar_0.1.3_x64-setup.exe' -Algorithm SHA256
Get-FileHash '.\Code Hangar AI Connector_0.1.3_x64-setup.exe' -Algorithm SHA256
```

The canonical values are the two entries in the attached `SHA256SUMS`. They may
also be copied verbatim into the external release description after staging.
Keeping them out of this tracked draft avoids changing the source commit after
artifact provenance has been recorded. Any hash from an earlier local candidate
is obsolete after a source change, rebuild or signature.

## Known limitations

- Windows only. WSL projects are discovered from Windows; there is no native
  Linux or macOS build.
- Early preview: compatibility and data formats may still change before 1.0.
- Online documentation, telemetry and automatic updates are intentionally absent.
- Real AI-app round-trip, signing and remote artifact verification remain
  explicit owner-performed gates for this candidate.
- GitHub Actions and Dependabot automation are absent from the source tree. They
  cannot consume CI credits or create routine notifications; the hash-bound
  `local-ci.ps1` evidence on the release worktree is the automated authority.

## Release verdict

**HOLD.** The tracked acceptance specification is
`docs/qa/v0.1.3-acceptance.md`; candidate proof must remain a separately sealed,
hash-revalidated private release-artifact proof plus
`ACCEPTANCE-EVIDENCE.private.json`. Publication is permitted only after all 50
pre-seal gates pass on the exact final commit, canonical staging exports the
closed public projection, and the owner clears the later upload and
download-verification gates in `RELEASE_CHECKLIST.md`.
