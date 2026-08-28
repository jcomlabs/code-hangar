import { ListChecks, Sparkles, Wand2 } from "lucide-react";
import { memo, useCallback, useEffect, useState } from "react";

import type { ContextMenuItem } from "../ContextMenu";
import { confirmExactAiSend, connectorApi, formatAiCredentialUse } from "../connectorApi";
import type {
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationStatus,
  AiSafeManageAdvisoryReceipt,
  AiSafeManageAdvisoryResult,
  AiSafeManageContextCandidate,
  AiSendDisclosure
} from "../connectorTypes";
import type {
  ProjectSummary,
  SafeManageConfidence,
  SafeManageProjectAssessment,
  SafeManageRecommendation
} from "../types";
import { AiAssistKeyCard, AiExplainPanel, type AiExplainTarget } from "./AiAssist";
import { AiUsageMeter } from "./AiUsageMeter";
import { RecapAiLayer } from "./RecapAiLayer";
import { RewriteDialog, type RewriteTarget } from "./RewriteDialog";
import { SettingsAutomationView, SettingsConnectedAppsView } from "./ConnectorSettingsViews";
import "./ConnectorEditionLayer.css";

export { RecapAiLayer as ConnectorRecapDetailLayer };

const SAFE_MANAGE_RECOMMENDATION_LABEL: Record<SafeManageRecommendation, string> = {
  keep: "Keep",
  review: "Review",
  archive: "Archive",
  clean_regenerables: "Clean regenerables",
  removal_candidate: "Removal candidate",
  do_not_touch: "Do not touch"
};

const SAFE_MANAGE_CONFIDENCE_LABEL: Record<SafeManageConfidence, string> = {
  high: "High",
  medium: "Medium",
  low: "Low",
  unknown: "Unknown"
};

export interface ConnectorSelectionContext {
  nodeId: number;
  projectId: number;
  path: string;
  snippet: string;
  safeToOffer: boolean;
}

export interface ConnectorEditionBridge {
  selectedTextItems: (context: ConnectorSelectionContext) => ContextMenuItem[];
  resetConsequence: () => string;
}

type Confirm = (
  message: string,
  options?: { confirmLabel?: string; tone?: "primary" | "danger" }
) => Promise<boolean>;

