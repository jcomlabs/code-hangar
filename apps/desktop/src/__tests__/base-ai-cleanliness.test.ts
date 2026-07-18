import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { commentsPanelHint } from "../comments";
import { CommentsPanel } from "../views/CommentsPanel";
import { fixtureApi } from "../fixtures";

// Regression guard for the owner's hard promise: the BASE (Local) edition UI must be completely
// clean of references to the app's OWN AI capabilities. The app's SUBJECT — the AI-coding apps it
// inventories (Claude, Codex, Cursor, Antigravity, Hermes) — is legitimate and stays everywhere;
// what must never surface in base is the app's own AI features. These phrases only ever name the
// latter, so they must not render while the AI Connector edition is inactive (connectorBuild=false,
// i.e. security.activeFeatures does NOT include "agent_automation").
const FORBIDDEN_OWN_AI = [
  "AI Assist",
  "connected AI apps",
  "Review AI summary request",
  "Explain this",
  "Rewrite this",
  "AI provider",
  "AI app integration",
  "AI Connector",
];

function assertClean(label: string, text: string) {
  for (const phrase of FORBIDDEN_OWN_AI) {
    expect(text.includes(phrase), `${label} leaked own-AI copy: "${phrase}"`).toBe(false);
  }
}

describe("base edition comments hint", () => {
  it("omits the connected-AI-apps clause in the base edition", () => {
    const hint = commentsPanelHint(false);
    expect(hint).toBe(
      "Notes you attach to this item. Stored locally and encrypted; never sent anywhere."
    );
    assertClean("commentsPanelHint(false)", hint);
  });

  it("restores the connected-AI-apps clause only in the Connector edition", () => {
    expect(commentsPanelHint(true)).toContain("connected AI apps");
  });
});

describe("base edition renders no own-AI copy", () => {
  it("keeps the rendered CommentsPanel clean when connectorBuild is false", () => {
    const html = renderToStaticMarkup(
      createElement(CommentsPanel, { nodeId: 1, connectorBuild: false })
    );
    assertClean("CommentsPanel(base)", html);
  });

  it("still surfaces the AI-apps note in the Connector edition", () => {
    const html = renderToStaticMarkup(
      createElement(CommentsPanel, { nodeId: 1, connectorBuild: true })
    );
    expect(html).toContain("connected AI apps");
  });
});

describe("base edition build-capability copy is clean", () => {
  it("never names the app's own AI in the core-preview security summary", async () => {
    const status = await fixtureApi.securityStatus();
    // The fixture stands in for the base build: core only, no agent_automation.
    expect(status.activeFeatures).not.toContain("agent_automation");
    assertClean(
      "securityStatus",
      `${status.outboundNetwork} | ${status.mutationExecutor} | ${status.agentIpc} | ${status.notes.join(" | ")}`
    );
  });
});
