// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Provider marks ship with the app as static files. The app never asks an icon host for one, and every
// provider is either drawn from a bundled mark or is named here as one that has none (it gets a letter tile).

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { APP_MARKS } from "@/components/agents/harness-mark";
import { PROVIDER_REGISTRY } from "@/bridge/generated/providers";

const root = resolve(__dirname, "../..");
const src = join(root, "src");

/** The visible providers with no brand mark to use: a clean letter tile. */
const NO_MARK = ["custom"];

function files(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name === "node_modules" || name === "wasm") continue;
      files(p, out);
    } else if (/\.(tsx?|css|html)$/.test(name)) out.push(p);
  }
  return out;
}

describe("provider marks", () => {
  it("every provider is drawn from a bundled mark, except the ones listed here", () => {
    const missing = PROVIDER_REGISTRY.filter((p) => !(`provider:${p.id}` in APP_MARKS) && !["anthropic", "openai", "google", "github"].includes(p.id)).map((p) => p.id);
    expect(missing).toEqual(NO_MARK);
  });

  it("every mark is a file in public/, in a light and a dark ink where it has both", () => {
    for (const [id, m] of Object.entries(APP_MARKS)) {
      for (const f of new Set([m.light, m.dark])) {
        expect(f.startsWith("/"), `${id}: ${f}`).toBe(true);
        expect(existsSync(join(root, "public", f)), `${id}: ${f}`).toBe(true);
      }
    }
  });

  it("no mark is fetched from another host: a bundled path only", () => {
    for (const [id, m] of Object.entries(APP_MARKS)) {
      for (const f of [m.light, m.dark]) expect(f, id).not.toMatch(/^[a-z]+:|^\/\//i);
    }
  });

  it("no component, style or page names an icon host or a favicon service", () => {
    const hosts = /(?:s2\/favicons|faviconV2|favicon\.ico\?|gstatic\.com\/faviconV2|google\.com\/s2|simpleicons\.org|cdn\.simpleicons|lobehub|lobe-icons|logo\.clearbit|icons\.duckduckgo|cdn\.jsdelivr\.net\/npm\/@lobehub|unpkg\.com\/@lobehub|iconify\.design)/i;
    const imageUrl = /(?:src|href|url\()\s*=?\s*[\("'`]\s*https?:\/\/[^"'`)\s]+\.(?:svg|png|ico|jpe?g|webp)/i;
    const bad: string[] = [];
    for (const f of files(src)) {
      if (/\.test\.|__tests__|\/test\//.test(f)) continue;
      const text = readFileSync(f, "utf8");
      if (hosts.test(text) || imageUrl.test(text)) bad.push(f.slice(root.length + 1));
    }
    expect(bad).toEqual([]);
  });

  it("the page that loads the app draws no mark from a host either", () => {
    expect(readFileSync(join(root, "index.html"), "utf8")).not.toMatch(/https?:\/\/[^"']+\.(?:svg|png|ico)/i);
  });
});
