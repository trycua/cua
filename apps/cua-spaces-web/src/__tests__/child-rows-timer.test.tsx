// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SubagentRow, childPageId } from "@/components/space-thread/child-rows";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const sub = (over: object) =>
  ({
    type: "subagent",
    sub: { id: "s1", agent: "claude", title: "Check the logs", state: "running", steps: 2, result: null, startedMs: 1_000_000, endedMs: null, ...over },
  }) as never;

describe("a running subagent row", () => {
  it("counts its running time up every second", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000 + 12_000);
    const { container } = render(<SubagentRow item={sub({})} />);
    expect(container.querySelector("[data-live-elapsed]")?.textContent).toBe("Running 12s");
    act(() => {
      vi.advanceTimersByTime(3_000);
    });
    expect(container.querySelector("[data-live-elapsed]")?.textContent).toBe("Running 15s");
  });

  it("shows the plain state word once done, with the time it took", () => {
    const { container } = render(<SubagentRow item={sub({ state: "done", endedMs: 1_000_000 + 65_000 })} />);
    expect(container.querySelector("[data-live-elapsed]")).toBeNull();
    expect(container.textContent).toContain("Done");
    expect(container.textContent).toContain("1m 05s");
  });
});

describe("the page a child Space row opens", () => {
  it("is the being-made page until the Space exists, then the Space", () => {
    expect(childPageId({ space: "local:mac", key: "mac:500", state: "creating" })).toBe("pending:agent-mac-500");
    expect(childPageId({ space: "local:mac", key: "mac:500", state: "failed" })).toBe("pending:agent-mac-500");
    expect(childPageId({ space: "local:mac", key: "mac:500", state: "ready" })).toBe("local:mac");
    expect(childPageId({ space: "local:old" })).toBe("local:old");
  });
});
