// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { prepareReduce, prepareView, type PrepareAction, type PrepareState } from "@/bridge";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";

import { PreparePanel } from "./prepare-panel";

afterEach(cleanup);

async function stateAfter(...actions: PrepareAction[]): Promise<PrepareState> {
  const core = await testCore();
  let s: PrepareState | null = null;
  for (const a of actions) s = prepareReduce(core, s, a)!.state;
  return s!;
}

const MB = 1_048_576;

describe.skipIf(!wasmBuilt)("the preparing block", () => {
  it("is plain text and one thin bar: four lines, a bar under the current step only, no boxes, no nested cards", async () => {
    const core = await testCore();
    const s = await stateAfter(
      { type: "start", nowMs: 1, harness: "claude-code", pendingId: "p" },
      { type: "phase", nowMs: 2, phase: "preparing", detail: "Setting up the Linux runtime", done: 176 * MB, total: 440 * MB, perSecond: 5 * MB },
    );
    const { container } = render(<PreparePanel view={prepareView(core, s)!} />);
    const rows = [...container.querySelectorAll("[data-prepare-step]")].map((e) => e.getAttribute("data-prepare-step"));
    expect(rows).toEqual(["runtime", "image", "home", "agent"]);
    // One bar, under the Linux runtime row, and no per-row bars.
    const bars = container.querySelectorAll("[data-prepare-bar]");
    expect(bars).toHaveLength(1);
    expect(bars[0]!.closest("[data-prepare-step]")!.getAttribute("data-prepare-step")).toBe("runtime");
    // No headline sentence repeating the row: the row says the bytes once.
    expect(container.textContent!.match(/176 MB of 440 MB/g)).toHaveLength(1);
    expect(container.querySelector("[data-prepare-line]")).toBeNull();
    expect(container.textContent).toContain("Your message sends as soon as your agent is ready");
    // No bordered cell, card or table: no element draws a border or a grouped container, and nothing nests a card.
    expect(container.querySelector("[data-grouped]")).toBeNull();
    const boxed = [...container.querySelectorAll("*")].filter((e) =>
      /(^|\s)(border|border-[trblxy]|divide-|rounded-(lg|xl|2xl)|bg-card|shadow)/.test(e.getAttribute("class") ?? ""),
    );
    expect(boxed).toEqual([]);
    expect(container.textContent).not.toContain("·");
  });

  it("a connection that dropped says so once above the steps, and Try again shows only after a failure", async () => {
    const core = await testCore();
    const waiting = await stateAfter({ type: "start", nowMs: 1, harness: "claude-code" }, { type: "failed", nowMs: 2, message: "reset", network: true });
    const a = render(<PreparePanel view={prepareView(core, waiting)!} onRetry={() => {}} />);
    expect(a.container.querySelector("[data-prepare-line]")!.textContent).toContain("connection dropped");
    expect(a.container.querySelector("[data-prepare-action]")).toBeNull();
    a.unmount();
    const failed = await stateAfter(
      { type: "start", nowMs: 1, harness: "claude-code" },
      { type: "failed", nowMs: 2, message: "disk full" },
      { type: "tick", nowMs: 10_000 },
      { type: "failed", nowMs: 10_001, message: "disk full" },
    );
    const b = render(<PreparePanel view={prepareView(core, failed)!} onRetry={() => {}} />);
    expect(b.container.querySelector("[data-prepare-action]")!.textContent).toBe("Try again");
  });
});
