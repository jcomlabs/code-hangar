# Gate 3 — Mutation hardening checklist

The mutation executor (backup → quarantine/move → restore → permanent delete) lives in
`crates/hangar-mutation`, behind the `mutation` feature, and is absent from the strict
`core` lane. The Local and AI Connector editions can include mutation for user-approved
local management, but final removal remains **OFF by default**. This gate must hold before
any published mutation-capable build is treated as safe for normal users.

### Empty-the-folder-completely (opt-in)

Removing a folder **always empties it 100%** — there is no partial mode. `include_protected`
is the standard behaviour (the UI passes it for every backup + move; there is no opt-out
checkbox): sensitive/protected files are backed up first and then moved like any other file
(so the backup-before-delete invariant still holds — the move-time content-binding check
refuses any sensitive file not covered by the verified backup), and junction/symlink **links**
are removed without ever being followed (the target is never touched; `remove_reparse_link`
refuses anything that is not currently a reparse point). The complete verified backup is what
makes this reversible: if removal breaks something the user restores from it. A strong
confirmation (`mutation_preview_protected`) still lists exactly which sensitive files and links
are included, and discloses that secrets are copied into the backup, before anything runs.
Two adversarial re-audits confirmed this path does not reopen any irreversible-loss or
backup-bypass path. Hard per-item issues (locked / identity-changed / missing) are reported
as failures; already completed entries remain recoverable, and the overall operation never
claims complete success when any file or disclosed link stayed behind.

## Non-negotiable invariants (enforced in the backend, not the UI)

1. **No permanent delete without a verified backup that covers the file.**
   `permanent_delete_entry` (`purge.rs`) refuses (`BackupRequired`/`BackupUnusable`) unless
   the entry links a `backup` row with `verified = 1` whose manifest (re-read from disk,
   `verified: true`) contains the file's source path. `mutation_move_start` refuses to move
   anything into the holding area unless a verified backup covers **every** concrete plan
   file. The state machine has no `Confirmed → Executing` edge — execute only follows
   `BackupVerified`. *Tests:* `permanent_delete_refused_without_a_verified_backup`,
   `execute_requires_a_verified_backup_first`.

2. **Plan freshness.** `concrete_items_for_plan` rebuilds the Operation Plan and aborts if
   `target_fingerprint` changed since the preview was built — so neither backup nor move
   acts on a stale plan. (`hangar-api/src/lib.rs`.)

3. **Protected entries require ownership proof, disclosure and complete backup; reparse
   targets are never followed.** Protected/sensitive files stay outside recoverable-space
   estimates. The complete-folder path may include only entries that accounting proves are
   locally mutation-owned; `mutation_preview_protected` discloses them, and the move refuses
   unless the verified backup content-covers every file. Shared/external protected entries
   remain ineligible. Reparse points are not backed up or followed: only the disclosed link
   itself may be removed after execution-time identity revalidation, and its target is never
   touched.

4. **Crash recovery.** Every destructive action is journaled before it runs. The permanent
   delete flips the entry to `deleting` **before** the unlink; `recover_interrupted`
   reconciles it by the on-disk truth of the held copy (back to `quarantined` if present,
   `permanently_deleted` if gone). Interrupted quarantines roll back (files returned). If
   both original and held copies exist, neither is discarded and the held copy is exposed
   in Recover; if neither side of a restore is visible, the operation stays `verifying` and
   blocks every new mutation. Terminal `failed` is reserved for reconciled outcomes.
   *Tests:* `rolls_back_an_interrupted_quarantine`,
   `exposes_a_cross_volume_copy_left_beside_the_original`,
   `keeps_a_restore_blocking_when_both_copies_are_missing`,
   `recovery_guard_blocks_only_unreconciled_operation_states`,
   `reconciles_an_interrupted_permanent_delete`.

5. **Backup engine safety.** Verify-after-write (blake3) before a backup is marked usable;
   engine-level path safety (only plain components, no `..`/absolute/drive escape); no
   overwrite of an existing file or manifest; destination refused if protected/sensitive or
   inside the source tree; large same-volume and insufficient-space refused.

