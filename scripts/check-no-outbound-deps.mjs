import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = process.cwd();
const argumentsList = process.argv.slice(2);
if (argumentsList.some((argument) => argument !== "--static-only") || argumentsList.length > 1) {
  throw new Error("Usage: node scripts/check-no-outbound-deps.mjs [--static-only]");
}
const staticOnly = argumentsList.includes("--static-only");
const supportedRustTarget = "x86_64-pc-windows-msvc";
const policyPath = path.join(root, "scripts/edition-network-policy.json");
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
  || editionPolicy.connector?.outboundCrate?.manifest !== "crates/hangar-ai/Cargo.toml"
  || editionPolicy.connector?.outboundCrate?.runtimeSource !== "crates/hangar-ai/src/lib.rs"
  || editionPolicy.connector?.outboundCrate?.orchestrationSource !== "crates/hangar-api/src/ai_assist.rs"
  || editionPolicy.connector?.outboundCrate?.apiManifest !== "crates/hangar-api/Cargo.toml"
  || editionPolicy.connector?.tauriCommandSource !== "apps/desktop/src-tauri/src/main.rs"
  || JSON.stringify(editionPolicy.connector?.frontendOnlyFiles) !== JSON.stringify(expectedFrontendOnlyFiles)
) {
  throw new Error("The edition network policy drifted from the closed Local/Connector boundary.");
}
const connectorOutbound = editionPolicy.connector.outboundCrate;
const connectorManifestPath = path.normalize(connectorOutbound.manifest);
const connectorApiManifestPath = path.normalize(connectorOutbound.apiManifest);
const cargoEnvironment = {
  ...process.env,
  CARGO_NET_OFFLINE: "true",
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS || "2"
};

const deniedNpmManifestDeps = new Set([
  "axios",
  "cross-fetch",
  "eventsource",
  "got",
  "http",
  "https",
  "http2",
  "net",
  "dns",
  "tls",
  "node-fetch",
  "needle",
  "request",
  "socket.io-client",
  "superagent",
  "undici",
  "ws",
  "ky",
  "reqwest",
  "hyper",
  "native-tls",
  "openssl",
  "ureq",
  "isahc",
  "surf",
  "attohttpc",
  "minreq",
  "curl",
  "curl-sys",
  "hickory-resolver",
  "trust-dns-resolver",
  "tokio-tungstenite",
  "tungstenite",
  "async-tungstenite",
  "tonic",
  "tonic-transport",
  "tauri-plugin-http",
  "@tauri-apps/plugin-http",
  "tauri-plugin-updater",
  "@tauri-apps/plugin-updater",
  "sentry"
]);
// Cargo's `http` crate is a data-types library already present through Tauri;
// unlike Node's `http` builtin, its mere presence is not an outbound transport.
// Keep language ecosystems separate so the Windows graph gate proves actual
// clients/transports without making a false lockfile-wide `http`/`url` claim.
const deniedCargoManifestDeps = new Set([
  "reqwest",
  "hyper",
  "native-tls",
  "openssl",
  "ureq",
  "isahc",
  "surf",
  "attohttpc",
  "minreq",
  "curl",
  "curl-sys",
  "hickory-resolver",
  "trust-dns-resolver",
  "tokio-tungstenite",
  "tungstenite",
  "async-tungstenite",
  "tonic",
  "tonic-transport",
  "tauri-plugin-http",
  "tauri-plugin-updater",
  "sentry"
]);
const connectorExclusiveManifestDeps = new Set(["hangar-ai", "url", "keyring"]);

// Allowed only as transitive dependencies of SQLCipher for local database
// encryption at rest. They must not be declared directly in Code Hangar
// manifests and do not permit TLS, HTTP, fetch, telemetry, or package access.
const sqlCipherAtRestCryptoDeps = new Set([
  "openssl-sys",
  "openssl-src"
]);

const manifestNames = ["package.json", "Cargo.toml"];

