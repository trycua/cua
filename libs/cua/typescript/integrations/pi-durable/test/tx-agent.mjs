// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// One driver process of the transactional-mode crash test (see
// crash-tx.test.mjs): a scripted model works through a plan in a local Space,
// printing `MARK <point>` at every step a crash can be injected after. The
// test SIGKILLs it at chosen marks and starts it again with the next epoch,
// as a coordinator would, until it prints DONE.
//
// Environment: TX_DIR, TX_CWD, TX_EPOCH, TX_STEPS, TX_MODE.

import { ImageDoc } from "../dist/index.js"
import { diff, scan, DEFAULT_TRANSACTIONAL_LIMITS } from "../dist/transactional.js"
import { StaleEpochError } from "../dist/index.js"
import { context, open } from "./fixture.mjs"
import { LocalSpacesd } from "./local-space.mjs"
import { crashPlan } from "./plans.mjs"

const mark = (point) => process.stdout.write(`MARK ${point}\n`)
const dir = process.env.TX_DIR
const cwd = process.env.TX_CWD

const inner = new LocalSpacesd(`${dir}/space`)
/** The Space's daemon with fault points around every keyed command. */
const spacesd = new Proxy(inner, {
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

const session = await open({
  dir,
  cwd,
  spacesd,
  epoch: Number(process.env.TX_EPOCH),
  mode: process.env.TX_MODE ?? "transactional",
  plan: crashPlan(cwd, Number(process.env.TX_STEPS ?? 8), Number(process.env.TX_SLEEP ?? 0.1)),
  trace: mark,
  onRecovery: (event) => process.stdout.write(`RECOVERY ${JSON.stringify(event)}\n`),
})
mark("open")
try {
  const settled = await session.run("run")
  const image = await session.harness.snapshot(ImageDoc, session.root.id, context)
  let same = "n/a"
  if (image?.files) {
    const drift = diff(image.files, await scan(spacesd, cwd, DEFAULT_TRANSACTIONAL_LIMITS))
    same = drift.upsert.length === 0 && drift.remove.length === 0 ? "yes" : `no ${JSON.stringify(drift)}`
  }
  const messages = await session.messages()
  const interrupted = messages.filter((m) => m.role === "toolResult" && m.isError).length
  process.stdout.write(`DONE ${settled.status} image=${same} errors=${interrupted}\n`)
  await session.close()
  process.exit(0)
} catch (error) {
  if (error instanceof StaleEpochError) {
    process.stdout.write(`STALE ${error.message}\n`)
    process.exit(75)
  }
  throw error
}
