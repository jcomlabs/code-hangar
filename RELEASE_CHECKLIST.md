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
| What it does | Full local management **including safe backup/delete** (backup → holding → explicit project/batch final removal) | Local + scoped local AI-app integration (MCP) + experimental opt-in AI Assist |
| Network / AI | **none** — zero outbound (local-gate enforced) | local MCP; one explicit loopback/HTTPS provider path through `hangar-ai` only |
| Connector code | **physically absent** from the Rust graph and executable frontend modules (gate-enforced) | local MCP/appconfig/named-pipe plus feature-gated provider surface |
| Ships `code-hangar-mcp.exe` | no | yes (Tauri sidecar, next to the app exe) |
| AI app integration panel | absent from the compiled frontend bundle | shown |
| Installer name | `Code Hangar_x.y.z_x64-setup.exe` | `Code Hangar AI Connector_x.y.z_x64-setup.exe` |
| Canonical wrapper | `scripts/package-local.ps1` | `scripts/package-connector.ps1` |
| Product name | `Code Hangar` | `Code Hangar AI Connector` |
| App data / `identifier` | `local.codehangar.desktop` (**shared** — both editions read/write the same encrypted catalog) | same |

The editions are designed to share that identity and catalog. Do not infer
coexistence or preservation from the identifier alone: the canonical lifecycle
below must prove install, switch, cross-uninstall and catalog preservation for
the exact final artifacts.

---

## 2. Pre-publish verification

