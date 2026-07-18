import { memo, Suspense, useEffect, useMemo, useState, type ComponentType, type MouseEvent, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Download, FileDiff, GitBranch, History, MessageSquare, RefreshCcw, ShieldCheck, Sparkles } from "lucide-react";

import { api } from "../api";
import { displayAppText } from "../app-meta";
import { ConceptHelp } from "../BeginnerHelp";
import type { ProjectReviewCheckpoint, RecapAiSourceMode, ReviewLedgerEntry, SessionChangeSet, SessionDiscoveryCandidate, SessionDiffLine } from "../types";
import { ExpandableText } from "../ui";

export type RecapDetailLayer = ComponentType<{
  projectId: number;
  sessionPaths: string[];
  sourceMode: RecapAiSourceMode;
  changeSet: SessionChangeSet;
}>;

function sessionKey(session: SessionDiscoveryCandidate) {
  return `${session.sourceKind}:${session.path}`;
}

function sourceLabel(session: SessionDiscoveryCandidate) {
  return displayAppText(session.sourceLabel || session.sourceKind || "Local session");
}

function lineNumber(line: SessionDiffLine) {
  if (line.kind === "added") return line.newLine ?? "";
  return line.oldLine ?? line.newLine ?? "";
}

function realityTitle(reality: { note: string; observedMs?: number | null }) {
  return reality.observedMs
    ? `${reality.note} Compared ${new Date(reality.observedMs).toLocaleString()}.`
    : reality.note;
}

function friendlyRealityLabel(status: string, fallback: string) {
  switch (status) {
    case "applied": return "This change still appears in the file";
    case "reverted": return "This change appears to have been undone";
    case "drifted": return "The file changed again afterwards";
    case "file_missing": return "The file is no longer present";
    case "unverified": return "Code Hangar could not confirm the current file";
    default: return fallback;
  }
}

function friendlyEditSummary(summary: string) {
  if (/recorded (file )?patch/i.test(summary)) return "A group of lines was changed";
  if (/recorded.*diff/i.test(summary)) return "A before-and-after change was recorded";
  return summary;
}

function friendlyEditSource(source: string) {
  const tool = source.split(/[·:]/)[0]?.trim();
  return tool ? `Recorded by ${tool}` : "Recorded in a local AI conversation";
}

function friendlyConfidence(confidence?: string | null) {
  switch (confidence) {
    case "observed": return "Read directly from a local record";
    case "retained": return "Kept from an earlier Code Hangar review";
    case "inferred": return "Inferred from incomplete local clues";
    default: return confidence ? `Evidence label: ${confidence}` : null;
  }
}

function coverageCopy(level: string) {
  if (level === "full") {
    return {
      title: "All checked sources were readable",
      note: "Code Hangar found usable information in every local source it checked for this view."
    };
  }
  if (level === "none") {
    return {
      title: "No readable change details were found",
      note: "This does not prove that nothing changed. It means the available local records did not contain changes Code Hangar could reconstruct."
    };
  }
  return {
    title: "Some information may be missing",
    note: "Code Hangar found useful changes, but one or more conversations, Git details or older records were incomplete. Treat this as a guide, not the whole history."
  };
}

function sessionDate(session: SessionDiscoveryCandidate) {
  return session.modifiedMs ? new Date(session.modifiedMs).toLocaleDateString() : "Date unavailable";
}

const RECAP_SESSION_PAGE_SIZE = 30;

