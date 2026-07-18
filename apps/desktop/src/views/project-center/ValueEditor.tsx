import { AlertTriangle, FileDiff, RefreshCcw, SlidersHorizontal } from "lucide-react";
import { memo, useCallback, useEffect, useState } from "react";

import { api } from "../../api";
import type { EditableValue, EditableValueSet, FileEditPreview, ValueEditRequest } from "../../types";
import { ChangeReviewDialog } from "./ChangeReviewDialog";
import { CorrectionChecks } from "./CorrectionChecks";
import { PreviousVersions } from "./PreviousVersions";

export function ValueEditorForm({
  valueSet,
  drafts,
  savingId,
  reviewingId,
  onDraftChange,
  onReview
}: {
  valueSet: EditableValueSet;
  drafts: Record<string, string>;
  savingId: string | null;
  reviewingId: string | null;
  onDraftChange: (valueId: string, draft: string) => void;
  onReview: (value: EditableValue) => void;
}) {
  return (
    <div className="value-list">
      {valueSet.values.map((value) => {
        const draft = drafts[value.id] ?? value.displayValue;
        const dirty = draft !== value.displayValue;
        return (
          <div className="value-row" key={value.id}>
            <label htmlFor={`value-${value.id}`}>
              <strong>{value.label}</strong>
              <span>{value.path}</span>
            </label>
            <div className="value-control">
              {value.kind === "boolean" ? (
                <label className="value-toggle">
                  <input id={`value-${value.id}`} type="checkbox" checked={draft === "true"} onChange={(event) => onDraftChange(value.id, String(event.target.checked))} disabled={savingId !== null} />
                  <span>{draft === "true" ? "On" : "Off"}</span>
                </label>
              ) : (
                <div className={value.kind === "color" ? "value-color-control" : undefined}>
                  {value.kind === "color" && /^#[0-9a-f]{6}$/i.test(draft) ? <span className="value-color-swatch" style={{ backgroundColor: draft }} aria-hidden="true" /> : null}
                  <input id={`value-${value.id}`} type={value.kind === "number" ? "number" : "text"} value={draft} step={value.kind === "number" ? "any" : undefined} onChange={(event) => onDraftChange(value.id, event.target.value)} disabled={savingId !== null} />
                </div>
              )}
              <button className="icon-button value-save" type="button" aria-label={`Review change to ${value.label}`} data-help="Review the exact line change and whole-file validation before applying only this value." onClick={() => onReview(value)} disabled={!dirty || savingId !== null || reviewingId !== null}>
                {reviewingId === value.id ? <RefreshCcw className="spin" size={15} /> : <FileDiff size={15} />}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export const ValueEditor = memo(function ValueEditor({
  projectId,
  nodeId,
  authorized,
  onFileMutated,
  setStatusText,
  onUndoAiSession
}: {
  projectId: number;
  nodeId: number;
  authorized: boolean;
  onFileMutated: (nodeId: number) => void | Promise<void>;
  setStatusText: (value: string) => void;
  onUndoAiSession?: (nodeId: number, sessionId: string) => Promise<void>;
}) {
  const [set, setSet] = useState<EditableValueSet | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [reviewingId, setReviewingId] = useState<string | null>(null);
  const [pending, setPending] = useState<{ value: EditableValue; request: ValueEditRequest; preview: FileEditPreview } | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const loaded = await api.editableValues(nodeId);
      setSet(loaded);
      setDrafts(Object.fromEntries(loaded.values.map((value) => [value.id, value.displayValue])));
    } catch (cause) {
      setSet(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [nodeId]);

  useEffect(() => {
    void load();
  }, [load]);

  function requestFor(value: EditableValue): ValueEditRequest | null {
    if (!set) return null;
    const draft = drafts[value.id] ?? value.displayValue;
    if (draft === value.displayValue) return null;
    return {
      valueId: value.id,
      expectedSourceHash: set.sourceHash,
      expectedRawValue: value.rawValue,
      newValue: draft
    };
  }

  async function review(value: EditableValue) {
    if (!authorized) {
      setError("Project file changes are locked. Unlock this project before reviewing a value change.");
      return;
    }
    const request = requestFor(value);
    if (!request) return;
    setReviewingId(value.id);
    setError(null);
    try {
      setPending({ value, request, preview: await api.previewValueEdit(nodeId, request) });
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
    } finally {
      setReviewingId(null);
    }
  }

  async function applyPending() {
    if (!pending) return;
    if (!authorized) {
      setPending(null);
      setError("Project file changes are locked. Unlock this project and review the value again.");
      return;
    }
    setSavingId(pending.value.id);
    try {
      const result = await api.applyValueEdit(nodeId, pending.request, pending.preview.afterHash);
      setPending(null);
      await onFileMutated(nodeId);
      await load();
      setStatusText(result.message);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setStatusText(message);
    } finally {
      setSavingId(null);
    }
  }

  if (loading && !set) {
    return <div className="value-editor-state" role="status"><RefreshCcw className="spin" size={20} /><strong>Loading safe values</strong></div>;
  }

  if (!set) {
    return (
      <div className="value-editor-state error" role="alert">
        <AlertTriangle size={20} />
        <strong>Values unavailable</strong>
        <p>{error}</p>
        <button type="button" onClick={() => void load()}><RefreshCcw size={15} /> Retry</button>
      </div>
    );
  }

  return (
    <div className="value-editor">
      <header className="value-editor-heading">
        <SlidersHorizontal size={18} />
        <div>
          <h2>Values</h2>
          <p>{set.format.toUpperCase()} · {set.values.length} individually editable values</p>
        </div>
        <button className="icon-button" type="button" aria-label="Reload values" data-help="Reload the file and discard unsaved field changes." onClick={() => void load()} disabled={loading || savingId !== null}>
          <RefreshCcw className={loading ? "spin" : ""} size={16} />
        </button>
      </header>
      {error ? <div className="value-editor-error" role="alert"><AlertTriangle size={15} /><span>{error}</span></div> : null}
      <ValueEditorForm
        valueSet={set}
        drafts={drafts}
        savingId={savingId}
        reviewingId={reviewingId}
        onDraftChange={(valueId, draft) => setDrafts((current) => ({ ...current, [valueId]: draft }))}
        onReview={(value) => void review(value)}
      />
      <PreviousVersions nodeId={nodeId} saving={savingId !== null} onFileMutated={async (changedNodeId) => { await onFileMutated(changedNodeId); await load(); }} setStatusText={setStatusText} onUndoAiSession={onUndoAiSession} />
      <CorrectionChecks projectId={projectId} nodeId={nodeId} busy={savingId !== null || Object.entries(drafts).some(([id, draft]) => set.values.find((value) => value.id === id)?.displayValue !== draft)} onFileMutated={async (changedNodeId) => { await onFileMutated(changedNodeId); await load(); }} setStatusText={setStatusText} />
      {pending ? (
        <ChangeReviewDialog
          preview={pending.preview}
          fileName={set.path.split(/[\\/]/).pop() ?? "file"}
          title={`Review ${pending.value.label}`}
          applyLabel="Apply one value"
          applying={savingId !== null}
          onClose={() => setPending(null)}
          onApply={() => void applyPending()}
        />
      ) : null}
    </div>
  );
});
