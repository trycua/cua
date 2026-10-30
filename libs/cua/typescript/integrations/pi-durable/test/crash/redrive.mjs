// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// CRASH_MODE=redrive: the coordinator's re-driver is the system under test.
//
// The runner starts the lab coordinator (CRASH_LAB_BIN, a command that serves the
// coordinator's request surface as JSON lines on 127.0.0.1:<port>: the ignored test
// `lab_coordinator_live` of cua-spaces-cluster in serve mode, see lab/README.md), creates a
// durable Space and starts a session whose driver is agent.mjs (wrapped in
// runDurableSession, reading the launch contract) in a real Space, then injects faults
// per run, one flavor each (CRASH_REDRIVE_FLAVORS, default all in rotation):
//
//   kill         the driver SIGKILLs itself at a fault marker, on 1 to 3 attempts
//   container    the driver's whole Space is removed (`docker rm -f`) at a marker
//   coordinator  a driver kill, then the coordinator is SIGKILLed while the session is
//                in backoff and restarted on the same state
//   hang         the driver freezes itself (SIGSTOP); its heartbeat stops, the coordinator's
//                stall detection (spec stall_secs = CRASH_STALL_SECS, default 20) sees its
//                output stop growing, cancels it and resumes: history shows `stalled`
//   exhaust      `max_attempts` 2 and more kills than that: the session must end
//                `failed` (exhausted) with an error
//   lease        (not in the default rotation: CRASH_REDRIVE_FLAVORS=lease) the session is bound to a
//                durable Space the COORDINATOR placed on a real host (CUA_LAB_DIRECT_IP, see the lab
//                test): every drive takes the Space's lease and gets CUA_SESSION_SPACE_ADDR/_TOKEN/_LEASE.
//                Rotates by seed over kill (driver kills), space (driver killed, then the Space's container
//                removed: the durable loop places generation+1, restored from scratch), spacelive (the Space
//                removed while the driver is alive), coordinator (driver kill, coordinator SIGKILL in
//                backoff) and race (two sessions for one Space: exactly one holds the lease, the other waits).
//                Invariants on top of the others: epoch == max(previous + 1, lease token), lease tokens
//                strictly increase, a superseded lease token is refused, the driver follows the Space's new
//                address and token, and the workspace equals the log after a from-scratch Space.
//
// Per run it asserts: the session reaches `done` (`failed`/exhausted for exhaust); drives
// equal kills plus one (no spurious extra resume); epochs strictly increase; exactly one
// driver is live at any time (history alternates, and outputs of successive epochs do not
// overlap in time); the final driver verified workspace == log (every step once, none
// duplicated). It measures resume latency: ms from a kill to the next driver's first MARK.
//
// Environment: CRASH_LAB_BIN (required; command, run through `sh -c exec ...`),
// CRASH_LAB_ARGS (default `--ignored --nocapture`), CRASH_LAB_HOME (the lab's throwaway
// CUA_HOME; never ~/.cua), CRASH_DRIVER_IMAGE (image with node and the built package at
// CRASH_DRIVER_DIR, default /opt/pi-durable), CRASH_DURABLE_IMAGE (the durable Space's
// image; empty: the canonical Linux image), CRASH_WORK (workspace in the durable Space),
// CRASH_DRIVER_ENV (JSON object added to the driver's env), CRASH_BACKEND=local
// (self-test with fake-lab.mjs: drivers run on this machine against the stand-in Space).

import { spawn, execFileSync } from "node:child_process"
import { createConnection, createServer } from "node:net"
import { mkdtempSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import { join } from "node:path"
import { BACKEND } from "./backend.mjs"
import { DockerSpaces } from "./docker-spaces.mjs"
import { reportRun } from "./campaign.mjs"
import { describe, latencyStats, newSummary, pick, seeded } from "./common.mjs"
import { EVERY_CALL, FAULTS_BY_MODE } from "./faults.mjs"

const FLAVORS = (process.env.CRASH_REDRIVE_FLAVORS ?? "kill,container,coordinator,hang,exhaust").split(",")
const RUN_TIMEOUT_MS = Number(process.env.CRASH_REDRIVE_TIMEOUT_S ?? 900) * 1000
const STALL_S = Number(process.env.CRASH_STALL_SECS ?? 20)
// "coordinator": a durable Space placed by the coordinator (needs a node that hosts Spaces); "docker": the campaign
// makes a Docker Space on the lab's engine; "none": the stand-in lab (CRASH_BACKEND=local) has the Space on disk.
const spaceMode = process.env.CRASH_REDRIVE_SPACE ?? (BACKEND === "local" ? "none" : "docker")
const POLL_MS = 100
const LEASE_SUBS = (process.env.CRASH_LEASE_SUBS ?? "kill,space,spacelive,coordinator,race").split(",")
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** A loopback port nothing listens on right now, from the OS (a guessed one can be taken: outbound sockets and other tests use the same range). */
function freePort() {
  return new Promise((resolve, reject) => {
    const probe = createServer()
    probe.once("error", reject)
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address()
      probe.close(() => resolve(port))
    })
  })
}

/** The lab coordinator process and its JSON-lines control connection. */
class Lab {
  /** `port` pins the control port (CRASH_LAB_PORT); without it the lab takes a free one and moves on if it is taken before the coordinator binds. */
  constructor(port, home) {
    this.port = port
    this.pinned = port !== undefined
    this.home = home
    this.starts = 0
  }

