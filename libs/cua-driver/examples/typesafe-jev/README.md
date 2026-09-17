# TypeSafe Jev with Cua Driver

This standalone example uses TypeSafe Jev to choose the next browser action
while Cua Driver observes the page, performs that action, and verifies the
result. Equivalent Python and TypeScript programs run the same bounded loop.

You can run the complete deterministic proof without credentials or network
access to Jev. If you have a TypeSafe API key, you can separately verify the
same loop against the live Jev service.

**Starting without prior context?** Follow the complete
[step-by-step setup guide](../../../../docs/content/docs/how-to-guides/driver/use-typesafe-jev.mdx).
It includes the unmerged-PR checkout, macOS permissions, Python provisioning,
secure credential entry, exact expected results, independent checks, and cleanup.

## What the example proves

The local fixture contains a small browser task and exposes its submitted value
through an independent `/state` endpoint. Each runner:

1. opens one persistent `cua-driver mcp` connection;
2. creates a Driver-owned isolated Chromium profile with `browser_prepare`;
3. navigates to the loopback fixture with typed browser tools;
4. captures a browser snapshot before asking for or performing an action;
5. asks the selected provider for one typed choice;
6. uses fresh page references for `browser_type` and `browser_click`; and
7. checks the fixture's `/state` endpoint instead of treating the action
   response or a screenshot as proof of success.

The MCP connection stays open across the entire loop. This preserves the
explicit named Cua Driver session and avoids rebuilding tool state for every
step.
Page references are snapshot-bound, so the runners take another snapshot after
the page changes rather than reusing an older reference.

## Install the prerequisites

Install Cua Driver so that `cua-driver` is on `PATH`, or set `CUA_DRIVER_BIN`
to its absolute executable path. You need an unlocked desktop and a signed
Chromium-based browser that Cua Driver can prepare. Python with `uv` supplies
the fixture and Python agent. Node.js 20 or later and npm are needed only for
the optional TypeScript agent.

From this directory, install the locked Python dependencies:

```bash
uv sync --frozen --python 3.12
```

For the TypeScript agent, also run:

```bash
npm ci
```

Start the loopback-only fixture in a separate terminal:

```bash
uv run --frozen --python 3.12 python fixture_server.py
```

The fixture prints its local URL. Leave it running while you run either demo.

## Run the deterministic mock proof

The mock provider returns deterministic typed choices through the same provider
boundary used by the live integration. It proves the Cua Driver MCP lifecycle,
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
probability vector, and decision/action timings. Final outcomes are
`verified`, `refuted`, `unknown`, `abstained`, or `budget_exhausted`. An
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

The live provider sends the task state and typed choice question to TypeSafe.
The runner still owns the control loop: Jev selects one bounded next action,
Cua Driver performs it, and the fixture's `/state` endpoint establishes the
postcondition. Keep sensitive page content out of live runs unless sending it
to the configured provider is appropriate.

The repository's credential-free checks do not establish live Jev behavior.
Record live verification separately when you run it with a valid key.

## MCP, CLI, and OCR boundaries

The Python and TypeScript programs are the two complete agent loops. They use a
persistent MCP transport and repeat one explicit session label because browser
targets and snapshot refs belong to that session. Use the Cua Driver CLI for
installation and diagnostics,
for example `cua-driver doctor` and `cua-driver status`, rather than maintaining
a third copy of the loop.

This fixture exposes semantic browser refs, so OCR would add weight without
improving the proof. For canvas, streamed desktop, or other non-semantic
surfaces, an optional OmniParser worker can enrich the observation before
candidate construction. Keep that worker out of the base install, persist it
across steps, pin its model/code versions and licenses, and still let Cua
Driver own targeting, freshness, execution, and postcondition verification.

## Run the checks

The tests use the deterministic provider and do not require
`TYPESAFE_API_KEY`:

```bash
uv run python -m unittest discover -s python/tests
npm test
npm run typecheck
```

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
