# jev-use with Cua Driver and TypeSafe Jev

`jev-use` is a public-preview recipe that uses TypeSafe Jev to choose the next browser action
while Cua Driver observes the page, performs that action, and verifies the
result. Equivalent Python and TypeScript programs run the same bounded loop.

The runnable path prefers fresh browser DOM and semantic evidence. It also
contains an optional adapter for the public `cua.visual_regions_v1` contract.
The checked-in typed fixtures exercise that adapter without a model or secret.
At runtime the agents use `parse_visual_regions` only when Driver also
advertises the capture-bound `click.capture_id` contract. They capture the
browser window with `get_window_state`, then keep that exact `capture_id` in the
click arguments. Otherwise they continue through the semantic path.

The TypeSafe-specific code lives in `python/jev_adapter.py` and
`typescript/jev_adapter.ts`, outside Driver. Both adapters send Jev the bounded
candidate IDs and descriptions plus a compact observation. When visual regions
are present, that observation includes their typed bounds and the exact
`capture_id`; it never includes screenshot bytes or extension internals. A live
answer must be one of the supplied IDs before the runner resolves it to the
original immutable action.

You can run the complete deterministic proof without credentials or network
access to Jev. If you have a TypeSafe API key, you can separately verify the
same loop against the live Jev service.

Follow the [setup guide](../../../../docs/content/docs/how-to-guides/driver/jev-use.mdx)
for installation and your first run. The sections below cover the implementation,
standalone runners, and development checks.

## What the example proves

The local fixture contains a small browser task and exposes its submitted value
through an independent `/state` endpoint. Each runner:

1. opens one persistent `cua-driver mcp` connection;
2. creates a Driver-owned isolated Chromium profile with `browser_prepare`;
3. navigates to the loopback fixture with typed browser tools;
4. captures a browser snapshot before asking for or performing an action;
5. constructs immutable candidates plus reserved `reobserve` and `abstain`
   choices;
6. asks the selected provider for one typed choice;
7. uses fresh page references for typing and semantic clicking, or one unique
   validated capture-bound visual Submit region when available; and
8. checks the fixture's `/state` endpoint instead of treating the action
   response or a screenshot as proof of success.

The MCP connection stays open across the entire loop. This preserves the
explicit named Cua Driver session and avoids rebuilding tool state for every
step.
Page references are snapshot-bound, so the runners take another snapshot after
the page changes rather than reusing an older reference.
Visual candidates are also bound to the exact capture ID and screenshot
reference. Driver receives the original screenshot-space point and capture ID
atomically, so a retired, changed, or mismatched capture is refused before
native dispatch. The agents never retry that refusal as an unbound coordinate
click. Stale, malformed, out-of-bounds, duplicate, or ambiguous visual results
produce no executable visual candidate.

## Install the prerequisites

Install Cua Driver so that `cua-driver` is on `PATH`, or set `CUA_DRIVER_BIN`
to its absolute executable path. You need an unlocked desktop and a signed
Chromium-based browser that Cua Driver can prepare. Python with `uv` supplies
the fixture and Python agent. Node.js 22 or later and npm are needed only for
the optional TypeScript agent.

The agents start `cua-driver mcp` with the MCP SDK's minimal default
environment plus the desktop-session variables (`DISPLAY`, `WAYLAND_DISPLAY`,
`XAUTHORITY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`,
`AT_SPI_BUS_ADDRESS`, `XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`) and any
`CUA_DRIVER_*` variables. Other variables, including `TYPESAFE_API_KEY`, are
not passed to the Driver. On Linux, run the agents from the logged-in desktop
session so those variables are set.

From this directory, install the locked Python dependencies:

```bash
uv sync --frozen --python 3.12
```

For the TypeScript agent, also run:

```bash
export npm_config_cache="$PWD/.venv/npm-cache"
npm ci
```

