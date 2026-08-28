# Permanent removal contract

Status: **normative product and release gate; implementation evidence is still
required**.

Permanent removal is a primary Code Hangar capability. It is deliberately OFF by
default, but must remain explicit and discoverable rather than becoming a hidden
or file-by-file escape hatch. The product must let an owner who deliberately
enables it finish removing dead projects and genuinely release their storage while
refusing only the concrete objects whose recovery proof is incomplete.

This contract supplements `SECURITY_INVARIANTS.md` and the Phase 3 rules in
`docs/engineering_details_by_phase.md`. If a stricter invariant applies, the
stricter invariant wins. A release must not claim full removal merely because a
project was moved into the recovery holding area.

## Product rules

1. The permanent-removal capability and its state are always discoverable in
   Recovery. It starts OFF. Enabling the durable owner setting requires typing
   `ENABLE PERMANENT REMOVAL` exactly; there is no environment-variable bypass.
   Preview, confirmation and batch start each recheck the setting, and disabling it
   immediately blocks start even after a confirmation was issued. This setting only
   makes the workflow available: authority to delete is still granted solely by the
   fresh, digest-bound confirmation for the exact batch shown.
2. The normal unit of work is a project or explicit multi-project batch. One
   structured confirmation and, when required, no more than one visible UAC
   consent cover that immutable batch preview.
3. Eligibility is fail-closed per object or atomic topology group. An unsupported
   object remains held with a stable reason and remediation; independent proved
   objects may proceed. Required parent directories remain until their blocked
   descendants are gone.
4. The elevated helper may capture and round-trip recovery objects. It has no
   generic path command and no delete/purge capability. Source disposition is a
   non-elevated, journaled, exact-handle operation after the proof-ready compare
   and set.
5. `backup_manifest/1` is content-only legacy evidence. It cannot authorize
   object-complete final removal or metadata-faithful cross-volume movement.
   That authority requires a verified `object_archive/2` proof for the exact
   object and topology group.
6. Recovery archives remain after final cleanup. The interface must say so. A
   future action that erases every recovery copy is a different, separately
   specified irreversible workflow.
   For large/full cleanup, the default recommendation is a verified archive on a
   different volume; a same-volume destination cannot be described as whole-disk
   space recovery and requires an explicit warning before override.
7. A partial result is first-class. Code Hangar reports exactly what was deleted,
   what remains, why it remains and whether the original project root still
   exists. It never labels a residual tree as fully removed.

## Object archive v2

An object-complete proof is created from already-bound parent handles and a
short-lived authenticated capability. For each object the helper must:

1. duplicate the parent-bound source and `CREATE_NEW` archive handles, then
   prove that the archive handle resolves to the exact path committed in the
   authenticated capability before reading or writing archive bytes; for a new
   archive that path is derived from the one-shot nonce and global batch index;
2. reopen only the same FileId with the narrowly enabled backup, restore and
   security privileges;
3. capture the allowlisted Windows backup stream representation, including the
   unnamed data stream, named data streams, extended attributes and the complete
   security stream;
4. write a bounded, checksummed `CHOBJV2` container;
5. restore it into a disposable object on the target NTFS volume;
6. recapture that object and require semantic equality;
7. recapture the still-bound source and require that it has not changed;
8. dispose of the scratch object through its exact handle; and
9. return a proof only after the archive is durable and scratch cleanup is
   complete.

Files and directories are separate objects; a directory archive never implies
that its children were captured. Tree topology is explicit and directories are
cleaned bottom-up. Hardlinks are an atomic topology group and physical storage is
counted once. Symlinks and junctions require a no-follow link-object profile that
captures and round-trips the reparse payload; their targets are never traversed.

The initial supported profile may be narrower than every NTFS feature, but the
limitation is per object. Cloud/recall objects, EFS without an encrypted raw-data
profile, unknown stream kinds, unproved external hardlinks, non-local filesystems
and identity drift are blocked with stable reason codes rather than silently
degraded to content-only copies.

### Final-disposition profile for v0.1.3

