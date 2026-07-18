import { memo, useEffect, useState } from "react";
import { AlertTriangle, BookOpen, ChevronDown, ChevronUp, Sparkles } from "lucide-react";

import { connectorApi } from "../connectorApi";
import type { AiExplainPreview, AiProviderMode, RecapAiSourceMode, SessionChangeSet } from "../types";
import { HelpPopover } from "../ui";
import { AiUsageMeter } from "./AiUsageMeter";
import { RecapChangeSet } from "./RecapView";
import "./recap-ai.css";

type RecapAiMode = "story" | "learn" | "check";

export function parseRecapAiSections(text: string) {
  const sections: Array<{ key: string; title: string; lines: string[] }> = [];
  const titles: Record<string, string> = {
    story: "Story",
    why: "Why",
    learn: "What to learn",
    unknowns: "Unknowns",
    "what-changed": "What changed",
    "how-to-read": "How to read it",
    "why-it-matters": "Why it matters",
    "be-careful": "Be careful",
    "double-check": "Double-check",
    "heads-up": "Heads-up",
    "what-looks-deliberate": "What looks deliberate"
  };
  let current: { key: string; title: string; lines: string[] } | null = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const match = /^\[([a-z-]+)\]$/i.exec(line);
    if (match) {
      const key = match[1].toLowerCase();
      current = { key, title: titles[key] ?? key.replaceAll("-", " "), lines: [] };
      sections.push(current);
    } else if (line && current) {
      current.lines.push(line.replace(/^[-*]\s+/, ""));
    }
  }
  return sections.length > 0
    ? sections.filter((section) => section.lines.length > 0)
    : [{ key: "response", title: "Model response", lines: [text.trim()].filter(Boolean) }];
}

