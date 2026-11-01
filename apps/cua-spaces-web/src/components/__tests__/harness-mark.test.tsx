// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { APP_MARKS, HarnessMark } from "@/components/agents/harness-mark";

const WEB = resolve(__dirname, "../../..");
const REGISTRY = resolve(WEB, "../../libs/cua/crates/cua-agent-setup/src/registry.rs");
/** Supported apps with no usable official mark: they keep the letter tile. */
const LETTER_TILE = ["kiro"];

afterEach(cleanup);

describe("coding app marks", () => {
  it("has an official mark for every supported coding app but the listed fallbacks", () => {
    const ids = [...readFileSync(REGISTRY, "utf8").matchAll(/^\s+id: "([^"]+)",$/gm)].map((m) => m[1] ?? "");
    expect(ids.length).toBeGreaterThan(10);
    expect(ids.filter((id) => !(id in APP_MARKS)).sort()).toEqual(LETTER_TILE);
  });

  it("ships every mark it names, with a source in NOTICE", () => {
    const notice = readFileSync(resolve(WEB, "public/onboarding/marks/NOTICE"), "utf8");
    for (const mark of Object.values(APP_MARKS))
      for (const src of [mark.light, mark.dark]) {
        expect(existsSync(resolve(WEB, "public", src.slice(1))), src).toBe(true);
        expect(notice, src).toContain(src.split("/").pop());
      }
  });

  it("shows Cursor's light and dark marks instead of a letter", () => {
    const { container } = render(<HarnessMark agent="cursor" name="Cursor" />);
    const imgs = [...container.querySelectorAll("img")].map((i) => [i.getAttribute("src"), i.className]);
    expect(container.textContent).toBe("");
    expect(imgs).toEqual([
      ["/onboarding/marks/mark-cursor.svg", expect.stringContaining("dark:hidden")],
      ["/onboarding/marks/mark-cursor-dark.svg", expect.stringContaining("dark:block")],
    ]);
  });

  it("shows VS Code's full-colour mark in both themes", () => {
    const { container } = render(<HarnessMark agent="vscode" name="VS Code (Copilot agent mode)" />);
    expect([...container.querySelectorAll("img")].map((i) => [i.getAttribute("src"), i.dataset.theme])).toEqual([["/onboarding/marks/mark-vscode.svg", "both"]]);
  });

  it("gives Codex and Hermes their white marks in dark mode, not a CSS invert", () => {
    for (const [agent, file] of [["codex", "openai"], ["hermes", "hermes"]] as const) {
      const { container } = render(<HarnessMark agent={agent} name={agent} />);
      const imgs = [...container.querySelectorAll("img")];
      expect(imgs.map((i) => i.getAttribute("src"))).toEqual([`/onboarding/marks/mark-${file}.svg`, `/onboarding/marks/mark-${file}-dark.svg`]);
      expect(imgs.some((i) => i.className.includes("invert"))).toBe(false);
      cleanup();
    }
  });

  it("falls back to the first letter for an app without a mark", () => {
    const { container } = render(<HarnessMark agent="kiro" name="Kiro" />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe("K");
  });
});
