// "edit" and "values" are frontend-only views backed by a source preview. The backend preview
// command only understands "rendered" | "source", so api.ts maps both to "source".
export type PreviewMode = "rendered" | "source" | "edit" | "values";
export type PreviewState = "ready" | "blocked" | "missing" | "unsupported";
export type FileKind = "text" | "markdown" | "binary" | "directory" | "symlink" | "cloudPlaceholder" | "unsupported";

export interface EditSnapshotSummary {
  id: number;
  nodeId: number;
  projectId: number;
  path: string;
  origin: "manual" | "value" | "ai_suggestion" | "ai_session" | "restore" | string;
  sessionId?: string | null;
  createdAt: string;
  status: string;
  bytes: number;
  blake3Before: string;
  blake3After?: string | null;
  restoredAt?: string | null;
}

export interface EditSnapshotRestoreResult {
  restoredSnapshotId: number;
  safetySnapshotId: number;
  nodeId: number;
  message: string;
}

export interface FileEditPreview {
  nodeId: number;
  projectId: number;
  beforeHash: string;
  afterHash: string;
  addedLines: number;
  removedLines: number;
  hunks: SessionDiffHunk[];
  diffTruncated: boolean;
  validation: EditValidationSummary;
  gitContext: EditGitContext;
}

export interface EditValidationSummary {
  status: "passed" | "warning" | string;
  label: string;
  note: string;
}

export interface EditGitContext {
  state: "clean" | "modified" | "staged" | "staged_and_modified" | "untracked" | "not_repository" | "unavailable" | string;
  label: string;
  note: string;
  otherChangedFiles: number;
}

export interface EditSnapshotComparison {
  snapshotId: number;
  nodeId: number;
  addedLines: number;
  removedLines: number;
  hunks: SessionDiffHunk[];
  diffTruncated: boolean;
  alreadyCurrent: boolean;
}

export interface EditableValueSet {
  nodeId: number;
  path: string;
  format: "json" | "toml" | string;
  sourceHash: string;
  values: EditableValue[];
}

export interface EditableValue {
  id: string;
  path: string;
  label: string;
  kind: "string" | "number" | "boolean" | "color" | string;
  displayValue: string;
  rawValue: string;
  startByte: number;
  endByte: number;
}

export interface ValueEditRequest {
  valueId: string;
  expectedSourceHash: string;
  expectedRawValue: string;
  newValue: string;
}

export interface ValueEditResult {
  nodeId: number;
  snapshotId: number;
  sourceHash: string;
  value: EditableValue;
  message: string;
}

export interface CorrectionCheckItem {
  id: string;
  label: string;
  status: "passed" | "warning" | "failed" | "not_applicable" | string;
  detail: string;
}

export interface CorrectionStaticCheckReport {
  nodeId: number;
  projectId: number;
  path: string;
  status: "passed" | "warning" | "failed" | string;
  checks: CorrectionCheckItem[];
  checkedAt: string;
  executedProjectCode: false;
}

export interface ProjectCheckDefinition {
  id: string;
  label: string;
  commandLabel: string;
  manifestPath: string;
  fingerprint: string;
  approved: boolean;
  approvedAt?: string | null;
  timeoutSeconds: number;
  memoryLimitMib: number;
  processLimit: number;
  riskDisclosure: string;
}

export interface ControlledCheckRun {
  projectId: number;
  nodeId: number;
  checkId: string;
  label: string;
  commandLabel: string;
  status: "passed" | "failed" | "timed_out" | string;
  exitCode?: number | null;
  durationMs: number;
  stdout: string;
  stderr: string;
  outputTruncated: boolean;
  rollbackSnapshotId?: number | null;
  rollbackAvailable: boolean;
  checkedAt: string;
  limitsSummary: string;
}

export type ProjectScanState = "scanned" | "scanning" | "outdated";

export interface StartupStatus {
  state: "starting" | "ready" | "failed";
  message: string;
  elapsedMs: number;
  dbOpenMs?: number | null;
}

export type ShellOpenMode = "known" | "viewer" | "automatic" | "manual";

export interface OpenTargetInspection {
  inputPath: string;
  targetKind: "file" | "folder";
  knownProjectRoot?: string | null;
  automaticProjectRoot: string;
  viewerRoot: string;
}

export interface DirectFilePreview {
  inputPath: string;
  viewerRoot: string;
  preview: FilePreview;
}

export interface OpenTargetPreparation {
  inputPath: string;
  targetKind: "file" | "folder";
  projectRoot: string;
  projectId: number;
  rootId: number;
  nodeId?: number | null;
  scanJobId?: string | null;
  scanAlreadyRunning: boolean;
  openMode: ShellOpenMode;
  temporary: boolean;
}

export interface OpenTargetScanStart {
  jobId: string;
  startedHere: boolean;
}

export interface ShellIntegrationStatus {
  available: boolean;
  markdownRegistered: boolean;
  contextMenuRegistered: boolean;
  runAtLogin: boolean;
  runAtLoginOwnedByCurrentEdition: boolean;
  runAtLoginOwnerExecutable?: string | null;
  backgroundRefreshEnabled: boolean;
  closeToTray: boolean;
  ownedByCurrentEdition: boolean;
  defaultGuidePending: boolean;
  ownerExecutable?: string | null;
}