  async start() {
    // The only way a start fails for the port is another process binding it between the probe and the coordinator's bind: pick again.
    for (let attempt = 1; ; attempt++) {
      if (!this.pinned && (this.port === undefined || attempt > 1)) this.port = await freePort()
      try {
        return await this.startOnce()
      } catch (error) {
        if (this.pinned || attempt >= 5 || !/EADDRINUSE/.test(String(error?.message))) throw error
      }
    }
  }

  async startOnce() {
    const bin = process.env.CRASH_LAB_BIN
    const args = process.env.CRASH_LAB_ARGS ?? "--ignored --nocapture"
    this.starts++
    this.stderr = []
    this.proc = spawn("sh", ["-c", `exec ${bin} ${args}`], {
      env: {
        ...process.env,
        CUA_HOME: this.home,
        CUA_LAB_COORD_LIVE: "1",
        CUA_LAB_SERVE: "1",
        CUA_LAB_CONTROL_PORT: String(this.port),
        ...(process.env.CRASH_DRIVER_IMAGE ? { CUA_JOB_SPACES_IMAGE: process.env.CRASH_DRIVER_IMAGE } : {}),
      },
      stdio: ["pipe", "ignore", "pipe"],
    })
    this.proc.stderr.on("data", (d) => this.stderr.push(d.toString("utf8")))
    this.exited = new Promise((resolve) => this.proc.on("exit", resolve))
    // "close" comes after the stderr stream is drained: an exit message that names EADDRINUSE must not be cut short.
    const closed = new Promise((resolve) => this.proc.on("close", resolve))
    // The coordinator says so on stderr once it has bound the port (the real lab and the stand-in both do). A connection that
    // succeeds before that line may be some other process's listener on a port taken after the probe, so it does not count.
    const announced = `lab control: 127.0.0.1:${this.port}`
    const deadline = Date.now() + Number(process.env.CRASH_LAB_START_S ?? 300) * 1000
    for (;;) {
      if (this.proc.exitCode !== null) {
        await closed
        throw new Error(`the lab coordinator exited ${this.proc.exitCode}: ${this.stderr.join("").slice(-1500)}`)
      }
      if (this.stderr.join("").includes(announced)) {
        try {
          await this.connect()
          return
        } catch {}
      }
      if (Date.now() > deadline) throw new Error(`the lab coordinator never listened on ${this.port}: ${this.stderr.join("").slice(-1500)}`)
      await sleep(100)
    }
  }

  connect() {
    return new Promise((resolve, reject) => {
      const socket = createConnection({ host: "127.0.0.1", port: this.port }, () => {
        this.socket = socket
        this.pending = new Map()
        this.nextId = 1
        let buffer = ""
        socket.on("data", (chunk) => {
          buffer += chunk.toString("utf8")
          let newline
          while ((newline = buffer.indexOf("\n")) !== -1) {
            const line = buffer.slice(0, newline)
            buffer = buffer.slice(newline + 1)
            try {
              const answer = JSON.parse(line)
              this.pending.get(answer.id)?.(answer)
              this.pending.delete(answer.id)
            } catch {}
          }
        })
        socket.on("close", () => {
          for (const resolveAnswer of this.pending.values()) resolveAnswer({ status: 0, body: { error: "control connection closed" } })
          this.pending.clear()
        })
        resolve()
      })
      socket.on("error", reject)
    })
  }

  /** One request; resolves with the body, throws on a non-200 status. */
  async call(method, params = {}) {
    const id = this.nextId++
    const answer = await new Promise((resolve) => {
      this.pending.set(id, resolve)
      this.socket.write(`${JSON.stringify({ id, method, params })}\n`)
    })
    if (answer.status !== 200) throw new Error(`${method}: ${answer.status} ${JSON.stringify(answer.body)}`)
    return answer.body
  }

  /** SIGKILL the coordinator process (the whole lab, with whatever it was doing). */
  async kill() {
    this.socket?.destroy()
    this.proc.kill("SIGKILL")
    await this.exited
  }

  async stop() {
    try {
      this.socket?.write('{"method":"quit"}\n')
    } catch {}
    const closed = await Promise.race([this.exited.then(() => true), sleep(5000).then(() => false)])
    if (!closed) await this.kill()
  }
}

/** Removes the container of the running driver: the whole Space / host runner goes. */
async function killDriverContainer(lab, name) {
  if (BACKEND === "local" || process.env.CRASH_KILL_CONTAINER === "lab") {
    await lab.call("x_kill_driver", { name }) // the fake lab: SIGKILL of the driver process tree
    return "fake"
  }
  const image = process.env.CRASH_DRIVER_IMAGE
  const rows = execFileSync("docker", ["ps", "--format", "{{.ID}} {{.Image}} {{.Names}}"], { encoding: "utf8" })
    .split("\n")
    .filter(Boolean)
    .map((l) => l.split(" "))
  const mine = rows.filter(([, img]) => (image ? img === image : true))
  if (mine.length === 0) throw new Error(`no driver container found among: ${rows.map((r) => r.join(" ")).join("; ")}`)
  // The newest container of the driver image is the current drive's.
  const [id] = mine[0]
  execFileSync("docker", ["rm", "-f", id], { stdio: "ignore" })
  return id
}

/** Container ids of the durable Space `name` (the coordinator names its copy on the host after it). */
function spaceContainers(name) {
  const driverImage = process.env.CRASH_DRIVER_IMAGE
  return execFileSync("docker", ["ps", "-a", "--format", "{{.ID}}\t{{.Names}}\t{{.Image}}"], { encoding: "utf8" })
    .split("\n")
    .filter(Boolean)
    .map((l) => l.split("\t"))
    .filter(([, names, image]) => names.includes(name) && image !== driverImage)
    .map(([id]) => id)
}