6. **Recursive folder deletion is bounded.** After a project/folder move, only the now-empty
   source directories within `cleanup_root` are removed (deepest-first); a directory still
   holding skipped content survives, and nothing above the root is touched.

7. **Final removal is irreversible and remains off by default in published mutation-capable
   builds.** Hard unlink, no Recycle Bin. During this Gate-3 wave the "Final remove" button
   was gated behind `import.meta.env.DEV` and the backend refused `mutation_final_remove_start`
   / `final_remove` tokens unless `CODEHANGAR_ENABLE_FINAL_REMOVE=1` was set for supervised QA.
   The current release path keeps the action unavailable until explicit Recover opt-in (or that
   supervised-QA env override); the per-action verified-backup + fresh-token + protected-refusal
   boundary below is unchanged. Hiding the button is not the safety boundary.

## Release gate (local, not GitHub Actions)

`scripts/local-ci.ps1` is the **mandatory written gate** for any mutation change. It must be
green before merging or building a mutation-enabled artifact. It runs both feature sets:

- `cargo test/clippy --no-default-features --features core` (`-D warnings`)
- `cargo test/clippy --no-default-features --features mutation` (`-D warnings`)
- `npm run check` (tsc + vitest), `tauri:build` (core) and `tauri:build:mutation`
- the safety guardrails: `check-no-forbidden-code.mjs`, `check-no-outbound-deps.mjs`, and
  `cargo tree --features core` must contain neither `hangar-mutation` nor `hangar-agent`.

GitHub Actions stays manual-trigger / core-only by choice; the local gate is the source of
truth for the mutation feature.

## Adversarial audit of the delete path (done — converged)

A dedicated multi-agent adversarial audit was run against the executor (four attack
dimensions, each finding refuted by independent skeptics), then further targeted
re-verification rounds. The initial rounds found and **closed** five real defects before
any flip-on:

1. **Backup matched by path, not content** (HIGH): `covers()` was a path-key check, so a
   stale/unrelated same-path backup could authorize deleting different live content. Now
   content-bound at both ends — the move requires each current source to hash-equal the
   backup's recorded blake3, and the purge requires the held copy to hash-equal it.
2. **Backup payload never proven restorable** (HIGH): the gate trusted the manifest hash
   but never confirmed the payload file still existed/matched; restore reads the held copy,
   so a missing/truncated payload made purge irreversible. `verify_payload` now re-checks
   the payload exists and re-hashes to its recorded value before the unlink.
3. **Interrupted restore mis-recovered** (HIGH): a completed-but-uncommitted restore was
   reverse-moved back into quarantine and left stuck. Restore is now excluded from the
   generic rollback and reconciled by on-disk truth, keyed by the entry id in `plan_json`
   (not the non-unique holding path).
4. **Recovery was advisory** (MED): the forward mutation entry points now refuse while any
   prior operation is left interrupted.
5. **Holding-area collision** (HIGH): the quarantine mover was the only mover without an
   occupied-destination guard, so two projects sharing a relative path moved into one
   holding folder could overwrite the first held copy. Holding paths are now namespaced per
   operation and the mover refuses to overwrite.

The v0.1.1 RC H5 recovery-state audit then found and closed four additional MEDIUM defects:

6. **Post-move restore warning left a phantom held entry**: the file was already at its
   destination, but a validation error marked the item failed while the entry still claimed
   `quarantined`. The executor now enters `verifying` after the move and records physical
   completion before returning the warning.
7. **Interrupted cross-volume duplicate could be invisible**: a stop after copy verification
   but before entry insertion left both copies while recovery retired the operation. Recovery
   now preserves both and synthesizes a recoverable held entry.
8. **Reparse removal failure reported success**: a raced link-to-directory replacement was
   safely refused but did not increment the failed count. It now makes the operation fail.
9. **Cross-volume recovered bytes were optimistic**: a failed source unlink still counted the
   copied bytes as freed. The unlink now controls both success and recovered-space accounting;
   failure retains the held entry and reports zero bytes.

Every current finding has focused regression or direct harness coverage. The canonical
acceptance report records the mandatory full-gate evidence and the completed release-GUI
journey.

## Live GUI QA findings (real machine) — fixed