export interface SystemResourceProfile {
  logicalCpuCount: number;
  totalMemoryBytes?: number | null;
  availableMemoryBytes?: number | null;
  gpuAcceleration: string;
  dedicatedVramBytes?: number | null;
  plans: PerformanceModePlan[];
}

export interface PerformanceModePlan {
  mode: "balanced" | "priority" | "max" | string;
  label: string;
  cpuThreads: number;
  processPriority: string;
  scanBatchSize: number;
  memoryBudgetBytes?: number | null;
  notes: string[];
}

export interface ProcessResourceUsage {
  cpuPercent: number;
  logicalCpuCount: number;
  memoryWorkingSetBytes?: number | null;
  memoryPrivateBytes?: number | null;
  totalMemoryBytes?: number | null;
  availableMemoryBytes?: number | null;
  gpuSummary: string;
  gpuUsagePercent?: number | null;
  sampled: boolean;
}

export interface ProjectSummary {
  id: number;
  name: string;
  path: string;
  source: string;
  contextCount: number;
  pinned: boolean;
  protectedLevel?: string | null;
  scanState: ProjectScanState;
  scanRootId?: number | null;
  /** Name given in the Antigravity (Gemini) IDE when it differs from the folder basename. */
  antigravityName?: string | null;
  /** True when the owning AI app still lists this project as active (open/recent), not archived. */
  isCurrent?: boolean;
  /** Owning AI app slug for the badge: "codex" | "claude" | "antigravity" | "cursor" | "hermes" | … */
  app?: string | null;
  /** Every app this project is used in (primary `app` plus others) — drives the app filter. */
  apps?: string[];
}

export interface ProjectDetail extends ProjectSummary {}

/** One reversible AI-app de-registration: the app's registry file was backed up then
 * deleted; restoring copies the backup back. Returned by `removeProjectFromApps`. */
export interface AppRemovalRecord {
  app: string;
  originalPath: string;
  backupPath: string;
}

/** A persisted "remove from AI apps", recoverable from Recover even after a restart. */
export interface PersistedAppRemoval {
  id: string;
  projectName: string;
  removedAtUnix: number;
  records: AppRemovalRecord[];
}

/** A local, no-network "what this project does" card from its README + manifests. */
export interface ProjectContextSummary {
  kinds: string[];
  readmeTitle: string | null;
  readmeExcerpt: string | null;
  runCommands: string[];
  manifestFiles: string[];
  markdownFiles: string[];
}

export interface NavItem {
  id: number;
  projectId: number;
  nodeId?: number | null;
  parentNavId?: number | null;
  path: string;
  displayPath: string;
  displayName: string;
  itemKind: "directory" | "file" | string;
  priority: number;
  isContext: boolean;
  isMarkdown: boolean;
  isSensitive: boolean;
  protectedLevel?: string | null;
  childCount: number;
  fullyScanned: boolean;
  collapseDefault: boolean;
  scanError?: string | null;
  aggregateApparentBytes?: number | null;
  aggregateAllocatedBytes?: number | null;
  aggregatePhysicalBytes?: number | null;
  aggregateBytesPartial: boolean;
  cloudOnly: boolean;
  /** Last-modified time as a Unix-epoch-seconds string (from the scanned node); used for "sort by date". */
  modifiedAt?: string | null;
  children: NavItem[];
}

export interface NavChildrenPage {
  items: NavItem[];
  total: number;
  hasMore: boolean;
}

export interface FolderExplanation {
  navId: number;
  projectId: number;
  displayPath: string;
  displayName: string;
  itemKind: string;
  classification: string;
  confidence: string;
  summary: string;
  signals: string[];
  caveats: string[];
  childCount: number;
  apparentBytes?: number | null;
  allocatedBytes?: number | null;
  physicalBytes?: number | null;
  footprintPartial: boolean;
  protectedLevel?: string | null;
  fullyScanned: boolean;
  scanError?: string | null;
}

export interface InvestigationHandle {
  rootId: number;
  jobId: string;
  path: string;
}

export interface InvestigationOwner {
  relation: string;
  name: string;
  path: string;
}

export interface FolderInvestigation {
  rootId: number;
  rootNodeId?: number | null;
  path: string;
  explanation?: FolderExplanation | null;
  owners: InvestigationOwner[];
  isOrphan: boolean;
  fileCount: number;
  totalBytes: number;
  hasGit: boolean;
}

export interface ContextFile {
  navId: number;
  nodeId: number;
  projectId: number;
  path: string;
  displayName: string;
  priority: number;
  contextRank: number;
  contextGroup: string;
  recommendationReason: string;
  recommended: boolean;
  isSensitive: boolean;
  protectedLevel?: string | null;
}

export interface GitRepoSummary {
  projectId: number;
  hasGit: boolean;
  currentBranch?: string | null;
  headRef?: string | null;
  originUrl?: string | null;
  metadataError?: string | null;
}

export interface ProjectFootprintSummary {
  projectId: number;
  name: string;
  path: string;
  apparentBytes: number;
  allocatedBytes?: number | null;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}

export interface DashboardSummary {
  totalProjects: number;
  totalItems: number;
  contextFiles: number;
  indexedDocuments: number;
  nonIndexedItems: number;
  partialItems: number;
  gitProjects: number;
  sensitiveFiles: number;
  protectedFiles: number;
  scanRoots: number;
  largestProjects: ProjectFootprintSummary[];
  staleOrDirty: string;
  adaptersNeedingReview: number;
}

