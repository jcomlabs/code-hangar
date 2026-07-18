import { renderMarkdownSafe } from "./markdown";
import { readVisualAcceptanceState, visualAcceptanceProjectCount } from "./visualAcceptance";

// Browser acceptance runs need the same capability surface as the frontend
// edition under test. Production Tauri builds replace this fixture status with
// the backend's compiled feature list; Local/test fixtures remain core-only.
const connectorAcceptanceFixture = import.meta.env.MODE === "connector";

import type {
  AdapterSummary,
  Comment,
  ContextFile,
  DashboardSummary,
  DbMaintenanceReport,
  DocumentSearchResult,
  DuplicateCandidates,
  DuplicateConfirmation,
  ExportResult,
  FilePreview,
  FolderExplanation,
  FolderInvestigation,
  InvestigationHandle,
  GitRepoSummary,
  GraphMap,
  LostProjectCandidate,
  LostProjectCandidates,
  MutationActivityLog,
  MutationBackupSummary,
  MutationFinalRemoveSummary,
  MutationLockInspection,
  MutationMoveSummary,
  MutationProtectedPreview,
  MutationRestoreSummary,
  MutationTokenResult,
  NavChildrenPage,
  NavItem,
  NodeRelationship,
  NodeRelationships,
  OperationPlan,
  OrphanCandidate,
  OrphanCandidates,
  OrphanStatus,
  PinnedItem,
  PreviewMode,
  PreviewPolicy,
  InstalledApp,
  ProjectDiscoveryReport,
  ProjectSummary,
  QuickOpenResult,
  RecentItem,
  RecoverableSummary,
  RecoveryPending,
  RecoveryResolveResult,
  RelationshipIssue,
  RiskReport,
  ScanRoot,
  SecurityStatus,
  StartupStatus,
  ProcessResourceUsage,
  SessionChangeSet,
  SessionPreview,
  SystemResourceProfile,
  WatcherStatus,
  ProtectedZone
} from "./types";

interface FixtureFile {
  nodeId: number;
  projectId: number;
  path: string;
  displayName: string;
  body?: string;
  isMarkdown: boolean;
  isContext: boolean;
  isSensitive: boolean;
  protectedLevel?: string | null;
}

let fixtureDiscoverySnapshot: string | null = null;
const visualAcceptanceState = readVisualAcceptanceState();

let projects: ProjectSummary[] = [
  {
    id: 1,
    name: "Fixture Markdown Project",
    path: "fixture://markdown-project",
    source: "fixture",
    app: "codex",
    apps: ["codex", "claude"],
    contextCount: 5,
    pinned: true,
    protectedLevel: null,
    scanState: "scanned",
    scanRootId: null
  },
  {
    id: 2,
    name: "Fixture Sensitive Project",
    path: "fixture://sensitive-project",
    source: "fixture",
    contextCount: 1,
    pinned: false,
    protectedLevel: null,
    scanState: "scanned",
    scanRootId: null
  },
  {
    id: 3,
    name: "Fixture Git-like Project",
    path: "fixture://git-like-project",
    source: "fixture",
    contextCount: 2,
    pinned: false,
    protectedLevel: null,
    scanState: "scanned",
    scanRootId: null
  }
];

