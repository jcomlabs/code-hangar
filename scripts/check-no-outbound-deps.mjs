import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = process.cwd();

const deniedManifestDeps = new Set([
  "axios",
  "got",
  "node-fetch",
  "undici",
  "ky",
  "reqwest",
  "hyper",
  "native-tls",
  "openssl",
  "ureq",
  "isahc",
  "surf",
  "hickory-resolver",
  "trust-dns-resolver",
  "tauri-plugin-updater",
  "@tauri-apps/plugin-updater",
  "sentry"
]);

// Allowed only as transitive dependencies of SQLCipher for local database
// encryption at rest. They must not be declared directly in Code Hangar
// manifests and do not permit TLS, HTTP, fetch, telemetry, or package access.
const sqlCipherAtRestCryptoDeps = new Set([
  "openssl-sys",
  "openssl-src"
]);

// The AI Connector edition's outbound HTTP client. Permitted to be DECLARED only in the
// connector-only `hangar-ai` crate, which the base build never links — proven below by the
// `--features core` tree check (it must contain neither `reqwest` nor `hangar-ai`). Every
// other denied crate (native-tls, openssl, updater, sentry, …) stays forbidden even there.
const aiCrateManifest = path.normalize("crates/hangar-ai/Cargo.toml");
const aiCrateAllowedNetworkDeps = new Set(["reqwest"]);

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
    ...Object.keys(pkg.dependencies ?? {}),
    ...Object.keys(pkg.devDependencies ?? {}),
    ...Object.keys(pkg.optionalDependencies ?? {})
  ];
}

function checkPackage(file) {
  const pkg = readJson(file);
  const bad = getPackageDeps(pkg).filter((dep) => deniedManifestDeps.has(dep));
  if (bad.length > 0) {
    throw new Error(`${path.relative(root, file)} contains outbound/network dependency: ${bad.join(", ")}`);
  }
}

function checkCargo(file) {
  const rel = path.normalize(path.relative(root, file));
  const text = fs.readFileSync(file, "utf8");
  let bad = [...deniedManifestDeps].filter((dep) => new RegExp(`(^|\\n)\\s*${dep.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*=`, "m").test(text));
  if (rel === aiCrateManifest) {
    bad = bad.filter((dep) => !aiCrateAllowedNetworkDeps.has(dep));
  }
  if (bad.length > 0) {
    throw new Error(`${path.relative(root, file)} contains outbound/network crate: ${bad.join(", ")}`);
  }
  const directCrypto = [...sqlCipherAtRestCryptoDeps].filter((dep) => new RegExp(`(^|\\n)\\s*${dep.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*=`, "m").test(text));
  if (directCrypto.length > 0) {
    throw new Error(`${path.relative(root, file)} declares SQLCipher crypto crate directly; keep ${directCrypto.join(", ")} transitive via rusqlite/libsqlite3-sys.`);
  }
}

for (const file of walk(root)) {
  if (file.endsWith("package.json")) checkPackage(file);
  if (file.endsWith("Cargo.toml")) checkCargo(file);
}

// The application itself is offline, and the installer must be too. Tauri's default
// `downloadBootstrapper` attempts to fetch WebView2 during setup; on a clean,
// network-disabled Windows Sandbox it aborts before installing the Local edition.
// Both editions inherit this base bundle setting.
const tauriConfig = readJson(path.join(root, "apps/desktop/src-tauri/tauri.conf.json"));
const webviewInstallMode = tauriConfig.bundle?.windows?.webviewInstallMode;
if (webviewInstallMode?.type !== "offlineInstaller" || webviewInstallMode?.silent !== true) {
  throw new Error(
    "Both Windows installers must embed the silent offline WebView2 installer; runtime setup may not require outbound access."
  );
}

// The connected-AI / connector surface. Must never appear in an offline (non-AI) edition.
const aiAndConnectorPackages = [
  "hangar-agent",
  // Connected-AI-app surface — feature-gated and never in an offline edition.
  // Named explicitly so the guard enforces what SECURITY_INVARIANTS.md promises,
  // rather than relying on transitive coverage from the hangar-agent denial.
  "hangar-appconfig",
  "hangar-mcp",
  "code-hangar-mcp",
  // AI outbound-network crate — must never be in an offline edition's graph.
  "hangar-ai"
];

// Assert a built edition's dependency tree contains no denied outbound-network crate and none of
// `forbiddenPackages`. SQLCipher at-rest crypto is allowed only transitively via rusqlite.
function checkEditionTree(featureName, forbiddenPackages) {
  const cargoTree = spawnSync(
    "cargo",
    ["tree", "--locked", "-p", "code-hangar-desktop", "--no-default-features", "--features", featureName, "--prefix", "none"],
    { cwd: root, encoding: "utf8" }
  );
  if (cargoTree.error?.code === "ENOENT") {
    console.warn("Cargo not found; skipped active Rust dependency tree deny check.");
    return;
  }
  if (cargoTree.status !== 0) {
    throw new Error(`cargo tree failed while checking the '${featureName}' edition dependencies:\n${cargoTree.stderr}`);
  }
  const activeRustPackages = new Set(
    cargoTree.stdout
      .split(/\r?\n/)
      .map((line) => /^([A-Za-z0-9_.-]+)\s+v/.exec(line.trim())?.[1])
      .filter(Boolean)
  );
  const bad = [...deniedManifestDeps].filter((dep) => activeRustPackages.has(dep));
  if (bad.length > 0) {
    throw new Error(`The '${featureName}' edition dependency tree contains denied outbound/network crates: ${bad.join(", ")}`);
  }
  const forbidden = forbiddenPackages.filter((dep) => activeRustPackages.has(dep));
  if (forbidden.length > 0) {
    throw new Error(`The '${featureName}' edition dependency tree contains forbidden feature-gated packages: ${forbidden.join(", ")}`);
  }
  const activeSqlCipherCrypto = [...sqlCipherAtRestCryptoDeps].filter((dep) => activeRustPackages.has(dep));
  if (activeSqlCipherCrypto.length > 0 && !(activeRustPackages.has("rusqlite") && activeRustPackages.has("libsqlite3-sys"))) {
    throw new Error(`SQLCipher at-rest crypto crates are present without rusqlite/libsqlite3-sys: ${activeSqlCipherCrypto.join(", ")}`);
  }
  if (activeSqlCipherCrypto.length > 0) {
    console.log(`['${featureName}'] Allowed SQLCipher at-rest crypto dependencies: ${activeSqlCipherCrypto.join(", ")}.`);
  }
}

// `core`: the strictest proof — read-only, no mutation either.
checkEditionTree("core", ["hangar-mutation", ...aiAndConnectorPackages]);
// `mutation`: the shipped LOCAL edition. It CAN delete (hangar-mutation is allowed) but must stay
// 100% local — no AI, no connector, and no outbound-network crate.
checkEditionTree("mutation", aiAndConnectorPackages);

console.log("No denied outbound-network dependencies found; Windows setup embeds offline WebView2.");
