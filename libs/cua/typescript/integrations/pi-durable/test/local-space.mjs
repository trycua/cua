// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A cua-spacesd stand-in that runs commands on this machine, for tests of the
// durable layer that need no Space. It keeps the parts of the daemon the
// durable layer relies on, on disk so they survive the death of the test
// process that used them (the "daemon" outlives its client):
//
// - the exec ledger: a command with a request id is claimed atomically
//   (mkdir) and runs detached with its output and exit code in files, so a
//   later spawn with the same id replays or reattaches;
// - the fencing epoch: a spawn with a lower epoch is refused, AdvanceEpoch
//   raises it and kills older-epoch commands;
// - GetExecRecord, SignalProcess, upload and download.
//
// It is not isolated: commands run as this user. Tests point `cwd` at a
// temporary directory and never touch anything else.

import { spawn } from "node:child_process"
import { createHash, randomUUID } from "node:crypto"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { dirname, join } from "node:path"

const encoder = new TextEncoder()

function alive(pid) {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function read(path) {
  try {
    return readFileSync(path)
  } catch {
    return Buffer.alloc(0)
  }
}

class LocalProcess {
  constructor(dir) {
    this.dir = dir
    this.out = 0
    this.err = 0
    this.done = false
  }
  pid() {
    return Number(read(join(this.dir, "pid")).toString() || 0)
  }
  async nextEvent() {
    for (;;) {
      if (this.done) return undefined
      const meta = JSON.parse(read(join(this.dir, "meta.json")).toString() || "{}")
      const exited = existsSync(join(this.dir, "exit"))
      const out = read(join(this.dir, "out"))
      if (out.length > this.out) {
        const chunk = out.subarray(this.out)
        this.out = out.length
        return { kind: 0, offset: 0n, data: chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.length) }
      }
      const err = read(join(this.dir, "err"))
      if (err.length > this.err) {
        const chunk = err.subarray(this.err)
        this.err = err.length
        return { kind: 1, offset: 0n, data: chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.length) }
      }
      if (exited) {
        this.done = true
        const code = Number(read(join(this.dir, "exit")).toString().trim())
        const exit = code > 128 && code < 160 ? { signal: code === 137 ? "kill" : "term", timedOut: false, success: false } : { code, timedOut: false, success: code === 0 }
        return { kind: 3, offset: 0n, data: new ArrayBuffer(0), exit }
      }
      const pid = Number(read(join(this.dir, "pid")).toString() || 0)
      // A client that died between claiming the key and starting the command leaves no pid: lost.
      const orphaned = !pid && Date.now() - statSync(this.dir).mtimeMs > 2000
      if ((orphaned || (pid && !alive(pid))) && !existsSync(join(this.dir, "exit"))) {
        this.done = true
        return {
          kind: 3,
          offset: 0n,
          data: new ArrayBuffer(0),
          exit: { timedOut: false, success: false, error: "the command was running when cua-spacesd stopped; its outcome is unknown" },
        }
      }
      void meta
      await new Promise((resolve) => setTimeout(resolve, 8))
    }
  }
  async writeStdin() {}
  async closeStdin() {}
  async detach() {}
}

export class LocalSpacesd {
  /**
   * @param stateDir where the ledger and the epoch live; keep it outside the workspace.
   * @param options.holdStart a file path: while it exists, a started command waits before it
   *   records its pid (tests use it to make the start-versus-AdvanceEpoch race deterministic).
   */
  constructor(stateDir, options = {}) {
    this.stateDir = stateDir
    this.holdStart = options.holdStart
    mkdirSync(join(stateDir, "ledger"), { recursive: true })
    this.spawned = []
  }

  epoch() {
    return Number(read(join(this.stateDir, "epoch")).toString() || 0)
  }

