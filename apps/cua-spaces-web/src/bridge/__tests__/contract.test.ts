// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The bridge's answers have the shapes the page reads (`contracts/shapes.ts`).
 *
 * - `bridge-shapes.json` is the shapes as exported for the Swift test
 *   (`pnpm contract:shapes` rewrites it).
 * - Every SwiftUI method has a shape or says why not.
 * - The demo host answers every operation in its shape, on every platform
 *   it plays and in the SwiftUI host's New Space shape (`?demo=mac-host`).
 *
 * With `CUA_BRIDGE_ANSWERS=<path>` (the Swift test's dump of its answers,
 * `pnpm contract:swift`), the SwiftUI host's real answers also go through
 * the webkit adapter, and what the page gets must have its shape.
 *
 * The Swift host's own answers are checked in
 * `apps/cua-spaces-macos/Tests/CuaSpacesMacTests/BridgeContractTests.swift`;
 * the Electron host's in `apps/cua-spaces-desktop/test/bridge-native.test.ts`.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { createDemoAdapter, type DemoOptions } from "../adapters/demo";
import { createWebkitAdapter } from "../adapters/webkit";
import { HOST_COVERAGE } from "../coverage";
import type { HostWindow } from "../detect";
import { demoPlatformOf } from "../adapters/demo/platform";
import { validate, type Schema } from "../contracts/schema";
import { OP_SHAPES, SHAPE_DEFS, WEBKIT_SHAPES, WEBKIT_UNSHAPED, bridgeShapesDocument } from "../contracts/shapes";
import { OPERATIONS } from "../protocol";
import { WEBKIT_METHODS, type WebkitRequest } from "../webkit-protocol";
import { DEMO_ARGS, DEMO_REFUSES } from "./contract-args";

const DOC_PATH = resolve(process.cwd(), "src/bridge/contracts/bridge-shapes.json");

function refs(s: unknown, out: Set<string> = new Set()): Set<string> {
  if (Array.isArray(s)) s.forEach((x) => refs(x, out));
  else if (s && typeof s === "object") {
    for (const [k, v] of Object.entries(s)) {
      if (k === "$ref" && typeof v === "string") out.add(v.replace("#/$defs/", ""));
      else refs(v, out);
    }
  }
  return out;
}

describe("bridge shapes", () => {
  it("bridge-shapes.json is up to date (pnpm contract:shapes)", () => {
    const want = `${JSON.stringify(bridgeShapesDocument(), null, 2)}\n`;
    if (process.env.UPDATE_BRIDGE_SHAPES) writeFileSync(DOC_PATH, want);
    expect(readFileSync(DOC_PATH, "utf8")).toBe(want);
  });

  it("names only shapes it defines", () => {
    const missing = [...refs(bridgeShapesDocument())].filter((n) => !SHAPE_DEFS[n]);
    expect(missing).toEqual([]);
  });

  it("has a shape for every operation", () => {
    expect(Object.keys(OP_SHAPES).sort()).toEqual([...OPERATIONS].sort());
  });

  it("has a shape, or a reason, for every SwiftUI method and no other", () => {
    const shaped = Object.keys(WEBKIT_SHAPES);
    const unshaped = Object.keys(WEBKIT_UNSHAPED);
    expect(shaped.filter((m) => unshaped.includes(m)), "both shaped and unshaped").toEqual([]);
    expect([...shaped, ...unshaped].sort()).toEqual([...WEBKIT_METHODS].sort());
  });

  it("says what is wrong, and where", () => {
    const s: Schema = OP_SHAPES["spaces.createOptions"];
    expect(validate(s, { local: null, gpus: null, cloudPricing: null, experiments: {}, maxCpus: 8 }, SHAPE_DEFS)).toEqual([]);
    // An answer the wizard can't run on, against what the SwiftUI host must answer.
    const swift = WEBKIT_SHAPES["spaces.createOptions"]!;
    expect(validate(swift, { local: null, gpus: null, cloudPricing: null, experiments: {}, maxCpus: 8 }, SHAPE_DEFS)).toEqual([
      "$.env: missing",
      "$.local: expected object, got null",
    ]);
    expect(validate(OP_SHAPES["spaces.list"], [{ id: "a", name: "A", provider: "moon" }], SHAPE_DEFS)).toEqual([
      "$[0].spacesdVersion: missing",
      "$[0].features: missing",
      "$[0].reachable: missing",
      '$[0].provider: "moon" is not one of "cloud", "local", "direct", "relay"',
    ]);
  });
});

/** `strict`: every operation answers (on the Mac's sample data, which
 * `DEMO_ARGS` names); otherwise a refusal is fine, but an answer must still
 * have its shape. */
const VARIANTS: [string, DemoOptions, boolean][] = [
  ["Mac", {}, true],
  ["mac-host", { macHost: true }, true],
  ["Windows", { platform: demoPlatformOf("win32", "x64") }, false],
  ["Linux", { platform: demoPlatformOf("linux", "arm64") }, false],
  ["fresh, empty, locked", { signedIn: false, onboarded: false, noSpaces: true, keyvaultLocked: true }, false],
];

describe("the demo host answers in the contract's shapes", () => {
  for (const [name, options, strict] of VARIANTS) {
    it(name, async () => {
      const problems: string[] = [];
      const refused: string[] = [];
      for (const op of OPERATIONS) {
        const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0, ...options });
        try {
          const result = await demo.call(op, DEMO_ARGS[op] as never);
          problems.push(...validate(OP_SHAPES[op], result, SHAPE_DEFS).map((e) => `${op} ${e}`));
        } catch (e) {
          if (DEMO_REFUSES[op]) refused.push(op);
          else if (strict) problems.push(`${op} failed: ${(e as Error).message}`);
        } finally {
          demo.dispose?.();
        }
      }
      expect(problems).toEqual([]);
      expect(refused.sort()).toEqual(Object.keys(DEMO_REFUSES).sort());
    });
  }
});

const ANSWERS = process.env.CUA_BRIDGE_ANSWERS;

describe.skipIf(!ANSWERS)("the SwiftUI host's answers, through the webkit adapter", () => {
  it("reach the page in the contract's shapes", async () => {
    const answers = JSON.parse(readFileSync(ANSWERS!, "utf8")) as Record<string, unknown>;
    const win = {
      webkit: {
        messageHandlers: {
          cua: {
            postMessage: (m: unknown) => {
              const req = m as WebkitRequest;
              return Promise.resolve(
                req.method in answers
                  ? { id: req.id, ok: true, result: answers[req.method] }
                  : { id: req.id, ok: false, error: { code: "unimplemented", message: `${req.method} not in the dump` } },
              );
            },
          },
        },
      },
      addEventListener: () => {},
      removeEventListener: () => {},
      open: () => null,
    } as unknown as HostWindow;
    const adapter = createWebkitAdapter(win);
    const problems: string[] = [];
    const checked: string[] = [];
    for (const op of OPERATIONS) {
      const methods = HOST_COVERAGE[op].webkit.methods as readonly string[];
      if (methods.length === 0 || !methods.every((m) => m in answers)) continue;
      try {
        const result = await adapter.call(op, DEMO_ARGS[op] as never);
        problems.push(...validate(OP_SHAPES[op], result, SHAPE_DEFS).map((e) => `${op} ${e}`));
        checked.push(op);
      } catch (e) {
        problems.push(`${op} failed: ${(e as Error).message}`);
      }
    }
    adapter.dispose?.();
    expect(problems).toEqual([]);
    expect(checked.length).toBeGreaterThan(40);
  });
});
