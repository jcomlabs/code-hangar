#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const PINNED_TAURI_CLI_VERSION = "2.11.2";
const TOOL_SPECS = [
  { packageName: "vite", commandName: "vite.cmd", entryPoint: "vite\\bin\\vite.js" },
  { packageName: "typescript", commandName: "tsc.cmd", entryPoint: "typescript\\bin\\tsc" },
  { packageName: "@tauri-apps/cli", commandName: "tauri.cmd", entryPoint: "@tauri-apps\\cli\\tauri.js" },
];
const FORBIDDEN_BUILD_ENV = [
  "NODE_PATH",
  "NODE_OPTIONS",
  "NAPI_RS_NATIVE_LIBRARY_PATH",
  "NAPI_RS_FORCE_WASI",
  "NAPI_RS_ENFORCE_VERSION_CHECK",
  "RUSTFLAGS",
  "RUSTDOCFLAGS",
  "RUSTC",
  "RUSTC_WRAPPER",
  "RUSTC_WORKSPACE_WRAPPER",
  "CARGO_ENCODED_RUSTFLAGS",
];
const NATIVE_SPECS = [
  {
    ownerPackage: "@tauri-apps/cli",
    packageName: "@tauri-apps/cli-win32-x64-msvc",
    binaryName: "cli.win32-x64-msvc.node",
    sha256: "6774afc8b70cdf4ccc536b5700c781eff2836c74d2555abab000444a63abb54b",
    fallbackPaths: ["node_modules/@tauri-apps/cli/cli.win32-x64-msvc.node"],
  },
  {
    ownerPackage: "rolldown",
    packageName: "@rolldown/binding-win32-x64-msvc",
    binaryName: "rolldown-binding.win32-x64-msvc.node",
    sha256: "51eddd073cfdd29e4ecea16bdcfee1572c4b45726871c5c35052a0859323eb95",
    fallbackPaths: ["node_modules/rolldown/dist/shared/rolldown-binding.win32-x64-msvc.node"],
  },
  {
    ownerPackage: "lightningcss",
    packageName: "lightningcss-win32-x64-msvc",
    binaryName: "lightningcss.win32-x64-msvc.node",
    sha256: "83e7e838ef1bbf5454c651103574fec31a1cec0b453b999d1173365cbc6c7841",
    fallbackPaths: ["node_modules/lightningcss/lightningcss.win32-x64-msvc.node"],
  },
];

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(`${label} is missing or invalid at ${path}: ${error.message}`);
  }
}

function assertNonEmptyFile(path, label) {
  if (!existsSync(path) || !lstatSync(path).isFile() || statSync(path).size <= 0) {
    throw new Error(`${label} is missing or empty: ${path}`);
  }
}

function assertInsideRoot(path, root, label) {
  const rootReal = realpathSync(root);
  const pathReal = realpathSync(path);
  const rel = relative(rootReal, pathReal);
  if (rel === "" || (!rel.startsWith("..") && !isAbsolute(rel))) {
    return pathReal;
  }
  throw new Error(`${label} resolves outside its required root: ${pathReal}`);
}

function readBasicPe(path, label) {
  const size = statSync(path).size;
  const handle = openSync(path, "r");
  try {
    const dos = Buffer.alloc(64);
    if (readSync(handle, dos, 0, dos.length, 0) !== dos.length || dos.readUInt16LE(0) !== 0x5a4d) {
      throw new Error(`${label} does not have a basic PE/MZ header: ${path}`);
    }
    const peOffset = dos.readUInt32LE(0x3c);
    if (peOffset < 0x40 || peOffset > size - 6) {
      throw new Error(`${label} has an invalid PE offset: ${path}`);
    }
    const pe = Buffer.alloc(6);
    if (readSync(handle, pe, 0, pe.length, peOffset) !== pe.length || pe.readUInt32LE(0) !== 0x00004550) {
      throw new Error(`${label} has an invalid PE signature: ${path}`);
    }
    return { machine: pe.readUInt16LE(4).toString(16).toUpperCase().padStart(4, "0") };
  } finally {
    closeSync(handle);
  }
}

function assertBasicPe(path, label) {
  readBasicPe(path, label);
}

function sha256File(path) {
  const hash = createHash("sha256");
  const handle = openSync(path, "r");
  const buffer = Buffer.alloc(1024 * 1024);
  try {
    let offset = 0;
    while (true) {
      const count = readSync(handle, buffer, 0, buffer.length, offset);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
      offset += count;
    }
    return hash.digest("hex");
  } finally {
    closeSync(handle);
  }
}

export function verifyNodeEnvironment(environment = process.env) {
  for (const name of FORBIDDEN_BUILD_ENV) {
    if (typeof environment[name] === "string" && environment[name].trim().length > 0) {
      throw new Error(`${name} must be empty for worktree-bound packaging`);
    }
  }
  for (const [name, value] of Object.entries(environment)) {
    if (/^TAURI_/i.test(name) && typeof value === "string" && value.trim().length > 0) {
      throw new Error(`${name} is a build-affecting TAURI_* override and must be empty for HOLD packaging`);
    }
    if (
      (/^CARGO_PROFILE_/i.test(name) || /^CARGO_TARGET_.+_(RUSTFLAGS|LINKER|RUNNER)$/i.test(name)) &&
      typeof value === "string" &&
      value.trim().length > 0
    ) {
      throw new Error(`${name} is a build-affecting Cargo override and must be empty for HOLD packaging`);
    }
  }
}