export const RecapAiLayer = memo(function RecapAiLayer({
  projectId,
  sessionPaths,
  sourceMode,
  changeSet
}: {
  projectId: number;
  sessionPaths: string[];
  sourceMode: RecapAiSourceMode;
  changeSet: SessionChangeSet;
}) {
  const [open, setOpen] = useState(true);
  const [mode, setMode] = useState<RecapAiMode>("story");
  const [selection, setSelection] = useState<{ filePath: string; editIndex: number } | null>(null);
  const [preview, setPreview] = useState<AiExplainPreview | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [providerLabel, setProviderLabel] = useState("Local model");
  const [providerMode, setProviderMode] = useState<AiProviderMode>("off");

  useEffect(() => {
    let current = true;
    void connectorApi.aiProviderGet()
      .then((config) => {
        if (!current) return;
        setProviderMode(config.mode);
        setProviderLabel(config.mode === "local" ? "Local model" : config.mode === "api" ? "External API" : "AI Assist is off");
      })
      .catch(() => {
        if (current) setProviderLabel("Configured model");
      });
    return () => { current = false; };
  }, []);

  useEffect(() => {
    let current = true;
    setPreview(null);
    setError(null);
    setResult(null);
    if (!open) return () => { current = false; };
    const filePath = mode === "learn" ? selection?.filePath : undefined;
    const editIndex = mode === "learn" ? selection?.editIndex : undefined;
    if (mode === "learn" && (!filePath || editIndex == null)) {
      return () => { current = false; };
    }
    void connectorApi.aiChangeSetPreview(projectId, sessionPaths, sourceMode, filePath, editIndex)
      .then((next) => {
        if (current) setPreview(next);
      })
      .catch((reason: unknown) => {
        if (current) setError(String(reason));
      });
    return () => { current = false; };
  }, [mode, open, projectId, selection, sessionPaths, sourceMode, changeSet]);

  async function runGuide() {
    if (!preview || preview.blocked.length > 0) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const next = mode === "story"
        ? await connectorApi.aiNarrateSessionChanges(projectId, sessionPaths, sourceMode, "vibe", "")
        : mode === "check"
          ? await connectorApi.aiReviewChangeSet(projectId, sessionPaths, sourceMode, "vibe", "")
          : selection
            ? await connectorApi.aiExplainChange(projectId, sessionPaths, sourceMode, selection.filePath, selection.editIndex, "vibe", "")
            : null;
      if (next) setResult(next);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  function learnFromChange(filePath: string, editIndex: number) {
    setSelection({ filePath, editIndex });
    setMode("learn");
    setOpen(true);
  }

  return (
    <>
      <GuidePanel
        open={open}
        mode={mode}
        selection={selection}
        preview={preview}
        result={result}
        error={error}
        busy={busy}
        providerLabel={providerLabel}
        providerMode={providerMode}
        onToggle={() => setOpen((value) => !value)}
        onMode={(next) => { setMode(next); if (next !== "learn") setSelection(null); }}
        onRun={() => void runGuide()}
      />
      <RecapChangeSet
        changeSet={changeSet}
        renderEditAction={(filePath, editIndex) => (
          <button type="button" className="secondary-button compact recap-explain-edit" title="Ask AI to explain this recorded change" onClick={() => learnFromChange(filePath, editIndex)}>
            <BookOpen size={13} /> Explain this edit
          </button>
        )}
      />
    </>
  );
});

function GuidePanel({
  open,
  mode,
  selection,
  preview,
  result,
  error,
  busy,
  providerLabel,
  providerMode,
  onToggle,
  onMode,
  onRun
}: {
  open: boolean;
  mode: RecapAiMode;
  selection: { filePath: string; editIndex: number } | null;
  preview: AiExplainPreview | null;
  result: string | null;
  error: string | null;
  busy: boolean;
  providerLabel: string;
  providerMode: AiProviderMode;
  onToggle: () => void;
  onMode: (mode: RecapAiMode) => void;
  onRun: () => void;
}) {
  const labels: Record<RecapAiMode, string> = { story: "Summary", learn: "This edit", check: "Risks to check" };
  return (
    <section className={`recap-ai-guide ${open ? "open" : ""}`}>
      <button type="button" className="recap-ai-toggle" aria-expanded={open} onClick={onToggle}>
        <Sparkles size={15} />
        <span><strong>Explain these changes with AI</strong><small>{providerLabel} · nothing is sent until you choose Explain</small></span>
        {open ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
      </button>
      {open ? (
        <div className="recap-ai-body">
          <div className="recap-ai-toolbar">
            <div className="recap-ai-tabs" role="tablist" aria-label="AI explanation mode">
              {(["story", "check", "learn"] as RecapAiMode[]).map((item) => (
                <button type="button" role="tab" aria-selected={mode === item} className={mode === item ? "active" : ""} disabled={item === "learn" && !selection} key={item} onClick={() => onMode(item)}>{labels[item]}</button>
              ))}
            </div>
            <HelpPopover title="AI explanation" label="What will AI see?">
              <p>Only the local change information shown here is prepared: your recorded requests, before-and-after lines, where they came from and known gaps.</p>
              <p>Secrets are checked again before sending. The explanation is advisory and cannot edit files, run the project or use Git.</p>
            </HelpPopover>
          </div>
          {mode === "learn" && selection ? <code className="recap-ai-selection">{selection.filePath} · edit {selection.editIndex + 1}</code> : null}
          {providerMode === "off" ? (
            <div className="recap-ai-off" role="status"><AlertTriangle size={16} /><span><strong>AI Assist is off</strong>Choose a local model or your own API in Settings → AI Assist. The change details below remain fully available without AI.</span></div>
          ) : error ? <p className="recap-ai-error" role="alert">{error}</p> : preview?.blocked.length ? (
            <p className="recap-ai-error" role="alert">Not sent. {preview.blocked.join(" ")}</p>
          ) : preview ? (
            <>
              <div className="recap-ai-send">
                <span>{preview.sendChars.toLocaleString()} checked characters</span>
                <span>about {preview.estTokens.toLocaleString()} input tokens</span>
                <button type="button" className="action-button compact" disabled={busy} onClick={onRun}><Sparkles size={14} /> {busy ? "Explaining…" : mode === "story" ? "Explain the changes" : mode === "check" ? "Explain what to check" : "Explain this edit"}</button>
              </div>
              <AiUsageMeter projectedInputTokens={preview.estTokens} projectedOutputTokens={mode === "learn" ? 900 : 1_200} providerMode={providerMode} />
            </>
          ) : <span className="recap-ai-preparing">Preparing the local change details…</span>}
          {result ? (
            <div className="recap-ai-result">
              {parseRecapAiSections(result).map((section) => (
                <section key={section.key} className={`recap-ai-section ${section.key}`}>
                  <strong>{section.title}</strong>
                  {section.lines.map((line, index) => <p key={`${section.key}-${index}`}>{line}</p>)}
                </section>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
