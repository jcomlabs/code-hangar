import fs from "node:fs";
import path from "node:path";

const invocationRoot = process.cwd();
const desktopRoot = fs.existsSync(path.join(invocationRoot, "src-tauri"))
  ? invocationRoot
  : path.join(invocationRoot, "apps", "desktop");
const repoRoot = path.resolve(desktopRoot, "..", "..");
const dist = path.join(desktopRoot, "dist");
const argumentsList = process.argv.slice(2);
const selfTestOnly = argumentsList.length === 1 && argumentsList[0] === "--self-test";
const edition = selfTestOnly ? "" : (argumentsList[0] || "").trim().toLowerCase();

if (!selfTestOnly && (argumentsList.length !== 1 || !new Set(["local", "connector"]).has(edition))) {
  throw new Error(`Expected exactly one frontend edition argument: local or connector; received '${argumentsList.join(" ") || "none"}'.`);
}

const policyPath = path.join(repoRoot, "scripts", "edition-network-policy.json");
const editionPolicy = JSON.parse(fs.readFileSync(policyPath, "utf8"));
const expectedFrontendOnlyFiles = [
  "apps/desktop/src/connectorTypes.ts",
  "apps/desktop/src/connectorApi.ts",
  "apps/desktop/src/aiTasks.ts",
  "apps/desktop/src/views/AiAssist.tsx",
  "apps/desktop/src/views/AiLearningTools.tsx",
  "apps/desktop/src/views/AiUsageMeter.tsx",
  "apps/desktop/src/views/ProjectAiSummary.tsx",
  "apps/desktop/src/views/ConnectorEditionLayer.tsx",
  "apps/desktop/src/views/ConnectorEditionLayer.css",
  "apps/desktop/src/views/ConnectorCommentPresentation.tsx",
  "apps/desktop/src/views/ConnectorSettingsViews.tsx",
  "apps/desktop/src/views/RecapAiLayer.tsx",
  "apps/desktop/src/views/RewriteDialog.tsx",
  "apps/desktop/src/views/ai-learning.css",
  "apps/desktop/src/views/recap-ai.css",
  "apps/desktop/src/__tests__/ai-learning-tools.test.ts",
  "apps/desktop/src/__tests__/ai-review-lens.test.ts",
  "apps/desktop/src/__tests__/ai-tasks-stream.test.ts",
  "apps/desktop/src/__tests__/ai-usage-meter.test.ts",
  "apps/desktop/src/__tests__/connected-app-security-contract.test.ts",
  "apps/desktop/src/__tests__/local-ai-ux.test.ts"
];
if (
  editionPolicy.schemaVersion !== 1
  || editionPolicy.documentType !== "codehangar/edition-network-policy/1"
  || editionPolicy.local?.feature !== "mutation"
  || editionPolicy.connector?.feature !== "agent_automation"
  || editionPolicy.connector?.outboundCrate?.name !== "hangar-ai"
  || JSON.stringify(editionPolicy.connector?.frontendOnlyFiles) !== JSON.stringify(expectedFrontendOnlyFiles)
) {
  throw new Error("The frontend edition policy drifted from the closed Local/Connector boundary.");
}
const canonicalConnectorIpcCommands = editionPolicy.connector?.frontendIpcCommands ?? [];
if (
  canonicalConnectorIpcCommands.length === 0
  || new Set(canonicalConnectorIpcCommands).size !== canonicalConnectorIpcCommands.length
  || canonicalConnectorIpcCommands.some((name) => !/^(?:ai_[A-Za-z0-9_]+|set_ai_[A-Za-z0-9_]+|apply_ai_suggestion|undo_ai_edit_session)$/.test(name))
) {
  throw new Error("The frontend edition policy has no valid, unique canonical Connector AI IPC inventory.");
}
const missingPolicySources = expectedFrontendOnlyFiles.filter((relative) => !fs.existsSync(path.join(repoRoot, relative)));
if (missingPolicySources.length > 0) {
  throw new Error(`Connector-only frontend policy names missing sources: ${missingPolicySources.join(", ")}.`);
}

