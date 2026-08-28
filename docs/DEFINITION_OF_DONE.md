# Code Hangar Definition of Done

Status: owner-approved product acceptance contract, 2026-08-27.

Code Hangar is complete when a new user can discover work left by supported AI
coding tools, understand what happened, navigate and understand the projects,
make the small corrections they explicitly choose, and recover or remove the
material they explicitly choose to manage, without knowing the implementation or
putting their data at risk.

This document is a release contract, not a feature inventory. A checked item
requires evidence from the exact distributable build. Implementation presence,
unit tests, or a source-tree screenshot alone are not sufficient.

## 1. Discovery and catalog

- [ ] Reliably detects projects, sessions and artifacts from supported tools,
  including Windows and opt-in WSL sources.
- [ ] Associates each session, conversation and artifact with the correct
  project without collapsing distinct sessions or inventing relationships.
- [ ] Labels orphaned, unassociated, ambiguous and low-confidence information
  instead of presenting it as fact.
- [ ] Handles large stores, corrupt files, denied access, long paths, Unicode,
  junctions and other real filesystem conditions without blocking the app.
- [ ] Lets the user add, exclude, rescan and manage observed sources.

## 2. Navigation and comprehension

- [ ] A user can enter a project and quickly find files, context, sessions,
  models, dependencies and other relevant information.
- [ ] README, AGENTS, CLAUDE, documentation, manifests and other important
  context files are highlighted automatically.
- [ ] Preview, search, Quick Open, navigation history and Inspector operate on
  real data and preserve context across each other.
- [ ] Complete sessions and transcripts can be read, with progressive loading
  for large records.
- [ ] Sensitive files, Protected Zones and dangerous content are never exposed
  accidentally through preview, search or indexing.

## 3. Reconstruction of what happened

- [ ] A user can determine what an AI tool did or attempted in a project without
  manually reconstructing dozens of sessions.
- [ ] Review Inbox and Recap distinguish recorded conversations, observed
  changes, current Git state and current file state.
- [ ] Changes that can no longer be proved are shown as unknown or incomplete,
  never reconstructed by guesswork.
- [ ] A user can navigate from a change or session to the file and context that
  explain it.
- [ ] Checkpoints and reviews preserve continuity across later app sessions.

## 4. Understanding and learning

- [ ] Code Hangar explains structure, files and code in language suitable for a
  user who may not know the stack.
- [ ] Explanations remain anchored to the real code and evidence that support
  them.
- [ ] Explain, What to check, walkthroughs and similar tools help the user
  understand rather than hide complexity.
- [ ] Insufficient information is stated explicitly.
- [ ] AI explanations are optional and are never required for fundamental Code
  Hangar workflows.

## 5. Safe small changes

- [ ] File editing starts locked and requires an explicit user action to unlock.
- [ ] Changes stay within the promised scope: small controlled corrections, not
  autonomous whole-project transformations.
- [ ] Before writing, the app shows the exact change and verifies that the file
  is still in the expected state.
- [ ] Every change creates enough evidence to review and revert it.
- [ ] A user can restore a previous version without manually reconstructing the
  file.
- [ ] Code Hangar never commits, pushes, checks out, switches branches, or
  performs another active or remote Git operation for the user.

## 6. Organization, cleanup and recovery

- [ ] Used space, ownership, references, duplicates, shared assets and possible
  orphans are shown with credible numbers and confidence levels.
- [ ] Before a destructive operation, the user sees exactly what will happen and
  its expected impact.
- [ ] Preview and execution refer to the same immutable plan, with no silent
  changes between them.
- [ ] Protected Zones, shared data, links and external paths are handled
  conservatively.
- [ ] Nothing is removed without a previously verified backup.
- [ ] Normal removal is reversible through holding, and restore survives
  interruption and restart.
- [ ] Permanent removal starts disabled and requires explicit activation, a
  valid backup and a fresh confirmation for every operation.
- [ ] A partial or interrupted failure is never reported as success.