function extractWorkspaceVersion(cargoToml, cargoPath) {
  const header = /^\[workspace\.package\]\s*$/m.exec(cargoToml);
  if (!header) throw new Error(`[workspace.package] is missing from ${cargoPath}`);
  const bodyStart = header.index + header[0].length;
  const remainder = cargoToml.slice(bodyStart);
  const nextHeader = /^\[/m.exec(remainder);
  const body = nextHeader ? remainder.slice(0, nextHeader.index) : remainder;
  const versions = [...body.matchAll(/^version\s*=\s*"([^"]+)"\s*$/gm)];
  if (versions.length !== 1) {
    throw new Error(`[workspace.package] must contain exactly one quoted version in ${cargoPath}`);
  }
  return versions[0][1];
}

export function verifyVersionCoherence(repoRoot) {
  const rootPackage = readJson(join(repoRoot, "package.json"), "root package.json");
  const desktopPackage = readJson(
    join(repoRoot, "apps", "desktop", "package.json"),
    "desktop package.json",
  );
  const lock = readJson(join(repoRoot, "package-lock.json"), "package-lock.json");
  const tauriConfig = readJson(
    join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json"),
    "tauri.conf.json",
  );
  const connectorConfig = readJson(
    join(repoRoot, "apps", "desktop", "src-tauri", "tauri.connector.conf.json"),
    "tauri.connector.conf.json",
  );
  const cargoPath = join(repoRoot, "Cargo.toml");
  const cargoVersion = extractWorkspaceVersion(readFileSync(cargoPath, "utf8"), cargoPath);
  const desktopCargoPath = join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml");
  const desktopCargo = readFileSync(desktopCargoPath, "utf8");
  if (!/^version\.workspace\s*=\s*true\s*$/m.test(desktopCargo)) {
    throw new Error(`desktop Cargo package must inherit the workspace version: ${desktopCargoPath}`);
  }
  const versions = new Map([
    ["Cargo.toml [workspace.package]", cargoVersion],
    ["tauri.conf.json", tauriConfig.version],
    ["root package.json", rootPackage.version],
    ["desktop package.json", desktopPackage.version],
    ["package-lock.json top-level", lock.version],
    ["package-lock.json root package", lock.packages?.[""]?.version],
    ["package-lock.json desktop workspace", lock.packages?.["apps/desktop"]?.version],
  ]);
  for (const [label, version] of versions) {
    if (version !== cargoVersion) {
      throw new Error(`release version mismatch: ${label} has ${version ?? "no version"}, expected ${cargoVersion}`);
    }
  }
  if (tauriConfig.productName !== "Code Hangar") {
    throw new Error(`Local productName must be exactly 'Code Hangar', got '${tauriConfig.productName ?? ""}'`);
  }
  if (connectorConfig.productName !== "Code Hangar AI Connector") {
    throw new Error(
      `Connector productName must be exactly 'Code Hangar AI Connector', got '${connectorConfig.productName ?? ""}'`,
    );
  }
  if (connectorConfig.version !== undefined && connectorConfig.version !== cargoVersion) {
    throw new Error(`Connector Tauri override version must be absent or exactly ${cargoVersion}`);
  }
  if (tauriConfig.bundle?.windows?.webviewInstallMode?.type !== "offlineInstaller") {
    throw new Error("base Tauri config must declare webviewInstallMode.type=offlineInstaller");
  }
  for (const [label, config] of [
    ["base Tauri config", tauriConfig],
    ["Connector Tauri config", connectorConfig],
  ]) {
    if (config.build?.beforeBundleCommand !== undefined) {
      throw new Error(`${label} must not define beforeBundleCommand; BundleSigned is a no-compile, byte-preserving phase`);
    }
  }
  return {
    version: cargoVersion,
    localInstaller: `Code Hangar_${cargoVersion}_x64-setup.exe`,
    connectorInstaller: `Code Hangar AI Connector_${cargoVersion}_x64-setup.exe`,
  };
}

function verifyOwnerLoadsNativePackage(repoRoot, spec) {
  const ownerDir = join(repoRoot, "node_modules", ...spec.ownerPackage.split("/"));
  if (spec.ownerPackage === "@tauri-apps/cli") {
    const loader = readFileSync(join(ownerDir, "index.js"), "utf8");
    if (!loader.includes(`require('${spec.packageName}')`) || !loader.includes("NAPI_RS_NATIVE_LIBRARY_PATH")) {
      throw new Error(`Tauri loader does not prove use of ${spec.packageName}`);
    }
  } else if (spec.ownerPackage === "rolldown") {
    const sharedDir = join(ownerDir, "dist", "shared");
    const loaders = readdirSync(sharedDir).filter((name) => /^binding-.*\.mjs$/.test(name));
    if (loaders.length !== 1) throw new Error(`expected exactly one Rolldown native loader, found ${loaders.length}`);
    const loader = readFileSync(join(sharedDir, loaders[0]), "utf8");
    if (!loader.includes(`__require("${spec.packageName}")`) || !loader.includes("NAPI_RS_NATIVE_LIBRARY_PATH")) {
      throw new Error(`Rolldown loader does not prove use of ${spec.packageName}`);
    }
  } else if (spec.ownerPackage === "lightningcss") {
    const loader = readFileSync(join(ownerDir, "node", "index.js"), "utf8");
    if (
      !loader.includes("require(`lightningcss-${parts.join('-')}`)") ||
      !loader.includes("parts.push('msvc')")
    ) {
      throw new Error(`Lightning CSS loader does not prove use of ${spec.packageName}`);
    }
  }
}

function verifyNativeBindingsAgainstHashes(repoRoot, expectedHashes) {
  if (process.platform !== "win32" || process.arch !== "x64") {
    throw new Error(`release packaging requires win32/x64 Node, got ${process.platform}/${process.arch}`);
  }
  const lock = readJson(join(repoRoot, "package-lock.json"), "package-lock.json");
  const vitePackage = readJson(join(repoRoot, "node_modules", "vite", "package.json"), "vite package.json");
  const lockedVite = lock.packages?.["node_modules/vite"];
  for (const dependency of ["rolldown", "lightningcss"]) {
    if (
      typeof vitePackage.dependencies?.[dependency] !== "string" ||
      vitePackage.dependencies[dependency] !== lockedVite?.dependencies?.[dependency]
    ) {
      throw new Error(`Vite's ${dependency} dependency does not match its locked dependency declaration`);
    }
    const nestedDependency = join(repoRoot, "node_modules", "vite", "node_modules", dependency);
    if (existsSync(nestedDependency)) {
      throw new Error(`unverified Vite-local ${dependency} override is present: ${nestedDependency}`);
    }
  }
  const evidence = [];
  for (const spec of NATIVE_SPECS) {
    const ownerPackageJson = readJson(
      join(repoRoot, "node_modules", ...spec.ownerPackage.split("/"), "package.json"),
      `${spec.ownerPackage} package.json`,
    );
    const ownerLockVersion = lock.packages?.[`node_modules/${spec.ownerPackage}`]?.version;
    if (ownerPackageJson.version !== ownerLockVersion) {
      throw new Error(
        `${spec.ownerPackage} installed version ${ownerPackageJson.version ?? "missing"} does not equal ` +
          `its lock version ${ownerLockVersion ?? "missing"}`,
      );
    }
    const expectedVersion = ownerPackageJson.optionalDependencies?.[spec.packageName];
    const lockVersion = lock.packages?.[`node_modules/${spec.packageName}`]?.version;
    if (typeof expectedVersion !== "string" || lockVersion !== expectedVersion) {
      throw new Error(
        `${spec.packageName} lock version ${lockVersion ?? "missing"} does not equal ` +
          `${spec.ownerPackage}'s exact optional dependency ${expectedVersion ?? "missing"}`,
      );
    }
    const packageDir = join(repoRoot, "node_modules", ...spec.packageName.split("/"));
    const packageJsonPath = join(packageDir, "package.json");
    assertNonEmptyFile(packageJsonPath, `${spec.packageName} package metadata`);
    assertInsideRoot(packageDir, repoRoot, `${spec.packageName} installation`);
    const nativePackage = readJson(packageJsonPath, `${spec.packageName} package.json`);
    if (nativePackage.version !== lockVersion || nativePackage.main !== spec.binaryName) {
      throw new Error(
        `${spec.packageName} installed metadata does not match lock/main ` +
          `(${nativePackage.version ?? "missing"}, ${nativePackage.main ?? "missing"})`,
      );
    }
    const binaryPath = join(packageDir, spec.binaryName);
    assertNonEmptyFile(binaryPath, `${spec.packageName} native binary`);
    assertInsideRoot(binaryPath, repoRoot, `${spec.packageName} native binary`);
    assertBasicPe(binaryPath, `${spec.packageName} native binary`);
    const binaryHash = sha256File(binaryPath);
    if (binaryHash !== expectedHashes.get(spec.packageName)) {
      throw new Error(`${spec.packageName} native binary SHA-256 does not match its audited locked artifact`);
    }
    for (const fallback of spec.fallbackPaths) {
      const fallbackPath = join(repoRoot, ...fallback.split("/"));
      if (existsSync(fallbackPath)) {
        throw new Error(`unverified loader-local native fallback is present: ${fallbackPath}`);
      }
    }
    verifyOwnerLoadsNativePackage(repoRoot, spec);
    evidence.push({ packageName: spec.packageName, version: lockVersion, binaryPath, sha256: binaryHash });
  }
  return evidence;
}

export function verifyNativeBindings(repoRoot) {
  return verifyNativeBindingsAgainstHashes(
    repoRoot,
    new Map(NATIVE_SPECS.map((spec) => [spec.packageName, spec.sha256])),
  );
}

export function verifyWorkspaceShimIsolation(repoRoot) {
  for (const relativeBin of [
    ["apps", "desktop", "node_modules", ".bin"],
    ["apps", "node_modules", ".bin"],
  ]) {
    const binDir = join(repoRoot, ...relativeBin);
    if (!existsSync(binDir)) continue;
    if (!lstatSync(binDir).isDirectory()) throw new Error(`workspace shim path is not a real directory: ${binDir}`);
    assertInsideRoot(binDir, repoRoot, "workspace shim directory");
    const entries = readdirSync(binDir);
    if (entries.length > 0) {
      throw new Error(`beforeBuildCommand could resolve unverified workspace shims from ${binDir}: ${entries.join(", ")}`);
    }
  }
}

export function verifyToolchain(repoRoot) {
  const lockPath = join(repoRoot, "package-lock.json");
  const lock = readJson(lockPath, "package-lock.json");
  if (lock.lockfileVersion !== 3 || !lock.packages) {
    throw new Error(`package-lock.json must be lockfileVersion 3 with a packages map: ${lockPath}`);
  }

  const versions = [];
  for (const spec of TOOL_SPECS) {
    const lockKey = `node_modules/${spec.packageName}`;
    const lockedVersion = lock.packages[lockKey]?.version;
    if (typeof lockedVersion !== "string" || lockedVersion.length === 0) {
      throw new Error(`package-lock.json has no exact version for ${lockKey}`);
    }
    if (spec.packageName === "@tauri-apps/cli" && lockedVersion !== PINNED_TAURI_CLI_VERSION) {
      throw new Error(
        `@tauri-apps/cli must remain ${PINNED_TAURI_CLI_VERSION} until its local bundler evidence is re-audited`,
      );
    }

    const packageDir = join(repoRoot, "node_modules", ...spec.packageName.split("/"));
    const packageJsonPath = join(packageDir, "package.json");
    assertNonEmptyFile(packageJsonPath, `${spec.packageName} package metadata`);
    assertInsideRoot(packageDir, repoRoot, `${spec.packageName} installation`);
    const installedVersion = readJson(packageJsonPath, `${spec.packageName} package.json`).version;
    if (installedVersion !== lockedVersion) {
      throw new Error(
        `${spec.packageName} version mismatch: package-lock.json requires ${lockedVersion}, ` +
          `but this worktree has ${installedVersion ?? "no version"}`,
      );
    }

    const commandPath = join(repoRoot, "node_modules", ".bin", spec.commandName);
    assertNonEmptyFile(commandPath, `${spec.commandName} worktree command`);
    assertInsideRoot(commandPath, repoRoot, `${spec.commandName} command`);
    const shim = readFileSync(commandPath, "utf8");
    if (!shim.includes(`%dp0%\\..\\${spec.entryPoint}`)) {
      throw new Error(`${spec.commandName} does not target the expected worktree package entry point`);
    }
    versions.push({ packageName: spec.packageName, version: lockedVersion, commandPath });
  }
  const unexpectedBinNode = join(repoRoot, "node_modules", ".bin", "node.exe");
  if (existsSync(unexpectedBinNode)) {
    throw new Error(`worktree command shims would use an unverified local node.exe: ${unexpectedBinNode}`);
  }
  return versions;
}

const PINNED_BUNDLER_MARKERS = [
  "tauri-bundler/2.9.2",
  "nsis-3.11.zip",
  "EF7FF767E5CBD9EDD22ADD3A32C9B8F4500BB10D",
  "75197FEE3C6A814FE035788D1C34EAD39349B860",
  "https://go.microsoft.com/fwlink/?linkid=2099617",
  "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/",
  "Configurations are merged in the order they are provided",
];

const DEFAULT_WEBVIEW2_MANIFEST = Object.freeze({
  schemaVersion: 1,
  filename: "MicrosoftEdgeWebView2RuntimeInstallerX64.exe",
  length: 203654864,
  sha256: "3A08103BED8A3D9AEFDFC9AC10A672EA69605163F2DCB08D76CFD3E0444511C9",
  fileVersion: "1.3.241.15",
  peMachine: "014C",
  signerSubject: "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US",
  signerThumbprint: "4028CAD637509D4744B17EC5B42AED8D7A31E6AF",
  signerIssuer: "CN=Microsoft Code Signing PCA 2024, O=Microsoft Corporation, C=US",
  timestampThumbprint: "536FE6CA38F0230817E5873C3911706E496C5E0E",
});

function assertExactObject(actual, expected, label) {
  if (!actual || typeof actual !== "object" || Array.isArray(actual)) {
    throw new Error(`${label} must be an object`);
  }
  const actualKeys = Object.keys(actual).sort();
  const expectedKeys = Object.keys(expected).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
    throw new Error(`${label} fields are not exact: ${actualKeys.join(", ")}`);
  }
  for (const key of expectedKeys) {
    if (actual[key] !== expected[key]) {
      throw new Error(`${label}.${key} does not equal its audited pin`);
    }
  }
}

