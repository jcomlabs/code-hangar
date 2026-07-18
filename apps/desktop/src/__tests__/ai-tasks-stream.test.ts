import { describe, expect, it } from "vitest";
import { clearAiTask, getAiTask, startAiStreamingTask } from "../aiTasks";

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("streaming AI task store", () => {
  it("publishes deltas before keeping the authoritative final response", async () => {
    const key = "stream:complete";
    let finish: ((value: { result: string; meta?: string }) => void) | undefined;
    startAiStreamingTask(key, "explain", "File", undefined, async (onDelta) => {
      onDelta("first ");
      onDelta("words");
      return new Promise((resolve) => { finish = resolve; });
    });

    await settle();
    expect(getAiTask(key)).toMatchObject({ status: "pending", result: "first words" });
    finish?.({ result: "first words complete", meta: "local" });
    await settle();
    expect(getAiTask(key)).toMatchObject({ status: "done", result: "first words complete", meta: "local" });
    clearAiTask(key);
  });

  it("keeps partial text on failure and ignores a cleared stale run", async () => {
    const failed = "stream:failed";
    startAiStreamingTask(failed, "review", "File", undefined, async (onDelta) => {
      onDelta("partial evidence");
      throw new Error("connection closed");
    });
    await settle();
    expect(getAiTask(failed)).toMatchObject({ status: "error", result: "partial evidence", error: "connection closed" });
    clearAiTask(failed);

    const cleared = "stream:cleared";
    let finish: ((value: { result: string }) => void) | undefined;
    startAiStreamingTask(cleared, "explain", "File", undefined, async () =>
      new Promise((resolve) => { finish = resolve; })
    );
    await settle();
    clearAiTask(cleared);
    finish?.({ result: "late result" });
    await settle();
    expect(getAiTask(cleared)).toBeUndefined();
  });
});
