// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// One Pi Durable worker process of the crash test: a scripted model drives
// write, edit and bash calls in a Space through `durableSpaces`. It prints
// `MARK <point>` at every step a fault can be injected after, and `DONE` once
// the run's submission has settled. The runner SIGKILLs it at random marks
// and starts it again over the same storage.
//
// Two ways to be started:
//
//  - by run.mjs (CRASH_* environment): CRASH_DB, CRASH_SEED, CRASH_STEPS, CRASH_SPACE,
//    CRASH_TOKEN, CRASH_WORK, CRASH_STORE, the backend choice of backend.mjs, and
//    - CRASH_DURABLE_MODE=checkpoint|transactional|auto (durableSpaces `mode`),
//    - CRASH_EPOCH=<n> to fence the log and the Space at that epoch (a worker the
//      Space or the log has moved past prints `STALE ...` and exits 75).
//  - as a session driver (CUA_SESSION set, the coordinator's launch contract): the
//    log, the epoch and the Space come from the contract through runDurableSession();
//    CRASH_SEED, CRASH_STEPS and CRASH_DURABLE_MODE come from the session's env, and the
//    Space is reached at CUA_SESSION_SPACE_ADDR with CUA_SESSION_SPACE_TOKEN (with
//    CRASH_BACKEND=local, the stand-in Space in CRASH_LOCAL_DIR instead). A driver
//    prints `EPOCH <epoch> <attempt> @<ms>` first, `MARK <point> @<ms>` at every
//    fault point, `VERIFY <json>` (workspace against log) and `DONE ...` last, and can
//    fault itself: CRASH_SELF_FAULTS={"<attempt>":{"kind":"kill"|"freeze","point":..,
//    "occurrence":..,"delay":..}} makes it print `KILL <point> @<ms>` and SIGKILL (or
//    SIGSTOP) itself that long after the occurrence-th mark of that point; kind "hold"
//    just stands still for `ms` there (the runner removes the driver's container meanwhile).

import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall, createModels } from "@earendil-works/pi-ai"
import { createRegistry, Harness } from "@earendil-works/pi-durable"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node"
import { CodingTools } from "@earendil-works/pi-durable/tools"
import { durableSpaces, ImageDoc, isStaleEpochError, openFencedSqlite, runDurableSession, staleEpochOf, WorkspaceSnapshots } from "../../dist/index.js"
import { diff, scan, DEFAULT_TRANSACTIONAL_LIMITS } from "../../dist/transactional.js"
import { sh } from "../../dist/spacesd.js"
import { backend, BACKEND } from "./backend.mjs"
import { execTools } from "./exec-tools.mjs"
import { implied, listing, nextTurn, steps } from "./plan.mjs"
import { createHash } from "node:crypto"
import { mkdtempSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const session = process.env.CUA_SESSION !== undefined
const attempt = Number(process.env.CUA_SESSION_ATTEMPT ?? 1)
/** The fault this driver injects into itself, if any (session mode). */
const selfFault = session ? JSON.parse(process.env.CRASH_SELF_FAULTS ?? "{}")[String(attempt)] : undefined
let selfSeen = 0
/** A driver reports the wall clock of every mark, for resume latency. */
const mark = (point) => {
  writeSync(1, session ? `MARK ${point} @${Date.now()}\n` : `MARK ${point}\n`)
  if (selfFault && point === selfFault.point && ++selfSeen === selfFault.occurrence) {
    if (selfFault.kind === "hold") {
      // The runner removes this driver's Space from outside: stand still until it does (or give up after `ms`).
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, selfFault.ms ?? 30000)
      return
    }
    setTimeout(() => {
      writeSync(1, `KILL ${point} @${Date.now()} ${selfFault.kind}\n`)
      process.kill(process.pid, selfFault.kind === "freeze" ? "SIGSTOP" : "SIGKILL")
    }, selfFault.delay)
  }
}
if (session) {
  // The coordinator keeps only a short tail of a failed job's stderr: put the diagnostics in the output.
  const say = (kind) => (error) => {
    writeSync(1, `${kind} ${String(error?.stack ?? error).replace(/\n/g, " | ").slice(0, 1500)}\n`)
    if (kind === "CRASH") process.exit(1)
  }
  process.on("uncaughtException", say("CRASH"))
  process.on("unhandledRejection", say("CRASH"))
}
if (session) writeSync(1, `EPOCH ${process.env.CUA_SESSION_EPOCH} ${attempt} @${Date.now()}\n`)
// With a durable Space attached by the coordinator: its lease fencing token, where it is and a digest of its token.
if (session && process.env.CUA_SESSION_SPACE_LEASE) {
  const digest = createHash("sha256").update(process.env.CUA_SESSION_SPACE_TOKEN ?? "").digest("hex").slice(0, 8)
  writeSync(1, `LEASE ${process.env.CUA_SESSION_SPACE_LEASE} ${process.env.CUA_SESSION_SPACE_ADDR} ${digest} @${Date.now()}\n`)
}
const seed = Number(process.env.CRASH_SEED)
const plan = steps(seed, Number(process.env.CRASH_STEPS))
const work = process.env.CRASH_WORK
const durableMode = process.env.CRASH_DURABLE_MODE ?? "checkpoint"
const workspaceName = () => process.env.CUA_SESSION_SPACE || process.env.CRASH_SPACE

