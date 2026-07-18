# Security policy

Code Hangar is an early Windows alpha. Security fixes are applied to the latest
`0.1.x` release; older preview builds may not receive backports.

## Report a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/jcomlabs/code-hangar/security/advisories/new)
for anything that could expose local files, credentials, AI-provider requests,
connected-app authority, backups, or destructive operations. Do not include
secrets, private project data, or exploit details in a public issue.

For a non-sensitive security question, open a
[public issue](https://github.com/jcomlabs/code-hangar/issues). This project has
no bug-bounty programme and cannot promise a fixed response time, but reports
will be acknowledged and triaged as promptly as possible.

## Security boundaries

- The Local edition is built without the connector and outbound-network crates.
- The AI Connector edition is opt-in and can contact only the local model server
  or provider endpoint configured by the user.
- MCP uses a local child-process channel. Read and write authority remains
  project- and scope-gated, audited, and subject to in-app approval.
- Protected Zones, credential material, and detected secrets are excluded from
  preview/search and blocked before AI requests.
- Backup-before-delete, holding-area recovery, and fresh confirmation are
  mandatory safety boundaries; permanent removal is disabled by default.

The detailed implementation invariants and threat model are in
[`SECURITY_INVARIANTS.md`](SECURITY_INVARIANTS.md). A passing test suite is not a
claim of independent security review.

## Release integrity

Release installers are currently unsigned and may trigger Windows SmartScreen.
Every published asset must be accompanied by a SHA-256 manifest. Verify the
downloaded bytes before running an installer.