let files: FixtureFile[] = [
  {
    nodeId: 101,
    projectId: 1,
    path: "README.md",
    displayName: "README.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: `# Fixture Markdown Project

This project is a small local fixture used to validate Code Hangar navigation.

## Context

- README files are context files.
- AGENTS.md should appear before ordinary source files.
- Remote links such as [example](https://example.invalid) stay inert in preview.

## Local Links

See [overview](docs/overview.md), [system prompt](prompts/system.md), and [missing note](missing-note.md).`
  },
  {
    nodeId: 102,
    projectId: 1,
    path: "AGENTS.md",
    displayName: "AGENTS.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: `# Fixture Agent Instructions

This file represents local project instructions.

## Rules

- Prefer fixture-backed behaviour while the database wiring is new.
- Do not perform filesystem mutation.
- Do not add network behaviour.`
  },
  {
    nodeId: 103,
    projectId: 1,
    path: "docs/overview.md",
    displayName: "overview.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: `# Overview

The fixture includes Markdown, source files and a package manifest so the UI can show context priority.

![Local diagram](./diagram.png)

![Blocked remote image](https://example.invalid/remote.png)`
  },
  {
    nodeId: 106,
    projectId: 1,
    path: "docs/diagram.png",
    displayName: "diagram.png",
    isMarkdown: false,
    isContext: false,
    isSensitive: false
  },
  {
    nodeId: 107,
    projectId: 1,
    path: "assets/unused.png",
    displayName: "unused.png",
    isMarkdown: false,
    isContext: false,
    isSensitive: false
  },
  {
    nodeId: 108,
    projectId: 1,
    path: "assets/copy-a.dat",
    displayName: "copy-a.dat",
    isMarkdown: false,
    isContext: false,
    isSensitive: false,
    body: "duplicate-fixture-payload\n".repeat(96)
  },
  {
    nodeId: 109,
    projectId: 1,
    path: "assets/copy-b.dat",
    displayName: "copy-b.dat",
    isMarkdown: false,
    isContext: false,
    isSensitive: false,
    body: "duplicate-fixture-payload\n".repeat(96)
  },
  {
    nodeId: 104,
    projectId: 1,
    path: "prompts/system.md",
    displayName: "system.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: "# System Prompt\n\nYou are operating in read-only navigation mode."
  },
  {
    nodeId: 105,
    projectId: 1,
    path: "package.json",
    displayName: "package.json",
    isMarkdown: false,
    isContext: true,
    isSensitive: false,
    body: '{\n  "name": "fixture-markdown-project",\n  "private": true\n}'
  },
  {
    nodeId: 201,
    projectId: 2,
    path: "README.md",
    displayName: "README.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: "# Fixture Sensitive Project\n\nSensitive files are visible as blocked entries without revealing contents."
  },
  {
    nodeId: 202,
    projectId: 2,
    path: ".env",
    displayName: ".env",
    isMarkdown: false,
    isContext: false,
    isSensitive: true,
    protectedLevel: "no_preview",
    body: "SECRET=value"
  },
  {
    nodeId: 203,
    projectId: 2,
    path: "credentials.json",
    displayName: "credentials.json",
    isMarkdown: false,
    isContext: false,
    isSensitive: true,
    protectedLevel: "no_preview",
    body: '{ "apiToken": "fixture-secret" }'
  },
  {
    nodeId: 204,
    projectId: 2,
    path: "token.json",
    displayName: "token.json",
    isMarkdown: false,
    isContext: false,
    isSensitive: true,
    protectedLevel: "no_preview",
    body: '{ "token": "fixture-token" }'
  },
  {
    nodeId: 301,
    projectId: 3,
    path: "README.md",
    displayName: "README.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: "# Fixture Git-like Project\n\nCode Hangar reads only local Git metadata here. It does not run remote Git commands."
  },
  {
    nodeId: 302,
    projectId: 3,
    path: "AGENTS.md",
    displayName: "AGENTS.md",
    isMarkdown: true,
    isContext: true,
    isSensitive: false,
    body: "# Git-like Fixture Instructions\n\nTreat the .git folder as protected metadata in Stage 1."
  },
  {
    nodeId: 303,
    projectId: 3,
    path: ".git/config",
    displayName: "config",
    isMarkdown: false,
    isContext: false,
    isSensitive: true,
    protectedLevel: "no_preview"
  }
];

if (visualAcceptanceState === "empty") {
  projects = [];
  files = [];
} else if (visualAcceptanceState === "partial") {
  projects = projects.map((project, index) => index === 0 ? { ...project, scanState: "outdated" } : project);
} else if (visualAcceptanceState === "saturated") {
  const generatedProjects = Array.from({ length: visualAcceptanceProjectCount(visualAcceptanceState) }, (_, index): ProjectSummary => {
    const ordinal = String(index + 1).padStart(3, "0");
    return {
      id: 1000 + index,
      name: `Acceptance Project ${ordinal} With A Deliberately Long Workspace Name`,
      path: `fixture://saturated/project-${ordinal}/deeply/nested/workspace`,
      source: "fixture",
      contextCount: 2,
      pinned: index < 8,
      protectedLevel: null,
      scanState: "scanned",
      scanRootId: null
    };
  });
  const generatedFiles = generatedProjects.flatMap((project, index): FixtureFile[] => {
    const nodeBase = 100_000 + index * 10;
    return [
      {
        nodeId: nodeBase,
        projectId: project.id,
        path: "README.md",
        displayName: "README.md",
        isMarkdown: true,
        isContext: true,
        isSensitive: false,
        body: `# ${project.name}\n\nSaturated acceptance fixture.`
      },
      {
        nodeId: nodeBase + 1,
        projectId: project.id,
        path: "AGENTS.md",
        displayName: "AGENTS.md",
        isMarkdown: true,
        isContext: true,
        isSensitive: false,
        body: "# Acceptance context\n\nKeep this fixture local and read-only."
      },
      {
        nodeId: nodeBase + 2,
        projectId: project.id,
        path: `logs/archive-${String(index + 1).padStart(3, "0")}.log`,
        displayName: `archive-${String(index + 1).padStart(3, "0")}.log`,
        isMarkdown: false,
        isContext: false,
        isSensitive: false,
        body: "bounded acceptance log\n".repeat(20)
      },
      {
        nodeId: nodeBase + 3,
        projectId: project.id,
        path: "src/components/very-long-acceptance-component-name.tsx",
        displayName: "very-long-acceptance-component-name.tsx",
        isMarkdown: false,
        isContext: false,
        isSensitive: false,
        body: "export const acceptanceFixture = true;\n"
      }
    ];
  });
  projects = [...projects, ...generatedProjects];
  files = [...files, ...generatedFiles];
}

let recentItems: RecentItem[] = [];
let pinnedItems: PinnedItem[] = projects.length > 0
  ? [{ nodeId: projects[0].id, projectId: null, itemKind: "project", path: projects[0].path, pinnedAt: new Date().toISOString() }]
  : [];
let comments: Comment[] = [];
let nextCommentId = 1;
let scanRoots: ScanRoot[] = [];

const adapters: AdapterSummary[] = [
  {
    id: 1,
    name: "generic_markdown_context",
    version: "1.0.0",
    adapterType: "context",
    source: "builtin",
    enabled: true,
    description: "Classifies local Markdown and agent context files without network access."
  },
  {
    id: 2,
    name: "generic_git_project",
    version: "1.0.0",
    adapterType: "project_metadata",
    source: "builtin",
    enabled: true,
    description: "Reads local .git metadata passively and never invokes remote Git commands."
  },
  {
    id: 3,
    name: "generic_model_workflow_assets",
    version: "1.0.0",
    adapterType: "asset_classifier",
    source: "builtin",
    enabled: true,
    description: "Labels common local model, workflow and generated asset files for review."
  }
];

export const fixtureApi = {
  async startupStatus(): Promise<StartupStatus> {
    return {
      state: "ready",
      message: "Fixture inventory is ready.",
      elapsedMs: 0,
      dbOpenMs: 0
    };
  },

  async systemResourceProfile(): Promise<SystemResourceProfile> {
    return {
      logicalCpuCount: 8,
      totalMemoryBytes: 16 * 1024 * 1024 * 1024,
      availableMemoryBytes: 8 * 1024 * 1024 * 1024,
      gpuAcceleration: "Not used by fixture inventory tasks. Current work is CPU/I/O bound.",
      dedicatedVramBytes: null,
      plans: [
        {
          mode: "balanced",
          label: "Balanced",
          cpuThreads: 1,
          processPriority: "normal",
          scanBatchSize: 2000,
          memoryBudgetBytes: 512 * 1024 * 1024,
          notes: ["Conservative local scan worker.", "Normal process priority."]
        },
        {
          mode: "priority",
          label: "Priority",
          cpuThreads: 7,
          processPriority: "above normal only while a heavy task runs on Windows",
          scanBatchSize: 12000,
          memoryBudgetBytes: 2 * 1024 * 1024 * 1024,
          notes: ["Parallel metadata workers.", "Returns to normal process priority when the task finishes."]
        },
        {
          mode: "max",
          label: "Max CPU",
          cpuThreads: 8,
          processPriority: "above normal only while a heavy task runs on Windows",
          scanBatchSize: 20000,
          memoryBudgetBytes: 4 * 1024 * 1024 * 1024,
          notes: ["Uses all fixture logical CPU threads.", "Idle process priority remains normal."]
        }
      ]
    };
  },

  async sessionPreview(path: string, reveal = false, options?: { maxBytes?: number; loadFull?: boolean }): Promise<SessionPreview> {
    const name = path.split(/[\\/]/).pop() ?? path;
    const secret = reveal ? "api_key=sk-FIXTUREexample0123456789" : "api_key=«redacted»";
    return {
      path,
      displayName: name,
      sessionKind: "fixture",
      sizeBytes: 4096,
      previewLimitBytes: options?.loadFull ? 4096 : Math.min(4096, options?.maxBytes ?? 4096),
      truncated: false,
      sourceTruncated: false,
      redactedCount: 1,
      revealed: reveal,
      text:
        `# Fixture session preview\n\nThis is sample local session content for ${name}.\n\n` +
        `In the desktop build this shows the real, secret-redacted transcript text.\n` +
        `Example referenced path: C:/AI/Codex/CodeHangar\n` +
        `Example masked secret: ${secret}\n`
    };
  },

  async sessionChangeSet(path: string): Promise<SessionChangeSet> {
    return {
      path,
      sourceKind: path.toLowerCase().includes("claude") ? "Claude" : "ChatGPT",
      coverage: {
        level: path.toLowerCase().includes("claude") ? "direct_edits" : "full",
        label: path.toLowerCase().includes("claude") ? "Recorded direct edits only" : "Full recorded turn diff",
        note: path.toLowerCase().includes("claude")
          ? "Reconstructed from direct file-edit calls. Shell changes may be absent."
          : "Reconstructed from recorded turn diffs. Shell changes may still be absent."
      },
      files: [
        {
          path: "apps/desktop/src/views/SettingsView.tsx",
          addedLines: 2,
          removedLines: 1,
          reality: {
            status: "applied",
            label: "Appears applied",
            note: "Fixture current-file evidence matches the recorded after-state.",
            observedMs: 1_700_000_000_000
          },
          edits: [
            {
              source: "Fixture turn diff",
              summary: "Recorded file patch",
              provenance: "Recorded fixture turn diff",
              confidence: "observed",
              reality: {
                status: "applied",
                label: "Appears applied",
                note: "Fixture current-file evidence matches this recorded edit.",
                observedMs: 1_700_000_000_000
              },
              request: "Make project removal safer and easier to understand.",
              addedLines: 2,
              removedLines: 1,
              hunks: [
                {
                  header: "@@ -42,3 +42,4 @@",
                  oldStart: 42,
                  newStart: 42,
                  lines: [
                    { kind: "context", content: "<section className=\"settings\">", oldLine: 42, newLine: 42 },
                    { kind: "removed", content: "<button>Remove</button>", oldLine: 43 },
                    { kind: "added", content: "<button>Remove selected</button>", newLine: 43 },
                    { kind: "added", content: "<small>Files stay on disk.</small>", newLine: 44 }
                  ]
                }
              ]
            }
          ]
        }
      ],
      editCount: 1,
      addedLines: 2,
      removedLines: 1,
      redactedCount: 0,
      parsedRecords: 24,
      omittedRecords: 0
    };
  },

  async projectGitChangeSet(projectId: number): Promise<SessionChangeSet> {
    const result = await this.sessionChangeSet(`git:fixture-project-${projectId}`);
    return {
      ...result,
      sourceKind: "Local Git",
      coverage: {
        level: "full",
        label: "Current local Git evidence",
        note: "Fixture staged and working-tree evidence. No remote Git operation is used."
      }
    };
  },

  async processResourceUsage(): Promise<ProcessResourceUsage> {
    const cpuPercent = Math.round((6 + Math.random() * 10) * 10) / 10;
    const workingSet = Math.round((180 + Math.random() * 40) * 1024 * 1024);
    return {
      cpuPercent,
      logicalCpuCount: 8,
      memoryWorkingSetBytes: workingSet,
      memoryPrivateBytes: workingSet + 48 * 1024 * 1024,
      totalMemoryBytes: 16 * 1024 * 1024 * 1024,
      availableMemoryBytes: 8 * 1024 * 1024 * 1024,
      gpuSummary: "GPU/VRAM not used by fixture inventory tasks (CPU/I/O bound).",
      gpuUsagePercent: null,
      sampled: true
    };
  },

  async watcherStatus(focusedProjectId?: number | null, currentNodeId?: number | null): Promise<WatcherStatus> {
    return {
      generatedAtMs: Date.now(),
      pollIntervalMs: 30000,
      debounceMs: 1500,
      staleProjects: 0,
      changedProjects: 0,
      projects: projects.map((project) => ({
        projectId: project.id,
        scanRootId: project.scanRootId ?? 0,
        name: project.name,
        path: project.path,
        state: "clean",
        reason: "Fixture data is static in browser mode.",
        lastScannedAt: null,
        rootModifiedAt: null
      })),
      focused: focusedProjectId ? {
        projectId: focusedProjectId,
        state: "clean",
        changedContextFiles: 0,
        currentNode: currentNodeId ? {
          nodeId: currentNodeId,
          path: "fixture://static",
          displayName: "fixture",
          state: "clean",
          isMarkdown: true,
          isContext: true,
          storedMtime: null,
          liveMtime: null,
          storedSize: null,
          liveSize: null
        } : null,
        message: "Fixture data is static in browser mode."
      } : null,
      message: "Fixture watcher is clean."
    };
  },

  async projectsList(): Promise<ProjectSummary[]> {
    const pinnedProjectIds = new Set(pinnedItems.filter((item) => item.itemKind === "project").map((item) => item.nodeId));
    return projects.map((project) => ({ ...project, pinned: pinnedProjectIds.has(project.id) }));
  },

  async projectGet(projectId: number): Promise<ProjectSummary | null> {
    return projects.find((project) => project.id === projectId) ?? null;
  },

  async projectNavTree(projectId: number): Promise<NavItem[]> {
    return buildTree(files.filter((file) => file.projectId === projectId).map(fileToNavItem));
  },

  async projectNavChildren(projectId: number, parentNavId: number | null = null, limit = 200, offset = 0): Promise<NavChildrenPage> {
    const tree = buildTree(files.filter((file) => file.projectId === projectId).map(fileToNavItem));
    const parent = parentNavId === null ? null : findNavItem(tree, parentNavId);
    const children = parentNavId === null ? tree : parent?.children ?? [];
    const items = children.slice(offset, offset + limit).map(stripChildren);
    return {
      items,
      total: children.length,
      hasMore: offset + items.length < children.length
    };
  },

  async projectNavPath(projectId: number, nodeId: number): Promise<NavItem[]> {
    const items = files.filter((file) => file.projectId === projectId).map(fileToNavItem);
    const path: NavItem[] = [];
    let current = items.find((item) => item.nodeId === nodeId);
    while (current && path.length < 256) {
      path.push(stripChildren(current));
      current = current.parentNavId == null
        ? undefined
        : items.find((item) => item.id === current?.parentNavId);
    }
    return path.reverse();
  },

  async projectGitStatus(projectId: number): Promise<GitRepoSummary> {
    if (projectId === 3) {
      return {
        projectId,
        hasGit: true,
        currentBranch: "main",
        headRef: "ref: refs/heads/main",
        originUrl: "https://example.invalid/passive-only.git",
        metadataError: null
      };
    }
    return { projectId, hasGit: false, currentBranch: null, headRef: null, originUrl: null, metadataError: null };
  },

  async investigateFolder(path: string): Promise<InvestigationHandle> {
    return { rootId: 9000, jobId: "fixture-investigate", path };
  },

  async investigationReport(rootId: number): Promise<FolderInvestigation> {
    return {
      rootId,
      rootNodeId: null,
      path: "fixture://investigated",
      explanation: null,
      owners: [],
      isOrphan: true,
      fileCount: 0,
      totalBytes: 0,
      hasGit: false
    };
  },

  async folderExplanation(navId: number): Promise<FolderExplanation | null> {
    const tree = buildTree(files.map(fileToNavItem));
    const item = findNavItem(tree, navId);
    if (!item) return null;
    const classification = item.itemKind === "directory"
      ? item.displayName === "docs"
        ? "documentation-context"
        : item.displayName === "prompts"
          ? "documentation-context"
          : item.displayName === ".git"
            ? "source-control-metadata"
            : "unknown"
      : "file";
    const confidence = classification === "unknown" ? "unknown" : classification === "file" ? "high" : "medium";
    return {
      navId: item.id,
      projectId: item.projectId,
      displayPath: item.displayPath,
      displayName: item.displayName,
      itemKind: item.itemKind,
      classification,
      confidence,
      summary: classification === "unknown"
        ? "Code Hangar cannot classify this relationship."
        : item.itemKind === "directory"
          ? "This fixture folder likely holds project context or local metadata."
          : "This is a file entry, not a folder. Folder relationship classification does not apply.",
      signals: [`${item.childCount} direct child entries`],
      caveats: classification === "unknown" ? ["Code Hangar cannot classify this relationship."] : [],
      childCount: item.childCount,
      apparentBytes: item.aggregateApparentBytes,
      allocatedBytes: item.aggregateAllocatedBytes,
      physicalBytes: item.aggregatePhysicalBytes,
      footprintPartial: item.aggregateBytesPartial,
      protectedLevel: item.protectedLevel,
      fullyScanned: item.fullyScanned,
      scanError: item.scanError
    };
  },

  async dashboardSummary(includeFixtureProjects = true): Promise<DashboardSummary> {
    const visibleProjects = includeFixtureProjects
      ? projects
      : projects.filter((project) => project.source !== "fixture");
    const visibleProjectIds = new Set(visibleProjects.map((project) => project.id));
    const visibleFiles = files.filter((file) => visibleProjectIds.has(file.projectId));
    const sensitiveFiles = visibleFiles.filter((file) => file.isSensitive).length;
    const protectedFiles = visibleFiles.filter((file) => file.protectedLevel).length;
    return {
      totalProjects: visibleProjects.length,
      totalItems: visibleFiles.length,
      contextFiles: visibleFiles.filter((file) => file.isContext).length,
      indexedDocuments: visibleFiles.filter((file) => file.body && !file.isSensitive && !file.protectedLevel).length,
      nonIndexedItems: visibleFiles.filter((file) => !file.body || file.isSensitive || file.protectedLevel).length,
      partialItems: visualAcceptanceState === "partial" ? Math.max(1, visibleFiles.length) : 0,
      gitProjects: visibleProjects.some((project) => project.id === 3) ? 1 : 0,
      sensitiveFiles,
      protectedFiles,
      scanRoots: scanRoots.filter((root) => root.enabled && visibleProjects.some((project) => project.path === root.path)).length,
      largestProjects: visibleProjects.map((project) => {
        const apparentBytes = visibleFiles
          .filter((file) => file.projectId === project.id)
          .reduce((total, file) => total + (file.body?.length ?? 0), 0);
        return {
          projectId: project.id,
          name: project.name,
          path: project.path,
          apparentBytes,
          allocatedBytes: null,
          physicalBytes: apparentBytes,
          footprintPartial: visualAcceptanceState === "partial" && project.id === projects[0]?.id
        };
      }).sort((left, right) => right.apparentBytes - left.apparentBytes).slice(0, 5),
      staleOrDirty: "No live disk check needed for fixture data or projects without file fingerprints.",
      adaptersNeedingReview: 0
    };
  },

  async adaptersList(): Promise<AdapterSummary[]> {
    return adapters;
  },

  async projectContextFiles(projectId: number): Promise<ContextFile[]> {
    return files
      .filter((file) => file.projectId === projectId && file.isContext)
      .sort((left, right) => contextRankFor(left.path, left.displayName) - contextRankFor(right.path, right.displayName) || priorityFor(left.path) - priorityFor(right.path) || left.displayName.localeCompare(right.displayName))
      .map((file) => ({
        navId: file.nodeId,
        nodeId: file.nodeId,
        projectId: file.projectId,
        path: file.path,
        displayName: file.displayName,
        priority: priorityFor(file.path),
        contextRank: contextRankFor(file.path, file.displayName),
        contextGroup: contextMetadataFor(file.path, file.displayName).group,
        recommendationReason: contextMetadataFor(file.path, file.displayName).reason,
        recommended: contextRankFor(file.path, file.displayName) < 60,
        isSensitive: file.isSensitive,
        protectedLevel: file.protectedLevel
      }));
  },

  async filePreview(nodeId: number, mode: PreviewMode, recordRecent = true, policy?: PreviewPolicy): Promise<FilePreview> {
    const file = files.find((candidate) => candidate.nodeId === nodeId);
    if (!file) {
      return blockedPreview(nodeId, 0, "", "Missing file", mode, "The requested node is not in the fixture index.", "missing");
    }
    if (recordRecent) {
      recentItems = [{ nodeId, projectId: file.projectId, itemKind: "file", path: file.path, openedAt: new Date().toISOString() }, ...recentItems.filter((item) => item.nodeId !== nodeId)].slice(0, 20);
    }
    const strongProtected = isStrongProtectedFixturePath(file.path);
    // Auto-preview only layers on top of the explicit reveal consent; on its own it
    // never exposes sensitive or protected content. Mirrors hangar-db file_preview_internal.
    const relaxedPreview = Boolean(policy?.relaxNonStrongProtectedPreview && policy?.allowSensitiveReveal && (file.isSensitive || file.protectedLevel) && !strongProtected);
    if ((file.isSensitive || file.protectedLevel) && !relaxedPreview) {
      return blockedPreview(nodeId, file.projectId, file.path, file.displayName, mode, "Preview blocked by sensitive-file or Protected Zone policy.", "blocked");
    }
    if (!file.body) {
      return blockedPreview(nodeId, file.projectId, file.path, file.displayName, mode, "This fixture file is not available for preview.", "unsupported");
    }
    const rendered = renderMarkdownSafe(file.body);
    return {
      nodeId,
      projectId: file.projectId,
      path: file.path,
      displayPath: file.path,
      displayName: file.displayName,
      mode,
      state: "ready",
      fileKind: file.isMarkdown || file.isContext ? "markdown" : "text",
      sizeBytes: file.body.length,
      truncated: false,
      previewLimitBytes: 2 * 1024 * 1024,
      systemErrorCode: null,
      wasRevealed: relaxedPreview,
      source: mode === "source" ? file.body : null,
      renderedHtml: mode === "rendered" ? (file.isMarkdown || file.isContext ? rendered.html : `<pre><code>${escapeForPre(file.body)}</code></pre>`) : null,
      blockedReason: null,
      headings: rendered.headings,
      links: rendered.links
    };
  },

  async fileReveal(nodeId: number, mode: PreviewMode, policy?: PreviewPolicy): Promise<FilePreview> {
    const file = files.find((candidate) => candidate.nodeId === nodeId);
    if (!file) {
      return blockedPreview(nodeId, 0, "", "Missing file", mode, "The requested node is not in the fixture index.", "missing");
    }
    if (!policy?.allowSensitiveReveal && (file.isSensitive || file.protectedLevel)) {
      return blockedPreview(nodeId, file.projectId, file.path, file.displayName, mode, "Temporary sensitive reveal is disabled for this session.", "blocked");
    }
    if (isStrongProtectedFixturePath(file.path)) {
      return blockedPreview(nodeId, file.projectId, file.path, file.displayName, mode, "Strong Protected Zone content cannot be revealed in this phase.", "blocked");
    }
    if (!file.body) {
      return blockedPreview(nodeId, file.projectId, file.path, file.displayName, mode, "This fixture file is not available for reveal.", "unsupported");
    }
    const rendered = renderMarkdownSafe(file.body);
    return {
      nodeId,
      projectId: file.projectId,
      path: file.path,
      displayPath: file.path,
      displayName: file.displayName,
      mode,
      state: "ready",
      fileKind: file.isMarkdown || file.isContext ? "markdown" : "text",
      sizeBytes: file.body.length,
      truncated: false,
      previewLimitBytes: 2 * 1024 * 1024,
      systemErrorCode: null,
      wasRevealed: true,
      source: mode === "source" ? file.body : null,
      renderedHtml: mode === "rendered" ? (file.isMarkdown || file.isContext ? rendered.html : `<pre><code>${escapeForPre(file.body)}</code></pre>`) : null,
      blockedReason: null,
      headings: rendered.headings,
      links: rendered.links
    };
  },

  async quickOpen(query: string, limit = 20): Promise<QuickOpenResult[]> {
    return files
      .map((file) => {
        const project = projects.find((candidate) => candidate.id === file.projectId);
        return {
          file,
          score: score(file.displayName, file.path, project?.name ?? "", project?.path ?? "", query)
        };
      })
      .filter((item) => item.score > 0)
      .sort((left, right) => right.score - left.score || left.file.displayName.localeCompare(right.file.displayName))
      .slice(0, limit)
      .map(({ file, score }) => ({ nodeId: file.nodeId, projectId: file.projectId, label: file.displayName, path: file.path, itemKind: "file", score }));
  },

  async searchDocuments(filters: { query: string; projectId?: number | null; indexedKind?: string; pathFilter?: string; nameFilter?: string; limit?: number; includeFixtureProjects?: boolean }): Promise<DocumentSearchResult> {
    const started = performance.now();
    const needle = filters.query.trim().toLowerCase();
    const limit = filters.limit === 0 ? 0 : Math.min(Math.max(filters.limit ?? 20, 1), 50);
    const pathNeedle = filters.pathFilter?.trim().toLowerCase() ?? "";
    const nameNeedle = filters.nameFilter?.trim().toLowerCase() ?? "";
    const visibleProjectIds = filters.includeFixtureProjects === false
      ? new Set(projects.filter((project) => project.source !== "fixture").map((project) => project.id))
      : null;
    if (needle.length < 2) return { hits: [], truncated: false, durationMs: 0 };
    const matches = files
      .filter((file) => !file.isSensitive && !file.protectedLevel && file.body?.toLowerCase().includes(needle))
      .filter((file) => !visibleProjectIds || visibleProjectIds.has(file.projectId))
      .filter((file) => !filters.projectId || file.projectId === filters.projectId)
      .filter((file) => {
        if (filters.indexedKind === "context") return file.isContext;
        if (filters.indexedKind === "markdown") return file.isMarkdown;
        return true;
      })
      .filter((file) => !pathNeedle || file.path.toLowerCase().includes(pathNeedle))
      .filter((file) => !nameNeedle || file.displayName.toLowerCase().includes(nameNeedle));
    const hits = limit === 0 ? matches : matches.slice(0, limit);
    return {
      hits: hits.map((file) => ({
        nodeId: file.nodeId,
        projectId: file.projectId,
        title: file.displayName,
        path: file.path,
        snippet: snippet(file.body ?? "", needle)
      })),
      truncated: limit !== 0 && matches.length > limit,
      durationMs: Math.round(performance.now() - started)
    };
  },

  async resolveLocalLink(projectId: number, fromNodeId: number, target: string): Promise<number | null> {
    const from = files.find((file) => file.projectId === projectId && file.nodeId === fromNodeId);
    if (!from) return null;
    const resolved = resolveFixturePath(from.path, target);
    return files.find((file) => file.projectId === projectId && file.path === resolved)?.nodeId ?? null;
  },

  async nodeRelationships(nodeId: number): Promise<NodeRelationships> {
    const relationships = buildFixtureRelationships();
    return {
      nodeId,
      outgoing: relationships.filter((relationship) => relationship.sourceNodeId === nodeId).map((relationship) => relationship.target),
      incoming: relationships.filter((relationship) => relationship.targetNodeId === nodeId).map((relationship) => relationship.source),
      issues: buildFixtureRelationshipIssues().filter((issue) => issue.nodeId === nodeId)
    };
  },

  // The web/dev shell has no DPAPI; keep the discovery snapshot in memory only so no
  // inventory data is ever written to plaintext browser storage.
  async cacheDiscoverySnapshot(snapshot: string): Promise<void> {
    fixtureDiscoverySnapshot = snapshot.length > 0 ? snapshot : null;
  },

  async readDiscoverySnapshot(): Promise<string | null> {
    return fixtureDiscoverySnapshot;
  },

  async projectGraphMap(projectId: number, limit = 300): Promise<GraphMap> {
    const project = projects.find((item) => item.id === projectId);
    if (!project) {
      return { projectId, nodes: [], edges: [], issues: [], totalNodes: 0, totalEdges: 0, totalIssues: 0, partial: false };
    }
    // Keep expanded fixture pages observable long enough to exercise pause/resume
    // in browser QA. The desktop backend does not use this synthetic delay.
    if (limit > 300) {
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    const base = `${project.path}`;
    const coreNodes = [
      {
        nodeId: project.id, projectId: project.id, path: project.path, displayName: project.name,
        itemKind: "project", graphKind: "project", confidence: "High", details: [],
        physicalBytes: null, protectedOrSensitive: false, sharedProjectIds: [project.id]
      },
      {
        nodeId: 90001, projectId: project.id, path: `${base}/workflows/portrait.workflow.json`, displayName: "portrait.workflow.json",
        itemKind: "file", graphKind: "workflow", confidence: "High", details: ["Workflow JSON", "1 model reference"],
        physicalBytes: 4096, protectedOrSensitive: false, sharedProjectIds: [project.id]
      },
      {
        nodeId: 90002, projectId: project.id, path: `${base}/models/checkpoints/sdxl_base.safetensors`, displayName: "sdxl_base.safetensors",
        itemKind: "file", graphKind: "model:checkpoint", confidence: "High",
        details: ["Safetensors header", "Architecture: stable-diffusion-xl-base-v1", "2 tensors"],
        physicalBytes: 6_938_000_000, protectedOrSensitive: false, sharedProjectIds: [project.id]
      },
      {
        nodeId: 90003, projectId: project.id, path: `${base}/models/loras/unused_style.safetensors`, displayName: "unused_style.safetensors",
        itemKind: "file", graphKind: "model:lora", confidence: "High",
        details: ["Safetensors header", "Title: Unused Style LoRA", "1 tensor"],
        physicalBytes: 151_000_000, protectedOrSensitive: false, sharedProjectIds: [project.id]
      },
      {
        nodeId: 90004, projectId: project.id, path: `${base}/models/vae/shared_vae.safetensors`, displayName: "shared_vae.safetensors",
        itemKind: "file", graphKind: "model:vae", confidence: "High", details: ["Safetensors header", "2 tensors"],
        physicalBytes: 335_000_000, protectedOrSensitive: false, sharedProjectIds: [project.id, project.id + 1000]
      },
      {
        nodeId: 90005, projectId: project.id, path: `${base}/.cache/huggingface`, displayName: "huggingface",
        itemKind: "directory", graphKind: "cache", confidence: "High", details: ["Hugging Face cache", "shared by default"],
        physicalBytes: 12_000_000_000, protectedOrSensitive: false, sharedProjectIds: [project.id]
      }
    ];
    // Browser-only stress fixture: reproduce a dependency-heavy project without
    // touching the user's real inventory. The first request stays bounded at 300;
    // the Hangar Map can then exercise complete-map loading, pause/resume and the
    // separation between direct review signals and vendored cache observations.
    const cacheNodes = Array.from({ length: 1360 }, (_, index) => {
      const nodeId = 91_000 + index;
      const workflow = index % 12 === 0;
      const path = workflow
        ? `${base}/.local/cargo/registry/src/package-${index}/workflows/example-${index}.json`
        : `${base}/.local/cargo/registry/cache/models/dependency-${index}.safetensors`;
      return {
        nodeId,
        projectId: project.id,
        path,
        displayName: path.split("/").pop() ?? path,
        itemKind: "file",
        graphKind: workflow ? "workflow" : "model:checkpoint",
        confidence: "Low",
        details: ["Dependency-cache fixture"],
        physicalBytes: workflow ? 2048 : 1_048_576,
        protectedOrSensitive: false,
        sharedProjectIds: [project.id]
      };
    });
    const nodes = [...coreNodes, ...cacheNodes];
    const edges = [
      { sourceNodeId: 90001, targetNodeId: 90002, kind: "workflow_references_model", confidence: "High", evidence: "ckpt_name: sdxl_base.safetensors" }
    ];
    const issues = Array.from({ length: 348 }, (_, index) => {
      const source = cacheNodes[(index * 12) % cacheNodes.length];
      return {
        nodeId: source.nodeId,
        projectId: project.id,
        sourcePath: source.path,
        kind: "missing_model_reference",
        confidence: "Low",
        target: `dependency-model-${index}.safetensors`,
        evidence: "Dependency-cache stress fixture; not a direct project review signal."
      };
    });
    return {
      projectId,
      nodes: nodes.slice(0, limit),
      edges,
      issues,
      totalNodes: nodes.length,
      totalEdges: edges.length,
      totalIssues: issues.length,
      partial: false
    };
  },

  async graphOrphans(limit = 50): Promise<OrphanCandidates> {
    return fixtureApi.orphanAssetCandidates({ limit });
  },

  async orphanAssetCandidates(filters: { minSizeBytes?: number; projectId?: number | null; assetKind?: string; minConfidence?: string; includePartial?: boolean; limit?: number; includeFixtureProjects?: boolean }): Promise<OrphanCandidates> {
    const referenced = new Set(buildFixtureRelationships().map((relationship) => relationship.targetNodeId));
    const limit = filters.limit ?? 50;
    const candidates = files
      .filter((file) => !file.isContext && !file.isSensitive && !file.protectedLevel && !referenced.has(file.nodeId))
      .filter((file) => filters.includeFixtureProjects !== false || projects.some((project) => project.id === file.projectId && project.source !== "fixture"))
      .filter((file) => !filters.projectId || file.projectId === filters.projectId)
      .map(fileToOrphanCandidate)
      .filter((candidate): candidate is OrphanCandidate => candidate !== null)
      .filter((candidate) => (candidate.physicalBytes ?? 0) >= (filters.minSizeBytes ?? 0))
      .filter((candidate) => confidenceRank(candidate.confidence) >= confidenceRank(filters.minConfidence ?? "Low"))
      .filter((candidate) => assetKindMatches(filters.assetKind ?? "all", assetKindForPath(candidate.path)));
    return {
      candidates: candidates.slice(0, limit),
      total: candidates.length
    };
  },

  async lostProjectCandidates(filters: { minSizeBytes?: number; projectId?: number | null; stalePreset?: string; signals?: string[]; keyword?: string; includePartial?: boolean; limit?: number; includeFixtureProjects?: boolean }): Promise<LostProjectCandidates> {
    const limit = filters.limit ?? 50;
    const requestedSignals = filters.signals ?? [];
    const keyword = filters.keyword?.trim().toLowerCase() ?? "";
    const candidates = projects
      .filter((project) => filters.includeFixtureProjects !== false || project.source !== "fixture")
      .filter((project) => !filters.projectId || project.id === filters.projectId)
      .map((project): LostProjectCandidate => {
        const projectFiles = files.filter((file) => file.projectId === project.id);
        const apparentBytes = projectFiles.reduce((total, file) => total + (file.body?.length ?? 0), 0);
        const signals = lostFixtureSignals(project, projectFiles);
        if (keyword && `${project.name} ${project.path}`.toLowerCase().includes(keyword)) {
          signals.push("keyword_match");
        }
        const confidence = signals.length >= 3 ? "High" : signals.length >= 2 ? "Medium" : "Low";
        return {
          projectId: project.id,
          nodeId: project.id,
          navId: null,
          candidateKind: "project",
          displayName: project.name,
          path: project.path,
          confidence,
          reason: `Passive project-review signal rated ${confidence} based on ${signals.join(", ")}.`,
          signals,
          apparentBytes,
          physicalBytes: apparentBytes,
          footprintPartial: false
        };
      })
      .filter((candidate) => (candidate.physicalBytes ?? 0) >= (filters.minSizeBytes ?? 0))
      .filter((candidate) => !keyword || candidate.signals.includes("keyword_match"))
      .filter((candidate) => requestedSignals.length === 0 || requestedSignals.every((signal) => candidate.signals.includes(signal)))
      .filter((candidate) => {
        if (filters.stalePreset === "quiet" || filters.stalePreset === "forgotten") return candidate.signals.includes("no_recent_opens");
        if (filters.stalePreset === "unfinished") return candidate.signals.includes("no_context") || candidate.signals.includes("name_markers");
        if (filters.stalePreset === "untracked") return candidate.signals.includes("git_absent");
        if (filters.stalePreset === "suspicious") return candidate.signals.length >= 2;
        if (filters.stalePreset === "custom") return requestedSignals.length > 0 || Boolean(keyword);
        return candidate.signals.length > 0;
      });
    return {
      candidates: candidates.slice(0, limit),
      total: candidates.length
    };
  },

  async projectDiscoveryReport(limit = 100): Promise<ProjectDiscoveryReport> {
    const candidates = projects.map((project, index) => ({
      path: project.path,
      displayName: project.name,
      projectKind: index === 0 ? "ai_assisted_project" : "documentation_project",
      confidence: index === 0 ? "High" : "Medium",
      score: index === 0 ? 92 : 54,
      sourceKinds: index === 0 ? ["known_folder", "codex_sessions"] : ["known_folder"],
      signals: [
        {
          kind: "readme",
          label: "README project context",
          detail: "README.md",
          confidence: "High"
        },
        {
          kind: "already_registered",
          label: "Already registered in Code Hangar",
          detail: null,
          confidence: "High"
        }
      ],
      alreadyRegistered: true,
      existingProjectId: project.id,
      overlapKind: "already_registered",
      nestedUnderRegistered: null,
      containsRegisteredRoots: [],
      estimatedFiles: files.filter((file) => file.projectId === project.id).length,
      estimatedBytes: files
        .filter((file) => file.projectId === project.id)
        .reduce((total, file) => total + (file.body?.length ?? 0), 0),
      estimatePartial: visualAcceptanceState === "partial" && project.id === projects[0]?.id
    }));
    const sessions: ProjectDiscoveryReport["sessions"] = visualAcceptanceState === "saturated"
      ? Array.from({ length: 240 }, (_, index) => {
        const project = projects[index % projects.length];
        const ordinal = String(index + 1).padStart(3, "0");
        return {
          path: `fixture://codex/sessions/saturated-${ordinal}.jsonl`,
          displayName: `Long acceptance session ${ordinal} with a descriptive title`,
          sourceKind: index % 3 === 0 ? "claude_code_sessions" : index % 3 === 1 ? "cursor_sessions" : "codex_sessions",
          sourceLabel: "Saturated fixture sessions",
          sessionKind: index % 3 === 0 ? "Claude" : index % 3 === 1 ? "Cursor" : "ChatGPT",
          confidence: "High",
          linkedProjectPaths: [project.path],
          linkedRegisteredProjectIds: [project.id],
          association: "registered_project"
        };
      })
      : projects.length > 0
        ? [
          {
            path: "fixture://codex/sessions/fixture-markdown.jsonl",
            displayName: "fixture-markdown.jsonl",
            sourceKind: "codex_sessions",
            sourceLabel: "Fixture ChatGPT sessions",
            sessionKind: "ChatGPT",
            confidence: "High",
            linkedProjectPaths: [projects[0].path],
            linkedRegisteredProjectIds: [projects[0].id],
            association: "registered_project"
          },
          {
            path: "fixture://claude/sessions/fixture-markdown-review.json",
            displayName: "fixture-markdown-review.json",
            sourceKind: "claude_code_sessions",
            sourceLabel: "Fixture Claude sessions",
            sessionKind: "Claude",
            confidence: "High",
            linkedProjectPaths: [projects[0].path],
            linkedRegisteredProjectIds: [projects[0].id],
            association: "registered_project"
          }
        ]
        : [];
    return {
      candidates: limit === 0 ? candidates : candidates.slice(0, limit),
      sessions,
      searchedLocations: [
        {
          sourceKind: "known_folder",
          sourceLabel: "Fixture known folders",
          path: "fixture://projects",
          exists: true,
          detail: "Browser fixture data. The Tauri build reads local folders and session metadata."
        },
        {
          sourceKind: "codex_sessions",
          sourceLabel: "Fixture ChatGPT sessions",
          path: "fixture://codex/sessions",
          exists: true,
          detail: "Simulated local transcript metadata."
        }
      ],
      durationMs: 12,
      totalCandidates: candidates.length,
      totalSessions: sessions.length
    };
  },

  async projectDiscoveryDeepScan(rootPath: string, limit = 250): Promise<ProjectDiscoveryReport> {
    const report = await this.projectDiscoveryReport(limit);
    return {
      ...report,
      searchedLocations: [
        {
          sourceKind: "deep_scan_root",
          sourceLabel: "Fixture deep scan root",
          path: rootPath,
          exists: true,
          detail: "Simulated recursive project discovery under a selected folder."
        },
        ...report.searchedLocations
      ],
      durationMs: report.durationMs + 8
    };
  },

  async detectInstalledApps(): Promise<InstalledApp[]> {
    // A representative mix so the dev UI exercises the "only show installed" path.
    return [
      { id: "claude", label: "Claude Code", present: true },
      { id: "codex", label: "ChatGPT", present: true },
      { id: "cursor", label: "Cursor", present: true },
      { id: "antigravity", label: "Antigravity", present: false },
      { id: "gemini", label: "Gemini CLI", present: false },
      { id: "windsurf", label: "Windsurf", present: false },
      { id: "openclaw", label: "OpenClaw", present: false },
      { id: "hermes", label: "Hermes / NemoClaw", present: false },
      { id: "pinokio", label: "Pinokio", present: false }
    ];
  },

  async wslScanEnabled(): Promise<boolean> {
    return false;
  },

  async setWslScanEnabled(_enabled: boolean): Promise<void> {
    // No persisted setting in the fixture backend.
  },

  async duplicateCandidates(filters: { minSizeBytes?: number; projectId?: number | null; fileKind?: string; currentFileNodeId?: number | null; limit?: number; includeFixtureProjects?: boolean } = {}): Promise<DuplicateCandidates> {
    const groups = buildFixtureDuplicateCandidates(filters);
    const limit = filters.limit ?? 25;
    const visibleGroups = limit === 0 ? groups : groups.slice(0, limit);
    return {
      groups: visibleGroups,
      total: groups.length
    };
  },

  async confirmDuplicateGroup(nodeId: number): Promise<DuplicateConfirmation> {
    const groups = buildFixtureDuplicateCandidates({ minSizeBytes: 0 });
    const group = groups.find((candidate) => candidate.members.some((member) => member.nodeId === nodeId));
    if (!group) {
      return {
        targetNodeId: nodeId,
        confirmedGroups: [],
        checkedFiles: 0,
        bytesHashed: 0,
        reclaimableBytes: 0,
        partial: false
      };
    }
    const physicalValues = group.members.map((member) => member.physicalBytes ?? 0);
    const total = physicalValues.reduce((acc, value) => acc + value, 0);
    const kept = physicalValues.length > 0 ? Math.max(...physicalValues) : 0;
    const reclaimable = Math.max(0, total - kept);
    return {
      targetNodeId: nodeId,
      confirmedGroups: [
        {
          fullHash: `fixture-full-${group.hashPartial}`,
          sizeBytes: group.sizeBytes,
          memberCount: group.members.length,
          reclaimableBytes: reclaimable,
          confidence: "High",
          members: group.members
        }
      ],
      checkedFiles: group.members.length,
      bytesHashed: group.sizeBytes * group.members.length,
      reclaimableBytes: reclaimable,
      partial: false
    };
  },

  async nodeOrphanStatus(nodeId: number): Promise<OrphanStatus> {
    const file = files.find((candidate) => candidate.nodeId === nodeId);
    if (!file) {
      return {
        nodeId,
        evaluated: false,
        isCandidate: false,
        candidateKind: null,
        confidence: null,
        reason: "File is not in the fixture inventory.",
        incomingReferences: 0,
        protectedOrSensitive: false,
        physicalBytes: null,
        footprintPartial: false
      };
    }
    const incomingReferences = buildFixtureRelationships().filter((relationship) => relationship.targetNodeId === nodeId).length;
    const detectedKind = assetKindForPath(file.path);
    const orphanKind = ["image", "video", "media", "model", "data"].includes(detectedKind) ? detectedKind : null;
    const isCandidate = Boolean(orphanKind) && !file.isContext && !file.isSensitive && !file.protectedLevel && incomingReferences === 0;
    return {
      nodeId,
      evaluated: true,
      isCandidate,
      candidateKind: isCandidate ? orphanKind : null,
      confidence: isCandidate ? (file.path.includes("unused") ? "High" : "Medium") : null,
      reason: isCandidate
        ? "No known local references and asset-like path."
        : file.isContext
          ? "Priority context files are not treated as orphan assets."
          : incomingReferences > 0
            ? "This file has known local references."
            : file.isSensitive || file.protectedLevel
              ? "Sensitive or protected files are excluded from orphan searches."
              : "This file type is not currently classified as a reviewable orphan asset.",
      incomingReferences,
      protectedOrSensitive: file.isSensitive || Boolean(file.protectedLevel),
      physicalBytes: file.body?.length ?? null,
      footprintPartial: false
    };
  },

  async projectRecoverableSummary(projectId: number): Promise<RecoverableSummary> {
    return fixtureRecoverableSummary(projectId);
  },

  async nodeRecoverableSummary(nodeId: number): Promise<RecoverableSummary> {
    const file = files.find((candidate) => candidate.nodeId === nodeId);
    return fixtureRecoverableSummary(file?.projectId ?? nodeId, nodeId);
  },

  async operationPlanBuild(targetNodeId: number, actionLabel: string): Promise<OperationPlan> {
    return fixtureOperationPlan(targetNodeId, actionLabel);
  },

  async riskReportBuild(plan: OperationPlan): Promise<RiskReport> {
    return fixtureRiskReport(plan);
  },

  async riskReportBuildForTarget(targetNodeId: number, actionLabel: string): Promise<RiskReport> {
    return fixtureRiskReport(fixtureOperationPlan(targetNodeId, actionLabel));
  },

  async riskReportExport(report: RiskReport, path: string): Promise<ExportResult> {
    return { path, bytesWritten: JSON.stringify(report, null, 2).length };
  },

  async recentItemsList(limit = 20): Promise<RecentItem[]> {
    return recentItems.slice(0, limit);
  },

  async pinnedItemsList(): Promise<PinnedItem[]> {
    return pinnedItems;
  },

  async pinItem(nodeId: number, itemKind: string): Promise<void> {
    if (!pinnedItems.some((item) => item.nodeId === nodeId && item.itemKind === itemKind)) {
      const file = files.find((candidate) => candidate.nodeId === nodeId);
      pinnedItems = [{ nodeId, projectId: file?.projectId ?? null, itemKind, path: file?.path ?? "", pinnedAt: new Date().toISOString() }, ...pinnedItems];
    }
  },

  async unpinItem(nodeId: number, itemKind: string): Promise<void> {
    pinnedItems = pinnedItems.filter((item) => !(item.nodeId === nodeId && item.itemKind === itemKind));
  },

  async commentsForNode(nodeId: number): Promise<Comment[]> {
    return comments.filter((comment) => comment.nodeId === nodeId);
  },

  async commentsCountForNode(nodeId: number): Promise<number> {
    return comments.filter((comment) => comment.nodeId === nodeId).length;
  },

  async commentAdd(nodeId: number, body: string, author?: string, source?: string): Promise<Comment> {
    const trimmed = body.trim();
    if (!trimmed) throw new Error("A comment cannot be empty.");
    const now = new Date().toISOString();
    const file = files.find((candidate) => candidate.nodeId === nodeId);
    const comment: Comment = {
      id: nextCommentId++,
      nodeId,
      projectId: file?.projectId ?? null,
      body: trimmed,
      author: author?.trim() || "user",
      source: source?.trim() || "user",
      createdAt: now,
      updatedAt: now,
    };
    comments = [...comments, comment];
    return comment;
  },

  async commentEdit(commentId: number, body: string): Promise<Comment> {
    const trimmed = body.trim();
    if (!trimmed) throw new Error("A comment cannot be empty.");
    const existing = comments.find((comment) => comment.id === commentId);
    if (!existing) throw new Error("Comment not found.");
    existing.body = trimmed;
    existing.updatedAt = new Date().toISOString();
    return existing;
  },

  async commentDelete(commentId: number): Promise<void> {
    comments = comments.filter((comment) => comment.id !== commentId);
  },

  async rootsList(): Promise<ScanRoot[]> {
    return scanRoots;
  },

  async rootsAdd(path: string): Promise<ScanRoot> {
    const existing = scanRoots.find((root) => root.path === path);
    if (existing) return existing;
    const root = { id: scanRoots.length + 1, path, enabled: true, lastScannedAt: null };
    scanRoots = [...scanRoots, root];
    if (!projects.some((project) => project.path === path)) {
      const parts = path.replace(/\\/g, "/").split("/").filter(Boolean);
      projects = [
        ...projects,
        {
          id: 1000 + root.id,
          name: parts.at(-1) ?? path,
          path,
          source: "scan",
          contextCount: 0,
          pinned: false,
          protectedLevel: null,
          scanState: "outdated",
          scanRootId: root.id
        }
      ];
    }
    return root;
  },

  async rootsSetEnabled(rootId: number, enabled: boolean): Promise<ScanRoot> {
    scanRoots = scanRoots.map((root) => root.id === rootId ? { ...root, enabled } : root);
    return scanRoots.find((root) => root.id === rootId) ?? { id: rootId, path: "", enabled, lastScannedAt: null };
  },

  async rootsUnregister(rootId: number): Promise<void> {
    const removed = scanRoots.find((root) => root.id === rootId);
    scanRoots = scanRoots.filter((root) => root.id !== rootId);
    if (removed) {
      projects = projects.filter((project) => project.scanRootId !== rootId && project.path !== removed.path);
    }
  },

  async projectsUnregister(projectId: number): Promise<void> {
    projects = projects.filter((project) => project.id !== projectId);
  },

  async resetAllProjects(): Promise<number> {
    const removed = projects.filter((project) => project.source !== "fixture").length;
    projects = projects.filter((project) => project.source === "fixture");
    scanRoots = [];
    return removed;
  },

  async compactDatabase(): Promise<DbMaintenanceReport> {
    // No real file to shrink in the browser fixture shell; report a representative reclaim.
    return { beforeBytes: 1_258_291_200, afterBytes: 134_217_728, freedBytes: 1_124_073_472 };
  },

  async restartApp(): Promise<void> {
    // In the browser fixture shell there is no process to relaunch; reload to
    // simulate a fresh start.
    if (typeof window !== "undefined") {
      window.location.reload();
    }
  },

  async scanStart(_rootIds?: number[]): Promise<string> {
    return "browser-fixture-scan";
  },

  async scanResumeSubtree(_navId: number): Promise<string> {
    return "browser-fixture-subtree-scan";
  },

  async scanCancel(_jobId: string): Promise<void> {
    return;
  },

  async scanStatus(jobId: string) {
    const now = Date.now();
    return { jobId, state: "completed", scanPhase: "completed", scannedFiles: 0, indexedDocuments: 0, startedAtMs: now, phaseStartedAtMs: now, lastProgressAtMs: now, updatedAtMs: now, estimatedTotalFiles: null, estimatedTotalBytes: null, workerCount: null, partial: false, rootIds: [], rootPaths: [], currentPath: null, error: null, message: "Browser preview uses fixture data. Tauri scans real folders." };
  },

  async zonesList(): Promise<ProtectedZone[]> {
    return [
      { id: 1, patternType: "glob", pattern: "**/.git/**", level: "no_preview", source: "builtin" },
      { id: 2, patternType: "glob", pattern: "**/.env", level: "no_preview", source: "builtin" },
      { id: 3, patternType: "glob", pattern: "**/*credential*", level: "no_preview", source: "builtin" }
    ];
  },

  async securityStatus(): Promise<SecurityStatus> {
    return {
      outboundNetwork: connectorAcceptanceFixture
        ? "available only through explicit Connector actions in the desktop runtime"
        : "not available in the fixture/core preview",
      mutationExecutor: "not available in fixture mode; Local builds gate disk actions explicitly",
      agentIpc: connectorAcceptanceFixture
        ? "Connector acceptance fixture; app registration requires the desktop runtime"
        : "not available in the fixture/core preview",
      activeFeatures: connectorAcceptanceFixture ? ["core", "agent_automation"] : ["core"],
      notes: ["Markdown preview blocks scripts and remote images.", "Sensitive and Protected Zone files are excluded from preview and FTS."]
    };
  },

  async mutationModeStatus(): Promise<boolean> {
    return false;
  },

  async mutationFinalRemoveEnabled(): Promise<boolean> {
    return false;
  },

  async recoveryPending(): Promise<RecoveryPending> {
    return {
      enabled: false,
      pending: false,
      operations: [],
      message: "Browser fixture mode has no recovery journal."
    };
  },

  async recoveryResolve(_decision: "rollback"): Promise<RecoveryResolveResult> {
    return {
      action: "rollback",
      recoveredOperations: 0,
      rolledBackItems: 0,
      message: "Browser fixture mode has no recovery journal."
    };
  },

  async mutationTokenIssue(action: "enter_mutation_mode" | "final_remove"): Promise<MutationTokenResult> {
    return { action, token: `fixture-${action}` };
  },

  async mutationBackupStart(
    plan: OperationPlan,
    destinationRoot: string,
    _level: "minimal" | "standard" | "full",
    _allowSameVolume: boolean,
    _includeProtected: boolean,
    _token: string
  ): Promise<MutationBackupSummary> {
    return {
      backupId: 1,
      manifestPath: `${destinationRoot}/codehangar-backup-manifest.json`,
      totalBytes: plan.recoverableBytes.total,
      verified: true,
      itemCount: plan.items.length
    };
  },

  async mutationMoveStart(plan: OperationPlan, holdingRoot: string, _verifiedBackupId: number, _includeProtected: boolean, _token: string): Promise<MutationMoveSummary> {
    return {
      operationId: 1,
      entries: plan.items.map((item, index) => ({
        originalPath: item.path,
        storedPath: `${holdingRoot}/${item.displayName || `item-${index}`}`,
        outcome: "Moved",
        bytes: item.sizeApparent,
        spaceRecovered: 0,
        detail: null
      })),
      spaceRecovered: 0,
      moved: plan.items.length,
      skipped: 0,
      failed: 0,
      removedDirs: 0,
      removedLinks: 0
    };
  },

  async mutationPreviewProtected(_plan: OperationPlan): Promise<MutationProtectedPreview> {
    return { protected: [], reparse: [] };
  },

  async mutationRestoreStart(entryId: number, _token: string): Promise<MutationRestoreSummary> {
    return {
      entryId,
      outcome: "restored",
      originalPath: "fixture://restored",
      restoredPath: "fixture://restored",
      conflictPath: null
    };
  },

  async mutationRestoreToFolderStart(entryId: number, destinationRoot: string, _token: string): Promise<MutationRestoreSummary> {
    return {
      entryId,
      outcome: "restored_elsewhere",
      originalPath: "fixture://original",
      restoredPath: `${destinationRoot}/fixture-restored`,
      conflictPath: null
    };
  },

  async mutationFinalRemoveStart(entryId: number, _token: string): Promise<MutationFinalRemoveSummary> {
    return { entryId, freedBytes: 0 };
  },

  async mutationActivityLog(_limit = 50): Promise<MutationActivityLog> {
    return {
      enabled: false,
      operations: [],
      items: [],
      backups: [],
      storedEntries: [],
      message: "Browser fixture mode has no mutation journal."
    };
  },

  async mutationLockInspectPath(path: string): Promise<MutationLockInspection> {
    return { path, state: "unavailable" };
  }
};

function fixtureRecoverableSummary(projectId: number, targetNodeId = projectId): RecoverableSummary {
  const projectFiles = files.filter((file) => file.projectId === projectId);
  const safeFiles = projectFiles.filter((file) => !file.isSensitive && !file.protectedLevel);
  const owned = safeFiles.reduce((total, file) => total + (file.body?.length ?? 0), 0);
  const target = files.find((file) => file.nodeId === targetNodeId);
  return {
    targetNodeId,
    projectId,
    targetPath: target?.path ?? projects.find((project) => project.id === projectId)?.path ?? "",
    targetKind: target ? "file" : "project",
    recoverableBytes: {
      owned,
      orphanedOnRemoval: 0,
      total: owned,
      partial: false
    },
    sharedCount: 0,
    protectedCount: projectFiles.filter((file) => file.protectedLevel).length,
    sensitiveCount: projectFiles.filter((file) => file.isSensitive).length,
    partialFootprint: false
  };
}

function fixtureOperationPlan(targetNodeId: number, actionLabel: string): OperationPlan {
  const file = files.find((candidate) => candidate.nodeId === targetNodeId);
  const project = projects.find((candidate) => candidate.id === (file?.projectId ?? targetNodeId)) ?? projects[0];
  const projectFiles = file ? [file] : files.filter((candidate) => candidate.projectId === project.id);
  const safeFiles = projectFiles.filter((candidate) => !candidate.isSensitive && !candidate.protectedLevel);
  const recoverable = fixtureRecoverableSummary(project.id, targetNodeId);
  const items = safeFiles.length > 4 && !file
    ? [{
      nodeId: project.id,
      path: project.path,
      displayName: project.name,
      itemKind: "recursive_dir",
      actionLabel,
      risk: "yellow" as const,
      confidence: "Medium",
      sizeApparent: recoverable.recoverableBytes.total,
      physicalBytes: recoverable.recoverableBytes.total,
      hardlinkGroup: null,
      freesSpace: recoverable.recoverableBytes.total > 0,
      recursiveDir: true,
      childCount: projectFiles.length,
      partial: false
    }]
    : projectFiles.map((candidate) => ({
      nodeId: candidate.nodeId,
      path: candidate.path,
      displayName: candidate.displayName,
      itemKind: "file",
      actionLabel,
      risk: candidate.isSensitive || candidate.protectedLevel ? "black" as const : candidate.path.includes("assets/") ? "orange" as const : "yellow" as const,
      confidence: "Medium",
      sizeApparent: candidate.body?.length ?? 0,
      physicalBytes: candidate.isSensitive || candidate.protectedLevel ? null : candidate.body?.length ?? 0,
      hardlinkGroup: null,
      freesSpace: !candidate.isSensitive && !candidate.protectedLevel,
      recursiveDir: false,
      childCount: 0,
      partial: false
    }));
  return {
    planId: `fixture-preview-${targetNodeId}`,
    schema: "operation_plan/1",
    createdAt: new Date().toISOString(),
    target: {
      nodeId: targetNodeId,
      projectId: project.id,
      kind: file ? "file" : "project",
      path: file?.path ?? project.path,
      displayName: file?.displayName ?? project.name
    },
    actionLabel,
    items,
    recoverableBytes: recoverable.recoverableBytes,
    sharedAssets: [],
    danglingAfter: buildFixtureRelationshipIssues().map((issue) => ({
      referrerNodeId: issue.nodeId,
      path: files.find((candidate) => candidate.nodeId === issue.nodeId)?.path ?? "",
      missingPath: issue.target,
      confidence: issue.confidence,
      projectId: project.id,
      projectName: project.name,
      dependencyKind: "reference",
      crossProject: false
    })),
    sensitiveFiles: projectFiles.filter((candidate) => candidate.isSensitive).map((candidate) => ({
      nodeId: candidate.nodeId,
      path: candidate.path,
      signature: "Sensitive fixture marker"
    })),
    protectedHits: projectFiles.filter((candidate) => candidate.protectedLevel).map((candidate) => ({
      nodeId: candidate.nodeId,
      path: candidate.path,
      level: candidate.protectedLevel ?? "protected"
    })),
    gitWarnings: project.id === 3 ? [{
      projectId: project.id,
      message: "Local Git metadata was detected. Review local state outside Code Hangar before relying on this review.",
      confidence: "Medium"
    }] : [],
    confidenceSummary: { high: 0, medium: items.length, low: 0, unknown: 0 },
    recommendedAction: "Use this review as evidence, not an action queue. Focus on shared references, protected paths and incomplete scan areas before deciding what matters.",
    readOnlyPreview: true,
    planStale: false,
    partialFootprint: false,
    danglingTruncated: false,
    externalServicesUnaffected: true,
    targetFingerprint: `fixture:${targetNodeId}:${items.length}:${recoverable.recoverableBytes.total}`
  };
}

function fixtureRiskReport(plan: OperationPlan): RiskReport {
  const riskCounts = ["green", "yellow", "orange", "red", "black"].map((tier) => {
    const tierItems = plan.items.filter((item) => item.risk === tier);
    return {
      tier: tier as "green" | "yellow" | "orange" | "red" | "black",
      count: tierItems.length,
      physicalBytes: tierItems.reduce((total, item) => total + (item.physicalBytes ?? 0), 0)
    };
  }).filter((row) => row.count > 0);
  return {
    schema: "risk_report/1",
    generatedAt: new Date().toISOString(),
    target: plan.target,
    actionLabel: plan.actionLabel,
    readOnlyPreview: true,
    externalServicesUnaffected: true,
    recoverableBytes: plan.recoverableBytes,
    riskCounts,
    sharedAssets: plan.sharedAssets,
    danglingAfter: plan.danglingAfter,
    danglingTruncated: plan.danglingTruncated,
    sensitiveFiles: plan.sensitiveFiles,
    protectedHits: plan.protectedHits,
    gitWarnings: plan.gitWarnings,
    confidenceSummary: plan.confidenceSummary,
    recommendedAction: plan.recommendedAction,
    caveats: ["Preview only: no filesystem action was run.", "External services are unaffected."]
  };
}

function fileToNavItem(file: FixtureFile): NavItem {
  const partial = visualAcceptanceState === "partial" && file.projectId === projects[0]?.id;
  return {
    id: file.nodeId,
    projectId: file.projectId,
    nodeId: file.nodeId,
    parentNavId: null,
    path: file.path,
    displayPath: file.path,
    displayName: file.displayName,
    itemKind: "file",
    priority: priorityFor(file.path),
    isContext: file.isContext,
    isMarkdown: file.isMarkdown,
    isSensitive: file.isSensitive,
    protectedLevel: file.protectedLevel,
    childCount: 0,
    fullyScanned: !partial,
    collapseDefault: file.path.includes("node_modules") || file.path.startsWith(".git/"),
    scanError: null,
    aggregateApparentBytes: file.body?.length ?? 0,
    aggregateAllocatedBytes: null,
    aggregatePhysicalBytes: file.body?.length ?? 0,
    aggregateBytesPartial: partial,
    // Deterministic, spread-out mtimes so the "sort by Date" control is demoable
    // in fixture/dev mode (no real disk timestamps here).
    modifiedAt: String(1_700_000_000 + file.nodeId * 3600),
    children: []
  };
}

function buildTree(items: NavItem[]): NavItem[] {
  const dirs = new Map<string, NavItem>();
  const roots: NavItem[] = [];

  for (const item of items) {
    const parts = item.path.split("/");
    if (parts.length === 1) {
      roots.push(item);
      continue;
    }
    let parentChildren = roots;
    let currentPath = "";
    for (const part of parts.slice(0, -1)) {
      currentPath = currentPath ? `${currentPath}/${part}` : part;
      let dir = dirs.get(currentPath);
      if (!dir) {
        dir = {
          id: -dirs.size - 1,
          projectId: item.projectId,
          nodeId: null,
          parentNavId: null,
          path: currentPath,
          displayPath: currentPath,
          displayName: part,
          itemKind: "directory",
          priority: 10,
          isContext: false,
          isMarkdown: false,
          isSensitive: false,
          protectedLevel: null,
          childCount: 0,
          fullyScanned: !(visualAcceptanceState === "partial" && item.projectId === projects[0]?.id),
          collapseDefault: currentPath === ".git" || currentPath.includes("node_modules"),
          scanError: null,
          aggregateApparentBytes: 0,
          aggregateAllocatedBytes: null,
          aggregatePhysicalBytes: 0,
          aggregateBytesPartial: visualAcceptanceState === "partial" && item.projectId === projects[0]?.id,
          children: []
        };
        dirs.set(currentPath, dir);
        parentChildren.push(dir);
      }
      parentChildren = dir.children;
    }
    parentChildren.push(item);
  }

  return sortNav(roots);
}

function sortNav(items: NavItem[]): NavItem[] {
  return items
    .map((item) => {
      const children = sortNav(item.children);
      if (item.itemKind !== "directory") {
        return { ...item, childCount: children.length || item.childCount, children };
      }
      const aggregateApparentBytes = children.reduce((total, child) => total + (child.aggregateApparentBytes ?? 0), item.aggregateApparentBytes ?? 0);
      const aggregatePhysicalBytes = children.reduce((total, child) => total + (child.aggregatePhysicalBytes ?? 0), item.aggregatePhysicalBytes ?? 0);
      return {
        ...item,
        childCount: children.length || item.childCount,
        aggregateApparentBytes,
        aggregatePhysicalBytes,
        aggregateBytesPartial: children.some((child) => child.aggregateBytesPartial),
        children
      };
    })
    .sort((left, right) => left.priority - right.priority || left.displayName.localeCompare(right.displayName));
}

function findNavItem(items: NavItem[], id: number): NavItem | null {
  for (const item of items) {
    if (item.id === id) return item;
    const child = findNavItem(item.children, id);
    if (child) return child;
  }
  return null;
}

function stripChildren(item: NavItem): NavItem {
  return { ...item, children: [] };
}

function buildFixtureRelationships(): Array<{ sourceNodeId: number; targetNodeId: number; source: NodeRelationship; target: NodeRelationship }> {
  const relationships: Array<{ sourceNodeId: number; targetNodeId: number; source: NodeRelationship; target: NodeRelationship }> = [];
  for (const source of files) {
    if (!source.body || source.isSensitive || source.protectedLevel) continue;
    const rendered = renderMarkdownSafe(source.body);
    for (const link of rendered.links.filter((candidate) => !candidate.isRemote)) {
      const resolved = resolveFixturePath(source.path, link.target);
      const target = resolved ? files.find((file) => file.projectId === source.projectId && file.path === resolved) : null;
      if (!target || target.nodeId === source.nodeId) continue;
      const evidence = link.label ? `${link.label} -> ${link.target}` : link.target;
      relationships.push({
        sourceNodeId: source.nodeId,
        targetNodeId: target.nodeId,
        source: {
          nodeId: source.nodeId,
          projectId: source.projectId,
          path: source.path,
          displayName: source.displayName,
          itemKind: "file",
          kind: "markdown_links_to",
          confidence: "High",
          evidence
        },
        target: {
          nodeId: target.nodeId,
          projectId: target.projectId,
          path: target.path,
          displayName: target.displayName,
          itemKind: "file",
          kind: "markdown_links_to",
          confidence: "High",
          evidence
        }
      });
    }
  }
  return relationships;
}

function buildFixtureRelationshipIssues(): RelationshipIssue[] {
  const issues: RelationshipIssue[] = [];
  for (const source of files) {
    if (!source.body || source.isSensitive || source.protectedLevel) continue;
    const rendered = renderMarkdownSafe(source.body);
    for (const link of rendered.links.filter((candidate) => !candidate.isRemote)) {
      if (link.target.trim().startsWith("#")) continue;
      const resolved = resolveFixturePath(source.path, link.target);
      if (!resolved || files.some((file) => file.projectId === source.projectId && file.path === resolved)) continue;
      const evidence = link.label ? `${link.label} -> ${link.target}` : link.target;
      issues.push({
        nodeId: source.nodeId,
        projectId: source.projectId,
        kind: "unresolved_markdown_link",
        confidence: link.target.includes("..") ? "Low" : "Medium",
        target: link.target,
        evidence
      });
    }
  }
  return issues;
}

function fileToOrphanCandidate(file: FixtureFile): OrphanCandidate | null {
  const lower = file.path.toLowerCase();
  const extension = lower.split(".").pop() ?? "";
  const assetExtensions = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "mp4", "mov", "wav", "mp3", "safetensors", "ckpt", "gguf", "onnx", "zip", "parquet", "csv", "bin"]);
  if (!assetExtensions.has(extension)) return null;
  return {
    nodeId: file.nodeId,
    projectId: file.projectId,
    projectName: projects.find((project) => project.id === file.projectId)?.name ?? "Unknown project",
    path: file.path,
    displayName: file.displayName,
    confidence: lower.includes("unused") || lower.includes("orphan") ? "Medium" : "Low",
    reason: extension === "png" ? "Unreferenced image asset" : "Unreferenced asset",
    physicalBytes: file.body?.length ?? 0,
    footprintPartial: false
  };
}

function buildFixtureDuplicateCandidates(filters: { minSizeBytes?: number; projectId?: number | null; fileKind?: string; currentFileNodeId?: number | null; limit?: number; includeFixtureProjects?: boolean } = {}): DuplicateCandidates["groups"] {
  const currentFile = filters.currentFileNodeId ? files.find((file) => file.nodeId === filters.currentFileNodeId) : null;
  const bySize = new Map<number, FixtureFile[]>();
  for (const file of files) {
    if (file.isSensitive || file.protectedLevel || !file.body || file.body.length < (filters.minSizeBytes ?? 1024)) continue;
    if (filters.includeFixtureProjects === false && !projects.some((project) => project.id === file.projectId && project.source !== "fixture")) continue;
    if (filters.projectId && file.projectId !== filters.projectId) continue;
    if (currentFile && file.body.length !== currentFile.body?.length) continue;
    if (!assetKindMatches(filters.fileKind ?? "all", assetKindForPath(file.path))) continue;
    const lower = file.path.toLowerCase();
    if ([".git/", ".ssh/", "node_modules/", ".venv/", "venv/", "target/", "dist/", "build/", ".cache/", "__pycache__/"].some((part) => lower.includes(part))) continue;
    const group = bySize.get(file.body.length) ?? [];
    group.push(file);
    bySize.set(file.body.length, group);
  }

  let id = 1;
  return [...bySize.entries()]
    .flatMap(([sizeBytes, sameSize]) => {
      const byPartial = new Map<string, FixtureFile[]>();
      for (const file of sameSize) {
        const partial = fixturePartialHash(file.body ?? "");
        const group = byPartial.get(partial) ?? [];
        group.push(file);
        byPartial.set(partial, group);
      }
      return [...byPartial.entries()]
        .filter(([, members]) => members.length > 1)
        .filter(([, members]) => !currentFile || members.some((member) => member.nodeId === currentFile.nodeId))
        .map(([hashPartial, members]) => ({
          id: id++,
          sizeBytes,
          hashPartial,
          confidence: "Medium",
          reason: "Same apparent size and first 64 KB hash. Full hash confirmation is deferred.",
          memberCount: members.length,
          physicalBytes: members.reduce((total, member) => total + (member.body?.length ?? 0), 0),
          footprintPartial: false,
          members: members.map((member) => ({
            nodeId: member.nodeId,
            projectId: member.projectId,
            projectName: projects.find((project) => project.id === member.projectId)?.name ?? "Unknown project",
            path: member.path,
            displayName: member.displayName,
            physicalBytes: member.body?.length ?? 0,
            footprintPartial: false
          }))
        }));
    })
    .sort((left, right) => right.sizeBytes - left.sizeBytes || left.members[0].path.localeCompare(right.members[0].path));
}

function lostFixtureSignals(project: ProjectSummary, projectFiles: FixtureFile[]) {
  const signals: string[] = [];
  if (!recentItems.some((item) => item.projectId === project.id)) signals.push("no_recent_opens");
  if (!projectFiles.some((file) => file.isContext)) signals.push("no_context");
  if (project.id !== 3) signals.push("git_absent");
  if (["old", "draft", "test", "unused", "archive"].some((marker) => `${project.name}/${project.path}`.toLowerCase().includes(marker))) {
    signals.push("name_markers");
  }
  return signals;
}

function isStrongProtectedFixturePath(path: string) {
  const normalized = path.replaceAll("\\", "/").toLowerCase();
  return normalized.startsWith(".ssh/") || normalized.includes("/.ssh/");
}

function assetKindForPath(path: string) {
  const extension = path.toLowerCase().split(".").pop() ?? "";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "ico"].includes(extension)) return "image";
  if (["mp4", "mov", "avi", "mkv", "webm"].includes(extension)) return "video";
  if (["wav", "mp3", "flac", "ogg"].includes(extension)) return "media";
  if (["safetensors", "ckpt", "gguf", "onnx", "pt", "pth"].includes(extension)) return "model";
  if (["zip", "7z", "tar", "gz", "parquet", "csv", "bin", "dat"].includes(extension)) return "data";
  return "other";
}

