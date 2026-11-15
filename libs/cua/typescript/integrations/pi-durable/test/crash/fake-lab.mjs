// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A stand-in for the lab coordinator, ONLY to test redrive.mjs on a machine without
// containers (`CRASH_BACKEND=local CRASH_LAB_BIN="node test/crash/fake-lab.mjs" CRASH_LAB_ARGS=""`).
// It speaks the lab's JSON-lines protocol (session_start/get/output/stop/delete,
// durable_create/delete, quit, and x_kill_driver for the container fault) and runs each
// drive of a session as a local process with the launch contract of the real re-driver
// (CUA_SESSION*, exit 0 done, 78 permanent, 75 fenced, anything else crashed and resumed
// after a backoff, up to max_attempts), state kept in $CUA_HOME/fake-lab.json so a killed
// stand-in restarts where it was. It is not the product: the Rust sessions tests and the
// real lab runs are. CRASH_FAKE_BACKOFF_MS shortens the backoff (default 400, doubling).

import { spawn } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { createServer } from "node:net"
import { join } from "node:path"

const home = process.env.CUA_HOME
const port = Number(process.env.CUA_LAB_CONTROL_PORT)
if (!home || !port) {
  console.error("fake lab: set CUA_HOME and CUA_LAB_CONTROL_PORT")
  process.exit(2)
}
mkdirSync(home, { recursive: true })
const statePath = join(home, "fake-lab.json")
const BACKOFF = Number(process.env.CRASH_FAKE_BACKOFF_MS ?? 400)
const sessions = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : {}
const drivers = new Map() // name -> { child, out }
// Durable Spaces and their leases (as the coordinator's durable table: a fencing counter that only grows, one holder).
const durables = existsSync(`${statePath}.durable`) ? JSON.parse(readFileSync(`${statePath}.durable`, "utf8")) : {}
const saveDurables = () => writeFileSync(`${statePath}.durable`, JSON.stringify(durables))
const durableView = (d) => ({ name: d.name, state: "running", node: "fake", generation: d.generation, restored: d.generation > 1 ? "from-scratch" : "", space: `direct:127.0.0.1:${40000 + d.generation}`, token: `tok-${d.generation}`, holder: d.holder || null })
const leased = (d) => d.holder && d.expires > Date.now()
function acquire(name, holder) {
  const d = durables[name]
  if (!d) throw new Error(`404 no durable Space ${name}`)
  if (leased(d) && d.holder !== holder) throw new Error(`403 ${name} is held by ${d.holder} for ${Math.round((d.expires - Date.now()) / 1000)} s more`)
  d.fence += 1
  d.lease = d.fence
  d.holder = holder
  d.expires = Date.now() + 7 * 24 * 3600 * 1000
  saveDurables()
  return { ...durableView(d), lease_token: d.lease, id: durableView(d).space }
}
function withLease(name, token) {
  const d = durables[name]
  if (!d) throw new Error(`404 no durable Space ${name}`)
  if (d.lease !== token || !leased(d)) throw new Error(`403 the lease of ${name} with token ${token} is no longer held (it ran out or was taken); acquire it again`)
  return d
}
function release(s) {
  const d = s.spec.space ? durables[s.spec.space] : undefined
  if (d && s.lease && d.lease === s.lease && d.holder === `session:${s.name}`) {
    d.holder = ""
    d.expires = 0
    saveDurables()
  }
}
const save = () => {
  writeFileSync(`${statePath}.tmp`, JSON.stringify(sessions))
  renameSync(`${statePath}.tmp`, statePath)
}
const view = (s) => ({
  name: s.name,
  state: s.state,
  reason: s.reason,
  epoch: s.epoch,
  attempts: s.attempts,
  max_attempts: s.max_attempts,
  next_attempt_in_secs: s.state === "backoff" ? Math.max(0, Math.ceil((s.next_ms - Date.now()) / 1000)) : null,
  driver_job: s.job,
  error: s.error,
  history: s.history,
})
const hist = (s, event, extra = {}) => s.history.push({ epoch: s.epoch, attempt: s.attempts, at_ms: Date.now(), event, node: "fake", job: s.job, exit_code: null, detail: "", ...extra })

// The real re-driver's stall limit: seconds without new output (default 120, 0 off).
const stallSecs = (s) => s.spec.stall_secs ?? 120

