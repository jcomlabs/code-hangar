# Packaging & release

Code Hangar ships as **two editions** built from the same source tree. This document is the release playbook.

## The two editions

| | Local | AI Connector |
|---|---|---|
| What it does | full local management **including safe delete** (backup → hold → final-remove) | Local + scoped local AI-app integration |
| Cargo features | `mutation` | `agent_automation` (⊇ `mutation`) |
| Tauri config | `tauri.conf.json` | `tauri.conf.json` + `tauri.connector.conf.json` |
| Network / AI | **none** (zero outbound — local-gate enforced) | local MCP plus an experimental, explicit loopback/HTTPS provider request through one allowlisted native client |
| Connector crates linked | none | `hangar-agent`, `hangar-mcp`, `hangar-appconfig`, `hangar-ai` |
| Ships `code-hangar-mcp.exe` | no | yes (Tauri sidecar, next to the app exe) |
| AI App Integration panel | absent from the compiled frontend bundle | shown |
| Canonical wrapper | `scripts/package-local.ps1` | `scripts/package-connector.ps1` |

The Local edition can delete (it links `hangar-mutation`) but is **100% local**:
its supported Windows graph rejects `hangar-ai`, `reqwest`, `keyring` and the
provider edge/source, while its checked bundle rejects provider/MCP chunks, IPC,
copy and CSS. Tauri already uses `url` transitively; Local does not falsely claim
a graph or lockfile without `url`, and no provider is reachable through it.

The Connector adds local MCP/app-configuration/named-pipe integration and one
feature-gated provider crate. Only `crates/hangar-ai` may use `reqwest 0.12`
(`default-features=false`, `blocking,json,native-tls`), `url 2` and `keyring 3`
(`default-features=false`, `windows-native`), activated by the single optional
`hangar-api` `agent_automation` edge. Telemetry, updater, remote Git, browser
network primitives, implicit/background requests and any second provider client
remain denied. `scripts/check-no-outbound-deps.mjs`,
`scripts/check-no-forbidden-code.mjs` and `scripts/check-frontend-edition.mjs`
enforce the manifest, source, supported Windows graph and physical bundle
boundaries. `local-ci.ps1` is the release authority; a remote workflow is not.

The supported desktop product and bundle configuration are **Windows x64 / NSIS
only**. `Cargo.lock` and Cargo's `--target all` metadata still contain
`reqwest 0.13.4` through Tauri 2.11.2's target-specific Android and non-macOS
Apple edge. Those targets are unsupported and are not compiled or shipped. The
guard checks that exact narrow metadata edge and version. In supported Windows
graphs, Local has no `reqwest`; Connector has exactly the reviewed `reqwest 0.12`
path through `hangar-ai`. The policy deliberately makes no false lockfile-wide
zero-`reqwest` claim.

**Naming & data.** The connector override sets a distinct `productName` ("Code Hangar AI Connector") so the two installers and Start-menu entries are unambiguous, but keeps the **same `identifier`** (`local.codehangar.desktop`) and is designed to use the same encrypted local catalog under `%APPDATA%`. The identifier is not proof of safe coexistence. Install-beside, edition switching, cross-uninstall and catalog/key preservation remain mandatory clean-Windows lifecycle gates for this candidate; do not promise that installing both is safe until those exact final bytes pass.

**Windows Explorer options.** Both NSIS installers use `src-tauri/windows/shell-integration.nsh` and offer two independent, default-off choices: register Code Hangar in `Open with` for Markdown extensions, and add context-menu commands for text files/folders. Silent/passive installs never enable a new choice and preserve an existing preference. Registrations are `HKCU`-only and never write `UserChoice`. The two editions record their executable paths; if the current registration owner is uninstalled, the hook rebinds to the other installed edition when it still exists.

**Resident startup.** The installed app exposes a separate **Start quietly with Windows** switch under Settings. It writes only the current user's Run value, quotes the exact executable and passes the fixed `--background` flag. Closing the window hides it in the notification area; the tray's Exit action ends the process. Uninstall removes the Run value only when it still points at the executable being removed, so it cannot erase a value another edition has taken over.

## Build

