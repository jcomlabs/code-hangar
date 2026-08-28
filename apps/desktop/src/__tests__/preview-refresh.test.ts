// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { refreshUnindexedPreview } from "../previewRefresh";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

describe("preview mode refresh routing", () => {
  it("keeps a cold shell-open preview on the isolated local reader before attachment", async () => {
    const refreshLocal = vi.fn(async (path: string) => `local:${path}`);
    const refreshAttached = vi.fn(async (projectId: number, path: string) => `project:${projectId}:${path}`);

    await expect(refreshUnindexedPreview(-1, "C:\\notes\\cold.md", refreshLocal, refreshAttached))
      .resolves.toBe("local:C:\\notes\\cold.md");
    await expect(refreshUnindexedPreview(0, "C:\\notes\\pending.md", refreshLocal, refreshAttached))
      .resolves.toBe("local:C:\\notes\\pending.md");

    expect(refreshLocal).toHaveBeenCalledTimes(2);
    expect(refreshAttached).not.toHaveBeenCalled();
  });

  it("uses the project-scoped reader only after a positive project id exists", async () => {
    const refreshLocal = vi.fn(async (path: string) => `local:${path}`);
    const refreshAttached = vi.fn(async (projectId: number, path: string) => `project:${projectId}:${path}`);

    await expect(refreshUnindexedPreview(42, "C:\\notes\\attached.md", refreshLocal, refreshAttached))
      .resolves.toBe("project:42:C:\\notes\\attached.md");

    expect(refreshLocal).not.toHaveBeenCalled();
    expect(refreshAttached).toHaveBeenCalledWith(42, "C:\\notes\\attached.md");
  });

  it("wires Rendered/Source mode changes through the guarded refresh route", () => {
    const start = appSource.indexOf("const backendMode = previewMode");
    const end = appSource.indexOf("// Seed the edit buffer", start);
    const modeRefresh = appSource.slice(start, end);

    expect(start).toBeGreaterThanOrEqual(0);
    expect(end).toBeGreaterThan(start);
    expect(modeRefresh).toContain("refreshUnindexedPreview(");
    expect(modeRefresh).toContain("api.openLocalFilePreviewFull(path, backendMode, previewPolicy)");
    expect(modeRefresh).toContain("(projectId, path) => api.openTargetPreview(projectId, path, backendMode, previewPolicy)");
    expect(modeRefresh).not.toContain("api.openTargetPreview(preview.projectId");
  });
});
