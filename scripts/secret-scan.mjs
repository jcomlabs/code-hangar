import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, statSync } from "node:fs";
import { delimiter, extname, isAbsolute, join, relative, resolve, sep } from "node:path";

const MAX_TEXT_FILE_BYTES = 10 * 1024 * 1024;
const detectors = [
  ["aws-access-key", /\bAKIA[0-9A-Z]{16}\b/u],
  ["github-token", /\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})\b/u],
  ["openai-style-key", /\bsk-(?:proj-|live-)?[A-Za-z0-9_-]{24,}\b/u],
  ["google-api-key", /\bAIza[0-9A-Za-z_-]{35}\b/u],
  ["slack-token", /\bxox[baprs]-[A-Za-z0-9-]{20,}\b/u],
  ["stripe-live-key", /\b(?:sk|rk)_live_[0-9A-Za-z]{20,}\b/u],
];

// These exact inert values exercise the product's own secret blocking. Keeping
// the allowlist exact prevents a real value with a similar prefix from passing.
const syntheticFixtures = [
  "AKIAIOSFODNN7EXAMPLE",
  "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
  "sk-ABCDEF1234567890ABCDEFGH",
  "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
  "sk-abcdefghijklmnopqrstuvwxyz123456",
  "sk-FIXTUREexample0123456789",
  "sk-test-12345678901234567890",
];

const root = resolve(process.cwd());
const git = resolveTool("git", root);
const files = execFileSync(git, ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
  cwd: root,
  encoding: "utf8",
  maxBuffer: 16 * 1024 * 1024,
})
  .split("\0")
  .filter(Boolean);

const findings = [];
let scanned = 0;
let skippedBinaryOrLarge = 0;
for (const file of files) {
  const absolute = resolve(root, file);
  if (absolute !== root && !absolute.startsWith(`${root}${sep}`)) {
    throw new Error(`Refusing to scan path outside repository: ${file}`);
  }
  if (!existsSync(absolute)) continue;
  const info = lstatSync(absolute);
  if (info.isDirectory()) continue;
  const bytes = readFileSync(absolute);
  if (bytes.byteLength > MAX_TEXT_FILE_BYTES || bytes.subarray(0, 8192).includes(0)) {
    skippedBinaryOrLarge += 1;
    continue;
  }
  scanned += 1;
  inspect(scrubFixtures(bytes.toString("utf8")), { surface: "worktree", file: file.replaceAll("\\", "/") });
}

const history = execFileSync(git, ["log", "--all", "--format=", "--no-ext-diff", "--no-color", "-p", "--", "."], {
  cwd: root,
  encoding: "utf8",
  maxBuffer: 256 * 1024 * 1024,
});
inspect(scrubFixtures(history), { surface: "history" });

if (findings.length > 0) {
  console.error("Potential high-confidence secrets detected; matching values are intentionally not printed:");
  for (const finding of findings) {
    console.error(`- ${finding.surface}${finding.file ? `:${finding.file}` : ""} [${finding.detector}]`);
  }
  process.exitCode = 1;
} else {
  console.log(`Secret scan passed: ${scanned} text files and Git patch history checked; ${skippedBinaryOrLarge} binary/large file(s) skipped.`);
}

function inspect(text, location) {
  if (containsPrivateKey(text)) findings.push({ ...location, detector: "private-key" });
  for (const [detector, pattern] of detectors) {
    if (pattern.test(text)) findings.push({ ...location, detector });
  }
}

function containsPrivateKey(text) {
  const begin = /-----BEGIN ((?:RSA |EC |OPENSSH |PGP )?)PRIVATE KEY-----/gu;
  for (const match of text.matchAll(begin)) {
    const end = `-----END ${match[1]}PRIVATE KEY-----`;
    const endIndex = text.indexOf(end, match.index + match[0].length);
    if (endIndex !== -1 && endIndex - match.index <= 128 * 1024) return true;
  }
  return false;
}

function scrubFixtures(text) {
  let output = text;
  for (const fixture of syntheticFixtures) output = output.replaceAll(fixture, "[synthetic-secret-fixture]");
  return output;
}

function resolveTool(name, rejectedRoot) {
  const pathKey = process.platform === "win32" ? Object.keys(process.env).find((key) => key.toLowerCase() === "path") : "PATH";
  const value = pathKey ? (process.env[pathKey] ?? "") : "";
  const names = process.platform === "win32" && !extname(name) ? [`${name}.exe`] : [name];
  for (const rawDirectory of value.split(delimiter)) {
    const directory = rawDirectory.trim().replace(/^"|"$/gu, "");
    if (!directory || !isAbsolute(directory)) continue;
    for (const candidateName of names) {
      try {
        const candidate = statSync(join(directory, candidateName));
        const rel = relative(resolve(rejectedRoot), resolve(directory, candidateName));
        if (rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel))) continue;
        if (candidate.isFile()) return resolve(directory, candidateName);
      } catch {
        // Keep searching absolute PATH entries outside the repository.
      }
    }
  }
  throw new Error(`Trusted ${name} executable was not found outside the repository`);
}