function drive(s) {
  let lease
  if (s.spec.space) {
    try {
      lease = acquire(s.spec.space, `session:${s.name}`)
    } catch (error) {
      s.state = "pending"
      s.error = String(error.message).replace(/^\d+ /, "")
      return
    }
    s.error = ""
  }
  s.lease = lease?.lease_token ?? 0
  s.epoch = Math.max(s.epoch + 1, s.lease)
  s.attempts += 1
  s.job += 1
  s.state = "running"
  hist(s, "started")
  s.outputs[s.job] = ""
  const jobData = join(home, "jobs", s.name)
  mkdirSync(jobData, { recursive: true })
  const env = {
    ...process.env,
    ...s.spec.env,
    CUA_JOB_DATA: jobData,
    CUA_SESSION: s.name,
    CUA_SESSION_EPOCH: String(s.epoch),
    CUA_SESSION_ATTEMPT: String(s.attempts),
    CUA_SESSION_RESUME: s.epoch > 1 ? "1" : "0",
    CUA_SESSION_SPACE: s.spec.space ?? "",
    CUA_SESSION_STATE_DIR: s.spec.state_dir ?? "",
    CUA_SESSION_HEARTBEAT_SECS: String(stallSecs(s) > 0 ? Math.max(1, Math.floor(stallSecs(s) / 4)) : 0),
    CUA_SESSION_SPACE_ADDR: lease ? lease.space : "direct:127.0.0.1:1",
    CUA_SESSION_SPACE_TOKEN: lease ? lease.token : "fake",
    CUA_SESSION_SPACE_LEASE: String(lease ? lease.lease_token : s.epoch),
  }
  const [program, ...args] = s.spec.argv
  const child = spawn(program, args, { env, stdio: ["ignore", "pipe", "pipe"], detached: true })
  const job = s.job
  const append = (d) => {
    s.outputs[job] += d.toString("utf8")
  }
  child.stdout.on("data", append)
  child.stderr.on("data", append)
  drivers.set(s.name, { child, job, size: 0, grew: Date.now(), stalled: false })
  let timer
  if (s.spec.timeout_secs) timer = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL") } catch {} }, s.spec.timeout_secs * 1000)
  child.on("exit", (code, signal) => {
    clearTimeout(timer)
    if (drivers.get(s.name)?.child !== child || s.job !== job) return
    const stalled = drivers.get(s.name).stalled
    drivers.delete(s.name)
    const exit_code = code ?? 137
    if (s.state === "stopped") return
    if (exit_code === 0) {
      s.state = "done"
      hist(s, "done", { exit_code })
    } else if (exit_code === 78) {
      s.state = "failed"
      s.reason = "permanent"
      s.error = "the driver reported a permanent failure"
      hist(s, "permanent", { exit_code })
    } else {
      hist(s, stalled ? "stalled" : exit_code === 75 ? "fenced" : "crashed", { exit_code, detail: stalled ? "wrote no output; wedged, cancelled" : (signal ?? "") })
      if (s.attempts >= s.max_attempts) {
        s.state = "failed"
        s.reason = "exhausted"
        s.error = `the driver failed ${s.attempts} times (last exit ${exit_code}); retries exhausted`
        hist(s, "exhausted")
      } else {
        s.state = "backoff"
        s.next_ms = Date.now() + BACKOFF * 2 ** (s.attempts - 1)
        s.error = `driver exited ${exit_code}`
      }
    }
    if (["done", "failed", "stopped"].includes(s.state)) release(s)
    save()
  })
  save()
}

setInterval(() => {
  for (const s of Object.values(sessions)) {
    const d = drivers.get(s.name)
    if (s.state === "running" && d && stallSecs(s) > 0) {
      const size = Buffer.byteLength(s.outputs[d.job] ?? "")
      if (size > d.size) {
        d.size = size
        d.grew = Date.now()
      } else if (!d.stalled && Date.now() - d.grew >= (d.size === 0 ? Math.max(stallSecs(s), 600) : stallSecs(s)) * 1000) {
        d.stalled = true
        // A frozen (SIGSTOP) group needs SIGCONT to take the kill.
        for (const sig of ["SIGKILL", "SIGCONT"]) try { process.kill(-d.child.pid, sig) } catch {}
      }
    }
    if (s.state === "backoff" && Date.now() >= s.next_ms) drive(s)
    else if (s.state === "pending" && s.job === 0 && s.attempts === 0) drive(s)
    // After a restart of this stand-in a driver that was running is gone: treat it as crashed.
    if (s.state === "running" && !drivers.has(s.name)) {
      hist(s, "crashed", { exit_code: 137, detail: "lost with the coordinator" })
      s.state = "backoff"
      s.next_ms = Date.now()
    }
  }
}, 100)