export interface AdapterSummary {
  id: number;
  name: string;
  version: string;
  adapterType: string;
  source: string;
  enabled: boolean;
  description: string;
}

export interface MarkdownLink {
  label: string;
  target: string;
  isRemote: boolean;
}

export interface NodeRelationship {
  nodeId: number;
  projectId: number;
  path: string;
  displayName: string;
  itemKind: string;
  kind: string;
  confidence: string;
  evidence?: string | null;
}

export interface RelationshipIssue {
  nodeId: number;
  projectId: number;
  kind: string;
  confidence: string;
  target: string;
  evidence?: string | null;
}

export interface NodeRelationships {
  nodeId: number;
  projectId: number;
  outgoing: NodeRelationship[];
  incoming: NodeRelationship[];
  issues: RelationshipIssue[];
}

export interface GraphNode {
  nodeId: number;
  projectId: number;
  path: string;
  displayName: string;
  itemKind: string;
  graphKind: string;
  confidence: string;
  details: string[];
  physicalBytes?: number | null;
  protectedOrSensitive: boolean;
  sharedProjectIds: number[];
}

export interface GraphEdge {
  sourceNodeId: number;
  sourceProjectId?: number | null;
  targetNodeId: number;
  kind: string;
  confidence: string;
  evidence?: string | null;
}

export interface GraphIssue {
  nodeId: number;
  projectId?: number | null;
  sourcePath?: string | null;
  kind: string;
  confidence: string;
  target: string;
  evidence?: string | null;
}

export interface GraphMap {
  projectId: number;
  nodes: GraphNode[];
  edges: GraphEdge[];
  issues: GraphIssue[];
  totalNodes: number;
  totalEdges: number;
  totalIssues: number;
  partial: boolean;
}

export type GraphMapExpansionStatus = "idle" | "loading" | "pausing" | "paused" | "complete" | "error";

export interface GraphMapExpansionState {
  status: GraphMapExpansionStatus;
  loadedItems: number;
  totalItems: number;
  message?: string | null;
}

export interface OrphanCandidate {
  nodeId: number;
  projectId: number;
  projectName: string;
  path: string;
  displayName: string;
  confidence: string;
  reason: string;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}

export interface OrphanCandidates {
  candidates: OrphanCandidate[];
  total: number;
}

export interface OrphanStatus {
  nodeId: number;
  projectId: number;
  evaluated: boolean;
  isCandidate: boolean;
  candidateKind?: string | null;
  confidence?: string | null;
  reason?: string | null;
  incomingReferences: number;
  protectedOrSensitive: boolean;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}

export interface LostProjectCandidate {
  projectId: number;
  nodeId?: number | null;
  navId?: number | null;
  candidateKind: string;
  displayName: string;
  path: string;
  confidence: string;
  reason: string;
  signals: string[];
  apparentBytes: number;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}

export interface LostProjectCandidates {
  candidates: LostProjectCandidate[];
  total: number;
}

export interface DuplicateMember {
  nodeId: number;
  projectId: number;
  projectName: string;
  path: string;
  displayName: string;
  physicalBytes?: number | null;
  footprintPartial: boolean;
}

export interface DuplicateGroup {
  id: number;
  sizeBytes: number;
  hashPartial: string;
  confidence: string;
  reason: string;
  memberCount: number;
  physicalBytes?: number | null;
  footprintPartial: boolean;
  members: DuplicateMember[];
}

export interface DuplicateCandidates {
  groups: DuplicateGroup[];
  total: number;
}

export interface ConfirmedDuplicateGroup {
  fullHash: string;
  sizeBytes: number;
  memberCount: number;
  reclaimableBytes: number;
  confidence: string;
  members: DuplicateMember[];
}

export interface DuplicateConfirmation {
  targetNodeId: number;
  confirmedGroups: ConfirmedDuplicateGroup[];
  checkedFiles: number;
  bytesHashed: number;
  reclaimableBytes: number;
  partial: boolean;
}

/** Live progress of an on-demand full-hash duplicate confirmation. */
export interface DuplicateConfirmProgress {
  checkedFiles: number;
  totalFiles: number;
  bytesHashed: number;
  totalBytes: number;
}

/** Status of an on-demand duplicate-confirmation job (full-hash verification runs off-thread). */
export interface DuplicateConfirmStatus {
  jobId: string;
  /** running | cancelling | completed | cancelled | failed */
  state: string;
  targetNodeId: number;
  message: string;
  error: string | null;
  progress: DuplicateConfirmProgress;
  result: DuplicateConfirmation | null;
}

export interface DiscoverySignal {
  kind: string;
  label: string;
  detail?: string | null;
  confidence: string;
}

export interface DiscoverySourceHit {
  sourceKind: string;
  sourceLabel: string;
  path: string;
  exists: boolean;
  detail?: string | null;
}

export interface InstalledApp {
  id: string;
  label: string;
  present: boolean;
}

export interface ProjectDiscoveryCandidate {
  path: string;
  displayName: string;
  projectKind: string;
  confidence: string;
  score: number;
  sourceKinds: string[];
  signals: DiscoverySignal[];
  alreadyRegistered: boolean;
  existingProjectId?: number | null;
  overlapKind: string;
  nestedUnderRegistered?: string | null;
  containsRegisteredRoots: string[];
  estimatedFiles?: number | null;
  estimatedBytes?: number | null;
  estimatePartial: boolean;
}

