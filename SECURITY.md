# Security policy

Code Hangar is an early Windows alpha. Security fixes are applied to the latest
`0.1.x` release; older preview builds may not receive backports.

## Report a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/jcomlabs/code-hangar/security/advisories/new)
for anything that could expose local files, credentials, connected-app requests,
connected-app authority, backups, or destructive operations. Do not include
secrets, private project data, or exploit details in a public issue.

For a non-sensitive security question, open a
[public issue](https://github.com/jcomlabs/code-hangar/issues). This project has
no bug-bounty programme and cannot promise a fixed response time, but reports
will be acknowledged and triaged as promptly as possible.

## Security boundaries

- Local is built without AI-provider/MCP code and without outbound HTTP, DNS,
  telemetry or updater clients.
- The AI Connector edition is opt-in. It adds local app configuration,
  child-process MCP stdio and Windows named-pipe integration, plus one
  feature-gated provider client. That client can contact only the loopback or
  HTTPS endpoint the user explicitly configures for a disclosed AI Assist
  operation; it is not telemetry, an updater or background traffic.
- MCP read and write authority remains
  project- and scope-gated, audited, and subject to in-app approval.
- Protected Zones, credential material, and detected secrets are excluded from
  preview/search and blocked before connected-app reads.
- Backup-before-delete, holding-area recovery, an immutable final preview, and
  a fresh single-use confirmation are mandatory safety boundaries. Permanent
  removal stays available to the local user; connected-app recommendations are
  separately gated and default off.

The detailed implementation invariants and threat model are in
[`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md). A passing test suite is not a
claim of independent security review.

## Release integrity

No `0.1.3` installer is approved for publication yet. A release must record an
explicit structured signing decision: inner application/helper signatures are
mandatory; unsigned setup/uninstaller bytes require a separate owner-approved
disclosure and may trigger Windows SmartScreen. Every published asset must be
accompanied by the final-byte SHA-256 manifest. Verify downloaded bytes before
running an installer.
