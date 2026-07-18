// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const quickOpenSource = appSource.slice(
  appSource.indexOf("function QuickOpenDialog("),
  appSource.indexOf("function useDialogFocusTrap(")
);
const commandSource = appSource.slice(appSource.indexOf("function CommandDialog("));

describe("Quick Open focus lifecycle", () => {
  it("uses the shared focus trap instead of autofocus stealing the opener", () => {
    expect(quickOpenSource).toContain("useDialogFocusTrap(onClose, returnFocus)");
    expect(quickOpenSource).toContain("data-dialog-initial-focus");
    expect(quickOpenSource).not.toContain("autoFocus");
  });

  it("gives Commands the same explicit return-focus contract", () => {
    expect(commandSource).toContain("useDialogFocusTrap(onClose, returnFocus)");
    expect(commandSource).not.toContain("previousFocusRef");
  });
});
