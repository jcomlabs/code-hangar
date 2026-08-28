// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const settings = readFileSync(new URL("../views/ConnectorSettingsViews.tsx", import.meta.url), "utf8");
const localSettings = readFileSync(new URL("../views/SettingsView.tsx", import.meta.url), "utf8");
const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const connectorApi = readFileSync(new URL("../connectorApi.ts", import.meta.url), "utf8");
const connectorLayer = readFileSync(new URL("../views/ConnectorEditionLayer.tsx", import.meta.url), "utf8");
const types = readFileSync(new URL("../types.ts", import.meta.url), "utf8");
const connectorTypes = readFileSync(new URL("../connectorTypes.ts", import.meta.url), "utf8");
const commentsPanel = readFileSync(new URL("../views/CommentsPanel.tsx", import.meta.url), "utf8");
const connectorComments = readFileSync(new URL("../views/ConnectorCommentPresentation.tsx", import.meta.url), "utf8");

describe("connected-app least-privilege UX contract", () => {
  it("keeps Connector implementation and copy out of the Local settings module", () => {
    for (const marker of [
      "connectorApi",
      "SettingsAutomationView",
      "SettingsConnectedAppsView",
      "AI app integration",
      "connected AI apps",
      "Model Context Protocol",
      "CODEHANGAR_MCP"
    ]) {
      expect(localSettings).not.toContain(marker);
    }
  });

  it("keeps the default connection body-free and makes advanced scopes explicit", () => {
    expect(settings).toContain("Claude, Cursor and Codex");
    expect(settings).not.toContain("Claude, Cursor and ChatGPT");
    expect(settings).toContain("The standard connection is body-free");
    expect(settings).toContain("This connector has no file-body tool");
    expect(settings).toContain("Opt-in history_search scope");
    expect(settings).toContain("Opt-in execute_plan scope for backup/holding requests and final-removal review recommendations only");
    expect(settings).toContain("Advanced access requires a second confirmation");
    expect(settings).toContain("Confirm advanced access for ${host.label}?");

    expect(connectorApi).toMatch(/includeHistorySearch\s*=\s*false/);
    expect(connectorApi).toMatch(/includeMutationRequests\s*=\s*false/);
    expect(connectorApi).toContain("includeHistorySearch,");
    expect(connectorApi).toContain("includeMutationRequests");
  });

  it("renders effective host access instead of inferring it from global toggles", () => {
    for (const field of [
      "effectiveScopes",
      "effectiveProjectIds",
      "credentialActive",
      "recoveryRequired"
    ]) {
      expect(connectorTypes).toContain(`${field}:`);
      expect(settings).toContain(`host.${field}`);
    }
    expect(settings).toContain("These are read from the exact enabled credential");
    expect(settings).toContain("Reconnect with selected access");
    expect(settings).toContain("Config present · inactive credential");
  });

  it("exposes immutable transport identity and DB-only orphan revocation", () => {
    for (const field of [
      "identityId",
      "agentKind",
      "allowedTransport",
      "connectedHost",
      "durableIdentityId",
      "durableCredentialEnabled",
      "credentialOrphaned",
      "orphanReason"
    ]) {
      expect(connectorTypes).toContain(`${field}`);
    }
    expect(connectorApi).toContain('"mcp_appconfig_revoke_orphan"');
    expect(connectorApi).toContain('"mcp_appconfig_forget_orphan"');
    expect(settings).toContain("Revoke credential only");
    expect(settings).toContain("Forget revoked record");
    expect(settings).toContain("database-only safety action");
    expect(settings).toContain("does not read, repair or edit");
    expect(settings).toContain("display name is never an authority boundary");
  });

  it("keeps connected-comment identity and copy in the Connector graph", () => {
    for (const marker of ["Connected AI apps", "connector-comment-badge", ">AI<"]) {
      expect(commentsPanel).not.toContain(marker);
      expect(connectorComments).toContain(marker);
    }
    expect(types).not.toContain("mcp_stdio");
    expect(types).not.toContain("interface AutomationAgentSummary");
  });

  it("treats a connected-app final-remove request as review-only", () => {
    expect(settings).toContain("recommends reviewing");
    expect(settings).toContain("Review in Recovery");
    expect(settings).toContain("This connected app is recommending a review only");
    expect(settings).toContain("this panel cannot delete it or approve the local final-removal batch");
    expect(settings).not.toContain("Delete permanently");
    expect(settings).not.toContain("PERMANENTLY delete");

    const finalRemoveBranch = settings.slice(
      settings.indexOf('if (request.kind === "final_remove") {', settings.indexOf("const confirmApprove")),
      settings.indexOf("// Comment + read-body kinds keep the light gate")
    );
    expect(finalRemoveBranch).toContain("Open Recovery & cleanup");
    expect(finalRemoveBranch).not.toContain("agentRequestResolve");
  });

  it("warns that an inventory reset invalidates every connected-app credential", () => {
    expect(connectorLayer).toContain("Every connected AI app will stop authenticating");
    expect(connectorLayer).toContain("Reconnect each app from Code Hangar");
    expect(connectorLayer).toContain("may retain an unusable Code Hangar entry");
    expect(app).toContain("editionConsequence={editionBridgeRef.current?.resetConsequence()}");
    expect(app).toContain("I also understand the installed-edition consequence listed above.");
    expect(localSettings).not.toContain("connected AI app");
  });
});
