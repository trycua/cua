# Agents in sandboxes

Run coding-agent harness X in image Y over a task file, fire-and-forget,
then collect verified results into a JSON report. Uses the cua SDK's agents
API (`sandbox.agents()`); see
[Run a coding agent in a sandbox](https://cua.ai/docs/reference/cua-sdk/run-a-coding-agent).

| File | What |
| --- | --- |
| `agents_in_sandboxes.py` | `launch`, `collect`, or `run` (both) |
| `tasks.jsonl` | Two tiny tasks, each with a shell `check` |
| `tour.py` | The docs page's code as one script |
| `tests/` | Hermetic tests for the pure logic |

## Tasks

One JSON object per line:

```json
{"id": "hello", "prompt": "Create a file hello.txt whose only line is: hello", "check": "grep -qx hello hello.txt"}
```

Each task runs in its own directory (`--workdir`, default
`/tmp/agents-in-sandboxes/<id>`). `check` is the verifier: a shell command run
there inside the sandbox after the run finishes; exit 0 passes. `mock` is a
script for the mock provider (see below). `timeout_s` caps how long `collect`
waits for the task.

## Run it with a real key

```bash
pip install cua
export ANTHROPIC_API_KEY=...
python agents_in_sandboxes.py launch --image ghcr.io/trycua/linux:24.04 \
  --harness claude-code --tasks tasks.jsonl --key-var ANTHROPIC_API_KEY \
  --mcp docs=https://mcp.example.com/mcp
# ... later, from any process:
python agents_in_sandboxes.py collect --wait 600 --out report.json --delete
```

`launch` creates the sandbox, installs the harness, starts one run per task
with `exit_when_idle` and writes `runs.json`. The runs keep going without it.
`collect` reconnects, finds the runs (`agents.list()`, by label), waits up to
`--wait` seconds for each, reads `run.result()` and `run.artifacts()`, runs
each `check`, and writes `report.json`:

```json
{"image": "...", "harness": "claude-code", "sandbox": "local:agents-in-sandboxes",
 "summary": {"tasks": 2, "finished": 2, "pending": 0, "passed": 2, "failed": 0, ...},
 "tasks": [{"id": "hello", "run_id": "run-...", "status": "idle", "stop_reason": "end_turn",
            "text": "...", "usage": {"input_tokens": ...}, "artifacts": [...], "verified": true}]}
```

Other options: `--image` takes any image with cua-spacesd (a registry image or
a benchmark image), `--connect local:NAME` reuses a sandbox, `--cloud` creates
it on Fleet, `--harness` takes any id from `cua agent harnesses`,
`--base-url`/`--model` point it at a proxy or gateway, `--no-sandbox-mcp`
drops the sandbox's own MCP tools. The exit code is 1 when a check failed.

## Run it against the mock provider

`cua-mock-llm` (in `libs/cua`) is a scripted provider that speaks the
Anthropic, OpenAI and Gemini wire formats. With `--scripted`, each task's
`mock` field is appended to its prompt as a `mock:` directive, and the mock
makes the harness run that shell command for real in the sandbox. The model
replies are scripted, not generated; the harness, its install, its tools, the
MCP wiring and the verifiers are real.

```bash
# build the mock (Linux binary) and start it where the sandbox can reach it
cd libs/cua && cargo zigbuild --release -p cua-mock-llm --target aarch64-unknown-linux-musl
MOCK_KEY=mock-$RANDOM$RANDOM
docker run -d --name cua-e2e-mock-llm --memory=256m -e CUA_MOCK_LLM_KEY=$MOCK_KEY \
  -v "$PWD/target/aarch64-unknown-linux-musl/release/cua-mock-llm:/usr/local/bin/cua-mock-llm:ro" \
  debian:bookworm-slim cua-mock-llm --listen 0.0.0.0:8787
MOCK_IP=$(docker inspect -f '{{.NetworkSettings.Networks.bridge.IPAddress}}' cua-e2e-mock-llm)

cd examples/agents-in-sandboxes
ANTHROPIC_API_KEY=$MOCK_KEY python agents_in_sandboxes.py run --name cua-e2e-ais \
  --harness claude-code --tasks tasks.jsonl --key-var ANTHROPIC_API_KEY \
  --base-url http://$MOCK_IP:8787 --model claude-mock-1 --scripted --delete
docker rm -f cua-e2e-mock-llm
```

The same flags run `tour.py` against the mock:
`ANTHROPIC_API_KEY=$MOCK_KEY OPENAI_API_KEY=$MOCK_KEY python tour.py --base-url http://$MOCK_IP:8787`.

## Tests

```bash
uv run --no-project --with pytest pytest examples/agents-in-sandboxes/tests -q
```

They cover task parsing, MCP flags, the verifier line, usage parsing and the
report, and check statically that every SDK call here and in `tour.py` exists
in the generated Python binding.