export interface SessionDiscoveryCandidate {
  path: string;
  displayName: string;
  sourceKind: string;
  sourceLabel: string;
  sessionKind: string;
  confidence: string;
  linkedProjectPaths: string[];
  linkedRegisteredProjectIds: number[];
  association: "registered_project" | "unregistered_project_reference" | "loose_session" | string;
  /** Source-file last-modified time (epoch ms) for recency sorting; null when stat failed. */
  modifiedMs?: number | null;
}

export interface SessionPreview {
  path: string;
  displayName: string;
  sessionKind: string;
  sizeBytes: number;
  /** Cumulative source/transcript byte window requested for this response. */
  previewLimitBytes: number;
  truncated: boolean;
  /** Raw Source remains bounded even when the readable conversation is complete. */
  sourceTruncated: boolean;
  text: string;
  /** Conversation-only window for Rendered; Source always uses the raw text. */
  renderedText?: string | null;
  redactedCount: number;
  revealed: boolean;
  /** File created / last-modified time (epoch ms), when the filesystem reports it. */
  createdMs?: number | null;
  modifiedMs?: number | null;
}


export type RecapAiSourceMode = "combined" | "git" | "session";

export interface SessionChangeSet {
  path: string;
  sourceKind: string;
  coverage: SessionChangeCoverage;
  files: SessionFileChange[];
  editCount: number;
  addedLines: number;
  removedLines: number;
  redactedCount: number;
  parsedRecords: number;
  omittedRecords: number;
}

export interface SessionChangeCoverage {
  level: "full" | "direct_edits" | "none" | string;
  label: string;
  note: string;
}

export interface SessionFileChange {
  path: string;
  edits: SessionChangeEdit[];
  addedLines: number;
  removedLines: number;
  reality?: SessionFileReality | null;
}

export interface SessionFileReality {
  status: "applied" | "reverted" | "drifted" | "file_missing" | "unverified" | string;
  label: string;
  note: string;
  observedMs?: number | null;
}

export interface ProjectReviewCheckpoint {
  projectId: number;
  reviewedAt: string;
  sessionCutoffMs: number;
  /** Content-free identity for the set of undated session records acknowledged at this checkpoint. */
  undatedSessionFingerprint?: string | null;
  gitFingerprint?: string | null;
  gitHead?: string | null;
}

export interface ReviewLedgerEntry {
  id: number;
  projectId: number;
  nodeId?: number | null;
  sourceKind: string;
  sourceRef: string;
  sourceModifiedMs?: number | null;
  observedAt: string;
  origin?: string | null;
  sessionId?: string | null;
  beforeHash?: string | null;
  afterHash?: string | null;
  previousEntryHash?: string | null;
  entryHash: string;
  encodedBytes: number;
  changeSet: SessionChangeSet;
}

export interface SessionChangeEdit {
  source: string;
  summary: string;
  provenance?: string | null;
  confidence?: "observed" | "retained" | "inferred" | string | null;
  reality?: SessionFileReality | null;
  request?: string | null;
  hunks: SessionDiffHunk[];
  addedLines: number;
  removedLines: number;
}

export interface SessionDiffHunk {
  header: string;
  oldStart?: number | null;
  newStart?: number | null;
  lines: SessionDiffLine[];
}

export interface SessionDiffLine {
  kind: "context" | "added" | "removed" | "note" | string;
  content: string;
  oldLine?: number | null;
  newLine?: number | null;
}

export interface ProjectDiscoveryReport {
  candidates: ProjectDiscoveryCandidate[];
  sessions: SessionDiscoveryCandidate[];
  searchedLocations: DiscoverySourceHit[];
  durationMs: number;
  totalCandidates: number;
  totalSessions: number;
}

export interface FilePreview {
  nodeId: number;
  projectId: number;
  path: string;
  displayPath: string;
  displayName: string;
  mode: PreviewMode;
  state: PreviewState;
  fileKind: FileKind;
  sizeBytes?: number | null;
  truncated: boolean;
  previewLimitBytes: number;
  systemErrorCode?: number | null;
  wasRevealed: boolean;
  source?: string | null;
  renderedHtml?: string | null;
  blockedReason?: string | null;
  headings: string[];
  links: MarkdownLink[];
}

export interface QuickOpenResult {
  nodeId: number;
  projectId: number;
  label: string;
  path: string;
  itemKind: string;
  score: number;
}

export interface DocumentHit {
  nodeId: number;
  projectId: number;
  title: string;
  path: string;
  snippet: string;
}

export interface DocumentSearchResult {
  hits: DocumentHit[];
  truncated: boolean;
  durationMs: number;
}

export interface PreviewPolicy {
  allowSensitiveReveal: boolean;
  relaxNonStrongProtectedPreview: boolean;
}

export interface RecentItem {
  nodeId: number;
  projectId?: number | null;
  itemKind: string;
  path: string;
  openedAt: string;
}

export interface PinnedItem {
  nodeId: number;
  projectId?: number | null;
  itemKind: string;
  path: string;
  pinnedAt: string;
}

