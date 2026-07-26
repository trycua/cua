# Cua Driver SDK and agent examples

These executable examples cover two intentionally different boundaries:

- **Native application route:** Python or Node imports `cua-driver` in process.
  Claude Agent SDK can wrap a narrow set of those calls as in-process custom
  tools.
- **MCP agent route:** Claude or Codex launches `cua-driver mcp`. The agent SDK
  already supplies the language-independent MCP client.

Codex SDK does not expose direct custom-tool callbacks. Its Cua Driver examples
therefore use MCP rather than inventing a bespoke native adapter.

## Deterministic perceive → act → verify fixture

Start the loopback-only fixture in one terminal:

```bash
python3 fixture_server.py
```

It opens a page with one autofocus input. The native examples capture the
desktop, type a unique value, press Enter, and verify the submitted value
through the independent `/state` endpoint. A timeout after a mutation is
treated as an unknown outcome: the examples observe the fixture before any
retry.

Python:

```bash
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python native_driver.py
```

TypeScript:

```bash
npm install
npm run native
```

Grant Screen Recording and Accessibility/Input permission to the importing
Python or Node process. Keep the fixture browser window focused when the native
example starts.

## Claude Agent SDK: native or MCP

Python:

```bash
.venv/bin/python claude_agent.py --route native \
  "Enter a short value in the fixture and submit it"

.venv/bin/python claude_agent.py --route mcp \
  "Open Calculator, compute 19 * 23, and report the result"
```

TypeScript:

```bash
npm run claude -- --route native \
  "Enter a short value in the fixture and submit it"

npm run claude -- --route mcp \
  "Open Calculator, compute 19 * 23, and report the result"
```

The native route supplies four narrow in-process callbacks: observe, click,
type, and key press. The MCP route discovers the complete live Cua Driver tool
surface. Set `CLAUDE_MODEL` optionally.

## Codex SDK: MCP

Python:

```bash
.venv/bin/python codex_agent.py \
  "Inspect the active app and summarize what is visible without changing it"
```

TypeScript:

```bash
npm run codex -- \
  "Inspect the active app and summarize what is visible without changing it"
```

Set `CODEX_MODEL` optionally. Both scripts create a unique Cua session outside
the agent, configure `cua-driver mcp`, and end the session in `finally`.

## Environment and safety

- Put `cua-driver` on `PATH`, or set `CUA_DRIVER_BIN` for MCP examples.
- Set `CUA_CAPTURE_SCOPE` to `auto`, `window`, or `desktop` for MCP examples.
- Authenticate the chosen agent SDK using its normal local login or API key.
- Run only trusted tasks. These examples remove interactive approval prompts.
- Keep purchases, messages, deletion, credential entry, and other irreversible
  actions out of unattended prompts.

## Credential-free checks

```bash
python3.12 -m py_compile \
  claude_agent.py codex_agent.py fixture_server.py native_driver.py native_tools.py
python3.12 -m unittest test_fixture_server.py
npm run typecheck
```