function walk(dir, acc = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if ([".git", ".local", "node_modules", "target", "dist"].includes(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (manifestNames.includes(entry.name)) acc.push(full);
  }
  return acc;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function getPackageDeps(pkg) {
  return [
    ...Object.entries(pkg.dependencies ?? {}),
    ...Object.entries(pkg.devDependencies ?? {}),
    ...Object.entries(pkg.optionalDependencies ?? {})
  ];
}

function getDeniedNpmDeclaration([declaredName, value]) {
  const specifier = String(value);
  if (deniedNpmManifestDeps.has(declaredName)) return declaredName;
  const alias = /^npm:((?:@[^/]+\/)?[^@]+)(?:@|$)/.exec(specifier)?.[1];
  return alias && deniedNpmManifestDeps.has(alias) ? `${declaredName} -> ${alias}` : null;
}

function checkPackage(file) {
  const pkg = readJson(file);
  const bad = getPackageDeps(pkg).map(getDeniedNpmDeclaration).filter(Boolean);
  if (bad.length > 0) {
    throw new Error(`${path.relative(root, file)} contains outbound/network dependency: ${bad.join(", ")}`);
  }
}

function stripTomlComment(line) {
  let quote = null;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (quote) {
      if (character === quote && line[index - 1] !== "\\") quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "#") {
      return line.slice(0, index);
    }
  }
  return line;
}

function findCargoDependencyDeclarations(text, deniedNames) {
  const findings = new Set();
  let dependencySection = false;
  let namedDependency = null;
  let pending = "";
  for (const rawLine of text.split(/\r?\n/)) {
    const line = stripTomlComment(rawLine).trim();
    if (!line) continue;
    const section = /^\[([^\]]+)\]$/.exec(line);
    if (section) {
      const sectionName = section[1];
      const match = /^(?:(?:workspace|target\.[^.]+|target\.'[^']+'|target\."[^"]+")\.)?(?:dev-|build-)?dependencies(?:\.(?:"([^"]+)"|'([^']+)'|([A-Za-z0-9_-]+)))?$/.exec(sectionName);
      dependencySection = Boolean(match);
      namedDependency = match ? (match[1] ?? match[2] ?? match[3] ?? null) : null;
      pending = "";
      if (namedDependency && deniedNames.has(namedDependency)) findings.add(namedDependency);
      continue;
    }
    if (!dependencySection) continue;
    pending = pending ? `${pending} ${line}` : line;
    const braceDelta = [...pending].reduce((count, character) => count + (character === "{" ? 1 : character === "}" ? -1 : 0), 0);
    if (braceDelta > 0) continue;
    const declaration = /^(?:"([^"]+)"|'([^']+)'|([A-Za-z0-9_-]+))\s*=\s*([\s\S]+)$/.exec(pending);
    if (declaration) {
      const declaredName = namedDependency ?? declaration[1] ?? declaration[2] ?? declaration[3];
      if (deniedNames.has(declaredName)) findings.add(declaredName);
      const value = declaration[4];
      const declarationKey = declaration[1] ?? declaration[2] ?? declaration[3];
      const renamedPackage = namedDependency && declarationKey === "package"
        ? /^["']([^"']+)["']/.exec(value)?.[1]
        : /\bpackage\s*=\s*["']([^"']+)["']/.exec(value)?.[1];
      if (renamedPackage && deniedNames.has(renamedPackage)) findings.add(`${declaredName} -> ${renamedPackage}`);
    }
    pending = "";
  }
  return [...findings];
}

