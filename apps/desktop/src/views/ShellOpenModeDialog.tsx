import { Eye, FolderOpen, Info, Radar, X } from "lucide-react";
import { useCallback, useLayoutEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { DIALOG_FOCUSABLE_SELECTOR, DIALOG_INITIAL_FOCUS_SELECTOR, nextDialogFocusIndex } from "../dialogFocus";
import type { OpenTargetInspection, ShellOpenMode } from "../types";
import { displayLocalPath } from "../ui";

type ShellOpenChoice = Exclude<ShellOpenMode, "known"> | "cancel";

export default function ShellOpenModeDialog({
  inspection,
  onChoose
}: {
  inspection: OpenTargetInspection;
  onChoose: (choice: ShellOpenChoice) => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(() => onChoose("cancel"));
  closeRef.current = () => onChoose("cancel");

  useLayoutEffect(() => {
    const previousFocus = document.activeElement;
    const initialControl = dialogRef.current?.querySelector<HTMLElement>(DIALOG_INITIAL_FOCUS_SELECTOR)
      ?? dialogRef.current?.querySelector<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR);
    initialControl?.focus({ preventScroll: true });
    return () => {
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) {
        previousFocus.focus({ preventScroll: true });
      }
    };
  }, []);

  const cancel = useCallback(() => closeRef.current(), []);
  const onDialogKeyDown = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeRef.current();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR) ?? []);
    const nextIndex = nextDialogFocusIndex(
      focusable.length,
      focusable.indexOf(document.activeElement as HTMLElement),
      event.shiftKey
    );
    event.preventDefault();
    if (nextIndex >= 0) focusable[nextIndex]?.focus();
  }, []);
  const itemLabel = inspection.targetKind === "folder" ? "folder" : "file";

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={cancel}>
      <div
        ref={dialogRef}
        className="command-dialog shell-open-mode-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shell-open-mode-title"
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onDialogKeyDown}
      >
        <header className="dialog-header">
          <div>
            <strong id="shell-open-mode-title">This {itemLabel} is not in a known project</strong>
            <span title={inspection.inputPath}>{displayLocalPath(inspection.inputPath)}</span>
          </div>
          <button className="icon-button" type="button" onClick={cancel} aria-label="Cancel opening this item">
            <X size={16} />
          </button>
        </header>
        <div className="shell-open-choice-grid">
          <button data-dialog-initial-focus type="button" className="shell-open-choice" onClick={() => onChoose("viewer")}>
            <Eye size={22} />
            <span>
              <strong>Open in Viewer</strong>
              <small>Open only <b>{displayLocalPath(inspection.viewerRoot)}</b>. Keep it out of Projects and do not aggregate AI sessions.</small>
            </span>
          </button>
          <button type="button" className="shell-open-choice recommended" onClick={() => onChoose("automatic")}>
            <Radar size={22} />
            <span>
              <strong>Find project root automatically <em>Recommended</em></strong>
              <small>Use <b>{displayLocalPath(inspection.automaticProjectRoot)}</b> as the detected root, scan it and correlate its local AI-app sessions.</small>
            </span>
          </button>
          <button type="button" className="shell-open-choice" onClick={() => onChoose("manual")}>
            <FolderOpen size={22} />
            <span>
              <strong>Choose root manually</strong>
              <small>Select the project root yourself. Code Hangar verifies that it contains this {itemLabel} before scanning.</small>
            </span>
          </button>
        </div>
        <p className="shell-open-choice-note"><Info size={15} /> Nothing is registered or scanned until you choose a mode.</p>
      </div>
    </div>
  );
}