export interface Comment {
  id: number;
  nodeId: number;
  projectId?: number | null;
  body: string;
  author: string;
  source: string;
  createdAt: string;
  updatedAt: string;
}

export interface ScanRoot {
  id: number;
  path: string;
  enabled: boolean;
  lastScannedAt?: string | null;
}

export interface WatcherStatus {
  generatedAtMs: number;
  pollIntervalMs: number;
  debounceMs: number;
  staleProjects: number;
  changedProjects: number;
  projects: WatcherProjectStatus[];
  focused?: FocusedWatcherStatus | null;
  message: string;
}

export interface WatcherProjectStatus {
  projectId?: number | null;
  scanRootId: number;
  name: string;
  path: string;
  state: "clean" | "stale" | "missing" | "needs_scan" | "empty" | "disabled" | string;
  reason: string;
  lastScannedAt?: string | null;
  rootModifiedAt?: number | null;
}

export interface FocusedWatcherStatus {
  projectId: number;
  state: "clean" | "dirty" | string;
  changedContextFiles: number;
  currentNode?: WatcherNodeStatus | null;
  message: string;
}

export interface WatcherNodeStatus {
  nodeId: number;
  path: string;
  displayName: string;
  state: "clean" | "changed" | "missing" | "untracked" | string;
  isMarkdown: boolean;
  isContext: boolean;
  storedMtime?: string | null;
  liveMtime?: string | null;
  storedSize?: number | null;
  liveSize?: number | null;
}

export interface FileChangeEvent {
  id: number;
  projectId: number;
  projectName: string;
  projectPath: string;
  nodeId?: number | null;
  path: string;
  displayName: string;
  changeKind: "created" | "modified" | "removed" | string;
  observedAt: string;
  modifiedAt?: string | null;
  sizeBytes?: number | null;
  apps: string[];
}

export interface DbMaintenanceReport {
  beforeBytes: number;
  afterBytes: number;
  freedBytes: number;
}

export interface ScanStatus {
  jobId: string;
  state: string;
  scanPhase: string;
  scannedFiles: number;
  indexedDocuments: number;
  startedAtMs: number;
  phaseStartedAtMs?: number | null;
  lastProgressAtMs?: number | null;
  updatedAtMs: number;
  estimatedTotalFiles?: number | null;
  estimatedTotalBytes?: number | null;
  workerCount?: number | null;
  estimateMs?: number | null;
  scanMs?: number | null;
  bodyReadMs?: number | null;
  persistMs?: number | null;
  finalizeMs?: number | null;
  accountingSelectMs?: number | null;
  accountingComputeMs?: number | null;
  accountingUpdateMs?: number | null;
  partial: boolean;
  rootIds: number[];
  rootPaths: string[];
  currentPath?: string | null;
  error?: string | null;
  message: string;
}

export interface ProtectedZone {
  id: number;
  patternType: string;
  pattern: string;
  level: string;
  source: string;
}

export interface SecurityStatus {
  mutationExecutor: string;
  activeFeatures: string[];
  notes: string[];
}

export interface RecoveryOperation {
  id: number;
  kind: string;
  status: string;
  targetNodeId?: number | null;
  targetFingerprint?: string | null;
  createdAt: string;
  startedAt?: string | null;
  error?: string | null;
  totalItems: number;
  doneItems: number;
  pendingItems: number;
  failedItems: number;
}

export interface RecoveryPending {
  enabled: boolean;
  pending: boolean;
  operations: RecoveryOperation[];
  message: string;
}

export interface RecoveryResolveResult {
  action: string;
  recoveredOperations: number;
  rolledBackItems: number;
  message: string;
}

export interface MutationTokenResult {
  action: string;
  token: string;
}

export interface MutationBackupSummary {
  backupId: number;
  manifestPath: string;
  totalBytes: number;
  verified: boolean;
  itemCount: number;
}

export interface MutationMoveEntry {
  originalPath: string;
  storedPath?: string | null;
  outcome: string;
  bytes: number;
  spaceRecovered: number;
  detail?: string | null;
}

export interface MutationMoveSummary {
  operationId: number;
  entries: MutationMoveEntry[];
  spaceRecovered: number;
  moved: number;
  skipped: number;
  failed: number;
  removedDirs: number;
  removedLinks: number;
}

export interface MutationProtectedPreview {
  protected: string[];
  reparse: string[];
}

export interface MutationRestoreSummary {
  entryId: number;
  outcome: string;
  originalPath: string;
  restoredPath?: string | null;
  conflictPath?: string | null;
}

export interface MutationFinalRemoveSummary {
  entryId: number;
  freedBytes: number;
}

// Project/batch final-cleanup contract. The single-entry summary above remains for
// compatibility with older mutation backends, but new UI must use this preview-bound
// contract so unsupported objects can stay held without disabling cleanup globally.
export type FinalRemoveScope =
  | { kind: "project"; groupId: string }
  | { kind: "groups"; groupIds: string[] }
  | { kind: "allEligible" };

export type FinalRemoveEligibility = "ready" | "needsArchiveV2" | "blocked";

