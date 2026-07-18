import { AlertTriangle, FileDiff, Lock, RefreshCcw, RotateCcw, Undo2 } from "lucide-react";
import { useEffect, useState, type MouseEvent } from "react";

import { api } from "../../api";
import type { FileEditPreview } from "../../types";
import type { FilePreview } from "../../types";
import { formatBytes } from "../../ui";
import { readableSourcePreviewText, shouldUseReadableSourcePreview } from "./previewDisplay";
import { CorrectionChecks } from "./CorrectionChecks";
import { SyntaxHighlightedSource } from "./SyntaxHighlightedSource";
import { PreviousVersions } from "./PreviousVersions";
import { ValueEditor } from "./ValueEditor";
import { ChangeReviewDialog } from "./ChangeReviewDialog";
import type { EditorBinding } from "./types";

function TextChangeEditor({
  preview,
  editor,
  onContextMenu,
  onFileMutated,
  setStatusText,
  onUndoAiSession,
  onOpenRecap
}: {
  preview: FilePreview;
  editor: EditorBinding;
  onContextMenu?: (event: MouseEvent<HTMLElement>) => void;
  onFileMutated?: (nodeId: number) => void | Promise<void>;
  setStatusText?: (value: string) => void;
  onUndoAiSession?: (nodeId: number, sessionId: string) => Promise<void>;
  onOpenRecap?: () => void;
}) {
  const [review, setReview] = useState<FileEditPreview | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const [reviewError, setReviewError] = useState<string | null>(null);

  useEffect(() => {
    setReview(null);
    setReviewError(null);
  }, [editor.draft]);

  async function reviewDraft() {
    if (editor.draft == null || preview.source == null) return;
    setReviewing(true);
    setReviewError(null);
    try {
      setReview(await api.fileEditPreview(preview.nodeId, editor.draft, preview.source));
    } catch (error) {
      setReviewError(error instanceof Error ? error.message : String(error));
    } finally {
      setReviewing(false);
    }
  }

  async function applyReviewed() {
    if (!review) return;
    if (await editor.onSave(review.afterHash)) setReview(null);
  }

  return (
    <div className="edit-pane change-text-pane">
      <div className="edit-toolbar">
        <button type="button" className="primary-button" disabled={!editor.dirty || editor.saving || reviewing || editor.draft == null} onClick={() => void reviewDraft()} data-help="Review the exact before/after lines, local structure check and existing Git state before writing this one file.">
          {reviewing ? <><RefreshCcw className="spin" size={15} /> Reviewing...</> : <><FileDiff size={15} /> Review change</>}
        </button>
        <button type="button" disabled={!editor.dirty || editor.saving} onClick={editor.onRevert} data-help="Discard the unsaved draft and return to the current file on disk."><Undo2 size={15} /> Discard draft</button>
        {editor.canUndo ? <button type="button" disabled={editor.saving} onClick={editor.onUndo} data-help="Restore the verified version from before your last applied change."><RotateCcw size={15} /> Undo last change</button> : null}
        <span className={`edit-status${editor.dirty ? " dirty" : ""}`}>{editor.dirty ? "Draft not applied" : "Matches disk"}</span>
      </div>
      {reviewError ? <div className="edit-review-error" role="alert"><AlertTriangle size={15} /><span>{reviewError}</span></div> : null}
      {editor.canUndo && !editor.dirty && onOpenRecap ? (
        <div className="change-applied-strip" role="status">
          <span>Change applied with a verified previous version.</span>
          <button type="button" onClick={onOpenRecap}><FileDiff size={14} /> View in What changed</button>
        </div>
      ) : null}
      <textarea
        className="edit-textarea"
        value={editor.draft ?? ""}
        disabled={editor.draft == null || editor.saving}
        spellCheck={false}
        onChange={(event) => editor.onChange(event.target.value)}
        onContextMenu={onContextMenu}
        aria-label="Advanced text change draft"
      />
      {onFileMutated && setStatusText ? (
        <>
          <PreviousVersions nodeId={preview.nodeId} saving={editor.saving} onFileMutated={onFileMutated} setStatusText={setStatusText} onUndoAiSession={onUndoAiSession} />
          <CorrectionChecks projectId={preview.projectId} nodeId={preview.nodeId} busy={editor.saving || editor.dirty} onFileMutated={onFileMutated} setStatusText={setStatusText} />
        </>
      ) : null}
      {review ? (
        <ChangeReviewDialog
          preview={review}
          fileName={preview.displayName}
          applying={editor.saving}
          onClose={() => setReview(null)}
          onApply={() => void applyReviewed()}
        />
      ) : null}
    </div>
  );
}

