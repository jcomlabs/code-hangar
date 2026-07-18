import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import type { EditableValueSet, FileEditPreview } from "../types";
import { ChangeAccessDialog } from "../views/project-center/ChangeAccessDialog";
import { ChangeReviewDialog } from "../views/project-center/ChangeReviewDialog";
import { ValueEditorForm } from "../views/project-center/ValueEditor";

const dialog = readFileSync(new URL("../views/RewriteDialog.tsx", import.meta.url), "utf8");
const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const connectorApi = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const previousVersions = readFileSync(new URL("../views/project-center/PreviousVersions.tsx", import.meta.url), "utf8");
const projectCenter = readFileSync(new URL("../views/ProjectCenterView.tsx", import.meta.url), "utf8");
const previewPane = readFileSync(new URL("../views/project-center/PreviewPane.tsx", import.meta.url), "utf8");
const valueEditor = readFileSync(new URL("../views/project-center/ValueEditor.tsx", import.meta.url), "utf8");
const changeReview = readFileSync(new URL("../views/project-center/ChangeReviewDialog.tsx", import.meta.url), "utf8");
const backend = readFileSync(new URL("../../../../crates/hangar-api/src/lib.rs", import.meta.url), "utf8");
const editReviewBackend = readFileSync(new URL("../../../../crates/hangar-api/src/edit_review.rs", import.meta.url), "utf8");
const snapshotBackend = readFileSync(new URL("../../../../crates/hangar-api/src/edit_snapshot.rs", import.meta.url), "utf8");
const tauriBackend = readFileSync(new URL("../../src-tauri/src/main.rs", import.meta.url), "utf8");
const aiBackend = readFileSync(new URL("../../../../crates/hangar-api/src/ai_assist.rs", import.meta.url), "utf8");
const values = readFileSync(new URL("../../../../crates/hangar-api/src/value_edit.rs", import.meta.url), "utf8");
const projectReviewBackend = readFileSync(new URL("../../../../crates/hangar-api/src/project_review.rs", import.meta.url), "utf8");
const commentsPanel = readFileSync(new URL("../views/CommentsPanel.tsx", import.meta.url), "utf8");

