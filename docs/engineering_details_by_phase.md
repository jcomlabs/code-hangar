# Engineering Details by Phase

This document complements `CodeHangar_Master_Spec_v20_Final.md`.

The master specification remains the source of truth for product direction, phase order, safety policy and build invariants. This file preserves detailed algorithms, field shapes, phase-specific schemas, cleanup tiers, backup mechanics, error taxonomy, invariant tests and fixture additions that are not needed in the first sprint but must be available when later phases begin.

Nothing in this document is required for Phase -1, Phase 0 or Phase 1. Do not implement these later-phase details during the first coding pass. Use this document only when the implementation reaches the relevant phase.

If the master specification defines a stricter rule, the master specification wins. In particular, the compact `recursive_dir` OperationPlan remains mandatory for large operations. This document does not permit per-file enumeration of huge directories.

---

## Phase 1.5, detection and confidence

### Confidence catalogue (which signals map to which confidence)

Use this catalogue to assign confidence during reference resolution.

High confidence:

- exact absolute path reference;
- resolved symlink target;
- Git worktree metadata;
- tool config declaring a path;
- workflow referencing a model by absolute path;
- known local database row linking a session to a working directory.

Medium confidence:

- unique filename match inside known model folders;
- strong folder-layout match;
- context file inside project root;
- agent history mentioning a project path that still exists;
- workflow model name resolved uniquely.

Low confidence:

- filename found in multiple locations;
- proximity only;
- fuzzy project-name match;
- history mentions without a path;
- model name mentioned in a prompt but not in a workflow or config.

Unknown:

- file detected but no adapter can classify it;
- unsupported tool format;
- corrupted local metadata;
- inaccessible path.

Destructive warnings must never use categorical language for Medium or Low confidence associations.

### Owned, shared and orphaned-on-removal (exact definitions)

For asset node X, `referrers(X)` is the set of project nodes from which X is reachable along `depends_on`, `referenced_by`, `generated_by`, `stored_in` and `configured_in` edges.

```text
owned-by-P: referrers(X) == {P}
shared:     |referrers(X)| > 1
orphan:     referrers(X) == {} and X is a leaf asset kind, not a root

owned_nodes         = { X : referrers(X) == {P} and X within P } union P's own files
orphaned_on_removal = { X : referrers(X) \ {P} == {} and |referrers(X)| > 1 and X not protected }
recoverable_bytes   = physical_sum(owned_nodes union orphaned_on_removal)
```

`physical_sum` counts each inode group once across the union. Protected nodes are excluded from every recoverable and cleanup figure. The reported number is `recoverable_bytes`, never the raw connected footprint.

Duplicate confirmation: a duplicate group is confirmed only after a full blake3 hash matches all members. Full hashing runs only when it drives a destructive decision or the user explicitly confirms a group, with progress and cancellation. Until confirmed, duplicates are Medium confidence.

### Reference resolution algorithm

```text
For each reference R extracted from artifact A belonging to project P:

if R is an absolute path and the target exists:
    edge(A -> target, depends_on, High)
elif R is a tool-declared identifier the adapter can map:
    resolve via adapter mapping
    edge(A -> target, depends_on, High)
elif R is a bare filename:
    candidates = files named R within adapter.reference_search_scopes(A) union project_root(P)
    if len(candidates) == 1:
        edge(A -> candidate, depends_on, Medium)
    elif len(candidates) > 1:
        for c in candidates:
            edge(A -> c, depends_on, Low)
        flag A.attributes.ambiguous_references += R
    else:
        A.attributes.missing_references += R
else:
    edge(A -> nearest plausible node, stored_in or belongs_to, Low)
```

Missing references drive the "would be left pointing at nothing" warnings and the Risk Report dangling projection.

Markdown links use a separate local resolution path:

```text
if link is a local relative path and target exists inside allowed roots:
    edge(current_doc -> target_doc, markdown_links_to, High)
elif link is a heading anchor in same file:
    record local anchor
elif link is remote:
    record inert remote URL string, no fetch, no open
else:
    record unresolved local link, Medium or Low depending on path shape
```

