import { describe, expect, it } from "vitest";

import {
  pinFailureMessage,
  pinSuccessMessage,
  postActionHoverHelp,
  scanRootToggleFailureMessage,
  scanRootToggleMessage
} from "../interactionFeedback";

describe("reversible action feedback", () => {
  it("acknowledges both pin states with the affected item", () => {
    expect(pinSuccessMessage("README.md", true)).toBe("README.md pinned for quick access.");
    expect(pinSuccessMessage("README.md", false)).toBe("README.md removed from Pinned.");
  });

  it("keeps pin failures specific to the requested direction", () => {
    expect(pinFailureMessage("README.md", true, new Error("database busy"))).toBe(
      "Could not pin README.md: database busy"
    );
    expect(pinFailureMessage("README.md", false, "database busy")).toBe(
      "Could not unpin README.md: database busy"
    );
  });

  it("explains that disabling a scan root preserves its inventory", () => {
    expect(scanRootToggleMessage("C:\\Work", true)).toBe("C:\\Work enabled for future scans.");
    expect(scanRootToggleMessage("C:\\Work", false)).toBe(
      "C:\\Work disabled. Existing inventory remains available."
    );
  });

  it("names the failed root action and path", () => {
    expect(scanRootToggleFailureMessage("C:\\Work", false, new Error("scan active"))).toBe(
      "Could not disable C:\\Work: scan active"
    );
  });

  it("drops help from a control that disappeared after an action", () => {
    expect(postActionHoverHelp(true, undefined)).toBeNull();
    expect(postActionHoverHelp(true, "  ")).toBeNull();
  });

  it("refreshes pointer help from the control now underneath and clears keyboard clicks", () => {
    expect(postActionHoverHelp(true, "Open the inventory overview.")).toBe("Open the inventory overview.");
    expect(postActionHoverHelp(false, "Brand navigation")).toBeNull();
  });
});
