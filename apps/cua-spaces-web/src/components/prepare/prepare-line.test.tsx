// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PrepareView } from "@/bridge";

import { PrepareLine } from "./prepare-line";

const view = (phase: string, extra: Partial<PrepareView> = {}) =>
  ({ phase, line: "Preparing stopped: disk is full", action: null, steps: [], queued: [], title: "", composerHint: null, ...extra }) as unknown as PrepareView;

afterEach(cleanup);

describe("PrepareLine", () => {
  it.each(["preparing", "pulling", "booting", "waiting", "ready", "idle"])("shows nothing while %s", (phase) => {
    const { container } = render(<PrepareLine view={view(phase, { line: "anything" })} />);
    expect(container.innerHTML).toBe("");
  });

  it("shows a quiet error with Try again only when failed", () => {
    const retry = vi.fn();
    const { container, getByText } = render(<PrepareLine view={view("failed", { action: { id: "retry", label: "Try again" } as never })} onRetry={retry} />);
    expect(container.textContent).toContain("Preparing stopped");
    fireEvent.click(getByText("Try again"));
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
