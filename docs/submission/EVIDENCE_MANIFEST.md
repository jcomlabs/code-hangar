# Candidate evidence manifest

Generated locally on **18 July 2026**. This is a sanitized summary of the final
candidate run; it contains no API key, personal project path, prompt, source
code excerpt, or private repository URL.

## Candidate identity

| Item | Value |
|---|---|
| Branch | `submission/openai-build-week` (local only; no upstream) |
| Declared pre-event baseline | `843530c` (12 July 2026) |
| Product candidate | `e831c14dfa15291dda152d7742766221438feaa3` |
| Review range | `843530c..e831c14dfa15291dda152d7742766221438feaa3` |
| Eligible-range size | 10 commits; 130 files changed; +30,676 / -4,119 |
| General hardening layer | `362f5c5` |
| Reusable GPT-5.6 layer | `519086f` |
| Submission package layer | `e831c14` |
| Evidence layer | contiguous documentation/proof-harness commits above `e831c14` |
| Version | `0.1.2` across Cargo workspace, Tauri, npm root/workspace, and lockfiles |
| Validation host | Windows 11 Pro x64, build 26200 |
| Candidate worktree | clean before packaging and evidence generation |

The application binaries were built from the product candidate. The later
evidence commit changes documentation only, avoiding a self-referential binary
hash or candidate hash.

## Validation results

| Command / gate | Result |
|---|---|
| `npm run check` | PASS — TypeScript, ESLint, 47 Vitest files / 428 tests, dependency guard, forbidden-code guard |
| Detached clean checkout: offline `npm install` then `npm run check` | PASS — 185 packages installed from local cache, 47 Vitest files / 428 tests passed, security gates passed, and the disposable worktree stayed clean |
| `npm --workspace apps/desktop run build:connector` | PASS — 22 Connector text assets; main chunk 480.93 kB |
| `npm --workspace apps/desktop run build:local` | PASS — 11 Local text assets; no Connector chunks, endpoints, or IPC commands; main chunk 486.20 kB |
| `scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild` | PASS in 207.6 s — npm gate, Local isolation, Rust formatting, sandbox-validator self-test, core/mutation/agent-automation tests, MCP/release-sidecar build, and warnings-denied Clippy |
| `npm --workspace apps/desktop run package:connector` | PASS in 184.6 s — release MCP sidecar staged and Connector NSIS created |
| `npm --workspace apps/desktop run package:local` | PASS in 131.0 s — staged Connector sidecar removed, Local isolation rerun, Local NSIS created |
| `scripts/checksums.ps1` | PASS — current-version release copies matched their source hashes |
| Exact 0.1.2 installer lifecycle on the build host | PASS with disclosed limitations — both editions installed, launched natively with isolated application profiles, showed the expected edition UI, and uninstalled; the six pre-existing catalog files remained byte-, timestamp-, and SHA-256-identical |
| `scripts/submission/sandbox-candidate-lifecycle.ps1` | BLOCKED PRE-PRODUCT — the exact Local and Connector candidates were attempted separately from an empty, network-disabled Windows Sandbox; guest Application Control blocked each unsigned setup before NSIS or product code ran, and both guests finished with zero installed apps/processes |
| `scripts/submission/codex-gpt56-mcp-smoke.ps1` against the installed sidecar | PASS — ChatGPT-authenticated Codex 0.144.1 ran GPT-5.6 Sol through the sidecar installed by the final Connector, completed two audited Code Hangar MCP reads, revoked the temporary credential, and retained only a sanitized result |

Representative Rust results from the final gate:

- `hangar-ai`: 26 passed, 1 ignored live-local Ollama check;
- `hangar-api`: 63 passed, 5 explicitly ignored environment/data-stress checks;
- `hangar-db`: 90 passed, 1 ignored file-backed performance fixture;
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
| `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` | 216,287,756 | `ffa66b3033ac4cd51e017bb2592f9e37dcbc8f688faff9f82f10a065d926d241` | `NotSigned` |
| `Code-Hangar_0.1.2_x64-setup.exe` | 212,282,701 | `52288762d0de48403cd545852374178bf6cb72815f0c1c7c08d14fb0ee521a47` | `NotSigned` |
| `SHA256SUMS` | 211 | contains only the two 0.1.2 artifacts above | n/a |

The two stale 0.1.1 release copies found during inspection were moved to the
recoverable `target/release/bundle/nsis/archive-0.1.1/` folder. No source file or
current artifact was deleted. After Local packaging, no staged
`code-hangar-mcp-*.exe` remained under the Tauri binaries directory.

Both installers are unsigned preview builds. Judges may see a Windows
SmartScreen unknown-publisher warning. Their exact host lifecycle is recorded
separately below; it is not relabelled as a clean disposable-machine result.

## GPT-5.6 proof boundary

Subscription-backed live proof completed on **18 July 2026**:

- Codex CLI `0.144.1` reported `Logged in using ChatGPT`;
- the explicit `gpt-5.6-sol` model returned a real answer;
- the MCP executable was the sidecar installed by
  `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe`, with SHA-256
  `6e8c2bd602977a456d24c094972a816a9f48bb4f19110e1eb7535e0401b4e97d`;
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
  `.local/acceptance/gpt56-mcp-installed-20260718-01/codex-gpt56-mcp-proof.json`.

The tracked reproduction procedure is
`scripts/submission/codex-gpt56-mcp-smoke.ps1`; its optional `-ServerPath`
parameter binds the proof to an explicitly installed candidate sidecar. The
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
  journey: not yet captured; the installed-sidecar subscription/MCP proof above
  has passed;
- optional paid direct-API round trip from in-app AI Assist: not run and not
  required for the primary demo;
- Codex `/feedback`: not sent;
- exact 0.1.2 host lifecycle: passed for both editions using isolated disposable
  application profiles. The Connector exposed its Connector-only controls and
  installed MCP sidecar; the Local edition exposed neither. Both uninstalled,
  no process or uninstall entry remained, and the six pre-existing real catalog
  files retained identical bytes, timestamps, and SHA-256 hashes. The sanitized
  summary is
  `.local/acceptance/host-lifecycle-20260718-01/HOST_LIFECYCLE_SUMMARY.json`;
- clean disposable-Windows install/uninstall: still not verified. On 18 July,
  the exact Local and Connector candidates were each attempted in a separate
  empty, network-disabled Windows Sandbox. Their staged hashes matched this
  manifest, but guest Application Control blocked both unsigned installers
  before setup or product code ran. Each fail-closed run began and ended with
  zero Code Hangar apps/processes. The sanitized combined result is
  `.local/acceptance/build-week-sandbox/SANDBOX_CANDIDATE_SUMMARY.json`; the raw
  Local and Connector manifests are in its adjacent candidate-04 and
  candidate-03 directories. Historical 0.1.1 success on 12 July is not evidence
  for these bytes, and host lifecycle proof is not silently promoted to
  clean-machine proof;
- uninstall residue: the isolated Connector launch left generated WebView2
  profile data after NSIS uninstall. No application binary, running process, or
  uninstall registry entry remained;
- repository publication or private judge sharing: not performed;
- installer upload/release: not performed;
- public narrated YouTube demo: not recorded or uploaded;
- Devpost account: created according to the owner; project draft/final submission
  not performed.

External items require explicit owner authorization; clean-machine proof needs a
supported disposable environment that can execute the candidate. No fixture,
contract test, package build, host lifecycle, or historical result is
substituted for a stronger proof class.
