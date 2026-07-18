# Code Hangar 0.1.2 judge candidate

This local candidate packages the eligible OpenAI Build Week work while keeping
event material isolated from reusable development.

## Candidate highlights

- reconstructs AI-assisted changes from local session, Git, current-file, and
  saved-review evidence, with coverage and unknowns shown first;
- improves large-project navigation through bounded progressive Hangar Map
  loading and separates dependency-cache observations from direct project
  signals;
- keeps graph/API/MCP list requests within a hard 1,000-item ceiling;
- adds the explicit OpenAI GPT-5.6 Connector preset and direct-OpenAI
  `max_completion_tokens` contract while preserving compatible providers;
- exposes the existing scoped/audited Code Hangar MCP sidecar as the primary
  GPT-5.6 judge journey through ChatGPT-authenticated Codex;
- retains the hard Local-versus-Connector build boundary, secret checks,
  Protected Zones, exact-request review, and reversible correction workflow.

## Editions

- `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` — primary judge build with
  opt-in AI Assist and local MCP sidecar.
- `Code-Hangar_0.1.2_x64-setup.exe` — Local isolation-proof build with no
  AI-provider or MCP frontend surface.

Both packages are Windows x64 preview builds. Final hashes, signing inspection,
and any clean-install limitation are recorded in the evidence manifest rather
than copied from an older release.

## Proof and owner-gated external actions

A synthetic subscription-backed GPT-5.6 Sol + Code Hangar MCP proof passed
against the final compiled sidecar, including two scoped audited reads,
temporary credential revocation, and zero persisted Codex session files. The
public assets were re-downloaded and their SHA-256 values matched the published
manifest. Exact host lifecycle was not repeated for these release hashes. An
empty network-disabled Windows Sandbox refused the final downloaded Local setup
under guest Application Control before product execution; its fail-fast result
left Connector unexecuted. Clean disposable-machine proof and the final public
installed-product video remain outstanding. A separately billed in-app OpenAI
API call is optional and has not been run.

The privacy-sanitized source repository is public. This candidate does not claim
`/feedback`, YouTube publication, or Devpost submission until the owner
explicitly performs or authorizes those account-bound actions.