const connectorFrontendSourceContracts = [
  {
    file: "apps/desktop/src/App.tsx",
    markers: ["connectorFrontendBuild", 'import("./views/ConnectorEditionLayer")']
  },
  {
    file: "apps/desktop/src/views/ProjectHomeViews.tsx",
    markers: ["connectorFrontendBuild", 'import("./ProjectAiSummary")']
  },
  {
    file: "apps/desktop/src/views/CommentsPanel.tsx",
    markers: ["connectorFrontendBuild", 'import("./ConnectorCommentPresentation")']
  },
  {
    file: "apps/desktop/src/views/ConnectorEditionLayer.tsx",
    markers: [
      'from "../connectorApi"', 'from "./AiAssist"', 'from "./RecapAiLayer"',
      'from "./RewriteDialog"', 'from "./ConnectorSettingsViews"',
      'import "./ConnectorEditionLayer.css"'
    ]
  },
  {
    file: "apps/desktop/src/views/ConnectorCommentPresentation.tsx",
    markers: ["Connected AI apps", "connector-comment-badge"]
  },
  {
    file: "apps/desktop/src/connectorTypes.ts",
    markers: ["interface AiProviderConfig", "interface AutomationAgentSummary", '"mcp_stdio"']
  },
  {
    file: "apps/desktop/src/views/ConnectorSettingsViews.tsx",
    markers: ['from "../connectorApi"', "SettingsAutomationView", "SettingsConnectedAppsView"]
  },
  {
    file: "apps/desktop/src/views/AiAssist.tsx",
    markers: ['from "../aiTasks"', 'from "./AiLearningTools"', 'from "./AiUsageMeter"']
  },
  {
    file: "apps/desktop/src/views/ProjectAiSummary.tsx",
    markers: ['from "../aiTasks"', 'from "../connectorApi"', 'from "./AiUsageMeter"']
  },
  {
    file: "apps/desktop/src/views/AiLearningTools.tsx",
    markers: ['from "../connectorApi"', 'from "./AiUsageMeter"', 'import "./ai-learning.css"']
  },
  {
    file: "apps/desktop/src/views/RecapAiLayer.tsx",
    markers: ['from "../connectorApi"', 'from "./AiUsageMeter"', 'import "./recap-ai.css"']
  },
  {
    file: "apps/desktop/src/views/RewriteDialog.tsx",
    markers: ['from "./AiUsageMeter"']
  }
];
const connectorFrontendSources = new Map(connectorFrontendSourceContracts.map(({ file }) => [
  file,
  fs.readFileSync(path.join(repoRoot, file), "utf8")
]));

function assertConnectorFrontendSourceGraph(sources) {
  for (const contract of connectorFrontendSourceContracts) {
    const source = sources.get(contract.file) ?? "";
    const missing = contract.markers.filter((marker) => !source.includes(marker));
    if (missing.length > 0) {
      throw new Error(`Connector frontend source graph is missing ${contract.file}: ${missing.join(", ")}.`);
    }
  }
}

assertConnectorFrontendSourceGraph(connectorFrontendSources);

const sharedTypeSource = fs.readFileSync(path.join(desktopRoot, "src", "types.ts"), "utf8");
const sharedStyleSource = fs.readFileSync(path.join(desktopRoot, "src", "styles.css"), "utf8");
const connectorStyleSource = fs.readFileSync(path.join(desktopRoot, "src", "views", "ConnectorEditionLayer.css"), "utf8");
const sharedCommentSource = [
  fs.readFileSync(path.join(desktopRoot, "src", "comments.ts"), "utf8"),
  fs.readFileSync(path.join(desktopRoot, "src", "views", "CommentsPanel.tsx"), "utf8")
].join("\n");
const sharedProjectCopySource = [
  fs.readFileSync(path.join(desktopRoot, "src", "views", "ProjectHomeViews.tsx"), "utf8"),
  fs.readFileSync(path.join(desktopRoot, "src", "views", "OverviewView.tsx"), "utf8")
].join("\n");
const connectorOnlyCssSelectors = [
  "brand-edition--connector",
  "automation-settings",
  "automation-choice-list",
  "project-scope-list",
  "credential-token",
  "automation-agent-row",
  "automation-grant-row",
  "request-approve",
  "automation-activity-list",
  "connector-comments-hint",
  "connector-comment-badge"
];

