import { describe, expect, it } from "vitest";

import { connectorGuidedTourStepCopy } from "../views/ConnectorGuidedTour";
import { guidedTourStepCopy, guidedTourStorageKey, TOUR_SELECTORS, TOUR_STEP_COPY, type TourStep } from "../views/GuidedTour";

const stepBySelector = (steps: readonly TourStep[], selector: string) =>
  steps.find((step) => step.selector === selector);

function expectAnchorsUsedOnce(steps: readonly TourStep[]) {
  const known = new Set<string>(Object.values(TOUR_SELECTORS));
  for (const step of steps) {
    if (step.selector) expect(known).toContain(step.selector);
  }
  for (const selector of Object.values(TOUR_SELECTORS)) {
    expect(steps.filter((step) => step.selector === selector)).toHaveLength(1);
  }
}

describe("edition-specific guided tours", () => {
  it("keeps the Local tour short and anchored to the real workflow", () => {
    expect(TOUR_STEP_COPY).toHaveLength(7);
    expect(TOUR_STEP_COPY[0].selector).toBeUndefined();
    expect(TOUR_STEP_COPY.at(-1)?.selector).toBeUndefined();
    expect(TOUR_STEP_COPY.filter((step) => step.selector)).toHaveLength(5);
    expectAnchorsUsedOnce(TOUR_STEP_COPY);

    const workspace = stepBySelector(TOUR_STEP_COPY, TOUR_SELECTORS.workspace);
    for (const label of ["What changed", "Context", "Files", "Sessions", "Values", "previous version"]) {
      expect(workspace?.body).toContain(label);
    }
    expect(stepBySelector(TOUR_STEP_COPY, TOUR_SELECTORS.safeManage)?.body).toContain("backup-first");
  });

  it("contains no Connector capability advertising in the Local tutorial", () => {
    const text = guidedTourStepCopy("first-run", true).map((step) => `${step.title} ${step.body}`).join(" ");
    for (const connectorOnly of ["AI Connector", "local model", "your own API", "provider", "exact destination"]) {
      expect(text).not.toContain(connectorOnly);
    }
  });

  it("adds one clear model step only to Code Hangar AI Connector", () => {
    const connector = connectorGuidedTourStepCopy("first-run", true);
    expect(connector).toHaveLength(8);
    expectAnchorsUsedOnce(connector);
    const text = connector.map((step) => `${step.title} ${step.body}`).join(" ");
    expect(text).toContain("Code Hangar AI Connector");
    expect(text).toContain("local model");
    expect(text).toContain("your own API");
    expect(text).toContain("exact destination");
    expect(text).toContain("sensitive content is blocked");
    expect(text).toContain("do not write the file");
  });

  it("tracks completion separately for each edition and tutorial revision", () => {
    expect(guidedTourStorageKey("local")).toBe("codehangar:tutorial-done-v2:local");
    expect(guidedTourStorageKey("connector")).toBe("codehangar:tutorial-done-v2:connector");
    expect(guidedTourStorageKey("local")).not.toBe(guidedTourStorageKey("connector"));
  });
});

describe("guided tour endings", () => {
  it("hands an empty first run to Add projects without doing that to existing catalogs", () => {
    const emptyOutro = guidedTourStepCopy("first-run", false).at(-1);
    expect(emptyOutro?.body).toContain("Add projects");
    expect(emptyOutro?.body).toContain("Deep Scan");
    expect(emptyOutro?.actionLabel).toBe("Finish and find projects");

    const existingOutro = guidedTourStepCopy("first-run", true).at(-1);
    expect(existingOutro?.body).toContain("Recent work");
    expect(existingOutro?.body).not.toContain("Deep Scan");
    expect(existingOutro?.actionLabel).toBe("Finish tour");
  });

  it("keeps replay copy side-effect free", () => {
    const replay = guidedTourStepCopy("replay", true);
    expect(replay).toHaveLength(7);
    expect(replay[0].body).toContain("preferences will be restored");
    expect(replay.at(-1)?.title).toBe("Refresher complete");
    expect(replay.at(-1)?.body).toContain("return to the screen where you started");
    expect(replay.at(-1)?.body).not.toContain("Deep Scan");
  });
});
