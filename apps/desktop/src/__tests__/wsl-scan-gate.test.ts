// @ts-expect-error Vitest runs in Node; the desktop build intentionally omits Node typings.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  runWslGatedDiscovery,
  type WslGatedDiscoveryScope,
  type WslScanPreferencePort
} from "../wslScanGate";

const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

describe("WSL discovery preference gate", () => {
  it("applies an immediate ON to OFF choice before folder discovery starts", async () => {
    let persisted = true;
    const events: string[] = [];
    const port: WslScanPreferencePort = {
      setEnabled: async (enabled) => {
        events.push(`set:${enabled}`);
        persisted = enabled;
      },
      readEnabled: async () => {
        events.push(`read:${persisted}`);
        return persisted;
      }
    };

    const result = await runWslGatedDiscovery({
      scope: "folder",
      requestedEnabled: false,
      port,
      start: async (appliedEnabled) => {
        events.push(`projectDiscoveryDeepScan:${appliedEnabled}`);
        return "folder-report";
      }
    });

    expect(result).toMatchObject({ scope: "folder", appliedEnabled: false, result: "folder-report" });
    expect(events).toEqual([
      "set:false",
      "read:false",
      "projectDiscoveryDeepScan:false"
    ]);
  });

  it("does not start discovery when persistence fails", async () => {
    let starts = 0;
    await expect(runWslGatedDiscovery({
      scope: "global",
      requestedEnabled: true,
      port: {
        setEnabled: async () => {
          throw new Error("preference store unavailable");
        },
        readEnabled: async () => false
      },
      start: async () => {
        starts += 1;
        return "must-not-run";
      }
    })).rejects.toMatchObject({
      name: "WslScanPreferenceApplyError",
      message: "preference store unavailable"
    });
    expect(starts).toBe(0);
  });

  it("does not start discovery when read-back disagrees with the requested scope", async () => {
    let starts = 0;
    await expect(runWslGatedDiscovery({
      scope: "folder",
      requestedEnabled: false,
      port: {
        setEnabled: async () => undefined,
        readEnabled: async () => true
      },
      start: async () => {
        starts += 1;
        return "must-not-run";
      }
    })).rejects.toMatchObject({ name: "WslScanPreferenceApplyError" });
    expect(starts).toBe(0);
  });

  it("routes global and folder discovery through the same verified gate", async () => {
    const scopes: WslGatedDiscoveryScope[] = ["global", "folder"];
    for (const scope of scopes) {
      const events: string[] = [];
      let persisted = false;
      const result = await runWslGatedDiscovery({
        scope,
        requestedEnabled: true,
        port: {
          setEnabled: async (enabled) => {
            events.push("set");
            persisted = enabled;
          },
          readEnabled: async () => {
            events.push("verify");
            return persisted;
          }
        },
        start: async (appliedEnabled) => {
          events.push(`${scope}-start:${appliedEnabled}`);
          return scope;
        }
      });
      expect(result.scope).toBe(scope);
      expect(events).toEqual(["set", "verify", `${scope}-start:true`]);
    }
    expect(appSource.match(/startWslGatedProjectDiscovery\("(?:global|folder)"/g)).toEqual([
      'startWslGatedProjectDiscovery("global"',
      'startWslGatedProjectDiscovery("folder"'
    ]);
    expect(appSource).toContain("if (!addProjectsVisible) return;");
    expect(appSource).toContain("void refreshWslScanPreference();");
    expect(appSource).toContain("onToggleWsl={updateWslScanPreference}");
  });
});