function verifyPinnedBundlerEvidence(repoRoot) {
  const nativeCli = join(
    repoRoot,
    "node_modules",
    "@tauri-apps",
    "cli-win32-x64-msvc",
    "cli.win32-x64-msvc.node",
  );
  assertNonEmptyFile(nativeCli, "pinned Tauri native CLI");
  const binary = readFileSync(nativeCli);
  for (const marker of PINNED_BUNDLER_MARKERS) {
    if (!binary.includes(Buffer.from(marker, "ascii"))) {
      throw new Error(`pinned Tauri native CLI does not contain expected local bundler evidence: ${marker}`);
    }
  }
}

function assertSchemaSupportsPinnedHook(repoRoot) {
  const schema = readJson(
    join(repoRoot, "node_modules", "@tauri-apps", "cli", "config.schema.json"),
    "pinned Tauri config schema",
  );
  const definitions = schema.definitions ?? {};
  const installModes = definitions.WebviewInstallMode?.oneOf ?? [];
  const supportsSkip = installModes.some((mode) => mode.properties?.type?.enum?.includes("skip"));
  if (!supportsSkip) throw new Error("pinned Tauri schema does not support webviewInstallMode.type=skip");
  if (!definitions.BundleConfig?.properties?.createUpdaterArtifacts) {
    throw new Error("pinned Tauri schema does not support bundle.createUpdaterArtifacts");
  }
  if (!definitions.WindowsConfig?.properties?.minimumWebview2Version) {
    throw new Error("pinned Tauri schema does not support windows.minimumWebview2Version");
  }
  if (!definitions.NsisConfig?.properties?.installerHooks) {
    throw new Error("pinned Tauri schema does not support windows.nsis.installerHooks");
  }
}

