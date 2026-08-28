import { describe, expect, it } from "vitest";

import {
  unregisterProjectConfirmationMessage,
  unregisterRootConfirmationMessage
} from "../settingsActions";

describe("Settings unregister confirmations", () => {
  it("names the exact scan folder and explains that disk files stay untouched", () => {
    const message = unregisterRootConfirmationMessage("C:\\AI\\Projects\\Example");

    expect(message).toContain("Unregister C:\\AI\\Projects\\Example from Code Hangar?");
    expect(message).toContain("Files on disk stay untouched.");
    expect(message).toContain("add the folder again later");
  });

  it("gives orphan-project removals the same reversible metadata framing", () => {
    const message = unregisterProjectConfirmationMessage("Example");

    expect(message).toContain("Remove Example from Code Hangar?");
    expect(message).toContain("local inventory entry");
    expect(message).toContain("add the project again later");
  });
});
