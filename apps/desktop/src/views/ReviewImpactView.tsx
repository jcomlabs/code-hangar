import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import { CheckCircle2, Database, ListChecks, ShieldCheck } from "lucide-react";
import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import type {
  FilePreview,
  OperationPlan,
  PlanPreviewStatus,
  ProjectSummary,
  RiskReport
} from "../types";
import { PlanList, formatBytes, formatDependentRow, formatOptionalBytes, type PlanRow } from "../ui";

type PlanTarget = { nodeId: number; label: string; kind: string };
type DiscoverView = "projects" | "search" | "lost" | "assets" | "duplicates";
type SafeManageResultView = "summary" | "attention" | "details";

export const SAFE_MANAGE_RISK_GUIDE = [
  {
    tier: "green",
    label: "Green",
    summary: "Cache and temp",
    detail: "Usually disposable cache or temporary material. Confirm the path still belongs to generated workspace state."
  },
  {
    tier: "yellow",
    label: "Yellow",
    summary: "Dependencies and builds",
    detail: "Dependency or build folders. Useful to measure, but still review them in the context of the project."
  },
  {
    tier: "orange",
    label: "Orange",
    summary: "Generated context",
    detail: "Generated outputs, datasets, prompts or workflow assets that may carry higher context value."
  },
  {
    tier: "red",
    label: "Red",
    summary: "Shared or referenced",
    detail: "Shared or multi-reference assets. Do not treat them as owned by a single project."
  },
  {
    tier: "black",
    label: "Black",
    summary: "Protected or sensitive",
    detail: "Protected, sensitive or out-of-bound entries. They stay excluded from recoverable estimates."
  }
] as const;

export function riskTierVisibility<T extends { count: number; physicalBytes?: number | null }>(tiers: T[]) {
  const active = tiers.filter((tier) => tier.count > 0 || (tier.physicalBytes ?? 0) > 0);
  return {
    active,
    clearCount: tiers.length - active.length
  };
}

export function safeManageAttentionCounts(
  plan: Pick<OperationPlan, "sharedAssets" | "danglingAfter" | "gitWarnings">,
  sensitiveProtectedCount: number
) {
  return {
    shared: plan.sharedAssets.length,
    dependents: plan.danglingAfter.length,
    sensitiveProtected: sensitiveProtectedCount,
    gitWarnings: plan.gitWarnings.length
  };
}

