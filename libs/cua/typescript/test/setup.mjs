// Preloaded into every test process (`node --import ./test/setup.mjs --test`):
// a private CUA_HOME, and CUA_TEST=1 so the SDK refuses any write to the
// user's real ~/.cua (Spaces registry, sandbox state, tokens) instead of
// leaking test state into their Spaces app. A test file may still point
// CUA_HOME at its own temp dir.
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

process.env.CUA_TEST = "1"
process.env.CUA_HOME = mkdtempSync(join(tmpdir(), "cua-ts-test-"))
