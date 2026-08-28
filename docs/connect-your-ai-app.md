# Connect your AI app to Code Hangar

This guide is for the **AI Connector edition** of Code Hangar. It explains how to let an AI coding app (Claude Code, Cursor, ChatGPT through Codex CLI, …) read your curated project knowledge, how the "total control" model keeps you in charge, and the caveats worth knowing.

> If you are running the **Local** edition, there is nothing to connect — it
> contains no AI/MCP integration or external-network client by design. The
> **Connector** adds two separate opt-in surfaces: local MCP/app-configuration
> integration, and experimental AI Assist. The whole Connector edition is
> optional, experimental and less tested than Local. MCP stays on local stdio. AI Assist
> starts off and can contact only the local model or external provider the user
> explicitly selects, after showing the frozen destination and exact redacted
> request for approval. There is no implicit or background outbound traffic.

---

## What "connecting" actually does

Connecting an AI app registers Code Hangar as an **MCP server** (Model Context Protocol) inside that app's configuration. Once connected, the AI app can read a **curated, body-free view** of the explicitly granted projects — structure, dependency and model metadata, comments, and redacted session search — over local stdio only. This MCP path never contacts the internet and is distinct from the Connector's separately configured AI Assist provider.

The important part is what the AI app **cannot** do: it never changes anything itself. For any change — editing a comment, reading a file's contents, backing up or moving or removing files — it **files a request** that waits for **your approval inside Code Hangar**. Code Hangar then performs the action **as you**, re-checking every safety gate first. This is the "total control" model, described below.

---

## Before you start

- You need the **AI Connector edition** installed (the Local edition hides this surface entirely).
- The AI app you want to connect (e.g. Claude Code) should be installed, so Code Hangar can find and safely edit its configuration file.
- The Connected-apps panel lives under **Settings → Advanced**, so **Advanced mode** must be on.

---

## Connect an app (step by step)

1. Open the **AI Connector edition** of Code Hangar.
2. Go to **Settings → Advanced → AI app integration** (the *Connected apps* panel). Each AI app Code Hangar recognises is listed with its status: **Connected**, **Not connected**, **No config yet**, or **Config unreadable**.
3. Decide **which projects** to share. By default a connection has no project grants until you add them; grant only the projects you want that app to see.
4. Click **Connect** on the app you want (for example, *Claude Code*). Code Hangar asks you to confirm, then:
   - **mints a per-app token** (each app gets its own; the token is stored only as a hash, bound to your Windows user via DPAPI — the raw value is not kept), and
   - **safely edits that app's config** to add the `code-hangar` MCP server: it backs the file up first and verifies the backup, changes **only** its own entry (every other key is preserved), writes atomically, and re-reads to confirm. If the config can't be parsed, Code Hangar refuses to touch it.
5. **Restart the AI app** and check its MCP server list — `code-hangar` should now appear.

To undo a connection, click **Disconnect**. Code Hangar removes only its own entry from that app's config and revokes that app's token; your other MCP entries are left untouched.

---

## The total-control model (how you stay in charge)

The differentiator: **the AI app never executes anything itself.** It can read a curated project surface, and for any change it files a request that you approve inside Code Hangar. Code Hangar then acts as you.

You control the surface with three settings in the Connected-apps panel, all **off by default**:

- **Read-only mode — freeze all AI writes** (the panic switch). When on, connected apps can still *read* your curated knowledge, but every write, comment change and action is refused — including any request already waiting for your approval. One flip freezes everything.
- **Allow AI apps to write comments.** When on, a connected app can add and edit **its own** comments only. It can never change a comment you wrote.
- **Give AI total control (advanced).** For a trusted, capable app only. This lets it **file** privileged requests — comment changes, a temporary file-content read, a protected backup, a move to the holding area, or a recommendation to review final removal of a held item. It does **not** let it execute them. Nothing changes until you approve each request, and Code Hangar re-checks the safety gates.

When you approve a privileged request, Code Hangar re-authorises the requesting app, re-checks project scope, and rebuilds and re-validates any applicable plan before acting as you. A connected-app request for **final, permanent removal** is deliberately different: approving an unscoped held-entry request cannot delete anything. Permanent removal is a single owner-controlled capability that starts OFF; while it is off, both connected-app recommendations and Local preview/confirmation/start are refused. Enabling it in **Recovery & cleanup** requires typing `ENABLE PERMANENT REMOVAL` exactly. The app then builds an object-complete, immutable project/batch preview and still requires a fresh confirmation for that exact selection. Turning the capability off asks any active batch to stop after its current topology group and is serialized with the Local execution worker; once the setting reports OFF, no admitted worker can continue to helper resolution, token consumption or deletion.

There is also a read-only "approve as preview" path: you can acknowledge a request without executing it.

---

## Caveats worth knowing

- **The MCP server works with the Code Hangar desktop app closed.** The AI app launches the `code-hangar-mcp` server as a child process, and it opens the encrypted catalog directly. You do not need the Code Hangar window open for a connected app to read your curated knowledge. (Approving a privileged request, however, happens in the Code Hangar app — so for anything beyond reading, you will open it.)
- **It coexists with your other MCP clients.** Code Hangar only ever adds, updates or removes its own `code-hangar` entry in an app's config. Every other MCP server you have configured is preserved.
- **Running the server executable by hand does nothing useful.** `code-hangar-mcp.exe` needs a per-app token that only Code Hangar mints. Launched by hand (without that token) it **exits immediately with a clear message** — it is meant to be started by the AI app, not run directly.
- **The token lives in the AI app's config in plain text.** Each app's config stores its token so the app can authenticate. Treat it as a same-Windows-user secret: it is bound to your Windows account (the encrypted catalog won't open for another user or on another machine), but anyone who can read that app's config on your logged-in account can read the token. Disconnecting revokes it.
- **After a "Reset all", every connected app must be reconnected.** *Reset all* wipes Code Hangar's local index — which includes the app registry and the stored token hashes — so previously connected apps will no longer authenticate. Re-run the connection flow for each app you want back.

---

_See also: [`README.md`](../README.md) for the overview, [`docs/total_control_extension.md`](total_control_extension.md) for the request-approval design, and [`SECURITY_INVARIANTS.md`](../SECURITY_INVARIANTS.md) for the full security model._