Backlinks are the reverse projection of `markdown_links_to` edges.

### Full adapter example (declarative)

This is a complete ComfyUI-style adapter to build the first adapters against.

```json
{
  "name": "ComfyUI",
  "adapter_version": "1.0.0",
  "schema_version": 1,
  "type": "model_workflow_tool",
  "platforms": ["windows"],
  "verified_versions": ["portable-2024", "portable-2025", "portable-2026"],
  "default_roots": [
    "%USERPROFILE%/ComfyUI",
    "%USERPROFILE%/Documents/ComfyUI",
    "%USERPROFILE%/Desktop/ComfyUI",
    "%USERPROFILE%/Downloads/ComfyUI"
  ],
  "root_signals": [
    { "path": "main.py", "weight": 30 },
    { "path": "models", "weight": 30 },
    { "path": "custom_nodes", "weight": 20 },
    { "path": "user/default/workflows", "weight": 20 }
  ],
  "context_files": [
    "README.md",
    "custom_nodes/*/README.md",
    "user/default/workflows/*.json"
  ],
  "model_folders": {
    "checkpoints": "models/checkpoints",
    "loras": "models/loras",
    "vae": "models/vae",
    "controlnet": "models/controlnet",
    "upscale_models": "models/upscale_models",
    "clip": "models/clip",
    "clip_vision": "models/clip_vision",
    "diffusion_models": "models/diffusion_models",
    "text_encoders": "models/text_encoders",
    "embeddings": "models/embeddings"
  },
  "input_folders": ["input"],
  "output_folders": ["output"],
  "temp_folders": ["temp"],
  "cache_folders": ["__pycache__", ".cache"],
  "file_patterns": [
    { "glob": "**/*.safetensors", "kind": "local_model_file", "classification_from_folder": true },
    { "glob": "**/*.ckpt", "kind": "local_model_file", "classification_from_folder": true },
    { "glob": "**/*.pt", "kind": "local_model_file", "classification_from_folder": true },
    { "glob": "**/*.json", "kind": "workflow", "parser": "workflow_json_model_refs" }
  ],
  "parsers": [
    {
      "name": "workflow_json_model_refs",
      "applies_to": "workflow",
      "extract_fields": [
        "inputs.ckpt_name",
        "inputs.lora_name",
        "inputs.vae_name",
        "inputs.control_net_name",
        "inputs.model_name",
        "inputs.clip_name",
        "inputs.unet_name"
      ]
    },
    { "name": "markdown_outline", "applies_to": "markdown_file" },
    { "name": "markdown_links", "applies_to": "markdown_file" }
  ],
  "reference_search_scopes": [
    "models/checkpoints",
    "models/loras",
    "models/vae",
    "models/controlnet",
    "models/upscale_models",
    "models/clip",
    "models/clip_vision",
    "models/diffusion_models",
    "models/text_encoders",
    "models/embeddings"
  ],
  "confidence_rules": {
    "absolute_path_reference": "High",
    "declared_config_path": "High",
    "unique_filename_reference": "Medium",
    "ambiguous_filename_reference": "Low",
    "proximity_only": "Low"
  },
  "cleanup_rules": [
    { "glob": "temp/**", "risk": "Green", "reason": "temporary render data" },
    { "glob": "output/**", "risk": "Orange", "reason": "generated final or intermediate output" },
    { "glob": "models/**", "risk": "Red", "reason": "model assets may be shared" },
    { "glob": "custom_nodes/**", "risk": "Yellow", "reason": "rebuildable custom-node code, but may have local changes" }
  ],
  "sensitive_signatures": [".env", ".env.*", "*token*.json", "*credential*.json"],
  "protected_paths": [
    { "path": "models", "level": "no_mutation", "reason": "shared model folder by default" }
  ]
}
```

### Optional navigation/intelligence tables

Add these tables when the corresponding feature lands.

