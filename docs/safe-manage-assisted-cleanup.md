# Safe Manage: Assisted Cleanup Decision Workflow

Status: owner-approved product requirement, 2026-08-27.

## Product promise

Discover says what exists. Safe Manage helps the user decide what to do with it.

Safe Manage is not only an execution surface for already-chosen cleanup. It is
the decision layer between discovery and the existing backup-first mutation
system. Its first screen is a portfolio-level evidence summary, not a file tree
and not a delete queue.

No recommendation authorizes an action. Permanent removal remains a central,
fully supported outcome, but starts disabled, is never automatic and retains its
own backup, activation and per-operation confirmation gates.

## Entry points

The analysis can begin:

1. after initial project discovery on first run; or
2. at any time from Safe Manage, using the current catalog and computer state.

After first-run discovery, ask:

> I found your work. Would you like help deciding what still matters and what is
> only taking space and attention?

The choices are:

- **Analyze now**;
- **Continue to Code Hangar** and analyze later; and
- **Do not suggest this automatically again**.

The prompt is non-modal with respect to using the rest of the product. Analysis
is never required. The choice and the last completed analysis are persisted.

## Workflow and authority boundary

```text
Discover
  -> global analysis
  -> classification
  -> recommendations
  -> user review and decision
  -> OperationPlan / Risk Report
  -> explicit confirmation
  -> verified backup
  -> holding
  -> restore OR permanent removal
```

The first five stages are read-only. A recommendation cannot be converted into
an OperationPlan until the user makes an explicit project or eligible group
decision. OperationPlan remains the immutable source of truth for preview and
execution. The existing mutation, protection, recovery and final-removal
invariants remain authoritative.

## Fast objective analysis

The first pass is local, deterministic, cancellable and designed to complete
without reading arbitrary file bodies. It evaluates every discovered project and
records the evidence revision used for the result.

Signals include, where available:

- last observed project, file and session activity;
- file count, file kinds, apparent and physical size and confidence;
- associated AI coding applications and sessions;
- Git repository presence, working-tree status and uncommitted changes;
- existence of a configured remote as passive metadata only;
- regenerable builds, caches and dependency folders;
- duplicate or materially similar project versions;
- references, shared assets and relationships with other projects;
- Protected Zones, sensitive content, links, placeholders and external paths;
- evidence of substantial work versus residual or generated-only content; and
- missing, stale, partial, corrupt or contradictory evidence.

The Local edition performs this pass entirely locally and without AI. Signals
whose cost is not bounded are reported as unavailable or deferred rather than
silently approximated.

## Classifications and recommendations

The summary leads with understandable portfolio counts, for example:

> 87 projects analyzed — 12 active, 31 dormant, 18 archive candidates, 9 cleanup
> candidates and 17 needing review.

Lifecycle classifications describe observed state and are kept distinct from
recommended decisions. The minimum lifecycle vocabulary is:

- **Active**;
- **Dormant**;
- **Archive candidate**;
- **Cleanup candidate**; and
- **Needs review**.

The recommendation vocabulary is:

- **Keep**;
- **Review**;
- **Archive**;
- **Clean regenerable files**;
- **Removal candidate**; and
- **Do not touch**.

Every recommendation carries:

- a stable project identity and analysis-run identity;
- a concise reason;
- confidence and confidence limitations;
- the exact signals that produced it;
- last activity and its source;
- associated applications and session counts;
- Git state;
- space estimates and accounting confidence;
- potentially important files or context markers;
- dependencies, shared assets and relationships that increase risk;
- stale, partial or unavailable evidence; and
- the ruleset version used to classify it.

Unknown facts never contribute as negative evidence. High-risk or contradictory
signals bias toward **Review** or **Do not touch**, never toward removal.

## User decisions

For one project, or a group whose eligibility and evidence are homogeneous, the
user may:

- keep;
- ignore for the current analysis;
- request deeper analysis;
- archive;
- clean only regenerable material; or
- prepare for removal.

Group actions show the exact included projects and exclude ineligible or
ambiguous items. A decision records who made it, when, against which analysis
revision and whether any evidence has since become stale. A changed project must
be revalidated before its decision can enter OperationPlan.

The file tree remains available for investigation, but is not the primary
decision interface.

