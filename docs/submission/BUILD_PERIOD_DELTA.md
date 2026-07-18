# Build-period delta and provenance

## Honest project status

Code Hangar existed before OpenAI Build Week 2026. This submission does not claim
that the entire application was created during the event.

- **Declared pre-event baseline:** `843530c`
- **Baseline date:** 12 July 2026
- **Eligible Build Week work begins:** 14 July 2026
- **Product candidate:** `e831c14dfa15291dda152d7742766221438feaa3`
- **Review range:** `843530c..e831c14dfa15291dda152d7742766221438feaa3`

Those identifiers belong to the preserved private development repository. Its
legacy author metadata was not safe to publish, so the public repository does
not pretend that the private range is directly resolvable. Instead, public
`main` is a single privacy-audited source snapshot, and the removable event
layer is fully inspectable as `main..submission/openai-build-week`.

## Required commit separation

The candidate history is intentionally split by reversibility, not by marketing
story:

| Layer | Intended contents | Final commit(s) |
|---|---|---|
| A. Reusable product snapshot | Eligible reusable work, general hardening, GPT-5.6 provider contract, tests, public docs, and publication gates; no Build Week branding | public `main` root |
| B. Submission-only package | Version 0.1.2 metadata, README event pointer, this root submission page, `docs/submission/`, proof scripts, demo copy, judge checklist, and fixture captures | commits in `main..submission/openai-build-week` |
| C. Private provenance record | Original eligible-period hashes retained locally to substantiate dates and scope without publishing unsafe legacy identity metadata | declared hashes above; not part of public Git history |

The event-specific layer must be the top commit or a contiguous top-commit
stack. It must not contain product behavior, general security fixes, or general
tests.

## Build Week change inventory

This table was derived from the final diff and candidate validation. It does not
infer completion from a plan or historical acceptance evidence.

| Area | General reusable change in candidate | Proof |
|---|---|---|
| Retrospective evidence | Review Inbox, What changed, recap coverage/unknowns, saved review history, and a private-safe Review Receipt | frontend review/recap suites plus full Rust gate in [the final manifest](EVIDENCE_MANIFEST.md) |
| Large-project usability and bounds | Progressive graph expansion, dependency-cache separation, source-path fallbacks, vendored Cargo-cache exclusion, and a hard 1,000-item API/MCP ceiling | `project-center-view` tests, graph/API/MCP regressions, and the final manifest |
| Beginner-facing review UX | Clearer project-centre navigation, beginner help, context actions, and progressive disclosure without silent truncation | beginner, context-menu, project-workspace, recap, and UI suites |
| GPT-5.6 integration | Explicit OpenAI GPT-5.6 preset and current direct-OpenAI request contract while provider compatibility remains intact | `hangar-ai` direct-OpenAI and compatible-endpoint contract tests in the final manifest |
| Subscription-backed MCP proof | Submission-only harness runs GPT-5.6 Sol in ChatGPT-authenticated Codex against a disposable scoped Code Hangar MCP catalog, verifies audit, then revokes credentials | `scripts/submission/codex-gpt56-mcp-smoke.ps1` and the sanitized local result in the final manifest |
| Candidate clean-machine proof | Submission-only runner stages exact candidate hashes in an empty network-disabled Windows Sandbox, validates fail-closed start/final state, and never converts a policy block into a product pass | `scripts/submission/sandbox-candidate-lifecycle.ps1` and the sanitized Sandbox summary in the final manifest |
| Secret and Protected Zone behavior | Existing fail-closed preview/send boundary retained while the GPT-5.6 path is added | AI safety, protected-zone, edition, and forbidden-code gates in the final manifest |
| Packaging/readiness | Sequential Connector/Local packaging, edition isolation, version 0.1.2 metadata, and staged portable filenames | final manifest plus local `SHA256SUMS` |

## How to audit the eligible delta

```powershell
# Inspect the complete, reversible public event layer
git log --oneline main..submission/openai-build-week
git diff --stat main..submission/openai-build-week
git diff --find-renames main..submission/openai-build-week

# Prove main contains no submission package
git ls-tree -r --name-only main -- BUILD_WEEK_SUBMISSION.md docs/submission scripts/submission
```

Commit timestamps are supporting context, not proof by themselves. The baseline,
diff, test evidence, and submitted narrative together define what is being
claimed.

## Removal after the event

The event branch forms one contiguous stack above public `main`. Revert that
stack newest first, or continue product work directly from `main`:

```powershell
$submissionCommits = git rev-list submission/openai-build-week ^main
foreach ($commit in $submissionCommits) {
  git revert $commit
}
```

The clean continuation point is `main`. No destructive history rewrite is
required.

After removing layer C, these should remain:

- all general product hardening;
- the reusable GPT-5.6 integration and its tests;
- normal product documentation and security invariants.

These should disappear:

- `BUILD_WEEK_SUBMISSION.md`;
- `docs/submission/`;
- `scripts/submission/`;
- event-specific screenshots, video copy, TODO URLs, and judge manifests.

No Build Week badge, wording, or deadline-dependent behavior is added to the
application UI, so the product can continue cleanly after the submission package
is removed.