Packaging is an offline, fail-closed step. Dependency provisioning is separate:
`node_modules` must already have been installed from the tracked `package-lock.json`.
The package scripts never run `npm ci`, never run `npx`, and never fetch a missing
tool. They require `node_modules/.bin/tauri.cmd`, Vite and TypeScript from this
exact worktree and require each installed version to equal its exact lockfile
version. They also bind the Windows x64 native packages used by Tauri, Rolldown
and Lightning CSS to their exact parent/lock versions and to non-empty PE files
whose real paths remain inside this worktree. Their SHA-256 values are pinned to
the three locally audited locked artefacts; changing any native byte fails the
preflight and requires an explicit dependency re-audit. Non-empty `NODE_PATH`,
`NODE_OPTIONS`, NAPI loader overrides, Rust compiler/wrapper flags, Cargo
profile/target linker-runner overrides and every `TAURI_*` environment override
are refused. Workspace-local `.bin` directories below `apps/` must be empty, so
Tauri's `beforeBuildCommand` cannot resolve shadow shims ahead of the verified
root tools.

The pinned CLI loader itself contains a vendor-generated stale native-version
check (`2.10.1`) even though the tracked lock metadata and installed CLI/native
package are `2.11.2`. `NAPI_RS_ENFORCE_VERSION_CHECK` is therefore also refused:
enabling it would reject the correct locked binding. Any Tauri CLI upgrade must
re-audit both the loader and the locally embedded bundler evidence before the
pin is changed.

From the repo root (Rust stable + Node 24 + Tauri Windows prerequisites already
installed), run the non-packaging checks first:

```powershell
pwsh -NoProfile -File scripts/package-local.ps1 -SelfTest
pwsh -NoProfile -File scripts/package-connector.ps1 -SelfTest

# Use one explicitly chosen local file. The wrappers never search/select a cache.
$webView2 = 'D:\release-inputs\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
pwsh -NoProfile -File scripts/package-connector.ps1 -PreflightOnly -WebView2InstallerPath $webView2
pwsh -NoProfile -File scripts/package-local.ps1 -PreflightOnly -WebView2InstallerPath $webView2
```

Connected-app acceptance has two deliberately separate flows. The offline
`scripts/mcp-fixture-smoke.ps1` path is fully synthetic and deterministic. The
Claude path uses a real authenticated client only during an explicit supervised
  owner gate, but still exposes only a synthetic fixture and a temporary strict
  MCP config. It never registers or unregisters the user's live Claude config.
  It requires independently hashed exact paths for the Claude executable,
  schema-3 Connector signing receipt and receipt-bound candidate sidecar, plus an
  explicit config root whose qualified `.claude.json` state must remain unchanged:

```powershell
$run = '.local\acceptance\v0.1.3\candidate\<current-run-id>'
$server = "$connectorPrep\code-hangar-mcp.exe"
$serverSha = (Get-FileHash -LiteralPath $server -Algorithm SHA256).Hash.ToLowerInvariant()
$receipt = "$connectorPrep\code-hangar-signing-receipt.json"
$receiptSha = '<independently recorded Connector receipt SHA-256>'
$claude = 'C:\exact\audited\path\claude.exe'
$claudeSha = (Get-FileHash -LiteralPath $claude -Algorithm SHA256).Hash.ToLowerInvariant()
$claudeConfigRoot = 'C:\exact\qualified\Claude-config-root'
$claudeEvidence = Join-Path $run 'evidence\OWNER-02\claude-live-run'
pwsh -NoProfile -File scripts/mcp-claude-real.ps1 `
  -OwnerAuthorized `
  -EvidenceDir $claudeEvidence `
  -ServerPath $server `
  -ExpectedSha256 $serverSha `
  -SigningReceiptPath $receipt `
  -ExpectedSigningReceiptSha256 $receiptSha `
  -ClaudeExecutablePath $claude `
  -ExpectedClaudeExecutableSha256 $claudeSha `
  -ClaudeConfigRoot $claudeConfigRoot
