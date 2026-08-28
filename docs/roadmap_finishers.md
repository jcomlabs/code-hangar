# Roadmap finishers (R4) — completion + deliberate deferrals

The post-Gate-3 roadmap — **A0 branding preflight → R1 Phase-6 depth → R2 fase-fatal → R3 AI agnostic → R4 finishers** — is complete on `main` as of `c11a719`. Each phase was gated green locally on `scripts/local-ci.ps1 -AgentAutomation` and adversarially reviewed where the logic warranted (A0, R1.1, R1.2, R1.3, R2, R3).

This document closes out R4: the genuine finishers that were done, and the items the plan deliberately defers (with the rationale, so they are decisions on record, not omissions).

## Done in R4
- **Historical UI polish for the then-current surfaces.** R2 originally shipped a
  persistent final-remove opt-in toggle, and R3's "Summarize with AI" controls/result reused
  the app's styled primitives (`toggle-row`, `action-button`, `muted`, `warning-inline`). The
  current permanent-removal contract supersedes that R2 local-user toggle with a fresh,
  digest-bound confirmation for each exact project/batch review. The R1.6 Hangar-Map filter bar
  grows to 8 tabs but `.graph-map-filters` already `flex-wrap`s, so it stays usable on narrow
  viewports.

## Deliberate deferrals (decisions on record)
- **Product rename ("Code Ariadne" or another).** DEFERRED pending a chosen, availability-checked name (the "Ariadne" namespace is crowded). When it happens it is an **isolated** task that MUST preserve the bundle identifier `local.codehangar.desktop` and the app-data / encrypted-DB path — change only `productName`, docs and in-app branding — or existing users would orphan their encrypted catalog. Not mixed with AI/mutation work.
- **Outbound model-provider integration.** The shipped Local edition remains
  zero-outbound and compile-absent from all provider/AI/MCP code. The separate
  Connector edition now permits one experimental, opt-in provider route through
  the exact feature-gated `hangar-ai` client, plus local MCP/app configuration;
  telemetry, updater, remote Git and implicit/background network remain outside
  both product boundaries. This supersedes the earlier R4 deferral recorded here.
- **Monolith decomposition** (`App.tsx`, `hangar-discovery`, `hangar-api`, `hangar-db` are large). Done **carve-on-touch**, not as a blind big-bang: each phase that touched a slice (e.g. R1.2 added `dup_jobs.rs`, R2/R3 extracted focused functions) kept it testable. Continued as ongoing practice.
- **Internal QA-record cleanup.** A previously-used encrypted DB carried a stale ad-hoc investigate root + an obsolete mutation journal from live QA. These are **excluded everywhere in the shipped build** (invisible — not a repo artifact, it is one user's runtime data); purging them needs the now-opt-in mutation app, or a Reset-all. No code change; nothing ships with them.
- **Broader UX (dark / OLED / high-DPI, Safe-Manage/Recover flow).** Continuous polish addressed as specific issues surface; the new R2/R3 elements are styled, and no regression was introduced.

## Release gate (unchanged)
Future release changes still need the hash-bound local gate before publish.
The repository ships no GitHub Actions workflow or Dependabot configuration, so
remote automation cannot consume CI credits or create routine notifications. It
cannot substitute for `scripts/local-ci.ps1 -AgentAutomation -SkipTauriBuild`,
the authoritative full local verification lane.