function assertExactConnectorManifestContract() {
  const expectedDirectDependencies = {
    reqwest: {
      version: "0.12",
      defaultFeatures: false,
      features: ["blocking", "json", "native-tls"]
    },
    url: { version: "2" },
    keyring: {
      version: "3",
      defaultFeatures: false,
      features: ["windows-native"]
    }
  };
  if (JSON.stringify(connectorOutbound.directDependencies) !== JSON.stringify(expectedDirectDependencies)) {
    throw new Error("The Connector provider direct-dependency policy drifted.");
  }
  const manifestFile = path.join(root, connectorOutbound.manifest);
  const apiManifestFile = path.join(root, connectorOutbound.apiManifest);
  if (!fs.existsSync(manifestFile) || !fs.existsSync(apiManifestFile)) {
    throw new Error("The Connector outbound crate or its feature-gated API edge is missing.");
  }
  const manifestText = fs.readFileSync(manifestFile, "utf8");
  const declarationLines = (name) => manifestText
    .split(/\r?\n/)
    .map((line) => stripTomlComment(line).trim())
    .filter((line) => line.startsWith(`${name} =`));
  const expectedLines = new Map([
    ["reqwest", 'reqwest={version="0.12",default-features=false,features=["blocking","json","native-tls"]}'],
    ["url", 'url="2"'],
    ["keyring", 'keyring={version="3",default-features=false,features=["windows-native"]}']
  ]);
  for (const [name, expected] of expectedLines) {
    const lines = declarationLines(name);
    if (lines.length !== 1 || lines[0].replace(/\s+/g, "") !== expected) {
      throw new Error(`${connectorOutbound.manifest} must declare exactly: ${expected}`);
    }
  }

  const apiText = fs.readFileSync(apiManifestFile, "utf8");
  const apiDeclarationLines = apiText
    .split(/\r?\n/)
    .map((line) => stripTomlComment(line).trim())
    .filter((line) => line.startsWith("hangar-ai ="));
  if (
    apiDeclarationLines.length !== 1
    || apiDeclarationLines[0].replace(/\s+/g, " ") !== connectorOutbound.apiDependencyDeclaration
  ) {
    throw new Error("hangar-api must contain the one exact optional path edge to hangar-ai.");
  }
  const featureMatch = /^agent_automation\s*=\s*\[([^\]]*)\]/m.exec(apiText);
  const featureMembers = featureMatch
    ? [...featureMatch[1].matchAll(/["']([^"']+)["']/g)].map((match) => match[1])
    : [];
  if (
    featureMembers.filter((member) => member === connectorOutbound.apiFeatureMember).length !== 1
    || (apiText.match(/dep:hangar-ai/g) ?? []).length !== 1
  ) {
    throw new Error("hangar-ai must be activated exactly once, only by hangar-api/agent_automation.");
  }
}

function isAllowedConnectorDeclaration(relative, finding) {
  if (relative === connectorManifestPath && ["reqwest", "url", "keyring"].includes(finding)) return true;
  if (relative === connectorApiManifestPath && finding === "hangar-ai") return true;
  return false;
}

function checkCargo(file) {
  const text = fs.readFileSync(file, "utf8");
  const relative = path.normalize(path.relative(root, file));
  const checkedNames = new Set([...deniedCargoManifestDeps, ...connectorExclusiveManifestDeps]);
  const bad = findCargoDependencyDeclarations(text, checkedNames)
    .filter((finding) => !isAllowedConnectorDeclaration(relative, finding));
  if (bad.length > 0) {
    throw new Error(`${path.relative(root, file)} contains outbound/network crate: ${bad.join(", ")}`);
  }
  const directCrypto = findCargoDependencyDeclarations(text, sqlCipherAtRestCryptoDeps);
  if (directCrypto.length > 0) {
    throw new Error(`${path.relative(root, file)} declares SQLCipher crypto crate directly; keep ${directCrypto.join(", ")} transitive via rusqlite/libsqlite3-sys.`);
  }
}