```

The separate clean-user Connector journey still proves the visible
Connect/register/read/Disconnect lifecycle and byte-safe config restoration.
Neither flow may be presented as evidence for the other.

The self-tests cover the lock, same-edition cleanup, preservation of the other
edition, exact filename, PE/hash validation, stale-file timestamp spoofing,
native/tool version mismatch, environment overrides and workspace shim
shadowing. They also exercise signing-receipt schema, externally supplied
receipt hash, edition/version/target/root/`Cargo.lock`/bundle-contract
continuity, exact parent/helper/verifier/MCP evidence, verifier/MCP tamper
rejection, and byte-exact atomic staging. WebView tests cover missing/wrong inputs, length/hash/version and
signer-evidence mismatches, reparse ancestry, unsafe NSIS quoting, denial of
write/delete while an input is locked, and Connector/release-overlay/override
ordering. They use only uniquely named temporary directories and perform no
product build or signing.

### Explicit pinned WebView2 input

Canonical packaging requires `-WebView2InstallerPath`; it never scans a cache,
chooses the first matching filename or follows a redirect. The tracked
`scripts/release-inputs/webview2-x64.json` is an independently reviewed pin for
one exact Microsoft offline installer. Before Tauri can start, the wrapper
requires all of the following to match that manifest:

- exact filename and byte length;
- SHA-256 and PE Machine (`014C` is expected for this Microsoft bootstrapper);
- file version;
- embedded Authenticode CMS signature, Microsoft leaf subject/thumbprint and
  issuer;
- RFC3161 timestamp signature/thumbprint;
- offline certificate chains plus cache-only `WinVerifyTrust` validation.

The explicit path and every ancestor must be fixed, local and non-reparse. The
wrapper opens the source with `FileShare.Read`, stages a same-hash safe-named
copy below this worktree's `.local/packaging-generated/`, opens that copy with
the same write/delete-denying share mode, and holds both streams until Tauri and
NSIS finish. Generated inputs are removed only after those locks close. The
worktree-wide packaging lock remains held for that whole interval.

The base tracked Tauri config deliberately remains `offlineInstaller`, so a
non-canonical invocation cannot silently omit the runtime. Canonical packaging
generates a final override that sets `createUpdaterArtifacts: false`,
`minimumWebview2Version: null`, `webviewInstallMode: { type: "skip" }` and the
generated `installerHooks` path. For Connector bundling, repeated configs are
passed in the audited Tauri 2.11.2 merge order: base (implicit), Connector,
Connector release overlay, then the generated WebView override. For Local, the
Local release overlay and generated override follow the implicit base.
Compile-only preparation omits the bundle-only release overlay because its
signed helper/manifest inputs do not yet exist. The generated override does not
set `externalBin`, so it cannot replace the exact release sidecar arrays.

The generated hook is compiled with the validated local NSIS 3.11 preprocessor
before the product build. The tracked shell hook refuses to compile without the
canonical generated macro, and that macro is the first instruction of the
single `NSIS_HOOK_PREINSTALL`. It embeds the locked staged EXE, hashes the
extracted copy with fully qualified .NET APIs under absolute Windows PowerShell
(`-NoProfile -NonInteractive`), compares both the process result and returned
hash to the literal pin, and prints the verified extracted SHA-256 in installer
details. Only then does it execute `/silent /install`. A hash failure or any
non-zero WebView installer exit aborts before Code Hangar files, registry values
or shortcuts are installed; reboot-required codes are not converted to success.
There is no URL, updater, EdgeUpdate or download fallback in the override/hook.

The canonical wrapper also hashes the complete local Tauri NSIS 3.11 cache as a
path/length/SHA-256 tree: exactly 442 non-reparse files must match digest
`037d77f1f7359f9cc5e5f90842ea28dd8b8f17c8f5d35f0a7266f534e700e619`.
This covers includes, stubs, plugins and executables beyond the individually
checked compiler/plugin markers. `BundleSigned` revalidates that tree
immediately before and after Tauri bundling.

`fixedRuntime` remains a fallback design only if the pinned preinstall hook
cannot pass the real isolated-VM acceptance gate. It would require a separately
reviewed manifest covering every runtime file; it is not an implicit or
auto-selected alternative.

### Two explicit release phases

There is deliberately no default package action. Calling either wrapper without
exactly one of `-PreflightOnly`, `-PrepareSigning`, `-BundleSigned` or
`-SelfTest` fails before a build or bundle. A release-root RSA public blob is
public material but must be supplied explicitly; neither wrapper creates a key,
certificate or production trust root.

Run preparation **sequentially**, because both editions reuse
`apps/desktop/dist`. Each preparation captures the checked edition's frontend
as an immutable hash-bound snapshot. Use new signing directories; the scripts
refuse to reuse or overwrite one:

```powershell
$releaseRoot = (Get-Content -LiteralPath 'D:\release-inputs\release-root-public-blob.hex' -Raw).Trim()
$connectorPrep = 'D:\release-work\0.1.3-connector'
$localPrep = 'D:\release-work\0.1.3-local'

