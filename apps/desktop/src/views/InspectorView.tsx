import { FileText, Info, Lock, Sparkles } from "lucide-react";
import type { ReactNode } from "react";
import { aiToolFile } from "../ai-tool-files";
import { ConceptHelp } from "../BeginnerHelp";
import { FILE_INSPECTOR_CONTEXT, type InspectorContextCopy } from "../projectInspector";
import type {
  FilePreview,
  FolderExplanation,
  GitRepoSummary,
  MutationLockInspection,
  NodeRelationships,
  OperationPlan,
  OrphanStatus,
  ProjectSummary,
  RiskReport
} from "../types";
import { SectionTitle, formatBytes, formatOptionalBytes, orphanReferenceSummary, plainConfidenceLabel, previewFileTypeLabel, previewStateHelp, previewStateLabel } from "../ui";
import { CommentsPanel } from "./CommentsPanel";
import { folderClassificationLabel, folderInventoryLabel } from "./project-center/FolderOverviewPane";

type PlanTarget = { nodeId: number; label: string; kind: string };

export function InspectorView({
  preview,
  folderExplanation,
  context = FILE_INSPECTOR_CONTEXT,
  gitStatus,
  relationships,
  relationshipsLoading,
  previewOrphanStatus,
  advancedMode,
  selectedProject,
  mutationAvailable,
  mutationLockLoading,
  mutationLockInspection,
  duplicateLoading,
  fileOrphanLoading,
  inspectCurrentFileDuplicates,
  evaluateCurrentFileOrphan,
  inspectCurrentFileLock,
  setPlanTargetNode,
  setOperationPlan,
  setRiskReport,
  showReview,
  setStatusText,
  openNode,
  explainPanel,
  connectorBuild = false
}: {
  preview: FilePreview | null;
  folderExplanation: FolderExplanation | null;
  context?: InspectorContextCopy;
  gitStatus: GitRepoSummary | null;
  relationships: NodeRelationships | null;
  relationshipsLoading: boolean;
  previewOrphanStatus: OrphanStatus | null | undefined;
  advancedMode: boolean;
  selectedProject: ProjectSummary | null;
  mutationAvailable: boolean;
  mutationLockLoading: boolean;
  mutationLockInspection: MutationLockInspection | null;
  duplicateLoading: boolean;
  fileOrphanLoading: boolean;
  inspectCurrentFileDuplicates: () => void;
  evaluateCurrentFileOrphan: () => void;
  inspectCurrentFileLock: () => void;
  setPlanTargetNode: (target: PlanTarget | null) => void;
  setOperationPlan: (plan: OperationPlan | null) => void;
  setRiskReport: (report: RiskReport | null) => void;
  showReview: () => void;
  setStatusText: (value: string) => void;
  openNode: (nodeId: number) => void;
  explainPanel?: ReactNode;
  /** True only in the AI Connector edition; gates the AI-apps mention in the comments hint. */
  connectorBuild?: boolean;
}) {
  const referenceSummary = orphanReferenceSummary(Boolean(preview), previewOrphanStatus);
  const sectionLabel = !preview && folderExplanation ? "Folder details" : context.sectionLabel;
  return (
    <>
    <section className="pane-section">
      <SectionTitle icon={<Info size={15} />} label={sectionLabel} />
      {!preview && !folderExplanation ? (
        <div className="details-empty">
          <FileText size={22} />
          <strong>{context.emptyTitle}</strong>
          <p>{context.emptyBody}</p>
        </div>
      ) : null}
      {preview ? (
        <>
      <dl className="inspector-list">
        <dt>Status</dt>
        <dd data-help={previewStateHelp(preview?.state)}>{previewStateLabel(preview?.state)}</dd>
        <dt>Path</dt>
        <dd data-help="Display path for the selected file or folder. Long-path prefixes are hidden here.">{preview?.displayPath ?? preview?.path ?? "None"}</dd>
        <dt>File type</dt>
        <dd data-help="Human-readable type inferred from the file name plus safe preview capability. Context files can still render safely when they are JSON or plain text.">{previewFileTypeLabel(preview)}</dd>
        <dt>File size</dt>
        <dd data-help="Known file length from the local inventory for the opened node. Large previews can still be capped below this size.">{formatOptionalBytes(preview?.sizeBytes)}</dd>
      </dl>
      {(() => {
        const tool = aiToolFile(preview?.path);
        if (!tool) return null;
        return (
          <div className="ai-tool-file" data-help="Code Hangar recognized this as a file that steers an AI coding tool, and explains what it does.">
            <h3><Sparkles size={14} /> {tool.tool} · {tool.role}</h3>
            <p>{tool.impact}</p>
            <small className="muted">Scope: {tool.scope}</small>
          </div>
        );
      })()}
      <details className="inspector-extra metadata-details">
        <summary>More file and Git details</summary>
        <dl className="inspector-list small">
          <dt>Headings</dt>
          <dd data-help="Markdown headings detected in the previewed document.">{preview?.headings.length ?? 0}</dd>
          <dt>Links</dt>
          <dd data-help="Markdown links detected in the previewed document. Remote links remain inert.">{preview?.links.length ?? 0}</dd>
          <dt>Branch</dt>
          <dd data-help="Passive local Git branch metadata, if a .git folder was detected. No Git command is run.">{gitStatus?.hasGit ? gitStatus.currentBranch ?? "detached" : "not detected"}</dd>
          <dt>Origin</dt>
          <dd data-help="Origin value read from local Git config metadata only. Code Hangar does not contact the remote.">{gitStatus?.originUrl ?? "None"}</dd>
        </dl>
      </details>
      {gitStatus?.metadataError ? <div className="warning"><Lock size={15} />{gitStatus.metadataError}</div> : null}
      {preview?.blockedReason ? <div className="warning"><Lock size={15} />{preview.blockedReason}</div> : null}
      <div className="inspector-actions-panel" data-help="Useful next steps for the currently opened file. Each check is local and starts only when you choose it.">
        <h3>Explore this file</h3>
        <div className="inspector-action-grid">
          <button type="button" disabled={!preview || duplicateLoading} onClick={() => void inspectCurrentFileDuplicates()} data-help="Compare this file with same-size local candidates using a bounded check. Nothing is removed.">
            Find duplicate files
          </button>
          <button
            type="button"
            className={fileOrphanLoading ? "is-loading" : ""}
            disabled={!preview || fileOrphanLoading}
            aria-busy={fileOrphanLoading}
            onClick={() => void evaluateCurrentFileOrphan()}
            data-help="Check whether the local inventory knows of another file that refers to this one. This is not a delete recommendation."
          >
            {fileOrphanLoading ? "Checking..." : "Check references"}
          </button>
          <button type="button" disabled={!preview || !mutationAvailable || mutationLockLoading} onClick={() => void inspectCurrentFileLock()} data-help="Supported builds only: check whether another process appears to be using this file before a disk action.">
            {mutationLockLoading ? "Checking..." : "Check file availability"}
          </button>
          <button type="button" disabled={!preview} onClick={() => {
            if (!preview) return;
            setPlanTargetNode({ nodeId: preview.nodeId, label: preview.displayName, kind: "file" });
            setOperationPlan(null);
            setRiskReport(null);
            showReview();
            setStatusText(`Safe Manage target set to ${preview.displayName}. Press Load review when ready.`);
          }} data-help="Open Safe Manage for a read-only local review of this file's ownership, references and protection. Nothing is changed.">
            Safe Manage
          </button>
        </div>
        <div className="inspector-reference-summary" data-help="Reference status is evaluated only when you press Check references. A missing reference does not mean the file is safe to remove.">
          <div className={`reference-status-card ${referenceSummary.tone}`}>
            <span>Reference state</span>
            <strong>{referenceSummary.state}</strong>
            <small>{referenceSummary.confidenceLabel}</small>
          </div>
          <div className="reference-status-card">
            <span>Counted refs</span>
            <strong>{referenceSummary.countLabel}</strong>
            <small>{referenceSummary.reason}</small>
          </div>
        </div>
        {advancedMode ? (
        <details className="inspector-extra">
          <summary>Advanced details</summary>
          <dl className="inspector-list small">
            <dt>Project</dt>
            <dd>{selectedProject?.name ?? preview?.projectId ?? "None"}</dd>
            <dt>Node ID</dt>
            <dd>{preview?.nodeId ?? "None"}</dd>
            <dt>Preview cap</dt>
            <dd>{preview ? formatBytes(preview.previewLimitBytes) : "None"}</dd>
            <dt>Truncated</dt>
            <dd>{preview ? (preview.truncated ? "Yes" : "No") : "None"}</dd>
            <dt>Revealed</dt>
            <dd>{preview ? (preview.wasRevealed ? "Yes, transient" : "No") : "None"}</dd>
          <dt>System error</dt>
          <dd>{preview?.systemErrorCode ?? "None"}</dd>
          <dt>Lock state</dt>
          <dd data-help="File availability checks are available only in editions with local disk actions. Free means the file could be opened for write; locked means another process likely holds a conflicting handle.">
            {!mutationAvailable ? "not available in this edition" : mutationLockInspection ? `${mutationLockInspection.state} · ${mutationLockInspection.path}` : "not inspected"}
          </dd>
        </dl>
        </details>
        ) : null}
      </div>
        </>
      ) : null}
      {preview && relationshipsLoading ? (
        <div className="relationships-panel" data-help={`Local Markdown and workflow relationships for ${preview.displayName} are loading after the preview.`}>
          <div className="heading-with-help"><h3>Local References</h3><ConceptHelp concept="references" /></div>
          <ul className="relationship-list relationship-list-skeleton" aria-hidden="true">
            {Array.from({ length: 3 }).map((_, index) => (
              <li key={`relationship-skeleton-${index}`}>
                <span className="skeleton skeleton-line skeleton-line-label" />
                <span className="skeleton skeleton-line skeleton-line-value" />
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {preview && !relationshipsLoading && relationships && (relationships.outgoing.length > 0 || relationships.incoming.length > 0 || relationships.issues.length > 0) ? (
        <div className="relationships-panel" data-help={`Show local Markdown links and workflow/model relationships for ${preview.displayName}.`}>
          <div className="heading-with-help"><h3>Local References</h3><ConceptHelp concept="references" /></div>
          {relationships.outgoing.length ? (
            <div className="relationship-group">
              <div className="relationship-group-head">
                <h4>Links from this file</h4>
                <span className="relationship-count-pill">{relationships.outgoing.length}</span>
              </div>
              <ul className="relationship-list">
                {relationships.outgoing.map((relationship) => (
                  <li key={`out-${relationship.nodeId}-${relationship.path}`}>
                    <button type="button" onClick={() => void openNode(relationship.nodeId)} data-help={`Open referenced file ${relationship.displayName}.`}>
                      <strong>{relationship.displayName}</strong>
                    </button>
                    <span className="relationship-confidence">{plainConfidenceLabel(relationship.confidence, "link")}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {relationships.incoming.length ? (
            <div className="relationship-group">
              <div className="relationship-group-head">
                <h4>Referenced by</h4>
                <span className="relationship-count-pill">{relationships.incoming.length}</span>
              </div>
              <ul className="relationship-list">
                {relationships.incoming.map((relationship) => (
                  <li key={`in-${relationship.nodeId}-${relationship.path}`}>
                    <button type="button" onClick={() => void openNode(relationship.nodeId)} data-help={`Open file linking here: ${relationship.displayName}.`}>
                      <strong>{relationship.displayName}</strong>
                    </button>
                    <span className="relationship-confidence">{plainConfidenceLabel(relationship.confidence, "link")}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {relationships.issues.length ? (
            <div className="relationship-group relationship-group-review">
              <div className="relationship-group-head">
                <h4>Needs review</h4>
                <span className="relationship-count-pill warn">{relationships.issues.length}</span>
              </div>
              <ul className="relationship-list warning-list">
                {relationships.issues.map((issue) => (
                  <li key={`issue-${issue.kind}-${issue.target}`}>
                    <strong>{issue.target}</strong>
                    <span className="relationship-confidence">{plainConfidenceLabel(issue.confidence, "signal")}</span>
                    <small>{issue.kind.replaceAll("_", " ")}{issue.evidence ? ` · ${issue.evidence}` : ""}</small>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      ) : null}
      {folderExplanation ? (
        <div className="folder-explanation folder-details" data-help={`Local inventory details for folder ${folderExplanation.displayName}.`}>
          <dl className="inspector-list small">
            <dt>Path</dt>
            <dd>{folderExplanation.displayPath}</dd>
            <dt>Classification</dt>
            <dd>{folderClassificationLabel(folderExplanation.classification)}</dd>
            <dt>Match strength</dt>
            <dd>{plainConfidenceLabel(folderExplanation.confidence, "folder match")}</dd>
            <dt>Direct children</dt>
            <dd>{folderExplanation.childCount}</dd>
            <dt>Space used</dt>
            <dd>{formatOptionalBytes(folderExplanation.physicalBytes)}{folderExplanation.footprintPartial ? "+" : ""}</dd>
            <dt>Total file sizes</dt>
            <dd>{formatOptionalBytes(folderExplanation.apparentBytes)}{folderExplanation.footprintPartial ? "+" : ""}</dd>
            <dt>Inventory</dt>
            <dd>{folderInventoryLabel(folderExplanation)}</dd>
          </dl>
        </div>
      ) : null}
    </section>
    {explainPanel}
    <CommentsPanel nodeId={preview?.nodeId ?? null} connectorBuild={connectorBuild} />
    </>
  );
}