export function ReviewImpactView({
  planTargetNode,
  setPlanTargetNode,
  selectedProject,
  selectedProjectId,
  preview,
  planJobId,
  planJobStatus,
  planLoading,
  operationPlan,
  setOperationPlan,
  riskReport,
  setRiskReport,
  reportLoading,
  advancedMode,
  setAdvancedMode,
  mutationAvailable,
  mutationBackupLevel,
  setMutationBackupLevel,
  mutationAllowSameVolume,
  setMutationAllowSameVolume,
  mutationModeToken,
  mutationBusy,
  mutationMessage,
  setPlanJobId,
  setPlanJobStatus,
  setPlanLoading,
  setStatusText,
  buildPreviewPlan,
  cancelPreviewPlan,
  exportRiskReport,
  enterMutationMode,
  runMutationBackup,
  runMutationMove,
  selectProject,
  showDiscover,
  showRecovery,
  setOrphanScope,
  setOrphanMode,
  setOrphanAutoRunSeq
}: {
  planTargetNode: PlanTarget | null;
  setPlanTargetNode: (target: PlanTarget | null) => void;
  selectedProject: ProjectSummary | null;
  selectedProjectId: number | null;
  preview: FilePreview | null;
  planJobId: string | null;
  planJobStatus: PlanPreviewStatus | null;
  planLoading: boolean;
  operationPlan: OperationPlan | null;
  setOperationPlan: (plan: OperationPlan | null) => void;
  riskReport: RiskReport | null;
  setRiskReport: (report: RiskReport | null) => void;
  reportLoading: boolean;
  advancedMode: boolean;
  setAdvancedMode: Dispatch<SetStateAction<boolean>>;
  mutationAvailable: boolean;
  mutationBackupLevel: "minimal" | "standard" | "full";
  setMutationBackupLevel: (value: "minimal" | "standard" | "full") => void;
  mutationAllowSameVolume: boolean;
  setMutationAllowSameVolume: (value: boolean) => void;
  mutationModeToken: string | null;
  mutationBusy: boolean;
  mutationMessage: string | null;
  setPlanJobId: (value: string | null) => void;
  setPlanJobStatus: (value: PlanPreviewStatus | null) => void;
  setPlanLoading: (value: boolean) => void;
  setStatusText: (value: string) => void;
  buildPreviewPlan: (nodeId?: number) => void;
  cancelPreviewPlan: () => void;
  exportRiskReport: () => void;
  enterMutationMode: () => void;
  runMutationBackup: () => void;
  runMutationMove: () => void;
  selectProject: (projectId: number) => void;
  showDiscover: (view: DiscoverView) => void;
  showRecovery: () => void;
  setOrphanScope: (value: "current" | "all") => void;
  setOrphanMode: (value: "lost" | "assets") => void;
  setOrphanAutoRunSeq: Dispatch<SetStateAction<number>>;
}) {
  const [resultView, setResultView] = useState<SafeManageResultView>("summary");
  const hasExportablePreview = Boolean(operationPlan && riskReport);
  const calculateDisabled = planLoading || hasExportablePreview;
  const calculateHelp = hasExportablePreview
    ? "This review already has an exportable JSON report. Change the target to load a fresh review."
    : "Load a read-only local review for the current target.";
  const targetName = planTargetNode ? planTargetNode.label : selectedProject?.name ?? "—";
  const statusLabel = planLoading
    ? "Loading review…"
    : operationPlan
      ? `Review ready · ${operationPlan.target.displayName}`
      : "Review not loaded";
  const sensitiveProtectedItems = operationPlan
    ? sensitiveProtectedRows(operationPlan.sensitiveFiles, operationPlan.protectedHits)
    : [];
  const reportedRiskTiers = new Map((riskReport?.riskCounts ?? []).map((risk) => [risk.tier, risk]));
  const riskTiers = riskReport ? [
    ...SAFE_MANAGE_RISK_GUIDE.map((guide) => ({
      ...(reportedRiskTiers.get(guide.tier) ?? { tier: guide.tier, count: 0, physicalBytes: 0 }),
      guide
    })),
    ...riskReport.riskCounts
      .filter((risk) => !SAFE_MANAGE_RISK_GUIDE.some((entry) => entry.tier === risk.tier))
      .map((risk) => ({ ...risk, guide: undefined }))
  ] : [];
  const visibleRiskTiers = riskTierVisibility(riskTiers);
  const attentionCounts = operationPlan
    ? safeManageAttentionCounts(operationPlan, sensitiveProtectedItems.length)
    : { shared: 0, dependents: 0, sensitiveProtected: 0, gitWarnings: 0 };
  const hasAttentionDetail = Object.values(attentionCounts).some((count) => count > 0);
  const attentionTotal = Object.values(attentionCounts).reduce((sum, count) => sum + count, 0);
  const clearedAttentionLabels = operationPlan ? [
    attentionCounts.shared === 0 ? "shared assets" : null,
    attentionCounts.dependents === 0 ? "dependents" : null,
    attentionCounts.sensitiveProtected === 0 ? "sensitive/protected paths" : null,
    attentionCounts.gitWarnings === 0 ? "Git warnings" : null
  ].filter((label): label is string => Boolean(label)) : [];
  const advancedPrompt = safeManageAdvancedPrompt(advancedMode);
  const showAdvancedDetails = () => {
    if (!advancedMode) setAdvancedMode(true);
    setResultView("details");
    setStatusText("Showing exact local evidence for the Safe Manage findings.");
  };

  useEffect(() => {
    setResultView("summary");
  }, [operationPlan]);

  return (
    <section className="pane-section compact safe-manage">
      <header className="safe-manage-header" data-help="This review reads the local inventory to explain ownership, references, protection and likely local impact. It never changes files, stores an action queue, or contacts any service. Byte totals are conservative: sensitive, protected, shared, reparse and uncertain entries are excluded. A '+' means the scan is incomplete, so the value means 'at least this much'.">
        <div className="safe-manage-heading">
          <span className="safe-manage-heading-icon"><ListChecks size={17} /></span>
          <div>
            <span>Read-only evidence review</span>
            <div className="heading-with-help">
              <h1>Safe Manage</h1>
              <ConceptHelp concept="safeManage" />
            </div>
          </div>
        </div>
        <p>Understand ownership, references, protected paths and scan gaps before deciding what still matters.</p>
        <div className="safe-manage-assurances" aria-label="Review assurances">
          <span><Database size={13} /> Local inventory only</span>
          <span><ShieldCheck size={13} /> Protected bytes excluded</span>
          <span><CheckCircle2 size={13} /> Review leaves files untouched</span>
        </div>
      </header>

      <div className="plan-workflow">
        {/* Step 1 — pick a target and load the read-only review */}
        <section className="plan-step" data-help="Choose a target (whole project, or the open file) and load a read-only review from the local inventory and relationship graph. Nothing on disk changes.">
          <div className="safe-manage-step">
            <h3>Choose a project or file</h3>
            <span className={`safe-manage-status ${operationPlan ? "ready" : ""}`} data-help="Whether a fresh preview has been calculated for the current target.">{statusLabel}</span>
          </div>
          <div className="plan-target">
            <span className="plan-target-label">
              Reviewing <strong>{targetName}</strong>
              <small>{planTargetNode ? planTargetNode.kind : "whole project"}</small>
            </span>
            <div className="plan-target-actions">
              <button type="button" className={planTargetNode ? "" : "active"} disabled={!selectedProjectId} data-help="Review the whole project. This only sets the target — nothing loads until you press Load review." onClick={() => {
                if (planJobId) void api.operationPlanCancel(planJobId);
                setPlanTargetNode(null);
                setOperationPlan(null);
                setRiskReport(null);
                setPlanJobId(null);
                setPlanJobStatus(null);
                setPlanLoading(false);
                setStatusText("Safe Manage target set to this project. Press Load review when ready.");
              }}>Whole project</button>
              <button type="button" className={planTargetNode ? "active" : ""} disabled={!preview} data-help="Review just the file currently open. This only sets the target — nothing loads until you press Load review." onClick={() => {
                if (!preview) return;
                if (planJobId) void api.operationPlanCancel(planJobId);
                setPlanTargetNode({ nodeId: preview.nodeId, label: preview.displayName, kind: "file" });
                setOperationPlan(null);
                setRiskReport(null);
                setPlanJobId(null);
                setPlanJobStatus(null);
                setPlanLoading(false);
                setStatusText(`Safe Manage target set to open file ${preview.displayName}. Press Load review when ready.`);
              }}>Open file{preview ? `: ${preview.displayName}` : ""}</button>
            </div>
          </div>
          <div className="button-row">
            <button className="action-button" type="button" disabled={calculateDisabled} data-help={calculateHelp} onClick={() => void buildPreviewPlan()}>
              {planLoading ? "Loading…" : hasExportablePreview ? "Review ready" : "Load review"}
            </button>
            <button className="secondary-button" type="button" disabled={!planJobId || planJobStatus?.state === "cancelling"} data-help="Stop the current review load at the next local checkpoint. This interrupts database reads and in-memory accounting; project files are not changed." onClick={() => void cancelPreviewPlan()}>
              {planJobStatus?.state === "cancelling" ? "Stopping…" : "Stop"}
            </button>
            <button className="secondary-button" type="button" disabled={reportLoading || !operationPlan} data-help="Write the current review JSON to a path you choose. This writes only the report, not project files." onClick={() => void exportRiskReport()}>
              {reportLoading ? "Exporting…" : "Export review"}
            </button>
          </div>
          {planLoading ? (
            <p className="plan-progress" data-help="The review is loading in the background. You can change performance mode or stop it.">
              {planJobStatus?.message ?? "Starting local review."}
            </p>
          ) : null}
          {!operationPlan && !planLoading ? (
            <p className="muted result-empty">No review loaded yet — pick a target and press <strong>Load review</strong>.</p>
          ) : null}
        </section>

        {/* Step 2 — results, only once a preview exists */}
        {operationPlan ? (
          <>
            <nav className="safe-manage-result-tabs" role="tablist" aria-label="Safe Manage review sections">
              <button type="button" role="tab" aria-selected={resultView === "summary"} className={resultView === "summary" ? "active" : ""} onClick={() => setResultView("summary")}>
                <span>Summary</span><small>Start here</small>
              </button>
              <button type="button" role="tab" aria-selected={resultView === "attention"} className={resultView === "attention" ? "active" : ""} onClick={() => setResultView("attention")}>
                <span>Needs attention</span><small>{attentionTotal} finding{attentionTotal === 1 ? "" : "s"}</small>
              </button>
              <button type="button" role="tab" aria-selected={resultView === "details"} className={resultView === "details" ? "active" : ""} onClick={showAdvancedDetails}>
                <span>Exact evidence</span><small>Paths and sources</small>
              </button>
            </nav>
            <p className="safe-manage-result-guide">
              {resultView === "summary"
                ? "Begin with the conservative space estimate and the recommended next review."
                : resultView === "attention"
                  ? "Use these counts to see why Code Hangar is being cautious. Nothing here is a deletion instruction."
                  : "These are the exact local clues behind the counts. Review paths and cross-project links before any optional disk action."}
            </p>
            {resultView === "summary" ? (
            <section className="plan-step" data-help="What appears to belong to the target and what is excluded from the recoverable estimate. Recoverable bytes are a conservative minimum; '+' / 'Incomplete' mean the scan still has more to count.">
              <div className="safe-manage-step">
                <div className="heading-with-help">
                  <h3>Review summary</h3>
                  <ConceptHelp concept="space" />
                </div>
              </div>
              <div className="safe-manage-metric-grid">
                <div className="primary" data-help="Minimum recoverable bytes counted after excluding sensitive, protected, shared and reparse entries.">
                  <span>Recoverable minimum</span>
                  <strong>{formatBytes(operationPlan.recoverableBytes.total)}{operationPlan.recoverableBytes.partial ? "+" : ""}</strong>
                  <small>Conservative local estimate</small>
                </div>
                <div data-help="Bytes that appear local to this target rather than shared with another project.">
                  <span>Owned by target</span>
                  <strong>{formatBytes(operationPlan.recoverableBytes.owned)}</strong>
                  <small>Known non-shared bytes</small>
                </div>
                <div data-help="Assets outside the target that may become unreferenced if the target disappears later. Review only.">
                  <span>Unreferenced after review</span>
                  <strong>{formatBytes(operationPlan.recoverableBytes.orphanedOnRemoval)}</strong>
                  <small>Discovery signal only</small>
                </div>
              </div>
              <div className="safe-manage-facts">
                <span><small>Target</small><strong>{operationPlan.target.displayName}</strong></span>
                <span data-help="Incomplete means some subtree still needs scanning; totals remain a lower bound."><small>Inventory</small><strong>{operationPlan.partialFootprint ? "Incomplete · minimum" : "Complete"}</strong></span>
                <span data-help="The preview is local-only and does not contact or modify cloud or external services."><small>Cloud/services</small><strong>{operationPlan.externalServicesUnaffected ? "Untouched" : "Unknown"}</strong></span>
              </div>
              <div className={`safe-manage-outcome ${attentionTotal > 0 ? "attention" : "clear"}`}>
                <strong>{attentionTotal > 0 ? `${attentionTotal} finding${attentionTotal === 1 ? "" : "s"} deserve a closer look` : "No warning category has findings"}</strong>
                <span>{attentionTotal > 0 ? "Open Needs attention before considering the optional disk actions." : "This lowers review effort, but it is not proof that an item should be removed."}</span>
              </div>
            </section>
            ) : null}

            {resultView === "attention" ? (
            <section className="plan-step" data-help="Risk tiers classify how much human review the local evidence deserves. Green = lowest effort (cache/temp); Yellow = dependencies/build; Orange = generated outputs/datasets/prompts; Red = shared or multi-referenced; Black = protected/sensitive, excluded from recoverable estimates.">
              <div className="safe-manage-step">
                <div className="heading-with-help">
                  <h3>Risk signals</h3>
                  <ConceptHelp concept="safeManage" />
                </div>
              </div>
              {visibleRiskTiers.active.length ? (
                <div className="risk-tier-grid" aria-label="Risk level totals">
                  {visibleRiskTiers.active.map((risk) => (
                    <div className={`risk-tier-card risk-${risk.tier}`} key={risk.tier} data-help={risk.guide?.detail ?? "Review this risk tier in context."}>
                      <div>
                        <strong>{risk.guide?.label ?? risk.tier}</strong>
                        <span>{risk.guide?.summary ?? "Review in context"}</span>
                      </div>
                      <div className="risk-tier-total">
                        <strong>{risk.count}</strong>
                        <small>{risk.count === 1 ? "item" : "items"} · {formatOptionalBytes(risk.physicalBytes)}</small>
                      </div>
                    </div>
                  ))}
                </div>
              ) : <p className="plan-clean-note"><span className="plan-clean-check" aria-hidden="true">✓</span>No risk signals in this review.</p>}
              {visibleRiskTiers.clearCount > 0 && visibleRiskTiers.active.length ? (
                <p className="safe-manage-clear-summary"><CheckCircle2 size={14} /> {visibleRiskTiers.clearCount} other risk level{visibleRiskTiers.clearCount === 1 ? " has" : "s have"} no findings.</p>
              ) : null}
              <details className="risk-guide-details">
                <summary>What each risk level means</summary>
                <div className="risk-guide-list">
                  {SAFE_MANAGE_RISK_GUIDE.map((entry) => (
                    <p key={entry.tier}><strong>{entry.label}:</strong> {entry.detail}</p>
                  ))}
                </div>
              </details>
            </section>
            ) : null}

            {resultView === "attention" || resultView === "details" ? (
            <section className="plan-step" data-help="The reasons Code Hangar refuses to present these bytes as simply safe to reclaim. Empty lists are good — they lower review risk. Shared, sensitive/protected, dangling refs and Git warnings each need a look before any later action.">
              <div className="safe-manage-step">
                <div className="heading-with-help">
                  <h3>{resultView === "details" ? "Exact local evidence" : "What needs attention"}</h3>
                  <ConceptHelp concept="evidence" />
                </div>
              </div>
              <div className="plan-attention-grid">
                <div>
                  <span>Shared assets</span>
                  <strong>{attentionCounts.shared}</strong>
                  <small>Excluded from owned totals</small>
                </div>
                <div>
                  <span>Files that rely on this</span>
                  <strong>{attentionCounts.dependents}</strong>
                  <small>{operationPlan.danglingAfter.filter((item) => item.crossProject).length} cross-project</small>
                </div>
                <div>
                  <span>Private or protected</span>
                  <strong>{attentionCounts.sensitiveProtected}</strong>
                  <small>Kept out of estimates</small>
                </div>
                <div>
                  <span>Git safety notes</span>
                  <strong>{attentionCounts.gitWarnings}</strong>
                  <small>Local metadata signals</small>
                </div>
              </div>
              <p className="safe-manage-confidence" data-help="How strongly the local clues support these warnings. Possible, weak or unclear clues need more manual review.">
                <strong>How certain these warnings are</strong><span>{reviewConfidenceLabel(operationPlan.confidenceSummary)}</span>
              </p>
              {clearedAttentionLabels.length ? (
                <p className="safe-manage-clear-summary"><CheckCircle2 size={14} /> No findings in {clearedAttentionLabels.join(", ")}.</p>
              ) : null}
              {resultView === "details" ? (
                <>
                  <details className="risk-guide-details">
                    <summary>What each risk level means</summary>
                    <div className="risk-guide-list">
                      {SAFE_MANAGE_RISK_GUIDE.map((entry) => (
                        <p key={entry.tier}><strong>{entry.label}:</strong> {entry.detail}</p>
                      ))}
                    </div>
                  </details>
                  {attentionCounts.shared > 0 ? <PlanList title="Shared assets" note="Shared with another registered project — resolve overlapping roots before treating it as recoverable." empty="No shared assets detected." items={operationPlan.sharedAssets.map((asset) => ({ primary: asset.displayName, segments: [{ text: asset.referencedBy.map((ref) => ref.projectName).join(", ") || "external reference", tone: "tag" as const }] }))} help="Shared assets are excluded from recoverable bytes and need manual review." /> : null}
                  {attentionCounts.dependents > 0 ? <PlanList title="Dependents" note="Local files and workflows that point at this target. Cross-project dependencies (⚠) deserve extra review." empty="No known local dependents." items={operationPlan.danglingAfter.map(formatDependentRow)} help="Cross-project dependents (⚠) mean this target is linked from another project." /> : null}
                  {operationPlan.danglingTruncated ? <p className="plan-value-note">Showing the first matches only — more dependents may exist.</p> : null}
                  {attentionCounts.sensitiveProtected > 0 ? <PlanList title="Sensitive/protected" note="Secrets, credentials and strong protected zones stay out of cleanup estimates unless explicitly reviewed." empty="No sensitive or protected hits inside target." items={sensitiveProtectedItems} help="Sensitive and protected entries are excluded from recoverable bytes." /> : null}
                  {attentionCounts.gitWarnings > 0 ? <PlanList title="Git warnings" note="Local-only Git signals — uncommitted or repository-like work worth reviewing outside Code Hangar." empty="No local Git metadata warning." items={operationPlan.gitWarnings.map((item) => item.message)} help="Warnings based only on local metadata already in the inventory." /> : null}
                </>
              ) : advancedPrompt && hasAttentionDetail ? (
                <div className="plan-detail-mode-prompt" data-help="Counts stay compact in Simple mode. Detailed evidence expands only categories with findings, including grouped dependency-cache rows.">
                  <span>{advancedPrompt.note}</span>
                  <button type="button" className="secondary-button slim" onClick={showAdvancedDetails}>
                    Show exact paths and sources
                  </button>
                </div>
              ) : null}
            </section>
            ) : null}

            {resultView === "summary" ? (
            <section className="plan-step" data-help="Guidance for what to do next. The report is evidence for you to review later — it is not an action queue and Code Hangar never executes it.">
              <div className="safe-manage-step">
                <h3>Recommended next review</h3>
              </div>
              <div className="plan-recommendation" data-help="Recommendation is only guidance for reviewing this preview. It is not an action queue.">
                <strong>Review guidance</strong>
                <span>{operationPlan.recommendedAction}</span>
              </div>
              <div className="plan-related" data-help="Unreferenced files are useful discovery candidates, but they are not automatically safe to remove.">
                <div>
                  <strong>Check unreferenced files</strong>
                  <span>These are discovery candidates only. Confirm references before deciding what still matters.</span>
                </div>
                <button type="button" className="secondary-button" data-help="Open Discover filtered to this project's unreferenced files." onClick={() => {
                  const projectId = operationPlan.target.projectId ?? selectedProjectId;
                  if (projectId) selectProject(projectId);
                  setOrphanScope("current");
                  setOrphanMode("assets");
                  showDiscover("assets");
                  setOrphanAutoRunSeq((seq) => seq + 1);
                }}>Review unreferenced files</button>
              </div>
              {mutationAvailable ? (
                <button type="button" className="secondary-button plan-recovery-link" data-help="Open Recover to review recovery records created by supported safe actions." onClick={showRecovery}>Open recovery history</button>
              ) : null}
            </section>
            ) : null}
          </>
        ) : null}

        {/* Disk actions — optional, last, calm. Backup or move; needs a fresh preview. */}
        {mutationAvailable ? (
          <details className="plan-step safe-manage-actions">
            <summary data-help="Optional. Files only ever change after this explicit sequence: calculate a preview, unlock one short-lived action, then choose verified backup OR move to the recovery holding area. A preview can never silently become a disk action.">
              <span>Optional disk actions (locked)</span>
              <small>preview + unlock + confirmation required</small>
            </summary>
            <div className="safe-manage-actions-body">
              <p className="mutation-warning" data-help="The staged sequence exists so a preview cannot silently become a disk action. Unlock issues a single short-lived token; backup copies + verifies without moving; move relocates into the local recovery area with a restore journal.">
                Order: <strong>Calculate</strong> a preview → <strong>Unlock</strong> one action → choose <strong>Backup</strong> (copy + verify, source untouched) or <strong>Move to recovery</strong> (relocate into the local recovery area, restorable later).
              </p>
              <div className="mutation-controls">
                <div className="backup-level-control" data-help="Backup level is a label saved in the backup manifest. Every level still copies the selected recoverable files, verifies each copy by hash, and writes a manifest.">
                  <span>Backup level</span>
                  <div className="backup-level-options" role="radiogroup" aria-label="Backup level">
                    <button type="button" className={mutationBackupLevel === "minimal" ? "active" : ""} data-help="Minimal: a narrow verified safety copy for this preview only." onClick={() => setMutationBackupLevel("minimal")}>
                      <strong>Minimal</strong>
                      <small>Small copy</small>
                    </button>
                    <button type="button" className={mutationBackupLevel === "standard" ? "active" : ""} data-help="Standard: recommended default. Copies the recoverable files, verifies source and copy hashes, writes an audit manifest." onClick={() => setMutationBackupLevel("standard")}>
                      <strong>Standard</strong>
                      <small>Recommended</small>
                    </button>
                    <button type="button" className={mutationBackupLevel === "full" ? "active" : ""} data-help="Full: same verified copy mechanics, but signals the broadest caution for the reviewed target." onClick={() => setMutationBackupLevel("full")}>
                      <strong>Full</strong>
                      <small>Broadest</small>
                    </button>
                  </div>
                </div>
                <label className="check-row" data-help="Same-disk means the backup destination is on the same physical drive as the files. Code Hangar refuses that by default — it doesn't protect you if that drive fails and it consumes the space you're trying to understand. Enable only for deliberate local test backups.">
                  <input type="checkbox" checked={mutationAllowSameVolume} onChange={(event) => setMutationAllowSameVolume(event.target.checked)} />
                  <span>Allow same-disk backup destination</span>
                </label>
                <p className="check-row-note" data-help="Removing a folder always empties it 100%: sensitive/protected files (.env, keys) are COPIED into the verified backup (secrets included) and then removed, and junction/symlink links are removed without being followed. The full backup keeps it reversible. Before anything is copied you get a per-project confirmation listing exactly which sensitive files and links are included.">
                  Removing a folder empties it fully — secrets are backed up first (then removed), links are removed (targets untouched), and you confirm exactly what's included first. The complete backup keeps it reversible.
                </p>
              </div>
              <div className="mutation-action-grid">
                <button type="button" className="action-button" disabled={!operationPlan || Boolean(mutationModeToken) || mutationBusy} data-help="Issue a short-lived confirmation token for the next verified backup or recovery action. It does not change files by itself." onClick={() => void enterMutationMode()}>
                  {mutationModeToken ? "One action unlocked" : "Unlock one action"}
                </button>
                <button type="button" className="secondary-button" disabled={!operationPlan || !mutationModeToken || mutationBusy} data-help="Choose a destination folder, copy recoverable files there, verify every copy by hash and record the manifest. Source files stay where they are." onClick={() => void runMutationBackup()}>
                  Verified backup…
                </button>
                <button type="button" className="secondary-button danger-outline" disabled={!operationPlan || !mutationModeToken || mutationBusy} data-help="Move concrete recoverable files into Code Hangar's local recovery holding area and record enough journal data to restore them. This changes source folders." onClick={() => void runMutationMove()}>
                  Move to recovery…
                </button>
              </div>
              <p className="mutation-footnote" data-help="Files held for recovery can be restored from Recover. Final removal requires a separate confirmation. Every disk action re-checks paths, file locks, protected locations and fingerprints before it starts.">
                Restores are handled in <strong>Recover</strong>. Every action re-checks paths, file locks, protected zones and fingerprints first.
              </p>
            </div>
          </details>
        ) : null}
        {mutationMessage ? <p className="mutation-message" data-help="Latest disk-action status message.">{mutationMessage}</p> : null}
      </div>
    </section>
  );
}

