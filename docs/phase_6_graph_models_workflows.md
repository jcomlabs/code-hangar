# Phase 6: graph, models and workflows

This phase turns the existing local inventory and `edge` table into a deeper
dependency map. It remains local-only and read-only in the core build.

## Implemented foundation

- `hangar-graph` contains pure, bounded model/workflow classification and JSON
  reference extraction.
- Likely workflow JSON is limited to explicit workflow paths and to 2 MiB per
  file.
- Sensitive files, Protected Zones, reparse points and cloud placeholders are
  never read by the workflow parser.
- Known model-reference fields are allowlisted. Arbitrary JSON strings are not
  promoted to graph edges merely because they occur in a JSON document.
- Video workflow fields such as motion, temporal, video, CLIP Vision and
  IP-Adapter models are included in the allowlist.
- Model references resolve with the confidence catalogue:
  - exact absolute or declared relative path: High;
  - unique filename or unique stem: Medium;
  - ambiguous filename: Low, with every candidate kept visible;
  - no match: a `missing_model_reference` issue.
- `project_graph_map` returns bounded project, workflow, model and cache nodes,
  model-use edges, issues, partial-inventory state and shared-project ownership.
- Model nodes may include bounded local header summaries:
  - GGUF reads only the fixed 24-byte header prefix for version, tensor count
    and metadata-entry count.
  - Safetensors reads the 8-byte header length plus the JSON header, capped at
    4 MiB, to summarize tensor count, dtype set and metadata-key count.
  - No model tensor payload/body bytes are read.
- Visible model nodes can report `duplicate_model_candidate` warnings when
  distinct physical model files share apparent size and the bounded first
  64 KiB hash. These are Medium-confidence candidates only; full hash
  confirmation remains explicit/on demand.
- Cache nodes are categorized as Hugging Face, Transformers, Ollama or generic
  local caches. Globally shared cache shapes are marked with
  `shared_cache_candidate` warnings so ownership remains conservative.
- Existing inventories are backfilled only when Connections is opened. Startup
  remains responsive; future scans rebuild the graph during scan finalization.
- The Connections workspace now exposes the Hangar Map and retains direct
  relationships for the open file.

## Safety boundaries

- No network calls, model downloads, remote metadata or documentation lookup.
- No scripts, plugin code, workflow execution or shell commands.
- No file content enters FTS because of graph parsing.
- Model bodies are not read. Classification uses path and extension metadata;
  GGUF/safetensors summaries inspect only bounded header bytes.
- Duplicate model candidates use a bounded partial hash only. Code Hangar does
  not compute full model hashes automatically while opening the map.
- Shared cache warnings are advisory. They do not mark bytes recoverable and do
  not authorize any cleanup action.
- Workflow parsing writes only derived local index edges/issues.
- Graph results are evidence, not permission for mutation or cleanup.

## Remaining Phase 6 work

The foundation is not the complete Phase 6 definition of done. Remaining work
includes richer project/asset/orphan/risk graph views, deeper GGUF and
safetensors metadata parsing, confirmed full-hash duplicate-model workflows,
even wider shared cache attribution across external tool registries,
workflow-format adapters beyond the generic JSON parser, and deeper
dangling-impact integration with OperationPlan/RiskReport.
