import { describe, expect, it } from "vitest";
import {
  disposeShellViewerSafely,
  shellOpenImmediateMode,
  shellOpenPreviewReadOwnsFocus,
  shellOpenRequestOwnsFocus,
  shellPreviewMode,
  shellScanFailedToOpen,
  shellScanIsPending
} from "./shellOpen";

describe("Windows shell-open routing", () => {
  it("renders Markdown extensions and opens other text as source", () => {
    expect(shellPreviewMode(String.raw`C:\Work\README.md`)).toBe("rendered");
    expect(shellPreviewMode(String.raw`C:\Work\notes.MARKDOWN`)).toBe("rendered");
    expect(shellPreviewMode(String.raw`C:\Work\page.mdx`)).toBe("rendered");
    expect(shellPreviewMode(String.raw`C:\Work\src\main.ts`)).toBe("source");
  });

  it("waits only for active scan states", () => {
    expect(shellScanIsPending({ state: "queued" })).toBe(true);
    expect(shellScanIsPending({ state: "running" })).toBe(true);
    expect(shellScanIsPending({ state: "cancelling" })).toBe(true);
    expect(shellScanIsPending({ state: "completed" })).toBe(false);
    expect(shellScanIsPending({ state: "partial" })).toBe(false);
  });

  it("blocks a not-yet-indexed open after failed or cancelled scans", () => {
    expect(shellScanFailedToOpen({ state: "failed" })).toBe(true);
    expect(shellScanFailedToOpen({ state: "cancelled" })).toBe(true);
    expect(shellScanFailedToOpen({ state: "partial" })).toBe(false);
  });

  it("opens known members in-project, unknown files in Viewer, and asks only for unknown folders", () => {
    expect(shellOpenImmediateMode({ knownProjectRoot: String.raw`C:\Work\Project`, targetKind: "file" })).toBe("known");
    expect(shellOpenImmediateMode({ knownProjectRoot: null, targetKind: "file" })).toBe("viewer");
    expect(shellOpenImmediateMode({ knownProjectRoot: null, targetKind: "folder" })).toBeNull();
  });

  it("lets only the newest untouched shell preview take foreground ownership", () => {
    expect(shellOpenRequestOwnsFocus(2, 2, 9, 9, 4, 4)).toBe(true);
    expect(shellOpenRequestOwnsFocus(1, 2, 8, 9, 4, 4)).toBe(false);
    expect(shellOpenRequestOwnsFocus(2, 2, 9, 10, 4, 4)).toBe(false);
    expect(shellOpenRequestOwnsFocus(2, 2, 9, 9, 4, 5)).toBe(false);
  });

  it("rejects a delayed first read after explicit navigation", async () => {
    let releaseRead: (() => void) | undefined;
    const delayedRead = new Promise<void>((resolve) => {
      releaseRead = resolve;
    });
    const initialSelection = 7;
    let currentSelection = initialSelection;

    const completion = delayedRead.then(() => shellOpenPreviewReadOwnsFocus(
      3,
      3,
      11,
      11,
      initialSelection,
      currentSelection
    ));
    currentSelection += 1; // showOverview/selectProject while the read is pending
    releaseRead?.();

    await expect(completion).resolves.toBe(false);
  });

  it("rejects a delayed first read after view-only or same-project navigation", async () => {
    let releaseRead: (() => void) | undefined;
    const delayedRead = new Promise<void>((resolve) => {
      releaseRead = resolve;
    });
    const requestIntent = 20;
    let currentIntent = requestIntent;

    const completion = delayedRead.then(() => shellOpenPreviewReadOwnsFocus(
      4,
      4,
      requestIntent,
      currentIntent,
      12,
      12
    ));
    currentIntent += 1; // Overview, Discover, or same-id selectProject
    releaseRead?.();

    await expect(completion).resolves.toBe(false);
  });

  it("discards a temporary Viewer even when its terminal scan status was pruned", async () => {
    const calls: string[] = [];
    await disposeShellViewerSafely(
      { rootId: 42, scanJobId: "pruned", temporary: true },
      {
        scanStatus: async () => {
          calls.push("status");
          throw new Error("Unknown scan job");
        },
        scanCancel: async () => calls.push("cancel"),
        waitForScan: async () => calls.push("wait"),
        discardInvestigation: async () => calls.push("discard")
      }
    );
    expect(calls).toEqual(["status", "discard"]);
  });

  it("cancels and waits for an active Viewer scan before discarding", async () => {
    const calls: string[] = [];
    await disposeShellViewerSafely(
      { rootId: 43, scanJobId: "active", temporary: true },
      {
        scanStatus: async () => {
          calls.push("status");
          return { state: "running" };
        },
        scanCancel: async () => calls.push("cancel"),
        waitForScan: async () => calls.push("wait"),
        discardInvestigation: async () => calls.push("discard")
      }
    );
    expect(calls).toEqual(["status", "cancel", "wait", "discard"]);
  });

  it("surfaces a fail-closed discard refusal after best-effort scan cleanup", async () => {
    const calls: string[] = [];
    await expect(disposeShellViewerSafely(
      { rootId: 44, scanJobId: "unknown", temporary: true },
      {
        scanStatus: async () => {
          calls.push("status");
          throw new Error("Unknown scan job");
        },
        scanCancel: async () => calls.push("cancel"),
        waitForScan: async () => calls.push("wait"),
        discardInvestigation: async () => {
          calls.push("discard");
          throw new Error("root still has a running job");
        }
      }
    )).rejects.toThrow(/root still has a running job.*Unknown scan job/);
    expect(calls).toEqual(["status", "discard"]);
  });
});
