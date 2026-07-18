# v1 readiness — the "fase fatal" (final removal)

**Decision (2026-06):** final removal ships **available but OFF by default**. Users must opt in from
Recover before Code Hangar offers the irreversible action. Every other safety layer is unchanged.
This document records the safety model + the evidence behind that decision.

## Safety model
Final removal is the only step that destroys data, so it sits behind layered friction:
- **Default OFF, fail-closed on error.** `final_remove_runtime_enabled(state)` = supervised-QA env
  var **OR** the encrypted `final_remove_enabled` setting explicitly set to `"1"`; a DB read error
  resolves to OFF (`.unwrap_or(false)`), so a broken DB never enables it.
- **Two enforcement points.** `ensure_final_remove_runtime_enabled` is checked both when a mutation
  token is issued (`PermanentDelete`) and again at execution, so a stale
  token cannot bypass the gate. If it is off, the refusal is *"Final removal is turned off. Enable
  it in Recover…"*.
- **User must opt in.** Recover ▸ "⚠ Final removal (irreversible) — off"; checking it asks for a
  confirmation. Unchecking hides the per-entry "Final remove" button again.
- **Every removal still requires:** a verified backup that covers the original file, a held copy
  whose content hash still matches that backup, a currently restorable backup payload, a fresh
  per-action confirmation, crash-consistent journaling and a readable activity log. Protected files
  can reach this point only through the earlier ownership + disclosure + complete-backup gate.

## Evidence
- **Backend pipeline proven (automated).** `final_remove_journey_via_in_app_opt_in` and the ignored
  `gate3_final_remove_journey_on_real_files` exercise the full journey on throwaway temp files:
  opt-in → backup → move-to-holding → final remove → **the verified backup survives**. The latter
  passed again on 2026-07-12 under the dedicated `Gate3` acceptance lane. The wider Gate-3 and
  `hangar-mutation` suites cover crash consistency, interrupted-restore recovery,
  holding-area collision and backup-covers-file refusal.
- **R2 adversarial review:** 0 findings on the irreversible surface; it was confirmed that final
  removal **cannot run without the opt-in**.
- **Perf gate stage 2 (blocking)** now guards against a gross slowdown silently regressing the delete
  pipeline (`scripts/local-ci.ps1 -PerfGate`, generous 2× tolerance + 5 s floor so it catches
  catastrophe, not noise).
- **No stale internal QA records ship.** Ad-hoc "Investigate a folder" roots are flagged `adhoc = 1`
  and excluded from the projects list, discovery and scan-root settings (hangar-db); the mutation
  journal is a mutation-edition surface, absent from the strict `core` lane.

## Hands-on pass on the current RC exe (2026-07-12)

The current run is restricted to
`.local/acceptance/v0.1.1/gate3-gui/20260712-051526`; no user project is a mutation target.

- **Default OFF and explicit opt-in were live.** Recover hid every per-entry irreversible action
  until the installation setting was enabled through the danger confirmation.
- **Complete protected backup was live.** Backup #5 wrote all four locally owned files (111 B),
  including the disclosed `.env`. The disclosed junction was not followed or backed up.
- **Holding and link containment were live.** Operation #8 held all four files and removed only the
  junction link. `outside-target/must-survive.txt` remained intact.
- **Crash/reopen was live.** The release process was forcibly terminated after the move. Reopening
  reconstructed the verified backup, operation and held entries from the journal.
- **Restore was live.** `restore-me.txt` returned to its original path with its exact marker.
- **Final GUI removal passed.** The three held throwaway files were separately confirmed and
  permanently removed. Recover ended with zero held entries; backup #5 still matched all three
  hashes, `restore-me.txt` and the external junction-target sentinel remained intact, and the
  irreversible option was turned off again. Evidence:
  `.local/acceptance/v0.1.1/gate3-gui/20260712-051526/gate3-final-result.json`.

The canonical status is [`qa/v0.1.1-acceptance.md`](qa/v0.1.1-acceptance.md). The older June GUI
exercise remains useful historical evidence; the current RC result above is the release evidence.
