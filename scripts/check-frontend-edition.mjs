import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const edition = (process.argv[2] || "").trim().toLowerCase();
const dist = path.resolve(root, "dist");
const tauriConfig = JSON.parse(fs.readFileSync(path.join(root, "src-tauri", "tauri.conf.json"), "utf8"));
const connectorConfig = JSON.parse(fs.readFileSync(path.join(root, "src-tauri", "tauri.connector.conf.json"), "utf8"));

if (!new Set(["local", "connector"]).has(edition)) {
  throw new Error(`Expected an explicit frontend edition argument: local or connector; received '${edition || "none"}'.`);
}
if (!fs.existsSync(dist)) {
  throw new Error(`Frontend bundle does not exist: ${dist}`);
}
if (tauriConfig.build?.beforeBuildCommand !== "npm run build:local") {
  throw new Error("The base Tauri config must select the Local frontend build explicitly.");
}
if (connectorConfig.build?.beforeBuildCommand !== "npm run build:connector") {
  throw new Error("The Connector Tauri override must select the Connector frontend build explicitly.");
}

function collectFiles(dir, files = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) collectFiles(full, files);
    else if (/\.(?:css|html|js)$/i.test(entry.name)) files.push(full);
  }
  return files;
}

const files = collectFiles(dist);
const relativeNames = files.map((file) => path.relative(dist, file).replaceAll("\\", "/"));
const bundleText = files.map((file) => fs.readFileSync(file, "utf8")).join("\n");
const maxJavaScriptChunkBytes = 500_000;
const oversizedJavaScriptChunks = files
  .filter((file) => file.endsWith(".js") && fs.statSync(file).size > maxJavaScriptChunkBytes)
  .map((file) => `${path.relative(dist, file).replaceAll("\\", "/")} (${fs.statSync(file).size} bytes)`);

if (oversizedJavaScriptChunks.length > 0) {
  throw new Error(`Frontend JavaScript chunks exceed 500 kB: ${oversizedJavaScriptChunks.join(", ")}.`);
}

const connectorChunkNames = [/AiAssist/i, /ProjectAiSummary/i, /RewriteDialog/i, /RecapAiLayer/i, /connectorApi/i];
const connectorMarkers = [
  "api.openai.com",
  "gpt-5.6",
  "api.anthropic.com",
  "openrouter.ai",
  "AI Assist",
  "Review AI summary request",
  "connected AI apps",
  "ai_provider_get",
  "ai_explain_file",
  "ai_review_file",
  "ai_send_disclosure",
  "ai_read_stream",
  "ai_local_discover",
  "ai_usage_status",
  "ai_usage_set_soft_cap",
  "ai_usage_reset",
  "ai_walkthrough_preview",
  "ai_walkthrough_file",
  "ai_follow_up_preview",
  "ai_follow_up",
  "ai_glossary_state",
  "ai_annotations_for_node",
  "ai_change_set_preview",
  "ai_narrate_session_changes",
  "ai_explain_change",
  "ai_review_change_set",
  "ai_rewrite_text",
  "apply_ai_suggestion",
  "ai_edit_sessions_for_node",
  "undo_ai_edit_session",
  "ai_summarize_project",
  "ai_summarize_project_preview",
  "ai_summarize_project_disclosure",
  "mcp_appconfig_register",
  "agent_request_resolve",
  "automation_register"
];

if (edition === "local") {
  const leakedChunks = relativeNames.filter((name) => connectorChunkNames.some((pattern) => pattern.test(name)));
  const leakedMarkers = connectorMarkers.filter((marker) => bundleText.includes(marker));
  if (leakedChunks.length > 0 || leakedMarkers.length > 0) {
    throw new Error(
      `Local frontend contains Connector artifacts. Chunks: ${leakedChunks.join(", ") || "none"}. ` +
      `Markers: ${leakedMarkers.join(", ") || "none"}.`
    );
  }
  console.log(`Local frontend isolation passed (${files.length} text assets; no Connector chunks, endpoints, or IPC commands).`);
} else {
  const missingChunks = connectorChunkNames.filter((pattern) => !relativeNames.some((name) => pattern.test(name)));
  const missingMarkers = connectorMarkers.filter((marker) => !bundleText.includes(marker));
  if (missingChunks.length > 0 || missingMarkers.length > 0) {
    throw new Error(
      `Connector frontend is incomplete. Missing chunks: ${missingChunks.map(String).join(", ") || "none"}. ` +
      `Missing markers: ${missingMarkers.join(", ") || "none"}.`
    );
  }
  console.log(`Connector frontend capability check passed (${files.length} text assets).`);
}