export function safeManageAdvancedPrompt(advancedMode: boolean) {
  if (advancedMode) return null;
  return {
    buttonLabel: "Show detailed evidence",
    note: "Counts stay compact. Detailed evidence expands only the categories with findings."
  };
}

export function reviewConfidenceLabel(summary: OperationPlan["confidenceSummary"]) {
  const levels = [`${summary.high} strong`, `${summary.medium} possible`, `${summary.low} weak`];
  if (summary.unknown > 0) levels.push(`${summary.unknown} unclear`);
  return levels.join(" · ");
}

/**
 * Merge display-only protection signals by path. The operation plan keeps its
 * original sensitive/protected arrays for all gate and accounting decisions; this
 * removes duplicate visual rows when one file matches more than one policy.
 */
export function sensitiveProtectedRows(
  sensitiveFiles: OperationPlan["sensitiveFiles"],
  protectedHits: OperationPlan["protectedHits"]
): PlanRow[] {
  const rows = new Map<string, PlanRow>();
  const addTag = (path: string, text: string) => {
    const key = path.replaceAll("\\", "/").toLocaleLowerCase();
    const row = rows.get(key) ?? { primary: path, segments: [] };
    const segments = row.segments ?? [];
    if (!segments.some((segment) => segment.text.toLocaleLowerCase() === text.toLocaleLowerCase())) {
      segments.push({ text, tone: "tag" });
    }
    row.segments = segments;
    rows.set(key, row);
  };
  for (const item of sensitiveFiles) addTag(item.path, "sensitive");
  for (const item of protectedHits) addTag(item.path, item.level);
  return [...rows.values()];
}
