import { describe, expect, it } from "vitest";

import { clampSessionTranscriptPage, compactSessionToolActivity, enrichedSessionDisplayName, initialSessionTranscriptPage, nextSessionPreviewLimit, parseSessionMetadata, parseSessionTranscript, sessionDisplayNameNeedsEnrichment, sessionRoleKey, sessionRoleLabel, sessionSupportsProgressiveLoading, sessionTranscriptPageCount, sessionTranscriptPageSlice } from "../session-transcript";

describe("nextSessionPreviewLimit", () => {
  it("doubles the cumulative window without jumping straight to the full session", () => {
    expect(nextSessionPreviewLimit(256 * 1024, 20 * 1024 * 1024)).toBe(512 * 1024);
    expect(nextSessionPreviewLimit(512 * 1024, 20 * 1024 * 1024)).toBe(1024 * 1024);
  });

  it("stops exactly at the session size", () => {
    expect(nextSessionPreviewLimit(1024 * 1024, 1536 * 1024)).toBe(1536 * 1024);
  });
});

describe("sessionSupportsProgressiveLoading", () => {
  it("offers the same expansion controls to project-linked and loose sessions", () => {
    const truncated = { truncated: true };
    expect(sessionSupportsProgressiveLoading("registered_project", truncated)).toBe(true);
    expect(sessionSupportsProgressiveLoading("loose_session", truncated)).toBe(true);
  });

  it("hides expansion controls once either kind of session is complete", () => {
    expect(sessionSupportsProgressiveLoading("registered_project", { truncated: false })).toBe(false);
    expect(sessionSupportsProgressiveLoading("loose_session", { truncated: false })).toBe(false);
  });
});

describe("session transcript paging", () => {
  it("keeps only one bounded page mounted while retaining every loaded turn", () => {
    const turns = Array.from({ length: 451 }, (_, index) => `turn-${index + 1}`);
    expect(sessionTranscriptPageCount(turns.length, 200)).toBe(3);
    expect(sessionTranscriptPageSlice(turns, 1, 200)).toEqual(turns.slice(200, 400));
    expect(sessionTranscriptPageSlice(turns, 2, 200)).toEqual(turns.slice(400));
  });

  it("opens tail previews on their newest page and full sessions at the beginning", () => {
    expect(initialSessionTranscriptPage(451, true, 200)).toBe(2);
    expect(initialSessionTranscriptPage(451, false, 200)).toBe(0);
  });

  it("clamps manual page jumps at both ends", () => {
    expect(clampSessionTranscriptPage(-4, 451, 200)).toBe(0);
    expect(clampSessionTranscriptPage(99, 451, 200)).toBe(2);
  });

  it("keeps a 50,000-turn full session bounded to one mounted page", () => {
    const turns = Array.from({ length: 50_000 }, (_, index) => `turn-${index + 1}`);
    expect(sessionTranscriptPageCount(turns.length, 200)).toBe(250);
    expect(sessionTranscriptPageSlice(turns, 0, 200)).toHaveLength(200);
    expect(sessionTranscriptPageSlice(turns, 124, 200)).toEqual(turns.slice(24_800, 25_000));
    expect(sessionTranscriptPageSlice(turns, 249, 200)).toEqual(turns.slice(49_800, 50_000));
  });
});

describe("compactSessionToolActivity", () => {
  it("condenses consecutive tool-only assistant turns without hiding their names", () => {
    expect(compactSessionToolActivity([
      { role: "assistant", label: "Assistant", content: "↳ used PowerShell" },
      { role: "assistant", label: "Assistant", content: "↳ used Edit" },
      { role: "assistant", label: "Assistant", content: "Done." }
    ])).toEqual([
      { role: "tool", label: "Tool activity", content: "2 tool calls: PowerShell, Edit" },
      { role: "assistant", label: "Assistant", content: "Done." }
    ]);
  });

  it("leaves assistant turns that mix text and tool activity unchanged", () => {
    const turns = [{ role: "assistant", label: "Assistant", content: "Checking now.\n\n↳ used PowerShell" }];
    expect(compactSessionToolActivity(turns)).toEqual(turns);
  });

  it("uses a singular label for one isolated tool call", () => {
    expect(compactSessionToolActivity([
      { role: "assistant", label: "Assistant", content: "↳ used Read" }
    ])).toEqual([
      { role: "tool", label: "Tool activity", content: "Tool used: Read" }
    ]);
  });
});

