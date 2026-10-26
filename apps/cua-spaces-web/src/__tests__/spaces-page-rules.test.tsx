// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces page's own rules (CUA-2204): what its URL holds, which Spaces
// it shows in which order, the tile's corner badge, the list's Size and
// Last used words, and the avatars for shared Spaces.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Space, SpaceAccess } from "@/bridge";
import { PeopleStack, SpaceMark } from "@/components/spaces/avatars";
import { accessOf, isMine, ownerLabel } from "@/lib/space-access";
import { computerChoices, gridArea, lastUsedText, namesText, nextSearch, shownSpaces, sizeCell, spaceMachineId, validateSpacesSearch } from "@/lib/spaces-page";

afterEach(cleanup);

const GB = 1_000_000_000;
const NOW = 1_800_000_000_000;
const MIN = 60_000;

const space = (name: string, extra: Partial<Space> = {}): Space => ({
  id: `local:${name}`,
  name,
  os: "linux",
  status: "running",
  detail: "",
  lastUsedAt: NOW,
  scene: "terminal",
  provider: "local",
  power: { control: "stop", off: false },
  ...extra,
}) as Space;
const off = (name: string, extra: Partial<Space> = {}) => space(name, { power: { control: "stop", off: true }, ...extra });

const withMe: SpaceAccess = {
  kind: "withMe",
  role: "editor",
  ownerName: "Sam",
  place: "Sam's Mac mini",
  people: [],
  canPower: true,
  canTeleport: false,
  canShare: false,
  canRename: false,
  canDelete: false,
  canLeave: true,
};
const team: SpaceAccess = { ...withMe, kind: "team", ownerName: "Priya", teamName: "QA team", place: "QA team" };
const sharedByMe: SpaceAccess = {
  ...withMe,
  kind: "mine",
  role: "owner",
  ownerName: null,
  place: "This Mac",
  canDelete: true,
  canLeave: false,
  people: ["Sam", "Priya", "Leo", "Mia", "Kai"].map((name) => ({ who: `${name.toLowerCase()}@example.com`, name, role: "editor" as const })),
};

describe("the URL", () => {
  it("keeps only what it knows, defaults left out", () => {
    expect(validateSpacesSearch({ view: "list", sort: "size", on: "this-mac", state: "stopped", owner: "shared", select: "stopped", v: "c", junk: 1 })).toEqual({
      view: "list",
      sort: "size",
      on: "this-mac",
      state: "stopped",
      owner: "shared",
      select: "stopped",
    });
    expect(validateSpacesSearch({ view: "grid", sort: "used", state: "all", owner: "anyone", on: "", select: "all" })).toEqual({});
    // The disk alert's old link still sorts by size.
    expect(validateSpacesSearch({ sort: "size" })).toEqual({ sort: "size" });
    expect(nextSearch({ view: "list", sort: "size" }, { sort: undefined, state: "running" })).toEqual({ view: "list", state: "running" });
  });
});

describe("which Spaces, in which order", () => {
  const a = space("alpha", { lastUsedAt: NOW - 5 * MIN });
  const b = off("bravo", { lastUsedAt: NOW - 1 * MIN });
  const c = space("charlie", { lastUsedAt: NOW - 30 * MIN, id: "relay:mac-mini/charlie", provider: "relay", host: "mac-mini" });
  const d = off("delta", { lastUsedAt: NOW - 2 * MIN, id: "relay:sam/delta", provider: "relay", access: withMe });
  const sizes: Record<string, number | null> = { [a.id]: 14 * GB, [b.id]: 41 * GB, [c.id]: null, [d.id]: null };
  const base = {
    spaces: [a, b, c, d],
    mine: isMine,
    machineOf: (s: Space) => spaceMachineId(s, [{ id: "this-mac", current: true, spaceIds: [] }, { id: "mac-mini", current: false, spaceIds: [] }]),
    bytes: (s: Space) => sizes[s.id] ?? null,
    matches: () => true,
  };
  const names = (search: Parameters<typeof shownSpaces>[0]["search"]) => shownSpaces({ ...base, search }).map((s) => s.name);

  it("grid by last used: running first, most recent first in each", () => {
    expect(names({})).toEqual(["alpha", "charlie", "bravo", "delta"]);
  });
  it("list by last used: strictly by the column", () => {
    expect(names({ view: "list" })).toEqual(["bravo", "delta", "alpha", "charlie"]);
  });
  it("name A to Z, size biggest first with unknown sizes last", () => {
    expect(names({ sort: "name" })).toEqual(["alpha", "bravo", "charlie", "delta"]);
    expect(names({ sort: "size", view: "list" })).toEqual(["bravo", "alpha", "charlie", "delta"]);
  });
  it("filters by state, owner and computer", () => {
    expect(names({ state: "stopped" })).toEqual(["bravo", "delta"]);
    expect(names({ owner: "mine" })).toEqual(["alpha", "charlie", "bravo"]);
    expect(names({ owner: "shared" })).toEqual(["delta"]);
    expect(names({ on: "this-mac" })).toEqual(["alpha", "bravo"]);
    expect(names({ on: "mac-mini" })).toEqual(["charlie"]);
  });
  it("offers This Mac first, then each machine, never a bare device", () => {
    const m = (id: string, name: string, current = false, device = false) => ({ id, name, current, device, os: "macos", via: "relay", online: true, spaceIds: [], limits: [] });
    expect(computerChoices([m("mac-mini", "Mac mini"), m("this-mac", "Ada's MacBook", true), m("ipad", "iPad", false, true)]).map((c) => c.value)).toEqual(["this-mac", "mac-mini"]);
  });
});