`object_archive/2` can capture and round-trip named data streams, but archive
support does not by itself make the final unlink race-free. On NTFS, another
process can create a hardlink or a named stream while the default-stream handle
is open, even when that handle grants no write or delete sharing. A default-stream
oplock also does not cover those namespace changes.

The required v0.1.3 release contract is the following conservative two-process,
same-handle sequence. The code-level receipt/ACK paths and regression tests do
not by themselves lift release HOLD: the signed-helper, UAC, process-death and
clean-VM matrix must still prove the exact final bytes:

1. after the elevated archive helper has returned, drop its share-compatible
   source handle and rebind the reviewed FileId/length/mtime/default-stream hash
   with no sharing; inability to obtain that exclusive final handle keeps the
   object held;
2. start the separately signed `code-hangar-elevated.exe` image in its fixed,
   non-elevated guardian mode; authenticate a fresh local named-pipe session and
   let that exact process duplicate the parent's final handle (no object path or
   handle value appears on its command line); launch uses Windows job breakaway
   so a parent kill-on-close job cannot silently kill both processes, and fails
   before arming when the enclosing job forbids breakaway;
3. make the guardian handle binding and `unprovedFinalProfile` delete intent
   durable in one transaction before either process may arm disposition;
4. durably authorize the arm, then arm delete-on-close on the parent handle;
5. require both the parent and guardian duplicates to observe `DeletePending`,
   the default-only stream profile and the mode-specific hardlink count
   (`FileDispositionInfoEx` and its legacy fallback expose different post-arm
   counts);
6. atomically move the item to `final_profile_proved_held`, then durably record
   `close_authorized` as an **intent only** before either armed handle is
   intentionally closed; and
7. send an authenticated close frame bound to the fresh session, duplicated
   handle, operation/item identity, FileId, disposition mode and nonce. After
   revalidation, the guardian writes a one-shot canonical receipt protected by
   a keyed MAC, verifies its bound file identity/length and calls
   `FlushFileBuffers` **before** closing the duplicated target handle. It then
   returns and flushes the authenticated `HandleClosed` reply. That reply is the
   normal fast-path ACK; it is not the sole crash-recovery authority. If the ACK
   is lost, recovery may settle absence only after it verifies the exact receipt
   and proves the exact guardian process identity is dead. A live guardian keeps
   the item pending. An unknown/live process state or a missing, substituted,
   torn, replayed or MAC-invalid receipt remains fail-closed and is never silently
   promoted to `deleted`. `close_authorized` alone is never deletion authority.

A pre-existing or raced named stream, additional hardlink or ambiguous query is
blocked per object and remains recoverable; it does not disable permanent
removal for independent plain files. A named-stream archive remains valid for
restore even when its held object is ineligible for final disposition.

If a pre-proof step fails after the kernel accepted delete-pending, both peers
attempt mode-aware cancellation and require the duplicated handle to report
`DeletePending=false`. If neither peer can prove cancellation, the parent leaks
its handle instead of closing it and disconnects from the guardian. The guardian
then keeps its duplicate open and retries cancellation without a timeout. This
closes the former parent-only crash gap: an abrupt desktop-process exit no longer
closes the last armed handle while the guardian remains alive.

Even when the parent proves cancellation first, it still sends the authenticated
`Cancel` frame. That frame excludes the parent's one possible future arm in the
guardian state machine and lets the child close its share-zero duplicate and
receipt after proving cancellation. Omitting it would preserve bytes but retain
the object until desktop exit, incorrectly blocking a same-session retry.

The guardian is not a claim of arbitrary-power-loss preservation. It is a
separate process on the same Windows host, not a service or a storage replica.
Simultaneous termination of both processes (including forced guardian kill,
session teardown, restart or power loss) can still close an unproved armed file
object before cancellation. The first public release therefore still requires a
supervised signed-binary process-kill matrix, and must describe machine-level
failure as a residual rather than promise zero data loss. Directories are
archive objects separate from their children and remain subject to bottom-up
emptiness/topology proof; the guardian does not turn one directory handle into a
recursive tree snapshot.

