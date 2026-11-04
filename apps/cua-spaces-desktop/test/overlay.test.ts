// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { BACKGROUND, DIM_ATTRIBUTE, SYMBOL, darken, overlayColors, overlaySteps } from "../src/overlay";

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
      const backdrop = src.split("\n").find((l) => l.includes("Backdrop "))!;
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
});