pwsh -NoProfile -File scripts/package-connector.ps1 -PrepareSigning `
  -WebView2InstallerPath $webView2 -ReleaseRootPublicBlobHex $releaseRoot `
  -SigningDirectory $connectorPrep
pwsh -NoProfile -File scripts/package-local.ps1 -PrepareSigning `
  -WebView2InstallerPath $webView2 -ReleaseRootPublicBlobHex $releaseRoot `
  -SigningDirectory $localPrep
```

`PrepareSigning` compiles the correct parent graph, elevated helper and release
verifier with `CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX` present at compile
time. Connector also compiles MCP. Every explicit Cargo command is
`--locked --offline`; Tauri then runs `build --no-bundle --no-sign` for the
correct edition. No installer is produced. The new directory contains:

- `code-hangar-desktop.exe` and `code-hangar-elevated.exe`, which the owner must
  Authenticode-sign externally;
- `code-hangar-release-verify.exe`, which must remain byte-identical;
- Connector only: `code-hangar-mcp.exe`, which must remain byte-identical; and
- `frontend-dist/` plus `code-hangar-frontend-dist.json`, the checked frontend
  tree and its edition-labelled canonical tree digest; and
- `code-hangar-signing-receipt.json` (schema 3), binding the exact clean source
  commit/tree, edition, version, target triple, public root, `Cargo.lock`, every
  prepared hash, the frontend snapshot, and a
  canonical bundle-contract digest over the edition's Tauri configs/overlay,
  NSIS hook, WebView pin, frontend-edition checker, Authenticode evidence
  checker, package metadata/lock and release scripts. For parent and helper the
  receipt also records a stable PE image digest that excludes only the
  Authenticode certificate table, PE checksum and PE security-directory entry.

Record the receipt SHA-256 printed by each preparation outside that directory.
`BundleSigned` requires it as `-ExpectedSigningReceiptSha256`; this prevents a
replacement receipt from authorizing a replacement verifier/MCP. The receipt is
opened once with write/delete denied; `BundleSigned` hashes and parses those same
locked bytes, so there is no close-and-reopen replacement window between the
external-hash check and validation. Omitting the expected receipt hash, supplying
the other edition's hash, or changing any receipt-bound field fails before any
bundle command.

Record the two receipt hashes before creating the detached manifests. The
manifest `release_id` is deterministic: it is SHA-256 over a fixed domain tag,
the exact edition and that receipt hash. Consequently a valid owner manifest
from the other edition or another preparation receipt is not acceptable even if
the same offline root key signed it.

```powershell
$connectorReceiptSha = '<64-hex SHA-256 printed by Connector PrepareSigning>'
$localReceiptSha = '<64-hex SHA-256 printed by Local PrepareSigning>'
```

After external Authenticode signing of parent and helper, create one detached
manifest per edition with the offline owner-held RSA private key. The exact
manifest leaf is `code-hangar-release-manifest.json`, and its entries must remain
`code-hangar-desktop.exe` and `code-hangar-elevated.exe`:

```powershell
pwsh -NoProfile -File scripts/new-release-identity-manifest.ps1 `
  -ParentPath "$connectorPrep\code-hangar-desktop.exe" `
  -HelperPath "$connectorPrep\code-hangar-elevated.exe" `
  -PrivateKeyPath 'X:\offline-key\release-root-private.pem' `
  -ExpectedPublicBlobHex $releaseRoot `
  -Edition Connector -SigningReceiptSha256 $connectorReceiptSha `
  -OutputPath "$connectorPrep\code-hangar-release-manifest.json"
