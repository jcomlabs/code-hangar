import { ChevronDown, ChevronUp, FileDiff, History, RotateCcw } from "lucide-react";
import { memo, useCallback, useEffect, useState } from "react";

import { api } from "../../api";
import { ConceptHelp } from "../../BeginnerHelp";
import type { EditSnapshotComparison, EditSnapshotSummary } from "../../types";
import { VersionCompareDialog } from "./ChangeReviewDialog";

export function snapshotOriginLabel(origin: string): string {
  switch (origin) {
    case "manual": return "Manual edit";
    case "value": return "Value edit";
    case "ai_suggestion": return "AI suggestion";
    case "ai_session": return "AI session";
    case "restore": return "Restore safety copy";
    default: return "Saved edit";
  }
}

function localTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export const PreviousVersions = memo(function PreviousVersions({
  nodeId,
  saving = false,
  onFileMutated,
  setStatusText,
  onUndoAiSession
}: {
  nodeId: number;
  saving?: boolean;
  onFileMutated: (nodeId: number) => void | Promise<void>;
  setStatusText: (value: string) => void;
  onUndoAiSession?: (nodeId: number, sessionId: string) => Promise<void>;
}) {
  const [versions, setVersions] = useState<EditSnapshotSummary[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [restoringId, setRestoringId] = useState<number | null>(null);
  const [confirmSessionId, setConfirmSessionId] = useState<string | null>(null);
  const [undoingSessionId, setUndoingSessionId] = useState<string | null>(null);
  const [comparingId, setComparingId] = useState<number | null>(null);
  const [comparison, setComparison] = useState<{ result: EditSnapshotComparison; label: string; restoreId?: number } | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setVersions(await api.editSnapshotsForNode(nodeId, 20));
    } catch (error) {
      setStatusText(`File history unavailable: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [nodeId, setStatusText]);

  useEffect(() => {
    if (!saving) void load();
  }, [load, saving]);

  async function restore(snapshotId: number) {
    setRestoringId(snapshotId);
    try {
      const result = await api.editSnapshotRestore(snapshotId);
      setComparison(null);
      await onFileMutated(nodeId);
      await load();
      setStatusText(result.message);
    } catch (error) {
      setStatusText(`Restore failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setRestoringId(null);
    }
  }

  async function undoSession(sessionId: string) {
    if (!onUndoAiSession) return;
    setUndoingSessionId(sessionId);
    try {
      await onUndoAiSession(nodeId, sessionId);
      setConfirmSessionId(null);
      await load();
    } catch (error) {
      setStatusText(`Session undo failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setUndoingSessionId(null);
    }
  }

  async function compare(version: EditSnapshotSummary, forRestore = false) {
    setComparingId(version.id);
    try {
      setComparison({
        result: await api.editSnapshotCompare(version.id),
        label: `${snapshotOriginLabel(version.origin)} · ${localTime(version.createdAt)}`,
        restoreId: forRestore ? version.id : undefined
      });
    } catch (error) {
      setStatusText(`Compare failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setComparingId(null);
    }
  }

  return (
    <section className="previous-versions" aria-label="Previous versions of this file">
      <div className="inline-disclosure-heading">
        <button className="previous-versions-toggle" type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          <History size={15} />
          <span>Previous versions</span>
          <small>{loading ? "Loading" : versions.length === 0 ? "None yet" : `${versions.length} saved`}</small>
          {expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
        </button>
        <ConceptHelp concept="versions" />
      </div>
      {expanded ? (
        <div className="previous-version-list">
          {versions.length === 0 && !loading ? <p>No previous version has been created for this file yet.</p> : null}
          {versions.map((version, index) => (
            <div className="previous-version-row" key={version.id}>
              <div>
                <strong>{snapshotOriginLabel(version.origin)}</strong>
                <span>{localTime(version.createdAt)}</span>
                {version.restoredAt ? <small>Restored {localTime(version.restoredAt)}</small> : null}
                {version.sessionId ? <small>Session {version.sessionId.slice(-8)}</small> : null}
              </div>
              <div className="previous-version-actions">
              <button type="button" onClick={() => void compare(version)} disabled={restoringId !== null || comparingId !== null}>
                {comparingId === version.id ? <RotateCcw className="spin" size={15} /> : <FileDiff size={15} />} {comparingId === version.id ? "Comparing..." : "Compare"}
              </button>
              {onUndoAiSession && version.sessionId && versions.findIndex((item) => item.sessionId === version.sessionId) === index ? (
                confirmSessionId === version.sessionId ? (
                  <div className="previous-version-confirm">
                    <span>Undo this session?</span>
                    <button type="button" onClick={() => setConfirmSessionId(null)} disabled={undoingSessionId !== null}>Cancel</button>
                    <button type="button" className="primary-button" onClick={() => void undoSession(version.sessionId!)} disabled={undoingSessionId !== null}>
                      {undoingSessionId === version.sessionId ? "Undoing..." : "Undo session"}
                    </button>
                  </div>
                ) : (
                  <button type="button" onClick={() => setConfirmSessionId(version.sessionId!)} disabled={restoringId !== null || undoingSessionId !== null}>
                    <RotateCcw size={15} /> Undo session
                  </button>
                )
              ) : null}
              <button className="icon-button" type="button" aria-label={`Review and restore version from ${localTime(version.createdAt)}`} data-help="Compare this verified previous version with the current file before restoring. The current file is saved first, so the restore can also be undone." onClick={() => void compare(version, true)} disabled={restoringId !== null || comparingId !== null}>
                <RotateCcw size={15} />
              </button>
              </div>
            </div>
          ))}
        </div>
      ) : null}
      {comparison ? (
        <VersionCompareDialog
          comparison={comparison.result}
          versionLabel={comparison.label}
          restoring={comparison.restoreId === restoringId}
          onClose={() => setComparison(null)}
          onRestore={comparison.restoreId == null ? undefined : () => restore(comparison.restoreId!)}
        />
      ) : null}
    </section>
  );
});