/** Removes the durable Space's container (the host lost it). Returns how many were removed. */
async function removeSpace(lab, name) {
  if (BACKEND === "local" || process.env.CRASH_KILL_CONTAINER === "lab") {
    await lab.call("x_remove_space", { name }) // the fake lab: the Space is lost and placed again
    return 1
  }
  const ids = spaceContainers(name)
  if (ids.length === 0) throw new Error(`no container of the durable Space ${name} found`)
  execFileSync("docker", ["rm", "-f", ...ids], { stdio: "ignore" })
  return ids.length
}

/** Waits until the coordinator has placed the durable Space; returns its view. */
async function durableRunning(lab, name, within = 180_000) {
  const deadline = Date.now() + within
  for (;;) {
    const view = await lab.call("durable_get", { name })
    if (view.state === "running" && view.space) return view
    if (Date.now() > deadline) throw new Error(`durable Space ${name} not placed after ${within / 1000}s: ${JSON.stringify({ state: view.state, error: view.error })}`)
    await sleep(500)
  }
}

function faultsFor(flavor, random, durableMode, sub) {
  // Only points every call of the chosen durable mode passes through: a fault that never fires proves nothing.
  const points = FAULTS_BY_MODE[durableMode].filter(([p]) => EVERY_CALL.includes(p))
  // The first drive may be hit at its second call; a resumed drive may have only a call or two left.
  const one = (kind, first = false) => {
    const [point, [lo, hi]] = pick(random, points)
    return { kind, point, occurrence: first ? 1 + Math.floor(random() * 2) : 1, delay: Math.round(lo + random() * (hi - lo)) }
  }
  switch (flavor) {
    case "kill": {
      const k = 1 + Math.floor(random() * 3)
      return Object.fromEntries(Array.from({ length: k }, (_, i) => [i + 1, one("kill", i === 0)]))
    }
    case "container":
      return { 1: one("container", true) }
    case "coordinator":
      return { 1: one("kill", true) }
    case "lease":
      // spacelive and race A: one driver kill (race) or none (the Space itself is attacked live).
      if (sub === "spacelive") return {}
      if (sub === "kill") {
        const k = 1 + Math.floor(random() * 3)
        return Object.fromEntries(Array.from({ length: k }, (_, i) => [i + 1, one("kill", i === 0)]))
      }
      return { 1: one("kill", true) }
    case "hang":
      return { 1: one("freeze") }
    case "exhaust":
      return { 1: one("kill", true), 2: one("kill"), 3: one("kill") }
    default:
      throw new Error(`unknown flavor ${flavor}`)
  }
}

/** The lease invariants of one session's drives: tokens grow, and each epoch is max(previous + 1, lease token). */
function checkLeases(drives, leases, problems, summary) {
  const byEpoch = new Map(leases.map((l) => [l.epoch, l]))
  let previous
  for (const d of drives) {
    const l = byEpoch.get(d.epoch)
    if (!l) continue // a drive that died before printing anything
    if (!(l.lease <= d.epoch)) {
      problems.push(`epoch ${d.epoch} is below its lease token ${l.lease}`)
      summary.leaseViolations++
    }
    if (previous) {
      if (!(l.lease > previous.lease)) {
        problems.push(`lease tokens do not grow: ${previous.lease} then ${l.lease}`)
        summary.leaseViolations++
      }
      if (d.epoch !== Math.max(previous.epoch + 1, l.lease)) {
        problems.push(`epoch ${d.epoch} is not max(${previous.epoch} + 1, lease ${l.lease})`)
        summary.leaseViolations++
      }
    } else if (d.epoch !== Math.max(1, l.lease)) {
      problems.push(`the first epoch ${d.epoch} is not max(1, lease ${l.lease})`)
      summary.leaseViolations++
    }
    previous = { epoch: d.epoch, lease: l.lease }
  }
  if (leases.length === 0) {
    problems.push("no driver reported a lease token (CUA_SESSION_SPACE_LEASE): the Space was not attached")
    summary.leaseViolations++
  }
}

/**
 * Two sessions bound to one durable Space. The first starts (with one driver kill); the second starts while it
 * holds the lease and must wait, clearly, until the first has ended: exactly one holder at a time. Each works in a
 * directory of its own on the shared Space. The second's epoch continues from the lease counter
 * (max(counter + 1, lease token) is the lease token), and the first's token is refused once the second holds it.
 */