```

The manifest script opens the private key without sharing, proves it matches the
explicit public blob, hashes the post-Authenticode bytes while locked, derives
the receipt-bound release identity, and signs the canonical payload with
RSA-PSS/SHA-256. It never signs an executable and never contacts a network.
Repeat for Local with its own signed parent/helper and `$localReceiptSha`. The
signed parent/helper and manifest must be returned under their canonical names
in the receipt-bound signing directory; `BundleSigned` refuses alternate paths
so an operator cannot accidentally authorize files from another edition.

Bundle the receipt-bound inputs, again sequentially:

```powershell
pwsh -NoProfile -File scripts/package-connector.ps1 -BundleSigned `
  -WebView2InstallerPath $webView2 -ReleaseRootPublicBlobHex $releaseRoot `
  -SigningDirectory $connectorPrep -ExpectedSigningReceiptSha256 $connectorReceiptSha `
  -SignedParentPath "$connectorPrep\code-hangar-desktop.exe" `
  -SignedHelperPath "$connectorPrep\code-hangar-elevated.exe" `
  -ReleaseManifestPath "$connectorPrep\code-hangar-release-manifest.json"

pwsh -NoProfile -File scripts/package-local.ps1 -BundleSigned `
  -WebView2InstallerPath $webView2 -ReleaseRootPublicBlobHex $releaseRoot `
  -SigningDirectory $localPrep -ExpectedSigningReceiptSha256 $localReceiptSha `
  -SignedParentPath "$localPrep\code-hangar-desktop.exe" `
  -SignedHelperPath "$localPrep\code-hangar-elevated.exe" `
  -ReleaseManifestPath "$localPrep\code-hangar-release-manifest.json"
