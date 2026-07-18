import { describe, expect, it } from "vitest";
import { api } from "../api";
import { fixtureApi } from "../fixtures";

const ACTION = "Read-only local review";

describe("operation plan preview (fixtures)", () => {
  it("builds a read-only operation plan that executes nothing", async () => {
    const projects = await fixtureApi.projectsList();
    const plan = await fixtureApi.operationPlanBuild(projects[0].id, ACTION);

    expect(plan.schema).toBe("operation_plan/1");
    expect(plan.readOnlyPreview).toBe(true);
    expect(plan.externalServicesUnaffected).toBe(true);
    // recoverable accounting is internally consistent and never negative
    expect(plan.recoverableBytes.owned + plan.recoverableBytes.orphanedOnRemoval).toBe(
      plan.recoverableBytes.total
    );
    // Copy stays evidence-oriented in the read-only build.
    expect(plan.recommendedAction.toLowerCase()).toContain("not an action queue");
  });

  it("projects a preview-only risk report with caveats", async () => {
    const projects = await fixtureApi.projectsList();
    const report = await fixtureApi.riskReportBuildForTarget(projects[0].id, ACTION);

    expect(report.readOnlyPreview).toBe(true);
    expect(report.externalServicesUnaffected).toBe(true);
    expect(report.caveats.some((caveat) => caveat.toLowerCase().includes("preview only"))).toBe(true);
  });

  it("lists sensitive files as hits but keeps them out of recoverable space", async () => {
    const projects = await fixtureApi.projectsList();
    const sensitive = projects.find((project) => project.name.toLowerCase().includes("sensitive"));
    expect(sensitive).toBeTruthy();

    const plan = await fixtureApi.operationPlanBuild(sensitive!.id, ACTION);
    expect(plan.sensitiveFiles.length).toBeGreaterThan(0);
    // every plan item touching a sensitive/protected path must not free space
    for (const item of plan.items) {
      if (item.risk === "black") {
        expect(item.freesSpace).toBe(false);
      }
    }
  });

  it("targets the whole project by default (project-bound)", async () => {
    const projects = await fixtureApi.projectsList();
    const plan = await fixtureApi.operationPlanBuild(projects[0].id, ACTION);
    expect(plan.target.kind).toBe("project");
  });

  it("can scope to a single file when a file node is the explicit target", async () => {
    const hits = await fixtureApi.quickOpen("readme");
    expect(hits.length).toBeGreaterThan(0);
    const plan = await fixtureApi.operationPlanBuild(hits[0].nodeId, ACTION);
    expect(plan.target.kind).toBe("file");
  });

  it("exposes node_id on lost-project candidates for plan targeting", async () => {
    const lost = await fixtureApi.lostProjectCandidates({ minSizeBytes: 0, includePartial: true });
    expect(lost.candidates.length).toBeGreaterThan(0);
    expect(lost.candidates.every((candidate) => typeof candidate.nodeId === "number")).toBe(true);
  });
});

describe("async operation plan job + performance mode", () => {
  it("start then status yields a completed, read-only, project-bound preview", async () => {
    const projects = await fixtureApi.projectsList();
    const jobId = await api.operationPlanStart(projects[0].id, ACTION, "balanced");
    expect(typeof jobId).toBe("string");

    const status = await api.operationPlanStatus(jobId);
    expect(status.state).toBe("completed");
    expect(status.plan?.readOnlyPreview).toBe(true);
    expect(status.plan?.target.kind).toBe("project");
    expect(status.report?.externalServicesUnaffected).toBe(true);
  });

  it("cancel and performance-mode toggles are safe no-ops without a backend", async () => {
    await expect(api.operationPlanCancel("fixture-plan-1")).resolves.toBeUndefined();
    await expect(api.performanceSetMode("priority")).resolves.toBeUndefined();
    await expect(api.performanceSetMode("max")).resolves.toBeUndefined();
    await expect(api.performanceSetMode("balanced")).resolves.toBeUndefined();
  });
});
