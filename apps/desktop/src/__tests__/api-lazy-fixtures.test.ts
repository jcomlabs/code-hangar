// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import {
  api,
  browserMutationAcceptanceEnabled,
  call,
  createLazyAsyncApi,
  optionalCommand
} from "../api";

afterEach(() => {
  invokeMock.mockReset();
  vi.unstubAllGlobals();
});

describe("lazy fixture API boundary", () => {
  it("does not run the loader on property access and loads it only once on invocation", async () => {
    let loadCount = 0;
    const concrete = {
      factor: 2,
      async multiply(value: number) {
        return this.factor * value;
      }
    };
    const lazy = createLazyAsyncApi(async () => {
      loadCount += 1;
      return concrete;
    });

    const detachedMethod = lazy.multiply;
    expect(loadCount).toBe(0);

    await expect(detachedMethod(3)).resolves.toBe(6);
    await expect(lazy.multiply(4)).resolves.toBe(8);
    expect(loadCount).toBe(1);
  });

  it("retries the loader after a rejected attempt and memoizes the successful retry", async () => {
    let loadCount = 0;
    const lazy = createLazyAsyncApi(async () => {
      loadCount += 1;
      if (loadCount === 1) {
        throw new Error("temporary chunk failure");
      }
      return {
        async value() {
          return "available";
        }
      };
    });

    await expect(lazy.value()).rejects.toThrow("temporary chunk failure");
    await expect(lazy.value()).resolves.toBe("available");
    await expect(lazy.value()).resolves.toBe("available");
    expect(loadCount).toBe(2);
  });

  it("is not thenable and does not load fixtures during object introspection", async () => {
    let loadCount = 0;
    const lazy = createLazyAsyncApi(async () => {
      loadCount += 1;
      return {
        async value() {
          return "available";
        }
      };
    });

    const resolved = await Promise.resolve(lazy);
    expect(resolved).toBe(lazy);
    expect(JSON.stringify(lazy)).toBe("{}");
    expect(String(lazy)).toBe("[object Object]");
    expect(Reflect.get(lazy, Symbol.toStringTag)).toBeUndefined();
    expect(loadCount).toBe(0);
  });

  it("does not invoke a fallback after a successful Tauri command", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    invokeMock.mockResolvedValue("desktop result");
    const fallback = vi.fn(async () => "fixture result");

    await expect(call("example_command", undefined, fallback)).resolves.toBe("desktop result");
    expect(invokeMock).toHaveBeenCalledWith("example_command", undefined);
    expect(fallback).not.toHaveBeenCalled();
  });

  it("falls back only for the exact missing Tauri command, not domain not-found errors", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    const fallback = vi.fn(async () => "fallback result");

    invokeMock.mockRejectedValueOnce(new Error("Project was not found."));
    await expect(optionalCommand("recovery_pending", undefined, fallback)).rejects.toThrow("Project was not found.");
    expect(fallback).not.toHaveBeenCalled();

    invokeMock.mockRejectedValueOnce("Command recovery_pending not found");
    await expect(optionalCommand("recovery_pending", undefined, fallback)).resolves.toBe("fallback result");
    expect(fallback).toHaveBeenCalledTimes(1);
  });

  it("uses small disabled-edition fallbacks for mutation startup probes", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    invokeMock.mockImplementation((command: string) => Promise.reject(`Command ${command} not found`));

    await expect(api.mutationModeStatus()).resolves.toBe(false);
    await expect(api.mutationFinalRemoveEnabled()).resolves.toBe(false);
    await expect(api.recoveryPending()).resolves.toEqual({
      enabled: false,
      pending: false,
      operations: [],
      message: "Recovery is not available in this edition."
    });
    await expect(api.mutationActivityLog()).resolves.toEqual({
      enabled: false,
      operations: [],
      items: [],
      backups: [],
      storedEntries: [],
      message: "Mutation history is not available in this edition."
    });
    await expect(api.mutationRecoveryDashboard()).resolves.toMatchObject({
      available: false,
      finalRemove: { state: "unknown" }
    });
    await expect(api.mutationRecoveryDashboard()).resolves.toHaveProperty(
      "message",
      expect.stringContaining("cannot prove whether a final-cleanup batch is active or interrupted")
    );
    await expect(api.mutationFinalRemovePreview({ kind: "allEligible" })).rejects.toThrow(
      "Project and batch final cleanup is not available in this backend yet"
    );
  });

  it("enables mutation acceptance only for the explicit browser development fixture", async () => {
    vi.stubGlobal("window", { location: { search: "?acceptanceMutation=fixture" } });

    expect(browserMutationAcceptanceEnabled()).toBe(true);
    await expect(api.mutationModeStatus()).resolves.toBe(true);
    expect(browserMutationAcceptanceEnabled("?acceptanceMutation=true")).toBe(false);

    vi.stubGlobal("window", {
      __TAURI_INTERNALS__: {},
      location: { search: "?acceptanceMutation=fixture" }
    });
    expect(browserMutationAcceptanceEnabled()).toBe(false);
  });

  it("uses a digest-bound one-shot synthetic batch in browser fixture mode", async () => {
    await expect(api.mutationRecoveryDashboard()).resolves.toMatchObject({
      available: true,
      finalRemove: { state: "idle", jobId: null }
    });
    const preview = await api.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    expect(preview).toMatchObject({ maxDeleteObjects: 2, blockedObjects: 1, archivesRetained: true });
    expect(preview.previewDigest).toMatch(/^v2:[0-9a-f]{64}$/u);
    const confirmation = await api.mutationFinalRemoveConfirm(
      preview.previewId,
      preview.previewDigest,
      preview.eligibleTopologyGroupIds
    );
    expect(confirmation.token).toMatch(/^[0-9a-f]{64}$/u);
    const request = {
      previewId: preview.previewId,
      previewDigest: preview.previewDigest,
      selectedTopologyGroupIds: preview.eligibleTopologyGroupIds,
      confirmationToken: confirmation.token
    };
    const started = await api.mutationFinalRemoveBatchStart(request);
    const status = await api.mutationFinalRemoveBatchStatus(started.jobId);
    expect(status.result).toMatchObject({ status: "completed", archiveRetained: true });
    await expect(api.mutationFinalRemoveBatchStart(request)).rejects.toThrow("missing, expired or bound to another preview");
  });

  it("rejects topology authority copied from a different fixture preview", async () => {
    const markdownPreview = await api.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-markdown-project" });
    const gitPreview = await api.mutationFinalRemovePreview({ kind: "project", groupId: "fixture-git-project" });
    await expect(api.mutationFinalRemoveConfirm(
      markdownPreview.previewId,
      markdownPreview.previewDigest,
      gitPreview.eligibleTopologyGroupIds
    )).rejects.toThrow("no eligible topology groups");
    await expect(api.mutationFinalRemoveConfirm(
      markdownPreview.previewId,
      `v2:${"f".repeat(64)}`,
      markdownPreview.eligibleTopologyGroupIds
    )).rejects.toThrow("no longer valid");
  });

  it("keeps fixtures behind a local dynamic import in the source graph", () => {
    const source = readFileSync(new URL("../api.ts", import.meta.url), "utf8");

    expect(source).not.toMatch(/import\s+\{\s*fixtureApi\s*\}\s+from\s+["']\.\/fixtures["']/u);
    expect(source).toContain('import("./fixtures")');
    expect(source).not.toContain("fixtureApi.mutationModeStatus");
    expect(source).not.toContain("fixtureApi.mutationFinalRemoveEnabled");
    expect(source).not.toContain("fixtureApi.recoveryPending");
    expect(source).not.toContain("fixtureApi.mutationActivityLog");
  });
});
