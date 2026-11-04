// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Pi Durable's ExecutionEnv conformance suite against SpacesExecutionEnv on a
// real Space (opt-in: see space.mjs).

import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { test } from "node:test"
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context"
import { createEnvConformance } from "@earendil-works/pi-durable/testing"
import { SpacesExecutionEnv } from "../dist/index.js"
import { sh } from "../dist/spacesd.js"
import { assertions, connectSpace, SPACE_URL } from "./space.mjs"

if (!SPACE_URL) {
  test("ExecutionEnv conformance on a Space (set CUA_PI_SPACESD_URL)", { skip: true }, () => {})
} else {
  const spacesd = await connectSpace()
  const cases = createEnvConformance({
    assertions: assertions(assert),
    shell: ["sh", "-c"],
    withEnv: async (use) => {
      const cwd = `/tmp/pi-conformance-${randomUUID()}`
      const made = await sh(spacesd, `mkdir -p '${cwd}'`)
      assert.equal(made.code, 0, made.stderr)
      const env = new SpacesExecutionEnv({ spacesd, id: `cua-space:conformance:${cwd}`, cwd })
      try {
        await use(env)
      } finally {
        await env.cleanup(BACKGROUND_CONTEXT)
        await env.close()
        await sh(spacesd, `rm -rf '${cwd}'`)
      }
    },
  })
  for (const testCase of cases) {
    test(testCase.name, { timeout: testCase.timeoutMs ?? 60_000 }, () => testCase.run())
  }
}