export type FinalRemoveReasonCode =
  | "archiveVerified"
  | "legacyContentOnly"
  | "archiveMissing"
  | "archiveCorrupt"
  | "unsupportedReparse"
  | "unsupportedEfs"
  | "unsupportedObjectStream"
  | "externalHardlink"
  | "nonNtfs"
  | "cloudOrRecall"
  | "locked"
  | "identityChanged"
  | "insufficientSpace"
  | "permissionDenied"
  | "helperUnsigned"
  | "helperUntrusted"
  | "releaseManifestMismatch"
  | "uacCancelled"
  | "capacityBlocked"
  | "scratchCleanupPending"
  | "stopRequested"
  | "interrupted";

export type FinalRemoveObjectArchiveState =
  | "none"
  | "contentOnlyLegacy"
  | "pending"
  | "objectCompleteVerified"
  | "invalid";

export interface FinalRemoveObjectDecision {
  entryId: number;
  groupId: string;
  topologyGroupId: string;
  relativePath: string;
  kind: "file" | "directory";
  lifecycle: "held" | "deleting" | "deleted" | "restored";
  eligibility: FinalRemoveEligibility;
  reasonCode: FinalRemoveReasonCode;
  reason: string;
  remediation?: string;
  archiveId?: string | null;
  objectArchiveState: FinalRemoveObjectArchiveState;
  heldVolumeId: string;
  heldVolumeLabel: string;
  logicalBytes: number;
  allocatedBytes?: number | null;
  measurement: "exactStreams" | "logicalUpperBound" | "unknown";
}

export interface FinalRemoveVolumeImpact {
  volumeId: string;
  label: string;
  alreadyFreedFromSourceBytes: number;
  heldAllocatedBytes: number;
  projectedReleaseBytes: number;
  archiveRetainedAllocatedBytes: number;
  freeBytesBefore?: number | null;
  freeBytesAfter?: number | null;
  observedDeltaBytes?: number | null;
  quality: "exactObjectAllocation" | "estimated" | "observedNoisy";
}

export interface FinalRemoveBlockedSubtree {
  root: string;
  count: number;
  reasonCodes: FinalRemoveReasonCode[];
}

export interface FinalRemoveProjectPreview {
  groupId: string;
  projectName: string;
  originalRoot: string;
  totalObjects: number;
  ready: number;
  needsArchiveV2: number;
  blocked: number;
  blockedSubtrees: FinalRemoveBlockedSubtree[];
}

export interface FinalRemoveBatchPreview {
  previewId: string;
  previewDigest: string;
  expiresAt: string;
  projects: FinalRemoveProjectPreview[];
  objects: FinalRemoveObjectDecision[];
  volumes: FinalRemoveVolumeImpact[];
  eligibleTopologyGroupIds: string[];
  requiresElevation: boolean;
  maxDeleteObjects: number;
  blockedObjects: number;
  archivesRetained: true;
}

export interface FinalRemoveConfirmationToken {
  token: string;
  previewId: string;
  previewDigest: string;
  expiresAt: string;
}

export interface FinalRemoveBatchStart {
  previewId: string;
  previewDigest: string;
  selectedTopologyGroupIds: string[];
  confirmationToken: string;
}

export interface FinalRemoveBatchStartSummary {
  jobId: string;
  batchId: string;
}

export type FinalRemoveItemState =
  | "planned"
  | "archiveFinalizing"
  | "archiveVerified"
  | "blocked"
  | "deleteIntent"
  | "deleted"
  | "deleteFailed"
  | "kept"
  | "reconciledDeleted";

export interface FinalRemoveBatchProgress {
  batchId: string;
  phase:
    | "waitingForUac"
    | "verifyingArchives"
    | "roundtrip"
    | "parentDisposition"
    | "deleting"
    | "cleaningDirs"
    | "stoppingAfterCurrentTopologyGroup"
    | "finished"
    | "interrupted";
  total: number;
  completed: number;
  currentPath?: string | null;
}

export interface FinalRemoveProjectResult {
  groupId: string;
  deleted: number;
  kept: number;
  failed: number;
}

export interface FinalRemoveItemResult {
  entryId: number;
  state: FinalRemoveItemState;
  reasonCode?: FinalRemoveReasonCode;
  error?: string;
}

export interface FinalRemoveBatchResult {
  batchId: string;
  status: "completed" | "partial" | "cancelled" | "failed" | "interrupted";
  requestedObjects: number;
  deletedObjects: number;
  keptObjects: number;
  failedObjects: number;
  projects: FinalRemoveProjectResult[];
  volumes: FinalRemoveVolumeImpact[];
  items: FinalRemoveItemResult[];
  archiveRetained: true;
}

export interface FinalRemoveBatchStatus {
  progress: FinalRemoveBatchProgress;
  result?: FinalRemoveBatchResult | null;
}

export interface MutationRecoveryDashboard {
  available: boolean;
  finalRemove: {
    state: "idle" | "active" | "interrupted" | "unknown";
    batchId?: string | null;
    jobId?: string | null;
    phase?: FinalRemoveBatchProgress["phase"] | null;
    message: string;
  };
  message: string;
}

export interface MutationLockInspection {
  path: string;
  state: string;
}

export interface MutationActivityOperation {
  id: number;
  kind: string;
  status: string;
  targetNodeId?: number | null;
  targetFingerprint?: string | null;
  recoveredBytes?: number | null;
  createdAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  error?: string | null;
}

export interface MutationActivityItem {
  id: number;
  operationId: number;
  nodeId?: number | null;
  action: string;
  fromPath?: string | null;
  toPath?: string | null;
  bytes?: number | null;
  status: string;
}

