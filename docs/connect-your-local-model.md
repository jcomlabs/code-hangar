# Connect your local model

Code Hangar AI Connector can run its optional review, explanation and learning features against
an OpenAI-compatible model server on this PC. Code Hangar does not download, install, start or
update a model server; keep that lifecycle under the server application you already trust.

The Local edition has no AI/network code. Install the **Code Hangar AI Connector** edition for
this workflow. Local mode needs no API key and refuses non-loopback destinations.

## Connect

1. Start your model server and load a model. Ollama, LM Studio and other servers work when they
   expose a compatible loopback HTTP API.
2. In Code Hangar, open **Settings -> System -> AI Assist**.
3. Choose **Local model server**.
4. Press **Find local models**. This explicit action checks only these fixed numeric endpoints,
   one at a time: `127.0.0.1:11434`, `:1234`, `:8000`, and `:8080`.
5. Choose a detected server/model, or enter a loopback base URL and model name manually.
6. Press **Save**, then **Test provider**. Testing sends only a fixed ping and counts as one model
   call in the current session meter.

A common Chat Completions base URL looks like:

```text
http://127.0.0.1:11434/v1
```

The final request is sent below that base URL. Code Hangar validates the exact final URL as
loopback again for every request. LAN addresses, public hosts, redirects away from loopback and
proxy routing are refused in Local mode.

## Before each send

- Open **Exactly what is sent** to inspect the final URL and literal JSON request body. Auth
  headers are never included in this disclosure.
- Sensitive paths, Protected Zones, likely secrets, binary input, stale selections and oversized
  context are blocked before any request.
- Loopback providers stream when supported. A server that explicitly rejects streaming may get
  one disclosed non-streaming fallback; an external API never gets that retry.
- The session meter estimates input and observed output tokens. Its soft cap is an advisory warning,
  not a hard block or provider bill. Local calls have no per-token API charge, although running the
  model still uses your machine's CPU/GPU and power.

## Small-model settings

Start with the **Vibe coder** explanation level and a single file or short selection. The prompts
ask for bounded, plain-language output and the app limits response size. For a smaller model:

- use **Explain** before **What to check**;
- select one narrow passage instead of a whole file;
- use a walkthrough with only one or two sections selected;
- ask one concrete follow-up at a time;
- keep the model's own context window above the disclosed request estimate plus the shown output
  allowance.

If a model returns prose around a requested correction, Code Hangar still stages only a bounded
proposal and requires the exact before/after review, validation, snapshot and confirmation before
any local write.

## Troubleshooting

**No compatible server answered**

Confirm that the server is running, its OpenAI-compatible API is enabled, and it listens on one of
the fixed discovery ports. Otherwise enter its numeric `127.0.0.1` URL manually.

**No models appear**

Some compatible servers do not expose `/models`. Type the model identifier exactly as the server
expects; model listing is optional.

**Test works but a file is refused**

Provider reachability and content policy are separate. Read the "Not sent" reason; Code Hangar
will not weaken a Protected Zone or secret gate for a working provider.

**Response stops partway through**

The app preserves readable partial local output and reports the stream failure. It does not loop
through paid retries. Check the model server log and retry explicitly after resolving the local
server error.

**Nothing should contact a model**

Set AI Assist to **Off**. For physical absence of HTTP/provider code, use the Local edition instead.
