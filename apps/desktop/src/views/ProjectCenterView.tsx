import type { CSSProperties, MouseEvent, MutableRefObject, PointerEvent as ReactPointerEvent } from "react";
import { AlertTriangle, Eye, FileDiff, FileText, Files, Folder, HardDrive, ListChecks, LockKeyhole, LockOpen, MessageSquare, Network, Pencil, Pin, PinOff, RefreshCcw, SlidersHorizontal, TerminalSquare } from "lucide-react";

import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import type { ContextFile, DashboardSummary, FilePreview, FolderExplanation, GraphMap, GraphMapExpansionState, NavItem, NodeRelationships, ProjectScanState, ProjectSummary, SessionDiscoveryCandidate } from "../types";
import { formatOptionalBytes } from "../ui";
import { projectViewLabel } from "../workspaceRoute";
import { ProjectConnectionsHome, ProjectContextHome, ProjectSessionsHome, ProjectSpaceHome, type FileContextMenuHandler, type ProjectScanAction, type SessionContextMenuHandler } from "./ProjectHomeViews";
import { RecapView, type RecapDetailLayer } from "./RecapView";
import { ProjectFileTree } from "./project-center/FileTree";
import { FolderOverviewPane } from "./project-center/FolderOverviewPane";
import { PreviewPane } from "./project-center/PreviewPane";
import { TabStrip } from "./project-center/TabStrip";
import type { EditorBinding, OpenTab, PreviewMode, ProjectView, TreePage } from "./project-center/types";

export type { OpenTab, PreviewMode, ProjectView, TreePage } from "./project-center/types";

export function valueEditorAvailable(preview: FilePreview | null, editorAvailable: boolean): boolean {
  if (!preview || !editorAvailable) return false;
  return /\.(json|toml|js|jsx|mjs|cjs|ts|tsx|py|pyw|rs|go|c|h|cc|cpp|cxx|hpp|java|kt|kts|cs|css|scss)$/i.test(preview.path);
}

export function projectContextScanAction({
  project,
  scanState,
  canRescanProject,
  onRescanProject,
  onOpenScanFolders
}: {
  project: ProjectSummary | null;
  scanState: ProjectScanState | null;
  canRescanProject: boolean;
  onRescanProject: () => void;
  onOpenScanFolders: () => void;
}): ProjectScanAction | null {
  if (!project || scanState === null || scanState === "scanned") return null;
  if (canRescanProject) {
    return {
      kind: "scan",
      label: scanState === "scanning" ? "Scanning project" : "Re-scan project",
      help: `Refresh Code Hangar metadata for ${project.name}. The scan reads local files but does not modify them.`,
      disabled: scanState === "scanning",
      onSelect: onRescanProject
    };
  }
  return {
    kind: "folders",
    label: "Open scan folders",
    help: `Review scan folders before rescanning ${project.name}. Files on disk are not changed.`,
    disabled: false,
    onSelect: onOpenScanFolders
  };
}

/**
 * Honest projects-sidebar counter. When the list is collapsed to a preview, the
 * "N of M shown" phrasing lies — only a couple of rows are actually rendered — so
 * switch to "N match · K shown", where K is the real visible-row count. Falls back
 * to the plain "N of M shown" when the whole matching list is on screen.
 */
export function projectSidebarSummaryLabel({
  matchCount,
  totalCount,
  collapsed,
  hiddenCount
}: {
  matchCount: number;
  totalCount: number;
  collapsed: boolean;
  hiddenCount: number;
}): string {
  if (collapsed && hiddenCount > 0) {
    const shown = Math.max(0, matchCount - hiddenCount);
    return `${matchCount} match · ${shown} shown`;
  }
  return `${matchCount} of ${totalCount} shown`;
}

/** Context already owns the recommended-file queue, so a second guide rail only
 * repeats the same actions and starves the reading surface at narrower widths. */
export function shouldShowGuideRail(_projectView: ProjectView, _hasProject: boolean): boolean {
  return false;
}

