// @vitest-environment node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Drift guard for the design language (CUA-2298, src/lib/design-lint.ts).
 *
 * Every .tsx under components/ and routes/ is scanned. A file may not have
 * more findings of a rule than design-baseline.json records for it, and a
 * file the baseline doesn't name may have none. Fixing a page lowers its
 * count: refresh the baseline with
 *   UPDATE_DESIGN_BASELINE=1 pnpm vitest run src/lib/design-lint.test.ts
 * A line that truly needs a full-width hairline (a page-wide header rule, a
 * sheet's footer) carries `design-ok: <why>` on it or the line above.
 */
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { countByRule, scanSettings, scanSource, type DesignRule } from "./design-lint";

const src = fileURLToPath(new URL("..", import.meta.url));
const baselinePath = fileURLToPath(new URL("./design-baseline.json", import.meta.url));

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return name === "__tests__" ? [] : walk(p);
    return p.endsWith(".tsx") && !p.endsWith(".test.tsx") ? [p] : [];
  });
}

function scanAll(): Record<string, Partial<Record<DesignRule, number>>> {
  const out: Record<string, Partial<Record<DesignRule, number>>> = {};
  for (const file of [...walk(join(src, "components")), ...walk(join(src, "routes"))].sort()) {
    const rel = relative(src, file).split("\\").join("/");
    const counts = countByRule(scanSource(readFileSync(file, "utf8"), { primitive: rel.startsWith("components/ui/") }));
    if (Object.keys(counts).length) out[rel] = counts;
  }
  return out;
}

describe("design language drift guard", () => {
  it("no page gains a partial line, raw radius, pill, outlined box or dot-chain", () => {
    const now = scanAll();
    if (process.env.UPDATE_DESIGN_BASELINE) {
      writeFileSync(baselinePath, `${JSON.stringify(now, null, 2)}\n`);
      return;
    }
    const baseline = JSON.parse(readFileSync(baselinePath, "utf8")) as Record<string, Partial<Record<DesignRule, number>>>;
    const worse: string[] = [];
    for (const [file, counts] of Object.entries(now)) {
      for (const [rule, n] of Object.entries(counts) as [DesignRule, number][]) {
        const allowed = baseline[file]?.[rule] ?? 0;
        if (n > allowed) {
          const lines = scanSource(readFileSync(join(src, file), "utf8"), { primitive: file.startsWith("components/ui/") })
            .filter((f) => f.rule === rule)
            .map((f) => `    ${file}:${f.line}  ${f.text}`);
          worse.push(`${file}: ${rule} ${n} (allowed ${allowed})\n${lines.join("\n")}`);
        }
      }
    }
    expect(worse, "Use GroupedSection / PlainList / SectionHeading (components/ui/grouped.tsx) and rounded-lg; see the header of design-lint.test.ts").toEqual([]);
  });

  it("the baseline only names files that still have findings (counts only go down)", () => {
    const now = scanAll();
    const baseline = JSON.parse(readFileSync(baselinePath, "utf8")) as Record<string, Partial<Record<DesignRule, number>>>;
    const stale = Object.entries(baseline).flatMap(([file, counts]) =>
      (Object.entries(counts) as [DesignRule, number][]).filter(([rule, n]) => (now[file]?.[rule] ?? 0) < n).map(([rule, n]) => `${file}: ${rule} ${now[file]?.[rule] ?? 0} < ${n}`),
    );
    expect(stale, "A page got better: refresh design-baseline.json (UPDATE_DESIGN_BASELINE=1)").toEqual([]);
  });
});

/** Where Settings tabs live: they share one spacing, heading and button style. */
const settingsFiles = () => [...walk(join(src, "components/settings")), ...walk(join(src, "routes/settings")), join(src, "components/settings-group.tsx"), join(src, "components/volume/driver-card.tsx")];

describe("Settings pages", () => {
  it("get their spacing, headings and row buttons from the shared components", () => {
    const found = settingsFiles().flatMap((file) => scanSettings(readFileSync(file, "utf8")).map((f) => `${relative(src, file)}:${f.line}  ${f.rule}  ${f.text}`));
    expect(found, "Use SettingsGroup (title, footer), GroupedAction and outline buttons; see the header of design-lint.ts").toEqual([]);
  });

  it("keep the Agents surfaces' row buttons outline too", () => {
    const found = [...walk(join(src, "components/agents")), ...walk(join(src, "routes/agents"))].flatMap((file) =>
      scanSettings(readFileSync(file, "utf8"), { rowButtonsOnly: true }).map((f) => `${relative(src, file)}:${f.line}  ${f.text}`),
    );
    expect(found, "Connect, Sign in and other small row buttons are variant=\"outline\"").toEqual([]);
    expect(scanSettings(`<Button size="sm" variant="secondary">Connect</Button>`, { rowButtonsOnly: true }).map((f) => f.rule)).toEqual(["row-button"]);
    expect(scanSettings(`<Button variant="secondary">Cancel</Button>`, { rowButtonsOnly: true })).toEqual([]);
  });

  it("flags a pulled-up link, a hand-made heading and a filled secondary row button", () => {
    const rules = (s: string) => scanSettings(s).map((f) => f.rule);
    expect(rules(`<button className="-mt-4 px-1 text-[13px]" />`)).toEqual(["settings-spacing"]);
    expect(rules(`<h2 className="mb-2 text-xs">Account keys</h2>`)).toEqual(["settings-heading"]);
    expect(rules(`<Button size="sm" variant="secondary">Reset</Button>`)).toEqual(["row-button"]);
    expect(rules(`<Button size="sm" variant="outline">Reset</Button>`)).toEqual([]);
  });
});

describe("scanSource", () => {
  const rules = (s: string, primitive = false) => scanSource(s, { primitive }).map((f) => f.rule);
  it("flags raw radii and accepts rounded-lg", () => {
    expect(rules(`<div className="rounded-md p-2" />`)).toEqual(["radius"]);
    expect(rules(`<div className="rounded-xl p-2" />`)).toEqual(["radius"]);
    expect(rules(`<div className="rounded-[11px] p-2" />`)).toEqual(["radius"]);
    expect(rules(`<div className="rounded p-2" />`)).toEqual(["radius"]);
    expect(rules(`<div className="rounded-lg p-2" />`)).toEqual([]);
  });
  it("flags pills, not dots or avatars", () => {
    expect(rules(`<span className="rounded-full bg-muted px-2 text-xs" />`)).toEqual(["pill"]);
    expect(rules(`<span className="size-2 rounded-full bg-success" />`)).toEqual([]);
  });
  it("flags partial lines and divide-*, except in primitives or with design-ok", () => {
    expect(rules(`<div className="border-t py-2" />`)).toEqual(["partial-line"]);
    expect(rules(`<div className="divide-y" />`)).toEqual(["partial-line"]);
    expect(rules(`<div className="border-t py-2" />`, true)).toEqual([]);
    expect(rules(`// design-ok: the header hairline spans the page\n<div className="border-b py-2" />`)).toEqual([]);
  });
  it("flags outlined boxes", () => {
    expect(rules(`<div className="rounded-lg border bg-card p-4" />`)).toEqual(["outlined-box"]);
    expect(rules(`<div className="rounded-lg bg-card p-4" />`)).toEqual([]);
  });
  it("flags dot-chains in copy", () => {
    expect(rules(`const line = "1 Space running · Provides Spaces";`)).toEqual(["dot-chain"]);
    expect(rules(`const line = parts.join(" · ");`)).toEqual(["dot-chain"]);
    expect(rules(`const line = "Running";`)).toEqual([]);
  });
});
