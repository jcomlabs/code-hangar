// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const discoverSource = readFileSync(new URL("../views/DiscoverView.tsx", import.meta.url), "utf8");
const inspectorSource = readFileSync(new URL("../views/InspectorView.tsx", import.meta.url), "utf8");

describe("cross-project document-hit routing contract", () => {
  it("keeps the project owner attached from the result row to preview loading", () => {
    expect(discoverSource).toContain('key={`${hit.projectId}:${hit.nodeId}`}');
    expect(discoverSource).toContain("openDocumentHit(hit.nodeId, hit.projectId)");
    expect(appSource).toContain('const cacheKey = `${options?.projectId ?? "any"}:${nodeId}:${requestedMode}`;');
    expect(appSource).toContain("previewPolicy, options?.projectId");
  });

  it("reopens a positive-id tab with its project and replaces ambiguous node-id collisions", () => {
    expect(appSource).toContain("projectId: options?.projectId ?? tab?.projectId");
    expect(appSource).toContain("const existingIndex = current.findIndex((tab) => tab.nodeId === nodeId);");
    expect(appSource).toContain("index === existingIndex ? nextTab : tab");
  });

  it("keeps the project owner when the active preview mode changes", () => {
    expect(appSource).toContain("api.filePreview(preview.nodeId, backendMode, false, previewPolicy, preview.projectId)");
    expect(appSource).toContain("api.fileReveal(preview.nodeId, backendMode, previewPolicy, preview.projectId)");
  });

  it("keeps owners on catalog, relationship and pin actions", () => {
    expect(discoverSource).toContain("openNode(candidate.nodeId, candidate.projectId)");
    expect(discoverSource).toContain("openNode(member.nodeId, member.projectId)");
    expect(inspectorSource).toContain("openNode(relationship.nodeId, relationship.projectId)");
    expect(appSource).toContain('api.pinItem(nodeId, "file", projectId)');
    expect(appSource).toContain('api.unpinItem(nodeId, "file", projectId)');
  });
});