```sql
-- Phase 1.5: duplicate detection
CREATE TABLE duplicate_group (
  id INTEGER PRIMARY KEY,
  size INTEGER NOT NULL,
  hash_partial TEXT,
  hash_full TEXT,
  confirmed INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE duplicate_member (
  group_id INTEGER NOT NULL REFERENCES duplicate_group(id) ON DELETE CASCADE,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, node_id)
);

-- Later (history depth): agent session metadata.
-- Do not create a permanent full-text index for agent histories by default.
-- Deep history search remains on-demand and adapter-structured.
CREATE TABLE agent_session (
  node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  tool TEXT NOT NULL,
  started_at TEXT,
  cwd TEXT,
  project_id INTEGER REFERENCES node(id),
  metadata_json TEXT
);
```

---

## Phase 1.5 and Phase 6, model classification reference

Keep the full model file extension and category lists for model classification.

Model file extensions: `.safetensors`, `.ckpt`, `.pt`, `.pth`, `.bin`, `.gguf`, `.onnx`, `.engine`, `.plan`.

Model categories: checkpoint, LoRA, VAE, ControlNet, IP-Adapter, upscaler, embedding, text encoder, CLIP, T5 encoder, diffusion model, LTX model, Flux model, Wan model, GGUF LLM, Ollama model, LM Studio model, unknown large model asset. All classifications carry confidence.

---

## Phase 2, cleanup tiers and OperationPlan fields

### Cleanup risk tiers (which files belong to which tier)

Use these tier definitions when implementing cleanup planning.

Green, safe cleanup: temporary files; render temp folders; failed build outputs; `.pytest_cache`; `.mypy_cache`; `.next`; `dist`; `build`; `.turbo`; workflow temp folders; video temporary frames; logs.

Yellow, rebuildable: `node_modules`; `.venv`; `venv`; `__pycache__`; local build caches; package-manager caches; generated dependency folders.

Orange, project-specific and important: generated images; final videos; local datasets; SQLite databases; exported results; saved workflows; AI-generated patches; notes; prompts; project-specific model files.

Red, shared or dangerous: shared model files; LoRAs; VAEs; ControlNets; text encoders; Ollama model blobs; LM Studio models; Hugging Face cached models; memories; reusable skills; shared package stores; shared Python environments; files referenced by multiple projects.

Black, protected: SSH keys; provider keys; OAuth credentials; password stores; operating-system folders; user documents outside the selected project boundary; Protected Zones; files manually marked protected.

### OperationPlan field reference (reconciled with v20)

Reconciliation note: the compact `recursive_dir` plan is the correct design and supersedes per-item enumeration. Do not list every file for large directories. The schema below is the field shape the plan and Risk Report must carry; for a `recursive_dir` item, `items` holds the single compact entry plus the mandatory dry-run aggregate summary, not millions of rows.

```json
{
  "plan_id": "uuid",
  "schema": "operation_plan/1",
  "created_at": "ISO-8601",
  "target": { "node_id": 0, "kind": "project", "path": "..." },
  "action": "quarantine",
  "items": [
    {
      "node_id": 0, "path": "...", "size_apparent": 0, "physical_bytes": 0,
      "hardlink_group": "vol:inode", "frees_space": true,
      "action": "move", "risk": "Yellow", "confidence": "High"
    }
  ],
  "recoverable_bytes": { "owned": 0, "orphaned_on_removal": 0, "total": 0 },
  "shared_assets": [
    { "node_id": 0, "path": "...", "physical_bytes": 0,
      "referenced_by": [ { "node_id": 0, "kind": "workflow" } ], "confidence": "High" }
  ],
  "dangling_after": [ { "referrer_node_id": 0, "missing_path": "...", "confidence": "Medium" } ],
  "sensitive_files": [ { "path": "...", "signature": "dotenv" } ],
  "protected_hits": [ { "path": "...", "zone_id": 0, "level": "no_mutation" } ],
  "git_warnings": [ { "repo_id": 0, "uncommitted": true, "untracked": 3, "only_local_copy": true } ],
  "backup": { "required": false, "recommended_level": "standard", "destination_ok": true },
  "confidence_summary": { "high": 0, "medium": 0, "low": 0, "unknown": 0 },
  "recommended_action": "Create standard backup, quarantine temporary frames only.",
  "read_only_preview": true
}
```