export interface MutationActivityBackup {
  id: number;
  level: string;
  destination: string;
  manifestPath: string;
  totalBytes?: number | null;
  verified: boolean;
  createdAt: string;
}

export interface MutationStoredEntry {
  id: number;
  operationId?: number | null;
  originalPath: string;
  storedPath: string;
  size?: number | null;
  fileCount?: number | null;
  riskLevel?: string | null;
  backupId?: number | null;
  spaceRecovered: number;
  scheduledDeleteAt?: string | null;
  status: string;
}

export interface MutationActivityLog {
  enabled: boolean;
  operations: MutationActivityOperation[];
  items: MutationActivityItem[];
  backups: MutationActivityBackup[];
  storedEntries: MutationStoredEntry[];
  message: string;
}

export type RiskTier = "green" | "yellow" | "orange" | "red" | "black";

export interface RecoverableBytes {
  owned: number;
  orphanedOnRemoval: number;
  total: number;
  partial: boolean;
}

export interface RecoverableSummary {
  targetNodeId: number;
  projectId: number;
  targetPath: string;
  targetKind: string;
  recoverableBytes: RecoverableBytes;
  sharedCount: number;
  protectedCount: number;
  sensitiveCount: number;
  partialFootprint: boolean;
}

export interface OperationPlanTarget {
  nodeId: number;
  projectId: number;
  kind: string;
  path: string;
  displayName: string;
}

export interface OperationPlanItem {
  nodeId?: number | null;
  path: string;
  displayName: string;
  itemKind: string;
  actionLabel: string;
  risk: RiskTier;
  confidence: string;
  sizeApparent: number;
  physicalBytes?: number | null;
  hardlinkGroup?: string | null;
  freesSpace: boolean;
  recursiveDir: boolean;
  childCount: number;
  partial: boolean;
}

export interface SharedAssetRef {
  projectId: number;
  projectName: string;
}

export interface SharedAsset {
  nodeId: number;
  path: string;
  displayName: string;
  physicalBytes?: number | null;
  referencedBy: SharedAssetRef[];
  confidence: string;
}

export interface DanglingAfter {
  referrerNodeId: number;
  path: string;
  missingPath: string;
  confidence: string;
  /** The project the referrer lives in (null when unresolved). */
  projectId: number | null;
  /** Human-readable name of that project, for badging. */
  projectName: string | null;
  /** "workflow" for a workflow→model reference, or the relationship-issue kind for a local break. */
  dependencyKind: string;
  /** True when the referrer is in a DIFFERENT project than the one being removed (higher risk). */
  crossProject: boolean;
}

export interface SensitiveFileRef {
  nodeId?: number | null;
  path: string;
  signature: string;
}

export interface ProtectedHit {
  nodeId?: number | null;
  path: string;
  level: string;
}

export interface GitWarning {
  projectId: number;
  message: string;
  confidence: string;
}

export interface ConfidenceSummary {
  high: number;
  medium: number;
  low: number;
  unknown: number;
}

export interface OperationPlan {
  planId: string;
  schema: "operation_plan/1";
  createdAt: string;
  target: OperationPlanTarget;
  actionLabel: string;
  items: OperationPlanItem[];
  recoverableBytes: RecoverableBytes;
  sharedAssets: SharedAsset[];
  danglingAfter: DanglingAfter[];
  /** True when the dangling-impact scan hit its per-query row cap (more may exist). */
  danglingTruncated: boolean;
  sensitiveFiles: SensitiveFileRef[];
  protectedHits: ProtectedHit[];
  gitWarnings: GitWarning[];
  confidenceSummary: ConfidenceSummary;
  recommendedAction: string;
  readOnlyPreview: boolean;
  planStale: boolean;
  partialFootprint: boolean;
  /** False means empty dependent/shared lists are not proof that no links exist. */
  relationshipEvidenceComplete: boolean;
  externalServicesUnaffected: boolean;
  targetFingerprint: string;
}

export interface RiskTierCount {
  tier: RiskTier;
  count: number;
  physicalBytes?: number | null;
}

export interface RiskReport {
  schema: "risk_report/1";
  generatedAt: string;
  target: OperationPlanTarget;
  actionLabel: string;
  readOnlyPreview: boolean;
  externalServicesUnaffected: boolean;
  recoverableBytes: RecoverableBytes;
  riskCounts: RiskTierCount[];
  sharedAssets: SharedAsset[];
  danglingAfter: DanglingAfter[];
  relationshipEvidenceComplete: boolean;
  /** True when the dangling-impact scan hit its per-query row cap (more may exist). */
  danglingTruncated: boolean;
  sensitiveFiles: SensitiveFileRef[];
  protectedHits: ProtectedHit[];
  gitWarnings: GitWarning[];
  confidenceSummary: ConfidenceSummary;
  recommendedAction: string;
  caveats: string[];
}

export interface PlanPreviewStatus {
  jobId: string;
  state: string;
  targetNodeId: number;
  actionLabel: string;
  message: string;
  error?: string | null;
  plan?: OperationPlan | null;
  report?: RiskReport | null;
}

export interface ExportResult {
  path: string;
  bytesWritten: number;
}

export type SafeManageLifecycle =
  | "active"
  | "dormant"
  | "archive_candidate"
  | "cleanup_candidate"
  | "needs_review";

