import { invoke } from "@tauri-apps/api/core";
import React, { useState } from "react";
import ReactDOM from "react-dom/client";

import { ErrorBoundary } from "./ErrorBoundary";
import { ChangeAccessDialog } from "./views/project-center/ChangeAccessDialog";
import { ValueEditor } from "./views/project-center/ValueEditor";
import "./styles.css";

interface HarnessContext {
  projectId: number;
  nodeId: number;
  projectName: string;
}

interface UiObservation {
  phases: string[];
  editRemoved: string[];
  editAdded: string[];
  compareRemoved: string[];
  compareAdded: string[];
}

function Harness({ context }: { context: HarnessContext }) {
  const [accessDialogOpen, setAccessDialogOpen] = useState(false);
  const [unlocked, setUnlocked] = useState(false);
  const [status, setStatus] = useState("Project changes are locked.");

  return (
    <main className="p1-e2e-shell">
      <header>
        <strong>P1 local edit journey</strong>
        <span>{context.projectName}</span>
      </header>
      {!unlocked ? (
        <button
          data-testid="p1-unlock"
          type="button"
          onClick={() => setAccessDialogOpen(true)}
        >
          Unlock changes
        </button>
      ) : (
        <ValueEditor
          projectId={context.projectId}
          nodeId={context.nodeId}
          authorized
          onFileMutated={() => undefined}
          setStatusText={setStatus}
        />
      )}
      <output data-testid="p1-status">{status}</output>
      {accessDialogOpen ? (
        <ChangeAccessDialog
          projectName={context.projectName}
          onCancel={() => setAccessDialogOpen(false)}
          onUnlock={() => {
            setUnlocked(true);
            setAccessDialogOpen(false);
            setStatus("Project changes unlocked for this journey.");
          }}
        />
      ) : null}
    </main>
  );
}

function delay(ms: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, ms));
}