The Risk Report should also carry an explicit "external services unaffected" line, which v20's field list dropped.

---

## Phase 3, backup, quarantine, state machine and journal

### Backup levels and verification

Use these details when implementing backup, quarantine and restore.

Minimal backup: project manifest, context files, Git metadata summary, dependency manifest, model reference list, workflow list, cleanup plan and Risk Report.

Standard backup: project source, context files, selected documentation, workflows, agent-history references, dependency manifest, disk-usage report, cleanup plan and restore manifest.

Full backup: project source, context files, selected generated outputs, selected AI histories, selected workflows, selected local model files, dependency manifest, disk-usage report and restore manifest.

Full backup of large assets requires an external or different-volume destination. The product refuses to create a large space-recovery backup on the same volume being cleaned unless the user explicitly overrides after warning. It checks destination free space before starting.

Every backup includes `codehangar-backup-manifest.json`, recording original path, backup path, backup date, included files, excluded files, excluded sensitive files, linked models, included models, omitted models, workflows, agent histories, Git status, uncommitted changes, disk snapshot, checksums, restore instructions and the cleanup plan that triggered the backup.

Backups are verified by recomputing blake3 checksums after write. A backup with `verified = 0` is never accepted as pre-deletion safety.

### Quarantine and restore mechanics

Quarantine moves files preserving relative original paths under the quarantine root, writes a per-entry manifest, and sets `space_recovered` truthfully. Same-volume is a rename and frees no space until permanent deletion; when the goal is to free space, actively suggest a different volume. Cross-volume is a journaled copy, verify, then delete of the source.

Restore is itself a journaled operation. It never overwrites. If the original path is occupied it surfaces the conflict and the user chooses new path, merge, skip or cancel. After restore, files are verified byte-identical to the quarantined copy.

### Destructive-action state machine

```text
DRAFT --build--> REVIEWED --user confirm--> CONFIRMED
CONFIRMED --backup requested--> BACKUP_RUNNING --verify--> BACKUP_VERIFIED
CONFIRMED or BACKUP_VERIFIED --validate plan--> EXECUTING --> VERIFYING --> DONE
any --error--> FAILED
FAILED or interrupted --recover--> ROLLED_BACK or resumed EXECUTING
```

Rules:

- Plan validity gate: before EXECUTING, re-fingerprint the target subtree and compare to `operation.target_fingerprint`. If changed, abort to DRAFT and require a rebuild.
- Journal first: on entering EXECUTING, all `operation_item` rows are written with intended `from_path`, `to_path` and `action`, status pending, before anything is touched.
- Per-item execution: same-volume move uses atomic rename. Cross-volume copies, fsyncs, verifies checksum, then deletes source. Source is not deleted until the copy is verified.
- Idempotence: each item is keyed. Re-running skips done items.
- Crash recovery: on launch, any operation in BACKUP_RUNNING, EXECUTING or VERIFYING is recovered. Done items are kept. Pending items are resumed if safe, otherwise the operation is rolled back by reversing completed moves from the journal.
- Verification: confirms items are gone from source and present in destination for quarantine, recomputes recovered bytes truthfully, and rescans affected nodes to populate the dangling projection.

### Journal and mutation table schemas

