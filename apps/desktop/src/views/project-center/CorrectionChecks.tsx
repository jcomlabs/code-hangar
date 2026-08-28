import { AlertTriangle, CheckCircle2, ChevronDown, ChevronUp, Play, RefreshCcw, RotateCcw, ShieldCheck, Trash2, XCircle } from "lucide-react";
import { memo, useCallback, useEffect, useState } from "react";

import { api } from "../../api";
import { ConceptHelp } from "../../BeginnerHelp";
import type { ControlledCheckRun, CorrectionStaticCheckReport, ProjectCheckDefinition } from "../../types";

function statusIcon(status: string) {
  if (status === "passed") return <CheckCircle2 className="check-pass" size={15} />;
  if (status === "failed" || status === "timed_out") return <XCircle className="check-fail" size={15} />;
  return <AlertTriangle className="check-warn" size={15} />;
}

function statusLabel(status: string) {
  switch (status) {
    case "passed": return "Passed";
    case "failed": return "Failed";
    case "timed_out": return "Timed out";
    case "not_applicable": return "Not available";
    default: return "Review";
  }
}

export const CorrectionChecks = memo(function CorrectionChecks({
  projectId,
  nodeId,
  busy = false,
  onFileMutated,
  setStatusText
}: {
  projectId: number;
  nodeId: number;
  busy?: boolean;
  onFileMutated: (nodeId: number) => void | Promise<void>;
  setStatusText: (value: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [definitions, setDefinitions] = useState<ProjectCheckDefinition[]>([]);
  const [staticReport, setStaticReport] = useState<CorrectionStaticCheckReport | null>(null);
  const [runResult, setRunResult] = useState<ControlledCheckRun | null>(null);
  const [loading, setLoading] = useState(true);
  const [runningId, setRunningId] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [approvalAcknowledged, setApprovalAcknowledged] = useState(false);
  const [confirmRunId, setConfirmRunId] = useState<string | null>(null);
  const [runAcknowledged, setRunAcknowledged] = useState(false);
  const [confirmRollback, setConfirmRollback] = useState(false);
  const [rollbackAcknowledged, setRollbackAcknowledged] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadDefinitions = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setDefinitions(await api.projectChecksDetect(projectId));
    } catch (cause) {
      setDefinitions([]);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    setStaticReport(null);
    setRunResult(null);
    setConfirmingId(null);
    setApprovalAcknowledged(false);
    setConfirmRunId(null);
    setRunAcknowledged(false);
    setConfirmRollback(false);
    setRollbackAcknowledged(false);
    void loadDefinitions();
  }, [loadDefinitions, nodeId]);

  async function runStatic() {
    setRunningId("static");
    setError(null);
    try {
      const report = await api.staticCorrectionCheck(nodeId);
      setStaticReport(report);
      setStatusText(report.status === "failed" ? "Static correction check failed." : "Static correction check finished without executing project code.");
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
    } finally {
      setRunningId(null);
    }
  }

  async function approve(definition: ProjectCheckDefinition) {
    if (!approvalAcknowledged) return;
    setRunningId(`approve:${definition.id}`);
    setError(null);
    try {
      const approved = await api.projectCheckApprove(projectId, definition.id, definition.fingerprint);
      setDefinitions((current) => current.map((item) => item.id === approved.id ? approved : item));
      setConfirmingId(null);
      setApprovalAcknowledged(false);
      setStatusText(`${approved.label} approved for this exact project manifest.`);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
      await loadDefinitions();
    } finally {
      setRunningId(null);
    }
  }

  async function revoke(definition: ProjectCheckDefinition) {
    setRunningId(`revoke:${definition.id}`);
    setError(null);
    try {
      await api.projectCheckRevoke(projectId, definition.id);
      setDefinitions((current) => current.map((item) => item.id === definition.id ? { ...item, approved: false, approvedAt: null } : item));
      setStatusText(`${definition.label} approval removed.`);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
    } finally {
      setRunningId(null);
    }
  }

  async function runProjectCheck(definition: ProjectCheckDefinition) {
    if (confirmRunId !== definition.id || !runAcknowledged) return;
    setRunningId(definition.id);
    setConfirmRunId(null);
    setRunAcknowledged(false);
    setRunResult(null);
    setConfirmRollback(false);
    setRollbackAcknowledged(false);
    setError(null);
    try {
      const result = await api.projectCheckRun(projectId, nodeId, definition.id, definition.fingerprint);
      setRunResult(result);
      setStatusText(`${definition.label}: ${statusLabel(result.status).toLowerCase()}.`);
      await loadDefinitions();
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
      await loadDefinitions();
    } finally {
      setRunningId(null);
    }
  }

  async function rollbackCorrection() {
    if (!runResult?.rollbackSnapshotId) return;
    setRunningId("rollback");
    setError(null);
    try {
      const restored = await api.editSnapshotRestore(runResult.rollbackSnapshotId);
      await onFileMutated(restored.nodeId);
      setConfirmRollback(false);
      setRollbackAcknowledged(false);
      setRunResult(null);
      setStaticReport(null);
      setStatusText(restored.message);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
    } finally {
      setRunningId(null);
    }
  }

  const running = runningId !== null;
  return (
    <section className="correction-checks" aria-label="Correction checks">
      <div className="inline-disclosure-heading">
        <button className="correction-checks-toggle" type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          <ShieldCheck size={15} />
          <span>Correction checks</span>
          <small>{staticReport ? statusLabel(staticReport.status) : definitions.length > 0 ? `${definitions.length} project check${definitions.length === 1 ? "" : "s"}` : loading ? "Detecting" : "Static only"}</small>
          {expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
        </button>
        <ConceptHelp concept="projectChecks" />
      </div>
      {expanded ? (
        <div className="correction-checks-body">
          {error ? <div className="correction-check-error" role="alert"><AlertTriangle size={15} /> {error}</div> : null}
          <div className="static-check-row">
            <div>
              <strong>Static analysis</strong>
              <span>Structural parse and local reference checks. Executes no project code.</span>
            </div>
            <button type="button" onClick={() => void runStatic()} disabled={busy || running}>
              {runningId === "static" ? <RefreshCcw className="spin" size={15} /> : <ShieldCheck size={15} />}
              {staticReport ? "Run again" : "Run static checks"}
            </button>
          </div>
          {staticReport ? (
            <div className="static-check-results" role="status">
              {staticReport.checks.map((check) => (
                <div key={check.id}>
                  {statusIcon(check.status)}
                  <p><strong>{check.label}</strong><span>{check.detail}</span></p>
                  <small>{statusLabel(check.status)}</small>
                </div>
              ))}
            </div>
          ) : null}

          {definitions.length > 0 ? <h3>Project code checks</h3> : null}
          {definitions.map((definition) => (
            <div className="project-check-row" key={definition.id}>
              <div className="project-check-main">
                <div>
                  <strong>{definition.label}</strong>
                  <span><code>{definition.commandLabel}</code> from {definition.manifestPath}</span>
                </div>
                <div className="project-check-actions">
                  {definition.approved ? (
                    <>
                      <button type="button" className="primary-button" onClick={() => { setConfirmRunId(definition.id); setRunAcknowledged(false); }} disabled={busy || running}>
                        {runningId === definition.id ? <RefreshCcw className="spin" size={15} /> : <Play size={15} />}
                        Review run
                      </button>
                      <button className="icon-button" type="button" aria-label={`Forget approval for ${definition.label}`} data-help="Remove this project's approval for the detected check." onClick={() => void revoke(definition)} disabled={busy || running}>
                        <Trash2 size={15} />
                      </button>
                    </>
                  ) : (
                    <button type="button" onClick={() => { setConfirmingId(definition.id); setApprovalAcknowledged(false); }} disabled={busy || running}>Review approval</button>
                  )}
                </div>
              </div>
              <small>{definition.timeoutSeconds}s · {definition.memoryLimitMib} MiB · {definition.processLimit} processes · approval {definition.approved ? "active" : "required"}</small>
              {definition.approved && confirmRunId === definition.id ? (
                <div className="project-check-approval project-check-run-confirm">
                  <div className="warning"><AlertTriangle size={15} /> This runs project code now. It may change files or create side effects outside the open file that Code Hangar cannot fully undo.</div>
                  <p>Command: <code>{definition.commandLabel}</code></p>
                  <label>
                    <input type="checkbox" checked={runAcknowledged} onChange={(event) => setRunAcknowledged(event.target.checked)} />
                    I checked this command and want to run this exact project check now
                  </label>
                  <div>
                    <button type="button" onClick={() => { setConfirmRunId(null); setRunAcknowledged(false); }} disabled={running}>Cancel</button>
                    <button type="button" className="primary-button" onClick={() => void runProjectCheck(definition)} disabled={!runAcknowledged || running}>
                      <Play size={15} /> Run once
                    </button>
                  </div>
                </div>
              ) : null}
              {confirmingId === definition.id ? (
                <div className="project-check-approval">
                  <div className="warning"><AlertTriangle size={15} /> {definition.riskDisclosure}</div>
                  <p>Code Hangar can restore the checked correction from its verified previous version. It cannot promise to undo files or side effects created by the project check itself.</p>
                  <label>
                    <input type="checkbox" checked={approvalAcknowledged} onChange={(event) => setApprovalAcknowledged(event.target.checked)} />
                    I understand this runs this project's code outside a sandbox
                  </label>
                  <div>
                    <button type="button" onClick={() => { setConfirmingId(null); setApprovalAcknowledged(false); }} disabled={running}>Cancel</button>
                    <button type="button" className="primary-button" onClick={() => void approve(definition)} disabled={!approvalAcknowledged || running}>
                      {runningId === `approve:${definition.id}` ? "Approving..." : "Approve this exact check"}
                    </button>
                  </div>
                </div>
              ) : null}
            </div>
          ))}

          {runResult ? (
            <section className={`controlled-check-result ${runResult.status}`} aria-label="Controlled check result" role="status">
              <header>{statusIcon(runResult.status)}<strong>{runResult.label}: {statusLabel(runResult.status)}</strong><span>{(runResult.durationMs / 1000).toFixed(1)}s</span></header>
              <p>{runResult.limitsSummary}</p>
              {runResult.outputTruncated ? <div className="warning"><AlertTriangle size={15} /> Output was capped; the process still ran to completion or timeout.</div> : null}
              {runResult.stdout || runResult.stderr ? (
                <details>
                  <summary>Local output</summary>
                  {runResult.stdout ? <pre>{runResult.stdout}</pre> : null}
                  {runResult.stderr ? <pre>{runResult.stderr}</pre> : null}
                </details>
              ) : null}
              {runResult.rollbackAvailable ? (
                confirmRollback ? (
                  <div className="rollback-confirm">
                    <span>Restore the correction's verified previous version?</span>
                    <label><input type="checkbox" checked={rollbackAcknowledged} onChange={(event) => setRollbackAcknowledged(event.target.checked)} disabled={running} /> I understand this replaces the current file</label>
                    <button type="button" onClick={() => { setConfirmRollback(false); setRollbackAcknowledged(false); }} disabled={running}>Cancel</button>
                    <button type="button" className="primary-button" onClick={() => void rollbackCorrection()} disabled={running || !rollbackAcknowledged}>
                      {runningId === "rollback" ? "Restoring..." : "Restore correction"}
                    </button>
                  </div>
                ) : (
                  <button type="button" onClick={() => { setConfirmRollback(true); setRollbackAcknowledged(false); }} disabled={running}>
                    <RotateCcw size={15} /> Restore checked correction
                  </button>
                )
              ) : <small>No verified correction version is available to restore for this file.</small>}
            </section>
          ) : null}
        </div>
      ) : null}
    </section>
  );
});