async function waitFor<T extends Element>(
  description: string,
  select: () => T | null,
  timeoutMs = 12_000
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const match = select();
    if (match) return match;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${description}.`);
}

function buttonWithText(text: string, root: ParentNode = document): HTMLButtonElement | null {
  return Array.from(root.querySelectorAll("button")).find(
    (button) => button.textContent?.trim() === text
  ) as HTMLButtonElement | undefined ?? null;
}

function setTextInput(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (!setter) throw new Error("The native text-input setter is unavailable.");
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function diffLines(dialog: Element, kind: "removed" | "added") {
  return Array.from(dialog.querySelectorAll(`.change-diff-line.${kind} code`)).map(
    (line) => line.textContent ?? ""
  );
}

function expectExact(actual: string[], expected: string[], label: string) {
  if (actual.length !== expected.length || actual.some((value, index) => value !== expected[index])) {
    throw new Error(`${label} mismatch: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}.`);
  }
}

async function runJourney(context: HarnessContext): Promise<UiObservation> {
  const observed: UiObservation = {
    phases: [],
    editRemoved: [],
    editAdded: [],
    compareRemoved: [],
    compareAdded: []
  };

  (await waitFor("locked-project action", () => document.querySelector<HTMLButtonElement>("[data-testid='p1-unlock']"))).click();
  const unlockDialog = await waitFor("production unlock dialog", () => document.querySelector<HTMLElement>("[role='dialog'][aria-label='Unlock project changes']"));
  unlockDialog.querySelector<HTMLInputElement>(".change-access-check input")?.click();
  const projectNameInput = unlockDialog.querySelector<HTMLInputElement>(".change-access-name input");
  if (!projectNameInput) throw new Error("Unlock project-name input was not rendered.");
  setTextInput(projectNameInput, context.projectName);
  const unlockButton = await waitFor("enabled unlock confirmation", () => {
    const button = buttonWithText("Unlock for this project", unlockDialog);
    return button && !button.disabled ? button : null;
  });
  unlockButton.click();
  observed.phases.push("unlock");

  const valueRow = await waitFor("editable enabled value", () =>
    Array.from(document.querySelectorAll<HTMLElement>(".value-row")).find((row) =>
      row.querySelector("label")?.textContent?.toLowerCase().includes("enabled")
    ) ?? null
  );
  const valueToggle = valueRow.querySelector<HTMLInputElement>(".value-toggle input[type='checkbox']");
  if (!valueToggle || valueToggle.checked) throw new Error("Expected the real enabled value to start false.");
  valueToggle.click();
  const reviewButton = valueRow.querySelector<HTMLButtonElement>("button[aria-label^='Review change to']");
  if (!reviewButton || reviewButton.disabled) throw new Error("Value review did not become available after the edit.");
  reviewButton.click();
  observed.phases.push("edit");

  const editDialog = await waitFor("exact value diff review", () => document.querySelector<HTMLElement>(".change-review-dialog:not(.version-compare-dialog)"));
  observed.editRemoved = diffLines(editDialog, "removed");
  observed.editAdded = diffLines(editDialog, "added");
  expectExact(observed.editRemoved, ['  "enabled": false,'], "apply review removed lines");
  expectExact(observed.editAdded, ['  "enabled": true,'], "apply review added lines");
  observed.phases.push("review-exact-diff");
  editDialog.querySelector<HTMLInputElement>(".change-review-actions input[type='checkbox']")?.click();
  const applyButton = buttonWithText("Apply one value", editDialog);
  if (!applyButton || applyButton.disabled) throw new Error("Reviewed Apply remained disabled.");
  applyButton.click();
  await waitFor("applied value status", () => {
    const output = document.querySelector<HTMLOutputElement>("[data-testid='p1-status']");
    return output?.textContent?.includes("Value saved") ? output : null;
  });
  observed.phases.push("apply");

  const versionsToggle = await waitFor("Previous versions disclosure", () => buttonWithText("Previous versions") ?? document.querySelector<HTMLButtonElement>(".previous-versions-toggle"));
  versionsToggle.click();
  const versionRow = await waitFor("saved Value edit version", () =>
    Array.from(document.querySelectorAll<HTMLElement>(".previous-version-row")).find((row) =>
      row.textContent?.includes("Value edit")
    ) ?? null
  );
  observed.phases.push("previous-versions");

  const compareButton = buttonWithText("Compare", versionRow);
  if (!compareButton) throw new Error("Previous Versions did not expose Compare.");
  compareButton.click();
  let compareDialog = await waitFor("read-only previous-version comparison", () => document.querySelector<HTMLElement>(".version-compare-dialog"));
  observed.compareRemoved = diffLines(compareDialog, "removed");
  observed.compareAdded = diffLines(compareDialog, "added");
  expectExact(observed.compareRemoved, ['  "enabled": true,'], "version compare removed lines");
  expectExact(observed.compareAdded, ['  "enabled": false,'], "version compare added lines");
  const doneButton = buttonWithText("Done", compareDialog);
  if (!doneButton) throw new Error("Read-only comparison did not expose Done.");
  doneButton.click();
  await waitFor("closed comparison", () => document.querySelector(".version-compare-dialog") ? null : document.body);
  observed.phases.push("compare");

  const restoreReviewButton = versionRow.querySelector<HTMLButtonElement>("button[aria-label^='Review and restore version from']");
  if (!restoreReviewButton) throw new Error("Previous Versions did not expose reviewed restore.");
  restoreReviewButton.click();
  compareDialog = await waitFor("restore comparison", () => document.querySelector<HTMLElement>(".version-compare-dialog"));
  expectExact(diffLines(compareDialog, "removed"), observed.compareRemoved, "restore review removed lines");
  expectExact(diffLines(compareDialog, "added"), observed.compareAdded, "restore review added lines");
  compareDialog.querySelector<HTMLInputElement>(".change-review-actions input[type='checkbox']")?.click();
  const restoreButton = buttonWithText("Restore this version", compareDialog);
  if (!restoreButton || restoreButton.disabled) throw new Error("Reviewed restore remained disabled.");
  restoreButton.click();
  await waitFor("restored version status", () => {
    const output = document.querySelector<HTMLOutputElement>("[data-testid='p1-status']");
    return output?.textContent?.includes("Previous version restored") ? output : null;
  });
  observed.phases.push("restore");
  return observed;
}

async function main() {
  let observed: UiObservation | null = null;
  try {
    const context = await invoke<HarnessContext>("p1_e2e_context");
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <ErrorBoundary>
          <Harness context={context} />
        </ErrorBoundary>
      </React.StrictMode>
    );
    observed = await runJourney(context);
    await invoke("p1_e2e_complete", { observed });
  } catch (cause) {
    const error = cause instanceof Error ? cause.stack ?? cause.message : String(cause);
    await invoke("p1_e2e_complete", { observed, error });
  }
}

void main();