function assertNoOutboundHookTokens(text, label) {
  const denied = [
    /https?:\/\//i,
    /downloadBootstrapper/i,
    /EdgeUpdate/i,
    /Invoke-WebRequest/i,
    /Start-BitsTransfer/i,
    /\bcurl(?:\.exe)?\b/i,
    /\bwget(?:\.exe)?\b/i,
  ];
  for (const pattern of denied) {
    if (pattern.test(text)) throw new Error(`${label} contains forbidden outbound/update token ${pattern}`);
  }
}

function verifyBaseHook(baseHookPath) {
  const hook = readFileSync(baseHookPath, "utf8");
  assertNoOutboundHookTokens(hook, "tracked NSIS hook");
  if (!/!ifndef\s+CODEHANGAR_PINNED_WEBVIEW2_READY[\s\S]*?!error\s+"[^"]+"/m.test(hook)) {
    throw new Error("tracked NSIS hook has no compile-time canonical-WebView2 guard");
  }
  const preinstallMatches = hook.match(/!macro\s+NSIS_HOOK_PREINSTALL\b/g) ?? [];
  if (preinstallMatches.length !== 1) {
    throw new Error(`tracked NSIS hook must contain one NSIS_HOOK_PREINSTALL, found ${preinstallMatches.length}`);
  }
  const preinstall = /!macro\s+NSIS_HOOK_PREINSTALL\s*\r?\n([^\r\n]*)/.exec(hook);
  if (!preinstall || preinstall[1].trim() !== "!insertmacro CODEHANGAR_INSTALL_PINNED_WEBVIEW2") {
    throw new Error("pinned WebView2 macro must be the first NSIS_HOOK_PREINSTALL instruction");
  }
}

function verifyGeneratedHook(hookPath, baseHookPath, webviewPath, manifest) {
  const hook = readFileSync(hookPath, "utf8");
  assertNoOutboundHookTokens(hook, "generated NSIS hook");
  const helperMacros = hook.match(/!macro\s+CODEHANGAR_INSTALL_PINNED_WEBVIEW2\b/g) ?? [];
  if (helperMacros.length !== 1 || /!macro\s+NSIS_HOOK_PREINSTALL\b/.test(hook)) {
    throw new Error("generated hook must define one helper macro and no second NSIS_HOOK_PREINSTALL");
  }
  const requiredFragments = [
    "!define CODEHANGAR_PINNED_WEBVIEW2_READY 1",
    `File /oname=$PLUGINSDIR\\${manifest.filename} "${webviewPath}"`,
    `SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", w "$PLUGINSDIR\\${manifest.filename}")`,
    "[Environment]::GetEnvironmentVariable('CODEHANGAR_PINNED_WEBVIEW2_PATH','Process')",
    "[System.IO.File]::OpenRead($$path)",
    `SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", p 0)`,
    "[System.Security.Cryptography.SHA256]::Create()",
    "[Console]::Out.Write($$actual)",
    manifest.sha256,
    `DetailPrint "Pinned WebView2 extracted SHA256 verified: $1"`,
    `ExecWait \`"$PLUGINSDIR\\${manifest.filename}" /silent /install\` $0`,
    `!include "${baseHookPath}"`,
  ];
  for (const fragment of requiredFragments) {
    if (!hook.includes(fragment)) throw new Error(`generated NSIS hook is missing required fragment: ${fragment}`);
  }
  if (/Get-FileHash|Import-Module|PSModulePath/i.test(hook)) {
    throw new Error("generated NSIS hook may not resolve a PowerShell cmdlet/module for hashing");
  }
  const hashIndex = hook.indexOf("[System.Security.Cryptography.SHA256]::Create()");
  const executeIndex = hook.indexOf("ExecWait");
  if (hashIndex < 0 || executeIndex < 0 || hashIndex >= executeIndex) {
    throw new Error("generated NSIS hook does not hash the embedded runtime before execution");
  }
  if (!/Pop \$0[\s\S]*?\$\{If\} \$0 != 0[\s\S]*?Abort/.test(hook)) {
    throw new Error("generated NSIS hook does not abort on hash/runtime failure");
  }
}

