#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Translates the Spaces app's UI strings with Claude, like `@lobehub/i18n-cli`:
 * only new or changed `en` keys are sent, per target language, and the result
 * is validated before anything is written.
 *
 *   node scripts/i18n-translate.mjs [--locales ja,es] [--force] [--dry-run]
 *        [--no-backtranslate] [--report path] [--provider api|claude-cli]
 *
 * Source: src/i18n/locales/en/<ns>.json (flat `<area>.<screen>.<element>` keys).
 * Output: src/i18n/locales/<lng>/<ns>.json and src/i18n/i18n.lock.json (the
 * hash of each English string each language was translated from).
 *
 * Provider `api` (default; CI) calls an OpenAI-compatible chat endpoint, our
 * LiteLLM gateway: I18N_BASE_URL, I18N_API_KEY, I18N_MODEL (default
 * claude-sonnet-5-5). Provider `claude-cli` shells out to `claude -p` for local
 * runs. Keys are read from the environment and never printed.
 */
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { TARGET_LOCALES, hashSource, validateTranslation } from "./i18n/lib.mjs"

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const i18nDir = join(app, "src", "i18n")
const localesDir = join(i18nDir, "locales")
const lockPath = join(i18nDir, "i18n.lock.json")
const glossary = JSON.parse(readFileSync(join(i18nDir, "glossary.json"), "utf8"))

const LOCALE_NAMES = {
  "zh-Hans": "Simplified Chinese (zh-Hans)",
  ko: "Korean",
  "pt-BR": "Brazilian Portuguese",
  ja: "Japanese",
  es: "Spanish (neutral, Latin America and Spain)",
}
const BATCH = 40
const RETRIES = 3

function parseArgs(argv) {
  const o = { locales: TARGET_LOCALES, force: false, dryRun: false, back: true, report: null, provider: process.env.I18N_PROVIDER || "api" }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === "--locales") o.locales = argv[++i].split(",").map((s) => s.trim()).filter(Boolean)
    else if (a === "--force") o.force = true
    else if (a === "--dry-run") o.dryRun = true
    else if (a === "--no-backtranslate") o.back = false
    else if (a === "--report") o.report = resolve(argv[++i])
    else if (a === "--provider") o.provider = argv[++i]
    else throw new Error(`unknown argument ${a}`)
  }
  for (const l of o.locales) if (!TARGET_LOCALES.includes(l)) throw new Error(`unsupported locale ${l}`)
  return o
}

const readJson = (p, fallback) => (existsSync(p) ? JSON.parse(readFileSync(p, "utf8")) : fallback)
const writeJson = (p, v) => {
  mkdirSync(dirname(p), { recursive: true })
  writeFileSync(p, JSON.stringify(v, null, 2) + "\n")
}

/** Dotted-key entries of a (possibly nested) catalog: { "a.b": "text" }. */
function flatten(obj, prefix = "", out = {}) {
  for (const [k, v] of Object.entries(obj)) {
    if (v && typeof v === "object") flatten(v, `${prefix}${k}.`, out)
    else out[`${prefix}${k}`] = v
  }
  return out
}

/** `values` (flat) laid out in the same shape as the English catalog `shape`. */
function nestLike(shape, values, prefix = "") {
  const out = {}
  for (const [k, v] of Object.entries(shape)) {
    if (v && typeof v === "object") {
      const inner = nestLike(v, values, `${prefix}${k}.`)
      if (Object.keys(inner).length) out[k] = inner
    } else if (`${prefix}${k}` in values) out[k] = values[`${prefix}${k}`]
  }
  return out
}

/** { ns: { key: english } } from locales/en (nested catalogs are flattened). */
function readSource() {
  const out = {}
  for (const f of readdirSync(join(localesDir, "en")).filter((n) => n.endsWith(".json")).sort()) {
    out[f.replace(/\.json$/, "")] = readJson(join(localesDir, "en", f), {})
  }
  return out
}

// ------------------------------------------------------------------ model

async function complete(system, user, opts) {
  if (opts.provider === "claude-cli") {
    const r = spawnSync("claude", ["-p", "--model", process.env.I18N_MODEL || "sonnet", "--append-system-prompt", system, "--output-format", "text"], {
      input: user,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
    })
    if (r.status !== 0) throw new Error(`claude -p failed (status ${r.status}, ${r.signal ?? "no signal"}): ${(r.stderr || r.stdout || "").slice(0, 300)}`)
    return r.stdout
  }
  const base = process.env.I18N_BASE_URL
  const key = process.env.I18N_API_KEY
  if (!base || !key) throw new Error("I18N_BASE_URL and I18N_API_KEY are required (provider api)")
  const res = await fetch(`${base.replace(/\/+$/, "")}/v1/chat/completions`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify({
      model: process.env.I18N_MODEL || "claude-sonnet-5-5",
      temperature: 0,
      max_tokens: 8000,
      messages: [
        { role: "system", content: system },
        { role: "user", content: user },
      ],
    }),
  })
  if (!res.ok) throw new Error(`gateway ${res.status}: ${(await res.text()).slice(0, 300).replace(key, "***")}`)
  const body = await res.json()
  return body.choices?.[0]?.message?.content ?? ""
}

