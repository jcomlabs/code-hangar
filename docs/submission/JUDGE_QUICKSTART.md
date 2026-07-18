# Judge quickstart

This is the shortest path through the working Code Hangar candidate. The
**Connector** is the primary Build Week build; the **Local** edition is the
edition-isolation proof.

## Supported platform

- **Current target:** Windows x64
- **Validated build host:** Windows 11 Pro x64, build 26200
- **Native macOS/Linux build:** not currently provided
- **WSL:** Windows Code Hangar can catalogue WSL projects; this is not a native
  Linux application

Both Windows installers are unsigned preview builds (`Authenticode: NotSigned`).
Windows SmartScreen may show an unknown-publisher warning.

The published 0.1.2 bytes were downloaded into a fresh directory and
hash-reverified. The final compiled Connector sidecar completed the GPT-5.6
subscription proof. Exact host install/launch/uninstall was not repeated for
these release hashes because an existing installation and parallel sessions had
to remain untouched. In an empty network-disabled Windows Sandbox, Application
Control blocked the final downloaded Local installer before setup or product
code ran; the fail-fast run ended cleanly and therefore did not execute the
Connector installer. See the
[evidence manifest](EVIDENCE_MANIFEST.md); do not weaken host security policy to
work around the clean-machine limitation.

## Option A — primary reviewer path: Connector installer

