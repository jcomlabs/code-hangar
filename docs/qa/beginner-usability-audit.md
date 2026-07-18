# Beginner usability audit

Date: 2026-07-14

This audit treats a person who uses AI coding tools but does not yet understand
programming vocabulary as a primary Code Hangar user. It covers the Local and AI
Connector editions. The safety and data contracts are unchanged.

## Display contract

1. Primary copy says what the information means and what the user can do next.
2. The first use of an unfamiliar concept has a discreet `?` with two short,
   local explanations: what it is, then why it matters here.
3. Technical provenance remains available only behind an explicit disclosure.
4. Long requests, explanations and evidence can be expanded rather than clipped.
5. Opening a view, help item, session or AI menu never edits a project or runs Git.

## Surface matrix

| Surface | Beginner question | In-place answer |
|---|---|---|
| What changed | What am I looking at? | Explains that this is an incomplete, read-only collection of local clues. Separates overview, current project files, AI conversations by app, and older saved reviews. |
| Git changes | What is Git, a commit or a branch? | Defines all three, recommends Git for recovery, and states that Code Hangar does not commit, push or change branches. |
| Added/removed lines | Do green and red mean good and bad? | Explains that they mean text added and removed, not correctness. |
| Evidence and confidence | Can I trust this as a complete history? | Uses Strong, Possible, Weak and Unclear in primary copy; parser/source details remain expandable. |
| AI conversations | Why is this session here? | Explains project-linked and Independent sessions and gradual loading. Sessions are grouped by AI app. |
| Selected text | Where are the AI actions? | The right-click menu has a visible title and sections for Explain, Check for risks, Suggest a change and normal Copy. |
| AI provider | Will this send my code? | Explains Off, local model and user-configured API choices. Nothing is sent before the exact request is approved. |
| Context and files | What are context, rendered and source? | Explains priority project documents, the file tree and read-only display modes. |
| Project map | What are space, links and references? | Uses disk-space and local-connection language; warns that a link is not deletion permission. |
| Safe Manage | Does opening this remove anything? | Explains the review-only first steps, protected/private items, dependencies and the separate guarded action path. |
| Recover and versions | What are held files, backups and previous versions? | Explains each recovery copy and the compare-before-restore contract. |
| Discover | What are forgotten, unreferenced and duplicate items? | Labels them as investigation candidates. Duplicate comparison is described without hash jargon. |
| Scan and inventory | Is Code Hangar changing my folders? | Explains read-only scanning, Needs scan versus Empty, and the separate private inventory. |
| Project checks | Is this normal browsing? | Warns that a check runs project code and requires approval of the exact command. |
| Local automation | What are endpoint, token and permissions? | Defines each term in ordinary language and recommends the smallest permissions. |

## Safety contract

- Git access in this workflow is read-only. The runtime allowlist rejects commit,
  push, branch changes, checkout, reset, restore and other unapproved commands.
- Project editing remains locked at startup and after changing projects. Unlocking,
  line review and a fresh confirmation are separate steps.
- AI explanations are explicit requests. The Local edition contains no provider
  path; the Connector shows the exact outgoing request before it can be sent.
- Cleanup and restore keep their existing backup, path, fingerprint and confirmation
  gates. Beginner copy does not weaken any backend input or decision.

## Regression contract

`beginner-usability.test.ts` checks the shared help catalogue, the Git safety
explanation, What changed grouping, and the selected-text AI/Copy menu. The focused
Recap, context-menu and Safe Manage suites cover the associated rendering contracts.
The complete Local CI remains the release gate for edition isolation, command safety,
Rust behavior, Connector scope and frontend regressions.

## Final evidence

- `scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild`: PASS in 154.7 s;
  46/46 frontend files and 410/410 tests, all executed Rust matrices, security,
  Local isolation, Connector/MCP surface and Clippy passed.
- Browser Connector and Local journeys: PASS at 1024x576 and 480x800 CSS in
  light and OLED. Document width stayed bounded, the open help panel stayed
  inside the viewport, and the OLED sweep found zero visible white surfaces.
- The live journey found and fixed two additional defects: the lazy Connector
  explanation layer could suspend the first What changed click, and left-column
  help panels could open beyond the viewport. Both now have regression coverage.
- Selected-text journey: PASS. The menu exposed Explain, Check for risks,
  guarded Suggest a change and Copy; Explain opened the Plain (vibe coder)
  panel without sending a request.
- Native Connector journey: PASS against the rebuilt release executable and
  the real CodeHangar project. What changed loaded 29 local conversations, Git
  help was fully readable, README remained Changes locked, and right-clicking
  selected text exposed the same AI/Copy menu.