// Executable manifest fixtures prevent aliases/renames and target-specific
// declarations from bypassing the static half of this guard.
for (const [fixture, expected] of [
  [{ dependencies: { transport: "npm:@tauri-apps/plugin-http@2.0.0" } }, "transport -> @tauri-apps/plugin-http"],
  [{ devDependencies: { adapter: "npm:axios@1.0.0" } }, "adapter -> axios"],
  [{ optionalDependencies: { "@tauri-apps/plugin-http": "2.0.0" } }, "@tauri-apps/plugin-http"]
]) {
  const findings = getPackageDeps(fixture).map(getDeniedNpmDeclaration).filter(Boolean);
  if (!findings.includes(expected)) throw new Error(`NPM alias negative fixture did not reject ${expected}.`);
}
if (
  !isAllowedConnectorDeclaration(connectorManifestPath, "reqwest")
  || isAllowedConnectorDeclaration(connectorManifestPath, "transport -> reqwest")
  || isAllowedConnectorDeclaration(path.normalize("crates/other/Cargo.toml"), "reqwest")
  || !isAllowedConnectorDeclaration(connectorApiManifestPath, "hangar-ai")
  || isAllowedConnectorDeclaration(connectorApiManifestPath, "provider -> hangar-ai")
) {
  throw new Error("The Connector manifest exception escaped its exact path and unrenamed dependency names.");
}
for (const [fixture, expected] of [
  ['[dependencies]\ntransport = { package = "reqwest", version = "1" }', "transport -> reqwest"],
  ['[target.\'cfg(windows)\'.dependencies.transport]\npackage = "hyper"\nversion = "1"', "transport -> hyper"],
  ['[workspace.dependencies]\nweb = {\n package = "ureq",\n version = "1"\n}', "web -> ureq"],
  ['[dependencies]\n"transport" = { package = "reqwest", version = "1" }', "transport -> reqwest"],
  ['[dependencies."transport"]\npackage = "hyper"\nversion = "1"', "transport -> hyper"],
  ['[dev-dependencies]\ntauri-plugin-http = "2"', "tauri-plugin-http"]
]) {
  const findings = findCargoDependencyDeclarations(fixture, deniedCargoManifestDeps);
  if (!findings.includes(expected)) throw new Error(`Cargo rename negative fixture did not reject ${expected}.`);
}

assertExactConnectorManifestContract();
for (const file of walk(root)) {
  if (file.endsWith("package.json")) checkPackage(file);
  if (file.endsWith("Cargo.toml")) checkCargo(file);
}

// The application itself is offline, and the installer must be too. Tauri's default
// `downloadBootstrapper` attempts to fetch WebView2 during setup; on a clean,
// network-disabled Windows Sandbox it aborts before installing the Local edition.
// Both editions inherit this base bundle setting.
const tauriConfig = readJson(path.join(root, "apps/desktop/src-tauri/tauri.conf.json"));
if (JSON.stringify(tauriConfig.bundle?.targets) !== JSON.stringify(["nsis"])) {
  throw new Error("The supported desktop bundle target must remain Windows NSIS only.");
}
const webviewInstallMode = tauriConfig.bundle?.windows?.webviewInstallMode;
if (webviewInstallMode?.type !== "offlineInstaller" || webviewInstallMode?.silent !== true) {
  throw new Error(
    "Both Windows installers must embed the silent offline WebView2 installer; runtime setup may not require outbound access."
  );
}
if (
  tauriConfig.bundle?.createUpdaterArtifacts !== undefined &&
  tauriConfig.bundle.createUpdaterArtifacts !== false
) {
  throw new Error("The base Tauri configuration may not create updater artifacts.");
}

// The tracked configuration deliberately remains `offlineInstaller`: a direct,
// non-canonical Tauri invocation must not silently omit the runtime. Canonical
// packaging alone generates a last-applied `skip` override and a pinned NSIS
// preinstall hook. Audit both the tracked hook and that generator here so an
// online/update path cannot be introduced between release preflights.
const webViewManifest = readJson(path.join(root, "scripts/release-inputs/webview2-x64.json"));
const expectedWebViewManifest = {
  schemaVersion: 1,
  filename: "MicrosoftEdgeWebView2RuntimeInstallerX64.exe",
  length: 203654864,
  sha256: "3A08103BED8A3D9AEFDFC9AC10A672EA69605163F2DCB08D76CFD3E0444511C9",
  fileVersion: "1.3.241.15",
  peMachine: "014C",
  signerSubject: "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US",
  signerThumbprint: "4028CAD637509D4744B17EC5B42AED8D7A31E6AF",
  signerIssuer: "CN=Microsoft Code Signing PCA 2024, O=Microsoft Corporation, C=US",
  timestampThumbprint: "536FE6CA38F0230817E5873C3911706E496C5E0E"
};
if (JSON.stringify(webViewManifest) !== JSON.stringify(expectedWebViewManifest)) {
  throw new Error("The tracked WebView2 release-input manifest drifted from the independently audited pin.");
}