```

`BundleSigned` does not compile. It first matches the receipt to the
owner-recorded SHA-256, then revalidates edition/version/triple, the explicit
release root, current `Cargo.lock`, the canonical bundle-contract digest, the
prepared verifier/MCP hashes and the exact frontend snapshot. It also requires
each signed parent/helper to retain its prepared PE image digest while changing
its full bytes through the permitted Authenticode fields. The receipt-bound Rust
verifier then holds parent/helper open, verifies the RSA-PSS manifest, exact
post-sign hashes, the receipt-bound release identity, distinct filesystem
objects and cache-only Authenticode chains. Only after that proof does the
wrapper place the exact signed parent at `target/release/code-hangar-desktop.exe`,
stage the helper as `binaries/code-hangar-elevated-<target-triple>.exe`, stage
the manifest resource, and, for Connector only, stage the unchanged
receipt-bound MCP sidecar. Local removes all Connector staging.

All bundle inputs and every existing frontend-snapshot file are held against
write/delete while Tauri runs `bundle --no-sign`. Immediately before bundling,
the wrapper moves the worktree's current `apps/desktop/dist` into its generated
run directory, restores only the receipt-bound snapshot at that path, verifies
the edition marker and tree again, and always restores the original worktree
`dist` in `finally`. It repeats the snapshot/tree checks after bundling. The
wrapper then rehashes each input, reconstructs the
installed parent/helper/manifest layout, runs the same verifier again, requires
the same release identity, and accepts exactly one fresh exact-version NSIS
file. A stale installer, touched creation time, changed `Cargo.lock`, mutated
input, wrong edition, wrong manifest name, modified verifier or modified MCP
fails closed. The wrapper also requires the resulting outer setup to report
Authenticode `NotSigned`, rehashes it around that check, and labels it only as a
raw **UNSIGNED HOLD** candidate. It makes no signed-uninstaller claim.

Tauri strips each sidecar triple at installation. It installs
`code-hangar-elevated.exe` and, for Connector, `code-hangar-mcp.exe` next to the
desktop executable; the manifest resource is installed there too. The Local
release overlay contains no MCP sidecar.

The desired shipped installers include the explicitly proven WebView2 runtime,
so a clean Local install does not contact the network or fail merely because
WebView2 is absent. The tracked `offlineInstaller` setting expresses that
product requirement for non-canonical invocations; the canonical last override
and pinned hook provide explicit provenance without trusting Tauri's dynamic
WebView cache. `check:security` rejects a return to Tauri's online
`downloadBootstrapper`, updater or EdgeUpdate paths.

Both package scripts set `CARGO_NET_OFFLINE=true` and
`npm_config_offline=true`. `PrepareSigning` primes every graph explicitly with
`--locked --offline`, keeps Cargo offline during Tauri's compile-only pass and
rejects a changed `Cargo.lock`. `BundleSigned` invokes no compiler and likewise
rejects a changed lock. Those settings are necessary but are not proof that all
third-party bundler paths are offline. Run both phases while outbound network is
blocked at the OS/sandbox boundary; the third-party bundler has no complete
zero-network guarantee.

`pwsh scripts/local-ci.ps1 -CoreOnly` never creates a Tauri/NSIS package named
Code Hangar Local. Without `-SkipTauriBuild`, it compiles a core-only release
binary directly with Cargo and explicitly produces no installer; `mutation` is
the only feature lane that may produce the shipped Local bundle.

Any raw installers remain under HOLD. The wrappers never create canonical
`release-assets/`, checksums, a private key or a release-identity manifest.
`-BundleSigned` means the inner parent/helper are signed and identity-bound; it
does **not** mean the outer NSIS chain is release-signed. The exact outer setup is
proven `NotSigned`; the embedded uninstaller remains unclaimed until an audited
owner-certificate `signCommand` flow and reset-VM verification exist. A real
outbound-blocked run and isolated Windows Sandbox/VM acceptance are not performed
by preflight.

Before lifecycle/canonical staging, test **both** raw editions in a fresh
Windows Sandbox/VM with outbound network blocked and WebView2 absent. Capture
the installer detail line `Pinned WebView2 extracted SHA256 verified: ...` and
prove it equals the tracked manifest; prove the runtime installs without a
network request, Code Hangar then installs and launches, and edition identity is
correct. Repeat from a reset image for the other edition. These are release
evidence requirements, not claims made by the packaging preflight.

## Versioning

Bump the version together in `Cargo.toml` (`[workspace.package]`), root
`package.json`, `apps/desktop/package.json`, `package-lock.json` (top-level,
root and desktop workspace entries), and
`apps/desktop/src-tauri/tauri.conf.json`. Preflight requires exact coherence and
derives these filenames only:

- `Code Hangar_<version>_x64-setup.exe`
- `Code Hangar AI Connector_<version>_x64-setup.exe`

## Icons

The icon set under `apps/desktop/src-tauri/icons/` is generated from `apps/desktop/src-tauri/app-icon.svg`:

```powershell
Set-Location apps/desktop
& ..\..\node_modules\.bin\tauri.cmd icon src-tauri/app-icon.svg
```

(Then delete the generated `ios/` and `android/` folders — this is a desktop-only app.)

## Code signing owner gates

The inner release identity and the NSIS signing chain are separate gates:

1. Before `BundleSigned`, the owner Authenticode-signs the desktop parent and
   elevated helper. The detached RSA-PSS manifest binds their final signed
   hashes, and the receipt-bound verifier proves both offline.
2. `tauri bundle --no-sign` deliberately performs no certificate operation. The
   wrapper proves the output setup is `NotSigned` and keeps it as a raw UNSIGNED
   HOLD candidate. Because no audited uninstaller `signCommand` ran, the pipeline
   makes no signed-uninstaller or complete-chain claim.
3. Signing only the outer `*-setup.exe` after bundling is **not sufficient**: it
   does not retroactively sign the embedded uninstaller. A fully signed release
   needs an audited owner-certificate Tauri/NSIS signing command that supplies
   the uninstaller signing command during bundle creation and signs/verifies the
   final setup too.
4. A reset Windows VM must prove valid Authenticode on the installed parent,
   helper and uninstaller, plus the setup before execution. It must also prove
   clean uninstall. Record signer/timestamp evidence and exact hashes.

The OV/EV certificate and private key are owner-held inputs and must never live
in the repository. This pipeline neither fabricates them nor claims that the raw
candidate is fully signed. An explicitly approved unsigned outer release would
still require signed parent/helper for the helper trust protocol, plus an honest
SmartScreen/unsigned-uninstaller disclosure; it is not the default release path.

### Private final-byte release proof

After the signing decision is final and the exact installers have passed the
source-bound lifecycle, create one new private proof directory. The proof script
does not sign, bundle or contact a network. It opens each caller-supplied input
with write/delete sharing denied, copies those bytes into an immutable local
snapshot, and performs hash, receipt, release-identity, Authenticode and
timestamp checks on the same locked snapshot bytes. RFC3161 evidence is not
accepted from time alone: its `messageImprint` digest must bind the exact primary
Authenticode signature. Parent and elevated helper
must always be distinct and Authenticode-valid. In `Signed` mode, setup and the
installed uninstaller must also be validly signed; in `Unsigned` mode they must
honestly be `NotSigned` and the owner must explicitly accept the SmartScreen and
unsigned-uninstaller disclosure.

```powershell
$proof = '.local\acceptance\v0.1.3\release-proof\<new-proof-id>'
$proofArgs = @{
  Create = $true
  EvidenceDir = $proof
  SigningDecision = 'Signed' # or 'Unsigned'
  OwnerAuthorized = $true
  ExpectedSignerSubject = '<exact Authenticode signer subject>'
  ExpectedSignerThumbprint = '<canonical uppercase signer thumbprint>'
  ReleaseRootPublicBlobHex = $releaseRoot
  LocalSigningReceiptPath = "$localPrep\code-hangar-signing-receipt.json"
  ExpectedLocalSigningReceiptSha256 = $localReceiptSha
  ConnectorSigningReceiptPath = "$connectorPrep\code-hangar-signing-receipt.json"
  ExpectedConnectorSigningReceiptSha256 = $connectorReceiptSha
  LocalReleaseIdentityPath = "$localPrep\code-hangar-release-manifest.json"
  ExpectedLocalReleaseIdentitySha256 = '<independently recorded lowercase SHA-256>'
  ConnectorReleaseIdentityPath = "$connectorPrep\code-hangar-release-manifest.json"
  ExpectedConnectorReleaseIdentitySha256 = '<independently recorded lowercase SHA-256>'
  LocalSetupPath = '<exact final Local setup>'
  LocalParentPath = '<exact lifecycle-extracted Local parent>'
  LocalHelperPath = '<exact lifecycle-extracted Local helper>'
  LocalUninstallerPath = '<exact lifecycle-extracted Local uninstaller>'
  ConnectorSetupPath = '<exact final Connector setup>'
  ConnectorParentPath = '<exact lifecycle-extracted Connector parent>'
  ConnectorHelperPath = '<exact lifecycle-extracted Connector helper>'
  ConnectorUninstallerPath = '<exact lifecycle-extracted Connector uninstaller>'
  ConnectorMcpPath = '<exact lifecycle-extracted Connector MCP sidecar>'
  LifecycleManifestPath = '<exact lifecycle-dir>\lifecycle-manifest.json'
  ExpectedLifecycleManifestSha256 = '<independently recorded lowercase SHA-256>'
}
# For the explicitly accepted unsigned-outer path only:
# $proofArgs.OwnerAcceptUnsignedOuter = $true
pwsh -NoProfile -File scripts/release-artifact-proof.ps1 @proofArgs
$proofSha = '<lowercase SHA-256 printed by Create and recorded outside the proof dir>'
pwsh -NoProfile -File scripts/release-artifact-proof.ps1 -ValidateOnly `
  -EvidenceDir $proof -ExpectedReportSha256 $proofSha
```

