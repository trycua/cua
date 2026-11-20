// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { BACKGROUND, DIM_ATTRIBUTE, SURFACE_ATTRIBUTE, SYMBOL, darken, overlayColors, overlayColorsFor, overlaySteps, parseSurface } from "../src/overlay";

const web = path.join(__dirname, "../../cua-spaces-web/src");

describe("title bar overlay", () => {
  it("uses the page's colours when nothing is open", () => {
    expect(overlayColors("light", false)).toEqual({ color: BACKGROUND.light, symbolColor: SYMBOL.light });
    expect(overlayColors("dark", false)).toEqual({ color: BACKGROUND.dark, symbolColor: SYMBOL.dark });
  });

  it("dims like the dialog backdrop: black at 25% light, 40% dark", () => {
    expect(darken("#ffffff", 0.25)).toBe("#bfbfbf");
    expect(overlayColors("light", true)).toEqual({ color: "#b9babc", symbolColor: "#121417" });
    expect(overlayColors("dark", true)).toEqual({ color: "#0d0e11", symbolColor: "#8c8d8f" });
  });

  it("matches the web UI's backdrop and colours", () => {
    for (const file of ["components/ui/dialog.tsx", "components/ui/alert-dialog.tsx"]) {
      const src = readFileSync(path.join(web, file), "utf8");
      const backdrop = src.match(/Primitive\.Backdrop\b[^>]*\/>/)?.[0];
      expect(backdrop).toContain(`${DIM_ATTRIBUTE}=""`);
      expect(backdrop).toContain("bg-black/25");
      expect(backdrop).toContain("dark:bg-black/40");
    }
  });

  it("lays the Windows buttons out again so they repaint in the new colours; Linux sets them once", () => {
    const dimmed = overlayColors("light", true);
    expect(overlaySteps("linux", dimmed, 40)).toEqual({ now: [{ ...dimmed, height: 40 }], next: null });
    const win = overlaySteps("win32", dimmed, 40);
    // Every step carries the new colours; the height ends where it was.
    expect(win.now).toEqual([{ ...dimmed, height: 39 }]);
    expect(win.next).toEqual({ ...dimmed, height: 40 });
  });

  it("follows a dark surface the page marks (the first run), whatever the theme, and the theme again after", () => {
    expect(overlayColorsFor("light", false, "#050607")).toEqual({ color: "#050607", symbolColor: SYMBOL.dark });
    expect(overlayColorsFor("light", true, "#050607")).toEqual({ color: darken("#050607", 0.4), symbolColor: darken(SYMBOL.dark, 0.4) });
    expect(overlayColorsFor("light", false, null)).toEqual(overlayColors("light", false));
    expect(overlayColorsFor("dark", true, undefined)).toEqual(overlayColors("dark", true));
  });

  it("accepts only a #rrggbb surface", () => {
    expect(parseSurface("#050607")).toBe("#050607");
    expect(parseSurface("#ABCDEF")).toBe("#abcdef");
    for (const bad of [null, undefined, 5, "red", "#fff", "url(x)", "#05060"]) expect(parseSurface(bad)).toBeNull();
  });

  it("is marked by the web UI's first run with its own background", () => {
    const src = readFileSync(path.join(web, "components/first-run-new/first-run-new.tsx"), "utf8");
    expect(src).toContain(`${SURFACE_ATTRIBUTE}="#050607"`);
    expect(src).toContain("bg-[#050607]");
  });
});