function assetKindMatches(filter: string, candidateKind: string) {
  if (!filter || filter === "all") return true;
  if (filter === "media") return ["image", "video", "media"].includes(candidateKind);
  if (["datasets", "archives", "data"].includes(filter)) return candidateKind === "data";
  if (filter === "models") return candidateKind === "model";
  return filter === candidateKind;
}

function confidenceRank(confidence: string) {
  if (confidence === "High") return 3;
  if (confidence === "Medium") return 2;
  if (confidence === "Low") return 1;
  return 0;
}

function fixturePartialHash(body: string): string {
  let hash = 2166136261;
  for (const character of body.slice(0, 64 * 1024)) {
    hash ^= character.charCodeAt(0);
    hash = Math.imul(hash, 16777619);
  }
  return `fixture-${hash >>> 0}`;
}

function blockedPreview(nodeId: number, projectId: number, path: string, displayName: string, mode: PreviewMode, reason: string, state: FilePreview["state"]): FilePreview {
  return {
    nodeId,
    projectId,
    path,
    displayPath: path,
    displayName,
    mode,
    state,
    fileKind: "unsupported",
    sizeBytes: null,
    truncated: false,
    previewLimitBytes: 2 * 1024 * 1024,
    systemErrorCode: null,
    wasRevealed: false,
    source: null,
    renderedHtml: null,
    blockedReason: reason,
    headings: [],
    links: []
  };
}