1. Download `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` from the
   [public prerelease](https://github.com/jcomlabs/code-hangar/releases/tag/v0.1.2-alpha).
2. Verify its SHA-256:

   ```powershell
   Get-FileHash .\Code-Hangar-AI-Connector_0.1.2_x64-setup.exe -Algorithm SHA256
   ```

   Expected:
   `9103b2c657347ee39bb55f28eb0d8c78acd4400043459efde7d681ecfff1ee01`
3. Install and launch **Code Hangar — AI Connector**. It is a per-user Windows
   application and should not require administrator rights.
4. Start with the included sample project or register a disposable local project.
   Never use sensitive production data for a judge walkthrough.
5. Open **Overview → Review Inbox**, choose the disposable project, then open
   **What changed**.
6. Under **Settings → Advanced → AI app integration**, grant Codex only that
   project and connect it. Restart Codex if the new `code-hangar` MCP server is
   not yet visible.
7. Sign Codex in with ChatGPT, choose GPT-5.6 Sol, and ask it to use Code Hangar
   to list and explain the granted project. Confirm that the answer is grounded
   in the disposable project, then inspect Code Hangar's connected-app activity
   for the matching allowed reads.
8. Return to Code Hangar. Confirm AI Assist starts **Off**, show a secret-like
   selection being blocked, and inspect the exact bounded request for a safe
   selection. A direct provider send is optional and requires the reviewer's own
   key; it is not needed for the subscription-backed MCP proof.
9. Inspect the small-correction preview and reversible-history controls. Do not
   apply a correction to anything except disposable sample data.

### Live-model proof status

On 18 July 2026, Codex CLI 0.144.1 signed in with ChatGPT completed a real
`gpt-5.6-sol` response through the final compiled Connector sidecar
after calling Code Hangar's `list_catalog` and `get_project_context` MCP tools
against a synthetic catalog. Both reads were audited, the temporary credential
was revoked, and the sanitized local report contains no token or personal data.
Reproduce it from a repository release sidecar with:

```powershell
powershell -ExecutionPolicy Bypass -File `
  scripts/submission/codex-gpt56-mcp-smoke.ps1 `
  -EvidenceDir .local/acceptance/judge-gpt56-mcp-proof
```

To bind the same proof to an installed Connector in a disposable environment,
add:

```powershell
-ServerPath "$env:LOCALAPPDATA\Code Hangar AI Connector\code-hangar-mcp.exe"
```

This proves the subscription-backed model/MCP path. The final public video must
still show the installed Connector and a disposable project; a browser fixture
must not be substituted for that native product journey. The separately billed
in-app OpenAI API route remains contract-tested but has no paid live call.

## Option B — no-key sample-data tour

This route is useful for visual and workflow review without reading local user
projects or spending API credits. It is a development fixture, not live-model
evidence.

Prerequisites: Node.js 20+ and npm.

```powershell
npm install
npm --workspace apps/desktop run dev:connector
```

Open:

```text
http://127.0.0.1:5173/?acceptanceState=saturated
```

The browser fallback uses synthetic `fixture://` projects. To open the
Connector AI panel directly during a fixture review, use:

```text
http://127.0.0.1:5173/?acceptanceState=saturated&acceptanceAiPanel=file
```

Important limitations:

- browser fixtures exercise the interface and deterministic gates;
- they do not read a reviewer's disk;
- they do not prove a live OpenAI request;
- native scanning, encrypted storage, credentials, and mutation safeguards must
  be judged in the packaged Tauri application and repository tests.

## Option C — Local isolation proof

1. Download `Code-Hangar_0.1.2_x64-setup.exe` from the
   [public prerelease](https://github.com/jcomlabs/code-hangar/releases/tag/v0.1.2-alpha).
2. Verify SHA-256 against
   `b4433c85eb30afe25afb77ede6c2ab3bf08a7608154d2f06d82e4c3c1e919acb`.
3. Launch **Code Hangar (Local)** and follow the same Review Inbox and recap path.
4. Confirm that AI-provider and connected-app controls are absent.
5. For source-level proof, run the edition checker through the normal build:

   ```powershell
   npm --workspace apps/desktop run build:local
   ```

The Local build is not merely Connector with calls disabled. The build gate
rejects Connector markers, provider endpoints, and AI/MCP command surfaces from
the Local frontend.

## Build from source

Prerequisites:

- Windows x64 with Tauri v2 Windows prerequisites
- Node.js 20+
- stable Rust toolchain with `cargo` on `PATH`

```powershell
npm install
npm run check

# Full local release gate, including Connector/agent-automation lanes
powershell -ExecutionPolicy Bypass -File scripts/local-ci.ps1 -AgentAutomation

# Package sequentially: Connector first, Local last
npm --workspace apps/desktop run package:connector
npm --workspace apps/desktop run package:local

# Stage the two release assets and calculate final hashes
powershell -ExecutionPolicy Bypass -File scripts/checksums.ps1
```

Sequential packaging matters because the Connector has an edition-specific
sidecar and the Local package step verifies that no Connector sidecar remains in
its bundle.

## What to evaluate in ten minutes

| Time | Surface | Question answered |
|---:|---|---|
| 0–2 min | Review Inbox | Can I see which AI-assisted projects need review? |
| 2–4 min | What changed / Recap | Does the product separate recorded facts, current state, and unknowns? |
| 4–6 min | Codex + Code Hangar MCP | Can GPT-5.6 read only the project the user granted, with an audit trail? |
| 6–8 min | Secret/Protected Zone and AI-in preview | Can unsafe context reach the optional direct provider transport? |
| 8–10 min | Correction preview/history | Is the proposed change small, reviewed, validated, and reversible? |

## Repository and license

- Repository/judge-access URL: <https://github.com/jcomlabs/code-hangar>
- License: Apache License 2.0 (`LICENSE`)
- Public release candidate: `c5cabbec8f5127fdf126d3ddb5e4c72a638e0931`
- Private provenance range: `843530c..e831c14dfa15291dda152d7742766221438feaa3`
- Final local results: [evidence manifest](EVIDENCE_MANIFEST.md)

## Prepared screenshots

- [What changed](assets/01-what-changed.jpg)
- [Hangar Map](assets/02-hangar-map.jpg)
- [GPT-5.6 preset](assets/03-gpt56-preset.jpg)
- [Capture provenance and limitations](assets/README.md)

These are synthetic fixture captures. In particular, the preset image is not a
live-model result.
