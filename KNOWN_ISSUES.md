# Known issues

Code Hangar `0.1.x` is an early alpha.

- Windows is the only supported desktop platform. WSL projects are catalogued
  from Windows; there is no native Linux or macOS build.
- Installers are not yet Authenticode-signed, so Windows SmartScreen may show an
  unknown-publisher warning.
- The AI Connector is an advanced preview. Host-app registration and round-trip
  behaviour can vary as those applications change their local configuration.
- Safe Manage AI recommendations depend on the selected context and configured
  model. They can materially disagree with the deterministic baseline and may be
  wrong; the UI keeps both visible, rejects malformed typed results and requires
  an independent explicit user decision before any plan or disk action.
- Session reconstruction is evidence-bounded. Unsupported or missing tool
  records remain visible as unknowns rather than inferred changes.
- The project has extensive deterministic and lifecycle tests, but no claim of
  an independent security audit.

Please use the [issue tracker](https://github.com/jcomlabs/code-hangar/issues)
for reproducible, non-sensitive reports. Use private vulnerability reporting
for security issues.
