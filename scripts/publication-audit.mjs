import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

const enforcePublicHistory = process.argv.includes("--public-history");
const expectedRepository = "https://github.com/jcomlabs/code-hangar";
const approvedAuthor = "JC-OM";
const approvedEmail = "268269267+JigSawPT@users.noreply.github.com";
const placeholderUsers = new Set(["me", "person", "someone", "user", "x"]);
const generatedPath = /(^|\/)(?:target|dist|build|node_modules|\.venv|__pycache__|\.pytest_cache|release-assets)(?:\/|$)/iu;
const releaseBinary = /\.(?:7z|dll|exe|gz|msi|pdb|tar|zip)$/iu;
const privateEmail = /\b[A-Z0-9._%+-]+@(?:gmail|hotmail|outlook)\.[A-Z]{2,}\b/giu;
const windowsUserPath = /C:[\\/]Users[\\/]([^\\/\s"']+)/giu;
const staleRepository = /github\.com[\\/]JigSawPT[\\/]CodeHangar/iu;

const root = resolve(process.cwd());
const tracked = git(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
  .split("\0")
  .filter(Boolean)
  .map((file) => file.replaceAll("\\", "/"));
const findings = [];
let textFiles = 0;

for (const file of tracked) {
  if (generatedPath.test(file) || releaseBinary.test(file)) add("generated-or-release-artifact", file);
  const absolute = resolve(root, file);
  if (!existsSync(absolute) || statSync(absolute).isDirectory()) continue;
  const bytes = readFileSync(absolute);
  if (bytes.byteLength > 10 * 1024 * 1024 || bytes.subarray(0, 8192).includes(0)) continue;
  textFiles += 1;
  inspectText(bytes.toString("utf8"), file);
}

if (enforcePublicHistory) {
  const remote = git(["remote", "get-url", "origin"]).trim().replace(/\.git$/u, "");
  if (remote !== expectedRepository) add("unexpected-origin", "origin");

  const metadata = git(["log", "--all", "--format=%H%x09%an%x09%ae"]);
  for (const line of metadata.split(/\r?\n/u).filter(Boolean)) {
    const [hash, name, email] = line.split("\t");
    const approvedBot = name?.endsWith("[bot]") && (email === "noreply@github.com" || email?.endsWith("@users.noreply.github.com"));
    if (!approvedBot && (name !== approvedAuthor || email !== approvedEmail)) add("unapproved-public-author", hash?.slice(0, 12));
  }

  const history = git(["log", "--all", "--format=", "--no-ext-diff", "--no-color", "-p", "--", "."], 256 * 1024 * 1024);
  inspectText(history, "Git history");
}

if (findings.length > 0) {
  console.error("Publication audit failed; sensitive values are intentionally not printed:");
  for (const finding of findings) console.error(`- ${finding.surface} [${finding.kind}]`);
  process.exitCode = 1;
} else {
  const scope = enforcePublicHistory ? "worktree and complete public history" : "worktree";
  console.log(`Publication audit passed: ${tracked.length} files (${textFiles} text) checked across ${scope}.`);
}

function inspectText(text, surface) {
  if (staleRepository.test(text)) add("stale-private-repository-url", surface);
  if (privateEmail.test(text)) add("private-email-domain", surface);
  privateEmail.lastIndex = 0;
  for (const match of text.matchAll(windowsUserPath)) {
    if (!placeholderUsers.has(match[1].toLowerCase())) add("non-synthetic-user-path", surface);
  }
  windowsUserPath.lastIndex = 0;
}

function add(kind, surface) {
  findings.push({ kind, surface });
}

function git(args, maxBuffer = 32 * 1024 * 1024) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", maxBuffer });
}