```sql
CREATE TABLE operation (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  target_node_id INTEGER REFERENCES node(id),
  target_fingerprint TEXT,
  backup_id INTEGER REFERENCES backup(id),
  recovered_bytes INTEGER,
  created_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  error TEXT
);

CREATE TABLE operation_item (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER NOT NULL REFERENCES operation(id) ON DELETE CASCADE,
  node_id INTEGER REFERENCES node(id),
  action TEXT NOT NULL,          -- move | copy_delete | delete | copy | noop
  from_path TEXT,
  to_path TEXT,
  bytes INTEGER,
  checksum_before TEXT,
  checksum_after TEXT,
  status TEXT NOT NULL           -- pending | done | failed | skipped | rolled_back
);
CREATE INDEX idx_opitem_op ON operation_item(operation_id, status);

CREATE TABLE backup (
  id INTEGER PRIMARY KEY,
  level TEXT NOT NULL,
  destination TEXT NOT NULL,
  manifest_path TEXT NOT NULL,
  total_bytes INTEGER,
  verified INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE quarantine_entry (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER REFERENCES operation(id),
  original_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  size INTEGER,
  file_count INTEGER,
  risk_level TEXT,
  backup_id INTEGER REFERENCES backup(id),
  space_recovered INTEGER NOT NULL DEFAULT 0,
  scheduled_delete_at TEXT,
  status TEXT NOT NULL,          -- quarantined | restored | permanently_deleted
  manifest_json TEXT NOT NULL
);
```

The mutation IPC commands are gated behind the `mutation` feature flag. A `confirm_token`, obtained by the UI only after showing the relevant warning, is required to enter mutation mode and to permanently delete, so a programmatic caller cannot skip the human confirmation step.

---

## Cross-cutting, error taxonomy and tests

### Error taxonomy

Typed errors with defined handling:

- PermissionDenied: skip, flag, continue during scan; abort mutation if target item cannot be accessed.
- SharingViolation: skip, flag, continue during scan; abort or retry for mutation.
- BrokenSymlink: record as dangling reference and continue.
- PathTooLong: mitigated by `\\?\`; otherwise skip and flag.
- CrossVolumeMoveFailed: abort operation, roll back via journal, surface clearly.
- ChecksumMismatch: abort operation, roll back via journal. A backup checksum mismatch voids the backup.
- InsufficientDestinationSpace: abort before writing.
- AdapterParseError: degrade to Unknown confidence, flag in Adapter Manager.
- CorruptMetadata: degrade to Unknown confidence, flag in Adapter Manager.
- PlanStale: abort to DRAFT, require rebuild.
- Cancelled: clean stop, partial scan retained as partial, no mutation.
- ProtectedZoneViolation: refuse mutation and surface zone policy.
- SensitiveFileBlocked: block preview or index and explain why.
- SensitiveFileRevealed: allowed only after explicit local user action; return content transiently to the UI; never persist in SQLite, FTS, preview cache or logs.
- StrongProtectedRevealBlocked: refuse reveal for strong Protected Zones such as `.ssh` and app/system zones.
- DatabaseLocked: retry with backoff; surface if persistent.
- EncryptionKeyUnavailable: refuse database access and offer recovery or purge path.

### Property and invariant tests

These complement v20's gates and acceptance criteria.

- `physical_sum(owned) + physical_sum(shared once) + physical_sum(orphan)` equals total physical bytes scanned within link accounting.
- `recoverable_bytes <= owned + orphaned_on_removal`.
- The OperationPlan produced by the plan builder equals the set executed in a sandbox.
- Protected Zones override every cleanup tier.
- Sensitive-file contents are never inserted into the database or logs.
- Sensitive reveal stays transient and does not populate `node.attributes`, `document_fts`, persistent caches or logs.
- Markdown preview never fetches remote resources.
- History indexing can be purged and rebuilt.

Plus: crash-recovery tests (kill at every item boundary, relaunch resumes or rolls back consistently), adapter golden tests (sample files in, expected references and confidence out), reversibility tests (quarantine then restore yields byte-identical files), and a no-network integration test (no outbound connection during full scan, preview, backup, quarantine and restore).

### Fixture additions

When the relevant phases arrive, extend `docs/fixtures.md` with: duplicate models as true copies, as hardlinks, and as symlinked or junction-linked folders; orphan models, caches and histories; a second local clone sharing the same origin URL; a ComfyUI-style tree with absolute, unique-filename, ambiguous-filename and at least one missing reference; Protected Zones at every level; a Python venv; node_modules; a Hugging Face-style cache; and agent history files mentioning project paths. Ship golden expectations alongside (orphan set, recoverable bytes per project, duplicate groups, confidence assignments, dangling references).
