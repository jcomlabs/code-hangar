import { memo, useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { AlertTriangle, CheckCircle2, ChevronLeft, ChevronRight, ExternalLink, Info, ListChecks, Loader2, PanelRight, Search, Server, Sparkles, X } from "lucide-react";
import { connectorApi as api } from "../connectorApi";
import { clearAiTask, hashSnippet, startAiStreamingTask, useAiTask } from "../aiTasks";
import type { AiExplainPreview, AiLocalProviderCandidate, AiProviderConfig, AiProviderFormat, AiProviderMode, AiSendDisclosure } from "../types";
import { HelpPopover } from "../ui";
import { AiLearningTools } from "./AiLearningTools";
import { AiMarkdown } from "./AiMarkdown";
import { AiUsageMeter } from "./AiUsageMeter";

type Preset = {
  label: string;
  mode: AiProviderMode;
  baseUrl: string;
  format: AiProviderFormat;
  model?: string;
  hint: string;
};

// Quick-fill presets. These are DATA, not defaults: the stored config starts as mode "off", and
// no provider is privileged — local model servers and API providers are offered side by side as
// examples. The user can also type any Chat Completions– or Messages-API–compatible endpoint by hand.
const PRESETS: Preset[] = [
  { label: "Ollama", mode: "local", baseUrl: "http://localhost:11434/v1", format: "chat_completions", hint: "Local · no key" },
  { label: "LM Studio", mode: "local", baseUrl: "http://localhost:1234/v1", format: "chat_completions", hint: "Local · no key" },
  { label: "vLLM", mode: "local", baseUrl: "http://localhost:8000/v1", format: "chat_completions", hint: "Local · no key" },
  { label: "OpenRouter", mode: "api", baseUrl: "https://openrouter.ai/api/v1", format: "chat_completions", hint: "API · your key · gateway to many models" },
  { label: "OpenAI · GPT-5.6", mode: "api", baseUrl: "https://api.openai.com/v1", format: "chat_completions", model: "gpt-5.6", hint: "GPT-5.6 Sol alias · your own OpenAI API key (a ChatGPT subscription won't work)" },
  { label: "Anthropic", mode: "api", baseUrl: "https://api.anthropic.com", format: "messages_api", hint: "API · your own key (a Claude subscription won't work)" }
];

const EMPTY_CONFIG: AiProviderConfig = { mode: "off", baseUrl: "", model: "", format: "chat_completions" };

/** Best-effort host[:port] for the "where this goes" disclosure. */
function hostLabel(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return "";
  try {
    const url = new URL(trimmed.includes("://") ? trimmed : `http://${trimmed}`);
    return url.host || trimmed;
  } catch {
    return trimmed;
  }
}

/** Plain-language summary of where a send would go — never a provider brand, always the host. */
function providerSummary(config: AiProviderConfig): string {
  if (config.mode === "off") return "AI Assist is off";
  const where = hostLabel(config.baseUrl) || "an unconfigured endpoint";
  return config.mode === "local" ? `your local model at ${where}` : `your API provider at ${where}`;
}

export type AiReviewSection = {
  label: "be-careful" | "double-check" | "heads-up" | "what-looks-deliberate" | "unknowns";
  items: string[];
};

const REVIEW_SECTION_ORDER: AiReviewSection["label"][] = [
  "be-careful",
  "double-check",
  "heads-up",
  "what-looks-deliberate",
  "unknowns"
];

/** Parse the deliberately small review schema. Unknown model output stays plain text. */
export function parseAiReviewSections(text: string): AiReviewSection[] {
  const sections = new Map<AiReviewSection["label"], string[]>();
  let current: AiReviewSection["label"] | null = null;
  for (const raw of text.replaceAll("\r\n", "\n").split("\n")) {
    const line = raw.trim();
    const heading = line
      .replace(/^#{1,6}\s*/, "")
      .replace(/^\*\*(.+)\*\*:?$/, "$1")
      .replace(/^\[|\]$/g, "")
      .replace(/:$/, "")
      .toLowerCase()
      .replaceAll(" ", "-");
    if (REVIEW_SECTION_ORDER.includes(heading as AiReviewSection["label"])) {
      current = heading as AiReviewSection["label"];
      if (!sections.has(current)) sections.set(current, []);
      continue;
    }
    if (!current || !line) continue;
    const clean = line.replace(/^[-*•]\s*/, "").trim();
    if (!clean) continue;
    const items = sections.get(current) ?? [];
    if (/^[-*•]\s/.test(line) || items.length === 0) items.push(clean);
    else items[items.length - 1] = `${items[items.length - 1]} ${clean}`;
    sections.set(current, items);
  }
  return REVIEW_SECTION_ORDER
    .map((label) => ({ label, items: sections.get(label) ?? [] }))
    .filter((section) => section.items.length > 0);
}

function AiReviewResult({ text }: { text: string }) {
  const sections = parseAiReviewSections(text);
  if (sections.length === 0) {
    return (
      <div className="ai-review-fallback">
        <small className="muted">This model returned plain text instead of the review structure. Nothing was discarded.</small>
        <div className="ai-explanation">{text}</div>
      </div>
    );
  }
  const labels: Record<AiReviewSection["label"], string> = {
    "be-careful": "Be careful",
    "double-check": "Double-check",
    "heads-up": "Heads-up",
    "what-looks-deliberate": "What looks deliberate",
    unknowns: "Unknowns"
  };
  return (
    <div className="ai-review-sections">
      {sections.map((section) => (
        <section className={`ai-review-section ${section.label}`} key={section.label}>
          <h4>
            {section.label === "be-careful" ? <AlertTriangle size={15} /> : section.label === "what-looks-deliberate" ? <CheckCircle2 size={15} /> : <Info size={15} />}
            {labels[section.label]}
          </h4>
          <ul>{section.items.map((item, index) => <li key={`${section.label}-${index}`}>{item}</li>)}</ul>
        </section>
      ))}
    </div>
  );
}

/**
 * Where an "Explain this" request points: a file already in the inventory (resolved + gated by
 * node id) or a free-text selection the user made in the preview pane (gated on the exact bytes).
 */
type AiLens = "explain" | "review";

export type AiExplainTarget =
  | { kind: "file"; nodeId: number; path: string; initialLens?: AiLens }
  | { kind: "text"; nodeId: number; snippet: string; label: string; initialLens?: AiLens };

function explainKey(target: AiExplainTarget, lens: AiLens): string {
  return target.kind === "file"
    ? `${lens}:file:${target.nodeId}`
    : `${lens}:text:${target.nodeId}:${hashSnippet(target.snippet)}`;
}

/**
 * "Explain this" panel (connector edition only). Non-blocking: it lives docked in the right column
 * (before the comments) or popped out into a draggable floating window, never a modal — so the user
 * can keep working while it thinks. The send runs in the shared aiTasks store, so navigating away
 * does not cancel it and the result stays put when you return. The send-gate runs server-side on
 * the exact bytes (sensitive paths / secrets hard-blocked); this only assembles the request.
 */
export const AiExplainPanel = memo(function AiExplainPanel({
  target,
  docked,
  edge = false,
  collapsed = false,
  onToggleCollapse,
  pos,
  onToggleDock,
  onClose,
  onPosChange
}: {
  target: AiExplainTarget;
  docked: boolean;
  /** Docked to the window's right edge as a small card — used when the current
      view has no Inspector column to host the in-flow docked panel. */
  edge?: boolean;
  /** Edge card collapsed to a thin strip (reclaims its reserved space). */
  collapsed?: boolean;
  onToggleCollapse?: () => void;
  pos: { x: number; y: number };
  onToggleDock: () => void;
  onClose: () => void;
  onPosChange: (pos: { x: number; y: number }) => void;
}) {
  // "fixed" = not draggable (either in-flow docked, or pinned to the right edge).
  const fixed = docked || edge;
  const [lens, setLens] = useState<AiLens>(target.initialLens ?? "explain");
  const key = explainKey(target, lens);
  const task = useAiTask(key);
  const title = target.kind === "file" ? (target.path.split(/[\\/]/).pop() ?? target.path) : target.label;
  const subtitle = target.kind === "file" ? target.path : "Selected code";

  const [preview, setPreview] = useState<AiExplainPreview | null>(null);
  const [config, setConfig] = useState<AiProviderConfig | null>(null);
  const [hasKey, setHasKey] = useState<boolean | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [level, setLevel] = useState<"vibe" | "engineer">("vibe");
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [disclosure, setDisclosure] = useState<AiSendDisclosure | null>(null);
  const [disclosureBusy, setDisclosureBusy] = useState(false);
  const [disclosureError, setDisclosureError] = useState<string | null>(null);

  useEffect(() => {
    setLens(target.initialLens ?? "explain");
  }, [target]);

  // Load the compose context (provider config, key status, and for a file the gate preview/cost).
  // Re-runs when the target changes. The send itself is detached (store), so this only feeds the
  // pre-send controls; a finished task renders straight from the store without needing any of it.
  // All three loads are local backend reads — the model-list request, an OUTBOUND call to the
  // provider, is deliberately NOT here: the network is only touched on explicit user action.
  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setError(null);
    const loads: [Promise<AiProviderConfig>, Promise<boolean>, Promise<AiExplainPreview | null>] = [
      api.aiProviderGet(),
      api.aiKeyStatus(),
      target.kind === "file" ? api.aiExplainPreview(target.nodeId) : Promise.resolve(null)
    ];
    void Promise.all(loads)
      .then(([c, k, p]) => {
        if (cancelled) return;
        setConfig(c);
        setHasKey(k);
        setModel(c.model);
        if (p) setPreview(p);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
      });
    return () => { cancelled = true; };
  }, [key, target]);

  // A disclosure is tied to the exact target + controls that produced it. Changing any of those
  // clears stale bytes instead of leaving a plausible-looking but incorrect request on screen.
  useEffect(() => {
    setDisclosure(null);
    setDisclosureError(null);
  }, [key, lens, level, model]);

  // Explicit, user-initiated model-list fetch from the configured endpoint.
  const refreshModels = () => {
    if (!config || config.mode === "off") return;
    api.aiProviderModels(config.mode, config.baseUrl, config.model, config.format)
      .then((list) => setModels([...new Set(list)]))
      .catch(() => { /* model list is best-effort; the field stays free-text */ });
  };

  // Drag the floating window by its header (ignored while docked, and when grabbing a header button).
  const dragOffset = useRef<{ dx: number; dy: number } | null>(null);
  const onHeaderMouseDown = (event: ReactMouseEvent<HTMLElement>) => {
    if (fixed) return;
    if ((event.target as HTMLElement).closest("button")) return;
    dragOffset.current = { dx: event.clientX - pos.x, dy: event.clientY - pos.y };
    event.preventDefault();
  };
  useEffect(() => {
    if (fixed) return;
    const onMove = (event: globalThis.MouseEvent) => {
      if (!dragOffset.current) return;
      const x = Math.max(8, Math.min(window.innerWidth - 340, event.clientX - dragOffset.current.dx));
      const y = Math.max(8, Math.min(window.innerHeight - 90, event.clientY - dragOffset.current.dy));
      onPosChange({ x, y });
    };
    const onUp = () => { dragOffset.current = null; };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [fixed, onPosChange]);

  const providerOff = config?.mode === "off";
  const needsKey = config?.mode === "api" && hasKey === false;
  const noModel = !model.trim();
  const blocked = target.kind === "file" && (preview?.blocked.length ?? 0) > 0;
  const loadingPreview = target.kind === "file" && !preview && !error;
  const estTokens = target.kind === "file" ? (preview?.estTokens ?? 0) : Math.ceil(target.snippet.length / 4);
  const canSend = !providerOff && !needsKey && !noModel && !blocked;

  const send = () => {
    if (!canSend || !disclosure) return;
    const chosenModel = disclosure.model;
    startAiStreamingTask(key, lens, title, subtitle, async (onDelta) => {
      const text = await api.aiReadStream(
        target.nodeId,
        target.kind === "text" ? target.snippet : null,
        lens,
        level,
        chosenModel,
        onDelta
      );
      return { result: text, meta: `${chosenModel || config?.model || "model"} · ~${estTokens.toLocaleString()} input tokens` };
    });
    setDisclosure(null);
  };

  const showDisclosure = async () => {
    if (!canSend) return;
    setDisclosureBusy(true);
    setDisclosureError(null);
    try {
      const exact = await api.aiSendDisclosure(
        target.nodeId,
        target.kind === "text" ? target.snippet : null,
        lens,
        level,
        model.trim()
      );
      setDisclosure(exact);
    } catch (err) {
      setDisclosure(null);
      setDisclosureError(err instanceof Error ? err.message : String(err));
    } finally {
      setDisclosureBusy(false);
    }
  };

  let body: ReactNode;
  if (task?.status === "pending") {
    body = (
      <div className="ai-streaming-result">
        <div className="ai-thinking">
          <Loader2 className="ai-spin" size={16} />
          <div>
            <strong>{task.result ? "Responding…" : "Thinking…"}</strong>
            <small className="muted">You can keep working — this stays here as the answer arrives.</small>
          </div>
        </div>
        {task.result ? <AiMarkdown text={task.result} className="ai-explanation streaming" /> : null}
      </div>
    );
  } else if (task?.status === "done") {
    body = (
      <>
        {lens === "review" ? <AiReviewResult text={task.result ?? ""} /> : <AiMarkdown text={task.result ?? ""} className="ai-explanation" />}
        {task.meta ? <small className="muted">{task.meta}</small> : null}
        <button type="button" className="action-button subtle" onClick={() => clearAiTask(key)}>{lens === "review" ? "Review again" : "Re-explain"}</button>
      </>
    );
  } else {
    body = (
      <>
        {task?.status === "error" && task.result ? <AiMarkdown text={task.result} className="ai-explanation" /> : null}
        {task?.status === "error" ? <p className="ai-blocked"><strong>The response stopped.</strong> {task.error}</p> : null}
        {error ? <p className="ai-blocked">{error}</p> : null}
        {loadingPreview ? <p className="muted">Checking the file…</p> : null}
        {blocked && preview ? (
          <div className="ai-blocked">
            <strong>Not sent — this file is protected:</strong>
            <ul>{preview.blocked.map((reason, index) => <li key={index}>{reason}</li>)}</ul>
            <small>Nothing left your machine.</small>
          </div>
        ) : null}
        {!blocked && config && (target.kind === "text" || preview) ? (
          <>
            <p className="muted">
              Sends {target.kind === "file" ? `this ${preview?.language ?? ""} file` : "the selected snippet"}{" "}
              (~{estTokens.toLocaleString()} tokens) to <strong>{providerSummary(config)}</strong> for a {lens === "review" ? "read-only review checklist" : "plain-language explanation"}. Nothing else leaves your machine.
            </p>
            <AiUsageMeter projectedInputTokens={estTokens} providerMode={config.mode} />
            <div className="ai-lens-tabs" role="tablist" aria-label="Code reading lens">
              <button type="button" role="tab" aria-selected={lens === "explain"} className={lens === "explain" ? "active" : ""} onClick={() => setLens("explain")}>
                <Sparkles size={14} /> Explain
              </button>
              <button type="button" role="tab" aria-selected={lens === "review"} className={lens === "review" ? "active" : ""} onClick={() => setLens("review")}>
                <ListChecks size={14} /> What to check
              </button>
            </div>
            <div className="ai-controls">
              <label>
                Level
                <select value={level} onChange={(event) => setLevel(event.target.value as "vibe" | "engineer")}>
                  <option value="vibe">Plain (vibe coder)</option>
                  <option value="engineer">Engineer</option>
                </select>
              </label>
              <label>
                Model
                <input list="ai-explain-models" value={model} onChange={(event) => setModel(event.target.value)} placeholder="model name" spellCheck={false} autoComplete="off" />
                <datalist id="ai-explain-models">
                  {[...new Set(models)].map((m) => <option key={m} value={m} />)}
                </datalist>
              </label>
              <button type="button" className="action-button subtle" disabled={providerOff} onClick={refreshModels} data-help="Ask the configured endpoint for its model list to fill the Model suggestions. This is the only request made before you press Explain, and only when you click.">
                Refresh models
              </button>
            </div>
            {providerOff ? <p className="ai-blocked"><strong>AI Assist is off.</strong> Choose a local model or an API provider in Settings ▸ Advanced ▸ AI Assist first.</p> : null}
            {needsKey ? <p className="ai-blocked"><strong>No API key saved.</strong> Add one in Settings ▸ Advanced ▸ AI Assist first.</p> : null}
            {!providerOff && !needsKey && noModel ? <p className="ai-blocked"><strong>No model set.</strong> Type a model name above, or set a default in Settings ▸ Advanced ▸ AI Assist.</p> : null}
            <div className="ai-send-actions">
              {disclosure ? (
                <>
                  <button type="button" className="primary-button" disabled={!canSend} onClick={send}>{lens === "review" ? "Send for review checklist" : "Send for explanation"}</button>
                  <button type="button" className="secondary-button" onClick={() => setDisclosure(null)}>Cancel</button>
                </>
              ) : (
                <button type="button" className="primary-button" disabled={!canSend || disclosureBusy} onClick={() => void showDisclosure()}>
                  {disclosureBusy ? <Loader2 className="ai-spin" size={14} /> : <Info size={14} />}
                  Review exactly what is sent
                </button>
              )}
            </div>
            {disclosureError ? <p className="ai-blocked">{disclosureError}</p> : null}
            {disclosure ? (
              <section className="ai-send-disclosure" aria-label="Exact AI request">
                <div className="ai-send-disclosure-head">
                  <strong>{disclosure.method} {disclosure.url}</strong>
                  <span>{disclosure.mode === "local" ? "This machine" : "External API"}</span>
                </div>
                <dl>
                  <div><dt>Model</dt><dd>{disclosure.model}</dd></div>
                  <div><dt>Transport</dt><dd>{disclosure.transport}</dd></div>
                  <div><dt>Content</dt><dd>{disclosure.sendChars.toLocaleString()} chars · ~{disclosure.estTokens.toLocaleString()} tokens</dd></div>
                </dl>
                <pre>{disclosure.requestBody}</pre>
                {disclosure.fallbackRequestBody ? (
                  <details>
                    <summary>Possible non-stream fallback body</summary>
                    <pre>{disclosure.fallbackRequestBody}</pre>
                  </details>
                ) : null}
                <small className="muted">Credentials are added inside Rust and are never shown here. The send gate rebuilds this request from the current file when you press send.</small>
              </section>
            ) : null}
          </>
        ) : null}
      </>
    );
  }

  // Clamp the floating position to the viewport at render time too, so shrinking the window can
  // never strand the panel off-screen (the drag handler also clamps live).
  const viewportW = typeof window !== "undefined" ? window.innerWidth : 1280;
  const viewportH = typeof window !== "undefined" ? window.innerHeight : 800;
  const floatingStyle = !fixed
    ? { left: Math.max(8, Math.min(viewportW - 60, pos.x)), top: Math.max(8, Math.min(viewportH - 60, pos.y)) }
    : undefined;

  // Collapsed edge card: a thin strip on the right that reclaims its reserved
  // space; click to expand. Mirrors the right-pane collapse affordance.
  if (edge && collapsed) {
    return (
      <button
        className="ai-explain-panel-collapsed"
        type="button"
        onClick={onToggleCollapse}
        title={`Expand ${lens === "review" ? "What to check" : "Explain this"} — ${title}`}
        aria-label={`Expand ${lens === "review" ? "What to check" : "Explain this"} — ${title}`}
      >
        <ChevronLeft size={15} />
        {lens === "review" ? <ListChecks size={14} /> : <Sparkles size={14} />}
      </button>
    );
  }

  return (
    <div
      className={`ai-explain-panel ${edge ? "edge" : docked ? "docked" : "floating"}`}
      style={floatingStyle}
      role="dialog"
      aria-label={`${lens === "review" ? "Review" : "Explain"} ${title}`}
    >
      <header className="ai-explain-panel-header" onMouseDown={onHeaderMouseDown}>
        <div className="ai-explain-panel-title">
          <strong>{lens === "review" ? <ListChecks size={14} /> : <Sparkles size={14} />} {lens === "review" ? "What to check" : "Explain this"}</strong>
          <span title={subtitle}>{title}</span>
        </div>
        <div className="ai-explain-panel-actions">
          {edge && onToggleCollapse ? (
            <button className="icon-button" type="button" onClick={onToggleCollapse} title="Collapse to the edge" aria-label="Collapse Explain this to the edge">
              <ChevronRight size={15} />
            </button>
          ) : null}
          <button className="icon-button" type="button" onClick={onToggleDock} title={fixed ? "Pop out into a floating window" : "Dock to the side"} aria-label={fixed ? "Pop out into a floating window" : "Dock to the side"}>
            {fixed ? <ExternalLink size={15} /> : <PanelRight size={15} />}
          </button>
          <button className="icon-button" type="button" onClick={onClose} aria-label="Close"><X size={15} /></button>
        </div>
      </header>
      <div className="ai-explain-panel-body">
        {body}
        {config ? (
          <AiLearningTools
            key={target.kind === "file" ? `learning:file:${target.nodeId}` : `learning:text:${target.nodeId}:${hashSnippet(target.snippet)}`}
            target={target}
            config={config}
            model={model.trim()}
            level={level}
            destination={providerSummary(config)}
            canSend={canSend}
            explanation={task?.result ?? ""}
          />
        ) : null}
      </div>
    </div>
  );
});

/**
 * Settings card to configure the AI provider (connector edition only). The user chooses Off
 * (default — nothing leaves the machine), a local model server (loopback only, keyless), or an
 * external API provider (their own endpoint + key). No provider is hardcoded; the key, when
 * used, lives only in the Windows Credential Manager — never in the app DB or logs.
 */
export const AiAssistKeyCard = memo(function AiAssistKeyCard() {
  const [config, setConfig] = useState<AiProviderConfig | null>(null);
  const [keyStatus, setKeyStatus] = useState<boolean | null>(null);
  const [keyValue, setKeyValue] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [localCandidates, setLocalCandidates] = useState<AiLocalProviderCandidate[]>([]);
  const [discovering, setDiscovering] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  // Local backend reads only — no mount-time model fetch: the outbound call to the
  // endpoint happens only on the explicit Test / Save / Refresh models actions.
  useEffect(() => {
    void Promise.all([api.aiProviderGet(), api.aiKeyStatus()])
      .then(([c, k]) => {
        setConfig(c);
        setKeyStatus(k);
      })
      .catch((err: unknown) => {
        setConfig(EMPTY_CONFIG);
        setKeyStatus(false);
        setMessage(err instanceof Error ? err.message : String(err));
      });
  }, []);

  if (!config) {
    return (
      <div className="dashboard-card">
        <h3>AI Assist — provider</h3>
        <p className="muted">Loading…</p>
      </div>
    );
  }

  const update = (patch: Partial<AiProviderConfig>) => setConfig({ ...config, ...patch });

  const applyPreset = (preset: Preset) => {
    setMessage(null);
    update({
      mode: preset.mode,
      baseUrl: preset.baseUrl,
      format: preset.format,
      model: preset.model ?? ""
    });
  };

  const discoverLocal = async () => {
    setDiscovering(true);
    setMessage(null);
    try {
      const found = await api.aiLocalDiscover();
      setLocalCandidates(found);
      setMessage(found.length === 0
        ? "No compatible local model server answered on the common loopback ports."
        : `Found ${found.length} compatible local model ${found.length === 1 ? "server" : "servers"}. Choose one below, then save.`);
    } catch (err) {
      setLocalCandidates([]);
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setDiscovering(false);
    }
  };

  const selectLocalCandidate = (candidate: AiLocalProviderCandidate) => {
    update({ mode: "local", baseUrl: candidate.baseUrl, format: candidate.format, model: candidate.models[0] ?? "" });
    setModels(candidate.models);
    setMessage(`${candidate.label} selected as a draft. Save to make it the active provider.`);
  };

  // Best-effort model dropdown for the given (draft) config — read-only, never persists.
  const refreshModels = (cfg: AiProviderConfig) => {
    if (cfg.mode === "off") {
      setModels([]);
      return;
    }
    api.aiProviderModels(cfg.mode, cfg.baseUrl.trim(), cfg.model.trim(), cfg.format)
      .then((list) => setModels([...new Set(list)]))
      .catch(() => setModels([]));
  };

  const save = async () => {
    setBusy(true);
    setMessage(null);
    try {
      // aiProviderSet validates the config server-side (and loopback for a local endpoint).
      await api.aiProviderSet(config.mode, config.baseUrl.trim(), config.model.trim(), config.format);
      setMessage("Provider settings saved.");
      refreshModels(config);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setBusy(true);
    setMessage(null);
    try {
      // Non-destructive: probe the on-screen draft WITHOUT persisting it, so a connectivity check
      // never overwrites the saved provider.
      const result = await api.aiProviderTest(config.mode, config.baseUrl.trim(), config.model.trim(), config.format);
      setMessage(`✓ ${result}`);
      refreshModels(config);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const saveKey = async () => {
    setBusy(true);
    setMessage(null);
    try {
      await api.aiKeySet(keyValue.trim());
      setKeyValue("");
      setKeyStatus(true);
      setMessage("Key saved to the Windows Credential Manager.");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const clearKey = async () => {
    setBusy(true);
    setMessage(null);
    try {
      await api.aiKeyClear();
      setKeyStatus(false);
      setMessage("Key removed.");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const showEndpoint = config.mode !== "off";
  const showKey = config.mode === "api";

  return (
    <div
      className="dashboard-card ai-provider-card"
      data-help="Choose how 'Explain this' runs: Off (nothing leaves your machine), a local model server on this machine, or your own API provider. Both Chat Completions–compatible and Messages-API–compatible endpoints work. A ChatGPT or Claude subscription is not an API key — use your own API key (or reach many models through one OpenRouter key). Any API key is stored only in the Windows Credential Manager. Sensitive files and files containing secrets are hard-blocked before anything is sent."
    >
      <div className="ai-card-head">
        <div className="heading-with-help">
          <h3>Choose how AI explanations run</h3>
          <HelpPopover title="Local model or API?" compact>
            <p>A local model runs on this computer through a program such as Ollama or LM Studio. Your project text stays on this machine.</p>
            <p>An API sends only the text you explicitly approve to the address shown before sending. A ChatGPT or Claude subscription is not an API key.</p>
          </HelpPopover>
        </div>
        <span className={`ai-status-pill ${config.mode === "off" ? "off" : "on"}`}>{config.mode === "off" ? "Off · optional" : config.mode === "local" ? "Local model" : "Your API"}</span>
      </div>
      <p className="muted help-copy">
        Right-click a file or selected text, then choose <strong>Explain with AI</strong>. Leave this Off, use a model running on this computer, or connect an API with your own key. Nothing is sent until you approve the exact request.
      </p>

      <div className="ai-section">
        <p className="ai-section-head">Mode</p>
        <div className="ai-field">
          <label htmlFor="ai-mode">Provider</label>
          <select
            id="ai-mode"
            value={config.mode}
            onChange={(event) => {
              // Reset the endpoint fields on a mode change so a previous mode's values can't carry
              // over (e.g. an https://api.openai.com URL lingering after switching to 'local').
              setMessage(null);
              update({ mode: event.target.value as AiProviderMode, baseUrl: "", model: "", format: "chat_completions" });
            }}
          >
            <option value="off">Off — no AI (nothing leaves this machine)</option>
            <option value="local">Local model server (on this machine)</option>
            <option value="api">API provider (your own endpoint + key)</option>
          </select>
        </div>
      </div>

      {showEndpoint ? (
        <>
          <div className="ai-section">
            <p className="ai-section-head">Endpoint</p>
            {config.mode === "local" ? (
              <div className="ai-local-discovery">
                <div>
                  <strong>Models already running on this PC</strong>
                  <small className="muted">Checks only common numeric 127.0.0.1 ports, and only when you click.</small>
                </div>
                <button type="button" className="secondary-button" disabled={busy || discovering} onClick={() => void discoverLocal()}>
                  {discovering ? <Loader2 className="ai-spin" size={14} /> : <Search size={14} />}
                  Find local models
                </button>
                {localCandidates.length > 0 ? (
                  <div className="ai-local-results" aria-label="Discovered local model servers">
                    {localCandidates.map((candidate) => (
                      <div className="ai-local-result" key={candidate.baseUrl}>
                        <Server size={16} />
                        <div>
                          <strong>{candidate.label}</strong>
                          <small>{candidate.baseUrl} · {candidate.models.length} {candidate.models.length === 1 ? "model" : "models"}</small>
                        </div>
                        <button type="button" className="action-button subtle" onClick={() => selectLocalCandidate(candidate)}>Use</button>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : null}
            <div className="ai-preset-row">
              <span className="muted">Quick fill:</span>
              {PRESETS.filter((p) => p.mode === config.mode).map((preset) => (
                <button key={preset.label} type="button" className="ai-preset-chip" title={preset.hint} onClick={() => applyPreset(preset)}>
                  {preset.label}
                </button>
              ))}
            </div>

          <div className="ai-field">
            <label htmlFor="ai-base-url">Endpoint URL</label>
            <input
              id="ai-base-url"
              value={config.baseUrl}
              onChange={(event) => update({ baseUrl: event.target.value })}
              placeholder={config.mode === "local" ? "http://localhost:11434/v1" : "https://…/v1"}
              spellCheck={false}
              autoComplete="off"
            />
            <small className="muted">
              {config.mode === "local"
                ? "Must be on this machine (127.0.0.1, localhost or ::1). For Chat Completions–compatible servers include the /v1 suffix."
                : "Chat Completions–compatible endpoints include the /v1 suffix; for the Messages-API format use the host root (no /v1)."}
            </small>
          </div>

          <div className="ai-field">
            <label htmlFor="ai-format">API format</label>
            <select
              id="ai-format"
              value={config.format}
              onChange={(event) => update({ format: event.target.value as AiProviderFormat })}
            >
              <option value="chat_completions">Chat Completions–compatible (most local servers + API providers)</option>
              <option value="messages_api">Messages API–compatible</option>
            </select>
          </div>
          </div>

          <div className="ai-section">
            <p className="ai-section-head">Model</p>
          <div className="ai-field">
            <label htmlFor="ai-model">Model</label>
            <input
              id="ai-model"
              list="ai-provider-models"
              value={config.model}
              onChange={(event) => update({ model: event.target.value })}
              placeholder="model name (e.g. qwen2.5-coder, gpt-5.6, claude-…) "
              spellCheck={false}
              autoComplete="off"
            />
            <datalist id="ai-provider-models">
              {[...new Set(models)].map((m) => <option key={m} value={m} />)}
            </datalist>
            <button type="button" className="secondary-button" disabled={busy} onClick={() => refreshModels(config)} data-help="Fetch the endpoint's model list into the suggestions above. Nothing is sent besides that request, and only when you click.">
              Refresh models
            </button>
          </div>
          </div>

          {showKey ? (
            <div className="ai-section">
              <p className="ai-section-head">API key<span className={`ai-status-pill ${keyStatus ? "on" : "warn"}`}>{keyStatus ? "Saved" : "Needed"}</span></p>
              <div className="ai-field">
                <div className="ai-key-row">
                  <input type="password" placeholder="your API key" value={keyValue} onChange={(event) => setKeyValue(event.target.value)} autoComplete="off" spellCheck={false} />
                  <button type="button" className="secondary-button" disabled={busy || keyValue.trim().length < 12} onClick={() => void saveKey()}>Save key</button>
                  <button type="button" className="secondary-button" disabled={busy || !keyStatus} onClick={() => void clearKey()}>Remove</button>
                </div>
                <small className="muted">Stored only in the Windows Credential Manager — never in Code Hangar's database or logs.</small>
              </div>
            </div>
          ) : null}

          <p className="muted ai-where">Requests go to <strong>{providerSummary(config)}</strong>.</p>
        </>
      ) : (
        <p className="muted ai-where">No AI provider configured — "Explain this" is off, and nothing leaves this machine until you turn it on.</p>
      )}

      <AiUsageMeter
        projectedInputTokens={showEndpoint ? 17 : undefined}
        projectedOutputTokens={16}
        providerMode={config.mode}
        editable
      />
      <div className="ai-section ai-actions-section">
        <p className="ai-section-head">Actions</p>
        <div className="ai-provider-actions">
          <button type="button" className="primary-button" disabled={busy} onClick={() => void save()}>Save</button>
          {showEndpoint ? (
            <button type="button" className="secondary-button" disabled={busy} onClick={() => void test()}>Test provider</button>
          ) : null}
        </div>
        {message ? <small className="muted ai-message">{message}</small> : null}
      </div>
    </div>
  );
});
