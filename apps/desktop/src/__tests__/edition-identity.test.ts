import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import {
  SettingsAutomationView,
  SettingsConnectedAppsView,
  automationIntegrationAvailabilityState,
  connectedAppsIntegrationAvailabilityState
} from "../views/ConnectorSettingsViews";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const connectorStyles = readFileSync(new URL("../views/ConnectorEditionLayer.css", import.meta.url), "utf8");

describe("compile-time edition identity", () => {
  it("derives the persistent label from the same Vite mode marker as connector code splitting", () => {
    expect(appSource).toContain('const connectorFrontendBuild = import.meta.env.MODE === "test" || import.meta.env.MODE === "connector";');
    expect(appSource).toContain('const frontendEditionLabel = connectorFrontendBuild ? "AI Connector" : "Local";');

    const badgeStart = appSource.indexOf('className={`brand-edition');
    const badge = appSource.slice(badgeStart, appSource.indexOf("</small>", badgeStart));
    expect(badgeStart).toBeGreaterThanOrEqual(0);
    expect(badge).toContain("frontendEditionLabel");
    expect(badge).toContain("connectorFrontendBuild");
    expect(badge).not.toContain("security");
    expect(badge).not.toContain("connectorBuild");
  });

  it("keeps the edition badge visible when wordmark spans are compacted", () => {
    expect(styles).toMatch(/\.brand-edition\s*\{[^}]*display:\s*inline-flex;/s);
    expect(styles).not.toMatch(/\.brand-edition--connector\s*\{/);
    expect(connectorStyles).toMatch(/\.brand-edition--connector\s*\{/);
  });
});

describe("integration availability truth", () => {
  it("classifies loading, error, not-compiled and ready without conflating them", () => {
    expect(automationIntegrationAvailabilityState(null, null)).toBe("loading");
    expect(automationIntegrationAvailabilityState(null, "status failed")).toBe("error");
    expect(automationIntegrationAvailabilityState({ enabled: false }, null)).toBe("not-compiled");
    expect(automationIntegrationAvailabilityState({ enabled: true }, null)).toBe("ready");

    expect(connectedAppsIntegrationAvailabilityState(false, null, 0)).toBe("loading");
    expect(connectedAppsIntegrationAvailabilityState(true, "config failed", 0)).toBe("error");
    expect(connectedAppsIntegrationAvailabilityState(true, null, 0)).toBe("not-compiled");
    expect(connectedAppsIntegrationAvailabilityState(true, null, 2)).toBe("ready");
  });

  it("renders an actionable error instead of simultaneous loading or not-compiled copy", () => {
    const html = renderToStaticMarkup(createElement(SettingsAutomationView, {
      status: null,
      agents: [],
      activity: [],
      credential: null,
      projects: [],
      currentFile: null,
      busy: false,
      error: "Named-pipe status failed.",
      onRefresh: vi.fn(),
      onRegister: vi.fn(),
      onRevoke: vi.fn(),
      onForget: vi.fn(),
      onGrantRead: vi.fn(),
      onCopy: vi.fn(),
      onClearCredential: vi.fn()
    }));

    expect(html).toContain('data-state="error"');
    expect(html).toContain('role="alert"');
    expect(html).toContain("Integration status unavailable");
    expect(html).toContain("Retry status check");
    expect(html).not.toContain("Checking integration");
    expect(html).not.toContain("Not compiled into this build");
  });

  it("shows a dedicated loading state before connected-app status resolves", () => {
    const html = renderToStaticMarkup(createElement(SettingsConnectedAppsView, {
      confirm: async () => false,
      projects: []
    }));

    expect(html).toContain('data-state="loading"');
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("Checking integration");
    expect(html).not.toContain("Not compiled into this build");
  });
});
