// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space page header in the call layout: back on its own row, the power
// icon (green while on, gray while off, a spinner while it changes), the
// pop-out and "⋯" (Delete), then Teleport an app and Share as pills. Share
// is plain: no caret, and "Link to a port" is gone from the page.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { SpaceHeader, type HeaderPower } from "@/components/space-detail/space-header";
import { TooltipProvider } from "@/components/ui/tooltip";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;

function header(power: HeaderPower) {
  return render(
    <TooltipProvider>
      <SpaceHeader back={<a href="/spaces">Spaces</a>} os="linux" title="ubuntu-build" place={<p>Linux box</p>} power={power} />
    </TooltipProvider>,
  );
}

describe("the power icon", () => {
  it("on: green, and its label turns it off", () => {
    const onClick = vi.fn();
    header({ state: "on", label: "Turn off", onClick });
    const button = screen.getByRole("button", { name: "Turn off" });
    expect(button.dataset.headerPower).toBe("on");
    expect(button.className).toContain("text-power-on");
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("off: gray, and its label turns it on", () => {
    const onClick = vi.fn();
    header({ state: "off", label: "Turn on", onClick });
    const button = screen.getByRole("button", { name: "Turn on" });
    expect(button.dataset.headerPower).toBe("off");
    expect(button.className).not.toContain("text-power-on");
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("pending: says what is happening, spins, and can't be pressed again", () => {
    const onClick = vi.fn();
    header({ state: "pending", label: "Starting…", onClick });
    const button = screen.getByRole("button", { name: "Starting…" });
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.querySelector(".animate-spin")).not.toBeNull();
    expect(button.className).not.toContain("text-power-on");
    fireEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe("the Space page header", () => {
  it("puts back on its own row, then power, pop-out and ⋯ before the Teleport and Share pills", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-call-header] [data-header-power]")).not.toBeNull(), { timeout: 4000 });
    const head = q("[data-call-header]")!;
    // "‹ Spaces" is on a row of its own, above the title's row.
    const back = within(head).getByRole("link", { name: /Spaces/ });
    expect(back.closest("div")!.contains(head.querySelector("[data-detail-title]"))).toBe(false);
    const power = within(head).getByRole("button", { name: "Turn off" });
    expect(power.dataset.headerPower).toBe("on");
    const buttons = within(head).getAllByRole("button");
    const at = (name: RegExp) => buttons.findIndex((b) => name.test((b.getAttribute("aria-label") ?? b.textContent ?? "").trim()));
    expect(at(/^Turn off$/)).toBe(0);
    expect(at(/^More actions$/)).toBeLessThan(at(/Teleport an app/));
    expect(at(/Teleport an app/)).toBeLessThan(at(/^Share$/));
    // Share is one plain pill; Delete lives in ⋯.
    expect(within(head).queryByRole("button", { name: "More ways to share" })).toBeNull();
    expect(within(head).queryByRole("button", { name: /Delete/ })).toBeNull();
    fireEvent.click(within(head).getByRole("button", { name: "More actions" }));
    expect(await screen.findByRole("menuitem", { name: /^Delete Space/ })).toBeTruthy();
  });

  it("turning it off goes through the confirm, then the icon is gray and turns it on", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const power = await screen.findByRole("button", { name: "Turn off" }, { timeout: 4000 });
    fireEvent.click(power);
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(q("[data-call-header] [data-header-power]")?.dataset.headerPower).toBe("off"), { timeout: 8000 });
    expect(q("[data-call-header] [data-header-power]")?.getAttribute("aria-label")).toBe("Turn on");
  });

  it("has no Link to a port, in the header or in Details' More", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-call-more]")).not.toBeNull(), { timeout: 4000 });
    fireEvent.click(q("[data-call-more]")!);
    await waitFor(() => expect(q("[data-call-more-sheet]")).not.toBeNull());
    expect(document.body.textContent).not.toMatch(/Link to a port/i);
  });
});

describe("the call layout's surfaces", () => {
  it("one radius and no lines that stop short: no larger radii, no rules between rail sections, a filled composer", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-members-panel]")).not.toBeNull(), { timeout: 4000 });
    const layout = q("[data-call-layout]")!;
    const all = [layout, ...layout.querySelectorAll<HTMLElement>("*")];
    const classes = (re: RegExp) => all.filter((el) => re.test(el.getAttribute("class") ?? ""));
    // Cards, fields and the desktop share the app's one radius (rounded-lg).
    expect(classes(/\brounded-(xl|2xl|3xl)\b/)).toEqual([]);
    // No partial rules: the rail is a surface tone, its sections plain headings.
    expect(classes(/(^|\s)border-[ltr](\s|$)/)).toEqual([]);
    const rail = q("[data-members-panel]")!;
    expect(rail.className).toContain("bg-chrome");
    // The only rule is the header's hairline, across the page.
    expect(q("[data-call-header]")!.className).toMatch(/(^|\s)border-b(\s|$)/);
    // The composer is a filled field without an outline.
    const field = q("[data-composer]")!.closest<HTMLElement>("[class*=focus-within]")!;
    expect(field.className).toContain("bg-muted");
    expect(field.className).not.toMatch(/(^|\s)border(\s|$)/);
  });
});

// WCAG contrast of a token on the header's surface, from index.css itself.
function tokens(block: ":root" | ".dark"): Record<string, string> {
  const css = readFileSync(resolve(process.cwd(), "src/index.css"), "utf8");
  const body = css.slice(css.indexOf(`${block} {`)).split("\n}")[0]!;
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
}
function srgb(value: string, t: Record<string, string>): number[] {
  const ref = /^var\(--([\w-]+)\)$/.exec(value);
  if (ref) return srgb(t[ref[1]!]!, t);
  const m = /^oklch\(([\d.]+) ([\d.]+) ([\d.]+)\)$/.exec(value);
  if (!m) throw new Error(`not oklch: ${value}`);
  const [L, C, h] = [Number(m[1]), Number(m[2]), (Number(m[3]) * Math.PI) / 180];
  const a = C * Math.cos(h), b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mm = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [4.0767416621 * l - 3.3077115913 * mm + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * mm - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * mm + 1.707614701 * s].map((x) => Math.min(1, Math.max(0, x)));
}
const luminance = ([r, g, b]: number[]) => 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
const contrast = (x: number[], y: number[]) => {
  const [hi, lo] = [luminance(x), luminance(y)].sort((p, q) => q - p);
  return (hi! + 0.05) / (lo! + 0.05);
};

describe("the power icon's green", () => {
  it.each([":root", ".dark"] as const)("reads at 3:1 or more on the header and on its hover (%s)", (block) => {
    const t = { ...tokens(":root"), ...(block === ".dark" ? tokens(".dark") : {}) };
    const green = srgb(t["power-on"]!, t);
    expect(contrast(green, srgb(t.background!, t))).toBeGreaterThanOrEqual(3);
    expect(contrast(green, srgb(t.accent!, t))).toBeGreaterThanOrEqual(3);
  });
});
