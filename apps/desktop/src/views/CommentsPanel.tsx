import { useCallback, useEffect, useState } from "react";
import { MessageSquare, Pencil, Trash2 } from "lucide-react";
import { api } from "../api";
import type { Comment } from "../types";
import { SectionTitle } from "../ui";
import { commentsPanelHint, formatCommentMeta, isAgentComment, validateCommentBody } from "../comments";

// A reusable panel that lists and edits the comments attached to one node
// (a project, folder or file). It is keyed entirely by node id and never reads
// the file body, so it is safe to show on sensitive/protected files too.
//
// `connectorBuild` gates the AI-apps mention in the help copy: only the Connector
// edition lets connected AI apps write comments, so the base (Local) edition keeps
// the hint clean of the app's own AI capabilities. Defaults to the clean base copy.
export function CommentsPanel({ nodeId, connectorBuild = false }: { nodeId: number | null; connectorBuild?: boolean }) {
  const [comments, setComments] = useState<Comment[]>([]);
  const [loading, setLoading] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const [deleteAcknowledged, setDeleteAcknowledged] = useState(false);

  const reload = useCallback(async (id: number) => {
    setLoading(true);
    try {
      setComments(await api.commentsForNode(id));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (nodeId == null) {
      setComments([]);
      return;
    }
    setEditingId(null);
    setDeletingId(null);
    setDeleteAcknowledged(false);
    setDraft("");
    void reload(nodeId);
  }, [nodeId, reload]);

  if (nodeId == null) return null;

  const addComment = async () => {
    const validation = validateCommentBody(draft);
    if (!validation.ok) {
      setError(validation.error ?? "A comment cannot be empty.");
      return;
    }
    setBusy(true);
    try {
      const created = await api.commentAdd(nodeId, validation.trimmed);
      setComments((current) => [...current, created]);
      setDraft("");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const saveEdit = async (commentId: number) => {
    const validation = validateCommentBody(editDraft);
    if (!validation.ok) {
      setError(validation.error ?? "A comment cannot be empty.");
      return;
    }
    setBusy(true);
    try {
      const updated = await api.commentEdit(commentId, validation.trimmed);
      setComments((current) => current.map((c) => (c.id === commentId ? updated : c)));
      setEditingId(null);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const removeComment = async (commentId: number) => {
    if (deletingId !== commentId || !deleteAcknowledged) return;
    setBusy(true);
    try {
      await api.commentDelete(commentId);
      setComments((current) => current.filter((c) => c.id !== commentId));
      setDeletingId(null);
      setDeleteAcknowledged(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="pane-section comments-panel">
      <SectionTitle
        icon={<MessageSquare size={15} />}
        label={comments.length > 0 ? `Comments (${comments.length})` : "Comments"}
      />
      <p className="comments-hint">{commentsPanelHint(connectorBuild)}</p>

      {comments.length === 0 && !loading ? (
        <div className="empty-state compact-empty">No comments yet.</div>
      ) : (
        <ul className="comment-list">
          {comments.map((comment) => {
            const agent = isAgentComment(comment);
            const editing = editingId === comment.id;
            return (
              <li key={comment.id} className={`comment-row${agent ? " comment-row-agent" : ""}`}>
                <div className="comment-meta">
                  <span>{formatCommentMeta(comment)}</span>
                  {agent ? <span className="comment-agent-badge">AI</span> : null}
                </div>
                {editing ? (
                  <div className="comment-edit">
                    <textarea
                      className="comment-textarea"
                      value={editDraft}
                      onChange={(event) => setEditDraft(event.target.value)}
                      rows={3}
                    />
                    <div className="comment-actions">
                      <button type="button" className="secondary-button" disabled={busy} onClick={() => void saveEdit(comment.id)}>
                        Save
                      </button>
                      <button type="button" className="secondary-button" disabled={busy} onClick={() => setEditingId(null)}>
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : (
                  <>
                    <div className="comment-body">{comment.body}</div>
                    {deletingId === comment.id ? (
                      <div className="comment-delete-confirm">
                        <label><input type="checkbox" checked={deleteAcknowledged} onChange={(event) => setDeleteAcknowledged(event.target.checked)} /> Delete this local comment permanently</label>
                        <div>
                          <button type="button" className="secondary-button" disabled={busy} onClick={() => { setDeletingId(null); setDeleteAcknowledged(false); }}>Cancel</button>
                          <button type="button" className="danger-button" disabled={busy || !deleteAcknowledged} onClick={() => void removeComment(comment.id)}>Delete comment</button>
                        </div>
                      </div>
                    ) : <div className="comment-actions">
                      {/* You can edit only your own comments (an AI app's words are kept
                          as written), but you can delete any comment to clean up. */}
                      {!agent ? (
                        <button
                          type="button"
                          className="comment-icon-button"
                          disabled={busy}
                          data-help="Edit your comment."
                          onClick={() => {
                            setEditingId(comment.id);
                            setEditDraft(comment.body);
                          }}
                        >
                          <Pencil size={13} /> Edit
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="comment-icon-button"
                        disabled={busy}
                        data-help={agent ? "Delete this AI-written comment. Your files are not affected." : "Delete your comment. Your files are not affected."}
                        onClick={() => { setDeletingId(comment.id); setDeleteAcknowledged(false); }}
                      >
                        <Trash2 size={13} /> Delete
                      </button>
                    </div>}
                  </>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <div className="comment-add">
        <textarea
          className="comment-textarea"
          placeholder="Add a comment…"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          rows={2}
        />
        <div className="comment-actions">
          <button
            type="button"
            className="primary-button"
            disabled={busy || !validateCommentBody(draft).ok}
            data-help="Attach this comment to the selected project, folder or file. Stored locally and encrypted."
            onClick={() => void addComment()}
          >
            Add comment
          </button>
        </div>
      </div>

      {error ? <div className="comment-error">{error}</div> : null}
    </section>
  );
}