For the supported plain-object profile, the no-sharing final rebind also refuses
an already-open external reader/writer/deleter, so Code Hangar does not knowingly
report success while such a handle keeps the file allocation pending. NTFS may
still expose free-space changes asynchronously, and directory descendants or
unsupported filesystem profiles are not covered by that statement. Observed
free-space deltas remain measurements, never disposition authority.

## Elevated helper boundary

The helper is a one-shot, mutation-only Windows executable. It is neither a
service nor a daemon and exposes no network surface.

- The parent creates a random, first-instance, remote-client-rejecting named pipe
  with a restrictive DACL before launch.
- Both peers verify PID, session, process start identity, image identity and the
  release signature before the parent sends the secret framing key.
- Authenticated frames bind protocol version, operation/batch identity, role and
  a strictly increasing sequence number. A second request or replay is refused.
- Large confirmed batches use one helper/UAC session and authenticated bounded
  chunks. The pre-UAC commitment contains every durable capability field,
  including source/archive identity and archive path; only process-local raw
  handle values are materialized lazily and excluded from that commitment.
- The caller creates one CSPRNG transport nonce, persists it with the batch
  journal before UAC and the transport uses that exact nonce verbatim. Pipe,
  archive-partial and scratch names are derived from the persisted nonce plus
  global item index, so recovery never has to guess or scan for helper residue.
- Filesystem authority travels as duplicated, least-privilege parent handles;
  an unauthenticated command line never supplies arbitrary readable paths.
- Only `TokenElevationTypeFull` at high integrity is accepted. The helper enables
  exactly `SeBackupPrivilege`, `SeRestorePrivilege` and `SeSecurityPrivilege`,
  proves their effective state and restores the previous token state on every
  exit path.
- Production object-complete proofs require a signed parent and helper. An
  explicit unsigned development mode is restricted to generated synthetic
  fixture roots and cannot authorize a real purge.

The command line contains only fixed protocol selectors, the pipe name and
parent identity. It contains no secret, object path, archive path, shell command
or delete verb.

## Batch lifecycle and crash safety

The server builds the preview from the complete persisted removal group, never
from a paginated activity log or the currently registered project list. A
confirmation capability is generated with the OS CSPRNG, expires quickly, is
single-use and is bound to the preview digest plus the selected topology groups.

The journal records batch and item intent before every side effect. The expected
states are:

```text
held -> archive_finalizing -> archive_verified -> guardian_handle_bound
   \-> blocked / kept                    -> arm_authorized_unproved
                                         -> armed_unproved
                                         -> final_profile_proved_held
                                         -> close_authorized
                                         -> guardian_handle_closed -> parent_handle_closed -> deleted
                                           \-> receipt valid + exact guardian dead -> deleted
                                           \-> receipt/guardian unknown -> fail-closed pending
                                           \-> cancelled_safe / cancellation_pending_retained
```

If UAC is cancelled or archive finalization fails before the first delete, zero
source objects are deleted. Once deletion has begun, Stop means “stop after the
current object, or after every pathname in its inseparable hardlink/topology
group”; the engine never splits an atomic group. Already deleted objects are not
recreated in holding. Only an explicit owner Stop may produce a clean
`cancelled` result. If the internal progress mirror fails, execution still stops
at the same safe group boundary, but the durable result is `interrupted` with an
internal-failure reason; it is never relabelled as though the owner cancelled.
A crash is reconciled by FileId/proof and absence checks. Recovery never
automatically
continues irreversible deletes: it records `interrupted_partial`, reports the
already completed items, and requires a new preview and confirmation for the
remainder.

After a physical delete, the item, held-entry lifecycle, batch/operation state
and space effect are committed together. Recovery must also reconcile a terminal
entry found beside a non-terminal operation, so a crash cannot leave history
that contradicts the disk.

## Space accounting

No scalar called “freed bytes” may combine different volumes or lifecycle
stages. Every preview and result separates, per volume:

- bytes already released from the original/source volume during a verified
  cross-volume quarantine;
- current allocated bytes in the holding area;
- allocated bytes projected to be released by final removal;
- allocated bytes retained by recovery archives/backups; and
- optional free-space-before/free-space-after observations, explicitly labelled
  noisy because unrelated processes can write concurrently.

