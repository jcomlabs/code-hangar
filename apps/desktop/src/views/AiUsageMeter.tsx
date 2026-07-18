import { memo, useCallback, useEffect, useState } from "react";
import { Gauge, RotateCcw } from "lucide-react";
import { AI_USAGE_CHANGED_EVENT, connectorApi as api } from "../connectorApi";
import type { AiProviderMode, AiUsageStatus } from "../types";

const CAP_OPTIONS = [10_000, 25_000, 50_000, 100_000, 250_000];

function tokenCount(value: number): string {
  if (value < 1_000) return value.toLocaleString();
  const precision = value < 10_000 ? 1 : 0;
  return `${(value / 1_000).toFixed(precision)}k`;
}

export const AiUsageMeter = memo(function AiUsageMeter({
  projectedInputTokens,
  projectedOutputTokens = 1_200,
  providerMode,
  editable = false
}: {
  projectedInputTokens?: number;
  projectedOutputTokens?: number;
  providerMode?: AiProviderMode;
  editable?: boolean;
}) {
  const projected = projectedInputTokens == null ? undefined : Math.max(0, Math.round(projectedInputTokens));
  const [status, setStatus] = useState<AiUsageStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    void api.aiUsageStatus(projected, projected == null ? undefined : projectedOutputTokens).then(setStatus).catch(() => setStatus(null));
  }, [projected, projectedOutputTokens]);

  useEffect(() => {
    refresh();
    window.addEventListener(AI_USAGE_CHANGED_EVENT, refresh);
    return () => window.removeEventListener(AI_USAGE_CHANGED_EVENT, refresh);
  }, [refresh]);

  if (!status) return null;

  const cap = status.softCapTokens;
  const percentage = cap == null ? 0 : Math.min(100, (status.estimatedTotalTokens / cap) * 100);
  const changeCap = async (value: string) => {
    setBusy(true);
    try {
      setStatus(await api.aiUsageSetSoftCap(value === "none" ? null : Number(value)));
    } finally {
      setBusy(false);
    }
  };
  const reset = async () => {
    setBusy(true);
    try {
      setStatus(await api.aiUsageReset());
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className={`ai-usage-meter ${status.overSoftCap || status.wouldExceedSoftCap ? "warn" : ""}`} aria-label="AI session usage">
      <div className="ai-usage-meter-head">
        <span><Gauge size={14} /> AI session</span>
        <strong>{tokenCount(status.estimatedTotalTokens)}{cap != null ? ` / ${tokenCount(cap)}` : " tokens"}</strong>
      </div>
      {cap != null ? <div className="ai-usage-track" aria-hidden="true"><span style={{ width: `${percentage}%` }} /></div> : null}
      <small>
        {status.requestCount.toLocaleString()} {status.requestCount === 1 ? "model call" : "model calls"} · ~{tokenCount(status.estimatedInputTokens)} in + ~{tokenCount(status.estimatedOutputTokens)} out
      </small>
      {projected != null ? <small>Next call: ~{tokenCount(projected)} input + up to {tokenCount(status.projectedOutputAllowance)} output tokens.</small> : null}
      {projected != null && status.wouldExceedSoftCap ? (
        <p><strong>Soft-cap warning.</strong> This operation could take the session to about {tokenCount(status.projectedTotalTokens)} tokens. It remains your choice to continue.</p>
      ) : null}
      <small className="muted">
        {providerMode === "local" ? "Local calls have no per-token API charge. " : ""}Token estimates are not a provider bill; prices vary by model. No prompt or answer is kept by this meter.
      </small>
      {editable ? (
        <div className="ai-usage-controls">
          <label>
            Soft cap
            <select disabled={busy} value={cap == null ? "none" : String(cap)} onChange={(event) => void changeCap(event.target.value)}>
              {CAP_OPTIONS.map((value) => <option value={value} key={value}>{tokenCount(value)} tokens</option>)}
              <option value="none">No warning cap</option>
            </select>
          </label>
          <button type="button" className="secondary-button" disabled={busy || status.requestCount === 0} onClick={() => void reset()}>
            <RotateCcw size={14} /> Reset session meter
          </button>
        </div>
      ) : null}
    </section>
  );
});