Driving the actual `mutation` `.exe` with computer-use (physical clicks + a literal forced
process kill) surfaced four real defects that the whole automated suite and the adversarial
code rounds missed, because they bypass the Tauri ACL and use synthetic in-memory fixtures:

1. **In-app confirmations were ACL-blocked.** `window.confirm` is routed by the webview to
   `plugin:dialog|confirm` and refused by the capability ACL ("not allowed by ACL") even with
   `dialog:allow-confirm` granted, silently breaking every destructive confirmation (move /
   restore / final-remove / empty / discard / reveal). Replaced all `window.confirm` with a
   promise-based **in-app confirm modal** (`requestConfirm`) — no ACL dependency, identical in
   the core and mutation builds.
2. **Investigating a folder reported "0 files / 0 B" and an empty preview**, blocking the
   destructive ritual. Root cause: `scan_inventory_stream` canonicalizes the walk root, so on
   Windows the indexed file nodes carry an extended-length `\\?\C:\…` path that does not share
   `scan_root.path`'s textual prefix — and `investigation_report` counted files with a
   `path LIKE 'C:\…\%'` match, which silently returned zero. Now counts via the
   `nav_item.project_id` join (the canonicalization-robust relationship the accounting already
   uses). *Test:* `investigation_report_counts_files_under_an_adhoc_root`.
3. **The investigated folder was not wired as the plan target**, so the review's manual
   "Calculate preview" and the backup/move actions could not resolve a target (an ad-hoc
   folder is deliberately kept out of the projects list). `runInvestigate` now sets the ad-hoc
   node as `planTargetNode`/`selectedProjectId`.
4. **Final remove was hard-blocked in any release build.** `runMutationFinalRemove` short-
   circuited on `import.meta.env.PROD`, so the release mutation build refused even with the env
   opt-in set and the button shown. It now gates on `!finalRemoveEnabled` so a release build still
   blocks by default but honours explicit Recover opt-in or `CODEHANGAR_ENABLE_FINAL_REMOVE=1`.
   The `mutation_final_remove_enabled` command surfaces that runtime state. The backend opt-in
   gate and the verified-backup-covers-the-file invariant still enforce the action.

## Current v0.1.1 RC adversarial QA (2026-07-12)

The canonical status and machine-local evidence pointers are in
[`../qa/v0.1.1-acceptance.md`](../qa/v0.1.1-acceptance.md). The current release GUI run uses
only the throwaway folder under `.local/acceptance/v0.1.1/gate3-gui/20260712-051526`:

- the pre-backup move refusal was exercised;
- verified backup #5 wrote all four locally owned files (111 B), including the disclosed
  `.env`; the junction itself was disclosed but not followed or backed up;
- holding operation #8 moved the four file entries and removed only the junction link;
  `outside-target/must-survive.txt` remained byte-for-byte intact;
- the release process was terminated with `Stop-Process -Force`; reopening reconstructed
  backup #5, operation #8 and all four held entries from the encrypted journal;
- `restore-me.txt` was restored to its original path with its exact marker;
- the current ignored real-file journey
  `gate3_final_remove_journey_on_real_files` passed again on 2026-07-12 and proved
  opt-in -> verified backup -> holding -> permanent unlink -> backup survives.
- after current-turn authorization, the release GUI separately final-removed `.env`,
  `remove-me.txt` and `nested/context.md`; Recover ended with zero held entries, all three
  backup hashes still matched, `restore-me.txt` and the external junction-target sentinel
  survived, and final removal was turned off again.

The authoritative result is
`.local/acceptance/v0.1.1/gate3-gui/20260712-051526/gate3-final-result.json`.
The GUI executable must be built with `npm.cmd --workspace apps/desktop run
tauri:build:mutation`; a bare `cargo build` omits Tauri's release custom protocol and opens
the development URL instead of the embedded frontend.

Two boundaries remain primarily automated because a GUI run cannot reliably create or hit
them: refusal without a covering backup (`permanent_delete_refused_without_a_verified_backup`)
and the precise mid-operation interruption windows
(`rolls_back_an_interrupted_quarantine`, `reconciles_an_interrupted_permanent_delete`).