export function ProjectLoadState({
  status,
  projectName,
  error,
  onRetry
}: {
  status: "loading" | "error";
  projectName: string | null;
  error?: string | null;
  onRetry?: () => void;
}) {
  return (
    <div className={`project-load-state ${status}`} role={status === "error" ? "alert" : "status"} aria-live="polite">
      {status === "loading" ? <RefreshCcw className="spin" size={22} /> : <AlertTriangle size={22} />}
      <strong>{status === "loading" ? `Loading ${projectName ?? "project"}` : `Could not load ${projectName ?? "project"}`}</strong>
      <p>{status === "loading" ? "Loading the project root, context and local metadata." : error ?? "The local project data could not be loaded."}</p>
      {status === "error" && onRetry ? <button className="action-button" type="button" data-help="Retry loading this project's root, context and local Git metadata." onClick={onRetry}>Retry</button> : null}
    </div>
  );
}

export function ProjectCenterView({
  projectView,
  setProjectView,
  selectedProject,
  selectedProjectId,
  preview,
  folderExplanation,
  previewMode,
  setPreviewMode,
  editor,
  togglePin,
  selectedPinned,
  tabs,
  draggedTabNodeId,
  tabDropTargetNodeId,
  showTabMenu,
  suppressNextTabClickRef,
  openNode,
  openNodeInTree,
  startTabPointerDrag,
  closeTab,
  loadStatus,
  loadError,
  loadProjectData,
  contentGridStyle,
  rootTreeItems,
  expandedTree,
  treePages,
  treeLoading,
  toggleExpandedTree,
  loadTreeChildren,
  continueSubtreeScan,
  explainFolder,
  showTreeMenu,
  showFileMenu,
  showSessionMenu,
  zoneShowProtectedMetadata,
  startTreeResize,
  contextFiles,
  projectOverlapWarning,
  showReview,
  selectedFootprint,
  projectScanState,
  projectStateLabel,
  canRescanProject,
  onRescanProject,
  onOpenScanFolders,
  projectSessions,
  onOpenSession,
  relationships,
  relationshipsNodeId,
  relationshipsLoading,
  graphMap,
  graphMapLoading,
  graphMapError,
  graphMapExpansion,
  onExpandGraphMap,
  onPauseGraphMap,
  onContinueGraphMap,
  revealPreview,
  zoneAllowSensitiveReveal,
  openProtectedSettings,
  setStatusText,
  onExplainSelection,
  onFileMutated,
  changesUnlocked,
  onRequestChangeAccess,
  onRelockChanges,
  connectorBuild = false,
  recapDetailLayer,
  onUndoAiSession
}: {
  projectView: ProjectView;
  setProjectView: (view: ProjectView) => void;
  selectedProject: ProjectSummary | null;
  selectedProjectId: number | null;
  preview: FilePreview | null;
  folderExplanation: FolderExplanation | null;
  previewMode: PreviewMode;
  setPreviewMode: (mode: PreviewMode) => void;
  editor: EditorBinding;
  togglePin: () => void;
  selectedPinned: boolean;
  tabs: OpenTab[];
  draggedTabNodeId: number | null;
  tabDropTargetNodeId: number | null;
  showTabMenu: (tab: OpenTab, event: MouseEvent<HTMLElement>) => void;
  suppressNextTabClickRef: MutableRefObject<boolean>;
  openNode: (nodeId: number) => void;
  openNodeInTree: (nodeId: number, projectId?: number | null) => void;
  startTabPointerDrag: (tab: OpenTab, event: ReactPointerEvent<HTMLButtonElement>) => void;
  closeTab: (nodeId: number) => void;
  loadStatus: "idle" | "loading" | "ready" | "error";
  loadError: string | null;
  loadProjectData: (projectId: number) => void;
  contentGridStyle: CSSProperties;
  rootTreeItems: NavItem[];
  expandedTree: Set<number>;
  treePages: Record<string, TreePage>;
  treeLoading: Set<string>;
  toggleExpandedTree: (navId: number) => void;
  loadTreeChildren: (parentNavId: number | null, options?: { append?: boolean; offset?: number }) => void | Promise<unknown>;
  continueSubtreeScan: (navId: number) => void;
  explainFolder: (item: NavItem) => void;
  showTreeMenu: (item: NavItem, event: MouseEvent<HTMLElement>) => void;
  showFileMenu: FileContextMenuHandler;
  showSessionMenu: SessionContextMenuHandler;
  zoneShowProtectedMetadata: boolean;
  startTreeResize: (event: ReactPointerEvent<HTMLDivElement> | MouseEvent<HTMLDivElement>) => void;
  contextFiles: ContextFile[];
  projectOverlapWarning: string | null;
  showReview: () => void;
  selectedFootprint: DashboardSummary["largestProjects"][number] | null;
  projectScanState: (project: ProjectSummary) => ProjectScanState;
  projectStateLabel: (state: ProjectScanState) => string;
  canRescanProject: boolean;
  onRescanProject: () => void;
  onOpenScanFolders: () => void;
  projectSessions: SessionDiscoveryCandidate[];
  onOpenSession: (session: SessionDiscoveryCandidate) => void;
  relationships: NodeRelationships | null;
  relationshipsNodeId: number | null;
  relationshipsLoading: boolean;
  graphMap: GraphMap | null;
  graphMapLoading: boolean;
  graphMapError: string | null;
  graphMapExpansion: GraphMapExpansionState;
  onExpandGraphMap: () => void;
  onPauseGraphMap: () => void;
  onContinueGraphMap: () => void;
  revealPreview: () => void;
  zoneAllowSensitiveReveal: boolean;
  openProtectedSettings: () => void;
  setStatusText: (value: string) => void;
  onExplainSelection?: (event: MouseEvent<HTMLElement>) => void;
  onFileMutated: (nodeId: number) => void | Promise<void>;
  changesUnlocked: boolean;
  onRequestChangeAccess: () => void;
  onRelockChanges: () => void;
  /** True only in the AI Connector edition; gates the optional AI summary and comments hint. */
  connectorBuild?: boolean;
  recapDetailLayer?: RecapDetailLayer;
  onUndoAiSession?: (nodeId: number, sessionId: string) => Promise<void>;
}) {
  const hasPreview = Boolean(preview);
  const hasValues = valueEditorAvailable(preview, editor.available);
  const fileDisplayPath = preview?.displayPath ?? folderExplanation?.displayPath ?? "Files";
  const fileDisplayName = preview?.displayName ?? folderExplanation?.displayName ?? "Browse files";
  const selectedProjectScanState = selectedProject ? projectScanState(selectedProject) : null;
  const selectedProjectScanAction = projectContextScanAction({
    project: selectedProject,
    scanState: selectedProjectScanState,
    canRescanProject,
    onRescanProject,
    onOpenScanFolders
  });
  const guideSolo = projectView !== "files";
  return (
    <>
      <nav className="project-workspace-nav" aria-label="Project workspace">
        <button className={projectView === "context" ? "active" : ""} type="button" onClick={() => setProjectView("context")} data-help="Read the files that explain what this project is and how it is meant to be used.">
          <FileText size={15} /> Context
        </button>
        <button className={projectView === "recap" ? "active" : ""} type="button" onClick={() => setProjectView("recap")} data-help="Reconstruct file edits recorded in local AI sessions without running code or guessing missing changes.">
          <FileDiff size={15} /> What changed
        </button>
        <button className={projectView === "files" ? "active" : ""} type="button" onClick={() => setProjectView("files")} data-help="Browse every inventoried file and folder in the selected project.">
          <Files size={15} /> Files
        </button>
        <button className={projectView === "space" ? "active" : ""} type="button" onClick={() => setProjectView("space")} data-help="Understand this project's local disk footprint and whether accounting is complete.">
          <HardDrive size={15} /> Space
        </button>
        <button className={projectView === "connections" ? "active" : ""} type="button" onClick={() => setProjectView("connections")} data-help="Inspect the project's local Hangar Map: workflows, models, caches, missing references and links for the open file. No remote service is contacted.">
          <Network size={15} /> Connections
        </button>
        <button className={projectView === "sessions" ? "active" : ""} type="button" onClick={() => setProjectView("sessions")} data-help="Show local AI conversations linked to this project from discovered session metadata.">
          <MessageSquare size={15} /> Sessions
        </button>
        <button className="project-review-button" type="button" onClick={showReview} disabled={!selectedProject} data-help="Review this selected project's ownership, protected areas, references and scan gaps. Nothing is changed.">
          <ListChecks size={15} /> Safe manage
        </button>
      </nav>
      <div className={`viewer-header ${projectView === "files" ? "" : "project-viewer-crumb"}`}>
        <div>
          <div className="breadcrumb">{selectedProject?.name ?? "No project"} / {projectView === "files" ? fileDisplayPath : projectViewLabel(projectView)}</div>
          {projectView === "files" ? <h1>{fileDisplayName}</h1> : null}
        </div>
        {projectView === "files" && hasPreview ? <div className="viewer-actions">
          <ConceptHelp concept="source" />
          <button className={`segmented ${hasPreview && previewMode === "rendered" ? "active" : ""}`} type="button" disabled={!hasPreview} onClick={() => setPreviewMode("rendered")} data-help={hasPreview ? "Show the rendered preview for the current file." : "Choose a file before switching preview modes."}>
            <Eye size={15} />
            Rendered
          </button>
          <button className={`segmented ${hasPreview && previewMode === "source" ? "active" : ""}`} type="button" disabled={!hasPreview} onClick={() => setPreviewMode("source")} data-help={hasPreview ? "Show the source text for the current file preview." : "Choose a file before switching preview modes."}>
            <TerminalSquare size={15} />
            Source
          </button>
          {editor.available && !changesUnlocked ? (
            <button className="segmented changes-locked" type="button" onClick={onRequestChangeAccess} data-help="Project files start locked. Read the safety warning and explicitly unlock this project before any edit controls become available.">
              <LockKeyhole size={15} />
              Changes locked
            </button>
          ) : null}
          {hasValues && changesUnlocked ? (
            <button className={`segmented ${previewMode === "values" ? "active" : ""}`} type="button" onClick={() => setPreviewMode("values")} data-help="Change one recognised text, number, colour or toggle. Review the exact line diff before applying one validated value.">
              <SlidersHorizontal size={15} />
              Safe values
            </button>
          ) : null}
          {editor.available && changesUnlocked ? (
            <button className={`segmented ${previewMode === "edit" ? "active" : ""}`} type="button" onClick={() => setPreviewMode("edit")} data-help="Prepare an advanced one-file text draft. Nothing is written until you review the exact diff, validation and local Git context.">
              <Pencil size={15} />
              Text change{editor.dirty ? " ●" : ""}
            </button>
          ) : null}
          {editor.available && changesUnlocked ? (
            <button className="segmented changes-unlocked" type="button" onClick={onRelockChanges} data-help="Lock project-file changes again. This closes the edit controls without changing the project.">
              <LockOpen size={15} />
              Lock changes
            </button>
          ) : null}
          <button className="icon-button" type="button" onClick={togglePin} disabled={!preview} aria-label={selectedPinned ? "Unpin file" : "Pin file"} data-help={selectedPinned ? "Remove this file from pinned items." : "Pin this file for quick access."}>
            {selectedPinned ? <PinOff size={16} /> : <Pin size={16} />}
          </button>
        </div> : projectView === "files" && folderExplanation ? (
          <span className="viewer-subject-kind"><Folder size={15} />Folder overview</span>
        ) : null}
      </div>

      {projectView === "files" ? (
        <TabStrip
          tabs={tabs}
          activeNodeId={preview?.nodeId ?? null}
          draggedTabNodeId={draggedTabNodeId}
          tabDropTargetNodeId={tabDropTargetNodeId}
          showTabMenu={showTabMenu}
          suppressNextTabClick={() => {
            if (!suppressNextTabClickRef.current) return false;
            suppressNextTabClickRef.current = false;
            return true;
          }}
          openNode={openNode}
          startTabPointerDrag={startTabPointerDrag}
          closeTab={closeTab}
        />
      ) : null}

      {loadStatus === "loading" ? (
        <ProjectLoadState status="loading" projectName={selectedProject?.name ?? null} />
      ) : loadStatus === "error" ? (
        <ProjectLoadState
          status="error"
          projectName={selectedProject?.name ?? null}
          error={loadError}
          onRetry={() => selectedProjectId ? void loadProjectData(selectedProjectId) : undefined}
        />
      ) : (
        <div className={`content-grid ${guideSolo ? "guide-grid-solo" : "file-grid"}`} style={contentGridStyle}>
          {projectView === "files" ? (
            <ProjectFileTree
              rootTreeItems={rootTreeItems}
              activeNodeId={preview?.nodeId ?? null}
              expandedTree={expandedTree}
              treePages={treePages}
              treeLoading={treeLoading}
              toggleExpandedTree={toggleExpandedTree}
              loadTreeChildren={loadTreeChildren}
              continueSubtreeScan={continueSubtreeScan}
              explainFolder={explainFolder}
              showTreeMenu={showTreeMenu}
              showProtectedMetadata={zoneShowProtectedMetadata}
              openNode={openNode}
            />
          ) : null}
          {projectView === "files" ? (
            <div
              className="tree-preview-resizer"
              role="separator"
              aria-label="Resize file tree"
              aria-orientation="vertical"
              data-help="Drag to resize the file tree."
              onMouseDown={startTreeResize}
            />
          ) : null}
          <article
            className="preview-surface"
            data-help={preview ? `Preview of ${preview.displayName}.` : "Preview area. Open a file from the tree, Context or Sessions."}
            onClick={(event) => {
              const target = event.target as HTMLElement;
              const link = target.closest<HTMLElement>("[data-local-path]");
              if (!link || !preview) return;
              event.preventDefault();
              const localPath = link.dataset.localPath;
              if (!localPath) return;
              void api.resolveLocalLink(preview.projectId, preview.nodeId, localPath).then((nodeId) => {
                if (nodeId) {
                  void openNodeInTree(nodeId, preview.projectId);
                } else {
                  setStatusText(`Local link not found inside project: ${localPath}`);
                }
              });
            }}
          >
            {projectView === "context" ? (
              <ProjectContextHome
                project={selectedProject}
                files={contextFiles}
                scanState={selectedProjectScanState}
                scanAction={selectedProjectScanAction}
                overlapWarning={projectOverlapWarning}
                onOpen={(nodeId) => void openNodeInTree(nodeId, selectedProjectId)}
                onContextMenu={showFileMenu}
                onOpenFiles={() => setProjectView("files")}
                connectorBuild={connectorBuild}
              />
            ) : projectView === "recap" && selectedProject ? (
              <RecapView projectId={selectedProject.id} sessions={projectSessions} onOpenSession={onOpenSession} onSessionContextMenu={showSessionMenu} DetailLayer={recapDetailLayer} />
            ) : projectView === "recap" ? (
              <div className="empty-state">Choose a project before opening its recap.</div>
            ) : projectView === "space" ? (
              <ProjectSpaceHome
                project={selectedProject}
                footprint={selectedFootprint}
                scanState={selectedProject ? projectScanState(selectedProject) : null}
                scanAction={selectedProjectScanAction}
                formatOptionalBytes={formatOptionalBytes}
                projectStateLabel={projectStateLabel}
              />
            ) : projectView === "connections" ? (
              <ProjectConnectionsHome
                preview={preview}
                relationships={relationships}
                relationshipsNodeId={relationshipsNodeId}
                relationshipsLoading={relationshipsLoading}
                graphMap={graphMap}
                graphMapLoading={graphMapLoading}
                graphMapError={graphMapError}
                graphMapExpansion={graphMapExpansion}
                onExpandGraphMap={onExpandGraphMap}
                onPauseGraphMap={onPauseGraphMap}
                onContinueGraphMap={onContinueGraphMap}
                onOpen={(nodeId) => void openNodeInTree(nodeId, selectedProjectId)}
                onContextMenu={showFileMenu}
              />
            ) : projectView === "sessions" ? (
              <ProjectSessionsHome
                key={selectedProjectId ?? "no-project"}
                projectId={selectedProjectId}
                sessions={projectSessions}
                onOpenSession={onOpenSession}
                onContextMenu={showSessionMenu}
              />
            ) : preview ? (
              <PreviewPane preview={preview} onReveal={revealPreview} canReveal={zoneAllowSensitiveReveal} onOpenProtectedSettings={openProtectedSettings} onContextMenu={onExplainSelection} editing={changesUnlocked && previewMode === "edit"} valuesEditing={changesUnlocked && previewMode === "values"} changeAuthorized={changesUnlocked} editor={editor} onFileMutated={onFileMutated} setStatusText={setStatusText} onUndoAiSession={onUndoAiSession} onOpenRecap={() => setProjectView("recap")} />
            ) : folderExplanation ? (
              <FolderOverviewPane folder={folderExplanation} />
            ) : (
              <div className="empty-state">Choose a file from the tree or Context. Projects with loaded context open their first useful file automatically.</div>
            )}
          </article>
        </div>
      )}
    </>
  );
}