function assertSharedSourceIsolation({ types, styles, connectorStyles, comments, projectCopy }) {
  const connectorTypeDeclaration = /^export\s+(?:interface|type)\s+(?:Ai[A-Z]\w*|Automation[A-Z]\w*|ConnectedApp\w*|AgentActionRequest|ResolveInputs|CodeAnnotation)\b/m;
  if (connectorTypeDeclaration.test(types) || /\bmcp(?:\b|_)/i.test(types)) {
    throw new Error("Shared types.ts contains a Connector-only wire contract or MCP token.");
  }
  for (const selector of connectorOnlyCssSelectors) {
    if (styles.includes(selector)) {
      throw new Error(`Shared styles.css contains Connector-only selector ${selector}.`);
    }
    if (!connectorStyles.includes(selector)) {
      throw new Error(`ConnectorEditionLayer.css is missing isolated selector ${selector}.`);
    }
  }
  if (/connected AI apps|AI-written|comment-agent-badge|never sent anywhere/i.test(comments)) {
    throw new Error("Shared comments source contains Connector capability or negative-network copy.");
  }
  if (/No AI, no network|contact the network/i.test(projectCopy)) {
    throw new Error("Shared Local help contains negative Connector/network teaser copy.");
  }
}

assertSharedSourceIsolation({
  types: sharedTypeSource,
  styles: sharedStyleSource,
  connectorStyles: connectorStyleSource,
  comments: sharedCommentSource,
  projectCopy: sharedProjectCopySource
});