describe("parseSessionTranscript", () => {
  it("returns no turns for empty or whitespace input", () => {
    expect(parseSessionTranscript("")).toEqual([]);
    expect(parseSessionTranscript("   \n  \n")).toEqual([]);
  });

  it("parses JSONL chat into labeled turns", () => {
    const text = [
      JSON.stringify({ role: "user", content: "Hello there" }),
      JSON.stringify({ role: "assistant", content: "Hi! How can I help?" })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns).toHaveLength(2);
    expect(turns[0]).toMatchObject({ role: "user", label: "You", content: "Hello there" });
    expect(turns[1]).toMatchObject({ role: "assistant", label: "Assistant", content: "Hi! How can I help?" });
  });

  it("recognises alternative role and content field names", () => {
    const text = [
      JSON.stringify({ type: "human", text: "from text field" }),
      JSON.stringify({ author: "gpt-4", message: "from message field" }),
      JSON.stringify({ sender: "system", body: "from body field" })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns.map((t) => [t.role, t.content])).toEqual([
      ["user", "from text field"],
      ["assistant", "from message field"],
      ["system", "from body field"]
    ]);
  });

  it("flattens array content (e.g. content blocks) into text", () => {
    const text = JSON.stringify({
      role: "assistant",
      content: [{ type: "text", text: "part one" }, "part two", { text: "part three" }]
    });
    const turns = parseSessionTranscript(text);
    expect(turns).toHaveLength(1);
    expect(turns[0].content).toBe("part one\n\npart two\n\npart three");
  });

  it("falls back to blank-line blocks for non-JSON transcripts", () => {
    const text = "First message paragraph.\n\nSecond message paragraph.\n\n\nThird.";
    const turns = parseSessionTranscript(text);
    expect(turns).toEqual([
      { content: "First message paragraph." },
      { content: "Second message paragraph." },
      { content: "Third." }
    ]);
    expect(turns.every((t) => t.role === undefined)).toBe(true);
  });

  it("falls back to blocks when too few lines are valid JSON", () => {
    // One JSON line amid prose should NOT be treated as a JSONL transcript.
    const text = "Intro prose line one.\nIntro prose line two.\n" + JSON.stringify({ role: "user", content: "x" });
    const turns = parseSessionTranscript(text);
    // The 0.6 JSON-ratio threshold isn't met (1/3), so it is one prose block.
    expect(turns).toHaveLength(1);
    expect(turns[0].role).toBeUndefined();
    expect(turns[0].content).toContain("Intro prose line one.");
  });

  it("does not dump structured tool-only JSONL into the readable transcript", () => {
    const text = [
      JSON.stringify({ type: "response_item", payload: { type: "custom_tool_call_output", output: "data:image/jpeg;base64,AAAA" } }),
      JSON.stringify({ type: "response_item", payload: { type: "function_call_output", output: "large command output" } })
    ].join("\n");

    expect(parseSessionTranscript(text)).toEqual([]);
  });

  it("drops lines with no readable text instead of dumping raw JSON", () => {
    const text = [
      JSON.stringify({ role: "tool", name: "run_command", args: { cmd: "ls" } }),
      JSON.stringify({ role: "assistant", content: "done" })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    // The structured tool line carries no human text, so it is omitted (no JSON dump).
    expect(turns).toHaveLength(1);
    expect(turns[0]).toMatchObject({ role: "assistant", content: "done" });
  });

  it("does not throw on malformed JSON-looking lines", () => {
    const text = "{ not valid json\n{ also broken";
    expect(() => parseSessionTranscript(text)).not.toThrow();
    // Neither line parses, so it falls back to a single block of the raw text.
    const turns = parseSessionTranscript(text);
    expect(turns.length).toBeGreaterThanOrEqual(1);
  });

  it("separates Claude session metadata from conversational turns", () => {
    const text = JSON.stringify({
      sessionId: "local-123",
      title: "Project review and fixes",
      cwd: "C:\\Work\\SampleProject",
      model: "claude-fable-5",
      createdAt: 1781331968981,
      lastActivityAt: 1781362901684,
      permissionMode: "bypassPermissions",
      isArchived: false,
      enabledMcpTools: { read_file: true, old_tool: false, search_files: true },
      mcpServers: { local: {}, browser: {} }
    });

    expect(parseSessionMetadata(text)).toEqual({
      title: "Project review and fixes",
      initialMessage: undefined,
      projectPath: "C:\\Work\\SampleProject",
      model: "claude-fable-5",
      createdMs: 1781331968981,
      lastActivityMs: 1781362901684,
      permissionMode: "bypassPermissions",
      archived: false,
      enabledToolCount: 2,
      mcpServerCount: 2
    });
    expect(parseSessionTranscript(text)).toEqual([]);
  });

  it("summarises a truncated Claude local-agent record instead of dumping raw JSON", () => {
    const text = [
      "{",
      '  "sessionId": "local_11111111-2222-4333-8444-555555555555",',
      '  "cwd": "C:\\\\Users\\\\sample-user\\\\AppData\\\\Roaming\\\\Claude\\\\outputs",',
      '  "userSelectedFolders": ["C:\\\\Work\\\\SampleProject"],',
      '  "createdAt": 1700000000000,',
      '  "lastActivityAt": 1700000300000,',
      '  "model": "claude-fable-5",',
      '  "permissionMode": "bypassPermissions",',
      '  "isArchived": false,',
      '  "title": "Sample application review",',
      '  "initialMessage": "Clean temporary files",',
      '  "enabledMcpTools": { "read_file": true'
    ].join("\n");

    expect(parseSessionMetadata(text)).toMatchObject({
      title: "Sample application review",
      initialMessage: "Clean temporary files",
      projectPath: "C:\\Work\\SampleProject",
      model: "claude-fable-5",
      createdMs: 1700000000000,
      lastActivityMs: 1700000300000,
      permissionMode: "bypassPermissions",
      archived: false
    });
    expect(parseSessionTranscript(text)).toEqual([]);
  });

  it("replaces UUID filenames only when a readable preview title is available", () => {
    const technicalName = "local_11111111-2222-4333-8444-555555555555.json";
    const preview = '{"sessionId":"local_11111111-2222-4333-8444-555555555555","title":"Sample application review"';

    expect(sessionDisplayNameNeedsEnrichment(technicalName)).toBe(true);
    expect(sessionDisplayNameNeedsEnrichment("Project review and fixes")).toBe(false);
    expect(enrichedSessionDisplayName(technicalName, preview)).toBe("Sample application review");
    expect(enrichedSessionDisplayName("Existing useful title", preview)).toBe("Existing useful title");
  });

  it("does not mistake a timestamped JSON chat message for session metadata", () => {
    const text = JSON.stringify({
      role: "user",
      content: "Keep this conversational turn.",
      createdAt: 1781331968981
    });

    expect(parseSessionMetadata(text)).toBeNull();
    expect(parseSessionTranscript(text)).toEqual([
      { role: "user", label: "You", content: "Keep this conversational turn." }
    ]);
  });

  it("does not mistake Claude JSONL with repeated session ids for one metadata record", () => {
    const text = [
      JSON.stringify({ type: "queue-operation", sessionId: "session-123", operation: "enqueue" }),
      JSON.stringify({ type: "user", sessionId: "session-123", message: { role: "user", content: "Review this project" } }),
      JSON.stringify({ type: "assistant", sessionId: "session-123", message: { role: "assistant", content: [{ type: "text", text: "**Review complete**" }] } })
    ].join("\n");

    expect(parseSessionMetadata(text)).toBeNull();
    expect(parseSessionTranscript(text)).toEqual([
      { role: "user", label: "You", content: "Review this project" },
      { role: "assistant", label: "Assistant", content: "**Review complete**" }
    ]);
  });
});

describe("session role mapping", () => {
  it("maps common role aliases to a stable key", () => {
    expect(sessionRoleKey("USER")).toBe("user");
    expect(sessionRoleKey("human")).toBe("user");
    expect(sessionRoleKey("assistant")).toBe("assistant");
    expect(sessionRoleKey("model")).toBe("assistant");
    expect(sessionRoleKey("gemini-2.0")).toBe("assistant");
    expect(sessionRoleKey("claude")).toBe("assistant");
    expect(sessionRoleKey("system")).toBe("system");
    expect(sessionRoleKey("tool_call")).toBe("tool");
    expect(sessionRoleKey("mystery")).toBe("other");
  });

  it("produces friendly labels and preserves unknown roles", () => {
    expect(sessionRoleLabel("user")).toBe("You");
    expect(sessionRoleLabel("assistant")).toBe("Assistant");
    expect(sessionRoleLabel("system")).toBe("System");
    expect(sessionRoleLabel("function")).toBe("Tool");
    expect(sessionRoleLabel("narrator")).toBe("narrator");
  });
});

describe("parseSessionTranscript — Codex rollouts (payload-wrapped)", () => {
  it("unwraps payload, renders the event_msg stream, and skips encrypted reasoning", () => {
    const text = [
      JSON.stringify({ type: "session_meta", payload: { cwd: "C:/Work/SampleProject" } }),
      JSON.stringify({ type: "event_msg", payload: { type: "user_message", message: "Push it to GitHub." } }),
      JSON.stringify({ type: "response_item", payload: { type: "reasoning", summary: [], content: null, encrypted_content: "gAAAAABoZ2VuY3J5cHRlZHJlYXNvbmluZ2Jsb2I" } }),
      JSON.stringify({ type: "event_msg", payload: { type: "agent_message", message: "Done — released v0.1.0 (commit 71e2686)." } }),
      JSON.stringify({ type: "response_item", payload: { type: "function_call", arguments: "{}" } })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns).toEqual([
      { role: "user", label: "You", content: "Push it to GitHub." },
      { role: "assistant", label: "Assistant", content: "Done — released v0.1.0 (commit 71e2686)." }
    ]);
    expect(turns.some((turn) => turn.content.includes("gAAAA"))).toBe(false);
  });

  it("falls back to response_item messages (content[].text) when no event_msg stream exists", () => {
    const text = [
      JSON.stringify({ type: "response_item", payload: { type: "reasoning", encrypted_content: "gAAAAencryptedblobblobblobblob" } }),
      JSON.stringify({ type: "response_item", payload: { type: "message", role: "user", content: [{ type: "input_text", text: "hello" }] } }),
      JSON.stringify({ type: "response_item", payload: { type: "message", role: "assistant", content: [{ type: "output_text", text: "hi there" }] } })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns.map((t) => [t.role, t.content])).toEqual([
      ["user", "hello"],
      ["assistant", "hi there"]
    ]);
  });

  it("renders an explicit timeline gap between recovered context and recent turns", () => {
    const text = [
      JSON.stringify({ type: "event_msg", payload: { type: "user_message", message: "The latest request" } }),
      JSON.stringify({ type: "event_msg", payload: { type: "session_gap", message: "Earlier activity is omitted." } }),
      JSON.stringify({ type: "event_msg", payload: { type: "agent_message", message: "The newest update" } })
    ].join("\n");

    expect(parseSessionTranscript(text)).toEqual([
      { role: "user", label: "You", content: "The latest request" },
      { role: "system", label: "Timeline", content: "Earlier activity is omitted." },
      { role: "assistant", label: "Assistant", content: "The newest update" }
    ]);
  });
});

describe("parseSessionTranscript — Claude Code (nested message.content)", () => {
  it("renders nested message content (string and typed parts) with the message role", () => {
    const text = [
      JSON.stringify({ type: "user", message: { role: "user", content: "Fix the discovery bug." }, cwd: "C:/AI/Codex/CodeHangar" }),
      JSON.stringify({
        type: "assistant",
        message: {
          role: "assistant",
          content: [
            { type: "text", text: "On it — let me read the file." },
            { type: "tool_use", name: "Read", input: { path: "lib.rs" } }
          ]
        }
      })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns).toHaveLength(2);
    expect(turns[0]).toMatchObject({ role: "user", label: "You", content: "Fix the discovery bug." });
    expect(turns[1].role).toBe("assistant");
    expect(turns[1].content).toBe("On it — let me read the file.\n\n↳ used Read");
  });

  it("omits tool results and internal thinking from the readable view", () => {
    const text = [
      JSON.stringify({ type: "assistant", message: { role: "assistant", content: [
        { type: "thinking", thinking: "secret chain of thought" },
        { type: "text", text: "Here is the answer." }
      ] } }),
      JSON.stringify({ type: "user", message: { role: "user", content: [
        { type: "tool_result", content: "a huge tool output blob", tool_use_id: "x" }
      ] } })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns).toHaveLength(1);
    expect(turns[0].content).toBe("Here is the answer.");
    expect(turns.some((t) => t.content.includes("secret chain of thought"))).toBe(false);
    expect(turns.some((t) => t.content.includes("huge tool output"))).toBe(false);
  });

  it("skips bookkeeping line types (queue-operation, summary, file-history-snapshot)", () => {
    const text = [
      JSON.stringify({ type: "queue-operation", operation: "enqueue", content: "queued text" }),
      JSON.stringify({ type: "summary", summary: "Conversation summary", leafUuid: "abc" }),
      JSON.stringify({ type: "file-history-snapshot", snapshot: { foo: 1 } }),
      JSON.stringify({ type: "assistant", message: { role: "assistant", content: "Real reply." } })
    ].join("\n");
    const turns = parseSessionTranscript(text);
    expect(turns).toHaveLength(1);
    expect(turns[0]).toMatchObject({ role: "assistant", content: "Real reply." });
  });
});