export function PreviewPane({
  preview,
  onReveal,
  canReveal,
  onOpenProtectedSettings,
  onContextMenu,
  editing,
  valuesEditing,
  changeAuthorized = false,
  editor,
  onFileMutated,
  setStatusText,
  onUndoAiSession,
  onOpenRecap
}: {
  preview: FilePreview;
  onReveal: () => void;
  canReveal: boolean;
  onOpenProtectedSettings: () => void;
  onContextMenu?: (event: MouseEvent<HTMLElement>) => void;
  editing?: boolean;
  valuesEditing?: boolean;
  changeAuthorized?: boolean;
  editor?: EditorBinding;
  onFileMutated?: (nodeId: number) => void | Promise<void>;
  setStatusText?: (value: string) => void;
  onUndoAiSession?: (nodeId: number, sessionId: string) => Promise<void>;
  onOpenRecap?: () => void;
}) {
  if (preview.state !== "ready") {
    return (
      <div className="blocked-preview">
        <Lock size={28} />
        <h2>{preview.state === "blocked" ? "Preview Blocked" : "Preview Unavailable"}</h2>
        <p>{preview.blockedReason}</p>
        {preview.state === "blocked" && canReveal ? <button type="button" data-help="Reveal this one non-strong sensitive file transiently. Content is not indexed, cached or logged." onClick={onReveal}>Reveal text</button> : null}
        {preview.state === "blocked" && !canReveal ? (
          <small>
            Enable temporary local visibility to reveal non-strong protected text.{" "}
            <button className="inline-link" type="button" data-help="Open Settings on Protected locations, where session-only visibility controls live." onClick={onOpenProtectedSettings}>
              Open Protected locations
            </button>
          </small>
        ) : null}
      </div>
    );
  }
  const notice = preview.truncated ? (
    <div className="preview-notice">Content truncated: showing the first {formatBytes(preview.previewLimitBytes)} only.</div>
  ) : preview.wasRevealed ? (
    <div className="preview-notice warning-tone">Sensitive content revealed transiently. It is not stored in the local index.</div>
  ) : null;
  if (valuesEditing && onFileMutated && setStatusText) {
    return <ValueEditor projectId={preview.projectId} nodeId={preview.nodeId} authorized={changeAuthorized} onFileMutated={onFileMutated} setStatusText={setStatusText} onUndoAiSession={onUndoAiSession} />;
  }
  if (editing && editor) {
    return <TextChangeEditor preview={preview} editor={editor} onContextMenu={onContextMenu} onFileMutated={onFileMutated} setStatusText={setStatusText} onUndoAiSession={onUndoAiSession} onOpenRecap={onOpenRecap} />;
  }
  if (preview.mode === "source") {
    return <>{notice}<SyntaxHighlightedSource source={preview.source ?? ""} path={preview.path} onContextMenu={onContextMenu} /></>;
  }
  if (shouldUseReadableSourcePreview(preview)) {
    return <>{notice}<SyntaxHighlightedSource source={readableSourcePreviewText(preview)} path={preview.path} readable onContextMenu={onContextMenu} /></>;
  }
  return <>{notice}<div className="markdown-preview" onContextMenu={onContextMenu} dangerouslySetInnerHTML={{ __html: preview.renderedHtml ?? "" }} /></>;
}