const trackedNsisHookPath = path.join(
  root,
  "apps/desktop/src-tauri/windows/shell-integration.nsh"
);
const trackedNsisHook = fs.readFileSync(trackedNsisHookPath, "utf8");
const packagingCommonPath = path.join(root, "scripts/packaging-common.ps1");
const packagingCommon = fs.readFileSync(packagingCommonPath, "utf8");
const hookTemplateMatches = [
  ...packagingCommon.matchAll(/\$hookTemplate\s*=\s*@'([\s\S]*?)\r?\n'@/g)
];
if (hookTemplateMatches.length !== 1) {
  throw new Error("The canonical packaging script must contain exactly one generated NSIS hook template.");
}
const generatedNsisHookTemplate = hookTemplateMatches[0][1];
const forbiddenHookTokens = [
  /https?:\/\//i,
  /downloadBootstrapper/i,
  /EdgeUpdate/i,
  /Invoke-WebRequest/i,
  /Start-BitsTransfer/i,
  /\bcurl(?:\.exe)?\b/i,
  /\bwget(?:\.exe)?\b/i
];
for (const [label, hook] of [
  ["tracked NSIS hook", trackedNsisHook],
  ["generated pinned-WebView2 hook template", generatedNsisHookTemplate]
]) {
  const forbidden = forbiddenHookTokens.find((pattern) => pattern.test(hook));
  if (forbidden) {
    throw new Error(`${label} contains a forbidden outbound/download token: ${forbidden}`);
  }
}
if (
  !/!ifndef\s+CODEHANGAR_PINNED_WEBVIEW2_READY[\s\S]*?!error/i.test(trackedNsisHook) ||
  (trackedNsisHook.match(/!macro\s+NSIS_HOOK_PREINSTALL\b/g) ?? []).length !== 1 ||
  !/!macro\s+NSIS_HOOK_PREINSTALL\b\s*!insertmacro\s+CODEHANGAR_INSTALL_PINNED_WEBVIEW2/i.test(
    trackedNsisHook
  )
) {
  throw new Error("The tracked NSIS hook no longer fails closed or installs pinned WebView2 first.");
}
for (const marker of [
  "!define CODEHANGAR_PINNED_WEBVIEW2_READY 1",
  "File /oname=$PLUGINSDIR\\__FILENAME__",
  "SetEnvironmentVariableW(w \"CODEHANGAR_PINNED_WEBVIEW2_PATH\"",
  "[Environment]::GetEnvironmentVariable('CODEHANGAR_PINNED_WEBVIEW2_PATH','Process')",
  "[System.IO.File]::OpenRead",
  "[System.Security.Cryptography.SHA256]::Create",
  "[Console]::Out.Write($$actual)",
  "__SHA256__",
  "DetailPrint \"Pinned WebView2 extracted SHA256 verified: $1\"",
  "ExecWait",
  "/silent /install",
  "Abort"
]) {
  if (!generatedNsisHookTemplate.includes(marker)) {
    throw new Error(`The generated pinned-WebView2 hook is missing security marker: ${marker}`);
  }
}
if (/Get-FileHash|Import-Module|PSModulePath|\/reboot/i.test(generatedNsisHookTemplate)) {
  throw new Error("The generated WebView2 hook may not use module-resolved hashing or automatic reboot flags.");
}
for (const marker of [
  "createUpdaterArtifacts = $false",
  "minimumWebview2Version = $null",
  'webviewInstallMode = [ordered]@{ type = "skip" }',
  "nsis = [ordered]@{ installerHooks = $hookPath }"
]) {
  if (!packagingCommon.includes(marker)) {
    throw new Error(`The canonical last-applied Tauri override is missing: ${marker}`);
  }
}

// Connector-only packages are compile-absent from Local. The Connector may link exactly the
// reviewed hangar-ai provider edge; MCP remains stdio and automation remains a named pipe.
const connectorPackages = [
  "hangar-agent",
  "hangar-ai",
  "keyring",
  // Connected-AI-app surface — feature-gated and never in the Local edition.
  // Named explicitly so the guard enforces what SECURITY_INVARIANTS.md promises,
  // rather than relying on transitive coverage from the hangar-agent denial.
  "hangar-appconfig",
  "hangar-mcp",
  "code-hangar-mcp"
];

