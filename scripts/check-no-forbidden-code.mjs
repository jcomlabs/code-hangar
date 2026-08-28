import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const editionPolicy = JSON.parse(fs.readFileSync(path.join(root, "scripts/edition-network-policy.json"), "utf8"));
if (
  editionPolicy.schemaVersion !== 1
  || editionPolicy.documentType !== "codehangar/edition-network-policy/1"
  || editionPolicy.local?.feature !== "mutation"
  || editionPolicy.connector?.feature !== "agent_automation"
  || editionPolicy.connector?.outboundCrate?.name !== "hangar-ai"
  || editionPolicy.connector?.outboundCrate?.runtimeSource !== "crates/hangar-ai/src/lib.rs"
  || editionPolicy.connector?.outboundCrate?.orchestrationSource !== "crates/hangar-api/src/ai_assist.rs"
  || editionPolicy.connector?.tauriCommandSource !== "apps/desktop/src-tauri/src/main.rs"
) {
  throw new Error("The edition network policy drifted from the exact Connector-only provider boundary.");
}
const connectorRuntimeNetworkFile = path.normalize(editionPolicy.connector.outboundCrate.runtimeSource);
const connectorOrchestrationFile = path.normalize(editionPolicy.connector.outboundCrate.orchestrationSource);
const connectorTauriCommandFile = path.normalize(editionPolicy.connector.tauriCommandSource);
const connectorFrontendIpcCommands = editionPolicy.connector.frontendIpcCommands ?? [];
if (
  connectorFrontendIpcCommands.length === 0
  || new Set(connectorFrontendIpcCommands).size !== connectorFrontendIpcCommands.length
  || connectorFrontendIpcCommands.some((name) => !/^(?:ai_[A-Za-z0-9_]+|set_ai_[A-Za-z0-9_]+|apply_ai_suggestion|undo_ai_edit_session)$/.test(name))
) {
  throw new Error("The edition policy has no valid, unique canonical Connector AI IPC inventory.");
}
const scanRoots = ["apps", "crates", "scripts", ".github"];
const scanExtensions = new Set([
  ".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs", ".json", ".toml", ".html", ".css",
  ".ps1", ".cmd", ".bat", ".yml", ".yaml", ".nsi", ".nsh"
]);
const compiledExtensions = new Set([
  ".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs", ".json", ".toml", ".html", ".css"
]);
const javascriptExtensions = new Set([".ts", ".tsx", ".js", ".mjs", ".cjs", ".html"]);
const rustExtensions = new Set([".rs"]);
const commandBearingExtensions = new Set([
  ".ps1", ".cmd", ".bat", ".yml", ".yaml", ".nsi", ".nsh", ".ts", ".tsx", ".js", ".mjs", ".cjs"
]);

