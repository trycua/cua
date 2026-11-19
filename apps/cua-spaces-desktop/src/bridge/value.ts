// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The core's views (UniFFI records and enums, as the generated Node bindings
// give them) as JSON the page reads: a port of the SwiftUI host's
// `BridgeValue.encode`, so both native hosts answer the same shapes.
//
// Records become objects with their camelCase field names (a missing
// optional is `null`, as Swift's `nil`); an enum without a payload is its
// case name (`"touchId"`: `pnpm native` gives flat enums their Swift case
// names as values; so is a payload-less case of an enum with payloads); one
// with a payload is `{type: "<case>", ...fields}` (an
// unlabeled payload is `value`, `value1`, ...). 64-bit integers become
// numbers, bytes base64, dates milliseconds, maps objects. Objects (UniFFI
// handles) are left out as `null`: they are live handles, not data.

const TYPE_NAME = Symbol.for("typeName");
const POINTER = Symbol.for("pointer");
const DESTRUCTOR = Symbol.for("destructor");

/** A Rust variant's name as Swift spells the case (heck's lowerCamelCase). */
export function caseName(variant: string): string {
  const words = variant.match(/[A-Z]+(?![a-z])|[A-Z]?[a-z0-9]+|[0-9]+/g) ?? [variant];
  return words.map((w, i) => (i === 0 ? w.toLowerCase() : w[0]!.toUpperCase() + w.slice(1).toLowerCase())).join("");
}

function isHandle(v: object): boolean {
  return POINTER in v || DESTRUCTOR in v || typeof (v as { uniffiDestroy?: unknown }).uniffiDestroy === "function";
}

function isTaggedEnum(v: object): v is { tag: string; inner?: unknown } {
  return TYPE_NAME in v && typeof (v as { tag?: unknown }).tag === "string";
}

const base64 = (bytes: Uint8Array) => Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString("base64");

export function encode(value: unknown): unknown {
  if (value === null || value === undefined) return null;
  switch (typeof value) {
    case "string":
    case "boolean":
      return value;
    case "number":
      return Number.isFinite(value) ? value : null;
    case "bigint":
      return Number(value);
    case "function":
    case "symbol":
      return null;
  }
  if (value instanceof Uint8Array) return base64(value);
  if (value instanceof ArrayBuffer) return base64(new Uint8Array(value));
  if (value instanceof Date) return value.getTime();
  if (value instanceof URL) return value.href;
  if (Array.isArray(value)) return value.map(encode);
  if (value instanceof Map) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of value) out[String(k)] = encode(v);
    return out;
  }
  if (value instanceof Set) return [...value].map(encode);
  const object = value as object;
  if (isTaggedEnum(object)) {
    // A case without a payload is its name, as Swift's `.idle` is.
    if (!("inner" in object)) return caseName(object.tag);
    const out: Record<string, unknown> = { type: caseName(object.tag) };
    const inner = object.inner;
    if (Array.isArray(inner)) {
      inner.forEach((v, i) => (out[i === 0 ? "value" : `value${i}`] = encode(v)));
    } else if (inner && typeof inner === "object") {
      for (const [k, v] of Object.entries(inner)) out[k] = encode(v);
    }
    return out;
  }
  if (isHandle(object)) return null;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(object)) out[k] = encode(v);
  return out;
}
