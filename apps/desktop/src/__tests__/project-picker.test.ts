import { describe, expect, it } from "vitest";

import { projectPickerInputStatus, resolveProjectPickerInput } from "../documentSearch";

const projects = [
  {
    id: 1,
    name: "CodeHangar",
    path: "C:\\Synthetic\\CodeHangarDemo",
    antigravityName: "Hangar de Código"
  },
  {
    id: 2,
    name: "MeaningOfLife",
    path: "C:\\AI\\AI_Projects\\meaningoflife",
    antigravityName: "Atlântida"
  },
  {
    id: 3,
    name: "MeaningOfLife Tools",
    path: "D:\\Archive\\meaningoflife-tools"
  }
];

describe("typed project picker", () => {
  it.each([
    ["CodeHangar", 1, "exact stored name"],
    ["codehangar", 1, "lower-case compact name"],
    ["code hangar", 1, "natural spacing in a camel-case name"],
    ["Hangar de Codigo", 1, "alias without accents"],
    ["C:\\Synthetic\\CodeHangarDemo", 1, "exact Windows path"],
    ["c:\\synthetic\\codehangardemo", 1, "lower-case compact Windows path"],
    ["atlântida", 2, "exact accented alias"]
  ])("resolves %s to project %s from its %s", (input, expectedId) => {
    const resolution = resolveProjectPickerInput(projects, input);

    expect(resolution.kind).toBe("resolved");
    expect(resolution.project?.id).toBe(expectedId);
    expect(projectPickerInputStatus(resolution)).toBeNull();
  });

  it("does not silently choose between two projects with the same typed prefix", () => {
    const resolution = resolveProjectPickerInput(projects, "meaning");

    expect(resolution.kind).toBe("ambiguous");
    expect(resolution.project).toBeNull();
    expect(resolution.matches.map((project) => project.id)).toEqual([2, 3]);
    expect(projectPickerInputStatus(resolution)).toBe(
      "2 projects match. Type the exact name or local path."
    );
  });

  it("does not silently resolve even a unique partial path match", () => {
    const resolution = resolveProjectPickerInput(projects, "AI Projects meaning");

    expect(resolution.kind).toBe("ambiguous");
    expect(resolution.project).toBeNull();
    expect(resolution.matches.map((project) => project.id)).toEqual([2]);
    expect(projectPickerInputStatus(resolution)).toBe(
      "A project is close. Choose its exact name or local path."
    );
  });

  it("prefers one exact name over another project's broader path prefix", () => {
    const resolution = resolveProjectPickerInput([
      { id: 10, name: "Atlas", path: "C:\\Work\\Atlas" },
      { id: 11, name: "Atlas Archive", path: "D:\\Atlas\\Archive" }
    ], "Atlas");

    expect(resolution.kind).toBe("resolved");
    expect(resolution.project?.id).toBe(10);
  });

  it.each(["component", "omp", "prompt", "missing"])(
    "rejects unrelated substring %s instead of guessing",
    (input) => {
      const resolution = resolveProjectPickerInput(projects, input);

      expect(resolution.kind).toBe("none");
      expect(resolution.project).toBeNull();
      expect(projectPickerInputStatus(resolution)).toBe(
        "No project matches this name, alias or local path."
      );
    }
  );

  it("keeps blank input distinct from an unmatched project", () => {
    const resolution = resolveProjectPickerInput(projects, "   ");

    expect(resolution.kind).toBe("empty");
    expect(projectPickerInputStatus(resolution)).toBe("Type a project name, alias or local path.");
  });

  it("returns ambiguous matches in stable name/path order for an accessible listbox", () => {
    const reversed = [...projects].reverse();
    const resolution = resolveProjectPickerInput(reversed, "meaning");

    expect(resolution.matches.map((project) => project.name)).toEqual([
      "MeaningOfLife",
      "MeaningOfLife Tools"
    ]);
  });
});
