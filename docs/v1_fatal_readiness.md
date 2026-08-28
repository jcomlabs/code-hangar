# v1 readiness — the "fase fatal" (final removal)

**Current decision (2026-08):** final removal is a first-class Code Hangar outcome. Its capability
and current state are visible in Recover, but the durable capability starts OFF. Enabling it requires
typing `ENABLE PERMANENT REMOVAL` exactly; Local preview, confirmation and batch start each recheck
the setting under the shared mutation boundary. Disabling asks any active batch to stop after its
current topology group; the worker rechecks the flag before helper resolution, token consumption or
deletion, so once disabling returns no admitted worker can continue. Safety then still requires an immutable preview,
an object/topology-complete archive proof, a short-lived single-use confirmation bound to that exact
preview, and a final handle-bound revalidation. A failure blocks only the affected object or topology
group; it does not disable cleanup for the rest of the batch.

The earlier 2026-06 global-toggle decision is retained in Git history and in the v0.1.1 Gate-3
evidence. It is not the product contract for the current release.

## Safety model
Final removal is the only step that destroys data, so it sits behind layered proof:
- **Discoverable, default-OFF and fail-closed per object.** Recovery does not hide the capability,
  but the local user must deliberately enable it with the exact activation phrase before a preview
  can be created. A DB, archive, identity, topology or revalidation error makes the affected object
  ineligible and leaves it held.
- **Two enforcement points.** Eligibility is checked when the immutable preview and single-use
  confirmation are issued and again immediately before the exact bound object is removed, so a stale
  token cannot bypass the gate.
- **The user confirms the exact batch.** Recover shows eligible, blocked and archive-required
  objects, topology groups, per-volume source bytes and retained archive allocation before it asks
  for the explicit irreversible acknowledgement. Changing the selection invalidates confirmation.
- **Every removal still requires:** a held source, an `object_archive/2` proof with a successful
  scratch round trip, unchanged handle-bound source identity and semantics, a currently readable
  archive, a fresh confirmation bound to the canonical preview digest, crash-consistent journaling
  and a readable per-object activity record. On Windows the final handle is rebound with no sharing
  and duplicated into an authenticated, separately signed cancellation guardian before delete-on-
  close may be armed. Protected objects can reach this point only through the earlier ownership and
  disclosure gates.

## Evidence

`close_authorized` is durable intent, never sufficient deletion authority. The
safe contract uses two related signals: an authenticated `HandleClosed` reply for
the normal fast path, and a one-shot keyed receipt written and
`FlushFileBuffers`-flushed by the guardian before it closes its duplicate. On
recovery, a lost ACK can settle only when the receipt is intact and bound to the
exact operation/object/mode/nonce **and** the exact guardian identity is proved
dead. A live/unknown guardian or missing, torn, substituted, replayed or
MAC-invalid receipt remains fail-closed. The broader release gate still requires
the automated/adversarial results plus the signed-helper and supervised NTFS/UAC
evidence listed in `docs/PERMANENT_REMOVAL.md`.

- **Existing v0.1.3 automated guardian evidence.** Deterministic Windows tests bind an exclusive
  final handle, duplicate it into the guardian protocol, introduce a hardlink in the pre-arm race,
  reject both post-arm proofs, inject parent cancellation failure, then remove the parent handle and
  authenticated pipe. The guardian independently cancels through its duplicate and both names and
  bytes remain readable. Receipt regressions additionally require flushed receipt
  plus exact guardian death after a lost ACK, keep a valid receipt pending while
  that guardian is alive, reject missing/torn/tampered/substituted and
  operation/object/mode/nonce replay, and never settle from `close_authorized`
  alone. These deterministic tests still do not simulate final signed
  executables, simultaneous process death, session teardown or power loss.
- **Current release hold.** Run the signed clean-VM process matrix, both extended and legacy
  disposition branches, UAC/installer journeys and real filesystem fault cases. A same-host guardian
  cannot guarantee preservation if it and the parent are terminated together, so machine-level
  failure remains an explicit residual and must not be advertised as a universal zero-data-loss
  guarantee.

The historical evidence below remains valuable regression evidence for the earlier v0.1.1
implementation, not a substitute for proving the new object-complete path.

- **Historical backend pipeline (automated).** `final_remove_journey_via_in_app_opt_in` and the ignored
  `gate3_final_remove_journey_on_real_files` exercise the full journey on throwaway temp files:
  opt-in → backup → move-to-holding → final remove → **the verified backup survives**. The latter
  passed again on 2026-07-12 under the dedicated `Gate3` acceptance lane. The wider Gate-3 and
  `hangar-mutation` suites cover crash consistency, interrupted-restore recovery,
  holding-area collision and backup-covers-file refusal.
- **Historical R2 adversarial review:** 0 findings on the then-current irreversible surface; it was
  confirmed that final removal could not run without the former opt-in.
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