describe("safe light editing contract", () => {
  it("renders source values as typed, individually saved controls", () => {
    const valueSet: EditableValueSet = {
      nodeId: 7,
      path: "fixture://settings.ts",
      format: "typescript",
      sourceHash: "fixture-hash",
      values: [
        { id: "name", path: "line 1", label: "App name", kind: "string", displayValue: "Hangar", rawValue: '"Hangar"', startByte: 12, endByte: 20 },
        { id: "count", path: "line 2", label: "Retries", kind: "number", displayValue: "3", rawValue: "3", startByte: 30, endByte: 31 },
        { id: "ready", path: "line 3", label: "Ready", kind: "boolean", displayValue: "true", rawValue: "true", startByte: 40, endByte: 44 },
        { id: "accent", path: "line 4", label: "Accent", kind: "color", displayValue: "#12abef", rawValue: '"#12abef"', startByte: 50, endByte: 59 }
      ]
    };
    const html = renderToStaticMarkup(createElement(ValueEditorForm, {
      valueSet,
      drafts: { count: "4" },
      savingId: null,
      reviewingId: null,
      onDraftChange: () => undefined,
      onReview: () => undefined
    }));

    expect(html).toContain('type="text"');
    expect(html).toContain('type="number"');
    expect(html).toContain('type="checkbox"');
    expect(html).toContain("value-color-swatch");
    expect(html).toContain('aria-label="Review change to Retries"');
    expect(html).not.toContain('aria-label="Review change to Retries" disabled');
    expect(html).toContain('aria-label="Review change to App name"');
  });

  it("requires a line review before applying one local file", () => {
    const preview: FileEditPreview = {
      nodeId: 7,
      projectId: 3,
      beforeHash: "before",
      afterHash: "after",
      addedLines: 1,
      removedLines: 1,
      hunks: [{
        header: "@@ -1,1 +1,1 @@",
        oldStart: 1,
        newStart: 1,
        lines: [
          { kind: "removed", oldLine: 1, newLine: null, content: "{\"ready\":false}" },
          { kind: "added", oldLine: null, newLine: 1, content: "{\"ready\":true}" }
        ]
      }],
      diffTruncated: false,
      validation: { status: "passed", label: "Valid JSON", note: "The edited file parses as JSON." },
      gitContext: { state: "modified", label: "Already modified", note: "Git already reports local changes.", otherChangedFiles: 2 }
    };
    const html = renderToStaticMarkup(createElement(ChangeReviewDialog, {
      preview,
      fileName: "settings.json",
      onClose: () => undefined,
      onApply: () => undefined
    }));

    expect(html).toContain("Review this change");
    expect(html).toContain("I reviewed every visible removed and added line");
    expect(html).toContain("+1");
    expect(html).toContain("-1");
    expect(html).toContain("Apply one file");
    expect(html).toMatch(/class="primary-button"[^>]*disabled/);
  });

  it("keeps project file changes locked until a named project is explicitly unlocked", () => {
    const html = renderToStaticMarkup(createElement(ChangeAccessDialog, {
      projectName: "My Project",
      onCancel: () => undefined,
      onUnlock: () => undefined
    }));

    expect(html).toContain("Project files are locked");
    expect(html).toContain("Type <strong>My Project</strong> to unlock");
    expect(html).toContain("never commits, pushes or changes a Git branch");
    expect(html).toMatch(/Unlock for this project<\/button>/);
    expect(html).toMatch(/class="primary-button"[^>]*disabled/);
    expect(projectCenter).toContain("Changes locked");
    expect(projectCenter).toContain("editor.available && changesUnlocked");
    expect(app).toContain("setUnlockedChangeProjectId(null)");
    expect(app).toContain("if (!changesUnlocked)");
    expect(valueEditor).toContain("if (!authorized)");
  });

  it("keeps project and file opening outside every project-write path", () => {
    const projectSelection = app.slice(
      app.indexOf("const selectProject = useCallback"),
      app.indexOf("const handleProjectSearchKeyDown = useCallback")
    );
    const fileOpening = app.slice(
      app.indexOf("const openNode = useCallback"),
      app.indexOf("const loadSessionPreviewContent = useCallback")
    );
    expect(projectSelection).toContain("loadProjectData(projectId)");
    expect(fileOpening).toContain("api.filePreview(");
    for (const source of [projectSelection, fileOpening]) {
      for (const forbidden of [
        "writeFileContent(",
        "applyValueEdit(",
        "applyAiSuggestion(",
        "editSnapshotRestore(",
        "mutationMoveStart(",
        "mutationFinalRemoveStart(",
        "rootsUnregister(",
        "projectsUnregister("
      ]) {
        expect(source).not.toContain(forbidden);
      }
    }
  });

  it("binds manual apply to the exact reviewed bytes and leaves Git read-only", () => {
    expect(previewPane).toContain("api.fileEditPreview");
    expect(previewPane).toContain("editor.onSave(review.afterHash)");
    expect(previewPane).not.toContain(">Save<");
    expect(backend).toContain("pub fn write_reviewed_file_content");
    expect(backend).toContain("reviewed_after_hash");
    expect(backend).toContain("draft changed after review");
    expect(tauriBackend).toContain("write_reviewed_file_content");
    expect(editReviewBackend).toContain("current_project_git_change_set");
    expect(editReviewBackend).not.toContain("Command::new");
    expect(changeReview).not.toMatch(/>\s*(Commit|Stage|Push|Create branch)\b/i);
    expect(projectReviewBackend).toContain('matches!(subcommand, "diff" | "status" | "rev-parse")');
    for (const command of ["commit", "push", "branch", "checkout", "reset", "restore"]) {
      expect(projectReviewBackend).toContain(`"${command}"`);
    }
  });

  it("previews values and previous-version restores through the same Lite diff", () => {
    expect(values).toContain("prepare_value_edit");
    expect(backend).toContain("pub fn preview_value_edit");
    expect(backend).toContain("pub fn apply_reviewed_value_edit");
    expect(tauriBackend).toContain("reviewed_after_hash: String");
    expect(previousVersions).toContain("api.editSnapshotCompare");
    expect(previousVersions).toContain("Review and restore version from");
    expect(previousVersions).toContain("onRestore={comparison.restoreId == null ? undefined");
    expect(changeReview).toContain("Restore this version");
    expect(changeReview).toContain("I reviewed the comparison and want to replace the current file");
  });

  it("keeps the lazy rewrite dialog behind a local suspense boundary", () => {
    const rewriteBranch = app.slice(app.indexOf("{connectorBuild && rewriteTarget ? ("), app.indexOf("{confirmRequest ? ("));
    expect(rewriteBranch).toContain("<Suspense");
    expect(rewriteBranch).toContain("<RewriteDialog");
    expect(rewriteBranch.indexOf("<Suspense")).toBeLessThan(rewriteBranch.indexOf("<RewriteDialog"));
    expect(rewriteBranch).toContain("Opening selected change...");
  });

  it("offers AI changes only for one explicit selection", () => {
    expect(dialog).toContain("Suggest a change to this selection");
    expect(dialog).toContain("What would change");
    expect(dialog).toContain("I reviewed the selected before and after text");
    expect(dialog).not.toContain('kind: "file"');
    expect(app).not.toContain("aiRewriteFile");
    expect(connectorApi).not.toContain("ai_rewrite_file");
    expect(aiBackend).not.toContain("ai_rewrite_file_for_path");
  });

  it("keeps provider generation read-only and applies the opaque proposal locally", () => {
    const providerBody = aiBackend.split("pub(crate) fn ai_rewrite_text_with_config")[1]?.split("#[cfg(test)]")[0] ?? "";
    expect(providerBody).toContain("hangar_ai::explain");
    for (const forbidden of ["write_file_with_snapshot", "write_file_content", "hangar_mutation", "Command::new"]) {
      expect(providerBody).not.toContain(forbidden);
    }
    const applyBody = backend.split("pub fn apply_ai_suggestion")[1]?.split("/// Optional AI-enriched")[0] ?? "";
    expect(applyBody).toContain("unique_selection_span");
    expect(applyBody).toContain("source_hash != pending.source_hash");
    expect(applyBody).toContain("content.push_str");
    expect(applyBody).toContain('"ai_suggestion"');
    expect(applyBody).toContain("Some(&pending.proposal.session_id)");
  });

  it("exposes source literals through an exact-span validity-checked editor", () => {
    expect(values).toContain("SourceScanner");
    expect(values).toContain("safe_hex_color");
    expect(values).toContain("validate_content_after_edit");
    expect(values).toContain("expected_source_hash");
    expect(values).toContain("source[start..end]");
    expect(values).toContain('"js" | "jsx"');
  });

  it("keeps session-level undo on the verified snapshot path", () => {
    expect(backend).toContain("pub fn undo_ai_edit_session");
    expect(backend).toContain("edit_snapshot::restore_ai_session(state, node_id, session_id)");
    expect(snapshotBackend).toContain("AND node_id = ?2");
    expect(dialog).toContain("Undo this AI change");
    expect(dialog).toContain("onUndo(applied.nodeId, applied.sessionId)");
    expect(previousVersions).toContain("onUndoAiSession(nodeId, sessionId)");
    expect(app).toContain("undoAiEditSession(nodeId, sessionId)");
    expect(connectorApi).toContain('{ nodeId, sessionId }');
    expect(tauriBackend).toContain("undo_ai_edit_session(&app_state, node_id, &session_id)");
  });

  it("requires a second explicit acknowledgement before deleting local review notes", () => {
    expect(commentsPanel).toContain("Delete this local comment permanently");
    expect(commentsPanel).toContain("deletingId !== commentId || !deleteAcknowledged");
    expect(commentsPanel).toMatch(/Delete comment<\/button>/);
  });
});