/** The Space of a session driver: the durable Space at the contract's address. */
function sessionSpaces() {
  // The coordinator's durable Space (direct:host:port), or one the campaign runner made (CRASH_EXT_SPACE_*).
  const url = (process.env.CUA_SESSION_SPACE_ADDR || process.env.CRASH_EXT_SPACE_ADDR).replace(/^direct:/, "http://")
  const token = process.env.CUA_SESSION_SPACE_TOKEN || process.env.CRASH_EXT_SPACE_TOKEN
  let client
  return {
    exists: async () => true,
    ensure: async () => {},
    invalidate: () => {
      client = undefined
    },
    async spacesd(name) {
      client ??= (async () => {
        process.env.CUA_TEST = "1"
        // A job's home may not be writable: the SDK keeps its state in a directory of its own.
        process.env.CUA_HOME ??= mkdtempSync(join(tmpdir(), "cua-pi-driver-"))
        const { embedded } = await import("@trycua/cua")
        const sandbox = await embedded().sandboxes().connectUrl(url, token, name)
        return sandbox.spacesd(10_000)
      })()
      client.catch(() => {
        client = undefined
      })
      return client
    },
  }
}

// A session driver reaches the durable Space at the contract's address, except with the local stand-in.
const external = session && BACKEND !== "local"
const chosen = external ? undefined : await backend()
const docker = external ? sessionSpaces() : chosen.spaces
/** The Space provider with fault points around every command. */
const spaces = {
  exists: (name) => docker.exists(name),
  ensure: (name) => docker.ensure(name),
  invalidate: (name) => docker.invalidate?.(name),
  ...(docker.kind ? { kind: (name) => docker.kind(name) } : {}),
  async spacesd(name) {
    const inner = await docker.spacesd(name)
    return new Proxy(inner, {
      get(target, property) {
        if (property !== "spawn") {
          const value = Reflect.get(target, property, target)
          return typeof value === "function" ? value.bind(target) : value
        }
        return async (command) => {
          const traced = command.requestId !== undefined
          if (traced) mark("exec-spawn")
          const process = await target.spawn(command)
          if (!traced) return process
          mark("exec-running")
          return new Proxy(process, {
            get(p, key) {
              if (key !== "nextEvent") {
                const value = Reflect.get(p, key, p)
                return typeof value === "function" ? value.bind(p) : value
              }
              return async () => {
                const event = await p.nextEvent()
                if (event?.kind === 3) mark("exec-exit")
                return event
              }
            },
          })
        }
      },
    })
  },
}
// Workspace checkpoints are also kept on this machine (CRASH_STORE) and
// machine checkpoints live with the cua daemon, so a lost Space is rebuilt.
const workspace = external ? chosenSnapshots(spaces) : chosen.snapshots(spaces)
const snapshots = {
  async checkpoint(space, label) {
    mark("checkpoint-begin")
    const id = await workspace.checkpoint(space, label)
    mark("checkpoint-end")
    return id
  },
  async restore(space, id) {
    mark("restore-begin")
    await workspace.restore(space, id)
    mark("restore-end")
  },
  fork: (id, source, target) => workspace.fork(id, source, target),
}
// wholeMachine and friends are read off the provider by the durable layer.
if ("wholeMachine" in workspace) snapshots.wholeMachine = workspace.wholeMachine

function chosenSnapshots(via) {
  // A driver in a Space: workspace checkpoints stored in the Space itself.
  return new WorkspaceSnapshots({ spaces: via, root: work })
}

