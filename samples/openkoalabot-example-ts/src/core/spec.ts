/**
 * The shared scenario (samples/openkoalabot-example-scenario/scenario.json): types,
 * validation, placeholder substitution and the deterministic fixtures every
 * implementation generates the same way.
 */
import { createHash, randomBytes } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

export type Lane = "fixture" | "docker" | "cloud"
export const LANES: readonly Lane[] = ["fixture", "docker", "cloud"]

export interface Step {
  id: string
  op: string
  [key: string]: unknown
}

/** The scripted model the agent-turn steps use (`scenario.json` `model`). */
export interface ScenarioModel {
  /** The environment variable holding the endpoint URL as the Space reaches it. */
  urlEnv: string
  /** The provider key variable forwarded from the runner's environment. */
  keyVar: string
  name: string
}

export interface Scenario {
  name: string
  version: number
  model?: ScenarioModel
  steps: Step[]
}

export const KNOWN_OPS = new Set([
  "space.open",
  "stream.desktop",
  "agent.thread",
  "file.send",
  "teleport.app",
  "presence.pair",
  "routine.schedule",
  "group.chat",
  "space.delete",
])

/** Parses and checks a scenario document. Throws naming the first problem. */
export function parseScenario(text: string): Scenario {
  const doc = JSON.parse(text) as Partial<Scenario>
  if (typeof doc.name !== "string") throw new Error("scenario: missing name")
  if (doc.version !== 2) throw new Error(`scenario: unsupported version ${String(doc.version)}`)
  if (!Array.isArray(doc.steps) || doc.steps.length === 0) throw new Error("scenario: no steps")
  const seen = new Set<string>()
  for (const s of doc.steps) {
    if (!s || typeof s.id !== "string" || typeof s.op !== "string") throw new Error("scenario: a step lacks id/op")
    if (!KNOWN_OPS.has(s.op)) throw new Error(`scenario: unknown op ${s.op} (step ${s.id})`)
    if (seen.has(s.id)) throw new Error(`scenario: duplicate step id ${s.id}`)
    seen.add(s.id)
  }
  if (doc.steps[0].op !== "space.open") throw new Error("scenario: the first step must be space.open")
  const m = doc.model
  if (doc.steps.some((s) => s.needs === "model") && (!m || typeof m.urlEnv !== "string" || typeof m.keyVar !== "string" || typeof m.name !== "string")) {
    throw new Error("scenario: steps need a model but `model` lacks urlEnv/keyVar/name")
  }
  return doc as Scenario
}

/** Replaces `{name}` placeholders; unknown ones are left as they are. */
export function substitute(template: string, values: Record<string, string>): string {
  return template.replace(/\{([A-Za-z]+)\}/g, (whole, key: string) =>
    Object.prototype.hasOwnProperty.call(values, key) ? values[key] : whole,
  )
}

/** 8 random hex characters. */
export function newNonce(): string {
  return randomBytes(4).toString("hex")
}

/** The spec's xorshift64 byte stream (`x ^= x<<13; x ^= x>>7; x ^= x<<17`,
 * u64 wrapping, byte = x & 0xff). */
export function xorshiftBytes(length: number, seed: bigint): Uint8Array {
  const mask = (1n << 64n) - 1n
  let x = seed & mask
  const out = new Uint8Array(length)
  for (let i = 0; i < length; i++) {
    x ^= (x << 13n) & mask
    x ^= x >> 7n
    x ^= (x << 17n) & mask
    out[i] = Number(x & 0xffn)
  }
  return out
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex")
}

export interface ProfileFixture {
  roots: string[]
  files: Record<string, string>
}

/** Writes the generated Firefox profile under `home` (a temp directory the
 * SDK reads as its teleport home). Returns the files written. */
export function writeProfile(home: string, fixture: ProfileFixture, marker: string): string[] {
  const written: string[] = []
  for (const root of fixture.roots) {
    for (const [rel, content] of Object.entries(fixture.files)) {
      const path = join(home, root, rel)
      mkdirSync(dirname(path), { recursive: true })
      writeFileSync(path, substitute(content, { marker }))
      written.push(path)
    }
  }
  return written
}

/** POSIX single-quote escaping for a guest shell. */
export function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`
}

export type StepStatus = "pass" | "fail" | "skip"

export interface StepResult {
  id: string
  status: StepStatus
  ms: number
  detail: string
}

export interface ScenarioResult {
  impl: "ts"
  lane: Lane
  ok: boolean
  totalMs: number
  steps: StepResult[]
  error?: string
}

/** A step outcome that is not a failure. */
export class Skip extends Error {}