// Assert a selected Cargo dependency tree contains no denied outbound-network crate and none of
// `forbiddenPackages`. SQLCipher at-rest crypto is allowed only transitively via rusqlite.
function assertInverseTreeConfinedToHangarAi(label, packageSpec, cargoArguments) {
  const inverse = spawnSync(
    "cargo",
    [
      "tree", "--locked", "--offline", "--target", supportedRustTarget,
      "-e", "all", ...cargoArguments, "-i", packageSpec, "--prefix", "depth", "--no-dedupe"
    ],
    { cwd: root, encoding: "utf8", env: cargoEnvironment }
  );
  if (inverse.status !== 0) {
    throw new Error(`cargo inverse tree failed for ${label}/${packageSpec}:\n${inverse.stderr}`);
  }
  const stack = [];
  let selectedRootCount = 0;
  for (const line of inverse.stdout.split(/\r?\n/)) {
    const match = /^(\d+)([A-Za-z0-9_.-]+)\s+v/.exec(line.trim());
    if (!match) continue;
    const depth = Number(match[1]);
    const name = match[2];
    stack.length = depth;
    stack[depth] = name;
    if (["code-hangar-desktop", "code-hangar-mcp"].includes(name)) {
      selectedRootCount += 1;
      if (!stack.slice(1, depth).includes("hangar-ai")) {
        throw new Error(`${label} contains ${packageSpec} on a path that bypasses the one allowlisted hangar-ai crate.`);
      }
    }
  }
  if (selectedRootCount === 0) {
    throw new Error(`${label} inverse tree for ${packageSpec} did not reach its selected shipped root.`);
  }
}

function checkCargoTree(label, cargoArguments, forbiddenPackages, { connectorProviderAllowed = false } = {}) {
  const cargoTree = spawnSync(
    "cargo",
    [
      "tree", "--locked", "--offline", "--target", supportedRustTarget,
      "-e", "all", ...cargoArguments, "--prefix", "none"
    ],
    { cwd: root, encoding: "utf8", env: cargoEnvironment }
  );
  if (cargoTree.error?.code === "ENOENT") {
    console.warn("Cargo not found; skipped active Rust dependency tree deny check.");
    return;
  }
  if (cargoTree.status !== 0) {
    throw new Error(`cargo tree failed while checking ${label} dependencies:\n${cargoTree.stderr}`);
  }
  const activeRustPackageVersions = new Map();
  for (const line of cargoTree.stdout.split(/\r?\n/)) {
    const match = /^([A-Za-z0-9_.-]+)\s+v([^\s]+)/.exec(line.trim());
    if (!match) continue;
    const versions = activeRustPackageVersions.get(match[1]) ?? new Set();
    versions.add(match[2]);
    activeRustPackageVersions.set(match[1], versions);
  }
  const activeRustPackages = new Set(activeRustPackageVersions.keys());
  const connectorTransportClosure = new Set(["reqwest", "hyper", "native-tls", "openssl"]);
  const bad = [...deniedCargoManifestDeps].filter((dep) =>
    activeRustPackages.has(dep) && !(connectorProviderAllowed && connectorTransportClosure.has(dep))
  );
  if (bad.length > 0) {
    throw new Error(`${label} dependency tree contains denied outbound/network crates: ${bad.join(", ")}`);
  }
  if (connectorProviderAllowed) {
    const reqwestVersions = [...(activeRustPackageVersions.get("reqwest") ?? [])];
    const keyringVersions = [...(activeRustPackageVersions.get("keyring") ?? [])];
    if (
      !activeRustPackages.has("hangar-ai")
      || reqwestVersions.length !== 1
      || !/^0\.12\./.test(reqwestVersions[0])
      || keyringVersions.length !== 1
      || !/^3\./.test(keyringVersions[0])
    ) {
      throw new Error(`${label} does not contain the exact feature-gated hangar-ai/reqwest 0.12/keyring 3 provider graph.`);
    }
    for (const packageName of connectorTransportClosure) {
      for (const version of activeRustPackageVersions.get(packageName) ?? []) {
        assertInverseTreeConfinedToHangarAi(label, `${packageName}@${version}`, cargoArguments);
      }
    }
  }
  const forbidden = forbiddenPackages.filter((dep) => activeRustPackages.has(dep));
  if (forbidden.length > 0) {
    throw new Error(`${label} dependency tree contains forbidden feature-gated packages: ${forbidden.join(", ")}`);
  }
  const activeSqlCipherCrypto = [...sqlCipherAtRestCryptoDeps].filter((dep) => activeRustPackages.has(dep));
  if (activeSqlCipherCrypto.length > 0 && !(activeRustPackages.has("rusqlite") && activeRustPackages.has("libsqlite3-sys"))) {
    throw new Error(`SQLCipher at-rest crypto crates are present without rusqlite/libsqlite3-sys: ${activeSqlCipherCrypto.join(", ")}`);
  }
  if (activeSqlCipherCrypto.length > 0) {
    console.log(`[${label}] Allowed SQLCipher at-rest crypto dependencies: ${activeSqlCipherCrypto.join(", ")}.`);
  }
}

