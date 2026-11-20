// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A real Space for the opt-in tests: set CUA_PI_SPACESD_URL and
// CUA_PI_SPACESD_TOKEN to a cua-spacesd you own (for example a local
// container), or CUA_PI_SPACE to the name of a sandbox of the local cua
// daemon. Tests that need one skip without either. With a URL the SDK runs
// with a private CUA_HOME.

import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

export const SPACE_NAME = process.env.CUA_PI_SPACE
export const SPACE_URL = process.env.CUA_PI_SPACESD_URL ?? (SPACE_NAME ? `daemon:${SPACE_NAME}` : undefined)
export const SPACE_TOKEN = process.env.CUA_PI_SPACESD_TOKEN

/** cua-spacesd of the configured Space, through the cua SDK. */
export async function connectSpace(name = "pi-durable-test") {
  if (SPACE_NAME) {
    const { connect } = await import("@trycua/cua")
    return (await connect().sandboxes().connect(SPACE_NAME)).spacesd(60_000)
  }
  process.env.CUA_TEST = "1"
  process.env.CUA_HOME ??= mkdtempSync(join(tmpdir(), "cua-pi-test-"))
  const { embedded } = await import("@trycua/cua")
  const sandbox = await embedded().sandboxes().connectUrl(SPACE_URL, SPACE_TOKEN, name)
  return sandbox.spacesd(30_000)
}

/** Node assertions in the shape of Pi's conformance suite. */
export function assertions(assert) {
  const partial = (actual, expected, path = "value") => {
    if (expected === null || typeof expected !== "object") {
      assert.strictEqual(actual, expected, path)
      return
    }
    assert.ok(actual !== null && typeof actual === "object", `${path} is an object`)
    for (const [key, value] of Object.entries(expected)) partial(actual[key], value, `${path}.${key}`)
  }
  return {
    ok: (value, message) => assert.ok(value, message),
    strictEqual: (actual, expected) => assert.strictEqual(actual, expected),
    deepEqual: (actual, expected) => assert.deepStrictEqual(actual, expected),
    partialDeepEqual: (actual, expected) => partial(actual, expected),
    greaterThan: (actual, expected) => assert.ok(actual > expected, `${actual} > ${expected}`),
    rejects: (operation, messageIncludes) =>
      assert.rejects(operation, (error) => String(error?.message ?? error).includes(messageIncludes)),
  }
}
