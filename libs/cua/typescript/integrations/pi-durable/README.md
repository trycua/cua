# @trycua/pi-durable

Cua Spaces as the execution environment of [Pi Durable](https://github.com/earendil-works/pi/tree/main/packages/durable).

- `SpacesExecutionEnv` implements Pi Durable's `ExecutionEnv` on a Space. Commands run as cua-spacesd processes;
  files go through Pi's own `pi-env` daemon, reached over a pipe through cua-spacesd, so file behavior is Pi's.
- `durableSpaces()` keeps the Space consistent with the conversation log across crashes: idempotent, fenced commands
  (cua-spacesd's exec ledger), a checkpoint after every mutating tool call, and recovery that adopts, reattaches or
  restores instead of guessing. Forked conversations get a Space forked from the parent's checkpoint at the fork entry.

The guarantees and their limits are described in the
[Build a durable agent in code](https://cua.ai/docs/cua-sdk/guides/build-a-durable-agent) guide.

```ts
import { embedded, SandboxCreateOptions } from "@trycua/cua"
import { createRegistry, Harness } from "@earendil-works/pi-durable"
import { CodingTools } from "@earendil-works/pi-durable/tools"
import { CuaSandboxSpaces, durableSpaces, WorkspaceSnapshots } from "@trycua/pi-durable"

const spaces = new CuaSandboxSpaces({
  sandboxes: embedded().sandboxes(),
  createOptions: (name) => SandboxCreateOptions.create({ image: "linux", name }),
})
const durable = durableSpaces({
  spaces,
  snapshots: new WorkspaceSnapshots({ spaces, root: "/home/cua/work" }),
  cwd: "/home/cua/work",
})
const registry = createRegistry()
registry.install(CodingTools)
registry.install(durable.extension)
const harness = await Harness.open(storage, { models, registry, env: durable.env }, context)
```

Snapshot providers: `WorkspaceSnapshots` (one directory, any Space, optionally copied to a `CheckpointStore`) and
`SandboxSnapshots` (the whole machine through the cua daemon's `checkpoint`/`fork`: memory and disk on a microVM).

`durableSpaces({ mode })` is `"auto"` (default), `"transactional"` (the call's workspace diff is committed with its
outcome, no per-call checkpoint; small container-like workspaces) or `"checkpoint"`. Writes are fenced by a session epoch:
`openFencedSqliteStorage(path, epoch)` / `FencedFileSystem` for the log, `epoch` / `CUA_SESSION_EPOCH` for the Space, and a
stale writer gets `StaleEpochError` instead of a tool result. `runDurableSession()` is the driver entry a coordinator
launches per attempt (env `CUA_SESSION*`; exit 0 done, 78 permanent, 75 fenced). See the guide for the selection rule.

Requires a Space whose cua-spacesd reports the `process.exec_ledger` capability.

## Tests

```bash
npm test                                   # unit tests (no Space), including fence, transactional mode and a crash test on a local stand-in Space
CUA_PI_SPACESD_URL=http://127.0.0.1:3211 CUA_PI_SPACESD_TOKEN=... npm test   # + a real Space: Pi's ExecutionEnv conformance suite, ledger, fencing, checkpoints
npm run test:crash -- 50 8 results.jsonl   # crash injection: random SIGKILLs, Space restarts and losses (Docker; see test/crash/Dockerfile)
npm run test:fork                          # a forked conversation gets a Space forked at the fork entry
CRASH_BACKEND=microvm CRASH_SNAPSHOTS=machine CRASH_IMAGE=<image> npm run test:crash -- 120 10 out.jsonl
                                           # the same on microVM Spaces of the local cua daemon, whole-machine restores
```

The Docker-based tests create containers labelled `ai.cua.test=pi-durable` from a `cua-pi-space:dev` image built
with a cua-spacesd binary, and remove them when done.
