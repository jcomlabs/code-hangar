import { describe, expect, it } from "vitest";
import {
  parseVisualAcceptanceState,
  shouldFailVisualAcceptanceCommand,
  visualAcceptanceDelayMs,
  visualAcceptanceProjectCount
} from "../visualAcceptance";

describe("visual acceptance fixtures", () => {
  it("exposes only known states and only when the development harness is enabled", () => {
    expect(parseVisualAcceptanceState("?acceptanceState=partial", true)).toBe("partial");
    expect(parseVisualAcceptanceState("?acceptanceState=SATURATED", true)).toBe("saturated");
    expect(parseVisualAcceptanceState("?acceptanceState=unknown", true)).toBe("default");
    expect(parseVisualAcceptanceState("?acceptanceState=error", false)).toBe("default");
  });

  it("keeps loading delay, failure injection and saturation bounded", () => {
    expect(visualAcceptanceDelayMs("loading")).toBe(2500);
    expect(visualAcceptanceDelayMs("default")).toBe(0);
    expect(shouldFailVisualAcceptanceCommand("error", "dashboard_summary")).toBe(true);
    expect(shouldFailVisualAcceptanceCommand("error", "startup_status")).toBe(false);
    expect(shouldFailVisualAcceptanceCommand("default", "dashboard_summary")).toBe(false);
    expect(visualAcceptanceProjectCount("saturated")).toBe(96);
    expect(visualAcceptanceProjectCount("partial")).toBe(0);
  });
});