On Windows, use a non-administrator desktop session and `npm.cmd ci` in
PowerShell. The managed verifier starts TypeScript through `node --import tsx`,
not an npm shell shim. On Linux, use a supported system browser and a desktop
session accessible to the same user as Driver; native Wayland has separate
compositor-specific requirements.

Development validation evidence and environment details are recorded in the
[validation report](../../docs/jev-use-validation.md).

## Verify setup in one agent command

The managed verifier starts the existing fixture on an unused loopback port,
runs the existing agent, requires both its verified event and an independent
HTTP state match, and closes the fixture even when a runner fails. This avoids
background-process lifetime and cross-call signal restrictions in agent shells.

```bash
uv run --frozen python verify_setup.py --output-dir proof-mock
uv run --frozen python verify_setup.py --live --output-dir proof-live
```

Use a new output directory for every attempt. `--live` adds live Jev after the
mock check; it reads `TYPESAFE_API_KEY`, prompts securely in an interactive
terminal, or stops before starting if an unattended run lacks a key. After
installing the TypeScript dependencies, add `--typescript` to verify both
languages. `summary.json` must say `complete: true`; partial results remain
false. Each runner has four decisions and a 180-second process timeout. These
are not provider billing caps.

## Run the standalone fixture

For an application or terminal that already owns the server lifecycle, start
the loopback-only fixture in a separate persistent terminal:

```bash
uv run --frozen --python 3.12 python fixture_server.py
```

The fixture prints its local URL. Leave it running while you run either demo.

## Run the deterministic mock proof

The mock adapter returns deterministic typed choices through the same provider
boundary used by the live adapter. It proves the Cua Driver MCP lifecycle,
browser observation and action loop, snapshot-bound references, and independent
postcondition check. It does not prove that the TypeSafe service accepted the
request or made the same choices.

Run the Python example:

```bash
uv run python/run.py --provider mock
```

Run the TypeScript example:

```bash
npm run demo:mock
```

A successful run ends only after the exact submitted value appears at
`/state`.

Both runners also accept `--dry-run`, `--max-steps`, `--token`, and `--log`.
The optional log is JSONL and records the selected candidate, the full
probability vector, and decision/action timings. The candidate set always
includes `reobserve` and `abstain`; reobservation sends no Driver action. Final
outcomes are `verified`, `refuted`, `unknown`, `abstained`, or
`budget_exhausted`. An
uncertain action failure becomes `unknown` and is never retried blindly.

## Verify against live Jev

Live verification is optional and requires your own TypeSafe credential. Set
`TYPESAFE_API_KEY` in the environment, then run one of these commands:

```bash
uv run python/run.py --provider live
```

```bash
npm run demo:live
```

The live adapter sends the task state, any typed visual-region summary, and one
bounded choice question to TypeSafe. It rejects an answer outside the supplied
candidate table.
The runner still owns the control loop: Jev selects one bounded next action,
Cua Driver performs it, and the fixture's `/state` endpoint establishes the
postcondition. Keep sensitive page content out of live runs unless sending it
to the configured provider is appropriate.

The repository's credential-free checks do not establish live Jev behavior.
Record live verification separately when you run it with a valid key.

## Use the bounded chooser CLI

Applications that already own capture, candidate construction, execution, and
verification can call the provider adapter as a single-request process. The
required Python interface reads one `cua.jev_choice_request_v1` JSON document
from stdin and writes one `cua.jev_choice_v1` response to stdout:

```bash
uv run python/choose_action.py --mock < fixtures/jev-choice-request-v1.json
uv run python/choose_action.py < fixtures/jev-choice-request-v1.json
```

The second command is live and lets the TypeSafe SDK read `TYPESAFE_API_KEY`
from its normal environment. The TypeScript equivalent is:

```bash
npm run choose:mock -- < fixtures/jev-choice-request-v1.json
npm run choose:live -- < fixtures/jev-choice-request-v1.json
```

