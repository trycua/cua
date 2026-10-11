// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { openableCount } from "../model/window";
import { FIXTURE_SPACES } from "../model/fixtures";
import type { Space } from "../model/types";
import { Ambient } from "./Ambient";

describe("Ambient (notched) teleport morph", () => {
  it("shows the 'N Spaces' tab and hides the teleport hint while idle", () => {
    render(
      <Ambient spaces={FIXTURE_SPACES} displayStyle="notched" onExpand={vi.fn()} teleportOpen={false} />,
    );
    // The idle tab is the clickable ambient affordance.
    const tab = screen.getByRole("button", { name: /Spaces, .* active/ });
    expect(tab).toHaveClass("ambient-tab");
    // Its container is not in the teleport state.
    expect(tab.parentElement).toHaveAttribute("data-teleport", "off");
    // The hint is aria-hidden while closed, so it is out of the a11y tree.
    expect(screen.queryByRole("status", { name: /Teleport to Cua/ })).toBeNull();
  });

  it("morphs into the compact 'Teleport to Cua' hint (no icon) when the drag starts", () => {
    render(
      <Ambient spaces={FIXTURE_SPACES} displayStyle="notched" onExpand={vi.fn()} teleportOpen />,
    );
    const tab = screen.getByRole("button", { name: /Spaces, .* active/ });
    expect(tab.parentElement).toHaveAttribute("data-teleport", "on");
    const hint = screen.getByRole("status", { name: /Teleport to Cua/ });
    expect(hint).toBeInTheDocument();
    // The hint is deliberately unintrusive: text only, no ghost/thumbnail.
    expect(hint.querySelector("img")).toBeNull();
  });
});

describe("Ambient tab and activity (the core's notch module)", () => {
  const remote = FIXTURE_SPACES.filter((s) => s.status !== "local");

  it("is two rows: the count over the word", () => {
    const { container } = render(<Ambient spaces={FIXTURE_SPACES} displayStyle="notched" onExpand={vi.fn()} />);
    // The Spaces the user can open (the core's count, the menu bar item's too):
    // the fixture's This Mac is shared and reachable, so it counts.
    const n = openableCount(FIXTURE_SPACES);
    expect(n).toBe(remote.length + 1);
    expect(container.querySelector(".ambient-tab-count")).toHaveTextContent(String(n));
    expect(container.querySelector(".ambient-tab-word")).toHaveTextContent(n === 1 ? "Space" : "Spaces");
  });

  it("shows a transfer before the hotspot before a starting Space, and nothing when idle", () => {
    const idle = FIXTURE_SPACES.filter((s) => s.status !== "provisioning");
    const starting: Space[] = [...idle, { ...remote[0]!, id: "starting", status: "provisioning", startedAt: 1 }];
    const kind = (props: Partial<Parameters<typeof Ambient>[0]>) => {
      const { container, unmount } = render(
        <Ambient spaces={idle} displayStyle="notched" onExpand={vi.fn()} {...props} />,
      );
      const k = container.querySelector("[data-activity]")?.getAttribute("data-activity") ?? null;
      unmount();
      return k;
    };
    expect(kind({})).toBeNull();
    expect(kind({ spaces: starting })).toBe("provisioning");
    expect(kind({ spaces: starting, hotspotActive: true })).toBe("hotspot");
    expect(kind({ spaces: starting, hotspotActive: true, transfer: { active: true, sent: 1, total: 4 } })).toBe(
      "transfer",
    );
  });
});
