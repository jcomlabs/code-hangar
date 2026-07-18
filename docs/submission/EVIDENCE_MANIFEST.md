# Candidate evidence manifest

Generated locally on **18 July 2026**. This is a sanitized summary of the final
candidate run; it contains no API key, personal project path, prompt, source
code excerpt, or private repository URL.

## Candidate identity

| Item | Value |
|---|---|
| Branch | public `submission/openai-build-week` |
| Declared pre-event baseline | `843530c` (12 July 2026) |
| Public release candidate | `c5cabbec8f5127fdf126d3ddb5e4c72a638e0931` (`v0.1.2-alpha`) |
| Private provenance range | `843530c..e831c14dfa15291dda152d7742766221438feaa3` |
| Eligible-range size | 10 commits; 130 files changed; +30,676 / -4,119 |
| Public general layer | `a6378adbf2a5b11200d90c14542a279d951519b6` (`main`) |
| Submission package layer | `c5cabbec8f5127fdf126d3ddb5e4c72a638e0931` |
| Evidence layer | later documentation-only commit above the release tag |
| Version | `0.1.2` across Cargo workspace, Tauri, npm root/workspace, and lockfiles |
| Validation host | Windows 11 Pro x64, build 26200 |
| Candidate worktree | clean before packaging and evidence generation |

The application binaries were built from the product candidate. The later
evidence commit changes documentation only, avoiding a self-referential binary
hash or candidate hash.

## Validation results