function verifyGeneratedOverride(overridePath, hookPath) {
  const override = readJson(overridePath, "generated Tauri override");
  const expected = {
    bundle: {
      createUpdaterArtifacts: false,
      windows: {
        minimumWebview2Version: null,
        webviewInstallMode: { type: "skip" },
        nsis: { installerHooks: hookPath },
      },
    },
  };
  const raw = readFileSync(overridePath, "utf8");
  if (/https?:\/\//i.test(raw) || /downloadBootstrapper/i.test(raw) || /EdgeUpdate/i.test(raw)) {
    throw new Error("generated Tauri override contains an outbound/update path");
  }
  if (JSON.stringify(override) !== JSON.stringify(expected)) {
    throw new Error("generated Tauri override is not the exact skip/null/no-updater overlay");
  }
}

function verifyReleaseOverlay(repoRoot, edition) {
  const configName = edition === "connector"
    ? "tauri.release-connector.conf.json"
    : edition === "local" ? "tauri.release-local.conf.json" : null;
  if (!configName) throw new Error("unknown release-overlay edition");
  const overlayPath = join(repoRoot, "apps", "desktop", "src-tauri", configName);
  const overlay = readJson(overlayPath, `${edition} release Tauri overlay`);
  const externalBin = edition === "connector"
    ? ["binaries/code-hangar-elevated", "binaries/code-hangar-mcp"]
    : ["binaries/code-hangar-elevated"];
  const expected = {
    $schema: "https://schema.tauri.app/config/2",
    bundle: {
      externalBin,
      resources: {
        "binaries/code-hangar-release-manifest.json": "code-hangar-release-manifest.json",
      },
    },
  };
  if (JSON.stringify(overlay) !== JSON.stringify(expected)) {
    throw new Error(`${edition} release Tauri overlay does not install the exact helper/manifest set`);
  }
  return overlayPath;
}

function assertLockedAgainstWrite(path) {
  let handle;
  try {
    handle = openSync(path, "r+");
  } catch {
    return;
  } finally {
    if (handle !== undefined) closeSync(handle);
  }
  throw new Error("staged WebView2 input is not held by a write/delete-denying parent lock");
}