export const ConnectorSettingsPanel = memo(function ConnectorSettingsPanel({
  projects,
  currentFile,
  confirm,
  onCopy,
  onStatus
}: {
  projects: ProjectSummary[];
  currentFile: { nodeId: number; displayName: string } | null;
  confirm: Confirm;
  onCopy: (value: string) => void;
  onStatus: (value: string) => void;
}) {
  const [status, setStatus] = useState<AutomationStatus | null>(null);
  const [agents, setAgents] = useState<AutomationAgentSummary[]>([]);
  const [activity, setActivity] = useState<AutomationActivityEntry[]>([]);
  const [credential, setCredential] = useState<AutomationCredential | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const nextStatus = await connectorApi.automationStatus();
      setStatus(nextStatus);
      if (nextStatus.enabled) {
        const [nextAgents, nextActivity] = await Promise.all([
          connectorApi.automationAgents(),
          connectorApi.automationActivity(100)
        ]);
        setAgents(nextAgents);
        setActivity(nextActivity);
      } else {
        setAgents([]);
        setActivity([]);
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const register = async (name: string, scopes: string[], projectIds: number[]) => {
    setBusy(true);
    setError(null);
    try {
      const nextCredential = await connectorApi.automationRegister(name, scopes, projectIds);
      setCredential(nextCredential);
      onStatus(`Registered local tool ${nextCredential.agent.name}. Store its token now; it is shown once.`);
      const [nextAgents, nextActivity] = await Promise.all([
        connectorApi.automationAgents(),
        connectorApi.automationActivity(100)
      ]);
      setAgents(nextAgents);
      setActivity(nextActivity);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (agentId: number) => {
    if (!(await confirm(
      "Revoke this local credential and all of its temporary file grants?",
      { confirmLabel: "Revoke credential", tone: "danger" }
    ))) return;
    setBusy(true);
    setError(null);
    try {
      await connectorApi.automationRevoke(agentId);
      setCredential((current) => current?.agent.id === agentId ? null : current);
      onStatus("Local credential revoked.");
      await refresh();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const forget = async (agentId: number) => {
    setBusy(true);
    setError(null);
    try {
      await connectorApi.automationForgetRevoked(agentId);
      onStatus("Revoked local credential entry removed. Activity records remain.");
      await refresh();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const grantRead = async (agentId: number, nodeId: number) => {
    setBusy(true);
    setError(null);
    try {
      await connectorApi.automationGrantRead(agentId, nodeId, 10);
      onStatus("Temporary file access granted for 10 minutes. Protected policy still applies.");
      setActivity(await connectorApi.automationActivity(100));
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <AiAssistKeyCard />
      <SettingsAutomationView
        status={status}
        agents={agents}
        activity={activity}
        credential={credential}
        projects={projects}
        currentFile={currentFile}
        busy={busy}
        error={error}
        onRefresh={() => void refresh()}
        onRegister={(name, scopes, projectIds) => void register(name, scopes, projectIds)}
        onRevoke={(agentId) => void revoke(agentId)}
        onForget={(agentId) => void forget(agentId)}
        onGrantRead={(agentId, nodeId) => void grantRead(agentId, nodeId)}
        onCopy={onCopy}
        onClearCredential={() => setCredential(null)}
      />
      <SettingsConnectedAppsView confirm={(message) => confirm(message)} projects={projects} />
    </>
  );
});

export const ConnectorSafeManageRecommendation = memo(function ConnectorSafeManageRecommendation({
  assessment
}: {
  assessment: SafeManageProjectAssessment;
}) {
  const [candidates, setCandidates] = useState<AiSafeManageContextCandidate[] | null>(null);
  const [selectedContextIds, setSelectedContextIds] = useState<string[]>([]);
  const [disclosure, setDisclosure] = useState<AiSendDisclosure | null>(null);
  const [result, setResult] = useState<AiSafeManageAdvisoryResult | null>(null);
  const [receipts, setReceipts] = useState<AiSafeManageAdvisoryReceipt[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const enrichable = !assessment.evidenceStale && assessment.recommendation !== "do_not_touch";

  useEffect(() => {
    setCandidates(null);
    setSelectedContextIds([]);
    setDisclosure(null);
    setResult(null);
    setReceipts([]);
    setError(null);
  }, [assessment.analysisRunId, assessment.evidenceRevision, assessment.projectId]);

  if (!enrichable) {
    return (
      <div className="connector-safe-manage-advisory connector-safe-manage-advisory-blocked">
        <strong>AI enrichment is not allowed to cross this safety floor.</strong>
        <small>The deterministic Do not touch result or stale evidence must be resolved locally before any optional AI recommendation.</small>
      </div>
    );
  }

  const loadCandidates = async () => {
    setBusy(true);
    setError(null);
    setDisclosure(null);
    setResult(null);
    try {
      const [nextCandidates, priorReceipts] = await Promise.all([
        connectorApi.aiSafeManageContextCandidates(
          assessment.projectId,
          assessment.analysisRunId,
          assessment.evidenceRevision
        ),
        connectorApi.aiSafeManageAdvisoryReceipts(assessment.projectId, 8)
      ]);
      setCandidates(nextCandidates);
      setSelectedContextIds([]);
      setReceipts(priorReceipts);
      if (nextCandidates.length === 0) {
        setError("No safe README, manifest, central file or linked session is available for explicit selection.");
      }
    } catch (caught) {
      setCandidates(null);
      setSelectedContextIds([]);
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const toggleContext = (selectionId: string, checked: boolean) => {
    setDisclosure(null);
    setResult(null);
    setSelectedContextIds((current) => {
      if (!checked) return current.filter((id) => id !== selectionId);
      if (current.includes(selectionId) || current.length >= 6) return current;
      return [...current, selectionId];
    });
  };

  const prepare = async () => {
    if (selectedContextIds.length === 0) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setDisclosure(await connectorApi.aiSafeManageAdvisoryDisclosure(
        assessment.projectId,
        assessment.analysisRunId,
        assessment.evidenceRevision,
        selectedContextIds,
        ""
      ));
    } catch (caught) {
      setDisclosure(null);
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const send = async () => {
    if (!disclosure) return;
    const reviewed = disclosure;
    // A send preview is one-shot even when a provider fails. Remove it from the UI before IPC so
    // an error cannot tempt the user to replay bytes that the backend has already consumed.
    setDisclosure(null);
    setBusy(true);
    setError(null);
    try {
      const nextResult = await connectorApi.aiSafeManageAdvisory(
        assessment.projectId,
        assessment.analysisRunId,
        assessment.evidenceRevision,
        reviewed.model,
        reviewed.previewId
      );
      setResult(nextResult);
      setReceipts((current) => [
        nextResult.receipt,
        ...current.filter((receipt) => receipt.receiptId !== nextResult.receipt.receiptId)
      ].slice(0, 8));
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="connector-safe-manage-advisory">
      <button type="button" className="secondary-button" disabled={busy} onClick={() => void loadCandidates()}>
        <Sparkles size={14} /> {busy ? "Loading safe context choices…" : "Enrich this recommendation with AI"}
      </button>
      <small>Experimental and less tested. Nothing is selected by default. The AI may agree with or materially change the displayed recommendation using only the exact redacted excerpts you select. It cannot record your decision or run a disk action.</small>
      {error ? <p className="connector-safe-manage-advisory-error" role="alert">{error}</p> : null}
      {candidates ? (
        <fieldset className="connector-safe-manage-contexts">
          <legend>Evidence for the optional AI recommendation</legend>
          <p>Select 1–6 items. Paths are never sent or stored in the receipt; files are re-read and gated only when you prepare the exact preview.</p>
          {candidates.map((candidate) => {
            const checked = selectedContextIds.includes(candidate.selectionId);
            return (
              <label key={candidate.selectionId}>
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={!checked && selectedContextIds.length >= 6}
                  onChange={(event) => toggleContext(candidate.selectionId, event.currentTarget.checked)}
                />
                <span>
                  <strong>{candidate.label}</strong>
                  <small>{candidate.detail} Up to {candidate.maxExcerptChars.toLocaleString()} characters.</small>
                </span>
              </label>
            );
          })}
          <div className="connector-safe-manage-context-actions">
            <span>{selectedContextIds.length} selected</span>
            <button
              type="button"
              className="secondary-button"
              disabled={busy || selectedContextIds.length === 0}
              onClick={() => void prepare()}
            >
              Prepare exact redacted payload
            </button>
          </div>
        </fieldset>
      ) : null}
      {disclosure ? (
        <section className="ai-send-disclosure connector-safe-manage-disclosure" aria-label="Exact AI recommendation request">
          <div className="ai-send-disclosure-head">
            <span>Review the frozen destination and exact redacted body. Nothing has been sent.</span>
            <strong>{disclosure.method} {disclosure.url}</strong>
            <span>{disclosure.model} · {disclosure.transport} · expires {new Date(disclosure.expiresAtUnix * 1000).toLocaleTimeString()}</span>
            <span>Credential: {formatAiCredentialUse(disclosure.credentialUse)}</span>
            {disclosure.receiptId ? <span>Receipt prepared: {disclosure.receiptId}</span> : null}
          </div>
          <pre>{disclosure.requestBody}</pre>
          {disclosure.fallbackRequestBody ? (
            <details><summary>Possible disclosed fallback body</summary><pre>{disclosure.fallbackRequestBody}</pre></details>
          ) : null}
          <AiUsageMeter projectedInputTokens={disclosure.estTokens} projectedOutputTokens={1_000} />
          <button type="button" className="primary-button" disabled={busy} onClick={() => void send()}>
            Send this exact recommendation request
          </button>
        </section>
      ) : null}
      {result ? (
        <section className="connector-safe-manage-advisory-result" aria-label="AI-enriched recommendation result">
          <div className="connector-safe-manage-recommendation-compare">
            <div>
              <span>Deterministic baseline</span>
              <strong>{SAFE_MANAGE_RECOMMENDATION_LABEL[result.deterministicRecommendation]}</strong>
              <small>{SAFE_MANAGE_CONFIDENCE_LABEL[result.deterministicConfidence]} deterministic confidence</small>
            </div>
            <span className="connector-safe-manage-recommendation-arrow" aria-hidden="true">→</span>
            <div className={result.aiRecommendation ? "parsed" : "unparsed"}>
              <span>AI recommendation</span>
              <strong>{result.aiRecommendation
                ? SAFE_MANAGE_RECOMMENDATION_LABEL[result.aiRecommendation]
                : "No valid typed result"}</strong>
              <small>{result.aiConfidence
                ? `${SAFE_MANAGE_CONFIDENCE_LABEL[result.aiConfidence]} AI confidence`
                : "Provider prose did not satisfy the typed contract"}</small>
            </div>
          </div>
          {result.aiRecommendation ? (
            <p className={`connector-safe-manage-recommendation-status ${result.recommendationChanged ? "changed" : "agrees"}`}>
              {result.recommendationChanged
                ? "The AI changed the recommendation for your review."
                : "The AI agrees with the deterministic recommendation."}
            </p>
          ) : (
            <p className="connector-safe-manage-recommendation-status invalid">
              The response remains readable below, but Code Hangar will not infer an action from malformed output.
            </p>
          )}
          <pre>{result.advisory}</pre>
          <p className="connector-safe-manage-advisory-boundary">
            This may change the recommendation displayed in Connector for review. It does not record your decision, build an OperationPlan, authorize a disk action or weaken any deterministic safety blocker.
          </p>
          <dl className="connector-safe-manage-receipt">
            <div><dt>Receipt</dt><dd>{result.receipt.receiptId}</dd></div>
            <div><dt>Status</dt><dd>{result.receipt.status}</dd></div>
            <div><dt>Request fingerprint</dt><dd>{result.receipt.requestHash}</dd></div>
            <div><dt>Result fingerprint</dt><dd>{result.receipt.resultHash ?? "Not recorded"}</dd></div>
            <div><dt>Selected sources</dt><dd>{result.receipt.sources.length}</dd></div>
          </dl>
        </section>
      ) : null}
      {receipts.length > 0 ? (
        <details className="connector-safe-manage-receipts">
          <summary>Recent fingerprint-only advisory receipts ({receipts.length})</summary>
          <ul>
            {receipts.map((receipt) => (
              <li key={receipt.receiptId}>
                <code>{receipt.receiptId}</code>
                <span>{receipt.status} · {receipt.sources.length} selected source{receipt.sources.length === 1 ? "" : "s"} · {new Date(receipt.createdAt).toLocaleString()}</span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </div>
  );
});

export const ConnectorEditionLayer = memo(function ConnectorEditionLayer({
  selectedProjectId,
  changesUnlocked,
  onRequestChangeAccess,
  onStatus,
  onRefreshNode,
  confirm,
  onBridge,
  onOverlayChange
}: {
  selectedProjectId: number | null;
  changesUnlocked: boolean;
  onRequestChangeAccess: () => void;
  onStatus: (value: string) => void;
  onRefreshNode: (nodeId: number, projectId: number) => void | Promise<void>;
  confirm: Confirm;
  onBridge: (bridge: ConnectorEditionBridge | null) => void;
  onOverlayChange: (open: boolean) => void;
}) {
  const [explainTarget, setExplainTarget] = useState<AiExplainTarget | null>(null);
  const [explainPos, setExplainPos] = useState({ x: 140, y: 96 });
  const [explainDocked, setExplainDocked] = useState(true);
  const [rewriteTarget, setRewriteTarget] = useState<RewriteTarget | null>(null);
  const [rewriteFileName, setRewriteFileName] = useState("");

  useEffect(() => {
    setRewriteTarget(null);
    setExplainTarget(null);
  }, [selectedProjectId]);

  useEffect(() => {
    onOverlayChange(explainTarget !== null || rewriteTarget !== null);
    return () => onOverlayChange(false);
  }, [explainTarget, onOverlayChange, rewriteTarget]);

  const selectedTextItems = useCallback((context: ConnectorSelectionContext): ContextMenuItem[] => {
    if (!context.safeToOffer || context.snippet.length === 0) return [];
    return [{
      id: "explain-selection",
      label: "Explain selected text with AI",
      section: "Experimental AI Assist",
      help: "Code Hangar blocks sensitive paths and secrets, then shows the exact provider request before sending.",
      icon: <Sparkles size={15} />,
      onSelect: () => setExplainTarget({
        kind: "text",
        nodeId: context.nodeId,
        snippet: context.snippet,
        label: "Selected text",
        initialLens: "explain"
      })
    }, {
      id: "review-selection",
      label: "Check selected text with AI",
      help: "Ask for read-only review questions. The AI cannot edit or execute anything.",
      icon: <ListChecks size={15} />,
      onSelect: () => setExplainTarget({
        kind: "text",
        nodeId: context.nodeId,
        snippet: context.snippet,
        label: "Selected text",
        initialLens: "review"
      })
    }, {
      id: "rewrite-selection",
      label: "Suggest one selected change",
      help: "The provider can only suggest replacement text. Applying it remains a separate local review step.",
      icon: <Wand2 size={15} />,
      onSelect: () => {
        if (!changesUnlocked) {
          onRequestChangeAccess();
          onStatus("Project changes are locked. Unlock them before reviewing an AI suggestion.");
          return;
        }
        setRewriteTarget({
          nodeId: context.nodeId,
          projectId: context.projectId,
          label: context.path,
          snippet: context.snippet
        });
        setRewriteFileName(context.path.split(/[\\/]/).pop() ?? context.path);
      }
    }];
  }, [changesUnlocked, onRequestChangeAccess, onStatus]);

  const resetConsequence = useCallback(() => (
    "Every connected AI app will stop authenticating after the restart because the local app registry and credential hashes are cleared. Reconnect each app from Code Hangar; its external config may retain an unusable Code Hangar entry until that reconciliation succeeds."
  ), []);

  useEffect(() => {
    onBridge({ selectedTextItems, resetConsequence });
    return () => onBridge(null);
  }, [onBridge, resetConsequence, selectedTextItems]);

  const runRewrite = async (instruction: string, level: string) => {
    if (!rewriteTarget) throw new Error("No selected passage is staged.");
    const disclosure = await connectorApi.aiRewriteDisclosure(
      rewriteTarget.nodeId,
      rewriteTarget.snippet,
      instruction,
      level,
      ""
    );
    if (!confirmExactAiSend(disclosure)) throw new Error("Cancelled. Nothing was sent.");
    return connectorApi.aiRewriteText(
      rewriteTarget.nodeId,
      rewriteTarget.snippet,
      instruction,
      level,
      disclosure.model,
      disclosure.previewId
    );
  };

  const applyRewrite = async (proposalId: string) => {
    if (!rewriteTarget) throw new Error("No selected passage is staged.");
    if (!changesUnlocked) throw new Error("Project changes are locked. Unlock this project and review the suggestion again.");
    const result = await connectorApi.applyAiSuggestion(proposalId);
    await onRefreshNode(result.nodeId, rewriteTarget.projectId);
    onStatus(result.message);
    return result;
  };

  const undoRewrite = async (nodeId: number, sessionId: string) => {
    if (!rewriteTarget) throw new Error("No selected passage is staged.");
    if (!changesUnlocked) throw new Error("Project changes are locked. Unlock this project before restoring an AI edit session.");
    if (!(await confirm(
      "Undo this AI edit session? Code Hangar restores the verified version from before that session only after rechecking the current file.",
      { confirmLabel: "Undo AI edit session", tone: "danger" }
    ))) throw new Error("Undo cancelled. No file changed.");
    const result = await connectorApi.undoAiEditSession(nodeId, sessionId);
    await onRefreshNode(result.nodeId, rewriteTarget.projectId);
    onStatus(result.message);
  };

  return (
    <>
      {explainTarget ? (
        <AiExplainPanel
          target={explainTarget}
          docked={explainDocked}
          edge={explainDocked}
          pos={explainPos}
          onToggleDock={() => setExplainDocked((value) => !value)}
          onClose={() => setExplainTarget(null)}
          onPosChange={setExplainPos}
        />
      ) : null}
      {rewriteTarget ? (
        <RewriteDialog
          target={rewriteTarget}
          fileName={rewriteFileName}
          onClose={() => setRewriteTarget(null)}
          onRun={runRewrite}
          onApply={applyRewrite}
          onUndo={undoRewrite}
        />
      ) : null}
    </>
  );
});