/** The JSON object in a model reply (tolerates code fences and chatter). */
function extractJson(text) {
  const start = text.indexOf("{")
  const end = text.lastIndexOf("}")
  if (start < 0 || end < start) throw new Error("no JSON object in reply")
  return JSON.parse(text.slice(start, end + 1))
}

function systemPrompt(locale) {
  const forced = Object.entries(glossary.terms[locale] ?? {}).map(([k, v]) => `  "${k}" -> "${v}"`).join("\n")
  return [
    `You translate the UI text of Cua Spaces, a desktop app, from English into ${LOCALE_NAMES[locale]}.`,
    `Tone: ${glossary.tone}`,
    "Rules:",
    "- Input is a JSON object of key -> English. Reply with ONE JSON object with exactly the same keys and the translated values. No prose, no code fences.",
    "- Keys describe where the text appears (<area>.<screen>.<element>). Use them as context only; never translate keys.",
    "- Messages use ICU MessageFormat. Keep every {variable}, every <tag>, and the whole ICU structure. Variable names, select keywords (the words before `{` in select blocks) and the `plural`/`select` keywords stay in English.",
    `- In plural blocks write every plural category ${LOCALE_NAMES[locale]} needs (CLDR: ${new Intl.PluralRules(locale).resolvedOptions().pluralCategories.join(", ")}), plus \`other\`. Use # for the number.`,
    `- Never translate or transliterate these product and brand names; keep them exactly as written, in Latin letters, even inside ${LOCALE_NAMES[locale]} sentences (for example "Cua", never a phonetic spelling, and "Space"/"Spaces", never a word meaning "space"): ${glossary.doNotTranslate.join(", ")}.`,
    "- Use one term per concept, consistently across keys. Preferred terms:",
    forced || "  (none)",
    "- Keep punctuation style natural for the language, keep the same length where you can (UI space is tight), and do not add or drop information.",
  ].join("\n")
}

async function translateBatch(locale, entries, opts) {
  const result = {}
  let pending = { ...entries }
  const problems = {}
  for (let attempt = 1; attempt <= RETRIES && Object.keys(pending).length; attempt++) {
    const feedback = Object.keys(problems).length
      ? `\n\nYour previous answer had problems; fix them:\n${Object.values(problems).flat().join("\n")}`
      : ""
    let reply
    try {
      reply = extractJson(await complete(systemPrompt(locale), JSON.stringify(pending, null, 2) + feedback, opts))
    } catch (e) {
      console.error(`  ${locale}: attempt ${attempt} failed: ${e.message}`)
      await new Promise((r) => setTimeout(r, 5000 * attempt))
      continue
    }
    for (const [key, source] of Object.entries(pending)) {
      const v = validateTranslation({ key, source, translation: reply[key], locale, doNotTranslate: glossary.doNotTranslate })
      if (v.errors.length) {
        problems[key] = v.errors
      } else {
        result[key] = { text: reply[key], warnings: v.warnings }
        delete pending[key]
        delete problems[key]
      }
    }
  }
  return { result, failed: pending, problems }
}

/** Translate each new string back to English; list where the meaning moved. */
async function backTranslate(locale, pairs, opts) {
  if (!pairs.length) return []
  const system = [
    `You review translations of UI text from English into ${LOCALE_NAMES[locale]}.`,
    "Input: JSON object key -> { en, tr }. For each key, translate `tr` back to English literally, then say whether its meaning drifted from `en`.",
    'Reply with ONE JSON object: key -> { "back": "...", "drift": true|false, "note": "short reason when drift is true" }. Flag only real meaning changes (added, lost or changed meaning), not wording or word order.',
  ].join("\n")
  const flags = []
  for (let i = 0; i < pairs.length; i += 60) {
    const chunk = Object.fromEntries(pairs.slice(i, i + 60).map(([k, en, tr]) => [k, { en, tr }]))
    try {
      const reply = extractJson(await complete(system, JSON.stringify(chunk, null, 2), opts))
      for (const [k, v] of Object.entries(reply)) if (v?.drift) flags.push({ locale, key: k, en: chunk[k]?.en, tr: chunk[k]?.tr, back: v.back, note: v.note })
    } catch (e) {
      console.error(`  ${locale}: back-translation skipped for a batch: ${e.message}`)
    }
  }
  return flags
}

function saveLock(lock) {
  const ordered = {}
  for (const l of Object.keys(lock).sort()) ordered[l] = Object.fromEntries(Object.entries(lock[l]).sort(([a], [b]) => a.localeCompare(b)))
  writeJson(lockPath, ordered)
}

// ------------------------------------------------------------------- main

