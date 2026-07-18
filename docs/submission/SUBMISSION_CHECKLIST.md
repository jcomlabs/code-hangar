# OpenAI Build Week submission checklist

Deadline from the [official Build Week rules](https://openai.devpost.com/rules):
**21 July 2026, 5:00 PM PT**, which is **22 July 2026, 01:00 in
Lisbon**.

This checklist deliberately separates local candidate preparation from external
publication. Do not push a public branch, create a release, upload a video, run
`/feedback`, share a private repository, or submit the Devpost form until the
owner explicitly authorizes that external action. The concise division between
owner-only decisions and delegable external work is in
[OWNER_HANDOFF.md](OWNER_HANDOFF.md).

## 1. Working candidate

- [x] General hardening commit(s) contain no event-specific documentation or UI.
- [x] Reusable GPT-5.6 integration is in its own general commit.
- [x] Submission-only files are one removable top commit or documented contiguous
      top-commit stack.
- [x] No Build Week branding or deadline logic appears in the product UI.
- [x] `git status --short` was clean at product candidate `e831c14`.
- [x] Candidate version is internally consistent across Cargo, Tauri, npm, and
      installer filenames.
- [x] The final Connector installer installs and launches the working native
      application with its expected Connector surfaces and MCP sidecar.
- [x] The Local installer installs and launches without Connector UI or sidecar.

## 2. Automated validation

- [x] `npm run check` passes.
- [x] Focused GPT-5.6 request-contract tests pass.
- [x] A non-OpenAI compatible-endpoint regression proves compatibility is
      preserved.
- [x] Secret and Protected Zone tests pass.
- [x] `scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild` passes from the
      final candidate; both Tauri editions were packaged separately.
- [x] Local and Connector edition checks pass independently.
- [x] Rust formatting and Clippy-with-warnings-denied pass.
- [x] Final results are retained in [EVIDENCE_MANIFEST.md](EVIDENCE_MANIFEST.md).

## 3. Native and clean-install evidence

- [x] Package **Connector first, Local last**.
- [x] Run `scripts/checksums.ps1` only after both final packages exist.
- [x] Record `Code-Hangar-AI-Connector_0.1.2_x64-setup.exe` and its SHA-256.
- [x] Record `Code-Hangar_0.1.2_x64-setup.exe` and its SHA-256.
- [x] Inspect Authenticode: both installers are `NotSigned`.
- [x] Disclose unsigned/SmartScreen behavior.
- [x] Complete the exact 0.1.2 install/native-launch/uninstall lifecycle for both
      editions on the build host using isolated application profiles, with the
      pre-existing real catalog unchanged.
- [x] Attempt each exact 0.1.2 edition separately from an empty, network-disabled
      Windows Sandbox and retain fail-closed evidence of the pre-setup
      Application Control block.
- [ ] Complete a clean disposable supported-Windows journey. It remains
      unverified because guest Application Control blocked the unsigned setup.
- [x] Disclose the generated Connector WebView2 profile residue left after NSIS
      uninstall; no application binary, process, or uninstall entry remained.
- [x] Record exact tested Windows version/build:
      Windows 11 Pro x64, build 26200.
- [x] Confirm the final local artifacts match the hashes in the evidence
      manifest. Reconfirm after any future upload.

## 4. Real GPT-5.6 proof

### Primary: ChatGPT subscription + Code Hangar MCP

- [x] Confirm Codex CLI is signed in with ChatGPT rather than a Platform API key.
- [x] Run the synthetic, ephemeral GPT-5.6 Sol + Code Hangar MCP proof.
- [x] Verify audited `list_catalog` and `project_context` reads.
- [x] Revoke the temporary MCP credential and remove the synthetic catalog.
- [x] Verify that the smoke creates zero persisted Codex session/rollout files.
- [x] Retain only the sanitized, gitignored proof result and record it in the
      evidence manifest.
- [x] Keep the tracked reproduction script free of tokens and personal data.
- [x] Repeat the model/MCP journey against the sidecar installed by the final
      Connector using a disposable catalog and narrow Codex project grant.
- [ ] Capture the real GPT-5.6 response and matching Code Hangar activity in the
      final video.
- [ ] Hide account details, credentials, local personal paths, and unrelated MCP
      servers during recording.

### Secondary: optional in-app AI Assist

- [x] Confirm AI Assist starts Off in the final installed Connector.
- [ ] Show a secret-like selection being blocked before transport.
- [ ] Show the exact safe request disclosure for a safe selection.
- [ ] If a direct OpenAI call is recorded, use an owner-authorized Platform API
      key/spending scope and label it as separately billed.
- [x] Describe direct-provider contract tests as contract tests, not as the
      subscription-backed live proof.
- [x] Describe the Codex app-server inbound bridge as future work, not as a
      shipped Code Hangar feature.

## 5. README and repository access

- [ ] Repository/judge access URL is available after owner authorization.
- [ ] Repository is public and licensed, **or** the private repository has been
      shared with `testing@devpost.com` and `build-week-event@openai.com` before
      the deadline.
- [x] Apache-2.0 `LICENSE` is visible.
- [x] README dependency installation and `npm run check` work from a detached
      clean checkout; native packaging is covered separately by the final
      candidate gates.
- [x] README explains the Windows-only supported platform accurately.
- [x] README identifies how to use synthetic sample data without reading personal
      files.
- [x] README explains how Codex was used during Build Week.
- [x] README links the judge quickstart and eligible build-period delta.
- [x] No private path, username, token, prompt, session transcript, or unpublished
      URL appears in tracked documentation or screenshots.

## 6. Under-three-minute public demo

- [ ] Final video follows [DEMO_SCRIPT.md](DEMO_SCRIPT.md).
- [ ] Runtime is below 3:00; target 2:45.
- [ ] Video has clear audible English narration.
- [ ] Working project is visible, not only slides or mockups.
- [ ] Codex collaboration and the eligible delta are explained.
- [ ] A real GPT-5.6 response through Code Hangar MCP is visible and tied to the
      disposable project grant.
- [ ] Matching scoped/audited Code Hangar reads are visible.
- [ ] Secret blocking and optional AI-in exact-request disclosure are visible.
- [ ] The small reviewed/reversible correction is visible.
- [ ] Connector-primary and Local-isolation roles are clear.
- [ ] Video contains no credential, personal path, or notification leak.
- [ ] Third-party trademarks and copyrighted music/assets are absent or used
      with documented authorization.
- [ ] Video is uploaded to YouTube and set **Public**.
- [ ] Public playback is tested while signed out.
- [ ] Record the owner-approved public YouTube URL.

## 7. Codex feedback requirement

- [ ] Run the required Codex `/feedback` flow only after owner authorization.
- [x] Record the selected Session ID exactly:
      `019f3315-12ff-7071-8534-04fe50ed534e` is the selected engineering
      session; no `/feedback` action or receipt is claimed before authorization.
- [ ] Put the Session ID in the Devpost entry and any repository location required
      by the official form.
- [ ] Do not publish private conversation content; submit only the required ID and
      approved description.

## 8. Devpost form

- [x] Devpost account exists (owner reported on 18 July 2026).
- [ ] Join/register for OpenAI Build Week on Devpost and accept the current
      official rules before the registration deadline.
- [x] English form copy is prepared in [DEVPOST_DRAFT.md](DEVPOST_DRAFT.md).
- [ ] Create/save the Code Hangar project draft in Devpost without submitting it.
- [ ] Category is **Developer Tools**.
- [ ] Project title and one-line pitch match the repository.
- [ ] Description is in English.
- [ ] Description makes the pre-existing-project baseline explicit.
- [ ] Public YouTube URL is present.
- [ ] Public repository URL or confirmed private judge access is present.
- [ ] Installation instructions, supported platform, and test-build link are
      present.
- [ ] The test build remains free and unrestricted through the judging end on
      **5 August 2026 at 5:00 PM PT**.
- [ ] Codex `/feedback` Session ID is present.
- [ ] GPT-5.6 use distinguishes subscription-backed MCP out, optional direct API
      in, and the unshipped app-server inbound extension.
- [ ] Claims address the four equally weighted criteria: technological
      implementation, design, potential impact, and quality of the idea.
- [ ] No final hash, test count, signing state, platform, or live-model claim was
      copied from historical evidence without rerunning it on the candidate.

## 9. Owner authorization and final external actions

- [ ] Owner has reviewed the final branch, commits, artifacts, video, form copy,
      and disclosure of known limitations.
- [ ] Owner explicitly authorizes public repository/branch or private judge share.
- [ ] Owner explicitly authorizes installer upload/release creation.
- [ ] Owner explicitly authorizes YouTube upload/publication.
- [ ] Owner explicitly authorizes `/feedback` and Devpost submission.
- [ ] Submission is completed before 21 July 2026, 5:00 PM PT.
- [ ] Final submitted URLs and timestamp are recorded locally after submission.

## Final values

| Item | Value |
|---|---|
| Product candidate commit | `e831c14dfa15291dda152d7742766221438feaa3` |
| Repository/judge access | Pending owner authorization |
| Public YouTube demo | Pending recording and owner-authorized upload |
| Codex session selected for `/feedback` | `019f3315-12ff-7071-8534-04fe50ed534e` (external action pending) |
| Connector installer | Local artifact ready; upload URL pending |
| Connector SHA-256 | `ffa66b3033ac4cd51e017bb2592f9e37dcbc8f688faff9f82f10a065d926d241` |
| Local installer | Local artifact ready; upload URL pending |
| Local SHA-256 | `52288762d0de48403cd545852374178bf6cb72815f0c1c7c08d14fb0ee521a47` |
| Native candidate lifecycle | Both exact 0.1.2 editions installed, launched with isolated profiles, showed the expected edition boundary, and uninstalled on the host; separate clean Sandbox attempts were blocked pre-setup by Application Control |
| Live GPT-5.6 evidence | ChatGPT subscription + GPT-5.6 Sol proof passed against the sidecar installed by the final Connector; final public native recording pending |
| Direct OpenAI API live call | Optional; not run; separate key/spend required only if demonstrated |
| Devpost | Account created; event registration, draft entry, and final submission pending; copy ready |
| Full candidate test manifest | [EVIDENCE_MANIFEST.md](EVIDENCE_MANIFEST.md) |