function handle(method, p) {
  switch (method) {
    case "durable_create":
      durables[p.name] ??= { name: p.name, generation: 1, holder: "", fence: 0, lease: 0, expires: 0 }
      saveDurables()
      return durableView(durables[p.name])
    case "durable_get":
      if (!durables[p.name]) throw new Error(`404 no durable Space ${p.name}`)
      return durableView(durables[p.name])
    case "durable_delete":
      delete durables[p.name]
      saveDurables()
      return { name: p.name }
    case "durable_connection":
      return { ...durableView(withLease(p.name, p.token)), lease_token: p.token }
    case "durable_renew":
      return { ...durableView(withLease(p.name, p.token)), lease_token: p.token }
    case "durable_release": {
      const d = withLease(p.name, p.token)
      d.holder = ""
      d.expires = 0
      saveDurables()
      return { name: p.name, released: true }
    }
    case "x_remove_space": {
      // The host lost the Space: the coordinator places generation + 1 from scratch (the stand-in Space's files go).
      const d = durables[p.name]
      d.generation += 1
      saveDurables()
      for (const s of Object.values(sessions)) {
        if (s.spec.space !== p.name) continue
        for (const dir of [s.spec.env?.CRASH_LOCAL_DIR, s.spec.env?.CRASH_WORK]) if (dir) rmSync(dir, { recursive: true, force: true })
        if (s.spec.env?.CRASH_WORK) mkdirSync(s.spec.env.CRASH_WORK, { recursive: true })
      }
      return { generation: d.generation }
    }
    case "session_start": {
      const s = { name: p.name, spec: p.spec, max_attempts: p.max_attempts || 5, state: "pending", reason: "", epoch: 0, attempts: 0, job: 0, error: "", history: [], outputs: {}, next_ms: 0 }
      sessions[p.name] = s
      drive(s)
      return view(s)
    }
    case "session_get":
      return view(sessions[p.name])
    case "session_output": {
      const s = sessions[p.name]
      const text = s.outputs[s.job] ?? ""
      const from = Math.min(p.from ?? 0, Buffer.byteLength(text))
      const chunk = Buffer.from(text).subarray(from).toString("utf8")
      return { name: p.name, epoch: s.epoch, job: s.job, from, next: from + Buffer.byteLength(chunk), text: chunk, done: ["done", "failed", "stopped"].includes(s.state) }
    }
    case "session_stop": {
      const s = sessions[p.name]
      s.state = "stopped"
      release(s)
      drivers.get(p.name)?.child.kill("SIGKILL")
      save()
      return view(s)
    }
    case "session_delete":
      if (sessions[p.name]) release(sessions[p.name])
      drivers.get(p.name)?.child.kill("SIGKILL")
      delete sessions[p.name]
      save()
      return { name: p.name }
    case "x_kill_driver": {
      const d = drivers.get(p.name)
      if (!d) throw new Error("no running driver")
      try { process.kill(-d.child.pid, "SIGKILL") } catch {}
      return { killed: true }
    }
    default:
      throw new Error(`no request ${method}`)
  }
}

const server = createServer((socket) => {
  let buffer = ""
  socket.on("data", (chunk) => {
    buffer += chunk.toString("utf8")
    let newline
    while ((newline = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, newline)
      buffer = buffer.slice(newline + 1)
      let req
      try { req = JSON.parse(line) } catch { continue }
      if (req.method === "quit") { for (const d of drivers.values()) try { process.kill(-d.child.pid, "SIGKILL") } catch {}; process.exit(0) }
      try {
        socket.write(`${JSON.stringify({ id: req.id, status: 200, body: handle(req.method, req.params ?? {}) })}\n`)
      } catch (error) {
        const status = Number(/^(\d{3}) /.exec(String(error.message))?.[1] ?? 404)
        socket.write(`${JSON.stringify({ id: req.id, status, body: { error: String(error.message) } })}\n`)
      }
    }
  })
  socket.on("error", () => {})
})
server.listen(port, "127.0.0.1", () => console.error(`lab control: 127.0.0.1:${port}`))