describe("the list's words", () => {
  it("Size: yours with a known size, else a dash", () => {
    expect(sizeCell(52 * GB, true)).toBe("52 GB");
    expect(sizeCell(52 * GB, false)).toBe("—");
    expect(sizeCell(null, true)).toBe("—");
  });
  it("Last used", () => {
    expect(lastUsedText(NOW - 20_000, NOW)).toBe("Just now");
    expect(lastUsedText(NOW - 2 * MIN, NOW)).toBe("2 min ago");
    expect(lastUsedText(NOW - 3 * 60 * MIN, NOW)).toBe("3 hr ago");
    expect(lastUsedText(NOW - 30 * 60 * MIN, NOW)).toBe("Yesterday");
    expect(lastUsedText(NOW - 9 * 24 * 60 * MIN, NOW)).toBe("9 days ago");
    expect(lastUsedText(NOW - 21 * 24 * 60 * MIN, NOW)).toBe("3 weeks ago");
  });
  it("names who has it", () => {
    expect(namesText(["Sam"])).toBe("Sam");
    expect(namesText(["Sam", "Priya"])).toBe("Sam and Priya");
    expect(namesText(["Sam", "Priya", "Leo", "Mia", "Kai"])).toBe("Sam, Priya and 3 others");
  });
});

describe("whose it is", () => {
  it("a Space without access facts is yours alone, on its computer", () => {
    const a = accessOf(space("a"), "Linux box");
    expect([a.kind, a.people, a.canDelete, a.canLeave]).toEqual(["mine", [], true, false]);
    expect(accessOf(space("a", { provider: "relay", id: "relay:x/a" }), "Linux box").place).toBe("Linux box");
    expect(isMine(space("a"))).toBe(true);
    expect(isMine(space("a", { access: withMe }))).toBe(false);
  });
  it("the avatar: the OS for yours, the owner's initial shared with you, the team's for a team's", () => {
    expect(ownerLabel(withMe)).toBe("Sam");
    expect(ownerLabel(team)).toBe("QA team");
    expect(ownerLabel(sharedByMe)).toBeNull();
    const { container, rerender } = render(<SpaceMark space={space("a")} access={sharedByMe} />);
    expect(container.querySelector("[data-avatar]")?.getAttribute("data-avatar")).toBe("mine");
    expect(container.querySelector("svg[aria-label=Linux]")).not.toBeNull();
    rerender(<SpaceMark space={space("a")} access={withMe} />);
    expect(container.querySelector('[data-avatar="withMe"]')?.textContent).toBe("S");
    rerender(<SpaceMark space={space("a")} access={team} />);
    expect(container.querySelector('[data-avatar="team"]')?.textContent).toBe("Q");
  });
  it("the people stack: three, then +2, only on yours", () => {
    const { container, rerender } = render(<PeopleStack access={sharedByMe} />);
    const stack = container.querySelector<HTMLElement>("[data-people]")!;
    expect(stack.textContent).toBe("SPL+2");
    expect(stack.title).toBe("Shared with Sam, Priya and 3 others");
    rerender(<PeopleStack access={withMe} />);
    expect(container.querySelector("[data-people]")).toBeNull();
  });
});

describe("the grid's place", () => {
  const base = { error: false, loading: false, guide: false, total: 3, shown: 3, query: "" };
  it("the Spaces, or why there are none", () => {
    expect(gridArea(base)).toBe("spaces");
    expect(gridArea({ ...base, error: true })).toBe("error");
    expect(gridArea({ ...base, loading: true, total: 0, shown: 0 })).toBe("loading");
    expect(gridArea({ ...base, guide: true, total: 0, shown: 0 })).toBe("guide");
    expect(gridArea({ ...base, total: 0, shown: 0 })).toBe("empty");
    expect(gridArea({ ...base, shown: 0, query: " qa " })).toBe("noMatches");
    expect(gridArea({ ...base, shown: 0 })).toBe("noneInFilter");
  });
});
