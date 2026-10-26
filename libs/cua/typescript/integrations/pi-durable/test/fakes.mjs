// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// In-process stand-ins for the cua SDK's SpacesdClient, for unit tests that
// need no Space.

import { BACKGROUND_CONTEXT, withAbortSignal } from "@earendil-works/chord/context"

export const context = BACKGROUND_CONTEXT

export function abortable() {
  const controller = new AbortController()
  return { controller, context: withAbortSignal(controller.signal, BACKGROUND_CONTEXT) }
}

const encoder = new TextEncoder()

/** A scripted guest process: `script` is a list of [kind, text] and a final exit. */
export class FakeProcess {
  constructor(events, { stdin } = {}) {
    this.events = events
    this.written = []
    this.waiters = []
    this.stdinHandler = stdin
    this.detached = false
  }
  pid() {
    return 42
  }
  push(event) {
    const waiter = this.waiters.shift()
    if (waiter) waiter(event)
    else this.events.push(event)
  }
  async nextEvent() {
    if (this.events.length > 0) return this.events.shift()
    return new Promise((resolve) => this.waiters.push(resolve))
  }
  async writeStdin(data) {
    this.written.push(new Uint8Array(data))
    this.stdinHandler?.(new Uint8Array(data), this)
  }
  async closeStdin() {}
  async detach() {
    this.detached = true
  }
}

export function out(text, kind = 0) {
  const data = encoder.encode(text)
  return { kind, offset: 0n, data: data.buffer.slice(data.byteOffset, data.byteOffset + data.length) }
}

export function exit(exit) {
  return { kind: 3, offset: 0n, data: new ArrayBuffer(0), exit: { timedOut: false, success: exit.code === 0, ...exit } }
}

/** A SpacesdClient whose `spawn` answers from `respond(command)`. */
export class FakeSpacesd {
  constructor(respond) {
    this.respond = respond
    this.spawned = []
    this.calls = []
  }
  async spawn(command) {
    this.spawned.push(command)
    const result = await this.respond(command)
    if (result instanceof Error) throw result
    return result instanceof FakeProcess ? result : new FakeProcess(result)
  }
  async callJson(method, json) {
    this.calls.push([method, JSON.parse(json)])
    return "{}"
  }
  async upload() {}
  async download() {
    return new ArrayBuffer(0)
  }
}
