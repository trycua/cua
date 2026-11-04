// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Checks shared by the translation script and the vitest guard: ICU syntax,
 * the same variables, tags and select keywords as English, complete CLDR
 * plural categories, kept product names, and a length warning.
 */
import { createHash } from "node:crypto"
import { IntlMessageFormat } from "intl-messageformat"

/** Target languages, in the order they are generated. */
export const TARGET_LOCALES = ["zh-Hans", "ko", "pt-BR", "ja", "es"]

/** Longer than this many times the English is flagged (warning, not blocking). */
export const LENGTH_RATIO_WARN = 1.5
/** Strings shorter than this are too small for a ratio to mean anything. */
export const LENGTH_MIN_CHARS = 12

/** Short stable hash of an English string, for change detection. */
export function hashSource(text) {
  return createHash("sha256").update(text).digest("hex").slice(0, 12)
}

// intl-messageformat AST element types.
const T = { literal: 0, argument: 1, number: 2, date: 3, time: 4, select: 5, plural: 6, pound: 7, tag: 8 }

/**
 * Parse an ICU message. Returns its structure: the variables used (with the
 * kind), tags, select keywords and plural categories, or `{ error }`.
 */
export function analyze(message) {
  let ast
  try {
    ast = new IntlMessageFormat(message, "en").getAst()
  } catch (e) {
    return { error: String(e?.message ?? e).split("\n")[0] }
  }
  const vars = new Map() // name -> kind
  const tags = new Set()
  const selects = new Map() // name -> sorted option keys
  const plurals = new Map() // name -> sorted option keys
  const walk = (els) => {
    for (const el of els) {
      switch (el.type) {
        case T.argument: vars.set(el.value, vars.get(el.value) ?? "arg"); break
        case T.number: vars.set(el.value, "number"); break
        case T.date: vars.set(el.value, "date"); break
        case T.time: vars.set(el.value, "time"); break
        case T.select:
        case T.plural: {
          vars.set(el.value, el.type === T.select ? "select" : "plural")
          const keys = Object.keys(el.options).sort()
          ;(el.type === T.select ? selects : plurals).set(el.value, keys)
          for (const opt of Object.values(el.options)) walk(opt.value)
          break
        }
        case T.tag: tags.add(el.value); walk(el.children); break
        default: break
      }
    }
  }
  walk(ast)
  return { vars, tags, selects, plurals }
}

const sameSet = (a, b) => a.size === b.size && [...a].every((x) => b.has(x))

/** CLDR plural categories a language needs in a `plural` block. */
export function pluralCategories(locale) {
  return new Intl.PluralRules(locale).resolvedOptions().pluralCategories
}

/**
 * Whole-word, case-sensitive match so `Pro` does not match `Product`. Only
 * Latin letters and digits count as word characters, so `Cua는` (a Korean
 * particle) and `Spaces를` still match.
 */
function hasTerm(text, term) {
  const esc = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
  return new RegExp(`(?<![A-Za-z0-9])${esc}(?![A-Za-z0-9])`, "u").test(text)
}

/**
 * Validate one translation. Returns `{ errors, warnings }`; any error blocks.
 * `doNotTranslate` terms present in the English must survive verbatim.
 */
export function validateTranslation({ key, source, translation, locale, doNotTranslate = [], forcedTerms = {} }) {
  const errors = []
  const warnings = []
  if (typeof translation !== "string" || translation.trim() === "") {
    return { errors: [`${key}: empty translation`], warnings }
  }
  const src = analyze(source)
  if (src.error) return { errors: [`${key}: English source does not parse (${src.error})`], warnings }
  const out = analyze(translation)
  if (out.error) return { errors: [`${key}: ICU syntax error (${out.error})`], warnings }

  const names = (m) => new Set(m.keys())
  if (!sameSet(names(src.vars), names(out.vars))) {
    const miss = [...src.vars.keys()].filter((v) => !out.vars.has(v))
    const extra = [...out.vars.keys()].filter((v) => !src.vars.has(v))
    errors.push(`${key}: variables differ (missing: ${miss.join(", ") || "none"}; extra: ${extra.join(", ") || "none"})`)
  } else {
    for (const [name, kind] of src.vars) {
      if (out.vars.get(name) !== kind) errors.push(`${key}: variable {${name}} changed type (${kind} to ${out.vars.get(name)})`)
    }
  }
  if (!sameSet(src.tags, out.tags)) errors.push(`${key}: tags differ (${[...src.tags].join(", ") || "none"} vs ${[...out.tags].join(", ") || "none"})`)
  for (const [name, keys] of src.selects) {
    if (JSON.stringify(out.selects.get(name) ?? []) !== JSON.stringify(keys)) errors.push(`${key}: select {${name}} keywords must stay ${keys.join(", ")}`)
  }
  const cats = pluralCategories(locale)
  for (const [name] of src.plurals) {
    const have = out.plurals.get(name)
    if (!have) continue // already reported as a variable-kind change
    const lacking = ["other", ...cats].filter((c) => !have.includes(c))
    if (lacking.length) errors.push(`${key}: plural {${name}} lacks ${[...new Set(lacking)].join(", ")} for ${locale}`)
  }
  for (const term of doNotTranslate) {
    if (hasTerm(source, term) && !hasTerm(translation, term)) errors.push(`${key}: "${term}" must stay in English`)
  }
  for (const [term, wanted] of Object.entries(forcedTerms)) {
    if (hasTerm(source.toLowerCase(), term.toLowerCase()) && !translation.includes(wanted)) {
      warnings.push(`${key}: expected "${wanted}" for "${term}"`)
    }
  }
  if (source.length >= LENGTH_MIN_CHARS && translation.length > source.length * LENGTH_RATIO_WARN) {
    warnings.push(`${key}: ${translation.length} chars is ${(translation.length / source.length).toFixed(1)}x the English (${source.length})`)
  }
  return { errors, warnings }
}