The private `RELEASE-ARTIFACT-PROOF.private.json` binds the clean source,
structured `SigningDecision`, current `Cargo.lock`, target triple,
bundle-contract digest, frontend manifest/tree and preparation timestamps from
both schema-3 receipts, both RSA-PSS release identities,
setup/parent/helper/uninstaller hashes and signature metadata, Connector MCP
hash, and the schema-3 lifecycle. That lifecycle exports the edition, role,
canonical installed path identity, byte length and hash for the **actually
installed** parent/helper/MCP/uninstaller tied to each exact setup hash. The
proof rejects caller snapshots that differ from those observed installed bytes.
Keep the entire proof private; later public files contain only its hash and
closed structured bindings.

## Publishing (manual — maintainer action)

1. Finalize the tracked release notes and limitations **first**, commit them, and
   start provenance only from that exact clean commit/tree. Do not edit tracked
   notes after any receipt or final artifact is created.
2. Complete `PrepareSigning`, external parent/helper Authenticode signing,
   release-identity creation and `BundleSigned` for both editions under outbound
   blocking; keep every output under HOLD. Complete either the audited signed
   setup/uninstaller flow or the explicit unsigned-outer decision. Any rebuild,
   repack or signature change invalidates every later proof.
3. Run the canonical `scripts/sandbox-lifecycle.ps1` flow with explicit final
   installer/helper paths and a new evidence directory. Revalidate the exact
   evidence with `-ValidateOnly`; `-Resume` is only for that same hash-bound set.
4. Create and immediately revalidate the private final-byte release proof above;
   independently record its printed SHA-256.
5. Complete the 50 dedicated gate envelopes/evidence subtrees, then seal and
   revalidate `ACCEPTANCE-EVIDENCE.private.json` with
   `scripts/acceptance-v013.ps1`, supplying the proof directory/hash and the
   independently recorded private-report hash. The tracked Markdown never
   receives candidate paths, hashes or statuses.
