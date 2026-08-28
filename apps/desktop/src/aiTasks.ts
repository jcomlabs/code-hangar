import { useSyncExternalStore } from "react";

/**
 * A tiny module-level store for in-flight + completed AI Assist results, shared across the app.
 *
 * Why this exists: an AI call started inside a component (e.g. "Explain this", "Summarize with AI")
 * used to be tied to that component's lifecycle — navigating away while it was "thinking" dropped
 * the result, and the answer vanished when the panel unmounted. Here the promise runs in the
 * module, detached from React, so:
 *   - navigating away never cancels an in-flight request,
 *   - the result persists for the session and reappears when you return,
 *   - both the docked and the floating Explain panel (and the Summarize card) read the same task.
 *
 * Results live in memory for the session only — nothing is written to disk, matching the zero-trace
 * posture of the AI surface.
 */

export type AiTaskKind = "explain" | "review" | "summary";
export type AiTaskStatus = "pending" | "done" | "error";

export interface AiTask {
  key: string;
  kind: AiTaskKind;
  title: string;
  subtitle?: string;
  status: AiTaskStatus;
  result?: string;
  meta?: string;
  error?: string;
  seq: number;
  runId: number;
}

const tasks = new Map<string, AiTask>();
const listeners = new Set<() => void>();
let seq = 0;
let runId = 0;
let snapshot: AiTask[] = [];

function emit(): void {
  snapshot = Array.from(tasks.values());
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): AiTask[] {
  return snapshot;
}

export function getAiTask(key: string): AiTask | undefined {
  return tasks.get(key);
}

export function clearAiTask(key: string): void {
  if (tasks.delete(key)) emit();
}

/**
 * Start (or restart) an AI task and run it detached from any component. A task already pending
 * under the same key is left alone, so re-opening a panel mid-think never fires a duplicate send.
 */
export function startAiTask(
  key: string,
  kind: AiTaskKind,
  title: string,
  subtitle: string | undefined,
  runner: () => Promise<{ result: string; meta?: string }>
): void {
  const existing = tasks.get(key);
  if (existing && existing.status === "pending") return;
  const activeRunId = (runId += 1);
  tasks.set(key, { key, kind, title, subtitle, status: "pending", seq: (seq += 1), runId: activeRunId });
  emit();
  runner().then(
    ({ result, meta }) => {
      const current = tasks.get(key);
      if (current?.runId !== activeRunId) return;
      tasks.set(key, { key, kind, title, subtitle, status: "done", result, meta, seq: (seq += 1), runId: activeRunId });
      emit();
    },
    (err: unknown) => {
      const current = tasks.get(key);
      if (current?.runId !== activeRunId) return;
      tasks.set(key, {
        key,
        kind,
        title,
        subtitle,
        status: "error",
        error: err instanceof Error ? err.message : String(err),
        seq: (seq += 1),
        runId: activeRunId
      });
      emit();
    }
  );
}

/** Start a task whose backend can emit bounded text deltas before the final result resolves. */
export function startAiStreamingTask(
  key: string,
  kind: AiTaskKind,
  title: string,
  subtitle: string | undefined,
  runner: (onDelta: (delta: string) => void) => Promise<{ result: string; meta?: string }>
): void {
  const existing = tasks.get(key);
  if (existing && existing.status === "pending") return;
  const activeRunId = (runId += 1);
  tasks.set(key, { key, kind, title, subtitle, status: "pending", result: "", seq: (seq += 1), runId: activeRunId });
  emit();
  const onDelta = (delta: string) => {
    if (!delta) return;
    const current = tasks.get(key);
    if (!current || current.runId !== activeRunId || current.status !== "pending") return;
    tasks.set(key, { ...current, result: `${current.result ?? ""}${delta}`, seq: (seq += 1) });
    emit();
  };
  runner(onDelta).then(
    ({ result, meta }) => {
      const current = tasks.get(key);
      if (current?.runId !== activeRunId) return;
      tasks.set(key, {
        key,
        kind,
        title,
        subtitle,
        status: "done",
        result: result || current.result || "",
        meta,
        seq: (seq += 1),
        runId: activeRunId
      });
      emit();
    },
    (err: unknown) => {
      const current = tasks.get(key);
      if (current?.runId !== activeRunId) return;
      tasks.set(key, {
        key,
        kind,
        title,
        subtitle,
        status: "error",
        result: current.result,
        error: err instanceof Error ? err.message : String(err),
        seq: (seq += 1),
        runId: activeRunId
      });
      emit();
    }
  );
}

/** Subscribe a component to the task keyed by `key` (or none when `key` is null). */
export function useAiTask(key: string | null): AiTask | undefined {
  const all = useSyncExternalStore(subscribe, getSnapshot);
  if (!key) return undefined;
  return all.find((task) => task.key === key);
}

/** Stable djb2 hash → short hex, for keying a task to the exact selected snippet. */
export function hashSnippet(text: string): string {
  let hash = 5381;
  for (let index = 0; index < text.length; index += 1) {
    hash = ((hash << 5) + hash + text.charCodeAt(index)) | 0;
  }
  return (hash >>> 0).toString(16);
}