function checkAllTargetTauriMobileException() {
  const metadataResult = spawnSync(
    "cargo",
    ["metadata", "--locked", "--offline", "--format-version", "1"],
    { cwd: root, encoding: "utf8", env: cargoEnvironment, maxBuffer: 64 * 1024 * 1024 }
  );
  if (metadataResult.error?.code === "ENOENT") return;
  if (metadataResult.error) {
    throw new Error(`cargo metadata could not be completed: ${metadataResult.error.message}`);
  }
  if (metadataResult.status !== 0) {
    throw new Error(`cargo metadata failed while auditing the all-target exception:\n${metadataResult.stderr}`);
  }
  const metadata = JSON.parse(metadataResult.stdout);
  const reqwestPackages = metadata.packages.filter(({ name }) => name === "reqwest");
  const mobileReqwestPackages = reqwestPackages.filter(({ version }) => version === "0.13.4");
  const providerReqwestPackages = reqwestPackages.filter(({ version }) => /^0\.12\./.test(version));
  if (reqwestPackages.length !== 2 || mobileReqwestPackages.length !== 1 || providerReqwestPackages.length !== 1) {
    throw new Error("All-target metadata contains an unreviewed reqwest package/version outside Connector 0.12 and Tauri-mobile 0.13.4.");
  }

  const resolvedParentsFor = (packageId) => {
    const parents = [];
    for (const node of metadata.resolve?.nodes ?? []) {
      for (const dependency of node.deps ?? []) {
        if (dependency.pkg === packageId) {
          parents.push({ parentId: node.id, dependency });
        }
      }
    }
    return parents;
  };
  const mobileReqwest = mobileReqwestPackages[0];
  const providerReqwest = providerReqwestPackages[0];
  const mobileResolvedParents = resolvedParentsFor(mobileReqwest.id);
  const providerResolvedParents = resolvedParentsFor(providerReqwest.id);
  const tauriPackage = metadata.packages.find(
    ({ name, version }) => name === "tauri" && version === "2.11.2"
  );
  const hangarAiPackage = metadata.packages.find(({ name }) => name === "hangar-ai");
  const mobileTarget = 'cfg(any(target_os = "android", all(target_vendor = "apple", not(target_os = "macos"))))';
  if (
    !tauriPackage
    || mobileResolvedParents.length !== 1
    || mobileResolvedParents[0].parentId !== tauriPackage.id
    || mobileResolvedParents[0].dependency.name !== "reqwest"
    || mobileResolvedParents[0].dependency.dep_kinds?.length !== 1
    || mobileResolvedParents[0].dependency.dep_kinds[0].kind !== null
    || mobileResolvedParents[0].dependency.dep_kinds[0].target !== mobileTarget
  ) {
    throw new Error("reqwest is no longer confined to Tauri's reviewed Android/non-macOS-Apple dependency edge.");
  }
  if (
    !hangarAiPackage
    || providerResolvedParents.length !== 1
    || providerResolvedParents[0].parentId !== hangarAiPackage.id
    || providerResolvedParents[0].dependency.name !== "reqwest"
    || providerResolvedParents[0].dependency.dep_kinds?.length !== 1
    || providerResolvedParents[0].dependency.dep_kinds[0].kind !== null
    || providerResolvedParents[0].dependency.dep_kinds[0].target !== null
  ) {
    throw new Error("reqwest 0.12 is no longer confined to the one feature-gated hangar-ai provider edge.");
  }

  const tauriReqwest = tauriPackage.dependencies.filter(({ name }) => name === "reqwest");
  if (
    tauriReqwest.length !== 1
    || tauriReqwest[0].target !== mobileTarget
    || tauriReqwest[0].req !== "^0.13"
    || tauriReqwest[0].uses_default_features !== false
    || tauriReqwest[0].optional !== false
    || JSON.stringify(tauriReqwest[0].features) !== JSON.stringify(["json", "stream"])
  ) {
    throw new Error("Tauri's reviewed mobile-only reqwest declaration drifted.");
  }

  const inverseTree = spawnSync(
    "cargo",
    ["tree", "--locked", "--offline", "--target", "all", "-e", "all", "-i", "reqwest@0.13.4", "--prefix", "none"],
    { cwd: root, encoding: "utf8", env: cargoEnvironment }
  );
  if (inverseTree.status !== 0) {
    throw new Error(`cargo tree --target all failed while auditing reqwest:\n${inverseTree.stderr}`);
  }
  const reqwestRoots = inverseTree.stdout.match(/^reqwest\s+v[^\r\n]+/gm) ?? [];
  if (
    reqwestRoots.length !== 1
    || reqwestRoots[0] !== "reqwest v0.13.4"
    || !/^tauri v2\.11\.2(?: \(\*\))?$/m.test(inverseTree.stdout)
  ) {
    throw new Error("The all-target inverse reqwest tree drifted from the reviewed Tauri mobile-only exception.");
  }
  console.log(
    `[all-target audit] reqwest ${providerReqwest.version} is confined to feature-gated hangar-ai; reqwest 0.13.4 remains only on Tauri 2.11.2's unsupported Android/non-macOS-Apple edge.`
  );
}

