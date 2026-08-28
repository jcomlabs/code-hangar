# Publication checklist

This checklist records the gate from a validated development candidate to a
clean public Code Hangar release. A checked item needs evidence.

## Public root

- [ ] Confirm `main` is exactly one root commit, with no parent, and that both
  its author and committer are `JC-OM` using the GitHub no-reply address.
- [ ] Confirm that root commit's tree object is byte-for-byte identical to the
  final validated source commit's tree object.
- [ ] Confirm development repositories and local-only refs remain private and
  are not mirrored into the public repository.
- [ ] Confirm the isolated candidate has exactly one local head (`main`), no
  tags, no shallow boundary, and exactly one remote named `origin`, whose fetch
  and push URLs are `https://github.com/jcomlabs/code-hangar.git`.
- [ ] Confirm `main` contains no private evidence, paths, credentials, caches,
  build output, acceptance data, or internal release experiments.
- [ ] In the isolated candidate, run
  `npm run audit:publication -- --source-tree <validated-source-tree-id> --evidence-dir .local/acceptance/v0.1.3/publication-audit/<new-run-id>`.
  Retain the newly created `PUBLICATION-AUDIT.private.json` and its printed
  SHA-256 outside Git. The proof is created only after a strict candidate pass
  and binds the exact commit/tree, one-root topology, identities, remote and
  pathname/content/history coverage. Before the root exists, use
  `npm run audit:publication:worktree` without `--evidence-dir`; that weaker mode
  is useful but is rejected as publication-candidate evidence.

## Validation

- [ ] On the release worktree, the authoritative local
  `scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild` lane passes and its
  private hash-bound evidence is retained outside Git. It covers formatting,
  frontend checks/build isolation, core/mutation/Connector Rust tests and
  Clippy, guards, release-script self-tests and compile-only Windows release
  builds. A mutable remote `npm audit` result is not release evidence.
- [ ] Apache-2.0, `NOTICE`, `SOURCES.md`, and `THIRD_PARTY_NOTICES.md` describe
  the distributed source and runtime material.
- [ ] `SECURITY.md`, `CONTRIBUTING.md`, and `KNOWN_ISSUES.md` are current.
- [ ] The only claimed platform is Windows. The source tree contains no GitHub
  Actions workflow or Dependabot configuration. Remote automation cannot consume
  CI credits or create routine notifications and cannot substitute for the local
  gate.

## GitHub and release

- [ ] The repository is public at `jcomlabs/code-hangar`, default branch `main`,
  with the approved description and topics.
- [ ] Private vulnerability reporting, secret scanning and push protection are
  enabled where the repository plan supports them. Dependabot PR automation is
  intentionally absent so publication cannot start weekly bot PRs or
  notifications; dependency review is manual/local.
- [ ] The only public branch is `main`. There are no dependency-update,
  campaign, release-candidate or experiment branches; no tag exists until its
  corresponding installer release has passed every owner gate.
- [ ] Release installers are built from the tagged public commit, uploaded with
  `SHA256SUMS`, downloaded again, rehashed, installed/launched, and uninstalled.
- [ ] Public profile links are updated only after the destination renders.