export function verifyExplicitWebView2Provenance({
  repoRoot,
  manifestPath,
  webviewPath,
  overridePath,
  hookPath,
  baseHookPath,
  edition,
  configOrder,
  expectedManifest = DEFAULT_WEBVIEW2_MANIFEST,
  requireParentLock = true,
}) {
  verifyPinnedBundlerEvidence(repoRoot);
  assertSchemaSupportsPinnedHook(repoRoot);
  const baseConfig = readJson(
    join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json"),
    "base Tauri config",
  );
  if (
    baseConfig.bundle?.windows?.webviewInstallMode?.type !== "offlineInstaller" ||
    baseConfig.bundle?.windows?.webviewInstallMode?.silent !== true
  ) {
    throw new Error("tracked base Tauri config must remain silent offlineInstaller");
  }

  const manifest = readJson(manifestPath, "pinned WebView2 manifest");
  assertExactObject(manifest, expectedManifest, "pinned WebView2 manifest");
  for (const [path, label] of [
    [webviewPath, "staged WebView2 input"],
    [overridePath, "generated Tauri override"],
    [hookPath, "generated NSIS hook"],
    [baseHookPath, "tracked NSIS hook"],
  ]) {
    assertNonEmptyFile(path, label);
  }
  assertInsideRoot(webviewPath, join(repoRoot, ".local", "packaging-generated"), "staged WebView2 input");
  assertInsideRoot(overridePath, join(repoRoot, ".local", "packaging-generated"), "generated Tauri override");
  assertInsideRoot(hookPath, join(repoRoot, ".local", "packaging-generated"), "generated NSIS hook");
  if (realpathSync(baseHookPath) !== realpathSync(join(repoRoot, "apps", "desktop", "src-tauri", "windows", "shell-integration.nsh"))) {
    throw new Error("base NSIS hook is not the tracked shell-integration.nsh");
  }
  if (webviewPath.includes("\n") || webviewPath.includes("\r") || /["$`]/.test(webviewPath)) {
    throw new Error("staged WebView2 path contains unsafe NSIS quoting characters");
  }
  if (webviewPath.split(/[\\/]/).pop() !== manifest.filename) {
    throw new Error("staged WebView2 input does not have the exact manifest filename");
  }
  if (lstatSync(webviewPath).isSymbolicLink()) throw new Error("staged WebView2 input may not be a symlink");
  if (statSync(webviewPath).size !== manifest.length) throw new Error("staged WebView2 length does not match manifest");
  if (sha256File(webviewPath).toUpperCase() !== manifest.sha256) throw new Error("staged WebView2 hash does not match manifest");
  if (readBasicPe(webviewPath, "staged WebView2 input").machine !== manifest.peMachine) {
    throw new Error("staged WebView2 PE Machine does not match manifest");
  }
  if (requireParentLock) assertLockedAgainstWrite(webviewPath);

  verifyBaseHook(baseHookPath);
  verifyGeneratedHook(hookPath, baseHookPath, webviewPath, manifest);
  verifyGeneratedOverride(overridePath, hookPath);
  const releaseOverlayPath = verifyReleaseOverlay(repoRoot, edition);
  const expectedOrder = edition === "connector"
    ? [
        join(repoRoot, "apps", "desktop", "src-tauri", "tauri.connector.conf.json"),
        releaseOverlayPath,
        overridePath,
      ]
    : edition === "local" ? [releaseOverlayPath, overridePath] : null;
  if (!expectedOrder || configOrder.length !== expectedOrder.length ||
      expectedOrder.some((value, index) => resolve(configOrder[index]) !== resolve(value))) {
    throw new Error("Tauri configuration order is not base -> Connector (if any) -> generated override last");
  }
  return { manifest, webviewPath, overridePath, hookPath, configOrder };
}

function writeFakePe(path, markers = []) {
  mkdirSync(dirname(path), { recursive: true });
  const header = Buffer.alloc(512);
  header[0] = 0x4d;
  header[1] = 0x5a;
  header.writeUInt32LE(0x80, 0x3c);
  header.writeUInt32LE(0x00004550, 0x80);
  header.writeUInt16LE(0x014c, 0x84);
  writeFileSync(path, Buffer.concat([header, Buffer.from(`\0${markers.join("\0")}`, "ascii")]));
}

function writeFixture(root) {
  const releaseVersion = "0.1.3";
  const toolVersions = new Map([
    ["vite", "8.1.5"],
    ["typescript", "5.9.3"],
    ["@tauri-apps/cli", "2.11.2"],
  ]);
  const packages = {
    "": { version: releaseVersion },
    "apps/desktop": { version: releaseVersion },
  };
  const binDir = join(root, "node_modules", ".bin");
  mkdirSync(binDir, { recursive: true });
  for (const spec of TOOL_SPECS) {
    const version = toolVersions.get(spec.packageName);
    const packageDir = join(root, "node_modules", ...spec.packageName.split("/"));
    mkdirSync(packageDir, { recursive: true });
    packages[`node_modules/${spec.packageName}`] = { version };
    writeFileSync(join(packageDir, "package.json"), JSON.stringify({ version }));
    writeFileSync(
      join(binDir, spec.commandName),
      `@echo off\r\n"%dp0%\\..\\${spec.entryPoint}" %*\r\n`,
    );
  }

  const viteDependencies = { rolldown: "~1.1.5", lightningcss: "^1.32.0" };
  packages["node_modules/vite"].dependencies = viteDependencies;
  writeFileSync(
    join(root, "node_modules", "vite", "package.json"),
    JSON.stringify({ version: "8.1.5", dependencies: viteDependencies }),
  );

  for (const [owner, version] of [["rolldown", "1.1.5"], ["lightningcss", "1.32.0"]]) {
    packages[`node_modules/${owner}`] = { version };
    mkdirSync(join(root, "node_modules", owner), { recursive: true });
  }
  const ownerPackages = new Map([
    ["@tauri-apps/cli", { version: "2.11.2", optionalDependencies: {} }],
    ["rolldown", { version: "1.1.5", optionalDependencies: {} }],
    ["lightningcss", { version: "1.32.0", optionalDependencies: {} }],
  ]);
  const expectedNativeHashes = new Map();
  for (const spec of NATIVE_SPECS) {
    const owner = ownerPackages.get(spec.ownerPackage);
    const nativeVersion = owner.version;
    owner.optionalDependencies[spec.packageName] = nativeVersion;
    packages[`node_modules/${spec.packageName}`] = { version: nativeVersion };
    const packageDir = join(root, "node_modules", ...spec.packageName.split("/"));
    mkdirSync(packageDir, { recursive: true });
    writeFileSync(
      join(packageDir, "package.json"),
      JSON.stringify({ version: nativeVersion, main: spec.binaryName }),
    );
    const binaryPath = join(packageDir, spec.binaryName);
    writeFakePe(
      binaryPath,
      spec.ownerPackage === "@tauri-apps/cli" ? PINNED_BUNDLER_MARKERS : [],
    );
    expectedNativeHashes.set(spec.packageName, sha256File(binaryPath));
  }
  for (const [owner, metadata] of ownerPackages) {
    const packageDir = join(root, "node_modules", ...owner.split("/"));
    writeFileSync(join(packageDir, "package.json"), JSON.stringify(metadata));
  }
  writeFileSync(
    join(root, "node_modules", "@tauri-apps", "cli", "index.js"),
    "const p = process.env.NAPI_RS_NATIVE_LIBRARY_PATH; require('@tauri-apps/cli-win32-x64-msvc');\n",
  );
  writeFileSync(
    join(root, "node_modules", "@tauri-apps", "cli", "config.schema.json"),
    JSON.stringify({
      definitions: {
        WebviewInstallMode: { oneOf: [{ properties: { type: { enum: ["skip"] } } }] },
        BundleConfig: { properties: { createUpdaterArtifacts: {} } },
        WindowsConfig: { properties: { minimumWebview2Version: {} } },
        NsisConfig: { properties: { installerHooks: {} } },
      },
    }),
  );
  const rolldownLoaderDir = join(root, "node_modules", "rolldown", "dist", "shared");
  mkdirSync(rolldownLoaderDir, { recursive: true });
  writeFileSync(
    join(rolldownLoaderDir, "binding-fixture.mjs"),
    'const p = process.env.NAPI_RS_NATIVE_LIBRARY_PATH; __require("@rolldown/binding-win32-x64-msvc");\n',
  );
  const lightningLoaderDir = join(root, "node_modules", "lightningcss", "node");
  mkdirSync(lightningLoaderDir, { recursive: true });
  writeFileSync(
    join(lightningLoaderDir, "index.js"),
    "const native = require(`lightningcss-${parts.join('-')}`); parts.push('msvc');\n",
  );

  writeFileSync(join(root, "package.json"), JSON.stringify({ version: releaseVersion }));
  const desktopDir = join(root, "apps", "desktop");
  mkdirSync(join(desktopDir, "src-tauri"), { recursive: true });
  writeFileSync(join(desktopDir, "package.json"), JSON.stringify({ version: releaseVersion }));
  writeFileSync(
    join(desktopDir, "src-tauri", "tauri.conf.json"),
    JSON.stringify({
      productName: "Code Hangar",
      version: releaseVersion,
      bundle: { windows: { webviewInstallMode: { type: "offlineInstaller", silent: true } } },
    }),
  );
  writeFileSync(
    join(desktopDir, "src-tauri", "tauri.connector.conf.json"),
    JSON.stringify({ productName: "Code Hangar AI Connector" }),
  );
  const releaseLocalOverlayPath = join(
    desktopDir,
    "src-tauri",
    "tauri.release-local.conf.json",
  );
  writeFileSync(
    releaseLocalOverlayPath,
    JSON.stringify({
      $schema: "https://schema.tauri.app/config/2",
      bundle: {
        externalBin: ["binaries/code-hangar-elevated"],
        resources: {
          "binaries/code-hangar-release-manifest.json": "code-hangar-release-manifest.json",
        },
      },
    }),
  );
  writeFileSync(
    join(desktopDir, "src-tauri", "tauri.release-connector.conf.json"),
    JSON.stringify({
      $schema: "https://schema.tauri.app/config/2",
      bundle: {
        externalBin: ["binaries/code-hangar-elevated", "binaries/code-hangar-mcp"],
        resources: {
          "binaries/code-hangar-release-manifest.json": "code-hangar-release-manifest.json",
        },
      },
    }),
  );
  writeFileSync(join(root, "Cargo.toml"), `[workspace.package]\nversion = "${releaseVersion}"\n`);
  writeFileSync(join(desktopDir, "src-tauri", "Cargo.toml"), "[package]\nversion.workspace = true\n");
  writeFileSync(
    join(root, "package-lock.json"),
    JSON.stringify({ name: "code-hangar", version: releaseVersion, lockfileVersion: 3, packages }),
  );

  const generatedDir = join(root, ".local", "packaging-generated", "webview2-local-fixture");
  mkdirSync(generatedDir, { recursive: true });
  const webviewPath = join(generatedDir, "MicrosoftEdgeWebView2RuntimeInstallerX64.exe");
  writeFakePe(webviewPath);
  const webviewManifest = {
    schemaVersion: 1,
    filename: "MicrosoftEdgeWebView2RuntimeInstallerX64.exe",
    length: statSync(webviewPath).size,
    sha256: sha256File(webviewPath).toUpperCase(),
    fileVersion: "1.2.3.4",
    peMachine: "014C",
    signerSubject: "CN=Fixture",
    signerThumbprint: "1111111111111111111111111111111111111111",
    signerIssuer: "CN=Fixture Issuer",
    timestampThumbprint: "2222222222222222222222222222222222222222",
  };
  const manifestPath = join(root, "scripts", "release-inputs", "webview2-x64.json");
  mkdirSync(dirname(manifestPath), { recursive: true });
  writeFileSync(manifestPath, JSON.stringify(webviewManifest));
  const baseHookPath = join(desktopDir, "src-tauri", "windows", "shell-integration.nsh");
  mkdirSync(dirname(baseHookPath), { recursive: true });
  writeFileSync(
    baseHookPath,
    "!ifndef CODEHANGAR_PINNED_WEBVIEW2_READY\n  !error \"canonical only\"\n!endif\n" +
      "!macro NSIS_HOOK_PREINSTALL\n  !insertmacro CODEHANGAR_INSTALL_PINNED_WEBVIEW2\n!macroend\n",
  );
  const hookPath = join(generatedDir, "codehangar-pinned-webview2.nsh");
  writeFileSync(
    hookPath,
    `!define CODEHANGAR_PINNED_WEBVIEW2_READY 1\n` +
      `!macro CODEHANGAR_INSTALL_PINNED_WEBVIEW2\n` +
      `  File /oname=$PLUGINSDIR\\${webviewManifest.filename} "${webviewPath}"\n` +
      `  System::Call 'kernel32::SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", w "$PLUGINSDIR\\${webviewManifest.filename}") i .r2'\n` +
      `  nsExec::ExecToStack \`$$path=[Environment]::GetEnvironmentVariable('CODEHANGAR_PINNED_WEBVIEW2_PATH','Process'); $$stream=[System.IO.File]::OpenRead($$path); $$sha=[System.Security.Cryptography.SHA256]::Create(); [Console]::Out.Write($$actual); ${webviewManifest.sha256}\`\n` +
      `  Pop $0\n  Pop $1\n  \${If} $0 != 0\n    Abort \"hash failure\"\n  \${EndIf}\n` +
      `  System::Call 'kernel32::SetEnvironmentVariableW(w "CODEHANGAR_PINNED_WEBVIEW2_PATH", p 0) i .r2'\n` +
      `  DetailPrint "Pinned WebView2 extracted SHA256 verified: $1"\n` +
      `  ExecWait \`"$PLUGINSDIR\\${webviewManifest.filename}" /silent /install\` $0\n` +
      `!macroend\n!include "${baseHookPath}"\n`,
  );
  const overridePath = join(generatedDir, "tauri.webview2.override.json");
  writeFileSync(
    overridePath,
    JSON.stringify({
      bundle: {
        createUpdaterArtifacts: false,
        windows: {
          minimumWebview2Version: null,
          webviewInstallMode: { type: "skip" },
          nsis: { installerHooks: hookPath },
        },
      },
    }),
  );
  return {
    expectedNativeHashes,
    provenance: {
      repoRoot: root,
      manifestPath,
      webviewPath,
      overridePath,
      hookPath,
      baseHookPath,
      edition: "local",
      configOrder: [releaseLocalOverlayPath, overridePath],
      expectedManifest: webviewManifest,
      requireParentLock: false,
    },
  };
}

function expectFailure(action, expectedText) {
  let message = "";
  try {
    action();
  } catch (error) {
    message = error instanceof Error ? error.message : String(error);
  }
  if (!message.includes(expectedText)) {
    throw new Error(`self-test expected failure containing '${expectedText}', received '${message}'`);
  }
}

function runSelfTest() {
  const fixtureRoot = mkdtempSync(join(tmpdir(), "codehangar-packaging-preflight-"));
  const expectedParent = realpathSync(tmpdir());
  const fixtureReal = realpathSync(fixtureRoot);
  if (dirname(fixtureReal) !== expectedParent || !fixtureReal.includes("codehangar-packaging-preflight-")) {
    throw new Error(`refusing unsafe self-test directory: ${fixtureReal}`);
  }
  try {
    const fixture = writeFixture(fixtureRoot);
    const { expectedNativeHashes } = fixture;
    verifyNodeEnvironment({});
    verifyToolchain(fixtureRoot);
    verifyVersionCoherence(fixtureRoot);
    verifyNativeBindingsAgainstHashes(fixtureRoot, expectedNativeHashes);
    verifyWorkspaceShimIsolation(fixtureRoot);
    verifyExplicitWebView2Provenance(fixture.provenance);

    const originalWebView = readFileSync(fixture.provenance.webviewPath);
    const alteredWebView = Buffer.from(originalWebView);
    alteredWebView[alteredWebView.length - 1] ^= 0xff;
    writeFileSync(fixture.provenance.webviewPath, alteredWebView);
    expectFailure(
      () => verifyExplicitWebView2Provenance(fixture.provenance),
      "staged WebView2 hash does not match manifest",
    );
    writeFileSync(fixture.provenance.webviewPath, originalWebView);
    expectFailure(
      () => verifyExplicitWebView2Provenance({
        ...fixture.provenance,
        webviewPath: join(dirname(fixture.provenance.webviewPath), "missing.exe"),
      }),
      "staged WebView2 input is missing or empty",
    );
    expectFailure(
      () => verifyExplicitWebView2Provenance({ ...fixture.provenance, configOrder: [] }),
      "configuration order",
    );

    for (const name of FORBIDDEN_BUILD_ENV) {
      expectFailure(() => verifyNodeEnvironment({ [name]: "1" }), `${name} must be empty`);
    }
    expectFailure(
      () => verifyNodeEnvironment({ TAURI_CONFIG: "untrusted.json" }),
      "TAURI_CONFIG is a build-affecting TAURI_* override",
    );
    expectFailure(
      () => verifyNodeEnvironment({ CARGO_PROFILE_RELEASE_LTO: "off" }),
      "CARGO_PROFILE_RELEASE_LTO is a build-affecting Cargo override",
    );

    const vitePackage = join(fixtureRoot, "node_modules", "vite", "package.json");
    writeFileSync(vitePackage, JSON.stringify({ version: "9.9.9" }));
    expectFailure(() => verifyToolchain(fixtureRoot), "version mismatch");
    writeFileSync(
      vitePackage,
      JSON.stringify({ version: "8.1.5", dependencies: { rolldown: "~1.1.5", lightningcss: "^1.32.0" } }),
    );

    const desktopPackage = join(fixtureRoot, "apps", "desktop", "package.json");
    writeFileSync(desktopPackage, JSON.stringify({ version: "9.9.9" }));
    expectFailure(() => verifyVersionCoherence(fixtureRoot), "release version mismatch");
    writeFileSync(desktopPackage, JSON.stringify({ version: "0.1.3" }));

    const baseTauriConfig = join(fixtureRoot, "apps", "desktop", "src-tauri", "tauri.conf.json");
    const originalBaseTauri = readJson(baseTauriConfig, "self-test base Tauri config");
    writeFileSync(
      baseTauriConfig,
      JSON.stringify({ ...originalBaseTauri, build: { beforeBundleCommand: "cargo build" } }),
    );
    expectFailure(
      () => verifyVersionCoherence(fixtureRoot),
      "must not define beforeBundleCommand",
    );
    writeFileSync(baseTauriConfig, JSON.stringify(originalBaseTauri));

    const tauriNativePackage = join(
      fixtureRoot,
      "node_modules",
      "@tauri-apps",
      "cli-win32-x64-msvc",
      "package.json",
    );
    writeFileSync(
      tauriNativePackage,
      JSON.stringify({ version: "9.9.9", main: "cli.win32-x64-msvc.node" }),
    );
    expectFailure(
      () => verifyNativeBindingsAgainstHashes(fixtureRoot, expectedNativeHashes),
      "installed metadata does not match lock/main",
    );
    writeFileSync(
      tauriNativePackage,
      JSON.stringify({ version: "2.11.2", main: "cli.win32-x64-msvc.node" }),
    );

    const tauriNativeBinary = join(
      fixtureRoot,
      "node_modules",
      "@tauri-apps",
      "cli-win32-x64-msvc",
      "cli.win32-x64-msvc.node",
    );
    const originalNative = readFileSync(tauriNativeBinary);
    const alteredNative = Buffer.from(originalNative);
    alteredNative[alteredNative.length - 1] ^= 0xff;
    writeFileSync(tauriNativeBinary, alteredNative);
    expectFailure(
      () => verifyNativeBindingsAgainstHashes(fixtureRoot, expectedNativeHashes),
      "native binary SHA-256 does not match",
    );
    writeFileSync(tauriNativeBinary, originalNative);

    const workspaceBin = join(fixtureRoot, "apps", "desktop", "node_modules", ".bin");
    mkdirSync(workspaceBin, { recursive: true });
    writeFileSync(join(workspaceBin, "vite.cmd"), "@echo off\r\n");
    expectFailure(
      () => verifyWorkspaceShimIsolation(fixtureRoot),
      "beforeBuildCommand could resolve unverified workspace shims",
    );
  } finally {
    rmSync(fixtureRoot, { recursive: true, force: true });
  }
  console.log("Packaging preflight self-test passed.");
}

function printToolchain(versions) {
  for (const tool of versions) {
    console.log(`${tool.packageName} ${tool.version}: ${tool.commandPath}`);
  }
}

function printNativeBindings(bindings) {
  for (const binding of bindings) {
    console.log(`${binding.packageName} ${binding.version}: ${binding.binaryPath} [sha256 ${binding.sha256}]`);
  }
}

function parseArguments(argv) {
  const flags = new Set();
  const values = new Map();
  const flagNames = new Set(["--self-test", "--toolchain", "--tauri"]);
  const valueNames = new Set([
    "--manifest",
    "--webview",
    "--override",
    "--hook",
    "--base-hook",
    "--edition",
    "--config-order",
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (flagNames.has(arg)) {
      if (flags.has(arg)) throw new Error(`duplicate argument: ${arg}`);
      flags.add(arg);
    } else if (valueNames.has(arg)) {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) throw new Error(`${arg} requires a value`);
      index += 1;
      const entries = values.get(arg) ?? [];
      entries.push(value);
      values.set(arg, entries);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return { flags, values };
}

function oneValue(parsed, name) {
  const entries = parsed.values.get(name) ?? [];
  if (entries.length !== 1) throw new Error(`${name} must be supplied exactly once`);
  return entries[0];
}

function main() {
  const parsed = parseArguments(process.argv.slice(2));
  const modes = ["--self-test", "--toolchain", "--tauri"].filter((name) => parsed.flags.has(name));
  if (modes.length !== 1) throw new Error("choose exactly one mode: --self-test, --toolchain or --tauri");
  if (parsed.flags.has("--self-test")) {
    if (parsed.values.size !== 0) throw new Error("--self-test cannot be combined with provenance arguments");
    runSelfTest();
    return;
  }

  verifyNodeEnvironment();
  const versions = verifyToolchain(REPO_ROOT);
  const release = verifyVersionCoherence(REPO_ROOT);
  const nativeBindings = verifyNativeBindings(REPO_ROOT);
  verifyWorkspaceShimIsolation(REPO_ROOT);
  printToolchain(versions);
  printNativeBindings(nativeBindings);
  console.log(`Release version: ${release.version}`);
  console.log(`Expected Local installer: ${release.localInstaller}`);
  console.log(`Expected Connector installer: ${release.connectorInstaller}`);
  if (parsed.flags.has("--tauri")) {
    const provenance = verifyExplicitWebView2Provenance({
      repoRoot: REPO_ROOT,
      manifestPath: oneValue(parsed, "--manifest"),
      webviewPath: oneValue(parsed, "--webview"),
      overridePath: oneValue(parsed, "--override"),
      hookPath: oneValue(parsed, "--hook"),
      baseHookPath: oneValue(parsed, "--base-hook"),
      edition: oneValue(parsed, "--edition").toLowerCase(),
      configOrder: parsed.values.get("--config-order") ?? [],
    });
    console.log(`Pinned WebView2: ${provenance.webviewPath}`);
    console.log(`Pinned WebView2 SHA256: ${provenance.manifest.sha256}`);
    console.log(`Generated Tauri override: ${provenance.overridePath}`);
    console.log("Explicit pinned-WebView2 packaging preflight passed.");
    return;
  }
  if (parsed.values.size !== 0) throw new Error("--toolchain does not accept provenance arguments");
  console.log("Worktree toolchain preflight passed (explicit WebView2 provenance was not requested).");
}

try {
  main();
} catch (error) {
  console.error(`Packaging preflight failed: ${error instanceof Error ? error.message : error}`);
  process.exitCode = 1;
}