6. Run `scripts/checksums.ps1` with `-ExpectedVersion`, lifecycle and acceptance
   directories, `-ExpectedPrivateAcceptanceSha256`, release-proof directory,
   `-ExpectedReleaseArtifactProofSha256` and the matching `-SigningDecision`.
   This is the sole route that may create canonical `release-assets/`; it exports
   only the closed public acceptance projection.
7. Verify the five staged public files. After the owner lifts HOLD, upload only
   those exact bytes, download all five into a fresh directory, and reproduce
   every local staged hash before announcing the release.

> Packaging, lifecycle validation, canonical staging and publication are separate gates. Raw packaging output is not ready to upload, and no publication occurs while HOLD remains in force.

## Pre-release checklist

- [ ] `pwsh scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild` is green for all non-packaging lanes, including compile-only Windows Local/Connector desktop releases, Connector backend Clippy and the sidecar; no raw Tauri bundle is accepted as evidence. This local run is authoritative. The repository contains no GitHub Actions workflow or Dependabot configuration; remote automation is neither required nor an equivalent release gate.
- [ ] Version is coherent across Cargo, Tauri, both package manifests and all relevant package-lock entries.
- [ ] Packaging self-tests and the exact worktree toolchain/native-binding preflight pass.
- [ ] Both `-PreflightOnly` commands pass for the explicit installer against `scripts/release-inputs/webview2-x64.json`; arbitrary cache candidates are never selected or accepted.
- [ ] Both `PrepareSigning` runs use new directories, the same explicit public root and pinned WebView input, produce no installer, and their printed receipt SHA-256 values are recorded outside those directories. Each schema-3 receipt binds the exact clean Git commit/tree, its edition-labelled frontend snapshot and parent/helper stable PE-image digests.
- [ ] Owner Authenticode-signs each prepared parent/helper, generates a separate RSA-PSS identity manifest with that edition and receipt SHA-256, and keeps each receipt-bound verifier/MCP unchanged.
- [ ] Connector then Local `BundleSigned` runs under outbound blocking; each pre/post verifier proof has one stable receipt-bound release identity, temporarily restores only its matching locked frontend snapshot and restores the prior worktree `dist`, Connector alone stages MCP, and each exact raw HOLD candidate path/hash is captured.
- [ ] From reset Windows Sandbox/VM images with WebView2 absent and outbound blocked, each edition shows the exact extracted SHA-256 pin, installs the runtime, installs Code Hangar and launches with the correct edition identity.
- [ ] Both editions installed on a **clean** Windows user and launched: Local shows no AI panel; connector connects a real AI app (register → MCP round-trip → remove config).
- [ ] The exact v0.1.3 MCP sidecar passed `mcp-claude-real.ps1` with `-OwnerAuthorized`, exact independently hashed server, schema-3 Connector receipt, exact independently hashed Claude executable and explicit qualified config root; its private schema-3 report proves receipt/source/edition/version/sidecar continuity and unchanged before/after config.
- [ ] On a clean Windows user, exercise all four installer choices (neither / Markdown / context menu / both), confirm Default Apps remains user-controlled, and open both a known and unknown Markdown file/folder from Explorer. A known file must preview before its refresh completes; an unknown file must open directly in temporary Viewer; an unknown folder must still show Viewer/Automatic/Manual. Verify Viewer stays temporary/isolated, Automatic shows and registers the detected root, Manual rejects a root that does not contain the target, then uninstall the owning edition and verify rebind/removal.
- [ ] Enable Start quietly with Windows, confirm a `--background` sign-in launch shows only the tray icon, confirm Close hides and tray Exit ends the process, and confirm uninstall removes only a Run value owned by that executable.
- [ ] Owner-certificate NSIS flow signs and verifies both each setup and its embedded installed uninstaller; signing only the outer setup is not accepted as a fully signed chain.
- [ ] Reset VMs verify Authenticode on setup, installed parent/helper and uninstaller, then complete clean uninstall. Any expressly approved unsigned outer path is accurately disclosed.
- [ ] The canonical lifecycle and private final-byte release proof passed immediate hash-bound revalidation; the private acceptance report then sealed all 50 unique gate envelopes and passed revalidation. Only then `checksums.ps1`, with both independently recorded private hashes, created fresh `release-assets/` with two installers, the closed public `ACCEPTANCE-EVIDENCE.json`, `SHA256SUMS` and `RELEASE-MANIFEST.json`.
- [ ] HOLD was explicitly lifted before any upload/publication, and release notes were finalized from `RELEASE_NOTES.md`.
