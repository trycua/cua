#!/usr/bin/env node
/**
 * openkoalabot-example-ts scenario runner (samples/openkoalabot-example-scenario):
 *
 *   node dist/scenario/cli.js --spec <scenario.json> --lane fixture|docker|cloud --out <result.json>
 *
 * Environment: OPENKOALABOTS_SCENARIO_URL / _TOKEN (fixture, docker),
 * OPENKOALABOTS_SCENARIO_IMPORT_ROOT (the guest directory teleport imports
 * under; default `$HOME`), OPENKOALABOTS_CLOUD_IMAGE (cloud), and the spec's
 * `model.urlEnv` (OPENKOALABOTS_SCENARIO_MODEL_URL) for the agent-turn
 * steps. Headless.
 */
import { readFileSync, writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { LANES, type Lane, parseScenario } from "../core/spec.js"
import { runScenario } from "./runner.js"

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`)
  return i >= 0 ? process.argv[i + 1] : undefined
}

async function main(): Promise<number> {
  const specPath = resolve(arg("spec") ?? "../openkoalabot-example-scenario/scenario.json")
  const lane = (arg("lane") ?? "fixture") as Lane
  if (!LANES.includes(lane)) throw new Error(`--lane must be one of ${LANES.join(", ")}`)
  const out = arg("out")
  const spec = parseScenario(readFileSync(specPath, "utf8"))
  const result = await runScenario({
    spec,
    specDir: dirname(specPath),
    lane,
    url: process.env.OPENKOALABOTS_SCENARIO_URL || undefined,
    token: process.env.OPENKOALABOTS_SCENARIO_TOKEN ?? undefined,
    importRoot: process.env.OPENKOALABOTS_SCENARIO_IMPORT_ROOT || "$HOME",
    cloudImage: process.env.OPENKOALABOTS_CLOUD_IMAGE || undefined,
    model: spec.model && process.env[spec.model.urlEnv] ? { url: process.env[spec.model.urlEnv] as string, name: spec.model.name, keyVar: spec.model.keyVar } : undefined,
    log: (l) => console.error(`[ts/${lane}] ${l}`),
  })
  const text = JSON.stringify(result, null, 2)
  if (out) writeFileSync(out, text + "\n")
  else console.log(text)
  return result.ok ? 0 : 1
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(e instanceof Error ? e.stack : e)
    process.exit(2)
  },
)
