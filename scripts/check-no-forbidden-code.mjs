import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const scanRoots = ["apps", "crates", "scripts"];

const deniedPatterns = [
  { pattern: /\bfetch\s*\(/, message: "browser fetch call" },
  { pattern: /\bWebSocket\s*\(/, message: "WebSocket client" },
  { pattern: /\bEventSource\s*\(/, message: "EventSource client" },
  { pattern: /\bTcpStream\b|\bUdpSocket\b/, message: "raw network socket" },
  { pattern: /\bplan_execute\b|\bbackup_run\b|\bquarantine_|\bpermanent_delete\b/, message: "mutation or execution command" },
  { pattern: /\btrusted_agent_|\bagent_(?:project_context|plan_|read_|activity_|scope|register|revoke|server)|\bmcp\b/i, message: "agent IPC/MCP surface" },
  { pattern: /\bgit\s+(fetch|pull|push|clone)\b/i, message: "remote Git operation" },
  { pattern: /\btauri_plugin_updater\b|\bplugin-updater\b/i, message: "auto-updater" },
  { pattern: /\btelemetry\b|\banalytics\b|\bsentry\b/i, message: "telemetry or analytics" }
];

const allowedFiles = new Set([
  path.normalize("scripts/check-no-outbound-deps.mjs"),
  path.normalize("scripts/check-no-forbidden-code.mjs")
]);

// Mutation/execution identifiers (quarantine_, backup_run, permanent_delete,
// plan_execute) are permitted only inside the dedicated, feature-gated mutation
// crate. They remain forbidden in every other crate/app so no executor can leak
// into the strict core lane.
const mutationCrateDir = path.normalize("crates/hangar-mutation");
const mutationApiFile = path.normalize("crates/hangar-api/src/lib.rs");

// Phase 5/6 agent + connected-AI-app surfaces are permitted only inside these
// dedicated, feature-gated crates/binaries, which the strict core lane and Local
// edition never link.
// The exception is deliberately narrow: it lifts ONLY the agent/IPC/connected-
// app naming ban. Outbound sockets, fetch/WebSocket clients, telemetry and the
// mutation/execution identifiers remain forbidden even here.
const agentSurfaceDirs = [
  "crates/hangar-agent", // Phase 5 local named-pipe automation
  "crates/hangar-mcp", // connected-AI-app JSON-RPC stdio runtime
  "crates/hangar-appconfig", // safe per-host config writer (registers the server)
  "apps/mcp-server", // the standalone connected-AI-app server binary
  // Connector-edition Tauri override: names the code-hangar-mcp sidecar. Used ONLY
  // by the connector build (`tauri build --config ...`); Local/core builds read only
  // tauri.conf.json, so this MCP reference never reaches the Local edition.
  "apps/desktop/src-tauri/tauri.connector.conf.json"
].map((dir) => path.normalize(dir));

// Read-only file-name knowledge base: it NAMES the steering files AI tools generate
// (incl. `.mcp.json`) to describe them in the UI. It ships in every edition and contains
// no MCP/agent code — only string literals naming files it may find on disk. This lifts
// ONLY the agent/IPC/MCP NAMING ban for these two files; fetch/socket/mutation/telemetry
// patterns are still enforced on them.
const aiToolKnowledgeFiles = [
  "apps/desktop/src/ai-tool-files.ts",
  "apps/desktop/src/__tests__/ai-tool-files.test.ts"
].map((file) => path.normalize(file));

function walk(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (["node_modules", "target", "dist", ".git"].includes(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (/\.(rs|ts|tsx|js|mjs|json|toml|html|css)$/.test(entry.name)) acc.push(full);
  }
  return acc;
}

// Strip line and block comments. Comments are never compiled, so a forbidden token in a comment
// can never reach the shipped base binary; scanning them only produces false positives (e.g. a
// doc-comment that explains why the connector surface is gated).
function stripComments(text) {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

// A cfg attribute that gates an item to a connector-only feature. Local/core builds never enable
// these features, so any item behind such a gate is compile-absent from those binaries —
// exactly what the agent/MCP surface ban is protecting. The shared Tauri entry point (main.rs)
// legitimately hosts these commands behind the gate; an UN-gated agent/MCP reference is still a
// real finding and is left intact for the scan to catch.
const CONNECTOR_GATE = /#\[\s*cfg\([^\]]*feature\s*=\s*"(?:agent_automation|mcp)"[^\]]*\)\s*\]/;

// Drop every item gated by a connector-only feature so the agent/MCP text scan mirrors what the
// base compiler emits. Balances () [] {} so multi-line fns and the gated `invoke_handler(
// generate_handler![ … ])` registration block are consumed in full.
function stripConnectorGatedItems(text) {
  const lines = text.split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    if (!CONNECTOR_GATE.test(lines[i])) {
      out.push(lines[i]);
      i += 1;
      continue;
    }
    i += 1; // drop the cfg gate line
    while (i < lines.length && /^\s*#\[/.test(lines[i])) i += 1; // drop further attributes
    let brace = 0;
    let group = 0;
    let openedBrace = false;
    let done = false;
    while (i < lines.length && !done) {
      for (const ch of lines[i]) {
        if (ch === "{") {
          brace += 1;
          openedBrace = true;
        } else if (ch === "}") {
          brace -= 1;
        } else if (ch === "(" || ch === "[") {
          group += 1;
        } else if (ch === ")" || ch === "]") {
          group -= 1;
        } else if (ch === ";" && !openedBrace && brace === 0 && group === 0) {
          done = true; // statement item (e.g. the gated invoke_handler call) ends here
        }
      }
      i += 1;
      if (openedBrace && brace === 0) done = true; // block item (fn) ends at its closing brace
    }
  }
  return out.join("\n");
}

function sourceForCheck(text, message) {
  if (message === "agent IPC/MCP surface") {
    return stripConnectorGatedItems(stripComments(text));
  }
  if (message !== "telemetry or analytics") return text;
  return text
    .replace(/\bno\s+(?:network\s+or\s+)?telemetry\b/gi, "")
    .replace(/\bwithout\s+telemetry\b/gi, "")
    .replace(/\bnever\s+sends?\s+telemetry\b/gi, "");
}

for (const rootName of scanRoots) {
  for (const file of walk(path.join(root, rootName))) {
    const rel = path.normalize(path.relative(root, file));
    if (allowedFiles.has(rel)) continue;
    const text = fs.readFileSync(file, "utf8");
    for (const { pattern, message } of deniedPatterns) {
      const matches = sourceForCheck(text, message).match(pattern);
      if (!matches) continue;
      if (
        message === "mutation or execution command"
        && (rel.startsWith(mutationCrateDir) || rel === mutationApiFile)
      ) continue;
      if (
        message === "agent IPC/MCP surface"
        && (agentSurfaceDirs.some((dir) => rel.startsWith(dir)) || aiToolKnowledgeFiles.includes(rel))
      ) continue;
      throw new Error(`${rel} contains forbidden ${message}: ${matches[0]}`);
    }
  }
}

console.log("No forbidden outbound, mutation, remote Git, updater, telemetry, or agent IPC code found.");