export type SafeManageRecommendation =
  | "keep"
  | "review"
  | "archive"
  | "clean_regenerables"
  | "removal_candidate"
  | "do_not_touch";

export type SafeManageConfidence = "high" | "medium" | "low" | "unknown";
export type SafeManageSignalState = "present" | "absent" | "unknown";
export type SafeManageEvidenceCoverage = "complete" | "partial" | "unavailable";

export type SafeManageDecisionKind =
  | "keep"
  | "ignore"
  | "request_deeper_review"
  | "archive"
  | "clean_regenerables"
  | "prepare_removal";

export interface SafeManageDecisionRequest {
  projectId: number;
  analysisRunId: string;
  decision: SafeManageDecisionKind;
  evidenceRevision: string;
}

export type SafeManageRegenerableEvidenceState =
  | "opaque_measured"
  | "opaque_partial"
  | "expanded_complete"
  | "expanded_partial";

export interface SafeManageRegenerableTarget {
  projectId: number;
  analysisRunId: string;
  evidenceRevision: string;
  navId: number;
  nodeId: number;
  path: string;
  kind: string;
  bytes?: number | null;
  evidenceState: SafeManageRegenerableEvidenceState;
  operationPlanEligible: boolean;
  scanError?: string | null;
}

export interface SafeManageRegenerableScanRequest {
  projectId: number;
  analysisRunId: string;
  evidenceRevision: string;
  navId: number;
  nodeId: number;
  path: string;
}

export interface SafeManageOperationTargetIdentity {
  navId: number;
  nodeId: number;
  path: string;
}

export interface SafeManageOperationPlanRequest {
  projectId: number;
  analysisRunId: string;
  evidenceRevision: string;
  decision: Extract<SafeManageDecisionKind, "archive" | "clean_regenerables" | "prepare_removal">;
  target?: SafeManageOperationTargetIdentity | null;
}

export interface SafeManageSignal {
  code: string;
  label: string;
  state: SafeManageSignalState;
  detail: string;
  source: string;
  observedAt?: string | null;
}

export interface SafeManageImportantFile {
  nodeId?: number | null;
  path: string;
  displayName: string;
  reason: string;
  protectedOrSensitive: boolean;
}

export interface SafeManageRiskRelation {
  kind: string;
  label: string;
  confidence: SafeManageConfidence;
  relatedProjectIds: number[];
}

export interface SafeManageFileKindCount {
  kind: string;
  label: string;
  fileCount: number;
}

export interface SafeManageFileKindProfile {
  coverage: SafeManageEvidenceCoverage;
  inspectedFileCount: number;
  counts: SafeManageFileKindCount[];
}

export interface SafeManageDuplicateEvidence {
  coverage: SafeManageEvidenceCoverage;
  inspectedFileCount: number;
  possibleCopyFileCount: number;
  indexedTextFileCount: number;
  confirmedIndexedTextCopyCount: number;
}

export interface SafeManageProjectAssessment {
  analysisRunId: string;
  projectId: number;
  projectName: string;
  projectPath: string;
  lifecycle: SafeManageLifecycle;
  recommendation: SafeManageRecommendation;
  confidence: SafeManageConfidence;
  reasonCode: string;
  reason: string;
  rulesetVersion: string;
  evidenceRevision: string;
  evidenceStale: boolean;
  lastActivityMs?: number | null;
  apps: string[];
  sessionCount?: number | null;
  hasGit: boolean;
  gitHasRemote?: boolean | null;
  gitUncommitted?: boolean | null;
  apparentBytes?: number | null;
  physicalBytes?: number | null;
  footprintPartial: boolean;
  fileKindProfile: SafeManageFileKindProfile;
  duplicateEvidence: SafeManageDuplicateEvidence;
  materiallySimilarProjectCount: number | null;
  signals: SafeManageSignal[];
  importantFiles: SafeManageImportantFile[];
  riskRelations: SafeManageRiskRelation[];
}

export interface SafeManagePortfolioCounts {
  total: number;
  active: number;
  dormant: number;
  archiveCandidates: number;
  cleanupCandidates: number;
  needsReview: number;
}

export interface SafeManageAnalysisRun {
  id: string;
  state: "queued" | "running" | "cancelling" | "completed" | "partial" | "cancelled" | "failed";
  rulesetVersion: string;
  catalogRevision: string;
  createdAt: string;
  startedAt?: string | null;
  completedAt?: string | null;
  processedProjects: number;
  totalProjects: number;
  counts: SafeManagePortfolioCounts;
  message: string;
  error?: string | null;
  assessments: SafeManageProjectAssessment[];
}

export interface SafeManageDecision {
  id: number;
  projectId: number;
  analysisRunId: string;
  decision: SafeManageDecisionKind;
  evidenceRevision: string;
  decidedBy: "local_user";
  decidedAt: string;
  evidenceStale: boolean;
}

export interface SafeManageFirstRunPreference {
  suggestAfterDiscovery: boolean;
  promptState: "pending" | "postponed" | "completed" | "suppressed";
  lastPromptedAt?: string | null;
}

export interface SafeManageOverview {
  latestRun?: SafeManageAnalysisRun | null;
  lastCompleteRun?: SafeManageAnalysisRun | null;
  decisions: SafeManageDecision[];
  firstRun: SafeManageFirstRunPreference;
}
