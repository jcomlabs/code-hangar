import { afterEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { api } from "../api";

afterEach(() => {
  invokeMock.mockReset();
  vi.unstubAllGlobals();
});

describe("project membership IPC contracts", () => {
  it("carries the selected project through a sensitive reveal", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    invokeMock.mockResolvedValue({});
    const policy = {
      allowSensitiveReveal: true,
      relaxNonStrongProtectedPreview: false
    };

    await api.fileReveal(71, "source", policy, 42);

    expect(invokeMock).toHaveBeenCalledWith("file_reveal", {
      nodeId: 71,
      projectId: 42,
      mode: "source",
      policy
    });
  });

  it("carries the selected project through pin and unpin", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    invokeMock.mockResolvedValue(undefined);

    await api.pinItem(71, "file", 42);
    await api.unpinItem(71, "file", 42);

    expect(invokeMock).toHaveBeenNthCalledWith(1, "pin_item", {
      nodeId: 71,
      itemKind: "file",
      projectId: 42
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "unpin_item", {
      nodeId: 71,
      itemKind: "file",
      projectId: 42
    });
  });

  it("scopes relationships and orphan status to the selected project membership", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    invokeMock.mockResolvedValue({});

    await api.nodeRelationships(42, 71);
    await api.nodeOrphanStatus(42, 71);

    expect(invokeMock).toHaveBeenNthCalledWith(1, "node_relationships", {
      projectId: 42,
      nodeId: 71
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "node_orphan_status", {
      projectId: 42,
      nodeId: 71
    });
  });
});
