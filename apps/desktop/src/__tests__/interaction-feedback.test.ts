import { describe, expect, it } from "vitest";

import {
  mutationMoveCompletionMessage,
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

  it("distinguishes source move, holding allocation and retained archive after a move", () => {
    const message = mutationMoveCompletionMessage({
      operationId: 42,
      entries: [],
      spaceRecovered: 1_572_864,
      moved: 3,
      skipped: 1,
      failed: 0,
      removedDirs: 1,
      removedLinks: 2
    });

    expect(message).toContain("3 supported original items moved to recovery holding, 1 skipped, 0 failed");
    expect(message).toContain("blocked objects remain at their original paths");
    expect(message).toContain("Held allocation remains until Finish removing");
    expect(message).toContain("verified backup/archive is retained separately");
    expect(message).toContain("legacy journal recorded 1.5 MiB as a source-volume effect");
    expect(message).toContain("not a measurement of current free space or held allocation");
    expect(message.toLowerCase()).not.toContain("recovered safely");
  });
});
