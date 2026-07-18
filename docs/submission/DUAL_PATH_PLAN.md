# GPT-5.6 dual-path plan

This is the submission architecture for Code Hangar. It separates what ships
today, what the primary demo proves, and what is a supported extension path.
It does not turn a ChatGPT session token into a generic OpenAI API key.

## The two implemented directions

| Direction | Candidate capability | Authentication and cost | Submission role |
|---|---|---|---|
| **MCP out** | Code Hangar exposes scoped, curated project evidence to Codex through its local `code-hangar-mcp` stdio sidecar | Codex owns its ChatGPT sign-in. GPT-5.6 usage follows the user's ChatGPT plan and limits; Code Hangar never reads the OAuth credential | **Primary live demo** |
| **AI in** | Connector AI Assist sends one disclosed, bounded request to a configured local server or provider API and receives an explanation | Local servers need no cloud key. Direct OpenAI API use needs a Platform API key and is billed separately | Optional secondary demo; request and safety contracts are already tested |

The Local edition contains neither direction. It remains the compile-time
isolation proof with no AI-provider or MCP frontend surface.

## Subscription-backed AI in

OpenAI documents Codex app-server as the interface for deep integrations inside
another product. It provides authentication, threads, approvals, and streamed
agent events over a local JSONL/stdio protocol. Because Codex can be signed in
with ChatGPT, a future Connector-only adapter can receive GPT-5.6 output through
that local process without asking Code Hangar to read, copy, or store the
ChatGPT OAuth credential.

The official interface makes that bridge technically viable, but it is **not
implemented or locally accepted as a Code Hangar adapter in this candidate** and
must not be presented as a shipped feature. The current in-app AI Assist remains
local-server or BYO-API-key based.

Official references:

- [Codex authentication](https://learn.chatgpt.com/docs/auth)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)
- [Codex models](https://learn.chatgpt.com/docs/models)
- [GPT-5.6 model guidance](https://developers.openai.com/api/docs/guides/model-guidance?model=gpt-5.6)

## Proof completed on 18 July 2026

The reproducible subscription proof is:

```powershell
powershell -ExecutionPolicy Bypass -File `
  scripts/submission/codex-gpt56-mcp-smoke.ps1 `
  -EvidenceDir .local/acceptance/gpt56-mcp-proof
```

When validating the packaged candidate, add `-ServerPath` pointing to the
`code-hangar-mcp.exe` installed by the Connector. The completed candidate run
used this form and recorded sidecar SHA-256
`6e8c2bd602977a456d24c094972a816a9f48bb4f19110e1eb7535e0401b4e97d`.

It performs an ephemeral, synthetic journey:

1. verifies that the local Codex CLI is signed in with ChatGPT;
2. creates a disposable Code Hangar catalog and per-client MCP credential;
3. snapshots the current Codex rollout-file set;
4. starts Codex with `--ephemeral`, outside every user project, and with only
   the temporary `code-hangar` MCP server;
5. runs `gpt-5.6-sol` against `list_catalog` and `get_project_context`;
6. verifies the exact synthetic project in the model response;
7. verifies both reads in Code Hangar's audit log;
8. asserts that no new Codex rollout/session file exists;
9. revokes every temporary client credential and deletes the fixture; and
10. retains only a sanitized, gitignored JSON result.

The completed local result used Codex CLI 0.144.1, ChatGPT subscription auth,
GPT-5.6 Sol, the sidecar installed by the final Connector, and the synthetic
`Fixture Git-like Project`. No API key, OAuth token, persisted Codex session,
personal project path, or personal project body was retained.

### Model-name compatibility note

On this installed CLI, the explicit `gpt-5.6-sol` identifier succeeds with
ChatGPT authentication. The `gpt-5.6` alias resolves to Sol through app-server,
but `codex exec` 0.144.1 rejected that alias after a stale local model-cache
warning. The proof script therefore pins the explicit Sol identifier and records
the CLI version. This is not converted into a broader availability claim.

## Demo decision

The final video should use the installed Connector and a disposable project:

1. show evidence and unknowns in Code Hangar;
2. connect Codex with a narrow project grant;
3. select GPT-5.6 Sol in Codex under ChatGPT sign-in;
4. ask Codex to use Code Hangar MCP to explain the curated evidence;
5. show the corresponding allowed MCP reads in Code Hangar;
6. show the in-app secret block and exact-request disclosure as the optional
   AI-in safety boundary; and
7. finish with one small reversible correction and the Local edition boundary.

A paid direct-API call is no longer a submission blocker. If one is recorded,
label it as the optional AI-in route, never as the subscription-backed proof.

## Post-submission general-product work

If the app-server bridge is implemented later, it belongs in a separate general
product commit below any event packaging. Minimum gates:

- Connector-only feature and UI; Local/core dependency graphs remain unchanged;
- explicit user opt-in and detection of an already installed Codex CLI;
- stdio child process only, with bounded messages, cancellation, and timeouts;
- no reads from `auth.json`, OS credential storage, or OAuth tokens by Code Hangar;
- exact disclosed context plus the existing secret/Protected Zone gate;
- read-only/sandboxed Codex thread policy for explanations;
- model, CLI version, and authentication mode surfaced without exposing a token;
- no automatic correction or mutation following a response; and
- regression tests plus a reproducible synthetic acceptance proof.
