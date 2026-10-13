// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A small JSON Schema subset for the bridge's answers (`shapes.ts`), with
 * builders that TypeScript checks against the contract types and a
 * validator. The same subset is validated in Swift
 * (`apps/cua-spaces-macos/Tests/CuaSpacesMacTests/BridgeContractTests.swift`)
 * from the exported document (`bridge-shapes.json`).
 *
 * Keywords: `type` (a name or a list), `properties`, `required`,
 * `additionalProperties` (a schema), `items`, `enum`, `anyOf` and `$ref`
 * (`#/$defs/<name>`). Unknown fields are allowed: a host may answer more
 * than the page reads.
 */

export type JsonType = "string" | "number" | "integer" | "boolean" | "null" | "object" | "array";

export interface Schema {
  type?: JsonType | JsonType[];
  properties?: Record<string, Schema>;
  required?: string[];
  additionalProperties?: Schema;
  items?: Schema;
  enum?: (string | number | boolean | null)[];
  anyOf?: Schema[];
  $ref?: string;
  description?: string;
}

/* ---- Builders ------------------------------------------------------------ */

export const str: Schema = { type: "string" };
export const num: Schema = { type: "number" };
export const bool: Schema = { type: "boolean" };
export const nul: Schema = { type: "null" };
/** Anything (a part the page passes on without reading). */
export const any: Schema = {};

export const list = (items: Schema): Schema => ({ type: "array", items });
export const dict = (values: Schema): Schema => ({ type: "object", additionalProperties: values });
export const oneOf = <T extends string | number | boolean>(...values: T[]): Schema => ({ enum: values });
export const union = (...schemas: Schema[]): Schema => ({ anyOf: schemas });

/** `s` or null. */
export function nullable(s: Schema): Schema {
  if (s.enum) return { ...s, enum: [...s.enum, null] };
  if (typeof s.type === "string" && !s.anyOf && !s.$ref) return { ...s, type: [s.type, "null"] };
  return { anyOf: [s, nul] };
}

const OPTIONAL = Symbol("optional");
/** A property that may be absent. */
export interface Optional {
  readonly [OPTIONAL]: Schema;
}
export const opt = (s: Schema): Optional => ({ [OPTIONAL]: s });
/** May be absent or null. */
export const optNull = (s: Schema): Optional => opt(nullable(s));

type Prop = Schema | Optional;
const isOptional = (p: Prop): p is Optional => OPTIONAL in p;

/** An object with these properties (`opt(...)`: may be absent). */
export function object(props: Record<string, Prop>): Schema {
  const properties: Record<string, Schema> = {};
  const required: string[] = [];
  for (const [k, p] of Object.entries(props)) {
    if (isOptional(p)) properties[k] = p[OPTIONAL];
    else {
      properties[k] = p;
      required.push(k);
    }
  }
  return required.length ? { type: "object", properties, required } : { type: "object", properties };
}

/** Every key of `T`, each optional one as `opt(...)`: a field added to the
 * contract type fails to compile until its shape says what it is. */
export type Props<T> = { [K in keyof T]-?: undefined extends T[K] ? Optional : Schema };

/** `object` checked against the contract type `T`. */
export const obj =
  <T>() =>
  (props: Props<T>): Schema =>
    object(props as Record<string, Prop>);

/* ---- Named shapes ---------------------------------------------------------- */

/** A named shape (`#/$defs/<name>` in the exported document). */
export class Defs {
  readonly all: Record<string, Schema> = {};
  ref(name: string, schema: Schema): Schema {
    if (this.all[name] && JSON.stringify(this.all[name]) !== JSON.stringify(schema)) throw new Error(`shape ${name} defined twice`);
    this.all[name] = schema;
    return { $ref: `#/$defs/${name}` };
  }
}

/* ---- Validation ------------------------------------------------------------ */

const typeOf = (v: unknown): JsonType =>
  v === null ? "null" : Array.isArray(v) ? "array" : typeof v === "number" ? "number" : (typeof v as JsonType);

function matchesType(t: JsonType, v: unknown): boolean {
  if (t === "integer") return typeof v === "number" && Number.isInteger(v);
  if (t === "number") return typeof v === "number" && Number.isFinite(v);
  return typeOf(v) === t;
}

/** What is wrong with `value` for `schema` (empty: it matches). `path`
 * names where, as `$.local.backends[0]`. */
export function validate(schema: Schema, value: unknown, defs: Record<string, Schema> = {}, path = "$"): string[] {
  if (schema.$ref) {
    const name = schema.$ref.replace(/^#\/\$defs\//, "");
    const target = defs[name];
    if (!target) return [`${path}: unknown shape ${schema.$ref}`];
    return validate(target, value, defs, path);
  }
  if (schema.anyOf) {
    const each = schema.anyOf.map((s) => validate(s, value, defs, path));
    if (each.some((e) => e.length === 0)) return [];
    // The closest alternative's errors.
    return each.sort((a, b) => a.length - b.length)[0] ?? [`${path}: matches no alternative`];
  }
  if (schema.enum && !schema.enum.some((e) => e === value)) {
    return [`${path}: ${JSON.stringify(value)} is not one of ${schema.enum.map((e) => JSON.stringify(e)).join(", ")}`];
  }
  if (schema.type) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((t) => matchesType(t, value))) return [`${path}: expected ${types.join(" or ")}, got ${value === undefined ? "nothing" : typeOf(value)}`];
  }
  const errors: string[] = [];
  if (Array.isArray(value) && schema.items) {
    value.forEach((x, i) => errors.push(...validate(schema.items!, x, defs, `${path}[${i}]`)));
  } else if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    const o = value as Record<string, unknown>;
    for (const k of schema.required ?? []) if (o[k] === undefined) errors.push(`${path}.${k}: missing`);
    for (const [k, s] of Object.entries(schema.properties ?? {})) {
      if (o[k] !== undefined) errors.push(...validate(s, o[k], defs, `${path}.${k}`));
    }
    if (schema.additionalProperties) {
      for (const [k, x] of Object.entries(o)) {
        if (!schema.properties?.[k] && x !== undefined) errors.push(...validate(schema.additionalProperties, x, defs, `${path}.${k}`));
      }
    }
  }
  return errors;
}