/** Builds the durable layer, the Harness over `storage`, and runs the plan to settlement. */
async function drive({ storage, epoch, fence, onStale, onHalt }) {
  const durable = durableSpaces({
    spaces,
    snapshots,
    cwd: work,
    spaceName: workspaceName,
    mode: durableMode,
    ...(epoch !== undefined ? { epoch, fence, onStale, onHalt } : {}),
    trace: (point) => mark(point),
    onRecovery: (event) => process.stdout.write(`RECOVERY ${JSON.stringify(event)}\n`),
  })
  // A fresh durable Space has no workspace yet; creating it again is harmless.
  if (session && !chosen) {
    await sh(await spaces.spacesd(workspaceName()), `mkdir -p ${work}`)
  }
  const faux = fauxProvider()
  const models = createModels()
  models.setProvider(faux.provider)
  const respond = (transcript) => {
    const turn = nextTurn(plan, transcript.messages)
    if (turn.done) return fauxAssistantMessage([fauxText("done")])
    return fauxAssistantMessage([fauxToolCall(turn.call.name, turn.call.arguments, { id: turn.call.id })], {
      stopReason: "toolUse",
    })
  }
  faux.setResponses(Array.from({ length: 2000 }, () => respond))

  const registry = createRegistry()
  // The local stand-in has no stdin for the file daemon: use exec-only tools there.
  registry.install(BACKEND === "local" ? execTools(work) : CodingTools)
  registry.install(durable.extension)

  const harness = await Harness.open(storage, { models, registry, env: durable.env }, context)
  harness.resume()
  const root = await harness.root(context, { agent: { model: { provider: "faux", modelId: "faux-1" }, cwd: work } })
  mark("open")
  let settled
  return {
    durable,
    async wait() {
      const submission = await root.submit({ type: "input", content: "run the plan", requestId: "run" }, context)
      settled = await submission.wait(context)
      return settled
    },
    /** After the run settled, before reporting done: a Space lost after the last call is rebuilt, then the image is checked. */
    async ensureSpace() {
      await durable.ensureSpace(harness, root.id, context)
      // Transactional mode: the log's image must equal the workspace.
      let image = ""
      try {
        const logged = await harness.snapshot(ImageDoc, root.id, context)
        if (logged?.files) {
          const sp = await spaces.spacesd(workspaceName())
          const drift = diff(logged.files, await scan(sp, work, DEFAULT_TRANSACTIONAL_LIMITS))
          image = drift.upsert.length === 0 && drift.remove.length === 0 ? " image=yes" : ` image=no ${JSON.stringify(drift)}`
        }
      } catch (error) {
        if (isStaleEpochError(error)) throw error
        image = ` image=error ${error?.message}`
      }
      await verify(root)
      process.stdout.write(`DONE ${settled.status}${image}\n`)
    },
    async close() {
      await harness.close(context).catch(() => {})
      await durable.close()
    },
  }
}

/** The workspace against what the committed log implies (a driver reports it; run.mjs checks it itself too). */
async function verify(root) {
  if (!session) return
  const messages = (await root.context(context)).messages
  const { files, applied, failed } = implied(messages)
  const expected = listing(files)
  let actual
  if (chosen?.listing) actual = await chosen.listing(workspaceName(), work)
  else {
    const result = await sh(await spaces.spacesd(workspaceName()), `cd ${work} && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum`)
    actual = result.stdout.trim()
  }
  const ok = expected === actual && applied.length === Number(process.env.CRASH_STEPS) && failed.length === 0
  writeSync(1, `VERIFY ${JSON.stringify({ ok, applied: applied.length, failed: failed.length, ...(failed.length > 0 ? { firstFailure: failed[0].text.slice(0, 200) } : {}), ...(ok ? {} : { expected, actual }) })}\n`)
}

function reportStale(error) {
  const stale = staleEpochOf(error) ?? error
  process.stdout.write(`STALE resource=${stale.resource} epoch=${stale.epoch} floor=${stale.floor} ${stale.message}\n`)
}

if (session) {
  let driven
  const code = await runDurableSession({
    log: (line) => writeSync(1, `LOG ${line.replace(/\n/g, " | ").slice(0, 1500)}\n`),
    get durable() {
      return driven?.durable
    },
    async start(ctx) {
      driven = await drive({ storage: ctx.log.storage, epoch: ctx.session.epoch, fence: ctx.fence, onStale: ctx.onStale, onHalt: ctx.onHalt })
      // A Space the coordinator did not attach (CRASH_EXT_SPACE_*): this driver raises its epoch itself.
      if (!ctx.session.space) await driven.durable.raiseEpoch(workspaceName())
      return {
        wait: () => driven.wait(),
        ensureSpace: () => driven.ensureSpace(),
        close: () => driven.close(),
      }
    },
  })
  process.exit(code)
}

const epoch = process.env.CRASH_EPOCH === undefined ? undefined : Number(process.env.CRASH_EPOCH)
let fenced
let storage
try {
  if (epoch !== undefined) {
    fenced = await openFencedSqlite(process.env.CRASH_DB, epoch)
    storage = fenced.storage
    // As runDurableSession does: the first refusal of the log stops the driver at once. Without
    // it Pi swallows the refused commit and the run neither settles nor fails.
    fenced.fence.onStale((error) => {
      reportStale(error)
      process.exit(75)
    })
  } else {
    storage = await openNodeSqliteStorage(process.env.CRASH_DB)
  }
  const driven = await drive({
    storage,
    epoch,
    fence: fenced?.fence,
    onStale: (error) => {
      reportStale(error)
      process.exit(75)
    },
    onHalt: (error) => {
      process.stdout.write(`HALT ${error.message}\n`)
      process.exit(error.permanent ? 78 : 1)
    },
  })
  if (epoch !== undefined) await driven.durable.raiseEpoch(workspaceName())
  await driven.wait()
  await driven.ensureSpace()
  await driven.close()
  process.exit(0)
} catch (error) {
  if (isStaleEpochError(error)) {
    reportStale(error)
    process.exit(75)
  }
  throw error
}