## Optional Connector analysis

The Connector edition adds an optional second recommendation layer. It does not
replace or relax the deterministic classifier: the local baseline and its
objective confidence remain visible and first-class. For any current project
except a deterministic **Do not touch** safety result, the user may explicitly
ask AI to enrich that recommendation. The AI may agree with the baseline or
return a materially different typed recommendation and its own, separately
labelled confidence. This includes high-confidence deterministic results; the
Connector is not limited to ambiguous projects.

Candidate context may include README files, manifests, selected central files
and selected session excerpts. Nothing is preselected. The frontend receives
only random, short-lived selectors and fixed safe labels; it never supplies a
file/session path or a content body back to the backend. Unknown or omitted
context is never treated as negative evidence.

Before any external-model request:

1. the backend resolves and filters the selected context;
2. secrets, sensitive files and Protected Zones are blocked;
3. absolute project/file/session paths are removed from selected excerpts;
4. Code Hangar constructs a frozen exact outbound payload;
5. the user inspects that payload and destination; and
6. the user explicitly sends it.

A local model follows the same selection and evidence rules, although it does
not create an external-network event. The provider must return one exact
recommendation token and one confidence token. Rust parses those fields
strictly; malformed or prose-only output remains readable but cannot be inferred
into an action. The response body is not stored as objective evidence. Instead,
Code Hangar persists a receipt with the request and result fingerprints,
character counts, typed opaque source references, excerpt fingerprints and
redaction counts. The receipt cannot represent provider keys, request/result
bodies, provider/model configuration, display names or source paths.

The AI result can change the recommendation displayed in Connector for human
review. It does not record the user's decision. Only a later explicit user
decision can enter OperationPlan / Risk Report, and those are rebuilt from
current deterministic evidence. **Do not touch**, Protected Zones, sensitive or
shared targets, stale evidence, revalidation, backup, holding, restore and final
removal gates remain authoritative regardless of the AI result. AI can recommend
and explain; it cannot approve, schedule or execute archive, cleanup, holding,
restore or permanent removal.

The Local edition contains no UI, copy, command, module, resource or dependency
related to this optional layer.

## Persistence and jobs

The implementation must persist, at minimum:

- analysis run state, timestamps, ruleset version and catalog/evidence revision;
- per-project objective signals and their provenance;
- lifecycle classification, recommendation, confidence and reason codes;
- user decisions and evidence revision at decision time;
- first-run prompt preference; and
- deeper-analysis request/result receipts in Connector only, containing hashes,
  counts, status and typed opaque source provenance but no provider keys,
  request/result bodies, model/endpoints, display names or source paths.

Analysis runs expose queued/running/cancelling/completed/partial/failed states,
progress counts and recoverable errors. A cancelled or crashed run remains
truthfully partial and never replaces the last complete result.

## Safety invariants

- Analysis and recommendation are read-only.
- No automatic or AI-triggered mutation exists.
- The deterministic baseline remains visible beside any AI recommendation.
- A deterministic **Do not touch** result cannot be overridden by AI.
- No recommendation silently becomes an OperationPlan.
- OperationPlan and Risk Report are rebuilt from current evidence after the user
  decision and are fingerprint-bound to execution.
- Protected, sensitive, shared, linked, ambiguous or changed targets fail closed.
- Backup verification precedes holding.
- Holding remains restorable across restart and interruption.
- Permanent removal starts off and requires activation, a valid backup, an exact
  target review and a fresh confirmation for each operation.
- Partial execution or uncertain recovery is never shown as success.

## Acceptance coverage

Release evidence must cover:

- first-run analyze, postpone and never-suggest choices;
- repeat analysis from Safe Manage;
- large portfolios, cancellation, restart and partial/corrupt evidence;
- deterministic classifications and confidence limitations;
- stale-decision invalidation after project change;
- safe eligible and mixed/ineligible group actions;
- the full recommendation-to-holding-to-restore journey;
- the full explicitly enabled permanent-removal journey;
- Local artifact isolation, including absence of Connector copy; and
- Connector selected-context preview, backend redaction, secure credentials,
  explicit send, valid/malformed typed recommendations, baseline agreement and
  disagreement, and proof that AI cannot cross the mutation authority boundary.