Logical length is not physical allocation. Named streams and topology groups are
included in an exact measurement; otherwise the value is labelled estimated or
unknown. The UI must not sum volumes into a single “disk freed” headline.

## Required interface

The primary surface groups persisted removal groups into:

- **Ready to clean** — every selected topology group already has an
  object-complete proof;
- **Needs archive verification** — one batch elevation can finish the proof;
- **Blocked** — stable object/subtree reason and remediation; and
- **Restorable/history** — archives and prior outcomes.

The review dialog names the projects, volumes, eligible-object count, blocked
subtrees, retained archives and whether a residual project directory will
remain. Its acknowledgement is exact, for example:

> Delete 412 eligible held objects from C: and D:. Keep 3 blocked objects and all
> recovery archives.

Cancel has initial focus and focus returns to the launcher. Progress is exposed
through an accessible progress bar and polite live-region announcements. After
deletion starts, the control becomes “Stop after current object/group” with an
explicit explanation of partial completion and the atomic topology-group boundary.

The minimum command surface is:

- `mutation_final_remove_preview(scope)`;
- `mutation_final_remove_confirm(preview_id, preview_digest, topology_groups)`;
- `mutation_final_remove_batch_start(request)`;
- `mutation_final_remove_batch_status(job_id)`;
- `mutation_final_remove_batch_stop(job_id)`; and
- `mutation_recovery_dashboard()` for complete server-side aggregates.

`mutation_activity_log()` remains history only. The legacy one-entry command name
may exist temporarily as a fail-closed compatibility boundary, but no action-only
grant can reach an irreversible executor. The primary UI and release evidence
exercise only the batch contract.

All three batch entry points above — preview, confirmation and start — must enforce
the default-OFF durable capability server-side. UI visibility is not the security
boundary. Enabling uses the exact phrase `ENABLE PERMANENT REMOVAL`; disabling takes
effect before helper resolution or confirmation-token consumption.

## Release gate

Permanent removal is not release-ready until all of the following are proved on
synthetic NTFS fixtures and then in the supervised clean-VM matrix:

- file and directory round-trip including ADS, EA, owner/group, DACL, SACL,
  mandatory label, supported attributes and timestamps;
- bottom-up project cleanup with mixed eligible and blocked subtrees;
- default-OFF state, wrong/missing activation phrase refusal, successful exact-phrase
  activation, and disable-after-confirmation refusal before batch start;
- same-volume and cross-volume quarantine/restore/final-cleanup journeys;
- hardlink and link-object topology handling without following external targets;
- identity swaps, locks, archive corruption, insufficient space, UAC cancel and
  helper/protocol replay refusal;
- fault injection at every archive, journal, promotion and disposition boundary;
- same-handle pre/post disposition races for hardlinks and named streams, plus
  cancel-failure followed by abrupt parent-process termination, guardian-only
  termination and simultaneous parent/guardian termination;
- close-frame replay/tamper/loss and crashes before/after the durable guardian
  receipt boundary; recovery must prove the keyed, flushed receipt plus exact
  guardian death (or use the authenticated `HandleClosed` fast path), and must
  never treat `close_authorized` alone as deletion authority;
- both extended and legacy Windows disposition modes (the development machine
  may support only the legacy fallback, which is not evidence for the extended
  branch);
- truthful per-volume storage effects and residual-project copy;
- one confirmation/one elevation for a large batch; and
- a signed production helper packaged in both Local and Connector editions,
  while the strict core build contains none of it.

Real UAC, signing, installer, clean-VM and publication gates remain owner-gated.
The deterministic kernel/protocol tests may prove parent-handle loss and local
guardian cancellation, but no automated unit test or unsigned helper substitutes
for those gates or for the machine-power-loss limitation above.
`SAFE-06` is therefore a supervised-manual gate: its typed attestation must bind
the exact release proof and separately record Extended NTFS, Legacy NTFS and
failed-cancel/delete-pending followed by abrupt-parent-termination results. A
local-CI claim or one supported disposition mode cannot seal it.