function RecapSessionGroup({
  label,
  items,
  defaultOpen,
  selectedKey,
  onSelect,
  onContextMenu
}: {
  label: string;
  items: SessionDiscoveryCandidate[];
  defaultOpen: boolean;
  selectedKey: string;
  onSelect: (key: string) => void;
  onContextMenu?: (session: SessionDiscoveryCandidate, event: MouseEvent<HTMLElement>) => void;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [limit, setLimit] = useState(RECAP_SESSION_PAGE_SIZE);
  useEffect(() => setLimit(RECAP_SESSION_PAGE_SIZE), [items]);
  const visibleItems = items.slice(0, limit);
  return (
    <details className="recap-session-app-group" open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary><span>{label}</span><small>{items.length} conversation{items.length === 1 ? "" : "s"}</small></summary>
      {open ? (
        <div>
          {visibleItems.map((session) => {
            const key = `session:${sessionKey(session)}`;
            return (
              <button type="button" className={selectedKey === key ? "active" : ""} key={key} onClick={() => onSelect(key)} onContextMenu={onContextMenu ? (event) => onContextMenu(session, event) : undefined} data-help="Review this conversation's recorded changes. Right-click to open the conversation, show its local record or copy its path.">
                <MessageSquare size={15} />
                <span><strong>{session.displayName}</strong><small>{sessionDate(session)}</small></span>
              </button>
            );
          })}
          {visibleItems.length < items.length ? (
            <div className="progressive-list-controls recap-session-controls">
              <span>{visibleItems.length} of {items.length} shown</span>
              <button type="button" onClick={() => setLimit((current) => Math.min(items.length, current + RECAP_SESSION_PAGE_SIZE))}>Load more</button>
              <button type="button" onClick={() => setLimit(items.length)}>Show all</button>
            </div>
          ) : null}
        </div>
      ) : null}
    </details>
  );
}

export const RecapView = memo(function RecapView({
  projectId,
  sessions,
  onOpenSession,
  onSessionContextMenu,
  DetailLayer
}: {
  projectId: number;
  sessions: SessionDiscoveryCandidate[];
  onOpenSession: (session: SessionDiscoveryCandidate) => void;
  onSessionContextMenu?: (session: SessionDiscoveryCandidate, event: MouseEvent<HTMLElement>) => void;
  DetailLayer?: RecapDetailLayer;
}) {
  const orderedSessions = useMemo(
    () => [...sessions].sort((left, right) => (right.modifiedMs ?? 0) - (left.modifiedMs ?? 0)),
    [sessions]
  );
  const [selectedKey, setSelectedKey] = useState("combined");
  const [changeSet, setChangeSet] = useState<SessionChangeSet | null>(null);
  const [checkpoint, setCheckpoint] = useState<ProjectReviewCheckpoint | null>(null);
  const [ledger, setLedger] = useState<ReviewLedgerEntry[]>([]);
  const [scope, setScope] = useState<"since" | "all">("since");
  const [loading, setLoading] = useState(false);
  const [markingReviewed, setMarkingReviewed] = useState(false);
  const [exportingReceipt, setExportingReceipt] = useState(false);
  const [receiptStatus, setReceiptStatus] = useState<string | null>(null);
  const [ledgerWarning, setLedgerWarning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const scopedSessions = useMemo(
    () => scope === "all" || !checkpoint
      ? orderedSessions
      : orderedSessions.filter((session) => (session.modifiedMs ?? 0) > checkpoint.sessionCutoffMs),
    [checkpoint, orderedSessions, scope]
  );
  const currentPaths = useMemo(
    () => new Set(orderedSessions.map((session) => session.path.toLowerCase())),
    [orderedSessions]
  );
  const retainedLedger = useMemo(
    () => ledger.filter((entry) => !currentPaths.has(entry.sourceRef.toLowerCase()) && !entry.sourceRef.toLowerCase().startsWith("git:")),
    [currentPaths, ledger]
  );
  const sessionGroups = useMemo(() => {
    const groups = new Map<string, SessionDiscoveryCandidate[]>();
    for (const session of scopedSessions) {
      const label = sourceLabel(session);
      groups.set(label, [...(groups.get(label) ?? []), session]);
    }
    return [...groups.entries()]
      .map(([label, items]) => ({ label, items }))
      .sort((left, right) => (right.items[0]?.modifiedMs ?? 0) - (left.items[0]?.modifiedMs ?? 0));
  }, [scopedSessions]);
  const selectedSession = orderedSessions.find((session) => `session:${sessionKey(session)}` === selectedKey) ?? null;
  const selectedLedger = retainedLedger.find((entry) => `ledger:${entry.id}` === selectedKey) ?? null;
  const detailSessionPaths = useMemo(
    () => selectedSession ? [selectedSession.path] : scopedSessions.map((session) => session.path),
    [scopedSessions, selectedSession]
  );
  const detailSourceMode: RecapAiSourceMode = selectedSession ? "session" : selectedKey === "git" ? "git" : "combined";

  useEffect(() => {
    let current = true;
    setSelectedKey("combined");
    setScope("since");
    setLedger([]);
    setLedgerWarning(null);
    void Promise.all([
      api.projectReviewCheckpoint(projectId),
      api.projectReviewLedger(projectId, 100).catch(() => null)
    ])
      .then(([nextCheckpoint, nextLedger]) => {
        if (!current) return;
        setCheckpoint(nextCheckpoint);
        setLedger(nextLedger ?? []);
        setLedgerWarning(nextLedger === null
          ? "One or more older review records did not pass Code Hangar's integrity check. Current Git and available AI conversations are still reviewed normally."
          : null);
      })
      .catch((reason: unknown) => {
        if (current) setError(String(reason));
      });
    return () => { current = false; };
  }, [projectId]);

  useEffect(() => {
    let current = true;
    setLoading(true);
    setError(null);
    setChangeSet(null);
    const request = selectedKey === "combined"
      ? api.projectRecap(projectId, scopedSessions.map((session) => session.path))
      : selectedKey === "git"
        ? api.projectGitChangeSet(projectId)
        : selectedSession
          ? api.projectSessionChangeSet(projectId, selectedSession.path)
          : selectedLedger
            ? Promise.resolve(selectedLedger.changeSet)
            : api.projectRecap(projectId, scopedSessions.map((session) => session.path));
    void request
      .then((result) => {
        if (current) setChangeSet(result);
      })
      .catch((reason: unknown) => {
        if (current) setError(String(reason));
      })
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => {
      current = false;
    };
  }, [projectId, retry, scope, scopedSessions, selectedKey, selectedLedger, selectedSession]);

  async function markReviewed() {
    setMarkingReviewed(true);
    setError(null);
    try {
      const latestSession = orderedSessions.reduce((latest, session) => Math.max(latest, session.modifiedMs ?? 0), 0);
      const next = await api.markProjectReviewed(projectId, Math.max(Date.now(), latestSession));
      setCheckpoint(next);
      setScope("since");
      setSelectedKey("combined");
      setRetry((value) => value + 1);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setMarkingReviewed(false);
    }
  }

  async function exportReviewReceipt() {
    setReceiptStatus(null);
    setExportingReceipt(true);
    setError(null);
    try {
      const path = await api.pickReviewReceiptPath();
      if (!path) return;
      const result = await api.projectReviewReceiptExport(
        projectId,
        scopedSessions.map((session) => session.path),
        scope,
        path
      );
      setReceiptStatus(`Private-safe review receipt exported (${Math.max(1, Math.ceil(result.bytesWritten / 1024))} KiB).`);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setExportingReceipt(false);
    }
  }

  const selectedTitle = selectedKey === "combined"
    ? scope === "since" ? "New changes since your last review" : "Everything Code Hangar found"
    : selectedKey === "git" ? "File changes Git can see now"
      : selectedSession?.displayName ?? selectedLedger?.changeSet.sourceKind ?? "Saved review record";
  const selectedSource = selectedKey === "combined" ? "All local clues together"
    : selectedKey === "git" ? "Project files checked with Git"
      : selectedSession ? `AI conversation from ${sourceLabel(selectedSession)}` : "Older evidence saved by Code Hangar";

  return (
    <div className="project-home recap-home">
      <div className="project-home-intro recap-intro">
        <div className="recap-intro-heading">
          <div>
            <span>What changed</span>
            <h2>See what your AI tools changed, in plain language</h2>
          </div>
          <ConceptHelp concept="whatChanged" />
        </div>
        <p>Start with the overview. Then choose Git or one AI app only when you want to understand where a change came from.</p>
        <div className="recap-reading-map" aria-label="How to read What changed">
          <div>
            <FileDiff size={16} />
            <span><strong>1. Overall picture</strong><small>Combines the local clues into one review. Best place to begin.</small></span>
          </div>
          <div>
            <GitBranch size={16} />
            <span><strong>2. Project files now</strong><small>Shows differences Git can see on this computer, whether or not an AI session recorded them.</small></span>
          </div>
          <div>
            <MessageSquare size={16} />
            <span><strong>3. AI conversations</strong><small>{scopedSessions.length} conversation{scopedSessions.length === 1 ? "" : "s"} can explain the request and recorded edits behind a change.</small></span>
          </div>
        </div>
        {DetailLayer ? <div className="recap-ai-available"><Sparkles size={15} /><span>An AI explanation appears first for every overview, Git view and conversation you select.</span></div> : null}
        <div className="recap-readonly-note"><ShieldCheck size={15} /><span><strong>Read-only review.</strong> Nothing on this page can commit, push, change a branch or edit a project file.</span></div>
        <div className="recap-scope-bar">
          <div className="recap-scope-tabs" role="tablist" aria-label="Recap scope">
            <button type="button" role="tab" aria-selected={scope === "since"} className={scope === "since" ? "active" : ""} onClick={() => { setScope("since"); setSelectedKey("combined"); }}>New since I last reviewed</button>
            <button type="button" role="tab" aria-selected={scope === "all"} className={scope === "all" ? "active" : ""} onClick={() => { setScope("all"); setSelectedKey("combined"); }}>Everything found</button>
          </div>
          <div className="recap-scope-actions">
            <button type="button" className="secondary-button compact" disabled={exportingReceipt} onClick={() => void exportReviewReceipt()} data-help="Export a self-contained HTML summary with counts and evidence limits. Project identity, prompts, transcript text, diffs, file names and local paths are omitted.">
              <Download size={14} /> {exportingReceipt ? "Saving…" : "Save private review record"}
            </button>
            <button type="button" className="secondary-button compact" disabled={markingReviewed} onClick={() => void markReviewed()}>
              <CheckCircle2 size={14} /> {markingReviewed ? "Remembering…" : "I've reviewed this"}
            </button>
            <ConceptHelp concept="reviewPoint" />
          </div>
        </div>
        <small className="recap-checkpoint">
          {checkpoint ? `You last marked this project reviewed on ${new Date(checkpoint.reviewedAt).toLocaleString()}.` : "You have not marked a review point yet, so the new-changes view currently includes everything found."}
        </small>
        {receiptStatus ? <small className="recap-receipt-status" role="status">{receiptStatus}</small> : null}
        {ledgerWarning ? (
          <div className="recap-ledger-warning" role="status">
            <AlertTriangle size={15} />
            <span><strong>Older saved evidence was left out.</strong> {ledgerWarning}</span>
          </div>
        ) : null}
      </div>
      <div className="recap-layout">
        <aside className="recap-session-list" aria-label="Choose which local change clues to review">
          <div className="recap-list-heading">
            <span className="recap-list-title"><strong>Choose what to review</strong><small>{scopedSessions.length} AI conversation{scopedSessions.length === 1 ? "" : "s"}</small></span>
            <ConceptHelp concept="evidence" />
          </div>
          <section className="recap-source-section">
            <div className="recap-source-section-heading"><span>Overview</span></div>
            <button type="button" className={selectedKey === "combined" ? "active" : ""} onClick={() => setSelectedKey("combined")}>
              <FileDiff size={15} />
              <span><strong>{scope === "since" ? "All new clues together" : "All clues together"}</strong><small>Best place to start</small></span>
            </button>
          </section>

          <section className="recap-source-section">
            <div className="recap-source-section-heading"><span>Project files now</span><ConceptHelp concept="git" /></div>
            <button type="button" className={selectedKey === "git" ? "active" : ""} onClick={() => setSelectedKey("git")}>
              <GitBranch size={15} />
              <span><strong>Changes Git can see</strong><small>Saved and unsaved differences on this computer</small></span>
            </button>
          </section>

          <section className="recap-source-section recap-conversation-sources">
            <div className="recap-source-section-heading"><span>AI conversations</span><ConceptHelp concept="aiConversations" /></div>
            {sessionGroups.length === 0 ? <small className="recap-source-empty">No linked AI conversations in this time range.</small> : null}
            {sessionGroups.map((group, groupIndex) => (
              <RecapSessionGroup
                key={group.label}
                label={group.label}
                items={group.items}
                defaultOpen={groupIndex === 0}
                selectedKey={selectedKey}
                onSelect={setSelectedKey}
                onContextMenu={onSessionContextMenu}
              />
            ))}
          </section>

          {retainedLedger.length > 0 ? (
            <section className="recap-source-section recap-retained-sources">
              <div className="recap-source-section-heading"><span>Older saved reviews</span><ConceptHelp concept="reviewPoint" /></div>
              {retainedLedger.map((entry) => (
                <button type="button" className={selectedKey === `ledger:${entry.id}` ? "active" : ""} key={`ledger:${entry.id}`} onClick={() => setSelectedKey(`ledger:${entry.id}`)}>
                  <History size={15} />
                  <span><strong>Review saved on {new Date(entry.observedAt).toLocaleDateString()}</strong><small>{entry.changeSet.files.length} file{entry.changeSet.files.length === 1 ? "" : "s"}</small></span>
                </button>
              ))}
            </section>
          ) : null}
        </aside>
        <section className="recap-detail" aria-live="polite">
          <header className="recap-detail-heading">
            <div>
              <span>{selectedSource}</span>
              <h3>{selectedTitle}</h3>
            </div>
            <div className="recap-detail-actions">
              <ConceptHelp concept={selectedKey === "git" ? "git" : selectedSession ? "aiConversations" : "evidence"} />
              {selectedSession ? <button type="button" className="secondary-button compact" onClick={() => onOpenSession(selectedSession)}>
                <MessageSquare size={14} /> Open conversation
              </button> : null}
            </div>
          </header>
          {loading ? (
            <div className="recap-state"><RefreshCcw className="spin" size={20} /><strong>Reading local change records</strong></div>
          ) : error ? (
            <div className="recap-state error" role="alert">
              <AlertTriangle size={20} />
              <div><strong>Could not reconstruct this session</strong><span>{error}</span></div>
              <button type="button" className="secondary-button compact" onClick={() => setRetry((value) => value + 1)}>Retry</button>
            </div>
          ) : changeSet ? (
            DetailLayer && (selectedKey === "combined" || selectedKey === "git" || selectedSession)
              ? <Suspense fallback={<div className="recap-state"><RefreshCcw className="spin" size={20} /><strong>Preparing AI explanation tools</strong></div>}>
                  <DetailLayer
                    key={`${projectId}:${selectedKey}:${scope}`}
                    projectId={projectId}
                    sessionPaths={detailSessionPaths}
                    sourceMode={detailSourceMode}
                    changeSet={changeSet}
                  />
                </Suspense>
              : <RecapChangeSet changeSet={changeSet} />
          ) : null}
        </section>
      </div>
    </div>
  );
});

const RECAP_FILE_PAGE_SIZE = 20;

export function nextRecapFileLimit(current: number, total: number) {
  return Math.min(total, Math.max(RECAP_FILE_PAGE_SIZE, current + RECAP_FILE_PAGE_SIZE));
}

export function RecapChangeSet({ changeSet, renderEditAction }: { changeSet: SessionChangeSet; renderEditAction?: (filePath: string, editIndex: number) => ReactNode }) {
  const [fileLimit, setFileLimit] = useState(RECAP_FILE_PAGE_SIZE);
  useEffect(() => setFileLimit(RECAP_FILE_PAGE_SIZE), [changeSet]);
  const visibleFiles = changeSet.files.slice(0, fileLimit);
  const coverage = coverageCopy(changeSet.coverage.level);
  const coverageIcon = changeSet.coverage.level === "full"
    ? <CheckCircle2 size={17} />
    : changeSet.coverage.level === "none"
      ? <AlertTriangle size={17} />
      : <ShieldCheck size={17} />;
  return (
    <>
      <div className="recap-plain-summary">
        <div>
          <strong>What this means</strong>
          <p>Code Hangar found {changeSet.editCount} recorded change{changeSet.editCount === 1 ? "" : "s"} across {changeSet.files.length} file{changeSet.files.length === 1 ? "" : "s"}. Open a file below to see the request and exact lines.</p>
        </div>
        <ConceptHelp concept="evidence" />
      </div>
      <div className={`recap-coverage coverage-${changeSet.coverage.level}`}>
        {coverageIcon}
        <div>
          <strong>{coverage.title}</strong>
          <span>{coverage.note}</span>
          <details className="recap-technical-note">
            <summary>How Code Hangar worked this out</summary>
            <p><strong>Technical label:</strong> {changeSet.coverage.label}</p>
            <p>{changeSet.coverage.note}</p>
          </details>
        </div>
      </div>
      <div className="recap-metrics" aria-label="Recorded change totals">
        <span><strong>{changeSet.files.length}</strong> files touched</span>
        <span><strong>{changeSet.editCount}</strong> change groups</span>
        <span className="added"><strong>+{changeSet.addedLines}</strong> lines added</span>
        <span className="removed"><strong>-{changeSet.removedLines}</strong> lines removed</span>
        <ConceptHelp concept="lineChanges" />
      </div>
      {changeSet.redactedCount > 0 || changeSet.omittedRecords > 0 ? (
        <p className="recap-bounds">
          {changeSet.redactedCount > 0 ? `${changeSet.redactedCount} possible secret${changeSet.redactedCount === 1 ? "" : "s"} masked.` : ""}
          {changeSet.omittedRecords > 0 ? ` ${changeSet.omittedRecords} malformed or oversized record${changeSet.omittedRecords === 1 ? " was" : "s were"} omitted.` : ""}
        </p>
      ) : null}
      {changeSet.files.length === 0 ? (
        <div className="recap-no-changes">
          <FileDiff size={22} />
          <strong>No supported file edits were recorded</strong>
          <span>The conversation is still available. This result means “not proven”, not “nothing changed”.</span>
        </div>
      ) : (
        <div className="recap-files-block">
          <div className="recap-files-heading">
            <div><strong>Files and exact lines</strong><span>Open any file to see each recorded request, its source and the complete available line comparison.</span></div>
            <ConceptHelp concept="lineChanges" />
          </div>
          <div className="recap-files">
          {visibleFiles.map((file, fileIndex) => (
            <RecapFileDetails
              key={`${file.path}:${file.edits.length}:${file.addedLines}:${file.removedLines}`}
              file={file}
              initiallyOpen={fileIndex === 0}
              renderEditAction={renderEditAction}
            />
          ))}
          </div>
          {visibleFiles.length < changeSet.files.length ? (
            <div className="progressive-list-controls recap-file-controls" aria-label={`${visibleFiles.length} of ${changeSet.files.length} changed files shown`}>
              <span>{visibleFiles.length} of {changeSet.files.length} files shown</span>
              <button type="button" className="secondary-button compact" onClick={() => setFileLimit((current) => nextRecapFileLimit(current, changeSet.files.length))}>Load more</button>
              <button type="button" className="secondary-button compact" onClick={() => setFileLimit(changeSet.files.length)}>Show all file names</button>
            </div>
          ) : null}
        </div>
      )}
    </>
  );
}

function RecapFileDetails({
  file,
  initiallyOpen,
  renderEditAction
}: {
  file: SessionChangeSet["files"][number];
  initiallyOpen: boolean;
  renderEditAction?: (filePath: string, editIndex: number) => ReactNode;
}) {
  const [open, setOpen] = useState(initiallyOpen);
  return (
    <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>
        <FileDiff size={15} />
        <code>{file.path}</code>
        {file.reality ? <span className={`recap-reality ${file.reality.status}`} title={realityTitle(file.reality)}>{friendlyRealityLabel(file.reality.status, file.reality.label)}</span> : null}
        <span className="recap-file-counts"><b>+{file.addedLines}</b> <i>-{file.removedLines}</i></span>
      </summary>
      {open ? (
        <div className="recap-file-edits">
          {file.edits.map((edit, editIndex) => (
            <section className="recap-edit" key={`${edit.source}-${editIndex}`}>
              <div className="recap-edit-heading">
                <div>
                  <strong>{friendlyEditSummary(edit.summary)}</strong>
                  <span>{friendlyEditSource(edit.source)}</span>
                  {edit.provenance || edit.confidence ? <small>{friendlyConfidence(edit.confidence) ?? "Read from a local change record"}</small> : null}
                  <details className="recap-technical-source">
                    <summary>Show technical source</summary>
                    <code>{edit.source}</code>
                    {edit.provenance ? <span>{edit.provenance}</span> : null}
                    {edit.confidence ? <span>Evidence status: {edit.confidence}</span> : null}
                  </details>
                </div>
                <div className="recap-edit-metrics">
                  {renderEditAction?.(file.path, editIndex)}
                  {edit.reality ? <span className={`recap-reality ${edit.reality.status}`} title={realityTitle(edit.reality)}>{friendlyRealityLabel(edit.reality.status, edit.reality.label)}</span> : null}
                  <span>{edit.addedLines} added / {edit.removedLines} removed</span>
                </div>
              </div>
              {edit.request ? <blockquote><span>You asked</span><ExpandableText text={edit.request} threshold={220} expandLabel="Show the full request" /></blockquote> : null}
              {edit.hunks.map((hunk, hunkIndex) => (
                <div className="recap-hunk" key={`${hunk.header}-${hunkIndex}`}>
                  <div className="recap-hunk-header">{hunk.header}</div>
                  <pre>{hunk.lines.map((line, lineIndex) => (
                    <span className={`recap-diff-line ${line.kind}`} key={`${lineIndex}-${line.content}`}>
                      <b>{lineNumber(line)}</b>
                      <i>{line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "}</i>
                      <code>{line.content || " "}</code>
                    </span>
                  ))}</pre>
                </div>
              ))}
            </section>
          ))}
        </div>
      ) : null}
    </details>
  );
}