async function raceCase({ lab, name, seed, steps, record, driverEnv, selfFaults, summary, out, driverDir, maxAttempts, flavor, problems }) {
  const sessions = { a: `${name}-a`, b: `${name}-b` }
  const state = {}
  for (const k of Object.keys(sessions)) state[k] = { events: [], leases: [], raw: [], readers: undefined, view: undefined }
  let ok = true
  try {
    const placed = await (async () => {
      await lab.call("durable_create", { name, wait: false, spec: { image: process.env.CRASH_DURABLE_IMAGE ?? "", kind: "container" } })
      return durableRunning(lab, name)
    })()
    record.durable = { space: placed.space, node: placed.node, generation: placed.generation }
    if (BACKEND === "local") for (const dir of [driverEnv.CRASH_WORK, `${driverEnv.CRASH_WORK}-b`]) (await import("node:fs")).mkdirSync(dir, { recursive: true })
    for (const k of Object.keys(sessions)) {
      const env = k === "a" ? { ...driverEnv, CRASH_SELF_FAULTS: JSON.stringify(selfFaults) } : { ...driverEnv, CRASH_SEED: String(seed + 7), CRASH_WORK: `${driverEnv.CRASH_WORK}-b`, CRASH_SELF_FAULTS: "{}" }
      state[k].start = async () => {
        await lab.call("session_start", {
          name: sessions[k],
          max_attempts: maxAttempts,
          spec: { argv: JSON.parse(process.env.CRASH_DRIVER_ARGV ?? JSON.stringify(["node", `${driverDir}/test/crash/agent.mjs`])), env, image: process.env.CRASH_DRIVER_IMAGE ?? "", kind: "container", space: name, label: "crash-campaign" },
        })
      }
      state[k].read = outputReader(lab, sessions[k], (line, epoch) => {
        const m = /^(EPOCH|LEASE|VERIFY|KILL|LOG)\b\s*(.*)$/.exec(line)
        if (!m) return
        const [, kind, rest] = m
        state[k].raw.push(line)
        const lastEpoch = [...state[k].events].reverse().find((x) => x.kind === "EPOCH")?.epoch
        state[k].events.push({ kind, rest, epoch })
        if (kind === "EPOCH") state[k].events.at(-1).epoch = Number(rest.split(" ")[0])
        if (kind === "VERIFY") state[k].verify = JSON.parse(rest)
        if (kind === "KILL") state[k].kills = (state[k].kills ?? 0) + 1
        if (kind === "LEASE") {
          const [n, addr, digest] = rest.split(" ")
          const entry = { epoch: lastEpoch ?? state[k].events.findLast((x) => x.kind === "EPOCH")?.epoch, lease: Number(n), addr, digest }
          const prev = state[k].leases.at(-1)
          state[k].leases.push(entry)
          if (k === "b" && !state.b.firstLease) state.b.firstLease = entry
          if (prev && prev.lease !== entry.lease) {
            // A superseded token of the same session.
            lab.call("durable_connection", { name, token: prev.lease }).then(
              () => {
                problems.push(`superseded token ${prev.lease} still accepted`)
                summary.leaseViolations++
              },
              (e) => /no longer held/.test(String(e.message)) && summary.staleTokensRefused++,
            )
          }
        }
      })
    }
    await state.a.start()
    // B starts once A is running (holds the lease).
    for (;;) {
      const v = await lab.call("session_get", { name: sessions.a })
      if (v.state === "running" || ["done", "failed", "stopped"].includes(v.state)) break
      await sleep(200)
    }
    await state.b.start()
    let sawWait = false
    let bTokenProbe = false
    const deadline = Date.now() + RUN_TIMEOUT_MS * 2
    for (;;) {
      for (const k of Object.keys(sessions)) {
        state[k].view = await lab.call("session_get", { name: sessions[k] })
        await state[k].read().catch(() => {})
      }
      const [va, vb] = [state.a.view, state.b.view]
      if (vb.state === "pending" && /held by/.test(vb.error ?? "") && va.state !== "done") sawWait = true
      // While B holds the lease, A's last token must be refused for every use.
      if (!bTokenProbe && state.b.firstLease && state.a.leases.length > 0) {
        bTokenProbe = true
        const stale = state.a.leases.at(-1).lease
        for (const method of ["durable_renew", "durable_connection", "durable_release"]) {
          try {
            await lab.call(method, { name, token: stale })
            problems.push(`${method} with session A's old token ${stale} was accepted while B holds the Space`)
            summary.leaseViolations++
          } catch (e) {
            if (/no longer held/.test(String(e.message))) summary.staleTokensRefused++
            else problems.push(`${method} with A's old token failed for another reason: ${String(e.message).slice(0, 160)}`)
          }
        }
      }
      if (["done", "failed", "stopped"].includes(va.state) && ["done", "failed", "stopped"].includes(vb.state)) {
        for (const k of Object.keys(sessions)) await state[k].read().catch(() => {})
        break
      }
      if (Date.now() > deadline) throw new Error(`race not finished: A ${va.state}/${va.attempts} B ${vb.state}/${vb.attempts} ${vb.error ?? ""}`)
      await sleep(POLL_MS)
    }
    if (sawWait) summary.raceWaits++
    else problems.push("B never showed a clear wait reason (`held by ...`) while A held the Space")
    const [va, vb] = [state.a.view, state.b.view]
    for (const k of ["a", "b"]) {
      const v = state[k].view
      if (v.state !== "done") problems.push(`session ${k.toUpperCase()} ended ${v.state}/${v.reason ?? ""}: ${v.error}`)
      if (!state[k].verify) problems.push(`session ${k.toUpperCase()}: no VERIFY line`)
      else if (!state[k].verify.ok) problems.push(`session ${k.toUpperCase()}: workspace differs from the log: ${JSON.stringify(state[k].verify).slice(0, 400)}`)
      const drives = (v.history ?? []).filter((h) => h.event === "started")
      if (v.attempts !== (state[k].kills ?? 0) + 1) problems.push(`session ${k.toUpperCase()}: ${v.attempts} drives for ${state[k].kills ?? 0} kills`)
      checkLeases(drives, state[k].leases, problems, summary)
    }
    // Exactly one holder: B's first drive started only after A's last ended.
    const aEnd = Math.max(...(va.history ?? []).filter((h) => ["done", "crashed", "fenced", "permanent", "exhausted", "stopped", "stalled"].includes(h.event)).map((h) => h.at_ms))
    const bStart = Math.min(...(vb.history ?? []).filter((h) => h.event === "started").map((h) => h.at_ms))
    if (!(bStart >= aEnd)) {
      problems.push(`B started (${bStart}) before A ended (${aEnd}): two holders of one Space`)
      summary.overlapViolations++
    }
    // B continues the lease counter: its token is above every token A had, and its epoch is that token.
    const aLast = state.a.leases.at(-1)
    const bFirst = state.b.leases[0]
    if (aLast && bFirst) {
      if (!(bFirst.lease > aLast.lease)) {
        problems.push(`B's lease ${bFirst.lease} is not above A's last ${aLast.lease}`)
        summary.leaseViolations++
      }
      if (bFirst.epoch !== bFirst.lease) {
        problems.push(`B's first epoch ${bFirst.epoch} is not its lease token ${bFirst.lease} (counter + 1 is 1)`)
        summary.leaseViolations++
      }
    }
    record.race = { aDrives: va.attempts, bDrives: vb.attempts, aLeases: state.a.leases.map((l) => l.lease), bLeases: state.b.leases.map((l) => l.lease), aEnd, bStart, waitedMs: bStart - aEnd }
    record.kills = state.a.kills ?? 0
    record.attempts = va.attempts + vb.attempts
    summary.kills += record.kills
    summary.attempts += record.attempts
    record.verify = state.b.verify
    if (record.kills === 0) problems.push("no fault landed in session A")
  } catch (error) {
    ok = false
    record.error = describe(error)
  }
  if (problems.length > 0) {
    ok = false
    record.error = `${record.error ? `${record.error}; ` : ""}${problems.join("; ")}`
  }
  record.ok = ok
  summary.flavors[flavor] = (summary.flavors[flavor] ?? 0) + 1
  if (!ok) record.log = [lab.stderr.join("").slice(-2000), [...state.a.raw.slice(-20), ...state.b.raw.slice(-20)].join("\n")]
  reportRun(summary, record, out)
  // CRASH_KEEP=1 leaves a failed run's sessions and Space for inspection (docker exec, session_get).
  if (!ok && process.env.CRASH_KEEP) return
  for (const k of Object.keys(sessions)) await lab.call("session_delete", { name: sessions[k] }).catch(() => {})
  await lab.call("durable_delete", { name }).catch(() => {})
}