  #dir(key) {
    return join(this.stateDir, "ledger", createHash("sha256").update(key).digest("hex").slice(0, 24))
  }

  async spawn(command) {
    this.spawned.push(command)
    const current = this.epoch()
    if (command.epoch !== undefined && Number(command.epoch) < current) {
      throw new Error(`epoch ${command.epoch} is older than the Space's current epoch ${current}`)
    }
    const key = command.requestId ?? `anon-${randomUUID()}`
    const dir = this.#dir(key)
    // Claim the key atomically AND completely: the entry is built where nobody looks (claims/) and
    // renamed into the ledger, so a reader (the next worker's GetExecRecord) never sees a half-made
    // entry, even when this client is frozen (SIGSTOP) or killed at any instruction. The real daemon
    // does the claim and the start in one request; a half-written meta.json here was a stand-in
    // artefact ("Unexpected end of JSON input" in a starved CI run).
    for (;;) {
      const claim = join(this.stateDir, "claims", randomUUID())
      mkdirSync(claim, { recursive: true })
      writeFileSync(join(claim, "meta.json"), JSON.stringify({ tag: command.tag, epoch: Number(command.epoch ?? 0), key }))
      try {
        renameSync(claim, dir)
        break
      } catch (error) {
        rmSync(claim, { recursive: true, force: true })
        if (error.code !== "ENOTEMPTY" && error.code !== "EEXIST") throw error
        // The key is claimed. The real daemon refuses a stale epoch and claims in one step, so a
        // claim from before the Space's epoch moved on whose command never ran can only be a
        // client that passed the epoch check, was frozen, and claimed after the advance: its
        // command never runs (it checks the epoch once its pid is visible, and marks itself aborted) and the key is free.
        if (!(await this.#deadClaim(dir, command))) return new LocalProcess(dir)
        try {
          renameSync(dir, join(this.stateDir, "claims", `dead-${randomUUID()}`))
        } catch {}
      }
    }
    const env = {
      ...process.env,
      ...Object.fromEntries(command.env ?? []),
      LEDGER: dir,
      STATE: this.stateDir,
      CMD_EPOCH: String(Number(command.epoch ?? 0)),
      HOLD: this.holdStart ?? "",
    }
    // The command records its own pid first thing (atomically), so a client frozen right after
    // starting it cannot leave a running command that the ledger calls lost. Then it looks at the
    // Space's epoch: the real daemon starts a process and applies the fence in one step, so a
    // process that AdvanceEpoch could not see yet (no pid) never runs on past the advance. Here
    // the advance writes the epoch and then looks for pids; this writes its pid and then looks
    // at the epoch, so one of the two always sees the other (found by campaign seeds 1101 and
    // 1303: a command claimed but not yet started, or still starting, when the next worker fenced
    // the Space ran after the undo; traced for 1303 in stale mode: the claim, a 3 s freeze of the
    // client before the fork, GetExecRecord = lost, AdvanceEpoch 3 and 4 killing nothing, then the fork).
    const script =
      '[ -n "$HOLD" ] && while [ -e "$HOLD" ]; do sleep 0.01; done; echo $$ >"$LEDGER/pid.tmp" && mv "$LEDGER/pid.tmp" "$LEDGER/pid"; ' +
      'if [ "$CMD_EPOCH" -gt 0 ] && [ "$(cat "$STATE/epoch" 2>/dev/null || echo 0)" -gt "$CMD_EPOCH" ]; then : >"$LEDGER/aborted"; exit 0; fi; ' +
      '"$@" >"$LEDGER/out" 2>"$LEDGER/err" </dev/null; echo $? > "$LEDGER/exit.tmp"; mv "$LEDGER/exit.tmp" "$LEDGER/exit"'
    const child = spawn("/bin/sh", ["-c", script, "sh", command.program, ...command.args], {
      cwd: command.cwd,
      env,
      detached: true,
      stdio: "ignore",
    })
    child.unref()
    return new LocalProcess(dir)
  }

  /**
   * Whether the claim at `dir` is a dead one for this request: made under an older epoch than the
   * request's and the Space's, and its command never ran. A command that has not started, or that is
   * still before its epoch check, will abort itself (the epoch has moved on), so this waits for the
   * outcome of that; one that exited, or was killed while it ran, is a real record and is replayed.
   */
  async #deadClaim(dir, command) {
    const wanted = Number(command.epoch ?? 0)
    let claimed
    try {
      claimed = Number(JSON.parse(read(join(dir, "meta.json")).toString() || "{}").epoch ?? 0)
    } catch {
      return false
    }
    if (!(wanted > 0 && claimed > 0 && claimed < wanted && claimed < this.epoch())) return false
    for (let i = 0; i < 600; i++) {
      if (existsSync(join(dir, "exit"))) return false
      if (existsSync(join(dir, "aborted"))) return true
      const pid = Number(read(join(dir, "pid")).toString() || 0)
      if (!pid) return true
      if (!alive(pid)) return !existsSync(join(dir, "exit")) && existsSync(join(dir, "aborted"))
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
    return false
  }

  #entries() {
    const root = join(this.stateDir, "ledger")
    return readdirSync(root).map((name) => {
      const dir = join(root, name)
      const meta = JSON.parse(read(join(dir, "meta.json")).toString() || "{}")
      return { dir, meta, pid: Number(read(join(dir, "pid")).toString() || 0), exited: existsSync(join(dir, "exit")) }
    })
  }

  async callJson(method, json) {
    const request = JSON.parse(json)
    if (method === "ProcessService/GetExecRecord") {
      const dir = this.#dir(request.requestId)
      const epoch = String(this.epoch())
      // No meta.json, or one still empty: the key is not claimed (yet), as far as any other client can tell.
      const raw = read(join(dir, "meta.json")).toString()
      if (!raw) return JSON.stringify({ epoch })
      const meta = JSON.parse(raw)
      const exited = existsSync(join(dir, "exit"))
      const pid = Number(read(join(dir, "pid")).toString() || 0)
      const record = {
        state: exited ? "EXEC_STATE_EXITED" : pid && alive(pid) ? "EXEC_STATE_RUNNING" : "EXEC_STATE_LOST",
        epoch: String(meta.epoch),
        outputBytes: String(read(join(dir, "out")).length),
      }
      if (exited) record.end = { exitCode: Number(read(join(dir, "exit")).toString().trim()) }
      return JSON.stringify({ record, epoch })
    }
    if (method === "ProcessService/AdvanceEpoch") {
      const target = Number(request.epoch)
      if (target < this.epoch()) throw new Error(`epoch ${target} is older than the Space's current epoch ${this.epoch()}`)
      writeFileSync(join(this.stateDir, "epoch.tmp"), String(target))
      renameSync(join(this.stateDir, "epoch.tmp"), join(this.stateDir, "epoch"))
      let killed = 0
      for (const entry of this.#entries()) {
        if (!entry.exited && entry.pid && alive(entry.pid) && entry.meta.epoch < target) {
          try {
            process.kill(-entry.pid, "SIGKILL")
            killed++
          } catch {}
        }
      }
      return JSON.stringify({ epoch: String(target), killed })
    }
    if (method === "ProcessService/SignalProcess") {
      for (const entry of this.#entries()) {
        if (entry.meta.tag === request.process?.tag && entry.pid) {
          try {
            process.kill(-entry.pid, "SIGKILL")
          } catch {}
        }
      }
      return "{}"
    }
    throw new Error(`LocalSpacesd: unsupported ${method}`)
  }

  async upload(path, data, options) {
    mkdirSync(dirname(path), { recursive: options?.createParents ?? true })
    writeFileSync(path, Buffer.from(data))
    if (options?.permissions) chmodSync(path, options.permissions)
  }

  async download(path) {
    const bytes = readFileSync(path)
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length)
  }
}

/** A `SpaceProvider` for one `LocalSpacesd`. */
export function localSpaces(spacesd, { kind = "container" } = {}) {
  return {
    exists: async () => true,
    ensure: async () => {},
    spacesd: async () => spacesd,
    invalidate: () => {},
    kind: async () => kind,
  }
}

/** A `SnapshotProvider` that remembers calls and keeps nothing. */
export class CountingSnapshots {
  constructor() {
    this.calls = []
    this.wholeMachine = false
  }
  async checkpoint(space, label) {
    this.calls.push(["checkpoint", label])
    return label
  }
  async restore(space, id) {
    this.calls.push(["restore", id])
  }
  async fork(id) {
    this.calls.push(["fork", id])
  }
}

export function workspaceListing(root) {
  const out = {}
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name)
      const info = statSync(path)
      if (info.isDirectory()) walk(path)
      else out[path.slice(root.length + 1)] = createHash("sha256").update(readFileSync(path)).digest("hex")
    }
  }
  walk(root)
  return out
}

export { encoder }
