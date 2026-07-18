export const DIALOG_FOCUSABLE_SELECTOR = [
  "button:not(:disabled)",
  "input:not(:disabled)",
  "select:not(:disabled)",
  "textarea:not(:disabled)",
  "a[href]",
  '[tabindex]:not([tabindex="-1"])'
].join(", ");

export const DIALOG_INITIAL_FOCUS_SELECTOR = "[data-dialog-initial-focus]";

export function nextDialogFocusIndex(
  focusableCount: number,
  currentIndex: number,
  reverse: boolean
): number {
  if (focusableCount <= 0) return -1;
  if (reverse) return currentIndex <= 0 ? focusableCount - 1 : currentIndex - 1;
  return currentIndex < 0 || currentIndex >= focusableCount - 1 ? 0 : currentIndex + 1;
}
