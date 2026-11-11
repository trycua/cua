// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Who's on a Space (CUA-2204): the names line, whose it is, and the avatars
// for shared Spaces.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Space, SpaceAccess } from "@/bridge";
import { PeopleStack, SpaceMark } from "@/components/spaces/avatars";
import { accessOf, isMine, ownerLabel } from "@/lib/space-access";
import { namesText } from "@/lib/spaces-page";

afterEach(cleanup);

const NOW = 1_800_000_000_000;

const space = (name: string, extra: Partial<Space> = {}): Space =>
  ({
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

describe("the list's words", () => {
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
