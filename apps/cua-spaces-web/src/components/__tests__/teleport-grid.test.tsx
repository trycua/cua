// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PickerTile, TeleportSession, TeleportStore } from "@/bridge";
import { TeleportGrid } from "@/components/teleport/grid";

afterEach(cleanup);

const tile = (id: string, over: Partial<PickerTile> = {}): PickerTile => ({
  id,
  title: id,
  help: "App and signed-in state",
  disabled: false,
  selected: false,
  icon: { kind: "host", path: `/Applications/${id}.app` },
  thumbnail: { kind: "none" },
  ...over,
});

describe("teleport grid", () => {
  it("says why an app cannot move, and asks for the icons of the tiles it draws", () => {
    const why = "no Linux build in the install manifest and no teleport provider for its state";
    const session = {
      grid: {
        sections: [
          { title: "Apps", tiles: [tile("Slack")] },
          { title: "Not available", tiles: [tile("1Password", { disabled: true, help: why })] },
        ],
        emptyText: null,
      },
    } as unknown as TeleportSession;
    const teleport = { icon: vi.fn(), thumbnail: vi.fn(), select: vi.fn(), activate: vi.fn(), step: vi.fn() } as unknown as TeleportStore;
    const images = new Map([["host:/Applications/Slack.app", "data:image/png;base64,AAAA"]]);
    const { container } = render(<TeleportGrid session={session} teleport={teleport} images={images} />);
    expect(screen.getByText(why).hasAttribute("data-tile-reason")).toBe(true);
    // An app that can move shows no reason line, only its tooltip.
    expect(container.querySelectorAll("[data-tile-reason]")).toHaveLength(1);
    expect(teleport.icon).toHaveBeenCalledTimes(2);
    expect(container.querySelector('[data-tile="Slack"] img')?.getAttribute("src")).toBe("data:image/png;base64,AAAA");
  });
});
