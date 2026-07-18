import { memo, useMemo, useState } from "react";
import { BookOpen, ChevronDown, Loader2, MessageCircleQuestion, NotebookPen, Trash2 } from "lucide-react";
import { connectorApi as api } from "../connectorApi";
import type {
  AiExplainPreview,
  AiFollowUpResult,
  AiGlossaryEntry,
  AiGlossaryState,
  AiProviderConfig,
  AiWalkthroughPreview,
  CodeAnnotation
} from "../types";
import type { AiExplainTarget } from "./AiAssist";
import { AiUsageMeter } from "./AiUsageMeter";
import { AiMarkdown } from "./AiMarkdown";
import "./ai-learning.css";

type Level = "vibe" | "engineer";

export function matchingSeedTerms(text: string, seeds: AiGlossaryEntry[]): AiGlossaryEntry[] {
  const normalized = ` ${text.toLocaleLowerCase()} `;
  return seeds.filter((entry) => {
    const term = entry.term.toLocaleLowerCase();
    return new RegExp(`(^|[^a-z0-9])${term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}([^a-z0-9]|$)`, "i").test(normalized);
  });
}

function costLine(tokens: number, config: AiProviderConfig, destination: string) {
  return (
    <small className="ai-learning-cost">
      ~{tokens.toLocaleString()} input tokens · {config.mode === "local" ? "Local" : "API"} · {destination}
    </small>
  );
}

