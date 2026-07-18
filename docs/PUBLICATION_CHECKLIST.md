# Publication checklist

This checklist records the gate from a private development history to a clean
public Code Hangar release. A checked item needs evidence.

## Public root

- [ ] Export the validated general-product tree into a new repository with a
  sanitized root commit authored by `JC-OM` using the GitHub no-reply address.
- [ ] Confirm the old private repository remains private and unchanged.
- [ ] Confirm `main` contains no Build Week copy, judge evidence, demo material,
  private paths, credentials, caches, build output, or acceptance data.
- [ ] Run `npm run audit:publication` with complete public Git history.

## Validation

- [ ] `npm ci`, `npm run check`, `npm audit --audit-level=high`, formatting,
  core tests, clippy, Connector frontend build, and Local Tauri build pass.
- [ ] Apache-2.0, `NOTICE`, `SOURCES.md`, and `THIRD_PARTY_NOTICES.md` describe
  the distributed source and runtime material.
- [ ] `SECURITY.md`, `CONTRIBUTING.md`, and `KNOWN_ISSUES.md` are current.
- [ ] CI is green on the only claimed platform (Windows); action revisions and
  default permissions are pinned/read-only.

## GitHub and release

- [ ] The repository is public at `jcomlabs/code-hangar`, default branch `main`,
  with the approved description and topics.
- [ ] Dependabot, private vulnerability reporting, secret scanning, and push
  protection are enabled where the repository plan supports them.
- [ ] The submission layer is a separate branch/commit above clean `main`.
- [ ] Release installers are built from the tagged public commit, uploaded with
  `SHA256SUMS`, downloaded again, rehashed, installed/launched, and uninstalled.
- [ ] Public profile links are updated only after the destination renders.