async function main() {
  const opts = parseArgs(process.argv.slice(2))
  const shapes = readSource() // as written in locales/en
  const source = Object.fromEntries(Object.entries(shapes).map(([ns, c]) => [ns, flatten(c)]))
  const lock = readJson(lockPath, {})
  const report = { translated: {}, removed: {}, warnings: [], drift: [], failures: [] }
  let touched = false

  for (const locale of opts.locales) {
    const lockFor = (lock[locale] ??= {})
    const todo = [] // [ns, key, en]
    for (const [ns, strings] of Object.entries(source)) {
      const file = join(localesDir, locale, `${ns}.json`)
      const have = flatten(readJson(file, {}))
      for (const [key, en] of Object.entries(strings)) {
        if (opts.force || !(key in have) || lockFor[key] !== hashSource(en)) todo.push([ns, key, en])
      }
    }
    // Removed keys (and namespaces) leave the locale files.
    const wanted = new Set(Object.entries(source).flatMap(([, s]) => Object.keys(s)))
    const removed = []
    for (const key of Object.keys(lockFor)) if (!wanted.has(key)) { delete lockFor[key]; removed.push(key) }
    report.removed[locale] = removed
    console.log(`${locale}: ${todo.length} to translate, ${removed.length} removed`)

    const outputs = {} // ns -> key -> text
    const newPairs = []
    for (let i = 0; i < todo.length; i += BATCH) {
      const batch = todo.slice(i, i + BATCH)
      if (opts.dryRun) continue
      const { result, failed, problems } = await translateBatch(locale, Object.fromEntries(batch.map(([, k, en]) => [k, en])), opts)
      for (const [ns, key, en] of batch) {
        if (result[key]) {
          ;(outputs[ns] ??= {})[key] = result[key].text
          lockFor[key] = hashSource(en)
          newPairs.push([key, en, result[key].text])
          for (const w of result[key].warnings) report.warnings.push(`${locale} ${w}`)
        } else if (key in failed) {
          report.failures.push(`${locale} ${key}: ${(problems[key] ?? ["no usable reply"]).join("; ")}`)
        }
      }
    }
    report.translated[locale] = newPairs.length
    if (opts.dryRun) continue

    for (const [ns, strings] of Object.entries(source)) {
      const file = join(localesDir, locale, `${ns}.json`)
      const have = flatten(readJson(file, {}))
      const next = {}
      for (const key of Object.keys(strings)) {
        if (outputs[ns]?.[key] !== undefined) next[key] = outputs[ns][key]
        else if (key in have && !opts.force && lockFor[key] === hashSource(strings[key])) next[key] = have[key]
        // else: changed or failed: left out, so the UI falls back to English until it is translated.
      }
      const nested = nestLike(shapes[ns], next)
      const before = existsSync(file) ? readFileSync(file, "utf8") : null
      const after = JSON.stringify(nested, null, 2) + "\n"
      if (before !== after) { mkdirSync(dirname(file), { recursive: true }); writeFileSync(file, after); touched = true }
    }
    // Namespaces that no longer exist in English.
    const dir = join(localesDir, locale)
    if (existsSync(dir)) for (const f of readdirSync(dir)) if (f.endsWith(".json") && !(f.replace(/\.json$/, "") in source)) { rmSync(join(dir, f)); touched = true }

    saveLock(lock)
    if (opts.back) report.drift.push(...(await backTranslate(locale, newPairs, opts)))
  }

  if (!opts.dryRun) saveLock(lock)
  const md = renderReport(report, opts)
  if (opts.report) writeFileSync(opts.report, md)
  console.log(md)
  if (report.failures.length) {
    console.error(`${report.failures.length} strings failed validation after ${RETRIES} tries; they stay English until retried.`)
    process.exitCode = 2
  }
  if (!touched && !opts.dryRun) console.log("No file changes.")
}

function renderReport(r, opts) {
  const total = Object.values(r.translated).reduce((a, b) => a + b, 0)
  const lines = ["## Translation summary", ""]
  lines.push(`Strings translated: ${total} (${Object.entries(r.translated).map(([l, n]) => `${l} ${n}`).join(", ") || "none"})`)
  const removed = Object.values(r.removed).reduce((a, b) => a + b.length, 0)
  if (removed) lines.push(`Keys removed: ${removed} per language set (${Object.keys(r.removed).length} languages)`)
  lines.push("", "Checks run on every string: valid ICU syntax, the same variables, tags and select keywords as English, complete plural categories, product names kept, no empty values.")
  lines.push("", `### Length warnings (over 1.5x the English)`, "")
  lines.push(...(r.warnings.length ? r.warnings.map((w) => `- ${w}`) : ["None."]))
  lines.push("", "### Possible meaning drift (back-translation review)", "")
  if (!opts.back) lines.push("Skipped.")
  else if (!r.drift.length) lines.push("None flagged.")
  else for (const d of r.drift) lines.push(`- ${d.locale} \`${d.key}\`: "${d.en}" became "${d.tr}", which reads back as "${d.back}". ${d.note ?? ""}`)
  if (r.failures.length) lines.push("", "### Failed validation (left in English)", "", ...r.failures.map((f) => `- ${f}`))
  lines.push("", "Machine translation by Claude Sonnet. Spot-check one launch language with a native speaker before it is published.")
  return lines.join("\n") + "\n"
}

main().catch((e) => {
  console.error(e.message)
  process.exit(1)
})
