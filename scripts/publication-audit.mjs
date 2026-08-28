import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, isAbsolute, relative, resolve, sep } from "node:path";

const argv = process.argv.slice(2);
const candidateMode = argv.includes("--candidate");
const enforcePublicHistory = candidateMode || argv.includes("--public-history");
const expectedRepository = "https://github.com/jcomlabs/code-hangar";
const approvedAuthor = "JC-OM";
const approvedEmail = "268269267+JigSawPT@users.noreply.github.com";
const expectedSourceTree = optionValue("--source-tree") ?? process.env.CODEHANGAR_PUBLICATION_SOURCE_TREE ?? null;
const evidenceDirectory = optionValue("--evidence-dir");
const evidenceArgumentPresent = argv.some((value) => value === "--evidence-dir" || value.startsWith("--evidence-dir="));
const publicationEvidenceSchema = "codehangar/publication-audit-evidence/1";
const publicationCandidateTestId = "AUTO-06/candidate-publication-audit";
const placeholderUsers = new Set(["me", "person", "someone", "user", "x"]);
const generatedPath = /(^|\/)(?:target|dist|build|node_modules|\.venv|__pycache__|\.pytest_cache|release-assets)(?:\/|$)/iu;
const releaseBinary = /\.(?:7z|dll|exe|gz|msi|pdb|tar|zip)$/iu;
const privateEmail = /\b[A-Z0-9._%+-]+@(?:gmail|hotmail|outlook)\.[A-Z]{2,}\b/giu;
const windowsUserPath = /C:[\\/]Users[\\/]([^\\/\s"']+)/giu;
const staleRepository = /github\.com[\\/]JigSawPT[\\/]CodeHangar/iu;
const objectId = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/iu;
// Keep retired publication markers out of the public source itself. The audit
// compares normalized token spans against irreversible digests so it can still
// prevent their accidental reintroduction in content, history or pathnames.
const retiredCampaignDigests = new Set([
  "1fce355b00c34ccbfdfe9c93f8c601a4259256e690541f0ef3e315912d0ef79b",
  "b6cabf7667c3182fccee80eb25a4dc8429f9d05b6396fced383069a489124364",
  "6e8bdb86dabcec3aaca9e0129824c8ede6969547a0cb397293dd3e12ce0373da",
  "78d2225c6d5432bc0f04f33413ad30435acf635ef8d60876d15bb39ef9f6c448",
  "264a8051542fb4e667df3a8668261207445c1251fd4e71da4498a09fb0869f79",
  "73dcd40b63cf15925a5661d36a40efc654133b045b155efee08d7b0debf26738",
  "9d434a9c2f6f67a627a55eb4ea05abac9a4142c8187761b5feacd2f13932e0d8",
  "10e2255a17ecc93277a31242a136307c30bca72b8776ca2d6563d4725e611eeb",
]);
const retiredCampaignLengths = new Set([6, 7, 10, 17, 28]);
const markerToken = /[a-z0-9]+(?:[-_][a-z0-9]+)*/giu;

const findings = [];

if (argv.includes("--self-test")) {
  if (argv.length !== 1) throw new Error("--self-test accepts no other arguments.");
  runSelfTests();
  console.log("Publication audit self-tests passed.");
  process.exit(0);
}

assertCandidateEvidenceRequest(candidateMode, evidenceArgumentPresent, evidenceDirectory);

const root = resolve(process.cwd());
const tracked = git(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
  .split("\0")
  .filter(Boolean)
  .map((file) => file.replaceAll("\\", "/"));
let textFiles = 0;

for (const file of tracked) {
  inspectText(file, "tracked pathname");
  if (generatedPath.test(file) || releaseBinary.test(file)) add("generated-or-release-artifact", file);
  const absolute = resolve(root, file);
  if (!existsSync(absolute) || statSync(absolute).isDirectory()) continue;
  const bytes = readFileSync(absolute);
  if (bytes.byteLength > 10 * 1024 * 1024 || bytes.subarray(0, 8192).includes(0)) continue;
  textFiles += 1;
  inspectText(bytes.toString("utf8"), file);
}

let metadata = [];
let candidateSnapshot = null;
if (enforcePublicHistory) {
  const remote = normalizeRepositoryUrl(gitMaybe(["remote", "get-url", "origin"]).trim());
  if (remote !== expectedRepository) add("unexpected-origin", "origin");

  metadata = readCommitMetadata();
  if (!candidateMode) {
    for (const commit of metadata) {
      const approvedBot =
        commit.authorName.endsWith("[bot]") &&
        (commit.authorEmail === "noreply@github.com" || commit.authorEmail.endsWith("@users.noreply.github.com"));
      if (!approvedBot && (commit.authorName !== approvedAuthor || commit.authorEmail !== approvedEmail)) {
        add("unapproved-public-author", commit.hash.slice(0, 12));
      }
    }
  }

  const history = git(["log", "--all", "--format=", "--no-ext-diff", "--no-color", "-p", "--", "."], 256 * 1024 * 1024);
  inspectText(history, "Git history");
  const messages = git(["log", "--all", "--format=%B"]);
  inspectText(messages, "Git history messages");
  const refs = git(["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes", "refs/tags"]);
  inspectText(refs, "Git refs");
}

if (candidateMode) {
  candidateSnapshot = collectCandidateSnapshot(metadata, expectedSourceTree);
  for (const violation of candidateTopologyViolations(candidateSnapshot)) add(violation.kind, violation.surface);
}

if (findings.length > 0) {
  console.error("Publication audit failed; sensitive values are intentionally not printed:");
  for (const finding of findings) console.error(`- ${finding.surface} [${finding.kind}]`);
  process.exitCode = 1;
} else {
  if (evidenceArgumentPresent) {
    // Re-snapshot immediately before proof creation so a candidate that drifted
    // after the content/history scan fails closed without emitting evidence.
    const proofSnapshot = collectCandidateSnapshot(readCommitMetadata(), expectedSourceTree);
    const evidence = createCandidateEvidence(proofSnapshot, {
      completedAtUtc: canonicalUtcTimestamp(),
      trackedFileCount: tracked.length,
      textFileCount: textFiles,
    });
    const written = writeCandidateEvidence(evidenceDirectory, evidence);
    console.log(`Private publication-candidate evidence: ${written.path}`);
    console.log(`Publication-candidate evidence SHA-256: ${written.sha256}`);
  }
  const scope = candidateMode
    ? "worktree, complete reachable history, and strict one-root candidate topology"
    : enforcePublicHistory
      ? "worktree and complete reachable history"
      : "worktree";
  console.log(`Publication audit passed: ${tracked.length} files (${textFiles} text) checked across ${scope}.`);
}

function createCandidateEvidence(snapshot, coverage) {
  if (!snapshot || candidateTopologyViolations(snapshot).length !== 0) {
    throw new Error("Refusing to create publication-candidate evidence without a fully valid candidate snapshot.");
  }
  if (!Number.isInteger(coverage.trackedFileCount) || coverage.trackedFileCount <= 0 ||
      !Number.isInteger(coverage.textFileCount) || coverage.textFileCount <= 0 ||
      coverage.textFileCount > coverage.trackedFileCount) {
    throw new Error("Refusing to create publication-candidate evidence with invalid scan coverage counts.");
  }
  const [remote] = snapshot.remotes;
  const [commit] = snapshot.metadata;
  return {
    schemaVersion: 1,
    documentType: publicationEvidenceSchema,
    version: "0.1.3",
    status: "PASS",
    completedAtUtc: coverage.completedAtUtc,
    source: {
      gitCommit: snapshot.head,
      gitTree: snapshot.headTree.toLowerCase(),
      sourceTreeDirty: false,
    },
    invocation: {
      candidate: true,
      publicHistory: true,
      sourceTree: snapshot.expectedSourceTree.toLowerCase(),
    },
    topology: {
      shallow: false,
      headBranch: "main",
      commitCount: snapshot.commits.length,
      rootCount: snapshot.roots.length,
      localHeadCount: snapshot.localHeads.length,
      tagCount: snapshot.tags.length,
      remoteCount: snapshot.remotes.length,
      remoteName: remote.name,
      fetchUrl: normalizeRepositoryUrl(remote.fetchUrls[0]),
      pushUrl: normalizeRepositoryUrl(remote.pushUrls[0]),
      author: { name: commit.authorName, email: commit.authorEmail },
      committer: { name: commit.committerName, email: commit.committerEmail },
    },
    coverage: {
      trackedFileCount: coverage.trackedFileCount,
      textFileCount: coverage.textFileCount,
      pathnamesInspected: true,
      worktreeContentInspected: true,
      historyInspected: true,
      historyMessagesInspected: true,
      refsInspected: true,
    },
    testIds: [publicationCandidateTestId],
  };
}

function assertCandidateEvidenceRequest(candidate, requested, directory) {
  if (!requested) return;
  if (!candidate) {
    throw new Error("--evidence-dir is accepted only with --candidate; worktree/history audits cannot issue candidate evidence.");
  }
  if (typeof directory !== "string" || directory.trim() === "") {
    throw new Error("--evidence-dir requires a non-empty directory value.");
  }
}

function canonicalUtcTimestamp(date = new Date()) {
  return date.toISOString().replace(/\.(\d{3})Z$/u, (_match, milliseconds) => `.${milliseconds}0000Z`);
}

function writeCandidateEvidence(requestedDirectory, evidence) {
  const allowedRoot = resolve(root, ".local", "acceptance", "v0.1.3", "publication-audit");
  const requested = isAbsolute(requestedDirectory) ? resolve(requestedDirectory) : resolve(root, requestedDirectory);
  const runName = basename(requested);
  if (dirname(requested).toLowerCase() !== allowedRoot.toLowerCase() ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/u.test(runName)) {
    throw new Error("--evidence-dir must be one new direct child of .local/acceptance/v0.1.3/publication-audit.");
  }

  ensurePlainDirectoryChain(root, allowedRoot);
  if (existsSync(requested)) throw new Error("Publication-candidate evidence directory already exists; refusing overwrite.");
  mkdirSync(requested);
  if (lstatSync(requested).isSymbolicLink()) throw new Error("Publication-candidate evidence directory is a link.");

  const output = resolve(requested, "PUBLICATION-AUDIT.private.json");
  const json = `${JSON.stringify(evidence, null, 2)}\n`;
  const descriptor = openSync(output, "wx", 0o600);
  try {
    writeFileSync(descriptor, json, { encoding: "utf8" });
    fsyncSync(descriptor);
  } finally {
    closeSync(descriptor);
  }
  return {
    path: output,
    sha256: createHash("sha256").update(json).digest("hex"),
  };
}

function ensurePlainDirectoryChain(base, target) {
  const suffix = relative(base, target);
  if (!suffix || suffix === ".." || suffix.startsWith(`..${sep}`) || isAbsolute(suffix)) {
    throw new Error("Publication evidence root escaped the repository.");
  }
  let current = base;
  for (const segment of suffix.split(sep)) {
    current = resolve(current, segment);
    if (!existsSync(current)) mkdirSync(current);
    const info = lstatSync(current);
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new Error("Publication evidence path contains a non-directory or link.");
    }
  }
}

function collectCandidateSnapshot(commitMetadata, sourceTree) {
  const head = gitMaybe(["rev-parse", "--verify", "HEAD"]).trim();
  const headTree = gitMaybe(["rev-parse", "--verify", "HEAD^{tree}"]).trim();
  const remoteNames = lines(gitMaybe(["remote"]));
  return {
    clean: gitMaybe(["status", "--porcelain=v1", "--untracked-files=all", "-z"]).length === 0,
    shallow: gitMaybe(["rev-parse", "--is-shallow-repository"]).trim(),
    headBranch: gitMaybe(["symbolic-ref", "--quiet", "--short", "HEAD"]).trim(),
    head,
    headTree,
    mainHead: gitMaybe(["rev-parse", "--verify", "refs/heads/main"]).trim(),
    commits: lines(gitMaybe(["rev-list", "--all"])),
    roots: lines(gitMaybe(["rev-list", "--max-parents=0", "--all"])),
    localHeads: lines(gitMaybe(["for-each-ref", "--format=%(refname)", "refs/heads"])),
    tags: lines(gitMaybe(["for-each-ref", "--format=%(refname)", "refs/tags"])),
    remotes: remoteNames.map((name) => ({
      name,
      fetchUrls: lines(gitMaybe(["remote", "get-url", "--all", name])),
      pushUrls: lines(gitMaybe(["remote", "get-url", "--push", "--all", name])),
    })),
    metadata: commitMetadata,
    expectedSourceTree: sourceTree,
    indexMatchesHead: gitSucceeds(["diff", "--cached", "--quiet", "HEAD", "--"]),
    worktreeMatchesIndex: gitSucceeds(["diff", "--quiet", "--"]),
  };
}

function candidateTopologyViolations(snapshot) {
  const violations = [];
  const reject = (kind) => violations.push({ kind, surface: "candidate topology" });

  if (!snapshot.clean) reject("dirty-worktree");
  if (snapshot.shallow !== "false") reject("shallow-or-unproved-repository");
  if (snapshot.headBranch !== "main") reject("head-is-not-main");
  if (!snapshot.head || snapshot.mainHead !== snapshot.head) reject("main-head-mismatch");
  if (snapshot.commits.length !== 1) reject("unexpected-commit-count");
  else if (snapshot.commits[0] !== snapshot.head) reject("reachable-commit-is-not-head");
  if (snapshot.roots.length !== 1) reject("unexpected-root-count");
  else if (snapshot.roots[0] !== snapshot.head) reject("root-is-not-head");
  if (snapshot.localHeads.length !== 1 || snapshot.localHeads[0] !== "refs/heads/main") reject("unexpected-local-heads");
  if (snapshot.tags.length !== 0) reject("unexpected-tags");

  if (snapshot.remotes.length !== 1) {
    reject("unexpected-remote-count");
  } else {
    const [remote] = snapshot.remotes;
    if (remote.name !== "origin") reject("unexpected-remote-name");
    if (remote.fetchUrls.length !== 1 || normalizeRepositoryUrl(remote.fetchUrls[0]) !== expectedRepository) {
      reject("unexpected-fetch-url");
    }
    if (remote.pushUrls.length !== 1 || normalizeRepositoryUrl(remote.pushUrls[0]) !== expectedRepository) {
      reject("unexpected-push-url");
    }
  }

  if (!snapshot.indexMatchesHead) reject("index-tree-does-not-match-head");
  if (!snapshot.worktreeMatchesIndex) reject("worktree-does-not-match-index");
  if (!snapshot.headTree || !objectId.test(snapshot.headTree)) reject("missing-or-invalid-head-tree");

  const sourceTree = snapshot.expectedSourceTree?.trim() ?? "";
  if (!sourceTree) reject("missing-source-tree");
  else if (!objectId.test(sourceTree)) reject("invalid-source-tree");
  else if (sourceTree.toLowerCase() !== snapshot.headTree.toLowerCase()) reject("source-tree-does-not-match-head");

  if (snapshot.metadata.length !== 1) {
    reject("unexpected-commit-metadata-count");
  } else {
    const [commit] = snapshot.metadata;
    if (commit.hash !== snapshot.head) reject("metadata-commit-is-not-head");
    if (commit.authorName !== approvedAuthor || commit.authorEmail !== approvedEmail) reject("unapproved-candidate-author");
    if (commit.committerName !== approvedAuthor || commit.committerEmail !== approvedEmail) reject("unapproved-candidate-committer");
  }

  return violations;
}

function readCommitMetadata() {
  return lines(gitMaybe(["log", "--all", "--format=%H%x09%an%x09%ae%x09%cn%x09%ce"])).map((line) => {
    const [hash = "", authorName = "", authorEmail = "", committerName = "", committerEmail = ""] = line.split("\t");
    return { hash, authorName, authorEmail, committerName, committerEmail };
  });
}

function runSelfTests() {
  const positives = [
    String.fromCharCode(88, 80, 82, 73, 90, 69),
    String.fromCharCode(79, 112, 101, 110, 65, 73, 32, 66, 117, 105, 108, 100, 32, 87, 101, 101, 107),
    String.fromCharCode(115, 117, 98, 109, 105, 115, 115, 105, 111, 110, 47, 111, 112, 101, 110, 97, 105, 45, 98, 117, 105, 108, 100, 45, 119, 101, 101, 107),
    String.fromCharCode(68, 101, 118, 112, 111, 115, 116),
  ];
  const negatives = ["retirement result", "release week number 12", "award allocation"];
  if (positives.some((value) => !textFindingKinds(value).includes("retired-campaign-trace"))) {
    throw new Error("Publication audit self-test did not detect a retired campaign marker.");
  }
  if (negatives.some((value) => textFindingKinds(value).includes("retired-campaign-trace"))) {
    throw new Error("Publication audit self-test rejected an unrelated phrase.");
  }
  const campaignPath = ["docs", `${positives[2]}.md`].join("/");
  if (!textFindingKinds(campaignPath).includes("retired-campaign-trace")) {
    throw new Error("Publication audit self-test did not inspect a retired marker in a pathname.");
  }

  const head = "a".repeat(40);
  const tree = "b".repeat(40);
  const baseline = {
    clean: true,
    shallow: "false",
    headBranch: "main",
    head,
    headTree: tree,
    mainHead: head,
    commits: [head],
    roots: [head],
    localHeads: ["refs/heads/main"],
    tags: [],
    remotes: [
      {
        name: "origin",
        fetchUrls: [`${expectedRepository}.git`],
        pushUrls: [`${expectedRepository}.git`],
      },
    ],
    metadata: [
      {
        hash: head,
        authorName: approvedAuthor,
        authorEmail: approvedEmail,
        committerName: approvedAuthor,
        committerEmail: approvedEmail,
      },
    ],
    expectedSourceTree: tree,
    indexMatchesHead: true,
    worktreeMatchesIndex: true,
  };
  if (candidateTopologyViolations(baseline).length !== 0) {
    throw new Error("Publication candidate topology self-test rejected the valid baseline.");
  }

  const cases = [
    ["dirty-worktree", (value) => (value.clean = false)],
    ["shallow-or-unproved-repository", (value) => (value.shallow = "true")],
    ["head-is-not-main", (value) => (value.headBranch = "release")],
    ["main-head-mismatch", (value) => (value.mainHead = "c".repeat(40))],
    ["unexpected-commit-count", (value) => value.commits.push("c".repeat(40))],
    ["reachable-commit-is-not-head", (value) => (value.commits = ["c".repeat(40)])],
    ["unexpected-root-count", (value) => value.roots.push("c".repeat(40))],
    ["root-is-not-head", (value) => (value.roots = ["c".repeat(40)])],
    ["unexpected-local-heads", (value) => value.localHeads.push("refs/heads/release")],
    ["unexpected-tags", (value) => value.tags.push("refs/tags/v0.1.3")],
    ["unexpected-remote-count", (value) => value.remotes.push(clone(value.remotes[0]))],
    ["unexpected-remote-name", (value) => (value.remotes[0].name = "public")],
    ["unexpected-fetch-url", (value) => (value.remotes[0].fetchUrls = ["https://example.invalid/code-hangar.git"])],
    ["unexpected-push-url", (value) => (value.remotes[0].pushUrls = ["https://example.invalid/code-hangar.git"])],
    ["index-tree-does-not-match-head", (value) => (value.indexMatchesHead = false)],
    ["worktree-does-not-match-index", (value) => (value.worktreeMatchesIndex = false)],
    ["missing-or-invalid-head-tree", (value) => (value.headTree = "")],
    ["missing-source-tree", (value) => (value.expectedSourceTree = null)],
    ["invalid-source-tree", (value) => (value.expectedSourceTree = "not-an-object-id")],
    ["source-tree-does-not-match-head", (value) => (value.expectedSourceTree = "c".repeat(40))],
    ["unexpected-commit-metadata-count", (value) => value.metadata.push(clone(value.metadata[0]))],
    ["metadata-commit-is-not-head", (value) => (value.metadata[0].hash = "c".repeat(40))],
    ["unapproved-candidate-author", (value) => (value.metadata[0].authorName = "Someone Else")],
    ["unapproved-candidate-committer", (value) => (value.metadata[0].committerEmail = "someone@example.invalid")],
  ];
  for (const [expectedKind, mutate] of cases) {
    const fixture = clone(baseline);
    mutate(fixture);
    const kinds = candidateTopologyViolations(fixture).map((violation) => violation.kind);
    if (!kinds.includes(expectedKind)) {
      throw new Error(`Publication candidate topology self-test missed ${expectedKind}.`);
    }
  }

  const evidence = createCandidateEvidence(baseline, {
    completedAtUtc: "2026-08-28T00:00:00.0000000Z",
    trackedFileCount: 100,
    textFileCount: 80,
  });
  if (evidence.documentType !== publicationEvidenceSchema || evidence.status !== "PASS" ||
      evidence.source.gitCommit !== head || evidence.source.gitTree !== tree ||
      evidence.source.sourceTreeDirty !== false || evidence.invocation.candidate !== true ||
      evidence.invocation.sourceTree !== tree ||
      JSON.stringify(evidence.testIds) !== JSON.stringify([publicationCandidateTestId])) {
    throw new Error("Publication candidate evidence self-test did not bind the exact clean candidate identity and claim.");
  }
  const invalidEvidenceSnapshot = clone(baseline);
  invalidEvidenceSnapshot.tags.push("refs/tags/v0.1.3");
  let invalidEvidenceRejected = false;
  try {
    createCandidateEvidence(invalidEvidenceSnapshot, {
      completedAtUtc: "2026-08-28T00:00:00.0000000Z",
      trackedFileCount: 100,
      textFileCount: 80,
    });
  } catch {
    invalidEvidenceRejected = true;
  }
  if (!invalidEvidenceRejected) {
    throw new Error("Publication candidate evidence self-test accepted an invalid candidate topology.");
  }
  let worktreeEvidenceRejected = false;
  try {
    assertCandidateEvidenceRequest(false, true, ".local/acceptance/v0.1.3/publication-audit/selftest");
  } catch {
    worktreeEvidenceRejected = true;
  }
  if (!worktreeEvidenceRejected) {
    throw new Error("Publication candidate evidence self-test allowed worktree mode to request candidate proof.");
  }
  assertCandidateEvidenceRequest(true, true, ".local/acceptance/v0.1.3/publication-audit/selftest");
}

function textFindingKinds(text) {
  const kinds = [];
  if (staleRepository.test(text)) kinds.push("stale-private-repository-url");
  if (hasRetiredCampaignTrace(text)) kinds.push("retired-campaign-trace");
  if (privateEmail.test(text)) kinds.push("private-email-domain");
  privateEmail.lastIndex = 0;
  for (const match of text.matchAll(windowsUserPath)) {
    if (!placeholderUsers.has(match[1].toLowerCase())) kinds.push("non-synthetic-user-path");
  }
  windowsUserPath.lastIndex = 0;
  return kinds;
}

function inspectText(text, surface) {
  for (const kind of textFindingKinds(text)) add(kind, surface);
}

function optionValue(name) {
  const inline = argv.find((value) => value.startsWith(`${name}=`));
  if (inline) return inline.slice(name.length + 1);
  const index = argv.indexOf(name);
  if (index < 0 || index + 1 >= argv.length || argv[index + 1].startsWith("--")) return null;
  return argv[index + 1];
}

function normalizeRepositoryUrl(value) {
  return value.trim().replace(/\.git$/u, "");
}

function lines(value) {
  return value.split(/\r?\n/u).filter(Boolean);
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function hasRetiredCampaignTrace(text) {
  const tokens = [...text.replaceAll("\\", "/").matchAll(markerToken)].map((match) => match[0].toLowerCase());
  markerToken.lastIndex = 0;
  for (let start = 0; start < tokens.length; start += 1) {
    let candidate = "";
    for (let width = 0; width < 4 && start + width < tokens.length; width += 1) {
      candidate = width === 0 ? tokens[start] : `${candidate} ${tokens[start + width]}`;
      if (!retiredCampaignLengths.has(candidate.length)) continue;
      const digest = createHash("sha256").update(candidate).digest("hex");
      if (retiredCampaignDigests.has(digest)) return true;
    }
  }
  return false;
}

function add(kind, surface) {
  findings.push({ kind, surface });
}

function git(args, maxBuffer = 32 * 1024 * 1024) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", maxBuffer });
}

function gitMaybe(args, maxBuffer = 32 * 1024 * 1024) {
  try {
    return execFileSync("git", args, {
      cwd: root,
      encoding: "utf8",
      maxBuffer,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return "";
  }
}

function gitSucceeds(args) {
  try {
    execFileSync("git", args, { cwd: root, stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}
