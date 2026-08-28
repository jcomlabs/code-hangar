import { AlertTriangle, CheckCircle2, FileDiff, GitBranch, ShieldCheck, X } from "lucide-react";
import { memo, useEffect, useState } from "react";

import type { EditSnapshotComparison, FileEditPreview, SessionDiffHunk, SessionDiffLine } from "../../types";

function lineNumber(line: SessionDiffLine) {
  if (line.kind === "added") return line.newLine ?? "";
  return line.oldLine ?? line.newLine ?? "";
}

export const LiteDiff = memo(function LiteDiff({
  hunks,
  addedLines,
  removedLines,
  truncated = false
}: {
  hunks: SessionDiffHunk[];
  addedLines: number;
  removedLines: number;
  truncated?: boolean;
}) {
  return (
    <section className="change-lite-diff" aria-label="Before and after line changes">
      <header>
        <strong><FileDiff size={16} /> Line changes</strong>
        <span className="change-diff-count added">+{addedLines}</span>
        <span className="change-diff-count removed">-{removedLines}</span>
      </header>
      {hunks.length === 0 ? <p className="muted">No visible line-content change.</p> : (
        <div className="change-diff-hunks">
          {hunks.map((hunk, hunkIndex) => (
            <div className="change-diff-hunk" key={`${hunk.header}-${hunkIndex}`}>
              <div className="change-diff-hunk-header">{hunk.header}</div>
              <pre>{hunk.lines.map((line, lineIndex) => (
                <span className={`change-diff-line ${line.kind}`} key={`${lineIndex}-${line.content}`}>
                  <b>{lineNumber(line)}</b>
                  <i>{line.kind === "added" ? "+" : line.kind === "removed" ? "-" : line.kind === "note" ? "!" : " "}</i>
                  <code>{line.content || " "}</code>
                </span>
              ))}</pre>
            </div>
          ))}
        </div>
      )}
      {truncated ? <small className="change-diff-truncated"><AlertTriangle size={13} /> The local diff limit omitted additional changed lines.</small> : null}
    </section>
  );
});

export const ChangeReviewDialog = memo(function ChangeReviewDialog({
  preview,
  fileName,
  title = "Review this change",
  applyLabel = "Apply one file",
  applying = false,
  onClose,
  onApply
}: {
  preview: FileEditPreview;
  fileName: string;
  title?: string;
  applyLabel?: string;
  applying?: boolean;
  onClose: () => void;
  onApply: () => void | Promise<void>;
}) {
  const [confirmed, setConfirmed] = useState(false);

  useEffect(() => {
    setConfirmed(false);
  }, [preview.afterHash]);

  const gitWarning = !new Set(["clean", "not_repository"]).has(preview.gitContext.state);
  return (
    <div className="modal-overlay change-review-overlay" role="dialog" aria-modal="true" aria-label={title}>
      <div className="modal change-review-dialog">
        <header className="change-review-heading">
          <div>
            <span>One local file</span>
            <strong><FileDiff size={17} /> {title}</strong>
            <small>{fileName}</small>
          </div>
          <button className="icon-button" type="button" aria-label="Close change review" onClick={onClose} disabled={applying}><X size={16} /></button>
        </header>

        <div className="change-review-facts">
          <div className={`change-review-fact ${preview.validation.status}`}>
            {preview.validation.status === "passed" ? <CheckCircle2 size={17} /> : <AlertTriangle size={17} />}
            <span><strong>{preview.validation.label}</strong><small>{preview.validation.note}</small></span>
          </div>
          <div className={`change-review-fact git-${preview.gitContext.state}${gitWarning ? " warning" : ""}`}>
            <GitBranch size={17} />
            <span><strong>{preview.gitContext.label}</strong><small>{preview.gitContext.note}</small></span>
          </div>
          <div className="change-review-fact protected">
            <ShieldCheck size={17} />
            <span><strong>Previous version prepared before writing</strong><small>Applying rechecks the complete file and refuses a stale draft. Git files outside this change are untouched.</small></span>
          </div>
        </div>

        <LiteDiff hunks={preview.hunks} addedLines={preview.addedLines} removedLines={preview.removedLines} truncated={preview.diffTruncated} />

        <footer className="change-review-actions">
          <label>
            <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} disabled={applying} />
            I reviewed every visible removed and added line and want to change this real file
          </label>
          <div>
            <button type="button" onClick={onClose} disabled={applying}>Back to draft</button>
            <button type="button" className="primary-button" onClick={() => void onApply()} disabled={!confirmed || applying}>
              {applying ? "Applying..." : applyLabel}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
});

export const VersionCompareDialog = memo(function VersionCompareDialog({
  comparison,
  versionLabel,
  restoring = false,
  onClose,
  onRestore
}: {
  comparison: EditSnapshotComparison;
  versionLabel: string;
  restoring?: boolean;
  onClose: () => void;
  onRestore?: () => void | Promise<void>;
}) {
  const [confirmed, setConfirmed] = useState(false);

  useEffect(() => {
    setConfirmed(false);
  }, [comparison.snapshotId]);

  return (
    <div className="modal-overlay change-review-overlay" role="dialog" aria-modal="true" aria-label="Compare previous version">
      <div className="modal change-review-dialog version-compare-dialog">
        <header className="change-review-heading">
          <div>
            <span>Restore preview</span>
            <strong><FileDiff size={17} /> Compare previous version</strong>
            <small>{versionLabel}</small>
          </div>
          <button className="icon-button" type="button" aria-label="Close version comparison" onClick={onClose}><X size={16} /></button>
        </header>
        {comparison.alreadyCurrent ? (
          <div className="change-review-fact passed"><CheckCircle2 size={17} /><span><strong>Already matches the current file</strong><small>Restoring this version would not change the file.</small></span></div>
        ) : (
          <div className="change-review-fact protected"><ShieldCheck size={17} /><span><strong>This is a read-only restore preview</strong><small>Green lines would be added and red lines removed. Nothing is restored from this dialog.</small></span></div>
        )}
        <LiteDiff hunks={comparison.hunks} addedLines={comparison.addedLines} removedLines={comparison.removedLines} truncated={comparison.diffTruncated} />
        <footer className="change-review-actions compare-only">
          {onRestore && !comparison.alreadyCurrent ? (
            <label>
              <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} disabled={restoring} />
              I reviewed the comparison and want to replace the current file with this version
            </label>
          ) : <span>Comparison only. No file has changed.</span>}
          {onRestore ? <button type="button" onClick={onClose} disabled={restoring}>Back</button> : null}
          <button
            type="button"
            className="primary-button"
            onClick={() => void (onRestore ? onRestore() : onClose())}
            disabled={restoring || (Boolean(onRestore) && (comparison.alreadyCurrent || !confirmed))}
          >
            {onRestore ? restoring ? "Restoring..." : "Restore this version" : "Done"}
          </button>
        </footer>
      </div>
    </div>
  );
});
