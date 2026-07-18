# Screenshot provenance

These images are submission-preparation evidence captured locally on
17 July 2026. They contain synthetic `fixture://` data only.

## Capture environment

- Code Hangar Connector browser fixture
- fixture state: `acceptanceState=saturated`
- local URL origin: `http://127.0.0.1:4177`
- browser console: zero warnings, zero errors during the reviewed journey
- capture host: Windows 11 Pro x64, build 26200

The development server was stopped after capture and port 4177 was confirmed
closed. No OpenAI key was entered and no model request was sent.

## Files

| File | Deterministic state shown | Limitation |
|---|---|---|
| `01-what-changed.jpg` | Evidence-first What changed view with coverage and review context | Fixture state; not a native filesystem scan |
| `02-hangar-map.jpg` | 1,365 total items, 299 loaded, and 348 dependency-cache observations kept separate | Synthetic large-project stress fixture |
| `03-gpt56-preset.jpg` | Connector settings with OpenAI GPT-5.6 endpoint/model preset and no API key | Configuration/contract UI only; not a live GPT-5.6 response |

## Reproduce the fixture

```powershell
npm --workspace apps/desktop run dev:connector -- --port 4177
```

Then open:

```text
http://127.0.0.1:4177/?acceptanceState=saturated
```

These screenshots may support the judge narrative, but they must never replace
the owner-authorized native live-model proof required for the final public demo.
