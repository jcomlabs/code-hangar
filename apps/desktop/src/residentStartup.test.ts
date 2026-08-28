import { describe, expect, it } from "vitest";
import { shouldDeferResidentUi } from "./residentStartup";

describe("resident startup", () => {
  it("defers UI hydration while a background launch remains hidden", () => {
    expect(shouldDeferResidentUi(true, false)).toBe(true);
  });

  it("trusts backend Tauri visibility rather than WebView DOM focus", () => {
    expect(shouldDeferResidentUi(true, true)).toBe(false);
  });

  it("never defers an ordinary interactive launch", () => {
    expect(shouldDeferResidentUi(false, false)).toBe(false);
  });
});
