# Roadmap finishers (R4) — completion + deliberate deferrals

The post-Gate-3 roadmap — **A0 branding preflight → R1 Phase-6 depth → R2 fase-fatal → R3 AI agnostic → R4 finishers** — is complete on `main` as of `c11a719`. Each phase was gated green locally on `scripts/local-ci.ps1 -AgentAutomation` and adversarially reviewed where the logic warranted (A0, R1.1, R1.2, R1.3, R2, R3).

This document closes out R4: the genuine finishers that were done, and the items the plan deliberately defers (with the rationale, so they are decisions on record, not omissions).

## Done in R4
- **UI polish for the new surfaces.** The R2 final-remove opt-in toggle and the R3 "Summarize with AI" controls/result reuse the app's styled primitives (`toggle-row`, `action-button`, `muted`, `warning-inline`) and get matching container styles in `styles.css` (theme-variable borders/spacing). The R1.6 Hangar-Map filter bar grows to 8 tabs but `.graph-map-filters` already `flex-wrap`s, so it stays usable on narrow viewports.

## Deliberate deferrals (decisions on record)
- **Product rename ("Code Ariadne" or another).** DEFERRED pending a chosen, availability-checked name (the "Ariadne" namespace is crowded). When it happens it is an **isolated** task that MUST preserve the bundle identifier `local.codehangar.desktop` and the app-data / encrypted-DB path — change only `productName`, docs and in-app branding — or existing users would orphan their encrypted catalog. Not mixed with AI/mutation work.
- **Codex-OAuth provider (ChatGPT-subscription via OAuth, not an API key).** DEFERRED — only if demand appears. The runtime Off / Local / API toggle plus BYO-key already cover the configured-provider need; an OAuth device-flow + refresh is a niche addition, and explicitly OUT of R3.
- **Edition-split ("local-AI-only, zero external HTTP" build).** DEFERRED — only if demand appears. The runtime Off/Local/API toggle already gives a user the "no external HTTP" posture by leaving AI Off or pointing it at a loopback model server; a separate build is redundant until requested.
- **Monolith decomposition** (`App.tsx`, `hangar-discovery`, `hangar-api`, `hangar-db` are large). Done **carve-on-touch**, not as a blind big-bang: each phase that touched a slice (e.g. R1.2 added `dup_jobs.rs`, R2/R3 extracted focused functions) kept it testable. Continued as ongoing practice.
- **Internal QA-record cleanup.** A previously-used encrypted DB carried a stale ad-hoc investigate root + an obsolete mutation journal from live QA. These are **excluded everywhere in the shipped build** (invisible — not a repo artifact, it is one user's runtime data); purging them needs the now-opt-in mutation app, or a Reset-all. No code change; nothing ships with them.
- **Broader UX (dark / OLED / high-DPI, Safe-Manage/Recover flow).** Continuous polish addressed as specific issues surface; the new R2/R3 elements are styled, and no regression was introduced.

## Release gate (unchanged)
Future release changes still need the local gate before publish. Pushes use `[skip ci]` where needed; GitHub Actions stays manual/core-only by choice and `scripts/local-ci.ps1` remains the authoritative gate for full local verification.
