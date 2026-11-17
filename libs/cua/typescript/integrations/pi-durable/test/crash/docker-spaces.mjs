// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Test Spaces: local Docker containers that run a cua-spacesd build, reached
// through the cua SDK. Only for the opt-in crash tests on a machine you own;
// every container is labelled and removed by `removeAll()`.

import { execFileSync } from "node:child_process"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

export const IMAGE = process.env.CUA_PI_SPACE_IMAGE ?? "cua-pi-space:dev"
const LABEL = "ai.cua.test=pi-durable"

function docker(...args) {
  return execFileSync("docker", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim()
}

let cua
async function sdk() {
  if (cua === undefined) {
    process.env.CUA_TEST = "1"
    process.env.CUA_HOME ??= mkdtempSync(join(tmpdir(), "cua-pi-crash-"))
    const { embedded } = await import("@trycua/cua")
    cua = embedded()
  }
  return cua
}

export class DockerSpaces {
  constructor({ token, prefix = "cua-pi-" } = {}) {
    this.token = token
    this.prefix = prefix
    this.clients = new Map()
  }

  container(name) {
    return `${this.prefix}${name}`
  }

  async exists(name) {
    try {
      docker("inspect", "--type", "container", this.container(name))
      return true
    } catch {
      return false
    }
  }

  async ensure(name) {
    if (await this.exists(name)) return
    docker(
      "run", "-d", "--name", this.container(name), "--label", LABEL,
      "-e", `CUA_ENV_TOKEN=${this.token}`, "-p", "127.0.0.1::3211", IMAGE,
    )
  }

  url(name) {
    const mapped = docker("port", this.container(name), "3211/tcp").split("\n")[0]
    return `http://${mapped.replace("0.0.0.0", "127.0.0.1")}`
  }

  /** The container's address on the engine's network: how another container (a session driver) reaches it. */
  address(name) {
    const ip = docker("inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}", this.container(name)).split(" ")[0]
    if (!ip) throw new Error(`container ${this.container(name)} has no address`)
    return `http://${ip}:3211`
  }

  spacesd(name) {
    let client = this.clients.get(name)
    if (client === undefined) {
      client = (async () => {
        const deadline = Date.now() + 60_000
        for (;;) {
          try {
            const sandbox = await (await sdk()).sandboxes().connectUrl(this.url(name), this.token, name)
            return await sandbox.spacesd(10_000)
          } catch (error) {
            if (Date.now() > deadline) throw error
            await new Promise((resolve) => setTimeout(resolve, 250))
          }
        }
      })()
      this.clients.set(name, client)
      client.catch(() => this.clients.delete(name))
    }
    return client
  }

  invalidate(name) {
    this.clients.delete(name)
  }

  /** The Space's machine restarts: every process in it dies, its disk stays. */
  restart(name) {
    this.clients.delete(name)
    docker("restart", "-t", "0", this.container(name))
  }

  async remove(name) {
    this.clients.delete(name)
    try {
      docker("rm", "-f", this.container(name))
    } catch {
      // Already gone.
    }
  }

  static removeAll() {
    const ids = docker("ps", "-aq", "--filter", `label=${LABEL}`)
    if (ids) docker("rm", "-f", ...ids.split("\n"))
  }
}