const deniedRules = [
  {
    id: "browser-fetch",
    extensions: javascriptExtensions,
    pattern: /(?:\bfetch\s*\(|\[\s*["']fetch["']\s*\]\s*\()/,
    message: "browser fetch call"
  },
  {
    id: "browser-stream-client",
    extensions: javascriptExtensions,
    pattern: /(?:\b(?:WebSocket|EventSource)\s*\(|\[\s*["'](?:WebSocket|EventSource)["']\s*\]\s*\()/,
    message: "browser streaming client"
  },
  {
    id: "browser-xhr-beacon",
    extensions: javascriptExtensions,
    pattern: /(?:\bXMLHttpRequest\s*\(|\bsendBeacon\s*\(|\[\s*["'](?:XMLHttpRequest|sendBeacon)["']\s*\]\s*\()/,
    message: "browser XHR/beacon client"
  },
  {
    id: "node-network-module",
    extensions: javascriptExtensions,
    pattern: /(?:\b(?:require|import|getBuiltinModule)\s*\(\s*["'](?:node:)?(?:http|https|http2|net|dns|tls|dgram)(?:\/[^"']*)?["']\s*\)|\b(?:import|export)\s+(?:[^;\r\n]*?\s+from\s+)?["'](?:node:)?(?:http|https|http2|net|dns|tls|dgram)(?:\/[^"']*)?["'])/i,
    message: "Node network module"
  },
  {
    id: "tauri-http-plugin",
    extensions: javascriptExtensions,
    pattern: /["']@tauri-apps\/plugin-http["']/i,
    message: "Tauri HTTP plugin import"
  },
  {
    id: "rust-socket",
    extensions: rustExtensions,
    pattern: /\b(?:std::net::)?(?:TcpStream|TcpListener|UdpSocket|ToSocketAddrs)\b/,
    message: "Rust raw network socket"
  },
  {
    id: "rust-http-client",
    extensions: rustExtensions,
    pattern: /\b(?:reqwest|hyper|ureq|isahc|surf)::/,
    message: "Rust HTTP client"
  },
  {
    id: "windows-network-api",
    extensions: scanExtensions,
    pattern: /\b(?:WinHttp[A-Za-z0-9_]*|WinInet|Internet(?:Open|Connect|ReadFile|WriteFile|CrackUrl|CanonicalizeUrl|SetOption|QueryOption|OpenUrl)[AW]?|Http(?:OpenRequest|SendRequest|QueryInfo|AddRequestHeaders)[AW]?|URLDownloadToFile[AW]?|WSA(?:Startup|Socket|Connect|Send|Recv|GetAddrInfo)[AW]?|Winsock|ConnectEx|GetAddrInfo(?:Ex)?[AW]?|getaddrinfo|DnsQuery(?:Ex|_[A-Z])?)\b/i,
    message: "Windows outbound network API"
  },
  {
    id: "powershell-network-client",
    extensions: commandBearingExtensions,
    pattern: /\b(?:Invoke-WebRequest|Invoke-RestMethod|iwr|irm|Start-BitsTransfer|HttpClient|HttpWebRequest|WebRequest|WebClient|(?:System\.)?Net\.(?:Dns|Http\.HttpClient|HttpWebRequest|WebRequest|WebClient|Sockets\.(?:Socket|TcpClient|TcpListener|UdpClient)))\b/i,
    message: "PowerShell/.NET network client"
  },
  {
    id: "download-command",
    extensions: commandBearingExtensions,
    pattern: /(?:^|[\s'"`;&|])(?:bitsadmin|curl|wget|certutil)(?:\.exe)?(?=$|[\s'"`;&|])/im,
    message: "download-capable command"
  },
  {
    id: "package-network-command",
    extensions: commandBearingExtensions,
    pattern: /\b(?:npm(?:\.cmd)?\s+(?:audit|install)|cargo(?:\.exe)?\s+fetch)\b/i,
    message: "package-manager network command"
  },
  {
    id: "mutation-command",
    extensions: compiledExtensions,
    pattern: /\bplan_execute\b|\bbackup_run\b|\bquarantine_|\bpermanent_delete\b/,
    message: "mutation or execution command"
  },
  {
    id: "agent-surface",
    extensions: compiledExtensions,
    pattern: /\btrusted_agent_|\bagent_(?:project_context|plan_|read_|activity_|scope|register|revoke|server)|\bmcp(?:\b|_)/i,
    message: "agent IPC/MCP surface"
  },
  {
    id: "remote-git",
    extensions: scanExtensions,
    pattern: /\bgit(?:\.exe)?\s+(?:fetch|pull|push|clone)\b/i,
    message: "remote Git operation"
  },
  {
    id: "updater",
    extensions: compiledExtensions,
    pattern: /\btauri_plugin_updater\b|\bplugin-updater\b/i,
    message: "auto-updater"
  },
  {
    id: "telemetry",
    extensions: compiledExtensions,
    pattern: /\btelemetry\b|\banalytics\b|\bsentry\b/i,
    message: "telemetry or analytics"
  }
];

// These checkers necessarily contain the deny expressions and negative fixture
// source they enforce. They are the only whole-file exceptions; both execute
// their negative fixtures whenever invoked.
const checkerPaths = new Set([
  path.normalize("scripts/check-no-forbidden-code.mjs"),
  path.normalize("scripts/check-frontend-edition.mjs")
]);

// These two security validators contain literal regex entries which reject
// download commands in generated installer material. Only a line which is
// exactly one inert regex literal is exempted; an invocation in either file is
// still rejected.
const outboundRegexGuardFiles = new Set([
  path.normalize("scripts/check-no-outbound-deps.mjs"),
  path.normalize("scripts/packaging-preflight.mjs")
]);
const quotedMarkerGuardFiles = new Set([
  path.normalize("scripts/release-pipeline-self-test.ps1")
]);
const policyDefinitionFiles = new Set([
  path.normalize("scripts/check-no-outbound-deps.mjs"),
  path.normalize("scripts/edition-network-policy.json")
]);
const inertOutboundRegexLiteral = /^\s*\/(?:Invoke-WebRequest|Invoke-RestMethod|Start-BitsTransfer|HttpClient|WebClient|\\b(?:curl|wget|certutil)[^/]*)\/i,?\s*$/;
const inertQuotedMarkerList = /^(?:["'][^"'`]+["']\s*,?\s*)+$/;
const inertDeniedDependencyName = /^\s*["'](?:curl|curl-sys)["']\s*,?\s*$/;

const exactLineAllowlist = new Map([
  [
    `${path.normalize("crates/hangar-discovery/src/lib.rs")}:agent-surface`,
    new Set(["\"mcp_\","])
  ]
]);

// Mutation/execution identifiers are permitted only inside the dedicated,
// feature-gated mutation crate and its feature-gated API dispatch surface.
const mutationCrateDir = path.normalize("crates/hangar-mutation");
const mutationApiFile = path.normalize("crates/hangar-api/src/lib.rs");

// Connected-app names are permitted only in compile-absent Connector surfaces.
// This exception never lifts any outbound rule.
const agentSurfaceDirs = [
  "crates/hangar-agent",
  "crates/hangar-mcp",
  "crates/hangar-appconfig",
  "apps/mcp-server"
].map((dir) => path.normalize(dir));
const agentSurfaceFiles = [
  ...(editionPolicy.connector.frontendOnlyFiles ?? []),
  "apps/desktop/src/views/ConnectorGuidedTour.tsx",
  "apps/desktop/src/__tests__/guided-tour.test.ts",
  "apps/desktop/src-tauri/tauri.connector.conf.json",
  "apps/desktop/src-tauri/tauri.release-connector.conf.json",
  "scripts/packaging-preflight.mjs",
  "scripts/release-gate-contracts.json"
].map((file) => path.normalize(file));
const aiToolKnowledgeFiles = [
  "apps/desktop/src/ai-tool-files.ts",
  "apps/desktop/src/__tests__/ai-tool-files.test.ts"
].map((file) => path.normalize(file));

function walk(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (["node_modules", "target", "dist", ".git", ".local"].includes(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (scanExtensions.has(path.extname(entry.name).toLowerCase())) acc.push(full);
  }
  return acc;
}

function stripCodeComments(text, extension) {
  // Preserve quoted text while blanking comments. A regex-only `//` remover
  // treats the slashes in `"https://..."` as a comment start, which lets code
  // later on the same line evade the guard. Newlines and character positions
  // are preserved so diagnostics and exact per-match allowlists remain stable.
  let output = "";
  let state = "code";
  let quote = "";
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    const next = text[index + 1] ?? "";

    if (state === "line-comment") {
      if (char === "\n") {
        output += char;
        state = "code";
      } else {
        output += " ";
      }
      continue;
    }
    if (state === "block-comment") {
      if (char === "*" && next === "/") {
        output += "  ";
        index += 1;
        state = "code";
      } else {
        output += char === "\n" ? "\n" : " ";
      }
      continue;
    }
    if (state === "quote") {
      output += char;
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        state = "code";
        quote = "";
      }
      continue;
    }

    if (char === "/" && next === "/") {
      output += "  ";
      index += 1;
      state = "line-comment";
      continue;
    }
    if (char === "/" && next === "*") {
      output += "  ";
      index += 1;
      state = "block-comment";
      continue;
    }
    if (char === '"' || char === "`" || char === "'") {
      // Rust lifetimes (`'a`, `'static`) are not character literals. Leaving
      // them in code state avoids swallowing the remainder of a Rust module.
      if (extension === ".rs" && char === "'") {
        const lifetime = /^'[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(index));
        if (lifetime && text[index + lifetime[0].length] !== "'") {
          output += char;
          continue;
        }
      }
      output += char;
      state = "quote";
      quote = char;
      escaped = false;
      continue;
    }
    output += char;
  }
  return output;
}

function stripWholeLineScriptComments(text, extension) {
  if (extension === ".ps1") {
    return text.replace(/<#[\s\S]*?#>/g, "").replace(/^\s*#.*$/gm, "");
  }
  if ([".yml", ".yaml"].includes(extension)) return text.replace(/^\s*#.*$/gm, "");
  if ([".nsi", ".nsh"].includes(extension)) return text.replace(/^\s*;.*$/gm, "");
  if ([".cmd", ".bat"].includes(extension)) return text.replace(/^\s*(?:rem\b|::).*$/gim, "");
  return text;
}

const CONNECTOR_GATE = /#\[\s*cfg\([^\]]*feature\s*=\s*"(?:agent_automation|mcp|test_support)"[^\]]*\)\s*\]/;

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
    i += 1;
    while (i < lines.length && /^\s*#\[/.test(lines[i])) i += 1;
    let brace = 0;
    let group = 0;
    let openedBrace = false;
    let done = false;
    while (i < lines.length && !done) {
      const line = lines[i];
      const lastTokenIndex = line.trimEnd().length - 1;
      for (const [charIndex, ch] of [...line].entries()) {
        if (ch === "{") {
          brace += 1;
          openedBrace = true;
        } else if (ch === "}") brace -= 1;
        else if (ch === "(" || ch === "[") group += 1;
        else if (ch === ")" || ch === "]") group -= 1;
        else if (ch === ";" && !openedBrace && brace === 0 && group === 0) done = true;
        else if (ch === "," && charIndex === lastTokenIndex && !openedBrace && brace === 0 && group === 0) done = true;
      }
      i += 1;
      if (openedBrace && brace === 0) done = true;
    }
  }
  return out.join("\n");
}

function stripReadOnlyMutationDisplayLabels(text) {
  return text.replace(/\bcase\s+["']permanent_delete["']\s*:/g, "case \"<historical-delete-kind>\":");
}

function sourceForRule(text, extension, rule) {
  let source = javascriptExtensions.has(extension) || rustExtensions.has(extension)
    ? stripCodeComments(text, extension)
    : stripWholeLineScriptComments(text, extension);
  if (rule.id === "agent-surface") source = stripConnectorGatedItems(source);
  if (rule.id === "mutation-command") source = stripReadOnlyMutationDisplayLabels(source);
  if (rule.id === "telemetry") {
    source = source
      .replace(/\bno\s+(?:network\s+or\s+)?telemetry\b/gi, "")
      .replace(/\bwithout\s+telemetry\b/gi, "")
      .replace(/\bnever\s+sends?\s+telemetry\b/gi, "");
  }
  return source;
}

function lineForIndex(source, index) {
  const start = source.lastIndexOf("\n", index - 1) + 1;
  const endCandidate = source.indexOf("\n", index);
  const end = endCandidate === -1 ? source.length : endCandidate;
  return source.slice(start, end).trim();
}

const connectorRuntimeTestSocketLines = new Set([
  "use std::net::TcpListener;",
  "let target = TcpListener::bind((target_ip, 0)).expect(\"bind redirect target\");",
  "let redirector = TcpListener::bind(\"127.0.0.1:0\").expect(\"bind redirector\");",
  "let listener = TcpListener::bind(\"127.0.0.1:0\").expect(\"bind loopback\");",
  "let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();"
]);
const credentialRaceTestFile = path.normalize("crates/hangar-api/src/lib.rs");
const credentialRaceTestSocketLines = new Set([
  "use std::net::TcpListener;",
  "let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();"
]);

function isAllowedMatch(relativePath, rule, matchedLine, matchIndex, source) {
  if (rule.id === "rust-http-client" && relativePath === connectorRuntimeNetworkFile) return true;
  if (rule.id === "rust-socket" && relativePath === connectorRuntimeNetworkFile) {
    const testModuleIndex = source.search(/#\[cfg\(test\)\]\s*\r?\n\s*mod tests \{/);
    return testModuleIndex !== -1
      && matchIndex > testModuleIndex
      && connectorRuntimeTestSocketLines.has(matchedLine);
  }
  if (rule.id === "rust-socket" && relativePath === credentialRaceTestFile) {
    const testModuleIndex = source.search(/#\[cfg\(test\)\]\s*\r?\n\s*mod tests \{/);
    const raceTestIndex = source.indexOf("fn credential_mutation_waits_until_linearized_provider_send_finishes()", testModuleIndex);
    return testModuleIndex !== -1
      && raceTestIndex > testModuleIndex
      && matchIndex > raceTestIndex
      && credentialRaceTestSocketLines.has(matchedLine);
  }
  if (
    outboundRegexGuardFiles.has(relativePath)
    && ["powershell-network-client", "download-command"].includes(rule.id)
    && inertOutboundRegexLiteral.test(matchedLine)
  ) return true;
  if (relativePath === path.normalize("scripts/check-no-outbound-deps.mjs")
      && rule.id === "download-command"
      && inertDeniedDependencyName.test(matchedLine)) return true;
  if (quotedMarkerGuardFiles.has(relativePath) && inertQuotedMarkerList.test(matchedLine)) return true;
  return exactLineAllowlist.get(`${relativePath}:${rule.id}`)?.has(matchedLine) ?? false;
}

function findViolations(relativePath, text) {
  const extension = path.extname(relativePath).toLowerCase();
  const violations = [];
  for (const rule of deniedRules) {
    if (!rule.extensions.has(extension)) continue;
    if (
      policyDefinitionFiles.has(relativePath)
      && ["agent-surface", "updater", "telemetry", "tauri-http-plugin"].includes(rule.id)
    ) continue;
    const source = sourceForRule(text, extension, rule);
    const matcher = new RegExp(rule.pattern.source, `${rule.pattern.flags.replaceAll("g", "")}g`);
    for (const match of source.matchAll(matcher)) {
      if (
        rule.id === "mutation-command"
        && (relativePath === mutationApiFile || relativePath === mutationCrateDir || relativePath.startsWith(`${mutationCrateDir}${path.sep}`))
      ) continue;
      if (
        rule.id === "agent-surface"
        && (
          agentSurfaceDirs.some((dir) => relativePath === dir || relativePath.startsWith(`${dir}${path.sep}`))
          || agentSurfaceFiles.includes(relativePath)
          || aiToolKnowledgeFiles.includes(relativePath)
        )
      ) continue;
      const matchedLine = lineForIndex(source, match.index);
      if (isAllowedMatch(relativePath, rule, matchedLine, match.index, source)) continue;
      violations.push({ rule, match: match[0], line: matchedLine });
    }
  }
  return violations;
}

function assertConnectorRuntimePolicy() {
  const runtimePath = path.join(root, connectorRuntimeNetworkFile);
  const orchestrationPath = path.join(root, connectorOrchestrationFile);
  if (!fs.existsSync(runtimePath) || !fs.existsSync(orchestrationPath)) {
    throw new Error("The exact Connector provider runtime/orchestration sources are missing.");
  }
  const runtimeText = fs.readFileSync(runtimePath, "utf8");
  const requiredRuntimeContracts = [
    "reqwest::blocking::Client::builder()",
    ".redirect(reqwest::redirect::Policy::none())",
    ".no_proxy()",
    "fn is_loopback_url(",
    "pub fn validate_local_endpoint(",
    "pub fn validate_remote_endpoint(",
    "fn finalize_url("
  ];
  const missingContracts = requiredRuntimeContracts.filter((marker) => !runtimeText.includes(marker));
  if (missingContracts.length > 0) {
    throw new Error(`Connector provider runtime is missing fail-closed network contracts: ${missingContracts.join(", ")}.`);
  }
  const testModuleMatch = /#\[cfg\(test\)\]\s*\r?\n\s*mod tests \{/.exec(runtimeText);
  if (!testModuleMatch) throw new Error("Connector provider runtime has no explicit cfg(test) boundary.");
  const productionText = runtimeText.slice(0, testModuleMatch.index);
  if (/\b(?:std::thread|thread)::spawn\b|\b(?:TcpStream|TcpListener|UdpSocket|ToSocketAddrs)\b/.test(productionText)) {
    throw new Error("Connector provider production source contains an implicit background thread or raw socket primitive.");
  }
}

const aiCommandNamePattern = /^(?:ai_[A-Za-z0-9_]+|set_ai_[A-Za-z0-9_]+|apply_ai_suggestion|undo_ai_edit_session)$/;

function parseHandlerBlocks(source) {
  const blocks = [];
  const pattern = /#\[cfg\(([^\]\r\n]+)\)\]\s*\r?\n\s*let builder = builder\.invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\);/g;
  for (const match of source.matchAll(pattern)) {
    blocks.push({
      cfg: match[1].trim(),
      body: match[2],
      commands: match[2]
        .split(/\r?\n/)
        .map((line) => /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*,?\s*$/.exec(line)?.[1])
        .filter(Boolean)
    });
  }
  return blocks;
}

function assertTauriAiCommandPolicy(source) {
  const allAiCommands = [...source.matchAll(/^\s*async\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm)]
    .map((match) => match[1])
    .filter((name) => aiCommandNamePattern.test(name));
  if (allAiCommands.length === 0 || new Set(allAiCommands).size !== allAiCommands.length) {
    throw new Error("Tauri AI command discovery is empty or contains duplicate function names.");
  }
  const sortedAiCommands = [...allAiCommands].sort();
  const sortedPolicyCommands = [...connectorFrontendIpcCommands].sort();
  if (JSON.stringify(sortedAiCommands) !== JSON.stringify(sortedPolicyCommands)) {
    const missing = connectorFrontendIpcCommands.filter((name) => !allAiCommands.includes(name));
    const unexpected = allAiCommands.filter((name) => !connectorFrontendIpcCommands.includes(name));
    throw new Error(
      `Tauri AI IPC drifted from the canonical Connector policy. `
      + `Missing: ${missing.join(", ") || "none"}; unexpected: ${unexpected.join(", ") || "none"}.`
    );
  }

  const gatedAiCommands = [...source.matchAll(
    /#\[cfg\(feature = "agent_automation"\)\]\s*\r?\n\s*#\[tauri::command\]\s*\r?\n(?:\s*#\[[^\]\r\n]+\]\s*\r?\n)*\s*async\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/g
  )]
    .map((match) => match[1])
    .filter((name) => aiCommandNamePattern.test(name));
  const gatedSet = new Set(gatedAiCommands);
  const missingGates = allAiCommands.filter((name) => !gatedSet.has(name));
  if (missingGates.length > 0 || gatedSet.size !== allAiCommands.length) {
    throw new Error(`Tauri AI commands are not each compile-gated immediately before #[tauri::command]: ${missingGates.join(", ") || "unexpected duplicate"}.`);
  }

  const commandParameters = (name) => {
    const match = new RegExp(`async\\s+fn\\s+${name}\\s*\\(([\\s\\S]*?)\\)\\s*->`).exec(source);
    if (!match) throw new Error(`Could not inspect Tauri AI command signature: ${name}.`);
    return match[1];
  };
  for (const name of ["ai_provider_test_disclosure", "ai_provider_models_disclosure"]) {
    const parameters = commandParameters(name);
    for (const required of ["State<'_, AppState>", "mode: String", "base_url: String", "model: String", "format: String"]) {
      if (!parameters.includes(required)) {
        throw new Error(`${name} must validate the complete provider draft before staging its one-shot preview (missing ${required}).`);
      }
    }
    if (parameters.includes("preview_id")) {
      throw new Error(`${name} must prepare a disclosure, not consume a preview id.`);
    }
  }
  for (const name of ["ai_provider_test", "ai_provider_models"]) {
    const parameters = commandParameters(name);
    for (const required of ["State<'_, AppState>", "preview_id: String"]) {
      if (!parameters.includes(required)) {
        throw new Error(`${name} must consume only an in-memory one-shot preview (missing ${required}).`);
      }
    }
    const forbidden = ["mode: String", "base_url: String", "model: String", "format: String"]
      .filter((field) => parameters.includes(field));
    if (forbidden.length > 0) {
      throw new Error(`${name} reintroduced provider draft fields after review: ${forbidden.join(", ")}.`);
    }
  }

  const handlerBlocks = parseHandlerBlocks(source);
  const connectorBlocks = handlerBlocks.filter(({ cfg }) => cfg === 'feature = "agent_automation"');
  const localBlocks = handlerBlocks.filter(({ cfg }) => cfg !== 'feature = "agent_automation"');
  if (connectorBlocks.length !== 1 || localBlocks.length < 2) {
    throw new Error("Expected one exact Connector handler and both compile-absent Local/core handlers.");
  }
  for (const block of localBlocks) {
    const leaked = block.commands.filter((name) => aiCommandNamePattern.test(name));
    if (leaked.length > 0) {
      throw new Error(`Local/core Tauri handler contains Connector-only AI IPC: ${leaked.join(", ")}.`);
    }
  }
  const connectorCounts = new Map();
  for (const name of connectorBlocks[0].commands.filter((candidate) => aiCommandNamePattern.test(candidate))) {
    connectorCounts.set(name, (connectorCounts.get(name) ?? 0) + 1);
  }
  const missingFromConnector = allAiCommands.filter((name) => connectorCounts.get(name) !== 1);
  const unexpectedInConnector = [...connectorCounts].filter(([name]) => !allAiCommands.includes(name)).map(([name]) => name);
  if (missingFromConnector.length > 0 || unexpectedInConnector.length > 0) {
    throw new Error(
      `Connector Tauri handler must expose every and only feature-gated AI command exactly once. `
      + `Missing/duplicate: ${missingFromConnector.join(", ") || "none"}; unexpected: ${unexpectedInConnector.join(", ") || "none"}.`
    );
  }
}

assertConnectorRuntimePolicy();
const tauriCommandSource = fs.readFileSync(path.join(root, connectorTauriCommandFile), "utf8");
assertTauriAiCommandPolicy(tauriCommandSource);

// Executable negative fixtures: each listed primitive must be rejected in its
// relevant language. This prevents a future extension/rule refactor from making
// the repository scan silently blind.
const negativeFixtures = [
  ["fixture.ps1", "Invoke-WebRequest https://invalid.example", "powershell-network-client"],
  ["fixture.ps1", "Invoke-RestMethod https://invalid.example", "powershell-network-client"],
  ["fixture.ps1", "iwr https://invalid.example", "powershell-network-client"],
  ["fixture.ps1", "irm https://invalid.example", "powershell-network-client"],
  ["fixture.ps1", "[System.Net.Http.HttpClient]::new()", "powershell-network-client"],
  ["fixture.ps1", "New-Object System.Net.WebClient", "powershell-network-client"],
  ["fixture.ps1", "Start-BitsTransfer -Source x -Destination y", "powershell-network-client"],
  ["fixture.cmd", "bitsadmin /transfer x a b", "download-command"],
  ["fixture.bat", "curl.exe https://invalid.example", "download-command"],
  ["fixture.yml", "- run: wget https://invalid.example", "download-command"],
  ["fixture.yaml", "run: certutil.exe -urlcache x y", "download-command"],
  ["fixture.nsi", "nsExec::ExecToLog 'curl https://invalid.example'", "download-command"],
  ["fixture.yml", "run: npm audit", "package-network-command"],
  ["fixture.cmd", "npm install foo", "package-network-command"],
  ["fixture.ps1", "cargo fetch --locked", "package-network-command"],
  ["fixture.mjs", "import http from 'node:http';", "node-network-module"],
  ["fixture.mjs", "import http from 'http';", "node-network-module"],
  ["fixture.mjs", "import 'https';", "node-network-module"],
  ["fixture.mjs", "export { request } from 'http';", "node-network-module"],
  ["fixture.mjs", "const http = await import('http');", "node-network-module"],
  ["fixture.ts", "require('https')", "node-network-module"],
  ["fixture.js", "const socket = require('net');", "node-network-module"],
  ["fixture.mjs", "import dns from 'node:dns';", "node-network-module"],
  ["fixture.mjs", "import tls from 'node:tls';", "node-network-module"],
  ["fixture.mjs", "import dgram from 'dgram';", "node-network-module"],
  ["fixture.mjs", "process.getBuiltinModule('node:https')", "node-network-module"],
  ["fixture.ts", "import { fetch as remoteFetch } from '@tauri-apps/plugin-http';", "tauri-http-plugin"],
  ["fixture.ps1", "[System.Net.WebRequest]::Create('https://invalid.example')", "powershell-network-client"],
  ["fixture.ps1", "[System.Net.Sockets.TcpClient]::new('invalid.example', 443)", "powershell-network-client"],
  ["fixture.ps1", "New-Object System.Net.Sockets.TcpClient", "powershell-network-client"],
  ["fixture.ps1", "[System.Net.Dns]::GetHostAddresses('invalid.example')", "powershell-network-client"],
  ["fixture.ts", "new XMLHttpRequest()", "browser-xhr-beacon"],
  ["fixture.ts", "navigator.sendBeacon('/events', data)", "browser-xhr-beacon"],
  ["fixture.ts", "globalThis['fetch']('/events')", "browser-fetch"],
  ["fixture.ts", "navigator['sendBeacon']('/events', data)", "browser-xhr-beacon"],
  ["fixture.ts", "new globalThis['WebSocket']('wss://invalid.example')", "browser-stream-client"],
  ["fixture.rs", "std::net::TcpListener::bind(addr)", "rust-socket"],
  ["fixture.rs", "use std::net::ToSocketAddrs;", "rust-socket"],
  ["fixture.rs", "reqwest::blocking::Client::new()", "rust-http-client"],
  ["fixture.rs", "let endpoint = \"https://invalid.example\"; reqwest::blocking::Client::new();", "rust-http-client"],
  ["fixture.rs", "WinHttpOpen(ptr, 0, ptr, ptr, 0)", "windows-network-api"],
  ["fixture.rs", "InternetOpenW(ptr, 0, ptr, ptr, 0)", "windows-network-api"],
  ["fixture.rs", "HttpSendRequestW(request, ptr, 0, ptr, 0)", "windows-network-api"],
  ["fixture.rs", "URLDownloadToFileW(null(), url, path, 0, null())", "windows-network-api"],
  ["fixture.rs", "WSAStartup(version, data)", "windows-network-api"],
  ["fixture.rs", "WSAConnect(socket, addr, len, null(), null(), null(), null())", "windows-network-api"],
  ["fixture.rs", "getaddrinfo(name, service, hints, result)", "windows-network-api"],
  ["fixture.rs", "DnsQuery_W(name, 1, 0, null(), null(), null())", "windows-network-api"]
];
for (const [fixturePath, source, expectedRule] of negativeFixtures) {
  const violations = findViolations(path.normalize(fixturePath), source);
  if (!violations.some(({ rule }) => rule.id === expectedRule)) {
    throw new Error(`Forbidden-code negative fixture did not trigger ${expectedRule}: ${source}`);
  }
}
if (findViolations(path.normalize("apps/example/install.ts"), "npm audit").length === 0
    || findViolations(path.normalize("scripts/bootstrap.ps1"), "cargo fetch --locked").length === 0) {
  throw new Error("A package-network command escaped the outbound guard.");
}
const passiveDiscoveryMarkerPath = path.normalize("crates/hangar-discovery/src/lib.rs");
if (findViolations(passiveDiscoveryMarkerPath, '"mcp_",').length !== 0
    || !findViolations(passiveDiscoveryMarkerPath, "fn escaped() { mcp_server(); }")
      .some(({ rule }) => rule.id === "agent-surface")) {
  throw new Error("The passive discovery-marker allowlist escaped its one exact source line.");
}
if (findViolations(path.normalize("scripts/packaging-preflight.mjs"), "  /Invoke-WebRequest/i,").length !== 0
    || findViolations(path.normalize("scripts/packaging-preflight.mjs"), "Invoke-WebRequest https://invalid.example").length === 0) {
  throw new Error("The inert outbound-regex allowlist is broader than intended.");
}
const allowedSocketFixture = [
  "#[cfg(test)]",
  "mod tests {",
  "    use std::net::TcpListener;",
  "    let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();",
  "}"
].join("\n");
if (findViolations(connectorRuntimeNetworkFile, allowedSocketFixture).length !== 0
    || findViolations(connectorRuntimeNetworkFile, "fn production() { use std::net::TcpListener; }").length === 0
    || findViolations(connectorRuntimeNetworkFile, `${allowedSocketFixture}\nfn escaped() { TcpListener::bind(\"0.0.0.0:0\"); }`).length === 0) {
  throw new Error("Connector test-socket allowlist escaped its exact cfg(test), path, or loopback-line boundary.");
}
const allowedCredentialRaceSocketFixture = [
  "#[cfg(test)]",
  "mod tests {",
  "    fn credential_mutation_waits_until_linearized_provider_send_finishes() {",
  "        use std::net::TcpListener;",
  "        let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();",
  "    }",
  "}"
].join("\n");
if (findViolations(credentialRaceTestFile, allowedCredentialRaceSocketFixture).length !== 0
    || findViolations(credentialRaceTestFile, "fn production() { use std::net::TcpListener; }").length === 0
    || findViolations(credentialRaceTestFile, `${allowedCredentialRaceSocketFixture}\nfn escaped() { TcpListener::bind(\"0.0.0.0:0\"); }`).length === 0) {
  throw new Error("Credential-race test socket allowlist escaped its exact test, path, or loopback-line boundary.");
}
if (findViolations(connectorRuntimeNetworkFile, "reqwest::blocking::Client::new()").length !== 0
    || findViolations(path.normalize("crates/hangar-api/src/ai_assist.rs"), "reqwest::blocking::Client::new()").length === 0) {
  throw new Error("Connector Rust HTTP allowlist escaped its one exact runtime source.");
}
const cfgElementFollowedByLocalLeak = [
  "let features = vec![",
  "    #[cfg(feature = \"agent_automation\")]",
  "    \"connector-only\",",
  "];",
  "fn local_surface() { mcp(); }"
].join("\n");
if (!findViolations(path.normalize("crates/example/src/lib.rs"), cfgElementFollowedByLocalLeak)
  .some(({ rule }) => rule.id === "agent-surface")) {
  throw new Error("A cfg-gated array element swallowed the following ungated Local source.");
}
const cfgFunctionWithGenericReturn = [
  "#[cfg(feature = \"agent_automation\")]",
  "pub fn connector_only() -> Result<McpTransportBinding, String> {",
  "    Err(\"MCP is unavailable\".to_string())",
  "}",
  "fn local_surface() { local_only(); }"
].join("\n");
if (findViolations(path.normalize("crates/example/src/lib.rs"), cfgFunctionWithGenericReturn).length !== 0) {
  throw new Error("A comma inside a cfg-gated Rust generic return type escaped the feature gate.");
}

// Mutate the actual Tauri source to prove the command gate and handler checks
// fail closed without having to maintain a second synthetic command inventory.
const firstAiCommand = [...tauriCommandSource.matchAll(/^\s*async\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm)]
  .map((match) => match[1])
  .find((name) => aiCommandNamePattern.test(name));
const wrongGateSource = tauriCommandSource.replace(
  `#[cfg(feature = "agent_automation")]\r\n#[tauri::command]\r\nasync fn ${firstAiCommand}`,
  `#[cfg(feature = "mutation")]\r\n#[tauri::command]\r\nasync fn ${firstAiCommand}`
).replace(
  `#[cfg(feature = "agent_automation")]\n#[tauri::command]\nasync fn ${firstAiCommand}`,
  `#[cfg(feature = "mutation")]\n#[tauri::command]\nasync fn ${firstAiCommand}`
);
let rejectedWrongTauriGate = false;
try {
  assertTauriAiCommandPolicy(wrongGateSource);
} catch {
  rejectedWrongTauriGate = true;
}
if (!firstAiCommand || wrongGateSource === tauriCommandSource || !rejectedWrongTauriGate) {
  throw new Error("Tauri AI command negative fixture did not reject a missing agent_automation gate.");
}
const unsafeProviderConfirmSource = tauriCommandSource.replace(
  /(async\s+fn\s+ai_provider_test\s*\([\s\S]*?preview_id:\s*String,?)(\s*\)\s*->)/,
  "$1\n    base_url: String,$2"
);
let rejectedProviderDraftBypass = false;
try {
  assertTauriAiCommandPolicy(unsafeProviderConfirmSource);
} catch {
  rejectedProviderDraftBypass = true;
}
if (unsafeProviderConfirmSource === tauriCommandSource || !rejectedProviderDraftBypass) {
  throw new Error("Tauri AI command negative fixture accepted provider draft fields after one-shot review.");
}
let rejectedUnreviewedTauriCommand = false;
try {
  assertTauriAiCommandPolicy(
    `${tauriCommandSource}\n#[cfg(feature = "agent_automation")]\n#[tauri::command]\nasync fn ai_unreviewed_surface() {}`
  );
} catch {
  rejectedUnreviewedTauriCommand = true;
}
if (!rejectedUnreviewedTauriCommand) {
  throw new Error("Tauri AI command negative fixture accepted an IPC outside the canonical edition policy.");
}

for (const rootName of scanRoots) {
  for (const file of walk(path.join(root, rootName))) {
    const relativePath = path.normalize(path.relative(root, file));
    if (checkerPaths.has(relativePath)) continue;
    const violations = findViolations(relativePath, fs.readFileSync(file, "utf8"));
    if (violations.length === 0) continue;
    const finding = violations[0];
    throw new Error(
      `${relativePath} contains forbidden ${finding.rule.message}: ${finding.match} (line: ${finding.line})`
    );
  }
}

console.log(
  `Forbidden-token guard passed across ${[...scanExtensions].join(", ")}; ${negativeFixtures.length} executable language-aware negative fixtures passed. This lexical gate complements, but does not claim to replace, dependency-graph and runtime isolation evidence.`
);
