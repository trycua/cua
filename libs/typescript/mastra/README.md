# Cua Fleet for Mastra

`CuaFleetSandbox` gives a Mastra agent computer tools backed by a Linux desktop
in Cua Fleet. It uses `@trycua/fleet` directly and implements Mastra's
`WorkspaceSandbox` interface. The provider creates a claim from an existing pool;
the application owns the pool's capacity and cleanup policy.

This package is in draft and has not been published to npm. The implementation
tracks [RFC #3640](https://github.com/trycua/cua/issues/3640).

## Develop locally

This integration has an independent npm lockfile so its pinned Mastra peer can
be checked without rebuilding the other SDKs. From `libs/typescript/mastra`:

```bash
npm ci --ignore-scripts
npm run typecheck
npm test
npm run build
npm run lint
npm pack --dry-run
```

Requires Node.js 22.18 or later. This draft is tested against `@mastra/core`
1.62.0 and `@trycua/fleet` 0.1.1. The package exports ESM and TypeScript
declarations from `dist`. For a local consuming application, run `npm pack`
and install the resulting tarball plus the matching Mastra peer.

## Attach a Fleet desktop to an agent

```ts
import { Agent } from '@mastra/core/agent';
import { Workspace } from '@mastra/core/workspace';
import { CuaFleetSandbox } from '@trycua/mastra';

const sandbox = new CuaFleetSandbox({
  id: 'unique-application-session',
  poolName: 'existing-linux-pool',
  clientId: process.env.CUA_CLIENT_ID!,
  clientSecret: process.env.CUA_CLIENT_SECRET!,
});

const agent = new Agent({
  id: 'desktop-agent',
  name: 'Desktop Agent',
  instructions: 'Use the computer tools to complete the requested task.',
  model: process.env.MASTRA_MODEL!,
  workspace: new Workspace({ sandbox }),
});

try {
  const result = await agent.generate('Describe the desktop.', { maxSteps: 10 });
  console.log(result.text);
} finally {
  await sandbox.destroy();
}
```

Configure the API key required by the selected Mastra model separately. Keep
Fleet client credentials server-side, in your secret manager or protected
process environment. The SDK exchanges and refreshes OAuth credentials;
credentials are not included in provider metadata or raw error messages.

The pool must contain a compatible Linux desktop image with computer-server
exposed as the `server` service. This provider is fixed to `run.cua.ai` and its
Fleet OAuth endpoint. It does not install software or provision a pool.

## Computer tools

A concrete `Workspace({ sandbox })` automatically registers Mastra's computer
tools for screenshots, left/right/double clicks, mouse movement, drag, typing,
key presses, scrolling, screen information, and waiting. Screenshots are PNG
bytes. Coordinates are integer display pixels from the top-left; scrolling
uses positive integer wheel ticks.

Examples: `press('Enter')`, `press('ArrowDown')`, and
`press(['Control', 's'])`. Single characters preserve case. Unknown named keys
are rejected instead of silently typing their names. Actions are serialized
within one instance so chords, dragging, and screenshots do not overlap.

Each concurrent application session needs a separate sandbox and workspace.
Mastra 1.62.0 does not discover computer tools from a dynamic sandbox resolver.
Sharing a provider instance shares its desktop.

## Lifecycle and limits

- Construction performs no network operations. `start()` or the first computer
  operation acquires a claim. Concurrent starts share one acquisition.
- `id` is an application identity, not a saved Fleet claim ID. Creating another
  instance with the same `id` still creates an independent claim.
- `destroy()` rejects new actions, waits for active work, releases only its
  owned claim, and verifies claim absence. It leaves the caller's pool intact.
  If cleanup fails, the instance retains cleanup state; retry `destroy()`.
- `stop()` rejects with `PAUSE_UNSUPPORTED_USE_DESTROY`. The provider does not
  pretend a released desktop can resume unchanged.
- The claim has a one-hour creation-age TTL by default. It is not an inactivity
  timer or automatic renewal, and startup time counts toward it. Configure
  `claimTtlSeconds` (maximum one day) for the intended session. A pool's own
  expiry can also end the desktop. TTL is not a spending cap.
- `startupTimeoutMs` defaults to 900000; `requestTimeoutMs` defaults to 60000.
  Configure a claim TTL longer than startup. Requests refuse redirects and
  have bounded HTTP timeouts. GUI commands are not replayed automatically on
  ambiguous failures, since replay can duplicate clicks or text.
- Shell execution, files, processes, live viewing, snapshots, cloning, and
  other operating systems are not advertised. `snapshot()` is Mastra's no-op
  compatibility hook and `supportsCheckpoints` is false.

## Live example

The example creates a temporary Linux pool, starts the provider, installs a
synthetic guest-local webpage, and invokes an agent to enter a unique value and
click its submit button. A separate authenticated guest command reads the
fixture's recorded value; the agent's final text is not the success oracle.
It then starts a second concurrent provider, checks that the two claims use
different guests, and verifies that the second guest cannot see the first
guest's fixture state. Both claims are released before pool cleanup.

```bash
# Set CUA_CLIENT_ID and CUA_CLIENT_SECRET securely first.
# Select a vision-capable model and configure its provider API key.
export MASTRA_MODEL='your-provider/your-model'
npm run example
```

For deterministic orchestration/GUI proof without a model API key:

```bash
npm run example -- scripted
```

Scripted mode uses a fixed LanguageModelV2 fixture with the real Mastra agent
loop and real desktop tools. It does not prove autonomous model reasoning.
The fixture uses fixed geometry in a kiosk browser; model mode locates controls
from screenshots. Screenshots and a small result record go to ignored `output/`.

The live example incurs Fleet usage, briefly holds two concurrent desktops,
creates temporary warm capacity, and uses
one-hour pool and claim TTLs. Its `finally` blocks destroy the provider and
delete only its exclusively reserved namespace. A successful namespace listing
without that namespace is cleanup proof at the account API level, not an
independent infrastructure termination check.

If interrupted, keep `output/run.json`, restore credentials for the same
account, and run:

```bash
npm run example -- cleanup
```

Cleanup checks the namespace creation timestamp before deletion and never
provisions resources. If reservation was not confirmed, the record has no
creation timestamp and cleanup refuses deletion: inspect the recorded random
namespace in Fleet before taking any manual action. A forbidden request is not
proof of resource absence. After confirmed cleanup, archive `output/` before
another run; the example refuses to overwrite a previous recovery record.