export const AiLearningTools = memo(function AiLearningTools({
  target,
  config,
  model,
  level,
  destination,
  canSend,
  explanation
}: {
  target: AiExplainTarget;
  config: AiProviderConfig;
  model: string;
  level: Level;
  destination: string;
  canSend: boolean;
  explanation: string;
}) {
  const [walkthrough, setWalkthrough] = useState<AiWalkthroughPreview | null>(null);
  const [selectedSections, setSelectedSections] = useState<string[]>([]);
  const [walkthroughResult, setWalkthroughResult] = useState("");
  const [walkthroughBusy, setWalkthroughBusy] = useState(false);
  const [walkthroughError, setWalkthroughError] = useState<string | null>(null);

  const [followSection, setFollowSection] = useState("");
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [followQuestion, setFollowQuestion] = useState("");
  const [followPreview, setFollowPreview] = useState<AiExplainPreview | null>(null);
  const [followAnswers, setFollowAnswers] = useState<AiFollowUpResult[]>([]);
  const [followBusy, setFollowBusy] = useState(false);
  const [followError, setFollowError] = useState<string | null>(null);

  const [glossary, setGlossary] = useState<AiGlossaryState | null>(null);
  const [glossaryBusy, setGlossaryBusy] = useState(false);
  const [glossaryError, setGlossaryError] = useState<string | null>(null);

  const [annotations, setAnnotations] = useState<CodeAnnotation[]>([]);
  const [note, setNote] = useState("");
  const [annotationsLoaded, setAnnotationsLoaded] = useState(false);
  const [annotationBusy, setAnnotationBusy] = useState(false);
  const [annotationError, setAnnotationError] = useState<string | null>(null);
  const [annotationDeleteId, setAnnotationDeleteId] = useState<number | null>(null);
  const [annotationDeleteAcknowledged, setAnnotationDeleteAcknowledged] = useState(false);

  const combinedLearningText = useMemo(
    () => [explanation, walkthroughResult, ...followAnswers.map((answer) => answer.answer)].join("\n"),
    [explanation, followAnswers, walkthroughResult]
  );
  const suggestedTerms = useMemo(
    () => matchingSeedTerms(combinedLearningText, glossary?.seeds ?? []),
    [combinedLearningText, glossary?.seeds]
  );
  const selectedCost = useMemo(() => {
    if (!walkthrough) return 0;
    const chosen = walkthrough.sections.filter((section) => selectedSections.includes(section.id));
    return chosen.reduce((sum, section) => sum + section.estTokens, 0) + chosen.length * 24;
  }, [selectedSections, walkthrough]);
  const selectedBatchBytes = useMemo(() => {
    if (!walkthrough) return 0;
    return 23 + walkthrough.sections
      .filter((section) => selectedSections.includes(section.id))
      .reduce((sum, section) => sum + section.contextBytes, 0);
  }, [selectedSections, walkthrough]);
  const batchTooLarge = Boolean(walkthrough && selectedBatchBytes > walkthrough.maxBatchBytes);

  const loadWalkthrough = async () => {
    if (target.kind !== "file" || walkthrough || walkthroughBusy) return;
    setWalkthroughBusy(true);
    setWalkthroughError(null);
    try {
      const preview = await api.aiWalkthroughPreview(target.nodeId);
      setWalkthrough(preview);
      const ids = preview.defaultSectionIds;
      setSelectedSections(ids);
      setFollowSection(preview.sections[0]?.id ?? "");
    } catch (error) {
      setWalkthroughError(error instanceof Error ? error.message : String(error));
    } finally {
      setWalkthroughBusy(false);
    }
  };

  const toggleSection = (id: string) => {
    setSelectedSections((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  };

  const sendWalkthrough = async () => {
    if (target.kind !== "file" || !canSend || selectedSections.length === 0 || batchTooLarge) return;
    setWalkthroughBusy(true);
    setWalkthroughError(null);
    try {
      setWalkthroughResult(await api.aiWalkthroughFile(target.nodeId, selectedSections, level, model));
    } catch (error) {
      setWalkthroughError(error instanceof Error ? error.message : String(error));
    } finally {
      setWalkthroughBusy(false);
    }
  };

  const changeFollowSection = (sectionId: string) => {
    setFollowSection(sectionId);
    setConversationId(null);
    setFollowAnswers([]);
    setFollowPreview(null);
    setFollowError(null);
  };

  const previewFollowUp = async () => {
    if (target.kind !== "file" || !followSection || !followQuestion.trim()) return;
    setFollowBusy(true);
    setFollowError(null);
    try {
      setFollowPreview(await api.aiFollowUpPreview(
        target.nodeId,
        followSection,
        conversationId,
        followQuestion.trim()
      ));
    } catch (error) {
      setFollowPreview(null);
      setFollowError(error instanceof Error ? error.message : String(error));
    } finally {
      setFollowBusy(false);
    }
  };

  const sendFollowUp = async () => {
    if (target.kind !== "file" || !canSend || !followPreview || !followQuestion.trim()) return;
    setFollowBusy(true);
    setFollowError(null);
    try {
      const answer = await api.aiFollowUp(
        target.nodeId,
        followSection,
        conversationId,
        followQuestion.trim(),
        level,
        model
      );
      setConversationId(answer.conversationId);
      setFollowAnswers((current) => [...current, answer]);
      setFollowQuestion("");
      setFollowPreview(null);
    } catch (error) {
      setFollowError(error instanceof Error ? error.message : String(error));
    } finally {
      setFollowBusy(false);
    }
  };

  const loadGlossary = async () => {
    if (glossary || glossaryBusy) return;
    setGlossaryBusy(true);
    setGlossaryError(null);
    try {
      setGlossary(await api.aiGlossaryState());
    } catch (error) {
      setGlossaryError(error instanceof Error ? error.message : String(error));
    } finally {
      setGlossaryBusy(false);
    }
  };

  const toggleGlossary = async () => {
    if (!glossary || glossaryBusy) return;
    setGlossaryBusy(true);
    setGlossaryError(null);
    try {
      setGlossary(await api.setAiGlossaryEnabled(!glossary.enabled));
    } catch (error) {
      setGlossaryError(error instanceof Error ? error.message : String(error));
    } finally {
      setGlossaryBusy(false);
    }
  };

  const rememberTerms = async () => {
    if (!glossary?.enabled || suggestedTerms.length === 0) return;
    setGlossaryBusy(true);
    setGlossaryError(null);
    try {
      setGlossary(await api.aiGlossaryRecord(suggestedTerms.slice(0, 12).map((entry) => entry.term)));
    } catch (error) {
      setGlossaryError(error instanceof Error ? error.message : String(error));
    } finally {
      setGlossaryBusy(false);
    }
  };

  const loadAnnotations = async () => {
    if (target.kind !== "text" || annotationsLoaded || annotationBusy) return;
    setAnnotationBusy(true);
    setAnnotationError(null);
    try {
      setAnnotations(await api.aiAnnotationsForNode(target.nodeId));
      setAnnotationsLoaded(true);
    } catch (error) {
      setAnnotationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAnnotationBusy(false);
    }
  };

  const addAnnotation = async () => {
    if (target.kind !== "text" || !note.trim() || annotationBusy) return;
    setAnnotationBusy(true);
    setAnnotationError(null);
    try {
      await api.aiAnnotationAdd(target.nodeId, target.snippet, note.trim());
      setNote("");
      setAnnotations(await api.aiAnnotationsForNode(target.nodeId));
      setAnnotationsLoaded(true);
    } catch (error) {
      setAnnotationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAnnotationBusy(false);
    }
  };

  const deleteAnnotation = async (annotationId: number) => {
    if (target.kind !== "text" || annotationBusy || annotationDeleteId !== annotationId || !annotationDeleteAcknowledged) return;
    setAnnotationBusy(true);
    setAnnotationError(null);
    try {
      await api.aiAnnotationDelete(target.nodeId, annotationId);
      setAnnotations((current) => current.filter((annotation) => annotation.id !== annotationId));
      setAnnotationDeleteId(null);
      setAnnotationDeleteAcknowledged(false);
    } catch (error) {
      setAnnotationError(error instanceof Error ? error.message : String(error));
    } finally {
      setAnnotationBusy(false);
    }
  };

  return (
    <div className="ai-learning-tools">
      {target.kind === "file" ? (
        <details onToggle={(event) => { if (event.currentTarget.open) void loadWalkthrough(); }}>
          <summary><BookOpen size={14} /> Walk me through this file <ChevronDown size={13} /></summary>
          <div className="ai-learning-body">
            {walkthroughBusy && !walkthrough ? <Loader2 className="ai-spin" size={15} /> : null}
            {walkthroughError ? <p className="ai-blocked">{walkthroughError}</p> : null}
            {walkthrough?.blocked.length ? <p className="ai-blocked">{walkthrough.blocked.join(" ")}</p> : null}
            {walkthrough && walkthrough.sections.length > 0 ? (
              <>
                <div className="ai-section-list">
                  {walkthrough.sections.map((section) => (
                    <label key={section.id}>
                      <input type="checkbox" checked={selectedSections.includes(section.id)} onChange={() => toggleSection(section.id)} />
                      <span><strong>{section.title}</strong><small>Lines {section.startLine}-{section.endLine} · ~{section.estTokens.toLocaleString()} tokens</small></span>
                    </label>
                  ))}
                </div>
                <small className="muted">
                  All {walkthrough.sections.length} sections across this file are available. Each selected batch stays under {Math.round(walkthrough.maxBatchBytes / 1024)} KiB; choose later sections to continue.
                </small>
                {batchTooLarge ? <p className="ai-blocked">This selection is too large for one send. Select fewer sections.</p> : null}
                {costLine(selectedCost, config, destination)}
                <AiUsageMeter projectedInputTokens={selectedCost} providerMode={config.mode} />
                <button type="button" className="secondary-button" disabled={!canSend || selectedSections.length === 0 || batchTooLarge || walkthroughBusy} onClick={() => void sendWalkthrough()}>
                  {walkthroughBusy ? <Loader2 className="ai-spin" size={14} /> : <BookOpen size={14} />} Walk through selected
                </button>
                {walkthroughResult ? <AiMarkdown text={walkthroughResult} className="ai-learning-answer" /> : null}
              </>
            ) : null}
          </div>
        </details>
      ) : null}

      {target.kind === "file" && walkthrough?.sections.length ? (
        <details>
          <summary><MessageCircleQuestion size={14} /> Ask about one section <ChevronDown size={13} /></summary>
          <div className="ai-learning-body">
            <label className="ai-learning-field">
              Section
              <select value={followSection} onChange={(event) => changeFollowSection(event.target.value)}>
                {walkthrough.sections.map((section) => <option key={section.id} value={section.id}>{section.title}</option>)}
              </select>
            </label>
            {followAnswers.map((answer) => (
              <div className="ai-follow-answer" key={`${answer.conversationId}-${answer.turn}`}>
                <small>Turn {answer.turn} · {answer.remainingTurns} left</small>
                <AiMarkdown text={answer.answer} />
              </div>
            ))}
            <label className="ai-learning-field">
              Question
              <textarea
                value={followQuestion}
                maxLength={600}
                rows={3}
                onChange={(event) => { setFollowQuestion(event.target.value); setFollowPreview(null); }}
              />
            </label>
            {followError ? <p className="ai-blocked">{followError}</p> : null}
            {followPreview ? <>{costLine(followPreview.estTokens, config, destination)}<AiUsageMeter projectedInputTokens={followPreview.estTokens} projectedOutputTokens={900} providerMode={config.mode} /></> : null}
            <div className="ai-learning-actions">
              <button type="button" className="action-button subtle" disabled={!followQuestion.trim() || followBusy || followAnswers.at(-1)?.remainingTurns === 0} onClick={() => void previewFollowUp()}>Check send</button>
              <button type="button" className="secondary-button" disabled={!canSend || !followPreview || followBusy} onClick={() => void sendFollowUp()}>
                {followBusy ? <Loader2 className="ai-spin" size={14} /> : <MessageCircleQuestion size={14} />} Ask
              </button>
            </div>
          </div>
        </details>
      ) : null}

      <details onToggle={(event) => { if (event.currentTarget.open) void loadGlossary(); }}>
        <summary><BookOpen size={14} /> Personal glossary <ChevronDown size={13} /></summary>
        <div className="ai-learning-body">
          {glossaryBusy && !glossary ? <Loader2 className="ai-spin" size={15} /> : null}
          {glossaryError ? <p className="ai-blocked">{glossaryError}</p> : null}
          {glossary ? (
            <>
              <label className="ai-learning-toggle">
                <input type="checkbox" checked={glossary.enabled} onChange={() => void toggleGlossary()} />
                Remember chosen terms locally
              </label>
              {suggestedTerms.length > 0 ? (
                <div className="ai-glossary-suggestions">
                  {suggestedTerms.slice(0, 12).map((entry) => <span key={entry.term} title={entry.definition}>{entry.term}</span>)}
                </div>
              ) : <small className="muted">No seed terms appear in the current explanations.</small>}
              <button type="button" className="action-button subtle" disabled={!glossary.enabled || suggestedTerms.length === 0 || glossaryBusy} onClick={() => void rememberTerms()}>Remember shown terms</button>
              {glossary.entries.length > 0 ? (
                <dl className="ai-glossary-list">
                  {glossary.entries.map((entry) => (
                    <div key={entry.term}><dt>{entry.term} <small>×{entry.count}</small></dt><dd>{entry.definition}</dd></div>
                  ))}
                </dl>
              ) : null}
            </>
          ) : null}
        </div>
      </details>

      {target.kind === "text" ? (
        <details onToggle={(event) => { if (event.currentTarget.open) void loadAnnotations(); }}>
          <summary><NotebookPen size={14} /> Anchored notes <ChevronDown size={13} /></summary>
          <div className="ai-learning-body">
            {annotationBusy && !annotationsLoaded ? <Loader2 className="ai-spin" size={15} /> : null}
            {annotationError ? <p className="ai-blocked">{annotationError}</p> : null}
            <label className="ai-learning-field">
              Note for this selection
              <textarea value={note} maxLength={2000} rows={3} onChange={(event) => setNote(event.target.value)} />
            </label>
            <button type="button" className="secondary-button" disabled={!note.trim() || annotationBusy} onClick={() => void addAnnotation()}>
              <NotebookPen size={14} /> Add note
            </button>
            {annotations.length > 0 ? (
              <ul className="ai-annotation-list">
                {annotations.map((annotation) => (
                  <li key={annotation.id}>
                    <div><span className={`ai-anchor-state ${annotation.anchorState}`}>{annotation.anchorState}</span><small>Lines {annotation.lineStart}-{annotation.lineEnd}</small></div>
                    <p>{annotation.note}</p>
                    {annotationDeleteId === annotation.id ? (
                      <div className="ai-annotation-delete">
                        <label><input type="checkbox" checked={annotationDeleteAcknowledged} onChange={(event) => setAnnotationDeleteAcknowledged(event.target.checked)} /> Delete this local note permanently</label>
                        <div>
                          <button type="button" className="secondary-button compact" onClick={() => { setAnnotationDeleteId(null); setAnnotationDeleteAcknowledged(false); }} disabled={annotationBusy}>Cancel</button>
                          <button type="button" className="danger-button compact" onClick={() => void deleteAnnotation(annotation.id)} disabled={annotationBusy || !annotationDeleteAcknowledged}>Delete note</button>
                        </div>
                      </div>
                    ) : <button type="button" className="icon-button" title="Delete note" aria-label="Delete anchored note" onClick={() => { setAnnotationDeleteId(annotation.id); setAnnotationDeleteAcknowledged(false); }}><Trash2 size={13} /></button>}
                  </li>
                ))}
              </ul>
            ) : annotationsLoaded ? <small className="muted">No anchored notes for this file.</small> : null}
          </div>
        </details>
      ) : null}
    </div>
  );
});