Requests contain a goal, one `capture_id`, compact typed regions, bounded
history, and at most 32 candidate IDs with descriptions. `reobserve` and
`abstain` are required. Tool names, action arguments, screenshot bytes, and
environment data are rejected. Responses contain only the schema, selected
allowlisted ID, provider model identity when available, confidence, and
probabilities. The chooser never executes an action or verifies completion.

## Automation environment and diagnostics

An automation host needs access to the network, loopback sockets, and writable
workspace, package-cache, installation, and temporary directories. On macOS,
`getconf DARWIN_USER_TEMP_DIR` identifies the system user-temp directory. If a
shell sandbox denies installation there, ask its operator to authorize the
specific path; do not disable the sandbox or change shared directory ownership.
For npm, the local cache export above avoids a root-owned global cache.

A run reports `verified` only after an independent fixture readback. If an action
fails with an uncertain outcome, inspect the retained log before retrying.
`--dry-run` on a standalone runner suppresses the candidate action but still
resets the fixture and prepares/navigates the browser; it is not side-effect-free.

The managed verifier bounds each child runner to 180 seconds. Provider SDKs may
retry network requests, so action and process limits do not guarantee a billing
cap. `decision_ms` remains the backward-compatible aggregate from the start of
semantic observation through validated provider choice. Step events also split
that interval into `semantic_observe_ms`, `visual_observe_ms`,
`candidate_build_ms`, and `provider_decision_ms`; `action_ms` remains
separate, and `total_step_ms` measures through the end of the action (or
reobserve decision). The small difference between the aggregate and named
sub-phases is local validation/bookkeeping rather than an unreported model or
Driver call. Logs contain choices and timings, not a complete request,
token-usage, or billing audit. The fixture tests integration rather than general
agent capability.

## MCP, CLI, and perception boundaries

The Python and TypeScript programs are the two complete agent loops. They use a
persistent MCP transport and repeat one explicit session label because browser
targets and snapshot refs belong to that session. Use the Cua Driver CLI for
installation and diagnostics,
for example `cua-driver doctor` and `cua-driver status`, rather than maintaining
a third copy of the loop.

This fixture exposes semantic browser refs, so the normal proof does not need
screenshot perception. The optional visual path consumes only the public
`parse_visual_regions` structured result and never adds a model, extension, or
Driver internals to this example. It validates capture identity, PNG geometry,
coordinate mapping, region IDs, bounds, content, confidence, and ambiguity
before constructing a `click` candidate containing the exact `capture_id`.
Visual evidence never replaces the semantic editable ref required by
`browser_type`.

The credential-free tests load `fixtures/parse-visual-regions-*-v1.json` to
exercise the same parser, candidate builder, and mock Jev boundary used by the
runtime adapter. Separate live-adapter contract tests use the official SDK with
a local fake transport to prove that the request contains the capture ID and
regions and that Jev can return only a supplied candidate ID.
They do not claim that an optional perception extension is installed or assess
its inference quality.

## Run the checks

The tests use the deterministic provider and do not require
`TYPESAFE_API_KEY`:

```bash
uv run python -m unittest discover -s python/tests
npm test
npm run typecheck
```

## Other decision models

For an optional Python-only closed-candidate interface spanning Jev and local
Cua-S1-4B, see [Closed-candidate decision models](decision-models.md). It does
not change the existing Python or TypeScript runners.

## Relationship to PR #3914

[PR #3914](https://github.com/trycua/cua/pull/3914) proposes an optional Jev
policy head inside the Cua Driver binary. This directory is a complementary
standalone example: it composes the public TypeSafe SDKs with the existing Cua
Driver MCP surface and keeps the decide-act-verify loop in application code.
It does not depend on the proposed `suggest_action` tool.

## Acknowledgments

The example was inspired by
[`awlevin/typesafe-computer-use`](https://github.com/awlevin/typesafe-computer-use),
an MIT-licensed early TypeSafe computer-use integration. This implementation
uses Cua Driver's current typed browser and MCP contracts and does not copy its
source code.
