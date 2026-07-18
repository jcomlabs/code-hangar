# Demo script — target 2:45

The final video must be **public on YouTube and under three minutes**. This
candidate deliberately uses audible English narration so no translation track
is required. It must show the working product, Codex's role in building it, and
a real GPT-5.6 interaction. Do not substitute a browser fixture for the
live-model segment.

## Recording setup

- Use the final **Connector** installer and a disposable, non-sensitive sample
  project prepared specifically for the demo.
- Set Code Hangar and Codex to legible 16:9 windows and hide notifications,
  credentials, account names, personal paths, and unrelated apps.
- Pre-populate a short recorded Codex session and a small source change whose
  explanation fits on one screen.
- Sign Codex in with ChatGPT, select GPT-5.6 Sol, connect only the disposable
  project through Code Hangar, and rehearse the MCP read before the final take.
- Do not show the Codex profile menu, token files, Code Hangar MCP token, or raw
  debug/app-server trace. The visible proof is the model, the scoped tool call,
  the synthetic result, and Code Hangar's corresponding audit activity.
- Keep a Local-build isolation screenshot ready for the closing proof.
- Record voice and screen together. Do not rely on captions as the only audio.

## Timed script

### 0:00–0:15 — Hook

**On screen:** Code Hangar Overview and Review Inbox.

**Narration:**

> Vibe coding is fast, but understanding what happened afterwards is not. Code
> Hangar is a local-first flight recorder for AI-assisted projects: evidence
> first, explanation second, and only small reversible corrections.

### 0:15–0:40 — The fragmented evidence problem

**On screen:** Open the sample project and its What changed / Recap surface.

**Action:** Briefly point to the recorded Codex session, current Git-visible
changes, current-file comparison, coverage, and unknowns.

**Narration:**

> Instead of trusting one transcript, Code Hangar reconstructs the review from
> local session records, Git evidence, current files, and saved review history.
> It shows what it knows and what it cannot prove before asking a model anything.

### 0:40–1:05 — Codex collaboration

**On screen:** Show a concise build-period diff or proof page, then return to the
product.

**Narration:**

> I used Codex as the engineering collaborator during Build Week to audit the
> existing project, implement bounded improvements, add regression tests, and
> validate the Local-versus-Connector boundary. The submission includes the
> eligible diff from the July 12 baseline and the Codex session selected for
> the required feedback step.

**Overlay or description field:**

- Baseline: `843530c`
- Product candidate: `e831c14dfa15291dda152d7742766221438feaa3`
- Codex session selected for `/feedback`: `019f3315-12ff-7071-8534-04fe50ed534e`
- Candidate-finalization session: `019f7226-c01a-71d3-9850-4c6f3b990ef2`

### 1:05–1:28 — Safety and the two directions

**On screen:** Select a deliberately secret-like fixture or Protected Zone item,
then briefly show **Settings → Advanced → AI app integration**.

**Action:** Attempt to prepare AI context and show the hard block. Then select a
safe source passage.

**Narration:**

> Code Hangar supports two explicit directions. Its local MCP server lets Codex
> read only granted, curated evidence. Its optional in-app AI Assist can receive
> an explanation from a local server or configured provider, but starts Off and
> hard-blocks secrets before any send.

### 1:28–1:58 — Real GPT-5.6 through Code Hangar MCP

**On screen:** In Codex, show GPT-5.6 Sol and the connected `code-hangar` MCP
server. Ask Codex to use Code Hangar to identify and explain the disposable
project. Show the returned project evidence, then switch to Code Hangar's
connected-app activity and show the allowed scoped reads.

**Narration:**

> Here GPT-5.6 runs in Codex using my ChatGPT subscription and reads the curated
> project through Code Hangar's local MCP connection. Code Hangar never receives
> my ChatGPT credential. Every read is project-scoped and audited, and the model
> answer remains an explanation rather than invented history.

**Recording gate:** this segment is valid only if the final installed Connector,
a disposable project grant, a real GPT-5.6 response, and the matching Code
Hangar audit entries are all visible. The repository's synthetic subscription
smoke is supporting proof, not a substitute for the recorded product journey.

### 1:58–2:28 — One small reversible correction

**On screen:** Open a small suggestion or recognised value change, show the
before/after diff and validation, apply it only to disposable sample data, then
show Previous versions / Undo.

**Narration:**

> If I accept a correction, Code Hangar keeps it intentionally small. I review
> the exact diff, local validation runs, the previous bytes are snapshotted, and
> I can compare and restore them. It never stages, commits, pushes, or performs a
> whole-project AI rewrite.

### 2:28–2:45 — Isolation, impact, close

**On screen:** Brief split view or title cards for Connector and Local; finish on
the product hero.

**Narration:**

> The Connector is the Build Week experience. The Local edition proves the
> privacy boundary by compiling without AI-provider or MCP surfaces. Code Hangar
> turns AI-assisted development from an opaque leap of faith into a reviewable,
> teachable, reversible workflow.

## YouTube description draft

```text
Code Hangar — the local-first flight recorder for vibe coding.

Developer Tools submission for OpenAI Build Week 2026. The demo shows local
evidence reconstruction, secret blocking, GPT-5.6 in Codex consuming scoped
Code Hangar context over MCP, and a small reversible correction.

Repository / judge access: pending owner authorization
Build Week delta: 843530c..e831c14dfa15291dda152d7742766221438feaa3
Codex session selected for /feedback: 019f3315-12ff-7071-8534-04fe50ed534e
```

Final public video URL: pending recording, review, and owner-authorized upload.

## Final video checks

- Runtime is below 3:00; target is 2:45.
- Narration is audible and in English.
- The installed Connector, Codex collaboration, real GPT-5.6 result, and matching
  Code Hangar MCP audit entries are visible.
- The narration says that ChatGPT authentication belongs to Codex; it does not
  call the subscription credential a general OpenAI API key.
- No key, personal path, prompt history, username, notification, or private
  repository URL is exposed.
- Fixture or staged sample data is labelled honestly.
- Claims match the final test manifest and candidate commit.
- The uploaded YouTube video is public and plays in a signed-out browser.