/** Reads a session's new output and feeds parsed events to `onEvent`. */
function outputReader(lab, name, onEvent) {
  let job
  let from = 0
  let buffer = ""
  return async () => {
    const o = await lab.call("session_output", { name, from })
    if (o.job !== job) {
      job = o.job
      from = 0
      buffer = ""
      if (o.next !== 0 && o.from !== 0) return
    }
    if (o.text) {
      buffer += o.text
      from = o.next ?? from + Buffer.byteLength(o.text)
      let newline
      while ((newline = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, newline)
        buffer = buffer.slice(newline + 1)
        onEvent(line, o.epoch)
      }
    }
  }
}

export async function campaign({ runs, steps, out, seedBase }) {
  if (!process.env.CRASH_LAB_BIN) throw new Error("CRASH_MODE=redrive needs CRASH_LAB_BIN (the lab coordinator command, see test/crash/redrive.mjs)")
  const summary = newSummary({
    mode: "redrive",
    backend: BACKEND,
    flavors: {},
    attempts: 0,
    coordinatorRestarts: 0,
    containersKilled: 0,
    hangs: 0,
    exhausted: 0,
    spuriousResumes: 0,
    epochViolations: 0,
    overlapViolations: 0,
    // flavor "lease"
    leaseSubs: {},
    spaceRemovals: 0,
    wastedDrives: 0,
    leaseViolations: 0,
    staleTokensRefused: 0,
    currentTokensOk: 0,
    spaceReplaced: 0,
    raceWaits: 0,
  })
  const home = process.env.CRASH_LAB_HOME || mkdtempSync(join(tmpdir(), "pi-crash-lab-"))
  if (home === join(homedir(), ".cua")) throw new Error("CRASH_LAB_HOME must be a throwaway directory, never ~/.cua")
  const lab = new Lab(process.env.CRASH_LAB_PORT ? Number(process.env.CRASH_LAB_PORT) : undefined, home)
  await lab.start()
  const latencies = []
  const startLatencies = []
  try {
    for (let run = 0; run < runs; run++) {
      const seed = seedBase + run
      const random = seeded(seed)
      const flavor = FLAVORS[seed % FLAVORS.length]
      const lease = flavor === "lease"
      const checkpointLimit = flavor === "lease" && (process.env.CRASH_DURABLE_MODE ?? "transactional") === "checkpoint" && !process.env.CRASH_CHECKPOINT_STORE
      const sub = lease ? LEASE_SUBS[seed % LEASE_SUBS.length] : undefined
      if (lease) summary.leaseSubs[sub] = (summary.leaseSubs[sub] ?? 0) + 1
      const useDurable = spaceMode === "coordinator" || lease
      // A name no earlier run used: the driver's journal (its state directory) is keyed by the session name.
      const name = `crash-${seed}-${Date.now().toString(36)}`
      const durableMode = process.env.CRASH_DURABLE_MODE ?? "transactional"
      const faults = faultsFor(flavor, random, durableMode, sub)
      // A drive against a Space that is gone but not yet replaced fails at once and counts: more room.
      const maxAttempts = flavor === "exhaust" ? 2 : lease ? 10 : 6
      const selfFaults = Object.fromEntries(
        Object.entries(faults).map(([a, f]) => [a, f.kind === "container" ? { ...f, kind: "hold", ms: 45000 } : f]),
      )
      const runnerFaults = Object.fromEntries(Object.entries(faults).filter(([, f]) => f.kind === "container"))
      if (sub === "spacelive") {
        // The Space goes while the first driver is alive and working in it.
        const [point, [lo, hi]] = pick(random, FAULTS_BY_MODE[durableMode].filter(([p]) => EVERY_CALL.includes(p)))
        runnerFaults[1] = { kind: "spacelive", point, occurrence: 1 + Math.floor(random() * 2), delay: Math.round(lo + random() * (hi - lo)) }
      }
      const leases = []
      const haltedDrives = []
      const probes = []
      // A superseded lease token must be refused (the coordinator's fence); the current one works.
      const probeTokens = (stale, current) => {
        probes.push(
          (async () => {
            try {
              await lab.call("durable_connection", { name, token: stale })
              problems.push(`the superseded lease token ${stale} was still accepted after ${current} was issued`)
              summary.leaseViolations++
            } catch (error) {
              if (/no longer held/.test(String(error.message))) summary.staleTokensRefused++
            }
            try {
              await lab.call("durable_connection", { name, token: current })
              summary.currentTokensOk++
            } catch {
              // The drive may have ended and released it already.
            }
          })(),
        )
      }
      const record = { run, seed, mode: "redrive", flavor, ...(sub ? { sub } : {}), steps, faults: Object.values(faults), recoveries: [], log: [], startedAt: Date.now() }
      const events = [] // { epoch, attempt?, kind, point?, at }
      const raw = [] // the driver's output lines, for diagnosing a failed run
      const killsSeen = []
      const driverDir = process.env.CRASH_DRIVER_DIR ?? "/opt/pi-durable"
      // With the local stand-in each run has its own workspace; in a real Space it is the same path of a fresh Space.
      const work = BACKEND === "local" ? join(home, `work-${seed}`) : (process.env.CRASH_WORK ?? "/home/cua/work")
      const driverEnv = {
        CRASH_SEED: String(seed),
        CRASH_STEPS: String(steps),
        CRASH_WORK: work,
        CRASH_DURABLE_MODE: durableMode,
        CRASH_SELF_FAULTS: JSON.stringify(selfFaults),
        ...(BACKEND === "local" ? { CRASH_BACKEND: "local", CRASH_LOCAL_DIR: join(home, `space-${seed}`), CRASH_STORE: join(home, `store-${seed}`) } : {}),
        ...JSON.parse(process.env.CRASH_DRIVER_ENV ?? "{}"),
      }
      let ok = true
      let docker
      const problems = []
      if (sub === "race") {
        await raceCase({ lab, name, seed, steps, record, driverEnv, selfFaults, summary, out, driverDir, maxAttempts, flavor, problems })
        continue
      }
      try {
        if (BACKEND === "local") (await import("node:fs")).mkdirSync(driverEnv.CRASH_WORK, { recursive: true })
        if (useDurable) {
          // The coordinator places the Space on a host that provides Spaces directly (the lab needs CUA_LAB_DIRECT_IP).
          await lab.call("durable_create", { name, wait: false, spec: { image: process.env.CRASH_DURABLE_IMAGE ?? "", kind: "container" } })
          const placed = await durableRunning(lab, name)
          record.durable = { space: placed.space, node: placed.node, generation: placed.generation }
        } else if (spaceMode === "docker") {
          // The lab node cannot host durable Spaces (no direct address), so the campaign makes the
          // Space itself, on the same engine, and hands the driver its address (the driver raises the epoch).
          docker = new DockerSpaces({ token: "redrive-token" })
          await docker.ensure(name)
          driverEnv.CRASH_EXT_SPACE_ADDR = docker.address(name)
          driverEnv.CRASH_EXT_SPACE_TOKEN = "redrive-token"
          driverEnv.CRASH_SPACE = name
        }
        await lab.call("session_start", {
          name,
          max_attempts: maxAttempts,
          spec: {
            argv: JSON.parse(process.env.CRASH_DRIVER_ARGV ?? JSON.stringify(["node", `${driverDir}/test/crash/agent.mjs`])),
            env: driverEnv,
            image: process.env.CRASH_DRIVER_IMAGE ?? "",
            kind: "container",
            ...(useDurable ? { space: name } : {}),
            label: "crash-campaign",
            ...(flavor === "hang" ? { stall_secs: STALL_S } : {}),
          },
        })
        // Poll the session and its output; inject the faults the driver cannot inject itself.
        const marks = {}
        const pendingRunnerKills = new Set()
        let coordinatorKilled = false
        const read = outputReader(lab, name, (line, epoch) => {
          raw.push(line)
          if (raw.length > 80) raw.shift()
          const m = /^(EPOCH|LEASE|MARK|KILL|VERIFY|DONE|RECOVERY|LOG|CRASH|STALE|HALT)\b\s*(.*)$/.exec(line)
          if (!m) return
          const [, kind, rest] = m
          const at = Number(/@(\d+)/.exec(rest)?.[1] ?? Date.now())
          const ev = { kind, at, epoch, rest }
          if (kind === "EPOCH") {
            const [e, a] = rest.split(" ")
            ev.epoch = Number(e)
            ev.attempt = Number(a)
          }
          events.push(ev)
          if (kind === "KILL") {
            killsSeen.push({ at, rest })
            // The host lost the Space as well: remove its container shortly after the driver died.
            if (sub === "space") {
              setTimeout(async () => {
                try {
                  summary.spaceRemovals += (await removeSpace(lab, name)) > 0 ? 1 : 0
                  record.spaceRemovals = (record.spaceRemovals ?? 0) + 1
                } catch (error) {
                  problems.push(`could not remove the durable Space: ${error.message}`)
                }
              }, 300)
            }
          }
          if (kind === "LEASE") {
            const [n, addr, digest] = rest.split(" ")
            const entry = { epoch: [...events].reverse().find((x) => x.kind === "EPOCH")?.epoch, lease: Number(n), addr, digest }
            const previous = leases.at(-1)
            leases.push(entry)
            if (previous && previous.lease !== entry.lease) probeTokens(previous.lease, entry.lease)
          }
          if (kind === "LOG" && /halted:/.test(rest)) haltedDrives.push(rest.slice(0, 160))
          if (kind === "VERIFY") record.verify = JSON.parse(rest)
          if (kind === "RECOVERY") {
            try {
              record.recoveries.push(JSON.parse(rest))
            } catch {}
          }
          if (kind === "MARK") {
            const point = rest.split(" ")[0]
            const attempt = [...events].reverse().find((x) => x.kind === "EPOCH")?.attempt
            const f = runnerFaults[attempt]
            const key = `${attempt}:${point}`
            marks[key] = (marks[key] ?? 0) + 1
            if (f && f.point === point && marks[key] === f.occurrence && !pendingRunnerKills.has(attempt)) {
              pendingRunnerKills.add(attempt)
              setTimeout(async () => {
                try {
                  if (f.kind === "spacelive") {
                    summary.spaceRemovals += (await removeSpace(lab, name)) > 0 ? 1 : 0
                    record.spaceRemovals = (record.spaceRemovals ?? 0) + 1
                    return
                  }
                  await killDriverContainer(lab, name)
                  killsSeen.push({ at: Date.now(), rest: `container ${point}` })
                  summary.containersKilled++
                } catch (error) {
                  problems.push(`could not kill the driver's container: ${error.message}`)
                }
              }, f.delay)
            }
          }
        })
        let view
        const deadline = Date.now() + RUN_TIMEOUT_MS
        for (;;) {
          view = await lab.call("session_get", { name })
          await read().catch(() => {})
          if (["done", "failed", "stopped"].includes(view.state)) {
            await read().catch(() => {})
            break
          }
          if ((flavor === "coordinator" || sub === "coordinator") && view.state === "backoff" && !coordinatorKilled) {
            coordinatorKilled = true
            await lab.kill()
            summary.coordinatorRestarts++
            await sleep(300)
            await lab.start()
            record.coordinatorRestarted = true
            continue
          }
          if (Date.now() > deadline) throw new Error(`session ${name} not finished after ${RUN_TIMEOUT_MS / 1000}s: ${JSON.stringify({ state: view.state, attempts: view.attempts, error: view.error })}`)
          await sleep(POLL_MS)
        }

        // The invariants.
        const history = view.history ?? []
        const drives = history.filter((h) => h.event === "started")
        const ended = history.filter((h) => ["crashed", "stalled", "fenced", "done", "permanent", "exhausted", "stopped"].includes(h.event))
        // A driver killed within a poll of its start leaves its KILL line unread (the output of an ended job is
        // only reachable while it is the newest one): the history's exit 137 (SIGKILL) counts those too.
        const selfKills = lease ? history.filter((h) => h.event === "crashed" && h.exit_code === 137).length : 0
        const kills = Math.max(killsSeen.length, selfKills)
        record.kills = kills
        record.attempts = view.attempts
        record.view = { state: view.state, reason: view.reason, epoch: view.epoch, attempts: view.attempts, error: view.error }
        if (lease) record.history = history.map((h) => ({ event: h.event, epoch: h.epoch, exit: h.exit_code, at: h.at_ms, detail: String(h.detail ?? "").slice(0, 160) }))
        summary.attempts += view.attempts
        summary.kills += kills
        for (const k of killsSeen) {
          const point = k.rest.split(" ")[k.rest.startsWith("container") ? 1 : 0]
          summary.faults[point] = (summary.faults[point] ?? 0) + 1
        }
        if (flavor === "hang") {
          const freezes = killsSeen.filter((k) => k.rest.includes("freeze")).length
          summary.hangs += freezes
          const stalled = (view.history ?? []).filter((h) => h.event === "stalled").length
          summary.stalls = (summary.stalls ?? 0) + stalled
          if (stalled !== freezes) problems.push(`hang: ${freezes} frozen drivers but ${stalled} stalled events (a freeze must be ended by stall detection, not a timeout)`)
        }
        if (flavor === "exhaust") {
          summary.exhausted++
          if (view.state !== "failed" || view.reason !== "exhausted") problems.push(`exhaust: the session ended ${view.state}/${view.reason}, not failed/exhausted`)
          if (!view.error) problems.push("exhaust: the failed session has no error text")
          if (view.attempts !== maxAttempts) problems.push(`exhaust: ${view.attempts} drives, expected the cap ${maxAttempts}`)
          if (kills < maxAttempts) problems.push(`exhaust: only ${kills} kills landed, fewer than the cap ${maxAttempts}`)
        } else if (checkpointLimit && (record.spaceRemovals ?? 0) > 0) {
          // Checkpoint mode with checkpoints kept inside the Space (no CheckpointStore reachable from every driver):
          // a replaced Space has nothing to restore from. The product limit, confirmed: the session must stop for good
          // at once, with the reason, instead of burning every attempt.
          summary.checkpointLimit = (summary.checkpointLimit ?? 0) + 1
          if (view.state !== "failed" || view.reason !== "permanent") problems.push(`limit: expected failed/permanent, got ${view.state}/${view.reason}: ${view.error}`)
          else if (!/CheckpointStore/.test(view.error ?? "")) problems.push(`limit: the failure does not name the CheckpointStore: ${view.error}`)
          else if (view.attempts > kills + 2) problems.push(`limit: ${view.attempts} drives before stopping`)
          record.limit = "checkpoints inside the lost Space"
          record.verifyOptional = true
        } else {
          if (view.state !== "done") problems.push(`the session ended ${view.state}${view.reason ? `/${view.reason}` : ""}: ${view.error}`)
          // A drive started against a Space that is gone (not yet placed again) halts at the fence and counts;
          // with the Space removed live (no driver kill) the driver itself fails once.
          const wasted = sub === "space" ? haltedDrives.length : 0
          const expectedDrives = kills + 1 + wasted
          // The Space removed under a live driver costs that driver (it halts, exit 1) and nothing else: one drive more.
          const expected = sub === "spacelive" ? 1 + (record.spaceRemovals ?? 0) : expectedDrives
          if (view.attempts !== expected) {
            problems.push(`${view.attempts} drives for ${kills} kills, ${record.spaceRemovals ?? 0} Space removals and ${wasted} halted at a missing Space (expected ${expected})`)
            summary.spuriousResumes++
          }
          summary.wastedDrives += wasted
          record.wastedDrives = wasted
          if (!record.verify) problems.push("the final driver printed no VERIFY line")
          else if (!record.verify.ok) problems.push(`workspace differs from the log: ${JSON.stringify(record.verify).slice(0, 600)}`)
        }
        if (drives.length !== view.attempts) problems.push(`history has ${drives.length} started events for ${view.attempts} drives`)
        // Epochs strictly increase.
        for (let i = 1; i < drives.length; i++) {
          if (!(drives[i].epoch > drives[i - 1].epoch)) {
            problems.push(`epochs do not increase: ${drives.map((d) => d.epoch).join(",")}`)
            summary.epochViolations++
            break
          }
        }
        // Exactly one live driver: each drive ended before the next one started.
        for (let i = 1; i < drives.length; i++) {
          const prev = ended.find((e) => e.epoch === drives[i - 1].epoch)
          if (!prev || prev.at_ms > drives[i].at_ms) {
            problems.push(`drive ${i + 1} started before drive ${i} ended`)
            summary.overlapViolations++
            break
          }
        }
        const byEpoch = new Map()
        for (const e of events.filter((x) => x.kind === "MARK" || x.kind === "KILL")) {
          const epoch = (() => {
            // attribute to the latest EPOCH line seen before it
            const idx = events.indexOf(e)
            for (let j = idx; j >= 0; j--) if (events[j].kind === "EPOCH") return events[j].epoch
          })()
          const w = byEpoch.get(epoch) ?? { first: Infinity, last: -Infinity }
          w.first = Math.min(w.first, e.at)
          w.last = Math.max(w.last, e.at)
          byEpoch.set(epoch, w)
        }
        const windows = [...byEpoch.entries()].sort((a, b) => a[0] - b[0])
        for (let i = 1; i < windows.length; i++) {
          if (windows[i][1].first < windows[i - 1][1].last) {
            problems.push(`outputs of epochs ${windows[i - 1][0]} and ${windows[i][0]} overlap in time`)
            summary.overlapViolations++
            break
          }
        }
        // Resume latency: kill -> the next driver's first MARK (and its EPOCH line).
        const epochLines = events.filter((e) => e.kind === "EPOCH")
        const mine = []
        for (const k of killsSeen) {
          const next = epochLines.find((e) => e.at > k.at)
          if (!next) continue
          const firstMark = events.find((e) => e.kind === "MARK" && e.at >= next.at)
          if (firstMark) {
            latencies.push(firstMark.at - k.at)
            mine.push(firstMark.at - k.at)
          }
          startLatencies.push(next.at - k.at)
        }
        record.resumeMs = mine
        if (lease) {
          await Promise.all(probes)
          checkLeases(drives, leases, problems, summary)
          record.leases = leases
          record.haltedDrives = haltedDrives
          const durable = await lab.call("durable_get", { name })
          record.durable = { ...record.durable, finalGeneration: durable.generation, restored: durable.restored, space: durable.space }
          if ((sub === "space" || sub === "spacelive") && (record.spaceRemovals ?? 0) > 0) {
            if (durable.generation < 2 || durable.restored !== "from-scratch") problems.push(`the Space was removed but the coordinator shows generation ${durable.generation} restored=${JSON.stringify(durable.restored)}`)
            else summary.spaceReplaced++
            const firstAddr = leases[0]?.addr
            const lastLease = leases.at(-1)
            if (lastLease && firstAddr && lastLease.addr === firstAddr && lastLease.digest === leases[0].digest) problems.push("the Space was replaced but the last driver got the old address and token")
          }
        }
        if (kills === 0 && !(record.spaceRemovals > 0)) problems.push("no fault landed (the session finished without being attacked)")
      } catch (error) {
        ok = false
        record.error = describe(error)
      }
      if (problems.length > 0) {
        ok = false
        record.error = `${record.error ? `${record.error}; ` : ""}${problems.join("; ")}`
      }
      record.ok = ok
      summary.flavors[flavor] = (summary.flavors[flavor] ?? 0) + 1
      if (!ok) record.log = [lab.stderr.join("").slice(-2000), raw.slice(-40).join("\n")]
      reportRun(summary, record, out)
      await lab.call("session_delete", { name }).catch(() => {})
      if (useDurable) await lab.call("durable_delete", { name }).catch(() => {})
      await docker?.remove(name)
    }
  } finally {
    await lab.stop()
  }
  summary.resumeLatencyMs = latencyStats(latencies)
  summary.restartLatencyMs = latencyStats(startLatencies)
  return summary
}