function checkEditionTree(featureName, forbiddenPackages, options = {}) {
  const featureArgs = options.packagingDefaults
    ? ["--features", featureName]
    : ["--no-default-features", "--features", featureName];
  checkCargoTree(
    `'${featureName}' desktop edition`,
    ["-p", "code-hangar-desktop", ...featureArgs],
    forbiddenPackages,
    options
  );
}

if (!staticOnly) {
  // `core`: the strictest proof — read-only, no mutation either.
  checkEditionTree("core", ["hangar-mutation", ...connectorPackages]);
  // `mutation`: the shipped LOCAL edition. It CAN delete (hangar-mutation is allowed) but must stay
  // 100% local — no AI, no connector, and no outbound-network crate.
  checkEditionTree("mutation", connectorPackages, { packagingDefaults: true });
  // `agent_automation`: the Connector desktop itself. Only the reviewed hangar-ai provider
  // transport closure is allowed; telemetry, updater, remote Git and any second client remain denied.
  checkEditionTree("agent_automation", [], { connectorProviderAllowed: true, packagingDefaults: true });
  // The standalone MCP executable is packaged beside the Connector desktop and has its own root.
  // Enforce the same one-provider-edge policy over that complete graph too.
  checkCargoTree("'code-hangar-mcp' sidecar", ["-p", "code-hangar-mcp"], [], { connectorProviderAllowed: true });
  checkAllTargetTauriMobileException();
}

console.log(
  staticOnly
    ? "Static edition policy passed: Local has no direct provider declarations; Connector has one exact feature-gated hangar-ai edge with pinned client features; aliases, updater/download paths and noncanonical clients are denied."
    : `Active edition policy passed for supported ${supportedRustTarget} graphs: Local contains no hangar-ai/reqwest/keyring provider path; Connector outbound transport is confined to hangar-ai; Windows-only NSIS setup remains offline.`
);
