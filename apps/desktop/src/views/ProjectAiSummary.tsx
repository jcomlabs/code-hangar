import { memo, useEffect, useState } from "react";
import { startAiTask, useAiTask } from "../aiTasks";
import { connectorApi } from "../connectorApi";
import type { AiExplainPreview, AiProviderMode, AiSendDisclosure } from "../types";
import { AiMarkdown } from "./AiMarkdown";
import { AiUsageMeter } from "./AiUsageMeter";

export const ProjectAiSummary = memo(function ProjectAiSummary({ projectId }: { projectId: number }) {
  const [configured, setConfigured] = useState(false);
  const [providerMode, setProviderMode] = useState<AiProviderMode>("off");
  const [providerModel, setProviderModel] = useState("");
  const [preview, setPreview] = useState<AiExplainPreview | null>(null);
  const [disclosure, setDisclosure] = useState<AiSendDisclosure | null>(null);
  const [disclosureBusy, setDisclosureBusy] = useState(false);
  const [disclosureError, setDisclosureError] = useState<string | null>(null);
  const [level, setLevel] = useState("vibe");
  const taskKey = `summary:${projectId}`;
  const task = useAiTask(taskKey);
  const loading = task?.status === "pending";

  useEffect(() => {
    let cancelled = false;
    setConfigured(false);
    setPreview(null);
    setDisclosure(null);
    setDisclosureError(null);
    void connectorApi.aiProviderGet()
      .then((config) => {
        if (cancelled) return;
        setConfigured(config.mode !== "off");
        setProviderMode(config.mode);
        setProviderModel(config.model);
        if (config.mode !== "off") {
          void connectorApi.aiSummarizeProjectPreview(projectId, level)
            .then((next) => { if (!cancelled) setPreview(next); })
            .catch(() => { if (!cancelled) setPreview(null); });
        }
      })
      .catch(() => {
        if (!cancelled) setConfigured(false);
      });
    return () => {
      cancelled = true;
    };
  }, [level, projectId]);

  if (!configured) return null;

  const blocked = (preview?.blocked.length ?? 0) > 0;

  const reviewRequest = async () => {
    if (!preview || blocked) return;
    setDisclosureBusy(true);
    setDisclosureError(null);
    try {
      setDisclosure(await connectorApi.aiSummarizeProjectDisclosure(projectId, level, providerModel));
    } catch (err) {
      setDisclosure(null);
      setDisclosureError(err instanceof Error ? err.message : String(err));
    } finally {
      setDisclosureBusy(false);
    }
  };

  const summarize = () => {
    if (!disclosure) return;
    const approvedModel = disclosure.model;
    startAiTask(taskKey, "summary", "Project summary", undefined, async () => {
      const summary = await connectorApi.aiSummarizeProject(projectId, level, approvedModel);
      return {
        result: summary.summary,
        meta: `${summary.model} · ~${summary.estimatedInputTokens} input tokens`
      };
    });
    setDisclosure(null);
  };

  return (
    <div className="project-summary-ai" data-help="Optional: send this project's local README/manifest context to your configured AI provider for a friendlier summary. Sensitive files and secrets are blocked before anything is sent; nothing leaves your machine until you click.">
      <div className="project-summary-ai-controls">
        <select value={level} onChange={(event) => setLevel(event.target.value)} disabled={loading} aria-label="Summary level">
          <option value="vibe">Plain language</option>
          <option value="engineer">Technical</option>
        </select>
        {disclosure ? (
          <>
            <button type="button" className="primary-button" disabled={loading} onClick={summarize}>
              {loading ? "Summarizing…" : disclosure.mode === "local" ? "Send to local model" : "Send to API"}
            </button>
            <button type="button" className="secondary-button" disabled={loading} onClick={() => setDisclosure(null)}>Cancel</button>
          </>
        ) : (
          <button type="button" className="action-button subtle" disabled={loading || disclosureBusy || !preview || blocked} onClick={() => void reviewRequest()}>
            {loading ? "Summarizing…" : disclosureBusy ? "Preparing exact request…" : task?.status === "done" ? "Review another AI summary" : "Review AI summary request"}
          </button>
        )}
      </div>
      {preview ? <AiUsageMeter projectedInputTokens={preview.estTokens} providerMode={providerMode} /> : <small className="muted">Preparing the local context estimate…</small>}
      {blocked ? <small className="warning-inline">Not sent. {preview?.blocked.join(" ")}</small> : null}
      {disclosureError ? <small className="warning-inline">{disclosureError}</small> : null}
      {disclosure ? (
        <section className="ai-send-disclosure" aria-label="Exact project summary request">
          <div className="ai-send-disclosure-head">
            <strong>{disclosure.method} {disclosure.url}</strong>
            <span>{disclosure.mode === "local" ? "This machine" : "External API"}</span>
          </div>
          <dl>
            <div><dt>Model</dt><dd>{disclosure.model}</dd></div>
            <div><dt>Transport</dt><dd>{disclosure.transport}</dd></div>
            <div><dt>Request</dt><dd>{disclosure.sendChars.toLocaleString()} chars · ~{disclosure.estTokens.toLocaleString()} tokens</dd></div>
          </dl>
          <pre>{disclosure.requestBody}</pre>
          <small className="muted">Review these exact request bytes, then use the separate send button above. Credentials are added inside Rust and never appear here.</small>
        </section>
      ) : null}
      {task?.status === "error" ? <small className="warning-inline">{task.error}</small> : null}
      {task?.status === "done" ? (
        <div className="project-summary-ai-result">
          <AiMarkdown text={task.result ?? ""} className="project-summary-ai-copy" />
          {task.meta ? <small className="muted">{task.meta}</small> : null}
        </div>
      ) : null}
    </div>
  );
});