| Command / gate | Result |
|---|---|
| `npm run check` | PASS — TypeScript, ESLint, 47 Vitest files / 430 tests, dependency guard, forbidden-code guard |
| `node scripts/publication-audit.mjs` and `node scripts/secret-scan.mjs` | PASS — 310 files audited; 288 text files plus Git patch history scanned |
| `npm audit --audit-level=high` | PASS — 0 vulnerabilities |
| Rust 1.97 `cargo fmt --all --check` and warnings-denied workspace Clippy | PASS |
| `cargo test --workspace --no-default-features --features core` | PASS — complete core workspace; environment/data/stress ignores remain explicitly unclaimed |
| GitHub Actions `main` | PASS — [run 29644002665](https://github.com/jcomlabs/code-hangar/actions/runs/29644002665) |
| GitHub Actions release candidate | PASS — [run 29644078144](https://github.com/jcomlabs/code-hangar/actions/runs/29644078144) |
| `npm --workspace apps/desktop run package:connector` | PASS — release MCP sidecar staged and Connector NSIS created |
| `npm --workspace apps/desktop run package:local` | PASS — staged Connector sidecar removed, Local isolation rerun, Local NSIS created |
| `scripts/checksums.ps1` | PASS — current-version release copies matched their source hashes |
| Public release re-download and independent SHA-256 verification | PASS — both downloaded executables matched the published `SHA256SUMS` |
| Exact public 0.1.2 installer lifecycle on the build host | NOT RUN — avoided to preserve the existing installed application and parallel sessions; earlier candidate lifecycle evidence is not promoted to the final release |
| `scripts/submission/sandbox-candidate-lifecycle.ps1` | BLOCKED PRE-PRODUCT — final downloaded Local setup was refused by guest Application Control in an empty network-disabled Sandbox; the fail-fast run left the guest clean and did not execute Connector |
| `scripts/submission/codex-gpt56-mcp-smoke.ps1` against the final compiled sidecar | PASS — ChatGPT-authenticated Codex 0.144.1 ran GPT-5.6 Sol, completed two audited MCP reads, revoked the temporary credential, and created zero persisted session files |

Representative Rust results from the final gate:

- `hangar-ai`: 26 passed, 1 ignored live-local Ollama check;
- `hangar-api`: 163 passed, 6 explicitly ignored environment/data-stress checks;
- `hangar-db`: 92 passed, 1 ignored file-backed performance fixture;
- MCP wire/integration suite: 20 passed;
- focused GPT-5.6 tests passed for direct-OpenAI request shape, exact disclosure,
  and non-OpenAI compatible-endpoint behavior.

Ignored environment, real-user-data, WSL, live-local-model, and stress lanes are
not counted as passes and are not represented as completed acceptance evidence.

## Release artifacts

Local directory:
`target/release/bundle/nsis/release-assets/`

| Artifact | Bytes | SHA-256 | Authenticode |
|---|---:|---|---|
| `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` | 216,288,811 | `9103b2c657347ee39bb55f28eb0d8c78acd4400043459efde7d681ecfff1ee01` | `NotSigned` |
| `Code-Hangar_0.1.2_x64-setup.exe` | 212,282,648 | `b4433c85eb30afe25afb77ede6c2ab3bf08a7608154d2f06d82e4c3c1e919acb` | `NotSigned` |
| `SHA256SUMS` | 211 | contains only the two 0.1.2 artifacts above | n/a |

The two stale 0.1.1 release copies found during inspection were moved to the
recoverable `target/release/bundle/nsis/archive-0.1.1/` folder. No source file or
current artifact was deleted. After Local packaging, no staged
`code-hangar-mcp-*.exe` remained under the Tauri binaries directory.

Both installers are unsigned preview builds. Judges may see a Windows
SmartScreen unknown-publisher warning. The public release is
<https://github.com/jcomlabs/code-hangar/releases/tag/v0.1.2-alpha>. Its three
assets were downloaded into a fresh directory outside the repository and both
executables were independently reverified against the published manifest.

## GPT-5.6 proof boundary

Subscription-backed live proof completed on **18 July 2026**:

- Codex CLI `0.144.1` reported `Logged in using ChatGPT`;
- the explicit `gpt-5.6-sol` model returned a real answer;
- the MCP executable was the final sidecar compiled from the tagged public
  candidate and staged by Connector packaging, with SHA-256
  `e36e3cbe522ef8bb96515f5582e69aa294ddcb2c8f7de1827954714e0bf95b07`;
- the model called Code Hangar's `list_catalog` and `get_project_context` tools
  over the shipped MCP stdio surface;
- the answer identified the exact synthetic `Fixture Git-like Project`;
- Code Hangar's audit verified both allowed reads for the temporary Codex client;
- disconnect revoked all temporary fixture credentials before fixture cleanup;
- Codex ran with `--ephemeral` from a disposable system temporary directory,
  and the proof verified that zero new rollout/session files were created;
- no API key, OAuth token, personal project path, or personal project body was
  retained; and
- the sanitized gitignored report is
  `.local/acceptance/gpt56-mcp-release-20260718-135432/codex-gpt56-mcp-proof.json`.

The tracked reproduction procedure is
`scripts/submission/codex-gpt56-mcp-smoke.ps1`; its optional `-ServerPath`
parameter binds the proof to an explicit sidecar executable and records its
hash; that executable may be an installed sidecar or the final packaging input. The
final public video must still show the native Connector, Codex, and the matching
audit activity; this synthetic-catalog proof is not presented as a clean-machine
or public-video result.

Repository-verifiable proof completed:

- Connector preset: official OpenAI endpoint plus model `gpt-5.6`;
- direct official GPT-5.6 Chat Completions requests serialize
  `max_completion_tokens`, never deprecated `max_tokens`;
- compatible local and third-party Chat-Completions endpoints retain
  `max_tokens`;
- the exact serialized request disclosure is tested;
- AI Assist remains Off by default and the secret/Protected Zone gates run
  before transport.

Optional direct-API proof **not completed**: no owner-authorized Platform API key
or spending scope was provided, so no separately billed live call was made from
the in-app OpenAI preset. That is no longer a blocker for the primary
subscription-backed MCP route. The preset screenshot remains configuration and
contract UI proof only and is labelled accordingly.

Codex app-server is documented only as a future inbound extension. Code Hangar
does not implement or claim acceptance proof for that adapter in this candidate.

## Visual and runtime fixture proof

- [What changed](assets/01-what-changed.jpg)
- [Hangar Map](assets/02-hangar-map.jpg)
- [GPT-5.6 preset](assets/03-gpt56-preset.jpg)
- [Capture provenance](assets/README.md)

The Connector fixture ran on loopback with synthetic data, produced zero browser
console warnings/errors in the captured journey, and was stopped afterwards;
port 4177 was confirmed closed.

## Packaging network note

Cargo dependency resolution was forced offline for final validation and
packaging. During the Connector Tauri bundle, Tauri fetched Microsoft's WebView2
offline redistributable from `go.microsoft.com`, as required by the configured
`offlineInstaller` bundle mode. This was a build-tool prerequisite fetch, not
runtime application traffic, telemetry, project publication, or submission.

## Owner-gated and unverified items

- final public installed-Connector + Codex + disposable-project GPT-5.6 video
  journey: not yet captured; the exact compiled-sidecar subscription/MCP proof
  above has passed, but it is not an installed-product claim;
- optional paid direct-API round trip from in-app AI Assist: not run and not
  required for the primary demo;
- Codex `/feedback`: not sent;
- exact public 0.1.2 host lifecycle: not run, to avoid interfering with the
  existing installation and parallel sessions. Earlier 0.1.2 candidates passed
  isolated-profile host lifecycle checks, but their hashes differ and they are
  predecessor evidence only;
- clean disposable-Windows install/uninstall: still not verified. On 18 July,
  the exact downloaded Local setup was attempted in an empty, network-disabled
  Windows Sandbox. Guest Application Control blocked it before setup or product
  code ran; the clean start and final inspection both passed with zero Code
  Hangar apps/processes. Because the runner is fail-fast, the final Connector was
  staged but not executed. Evidence is under
  `.local/acceptance/build-week-sandbox/20260718-135126/`. Historical candidates
  are not evidence for these bytes;
- repository and installer release: published and reverified at
  <https://github.com/jcomlabs/code-hangar/releases/tag/v0.1.2-alpha>;
- public narrated YouTube demo: not recorded or uploaded;
- Devpost account: created according to the owner; project draft/final submission
  not performed.

External items require explicit owner authorization; clean-machine proof needs a
supported disposable environment that can execute the candidate. No fixture,
contract test, package build, host lifecycle, or historical result is
substituted for a stronger proof class.
