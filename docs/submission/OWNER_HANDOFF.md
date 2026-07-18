# Owner handoff: OpenAI Build Week

This handoff separates work that is complete locally, work Codex may perform
after explicit authorization, and decisions or attestations that must remain
with the owner. No item in this document is evidence that an external action
has already happened.

## Complete locally

- The product candidate is isolated from the removable Build Week commit stack.
- Local and Connector installers are packaged and hashed.
- Both exact 0.1.2 installers completed install, native isolated-profile launch,
  edition UI inspection, and uninstall on the build host. The real catalog was
  verified unchanged; clean disposable-machine proof remains explicitly open.
- The exact Local and Connector candidates were also attempted separately from
  empty, network-disabled Windows Sandbox guests. Both hashes matched the final
  artifacts; current guest Application Control blocked each unsigned installer
  before setup/product execution, and the guests remained empty afterwards.
- Automated product, security, edition-isolation, Rust, and GPT-5.6 contract
  checks pass.
- A synthetic GPT-5.6 Sol run, authenticated by the owner's ChatGPT subscription,
  read a disposable Code Hangar catalog through the MCP sidecar installed by the
  final Connector.
- The GPT-5.6 smoke is ephemeral, runs outside the project, and asserts that it
  creates zero Codex rollout files.
- The accidental persisted smoke session was permanently deleted; its exact
  rollout file is absent, and the real Code Hangar discovery smoke still passes.
- Devpost copy, judge quickstart, evidence manifest, demo script, and disclosure
  of limitations are ready in this directory.
- A self-contained local handoff bundle is ready under
  `.local/submission-ready/code-hangar-build-week-20260718-01/`; it excludes the
  retained real-catalog backup and disposable application profiles, and includes
  the sanitized Sandbox block summary.

## Owner-only decisions and attestations

These cannot be inferred or approved by Codex:

1. Join/register for OpenAI Build Week, confirm eligibility, accept the current
   Build Week and Devpost terms, and attest that the submission is truthful and
   yours.
2. Choose repository access:
   - authorize a public repository/branch; or
   - keep it private and authorize judge access for the addresses required by
     the current rules.
3. Decide whether to ship the unsigned preview installers or provide a Windows
   code-signing identity and authorize a signed rebuild.
4. Approve the final screen recording after checking voice, identity, account,
   notification, path, and project-data privacy.
5. Approve the public YouTube upload and the free test-build hosting location.
6. Authorize the Codex `/feedback` transmission and approve the text attached to
   the selected engineering Session ID.
7. Review every final Devpost field and press the final submission button.

## External work Codex can perform after explicit authorization

The owner does not need to do these steps manually, but each changes external
state and therefore remains paused:

- create or fill the unsubmitted Devpost draft;
- push the isolated submission branch or create a private judge branch;
- create a release and upload the approved installer artifacts;
- share a private repository with the required judge accounts;
- upload the approved video and test signed-out playback;
- enter approved URLs, hashes, Session ID, and copy into Devpost.

Final submission and owner attestations are not delegated even if the form is
otherwise prepared automatically.

The official deadline is **21 July 2026 at 5:00 PM Pacific**, equivalent to
**22 July 2026 at 01:00 in Lisbon**. Do not use the converted time as a target;
leave room to verify public links and signed-out access before submission.

## Recommended owner sequence

1. Join the event on Devpost and verify that the account is eligible to enter.
2. Choose public versus private repository access and unsigned versus signed
   installer delivery.
3. Authorize the chosen repository and binary-hosting actions.
4. Record the installed Connector demo with a disposable project using the
   under-three-minute script; the underlying installed-sidecar proof has already
   passed locally.
5. Review and authorize the video publication.
6. Authorize `/feedback`, then retain its receipt or confirmation.
7. Review the populated Devpost draft, its links, limitations, hashes, and
   eligibility disclosure.
8. Submit before the deadline and record the submitted URLs and timestamp in the
   local checklist.

## Files to review

- [Submission checklist](SUBMISSION_CHECKLIST.md)
- [Devpost draft](DEVPOST_DRAFT.md)
- [Demo script](DEMO_SCRIPT.md)
- [Judge quickstart](JUDGE_QUICKSTART.md)
- [Evidence manifest](EVIDENCE_MANIFEST.md)
- [Technical proof](TECHNICAL_PROOF.md)
