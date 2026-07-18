import { memo, useState } from "react";
import { AlertTriangle, RotateCcw, Wand2, X } from "lucide-react";

import type { AiRewriteProposal, AiSuggestionApplyResult } from "../types";
import { AiUsageMeter } from "./AiUsageMeter";

export interface RewriteTarget {
  nodeId: number;
  label: string;
  snippet: string;
}

export const RewriteDialog = memo(function RewriteDialog({
  target,
  fileName,
  onClose,
  onRun,
  onApply,
  onUndo
}: {
  target: RewriteTarget;
  fileName: string;
  onClose: () => void;
  onRun: (instruction: string, level: string) => Promise<AiRewriteProposal>;
  onApply: (proposalId: string) => Promise<AiSuggestionApplyResult>;
  onUndo: (nodeId: number, sessionId: string) => Promise<void>;
}) {
  const [instruction, setInstruction] = useState("");
  const [level, setLevel] = useState("vibe");
  const [proposal, setProposal] = useState<AiRewriteProposal | null>(null);
  const [applied, setApplied] = useState<AiSuggestionApplyResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [undoing, setUndoing] = useState(false);

  const run = async () => {
    if (!instruction.trim()) return;
    setBusy(true);
    setError(null);
    setProposal(null);
    setApplied(null);
    setConfirmed(false);
    try {
      setProposal(await onRun(instruction.trim(), level));
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    if (!proposal || !confirmed) return;
    setBusy(true);
    setError(null);
    try {
      setApplied(await onApply(proposal.proposalId));
      setConfirmed(false);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const undo = async () => {
    if (!applied) return;
    setUndoing(true);
    setError(null);
    try {
      await onUndo(applied.nodeId, applied.sessionId);
      onClose();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      setUndoing(false);
    }
  };

  const unchanged = proposal !== null && proposal.original === proposal.replacement;
  const projectedInputTokens = Math.ceil((target.snippet.length + instruction.trim().length) / 4) + 350;
  const projectedOutputTokens = Math.min(4_096, Math.max(256, Math.ceil(target.snippet.length / 4) + 128));

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="Suggest one selected change">
      <div className="modal rewrite-dialog">
        <header className="rewrite-head">
          <strong><Wand2 size={16} /> Suggest a change to this selection</strong>
          <button className="icon-button" type="button" onClick={onClose} aria-label="Close">
            <X size={16} />
          </button>
        </header>

        <p className="rewrite-scope">One selected passage in <strong>{fileName}</strong>. Code Hangar will refuse a duplicate selection, a changed file, or a result that fails its local validity check.</p>

        <label className="rewrite-instruction">
          In plain language, what should change?
          <input
            value={instruction}
            onChange={(event) => setInstruction(event.target.value)}
            placeholder="For example: make this message friendlier, without changing what the button does"
            spellCheck
            autoComplete="off"
            maxLength={1000}
            disabled={busy || applied !== null}
          />
        </label>
        <div className="rewrite-controls">
          <select value={level} onChange={(event) => setLevel(event.target.value)} aria-label="Suggestion style" disabled={busy || applied !== null}>
            <option value="vibe">Plain and conservative</option>
            <option value="engineer">Technical and conservative</option>
          </select>
          <button type="button" className="primary-button" disabled={busy || !instruction.trim() || applied !== null} onClick={() => void run()}>
            {busy && !proposal ? "Preparing suggestion..." : proposal === null ? "Prepare suggestion" : "Prepare again"}
          </button>
        </div>
        <AiUsageMeter projectedInputTokens={projectedInputTokens} projectedOutputTokens={projectedOutputTokens} />

        {error ? <div className="warning"><AlertTriangle size={15} /> {error}</div> : null}

        {proposal && !applied ? (
          <>
            <section className="rewrite-summary" aria-label="Plain-language change summary">
              <h4>What would change</h4>
              <p>{proposal.summary}</p>
            </section>
            <div className="rewrite-diff">
              <div className="rewrite-col">
                <h4>Selected now</h4>
                <pre>{proposal.original}</pre>
              </div>
              <div className="rewrite-col">
                <h4>Proposed</h4>
                <pre>{proposal.replacement}</pre>
              </div>
            </div>
            {unchanged ? <div className="muted">The suggestion is identical. There is nothing to apply.</div> : null}
            <div className="warning rewrite-danger">
              <AlertTriangle size={15} />
              Only this uniquely matched selection will be replaced. A verified previous version is created automatically before the write.
            </div>
            <div className="rewrite-actions">
              <label className="rewrite-confirm">
                <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
                I reviewed the selected before and after text
              </label>
              <button type="button" className="primary-button" disabled={!confirmed || busy || unchanged} onClick={() => void apply()}>
                {busy ? "Applying one change..." : "Apply this one change"}
              </button>
            </div>
          </>
        ) : null}

        {applied ? (
          <section className="rewrite-applied" role="status">
            <strong>Change applied</strong>
            <p>{applied.message}</p>
            <div className="rewrite-actions">
              <button type="button" onClick={() => void undo()} disabled={undoing}>
                <RotateCcw size={15} /> {undoing ? "Undoing..." : "Undo this AI change"}
              </button>
              <button type="button" className="primary-button" onClick={onClose}>Done</button>
            </div>
          </section>
        ) : null}
      </div>
    </div>
  );
});