function priorityFor(path: string): number {
  const lower = path.toLowerCase();
  if (lower === "readme.md") return -100;
  if (lower === "agents.md") return -95;
  if (lower.includes("/docs/")) return -70;
  if (lower.includes("/prompts/")) return -65;
  if (lower === "package.json") return -55;
  if (lower.endsWith(".md")) return -40;
  return 0;
}

function contextRankFor(path: string, displayName: string): number {
  const lower = path.replaceAll("\\", "/").toLowerCase();
  const name = displayName.toLowerCase();
  const depth = (lower.match(/\//g) ?? []).length;
  if (lower === "readme.md") return 0;
  if (lower === "agents.md") return 1;
  if (lower === "claude.md" || lower === "gemini.md") return 2;
  if (lower.startsWith(".cursor/rules/")) return 4 + depth;
  if (["docs/readme.md", "docs/index.md", "docs/overview.md"].includes(lower)) return 8;
  if (lower.startsWith("docs/")) return 14 + depth;
  if (lower.startsWith("prompts/")) return 20 + depth;
  if (depth === 0 && ["package.json", "pyproject.toml", "cargo.toml", "go.mod", "requirements.txt"].includes(name)) return 24;
  if (name === "readme.md") return 60 + depth;
  if (lower.includes("/docs/")) return 72 + depth;
  if (lower.includes("/prompts/")) return 78 + depth;
  if (["package.json", "pyproject.toml", "cargo.toml", "go.mod", "requirements.txt"].includes(name)) return 86 + depth;
  return 100 + depth;
}

function contextMetadataFor(path: string, displayName: string): { group: string; reason: string } {
  const lower = path.replaceAll("\\", "/").toLowerCase();
  const name = displayName.toLowerCase();
  if (lower === "readme.md") return { group: "Project overview", reason: "Root README usually gives the fastest project overview." };
  if (["agents.md", "claude.md", "gemini.md"].includes(lower) || lower.startsWith(".cursor/rules/")) return { group: "Agent instructions", reason: "Local assistant or editor rules explain how the project should be handled." };
  if (lower.startsWith("docs/")) return { group: "Documentation", reason: "Documentation near the root is usually higher signal than repeated package READMEs." };
  if (lower.startsWith("prompts/")) return { group: "Prompt/workflow context", reason: "Prompt and workflow files describe how local AI work is organized." };
  if (["package.json", "pyproject.toml", "cargo.toml", "go.mod", "requirements.txt"].includes(name)) return { group: "Project manifest", reason: "Manifest files identify runtimes, dependencies and project shape." };
  if (name === "readme.md") return { group: "Nested README", reason: "Nested READMEs can be useful, but repeated package READMEs are lower priority." };
  return { group: "Additional context", reason: "Available as context, but lower priority than root docs and project instructions." };
}

function score(label: string, path: string, projectName: string, projectPath: string, query: string): number {
  const needle = query.trim().toLowerCase();
  if (!needle) return 1;
  const labelLower = label.toLowerCase();
  const pathLower = path.toLowerCase();
  const tokens = needle.split(/\s+/).filter(Boolean);
  if (tokens.length > 1) {
    const fileMatches = (token: string) => labelLower.includes(token) || pathLower.includes(token);
    const context = `${labelLower}\n${pathLower}\n${projectName.toLowerCase()}\n${projectPath.toLowerCase()}`;
    if (!tokens.every((token) => context.includes(token))) return 0;
    const fileTokens = tokens.filter(fileMatches);
    if (fileTokens.length === 0) return 0;
    return Math.max(...fileTokens.map((token) => score(label, path, "", "", token)))
      + (fileTokens.length - 1) * 4
      + (tokens.length - fileTokens.length) * 8;
  }
  if (labelLower === needle) return 100;
  if (labelLower.includes(needle)) return 80;
  if (pathLower.includes(needle)) return 50;
  return fuzzy(pathLower, needle) ? 25 : 0;
}

function fuzzy(value: string, query: string): boolean {
  let index = 0;
  for (const char of value) {
    if (char === query[index]) index += 1;
    if (index === query.length) return true;
  }
  return false;
}

function snippet(body: string, needle: string): string {
  const index = body.toLowerCase().indexOf(needle);
  if (index < 0) return body.slice(0, 140);
  return body.slice(Math.max(0, index - 60), Math.min(body.length, index + needle.length + 80)).replaceAll("\n", " ");
}

function resolveFixturePath(fromPath: string, target: string): string | null {
  const cleanTarget = decodeURIComponent(target.split("#")[0] ?? "").trim();
  if (!cleanTarget || cleanTarget.startsWith("http:") || cleanTarget.startsWith("https:") || cleanTarget.startsWith("//")) return null;
  const parts = cleanTarget.startsWith("/") ? [] : fromPath.split("/").slice(0, -1);
  for (const part of cleanTarget.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
    } else {
      parts.push(part);
    }
  }
  return parts.join("/");
}

function escapeForPre(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}