function assertConnectorApiCommandInventory(source) {
  const literalPattern = /["']((?:ai_[A-Za-z0-9_]+|set_ai_[A-Za-z0-9_]+|apply_ai_suggestion|undo_ai_edit_session))["']/g;
  const literals = [...source.matchAll(literalPattern)].map((match) => match[1]);
  const counts = new Map();
  for (const name of literals) counts.set(name, (counts.get(name) ?? 0) + 1);
  const missingOrRepeated = canonicalConnectorIpcCommands.filter((name) => counts.get(name) !== 1);
  const unexpected = [...counts.keys()].filter((name) => !canonicalConnectorIpcCommands.includes(name));
  if (missingOrRepeated.length > 0 || unexpected.length > 0) {
    throw new Error(
      `connectorApi.ts drifted from the canonical one-wrapper-per-command IPC policy. `
      + `Missing/repeated: ${missingOrRepeated.join(", ") || "none"}; unexpected: ${unexpected.join(", ") || "none"}.`
    );
  }
}

const connectorApiSource = fs.readFileSync(path.join(repoRoot, "apps", "desktop", "src", "connectorApi.ts"), "utf8");
assertConnectorApiCommandInventory(connectorApiSource);

const tauriConfig = JSON.parse(fs.readFileSync(path.join(desktopRoot, "src-tauri", "tauri.conf.json"), "utf8"));
const connectorConfig = JSON.parse(fs.readFileSync(path.join(desktopRoot, "src-tauri", "tauri.connector.conf.json"), "utf8"));
const localReleaseConfig = JSON.parse(fs.readFileSync(path.join(desktopRoot, "src-tauri", "tauri.release-local.conf.json"), "utf8"));
const connectorReleaseConfig = JSON.parse(fs.readFileSync(path.join(desktopRoot, "src-tauri", "tauri.release-connector.conf.json"), "utf8"));

function assertLocalNativeInstallerIsolation({ installerHooks, hookSource }) {
  if (installerHooks !== "./windows/shell-integration.nsh") {
    throw new Error("The Local Tauri config must use the reviewed neutral shell-integration hook.");
  }
  if (/\b(?:Connector|AI Assist|MCP|provider|API key|agent[_ ]automation|hangar-(?:agent|ai)|code-hangar-mcp)\b/i.test(hookSource)) {
    throw new Error("The Local native installer hook contains a Connector/AI capability, name or path.");
  }
}

const localInstallerHooks = tauriConfig.bundle?.windows?.nsis?.installerHooks ?? "";
const localInstallerHookSource = fs.readFileSync(
  path.join(desktopRoot, "src-tauri", localInstallerHooks.replace(/^\.\//, "")),
  "utf8"
);
assertLocalNativeInstallerIsolation({
  installerHooks: localInstallerHooks,
  hookSource: localInstallerHookSource
});

function assertReleaseOverlayResourceClosure(config, selectedEdition) {
  const expectedBins = selectedEdition === "Local"
    ? ["binaries/code-hangar-elevated"]
    : ["binaries/code-hangar-elevated", "binaries/code-hangar-mcp"];
  const resourceEntries = Object.entries(config.bundle?.resources ?? {});
  if (
    JSON.stringify(config.bundle?.externalBin) !== JSON.stringify(expectedBins)
    || resourceEntries.length !== 1
    || resourceEntries[0][0] !== "binaries/code-hangar-release-manifest.json"
    || resourceEntries[0][1] !== "code-hangar-release-manifest.json"
  ) {
    throw new Error(`${selectedEdition} release overlay contains an unreviewed sidecar or installed resource.`);
  }
}

assertReleaseOverlayResourceClosure(localReleaseConfig, "Local");
assertReleaseOverlayResourceClosure(connectorReleaseConfig, "Connector");
if (tauriConfig.build?.beforeBuildCommand !== "npm run build:local") {
  throw new Error("The base Tauri config must select the Local frontend build explicitly.");
}
if (connectorConfig.build?.beforeBuildCommand !== "npm run build:connector") {
  throw new Error("The Connector Tauri override must select the Connector frontend build explicitly.");
}
const localDescription = tauriConfig.bundle?.longDescription ?? "";
const connectorDescription = connectorConfig.bundle?.longDescription ?? "";
if (
  /\b(?:MCP|AI Assist|AI Connector|provider|API key|network|telemetry|FOMO)\b/i.test(localDescription)
) {
  throw new Error("Local installer metadata contains Connector/provider capability or negative teaser copy.");
}
for (const marker of [
  "separate local MCP stdio process",
  "AI Assist starts Off",
  "explicit, reviewed request",
  "user-selected loopback or HTTPS provider",
  "feature-gated hangar-ai client",
  "no telemetry, updater, remote Git or implicit/background network"
]) {
  if (!connectorDescription.includes(marker)) {
    throw new Error(`Connector installer metadata is missing its truthful edition marker: ${marker}.`);
  }
}
if (
  JSON.stringify(connectorConfig.bundle?.externalBin) !== JSON.stringify(["binaries/code-hangar-mcp"])
  || /\bno (?:model-provider|API-key|HTTP|outbound-network) capability\b/i.test(connectorDescription)
) {
  throw new Error("Connector installer metadata or sidecar inventory contradicts the reviewed opt-in provider boundary.");
}

const connectorChunkNames = [
  /connectorApi/i,
  /ConnectorEditionLayer/i,
  /ConnectorCommentPresentation/i,
  /ConnectorGuidedTour/i,
  /ProjectAiSummary/i,
];
// `connectorApi` may be folded into the lazy ConnectorEditionLayer chunk by
// Rolldown. Its source edge and exact IPC inventory are proved above, and its
// command markers are proved in the emitted bytes below; a filename is not a
// stable security boundary. The three UI roots are explicit lazy chunks and
// therefore remain required by name.
const connectorRequiredChunkNames = connectorChunkNames.slice(1);
const connectorMarkers = [
  "connected AI apps",
  "mcp_appconfig_register",
  "mcp_appconfig_remove",
  "agent_request_resolve",
  "automation_register"
];
const outboundProviderMarkers = [
  "api.openai.com",
  "gpt-5.6",
  "api.anthropic.com",
  "openrouter.ai",
  "AI Assist",
  "Review AI summary request",
  ...canonicalConnectorIpcCommands
];
const connectorRequiredProviderMarkers = ["AI Assist", ...canonicalConnectorIpcCommands];
const connectorCopyPatterns = [
  ["Connector-edition label", /\bAI Connector\b/i],
  ["provider capability copy", /\b(?:AI|API) provider\b|\bprovider (?:request|endpoint|model)\b/i],
  ["AI explanation capability copy", /\bAI explanation\b|\bAI sending\b|\bExplain this\b|\bWhat to check\b/i],
  ["local automation capability copy", /\blocal automation\b|\blocal endpoint\b|\bone-time password\b/i],
  ["model-inference capability copy", /\blocal model server\b|\blocal model\b.{0,60}\binference\b|\b(?:run|send to|choose|configure) (?:a |your )?local model\b/i],
  ["API-key copy", /\bAPI key\b/i],
  // `.mcp.json`, `MCP (shared)` and plural `MCP servers` are legitimate
  // local inventory classifications. Capability/promotional copy is not.
  ["MCP capability copy", /\bConnect via MCP\b|\bMCP server\b(?!s\b)|\bMCP (?:connection|connector|integration|registration|sidecar|process)\b/i],
  ["FOMO copy", /\bFOMO\b/i]
];
// FOMO language is forbidden from Local, but Connector need not contain it.
// The positive Connector contract requires capability disclosure, not a specific
// marketing phrase that the product is better off omitting.
const connectorRequiredCopyPatterns = connectorCopyPatterns.filter(([label]) => label !== "FOMO copy");
const connectorCssMarkers = [
  "ai-provider-card",
  "ai-send-disclosure",
  "ai-explain-panel",
  "ai-usage-meter",
  "modal.rewrite-dialog",
  "ai-learning-tools",
  "recap-ai",
  ...connectorOnlyCssSelectors
];
const alwaysForbiddenBundlePatterns = [
  ["browser fetch", /(?:\bfetch\s*\(|\[\s*["']fetch["']\s*\]\s*\()/],
  ["browser XHR", /\bXMLHttpRequest\b|\[\s*["']XMLHttpRequest["']\s*\]/],
  ["browser beacon", /\bsendBeacon\b|\[\s*["']sendBeacon["']\s*\]/],
  ["browser socket/stream", /\b(?:WebSocket|EventSource)\s*\(/],
  ["Tauri HTTP plugin", /@tauri-apps\/plugin-http|tauri_plugin_http/i],
  ["updater", /@tauri-apps\/plugin-updater|tauri_plugin_updater|plugin-updater/i],
  ["telemetry", /\b(?:telemetry|analytics|sentry)\b/i],
  ["remote Git", /\bgit(?:\.exe)?\s+(?:fetch|pull|push|clone)\b/i]
];

function matchingNames(relativeNames, patterns) {
  return relativeNames.filter((name) => patterns.some((pattern) => pattern.test(name)));
}

function assertBundleEdition(selectedEdition, relativeNames, bundleText) {
  // Exact negative security copy is not a telemetry implementation. Strip only
  // the two reviewed sentences used by the resource-profile UI before applying
  // the token-level runtime deny; any other telemetry spelling still fails.
  const runtimePolicyText = bundleText
    .replace(/\bThis never sends telemetry\b/gi, "")
    .replace(/\bno telemetry leaves this machine\b/gi, "");
  const forbiddenRuntime = alwaysForbiddenBundlePatterns
    .filter(([, pattern]) => pattern.test(runtimePolicyText))
    .map(([label]) => label);
  if (forbiddenRuntime.length > 0) {
    throw new Error(`${selectedEdition} frontend contains forbidden webview runtime primitives: ${forbiddenRuntime.join(", ")}.`);
  }

  const presentChunks = matchingNames(relativeNames, connectorChunkNames);
  const presentConnectorMarkers = connectorMarkers.filter((marker) => bundleText.includes(marker));
  const presentProviderMarkers = outboundProviderMarkers.filter((marker) => bundleText.includes(marker));
  const presentConnectorCopy = connectorCopyPatterns.filter(([, pattern]) => pattern.test(bundleText)).map(([label]) => label);
  const presentCssMarkers = connectorCssMarkers.filter((marker) => bundleText.includes(marker));

  if (selectedEdition === "local") {
    if (
      presentChunks.length > 0
      || presentConnectorMarkers.length > 0
      || presentProviderMarkers.length > 0
      || presentConnectorCopy.length > 0
      || presentCssMarkers.length > 0
    ) {
      throw new Error(
        `Local frontend contains Connector/AI artifacts. Chunks: ${presentChunks.join(", ") || "none"}; `
        + `IPC: ${presentConnectorMarkers.join(", ") || "none"}; provider: ${presentProviderMarkers.join(", ") || "none"}; `
        + `copy: ${presentConnectorCopy.join(", ") || "none"}; CSS: ${presentCssMarkers.join(", ") || "none"}.`
      );
    }
    return;
  }

  const missingChunks = connectorRequiredChunkNames.filter((pattern) => !relativeNames.some((name) => pattern.test(name)));
  const missingConnectorMarkers = connectorMarkers.filter((marker) => !bundleText.includes(marker));
  const missingProviderMarkers = connectorRequiredProviderMarkers.filter((marker) => !bundleText.includes(marker));
  const missingCopy = connectorRequiredCopyPatterns.filter(([, pattern]) => !pattern.test(bundleText)).map(([label]) => label);
  const missingCss = connectorCssMarkers.filter((marker) => !bundleText.includes(marker));
  if (
    missingChunks.length > 0
    || missingConnectorMarkers.length > 0
    || missingProviderMarkers.length > 0
    || missingCopy.length > 0
    || missingCss.length > 0
  ) {
    throw new Error(
      `Connector frontend is incomplete. Missing chunks: ${missingChunks.map(String).join(", ") || "none"}; `
      + `IPC: ${missingConnectorMarkers.join(", ") || "none"}; provider: ${missingProviderMarkers.join(", ") || "none"}; `
      + `copy: ${missingCopy.join(", ") || "none"}; CSS: ${missingCss.join(", ") || "none"}.`
    );
  }
}

function mustReject(label, action) {
  let rejected = false;
  try {
    action();
  } catch {
    rejected = true;
  }
  if (!rejected) throw new Error(`Frontend edition negative fixture was accepted: ${label}.`);
}

const syntheticConnectorNames = connectorChunkNames.map((pattern, index) => {
  const stem = [
    "connectorApi",
    "ConnectorEditionLayer",
    "ConnectorCommentPresentation",
    "ConnectorGuidedTour",
    "ProjectAiSummary"
  ][index];
  return `assets/${stem}-fixture.js`;
});
const syntheticConnectorText = [
  ...connectorMarkers,
  ...outboundProviderMarkers,
  "AI Connector",
  "AI provider",
  "AI explanation",
  "Local automation uses a local endpoint and a one-time password",
  "local model server",
  "API key",
  "MCP process",
  "FOMO",
  ...connectorCssMarkers
].join("\n");
assertBundleEdition("local", ["assets/index-fixture.js"], "offline fixture");
assertBundleEdition(
  "local",
  ["assets/index-fixture.js"],
  "This never sends telemetry. no telemetry leaves this machine."
);
assertBundleEdition("connector", syntheticConnectorNames, syntheticConnectorText);
mustReject("one Connector chunk in Local", () => assertBundleEdition("local", ["assets/ConnectorEditionLayer-leak.js"], "offline fixture"));
mustReject("copied Connector bytes in Local", () => assertBundleEdition("local", ["assets/index.js"], "AI provider API key MCP"));
mustReject("Connector type declaration in shared types", () => assertSharedSourceIsolation({
  types: `${sharedTypeSource}\nexport interface AutomationLeak { endpoint: string }`,
  styles: sharedStyleSource,
  connectorStyles: connectorStyleSource,
  comments: sharedCommentSource,
  projectCopy: sharedProjectCopySource
}));
mustReject("MCP token in shared types", () => assertSharedSourceIsolation({
  types: `${sharedTypeSource}\nexport type TransportLeak = "mcp_stdio";`,
  styles: sharedStyleSource,
  connectorStyles: connectorStyleSource,
  comments: sharedCommentSource,
  projectCopy: sharedProjectCopySource
}));
mustReject("Connector selector in shared CSS", () => assertSharedSourceIsolation({
  types: sharedTypeSource,
  styles: `${sharedStyleSource}\n.connector-comment-badge { display: block; }`,
  connectorStyles: connectorStyleSource,
  comments: sharedCommentSource,
  projectCopy: sharedProjectCopySource
}));
mustReject("Connector comment copy in shared source", () => assertSharedSourceIsolation({
  types: sharedTypeSource,
  styles: sharedStyleSource,
  connectorStyles: connectorStyleSource,
  comments: `${sharedCommentSource}\nconst leak = "connected AI apps";`,
  projectCopy: sharedProjectCopySource
}));
mustReject("negative network teaser in shared help", () => assertSharedSourceIsolation({
  types: sharedTypeSource,
  styles: sharedStyleSource,
  connectorStyles: connectorStyleSource,
  comments: sharedCommentSource,
  projectCopy: `${sharedProjectCopySource}\nconst leak = "No AI, no network";`
}));
for (const capabilityCopy of [
  "Connect via MCP",
  "MCP server",
  "Explain this with an AI explanation",
  "Local automation uses a local endpoint and a one-time password",
  "Configure a local model server for inference",
  "Choose your API provider and API key",
  "Avoid FOMO"
]) {
  mustReject(`Connector capability copy in Local: ${capabilityCopy}`, () =>
    assertBundleEdition("local", ["assets/index.js"], capabilityCopy));
}
// Local may describe files it inventories without advertising a Connector
// capability or including any Connector command/chunk.
assertBundleEdition(
  "local",
  ["assets/index.js"],
  "Local model files. .mcp.json. MCP (shared). MCP servers. Model weights."
);
mustReject("missing required Connector IPC", () => assertBundleEdition(
  "connector",
  syntheticConnectorNames,
  syntheticConnectorText.replace("ai_provider_get", "removed_provider_get")
));
mustReject("missing connectorApi wrapper", () => assertConnectorApiCommandInventory(
  connectorApiSource.replace('"ai_provider_get"', '"removed_provider_get"')
));
mustReject("unreviewed connectorApi IPC", () => assertConnectorApiCommandInventory(
  `${connectorApiSource}\nconst unreviewed = "ai_unreviewed_surface";`
));
const missingLayerCssEdge = new Map(connectorFrontendSources);
missingLayerCssEdge.set(
  "apps/desktop/src/views/ConnectorEditionLayer.tsx",
  missingLayerCssEdge.get("apps/desktop/src/views/ConnectorEditionLayer.tsx").replace('import "./ConnectorEditionLayer.css";', "")
);
mustReject("missing Connector frontend source edge", () => assertConnectorFrontendSourceGraph(missingLayerCssEdge));
const localOverlayWithInstalledCapabilityDoc = JSON.parse(JSON.stringify(localReleaseConfig));
localOverlayWithInstalledCapabilityDoc.bundle.resources["../../../docs/connect-your-ai-app.md"] = "connect-your-ai-app.md";
mustReject("Connector capability document in Local installed resources", () =>
  assertReleaseOverlayResourceClosure(localOverlayWithInstalledCapabilityDoc, "Local"));
mustReject("Connector installation path in Local native hook", () =>
  assertLocalNativeInstallerIsolation({
    installerHooks: localInstallerHooks,
    hookSource: `${localInstallerHookSource}\nReadRegStr $0 HKCU "Software\\JCOM Labs\\Code Hangar\\Installations\\Code Hangar AI Connector" "Executable"`
  }));
for (const [label, source] of [
  ["fetch", "globalThis['fetch']('/leak')"],
  ["XHR", "new XMLHttpRequest()"],
  ["beacon", "navigator.sendBeacon('/leak', body)"],
  ["Tauri HTTP alias", "import { fetch as providerFetch } from '@tauri-apps/plugin-http'"],
  ["updater", "@tauri-apps/plugin-updater"],
  ["telemetry", "start telemetry client"],
  ["telemetry", "sentry analytics"],
  ["remote Git", "git push origin main"]
]) {
  mustReject(`${label} in Local`, () => assertBundleEdition("local", ["assets/index.js"], source));
  mustReject(`${label} in Connector`, () => assertBundleEdition("connector", syntheticConnectorNames, `${syntheticConnectorText}\n${source}`));
}

if (selfTestOnly) {
  console.log("Frontend edition guard self-test passed: Local/Connector isolation, completeness and webview-network negative fixtures rejected as expected.");
  process.exit(0);
}

if (!fs.existsSync(dist)) throw new Error(`Frontend bundle does not exist: ${dist}`);

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

assertBundleEdition(edition, relativeNames, bundleText);
if (edition === "local") {
  console.log(
    `Local frontend isolation passed (${files.length} text assets): no Connector, AI Assist or MCP-capability chunks/copy/CSS/IPC, and no direct webview-network primitives.`
  );
} else {
  console.log(
    `Connector frontend completeness passed (${files.length} text assets): all feature-gated AI/MCP chunks and IPC are present; provider transport remains native-only and the webview has no direct network primitive.`
  );
}