- [ ] **Version bumped and in sync** — `Cargo.toml` (`[workspace.package]`), root `package.json`, `apps/desktop/package.json` and the effective base/Connector Tauri configuration resolve to the exact same version (the Connector overlay may inherit the base version; the NSIS installer filename embeds it).
- [ ] **Tracked release notes are final before artifact provenance starts** — set the date/signing wording and remove every `PENDING` claim, then commit. Final checksums live in the attached `SHA256SUMS` and may be copied into the external GitHub Release text; do not edit the tracked source tree after the canonical lifecycle/checksum run.
- [ ] **Acceptance specification/workspace is fixed** — the tracked `docs/qa/v0.1.3-acceptance.md` remains an unchanged 50-gate specification. A new private workspace created by `scripts/acceptance-v013.ps1 -Initialize` has exactly one structured gate envelope and unique `evidence/<ID>/` subtree per gate. Final sealing occurs only after the final-byte release proof in section 4; no generic shared file may satisfy several gates.
- [ ] **Full local gate is green and authoritative** — non-packaging lanes pass with `pwsh scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild`, including compile-only Windows mutation and Connector desktop/backend/sidecar lanes; no raw Tauri bundle is evidence. Any packaging run supplies the exact explicit `-WebView2InstallerPath` and runs with outbound traffic blocked. No GitHub check substitutes for this hash-bound local evidence.
- [ ] **GitHub automation remains absent** — the source tree contains no Actions workflow or Dependabot configuration, the repository has Actions disabled, and no remote run is required. Release evidence is produced by the hash-bound local gate; mutable remote `npm audit` is not accepted as a release check.
- [ ] **Public-history gate is green** — record `git rev-parse "<validated-source-commit>^{tree}"`, run the canonical local-CI lane so its source-bound evidence contributes only `AUTO-06/secret-scan`, then prove the isolated sanitized checkout with `npm run audit:publication -- --source-tree <recorded-tree-object-id> --evidence-dir .local/acceptance/v0.1.3/publication-audit/<new-run-id>`. The strict audit requires a clean, non-shallow `main` with exactly one root/commit/local head, zero tags, the exact public `origin`, exact author and committer identities, and a HEAD tree identical to the recorded source tree. Only that strict pass may create `PUBLICATION-AUDIT.private.json`; copy it beside `LOCAL-CI-EVIDENCE.private.json` in the acceptance workspace's `shared-evidence/` directory. A worktree audit can never issue the candidate claim, and `AUTO-06` remains unsealable unless both independent claims validate against the exact acceptance commit/tree.
- [ ] **Edition-aware network boundary re-checked for the supported product** — Windows x64 Local has no `hangar-ai`, `reqwest`, `keyring`, provider edge/source/chunk/IPC or MCP artifact; Tauri's existing transitive `url` is not falsely treated as a provider. Connector permits only the selected `hangar-ai` provider client and local MCP; telemetry, updater, remote Git, browser network and implicit/background requests remain denied. The all-target audit separately confines Tauri 2.11.2 → `reqwest 0.13.4` to unsupported Android/non-macOS-Apple metadata.
- [ ] **Pinned WebView2 preflight passes for both editions** — pass one explicitly chosen `MicrosoftEdgeWebView2RuntimeInstallerX64.exe` to both wrappers with `-PreflightOnly -WebView2InstallerPath <exact-path>`. It matches `scripts/release-inputs/webview2-x64.json` in name/length/hash/version/PE machine, embedded Microsoft signature, offline chains and timestamp. No cache glob or automatic candidate selection is allowed.
- [ ] **Prepare both editions, sequentially under outbound blocking** — run each wrapper with `-PrepareSigning`, the exact pinned WebView input, one explicit audited release-root public blob and a new edition-specific signing directory. The phase compiles parent/helper/verifier (plus Connector MCP), snapshots and hashes that edition's checked frontend `dist`, emits a schema-3 receipt bound to the exact clean Git commit/tree and produces no installer. Record each printed receipt SHA-256 outside its signing directory. Calling a wrapper without an explicit mode must fail.
- [ ] **Inner-binary owner signing is complete** — Authenticode-sign each prepared desktop parent and elevated helper externally. Generate an edition-specific `code-hangar-release-manifest.json` with the owner-held offline RSA private key and `scripts/new-release-identity-manifest.ps1`, supplying that edition and its recorded signing-receipt SHA-256; it binds the post-Authenticode parent/helper hashes, canonical install names and receipt-bound release identity. Verifier and MCP bytes still match their preparation receipts.
- [ ] **Bundle receipt-bound inputs, sequentially under outbound blocking** — run Connector then Local with `-BundleSigned`, their exact signing directory, externally recorded `-ExpectedSigningReceiptSha256`, signed parent/helper and RSA-PSS manifest. Each run also proves the edition's canonical bundle-contract digest has not changed, verifies signed installation layout before and after `tauri bundle --no-sign`, temporarily restores only that receipt-bound locked frontend snapshot and restores the prior worktree `dist` in `finally`, preserves MCP only in Connector, proves the outer setup is `NotSigned`, and prints one fresh raw UNSIGNED HOLD candidate path/hash. No compiler runs in this phase, and no signed-uninstaller/complete-chain claim is made.
- [ ] **Offline clean-VM WebView acceptance passes twice** — from a reset Windows Sandbox/VM with WebView2 absent and outbound blocked, each installer prints `Pinned WebView2 extracted SHA256 verified: 3A08103BED8A3D9AEFDFC9AC10A672EA69605163F2DCB08D76CFD3E0444511C9`, installs the runtime without a network path, completes Code Hangar installation and launches the correct edition. Preserve evidence separately for Connector and Local.
- [ ] **Local installer verified on a clean Windows user** — installs per-user (no admin), launches, and the **AI app integration panel is absent**. Discovery finds projects/sessions; the backup/restore pipeline works.
- [ ] **Connector installer verified on a clean Windows user** — installs, ships `code-hangar-mcp.exe` next to the app exe, starts with AI Assist off, exposes no automatic discovery/request, and a **real AI app round-trips locally**: Connect (registers into the app's config) → the app sees the `code-hangar` MCP server → a read works → Disconnect cleanly removes only Code Hangar's entry.
- [ ] **Exact sidecar passed the separate live-client fixture gate** — `scripts/mcp-claude-real.ps1` ran only with explicit owner authorization, exact independently hashed executable paths for the client and sidecar, the exact schema-3 Connector receipt plus independently recorded receipt hash, and an explicit qualified config root. Its private schema-3 report proves the receipt edition/version/source and sidecar bytes match the released Connector; records a non-empty exact client version and exit code zero; proves `list_catalog` and `project_context` plus the canonical audit/disconnect results; and proves the live config's existence, bytes, SHA-256, attributes, creation time and last-write time are identical before/after. This does not substitute for the clean-user registration lifecycle above.
- [ ] **Permanent-removal contract proved** — Recovery exposes a central capability that starts OFF. Prove wrong/missing activation phrases fail, exact `ENABLE PERMANENT REMOVAL` enables it, preview/confirm/start all recheck the durable flag, and disabling after confirmation blocks start. Enabling is not deletion authority: execution still consumes a short-lived confirmation bound to the exact immutable project/batch preview. Every deleted object has an exact `object_archive/2` round-trip proof; blocked objects remain held with stable reasons; the result reports residual trees and retained archive allocation per volume. Exercise backup → holding → mixed eligible/blocked batch → final removal → *archive survives* on synthetic NTFS fixtures before the supervised throwaway-folder click-through. The release remains blocked until both Extended and Legacy Windows disposition modes are proved and the `cancel delete-pending fails` → abrupt process termination case preserves all raced bytes; a durable ambiguous journal state alone is not physical preservation evidence.

---

## 3. Code signing (owner-performed — hard gates)

The wrappers never access an Authenticode certificate. Parent/helper signing is
an input to `BundleSigned`; the resulting NSIS file is proven `NotSigned` and is
still only a raw UNSIGNED HOLD candidate because Tauri is deliberately invoked
with `--no-sign`. Its embedded uninstaller is not claimed signed or release-ready.

- [ ] Owner has an **Authenticode OV/EV code-signing certificate** whose private key never enters the repository.
- [ ] Each desktop parent and elevated helper was signed before its RSA-PSS identity manifest was generated; `code-hangar-release-verify` accepts the exact post-sign bytes offline.
- [ ] An audited owner-certificate Tauri/NSIS signing path supplies the uninstaller signing command during bundling and signs the final setup. **Signing only the outer setup afterwards is insufficient** because it leaves the embedded uninstaller unsigned.
- [ ] `signtool verify /pa` (or equivalent offline evidence) succeeds for each final setup, the installed parent/helper and the installed uninstaller. Signer identity, timestamp and exact hashes are captured from a reset VM.
- [ ] Any rebuild, re-bundle, re-sign or manifest regeneration after this point invalidates all later lifecycle and checksum evidence.

If the owner expressly approves an unsigned **outer** release, parent/helper
still remain signed because the helper trust protocol requires them. Release
notes must accurately disclose SmartScreen and the unsigned uninstaller. This is
not the default path and must never be inferred from a missing certificate.

---

## 4. Canonical artifact lifecycle, private proofs and checksums (owner-performed)

Generate checksums **after** the signing decision and after the exact final binaries pass the lifecycle runner. `release-assets/` must not already exist; the script refuses to reuse staging so stale files cannot survive:

- [ ] **Final artifact state is fixed** — section 3 proved setup plus embedded uninstaller signing for both editions, or the owner explicitly chose the documented unsigned-outer path. Inner parent/helper signatures and RSA-PSS identities are mandatory in both cases. Signing or rebuilding after this point invalidates every later lifecycle/hash record.
- [ ] **Run the canonical lifecycle with explicit inputs and a new evidence directory** — invoke `scripts/sandbox-lifecycle.ps1` with both versions, all three installers/helpers and a previously unused `EvidenceDir`. `-Resume` is only for that same hash-bound evidence set; it validates the host-only provenance before writing `stop.flag`.
- [ ] Revalidate the evidence without starting Sandbox: `pwsh scripts/sandbox-lifecycle.ps1 -ValidateOnly -EvidenceDir <exact-evidence-dir> -BaselineVersion 0.1.1 -CandidateVersion 0.1.3`. It must contain exactly the authoritative result set, no accepted historical failure, unchanged shared-input hashes, and schema-3 installed-artifact records binding each setup to the role/path identity/length/hash of the parent, helper, MCP (Connector) and uninstaller actually observed in the guest.
- [ ] **Create the private final-byte release proof** — run `scripts/release-artifact-proof.ps1 -Create` into a new approved proof directory with owner authorization, the structured signing decision, expected signer subject/thumbprint, release-root public blob, both receipt and release-identity paths plus independent hashes, exact setup/installed parent/helper/uninstaller paths, Connector MCP, and lifecycle manifest/hash. It validates target/Cargo.lock/bundle-contract/frontend/timestamp fields from both schema-3 receipts, rejects supplied binaries that differ from lifecycle-observed installed bytes, and checks RFC3161 `messageImprint` against the exact primary Authenticode signature. Parent/helper must be distinct and validly signed in both modes; setup/uninstaller must be valid in `Signed` mode or honestly `NotSigned` with explicit `-OwnerAcceptUnsignedOuter` in `Unsigned` mode. Record the printed report hash outside the directory and immediately revalidate it with `-ValidateOnly -ExpectedReportSha256`.
- [ ] **Seal and revalidate private 50-gate acceptance** — every schema-2 `gate-proofs/<ID>.json` is source/spec-bound to its exact canonical producer, test IDs and shared evidence. Automated gates accept no arbitrary per-gate payload; supervised/owner gates require exactly their typed, release-proof-bound attestation/report below the unique `evidence/<ID>/` subtree. Duplicate payload hashes across gates are rejected; reusable evidence exists only in the explicit shared registry. `OWNER-02` includes the exact private schema-3 MCP report bound to the release proof. Run `acceptance-v013.ps1 -Finalize`, record the printed schema-3 private-report hash independently, then run `-ValidateOnly` with both expected hashes. Keep all private evidence out of Git and uploads.
- [ ] Run `pwsh scripts/checksums.ps1 -ExpectedVersion 0.1.3 -LifecycleEvidenceDir <exact-lifecycle-dir> -AcceptanceEvidenceDir <exact-private-acceptance-dir> -ExpectedPrivateAcceptanceSha256 <private-report-sha256> -ReleaseArtifactProofDir <exact-proof-dir> -ExpectedReleaseArtifactProofSha256 <proof-report-sha256> -SigningDecision Signed` (or `Unsigned`) from the exact clean release commit. It revalidates all three private inputs and stages exactly two portable-name installers plus the closed public `ACCEPTANCE-EVIDENCE.json`, `SHA256SUMS` and `RELEASE-MANIFEST.json` under `target/release/bundle/nsis/release-assets/`.
- [ ] Inspect the public acceptance projection and manifest: they contain source/specification, structured signing/receipt/identity/artifact/MCP/lifecycle hashes and 50 gate status/digests only — no private claim, note, command/output, path, config or secret-bearing payload.
- [ ] Inspect `SHA256SUMS`: it must contain exactly the two final installer names/hashes. Attach it as the canonical checksum file and, if desired, copy that block into the external GitHub Release description. Do **not** edit tracked release notes after provenance is recorded.

---

## 5. Before you publish

- [ ] Both editions passed `PrepareSigning` → external parent/helper signing → RSA-PSS manifest → `BundleSigned` under outbound blocking with the exact pinned WebView input and receipt continuity.
- [ ] Each final setup and its installed uninstaller passed the owner signing gate (or the explicitly approved unsigned-outer scope is documented), and both editions were smoke-tested from reset no-WebView/no-network Windows images.
- [ ] The private release-artifact and 50-gate acceptance reports passed immediate validation against their independently recorded hashes; only their closed public acceptance projection is staged.
- [ ] `SHA256SUMS` generated **after** the signing decision and from the exact lifecycle-tested/proof-bound bytes; its two entries are ready to attach/copy into the external release description.
- [ ] [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) was finalised and committed **before** the canonical artifact run — date and signed/unsigned wording correct, no `PENDING` claim, known limitations current.
- [ ] Create the GitHub Release for the tag and upload all **five staged assets**: the two clear-edition installers, `ACCEPTANCE-EVIDENCE.json`, `SHA256SUMS` and `RELEASE-MANIFEST.json`.
- [ ] Download all five uploaded assets into a fresh directory. Verify both installers against `SHA256SUMS`, and byte-hash downloaded `ACCEPTANCE-EVIDENCE.json`, `SHA256SUMS` and `RELEASE-MANIFEST.json` against their local staged originals; a local build hash alone does not prove the uploaded bytes.
- [ ] Update the README Releases link if it changed.
- [ ] Confirm no automatic GitHub workflow or dependency bot was enabled by publication. Do **not** wait for or require a GitHub Actions result; credits are unavailable and the authoritative local gate was already sealed. Confirm the downloaded assets match the uploaded `SHA256SUMS` before announcing the release.

> Publishing pushes binaries to the public — it is a deliberate manual action. `BundleSigned` alone does not satisfy this checklist: setup/uninstaller signing, VM evidence, lifecycle, checksums and an explicit HOLD lift still precede upload.

---

_See also: [`docs/PACKAGING.md`](docs/PACKAGING.md) (build internals), [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) (the release-notes draft), [`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md) (security model)._