The assisted decision workflow that precedes these operations is specified in
[`safe-manage-assisted-cleanup.md`](safe-manage-assisted-cleanup.md).

## 7. AI Connector and MCP

- [ ] The Local edition physically contains no AI/MCP functionality and no
  external-network capability. Its installed UI and documentation contain no
  reference or teaser for Connector-only functionality.
- [ ] In the Connector edition, AI Assist starts disabled and uses only the local
  server or provider explicitly selected by the user.
- [ ] Before sending content to an external model, the user can inspect the exact
  payload that will leave the machine.
- [ ] Secrets, sensitive files and Protected Zones are blocked by the backend
  before sending, independently of frontend behavior.
- [ ] API keys remain in secure operating-system storage and never appear in the
  database, logs, frontend, fixtures or IPC output.
- [ ] Every connected MCP app receives only explicitly granted projects and
  permissions.
- [ ] A connected app may request a privileged operation but can never execute it
  without the user's in-app approval.
- [ ] Connector credentials are bound to their app, host and transport and cannot
  cross into a more powerful local-agent channel.
- [ ] Disconnect and revoke invalidate access without damaging unrelated external
  application configuration.
- [ ] Connector is documented honestly as optional, experimental and less tested
  than Local, while still passing its declared release gates.

## 8. User experience and continuity

- [ ] A new user understands the purpose and can begin without technical setup
  documentation.
- [ ] Safe defaults allow a quick start without unnecessary mandatory
  configuration.
- [ ] Long operations show progress and can be cancelled where technically
  possible.
- [ ] Loading, empty, partial, stale, error and success are distinct and
  understandable states.
- [ ] Errors explain what happened and offer recovery or a next action when
  possible.
- [ ] Failure of a secondary feature leaves the rest of the app usable whenever
  possible.
- [ ] Project selection, preferences, panes, reviews and other useful state
  survive restart where appropriate.
- [ ] The interface remains usable in light and OLED themes, at different Windows
  scales, in narrow windows and with keyboard navigation.

## 9. Installation and lifecycle

- [ ] A new Windows account can install each edition without development tools.
- [ ] First run, upgrade, repair, edition switching and uninstall preserve exactly
  the data they promise to preserve.
- [ ] The editions correctly share the local catalog without contaminating their
  security boundaries.
- [ ] Upgrade does not destroy the catalog, secure keys, preferences or
  recoverable state.
- [ ] Reset clearly explains what it removes and which integrations must be set
  up again.

## 10. Quality evidence

- [ ] Critical workflows have end-to-end tests, not only internal unit tests.
- [ ] Fixed critical bugs have regression tests.
- [ ] Large, incomplete, corrupt and adversarial data is tested in addition to
  ideal fixtures.
- [ ] Discovery, review, editing, backup, holding, restore, permanent removal,
  Local isolation, AI Assist and MCP each pass their release gates.
- [ ] The exact intended distributable build passes the complete quality gate
  from a clean state.

## 11. Release

- [ ] Final installers correspond exactly to the accepted commit.
- [ ] Those exact final bytes pass clean install and lifecycle testing on a clean
  machine or Windows user.
- [ ] Local and Connector are re-verified as distinct products.
- [ ] The owner explicitly chooses signed binaries or an honestly warned unsigned
  release.
- [ ] Checksums are generated only after any signing step.
- [ ] Published artifacts are downloaded again and match the validated bytes.
- [ ] README, release notes, limitations and edition-specific installed
  documentation describe what is actually in those binaries.

## Final acceptance journey

Code Hangar is done when, without help from its developer, a person can:

1. find their projects and sessions;
2. understand what happened;
3. find the evidence supporting that conclusion;
4. understand the relevant code;
5. make and revert a small correction;
6. identify waste or risk;
7. back up, hold, restore or permanently remove material safely;
8. connect the AI layer or ignore it completely;
9. understand and recover from errors; and
10. close the app, return later and continue.

Code Hangar is not complete merely because every feature exists. It is complete
when these workflows can be completed end to end, with confidence, no hidden
steps and no dependence on the author.
