import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { commentsPanelHint } from "../comments";
import { CommentsPanel } from "../views/CommentsPanel";
import { fixtureApi } from "../fixtures";
import { BEGINNER_HELP } from "../BeginnerHelp";

// Regression guard for the owner's hard promise: the BASE (Local) edition UI must be completely
// clean of references to the app's OWN AI capabilities. The app's SUBJECT — the AI-coding apps it
// inventories (Claude, Codex, Cursor, Antigravity, Hermes) — is legitimate and stays everywhere;
// what must never surface in base is the app's own AI features. These phrases only ever name the
// latter, so they must not render while the edition extension is inactive,
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
  "AI explanation",
  "AI sending",
  "local automation",
  "local endpoint",
  "one-time password",
];

function assertClean(label: string, text: string) {
  for (const phrase of FORBIDDEN_OWN_AI) {
    expect(text.includes(phrase), `${label} leaked own-AI copy: "${phrase}"`).toBe(false);
  }
}

describe("base edition comments hint", () => {
  it("omits the connected-AI-apps clause in the base edition", () => {
    const hint = commentsPanelHint();
    expect(hint).toBe(
      "Notes you attach to this item. Stored locally and encrypted."
    );
    assertClean("commentsPanelHint", hint);
  });
});

describe("base edition renders no own-AI copy", () => {
  it("keeps the rendered CommentsPanel clean when the edition extension is inactive", () => {
    const html = renderToStaticMarkup(
      createElement(CommentsPanel, { nodeId: 1, editionExtensionActive: false })
    );
    assertClean("CommentsPanel(base)", html);
  });

  it("keeps Connector presentation out of the synchronous Local markup", () => {
    const html = renderToStaticMarkup(createElement(CommentsPanel, { nodeId: 1 }));
    expect(html).not.toContain("connector-comment");
  });
});

describe("base edition build-capability copy is clean", () => {
  it("keeps shared help free of Connector-only capability copy", () => {
    assertClean("BEGINNER_HELP", JSON.stringify(BEGINNER_HELP));
  });

  it("never names the app's own AI in the core-preview security summary", async () => {
    const status = await fixtureApi.securityStatus();
    // The fixture stands in for the base build: core only, no agent_automation.
    expect(status.activeFeatures).not.toContain("agent_automation");
    expect(status).not.toHaveProperty("outboundNetwork");
    expect(status).not.toHaveProperty("agentIpc");
    assertClean("securityStatus", JSON.stringify(status));
  });
});
