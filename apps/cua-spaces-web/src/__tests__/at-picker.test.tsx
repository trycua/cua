// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `@` in the prompt box (Home's, and the Agents tab's) lists your computers: filter as you type, arrows and Enter
// pick, Esc closes, and the pick is the where chip. No computers: one plain line.

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";

import { AskBox } from "@/components/ask-first/ask-box";
import { atPlaces, matchPlaces } from "@/components/ask-first/at-picker";
import type { NameableMachine } from "@/components/ask-first/at-picker";
import type { TaskWhere } from "@/lib/ask-first";

import { stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];

const MACHINES: NameableMachine[] = [
  { id: "m-off", name: "Studio Mac mini", current: false, online: false, os: "macos" },
  { id: "m-this", name: "dillon-mbp", current: true, online: true, os: "macos" },
  { id: "m-linux", name: "Linux box", current: false, online: true, os: "linux" },
  { id: "m-win", name: "Windows PC", current: false, online: true, os: "windows" },
];

function Harness({ machines = MACHINES, onWhereSeen }: { machines?: NameableMachine[]; onWhereSeen?: (w: TaskWhere) => void }) {
  const [text, setText] = useState("");
  const [where, setWhere] = useState<TaskWhere>({ kind: "agent" });
  return (
    <AskBox
      text={text}
      onText={setText}
      view={null}
      selected={null}
      onPickAccount={() => {}}
      onModel={() => {}}
      onSignIn={() => {}}
      repo={null}
      where={where}
      onWhere={(w) => {
        setWhere(w);
        onWhereSeen?.(w);
      }}
      kinds={[]}
      spaces={[]}
      machines={machines}
      agentHome
      busy={false}
      onSubmit={() => {}}
      compact
      pickerSide="up"
    />
  );
}

const box = () => q("textarea") as HTMLTextAreaElement;
const type = async (value: string) => {
  await act(async () => {
    fireEvent.change(box(), { target: { value, selectionStart: value.length } });
  });
};
const key = async (k: string) => {
  await act(async () => {
    fireEvent.keyDown(box(), { key: k });
  });
};
const options = () => qa("[data-at-option]").map((o) => o.dataset.atOption);

describe("@ in the prompt box", () => {
  it("opens on @ with your computers, online first, this one leading, offline greyed", async () => {
    render(<Harness />);
    expect(q("[data-at-picker]")).toBeNull();
    await type("@");
    await waitFor(() => expect(q("[data-at-picker]")).not.toBeNull());
    expect(options()).toEqual(["m-this", "m-linux", "m-win", "m-off"]);
    expect(q("[data-at-option='m-this']")?.textContent).toContain("This Mac");
    expect(q("[data-at-option='m-off']")?.dataset.atOffline).toBe("");
    expect(q("[data-at-option='m-linux'] svg")?.getAttribute("aria-label")).toBe("Linux");
  });

  it("filters as you type after @", async () => {
    render(<Harness />);
    await type("run tests @li");
    await waitFor(() => expect(options()).toEqual(["m-linux"]));
    await type("run tests @zzz");
    await waitFor(() => expect(q("[data-at-none]")?.textContent).toBe("No computer matches"));
  });

  it("arrows move, Enter picks: the pick becomes the chip and leaves the text", async () => {
    const seen: TaskWhere[] = [];
    render(<Harness onWhereSeen={(w) => seen.push(w)} />);
    await type("fix the build @");
    await waitFor(() => expect(q("[data-at-picker]")).not.toBeNull());
    await key("ArrowDown");
    await key("ArrowDown");
    expect(q("[data-at-option='m-win']")?.getAttribute("aria-selected")).toBe("true");
    await key("ArrowUp");
    await key("Enter");
    expect(seen).toEqual([{ kind: "machine", machine: { id: "m-linux", name: "Linux box" } }]);
    await waitFor(() => expect(q("[data-at-picker]")).toBeNull());
    expect(box().value).toBe("fix the build ");
    expect(q("[data-ask-where-chip='at']")?.textContent).toContain("Linux box");
    // The chip's X puts the choice back to the agent's.
    await act(async () => {
      fireEvent.click(q("[data-ask-where-clear]")!);
    });
    expect(q("[data-ask-where-chip='at']")).toBeNull();
  });

  it("a click picks; an offline computer cannot be picked", async () => {
    const seen: TaskWhere[] = [];
    render(<Harness onWhereSeen={(w) => seen.push(w)} />);
    await type("@");
    await waitFor(() => expect(q("[data-at-picker]")).not.toBeNull());
    await act(async () => {
      fireEvent.mouseDown(q("[data-at-option='m-off']")!);
    });
    expect(seen).toEqual([]);
    await act(async () => {
      fireEvent.mouseDown(q("[data-at-option='m-this']")!);
    });
    expect(seen).toEqual([{ kind: "this-mac" }]);
  });

  it("Esc closes it and keeps the text", async () => {
    render(<Harness />);
    await type("go @");
    await waitFor(() => expect(q("[data-at-picker]")).not.toBeNull());
    await key("Escape");
    expect(q("[data-at-picker]")).toBeNull();
    expect(box().value).toBe("go @");
  });

  it("with no computers it is one plain line, not an empty list", async () => {
    render(<Harness machines={[]} />);
    await type("@");
    await waitFor(() => expect(q("[data-at-none]")?.textContent).toBe("No computers yet"));
    expect(qa("[data-at-option]")).toHaveLength(0);
  });

  it("an @ inside a word (an address) does not open it", async () => {
    render(<Harness />);
    await type("mail ada@example.com");
    expect(q("[data-at-picker]")).toBeNull();
  });
});

describe("the places @ offers", () => {
  it("without agent homes: this computer, then the online computers; names match by word", () => {
    const places = atPlaces({ computers: [{ id: "relay:x", name: "Linux box", os: "linux" } as never] });
    expect(places.map((p) => p.id)).toEqual(["this-mac", "relay:x"]);
    expect(matchPlaces("bo", places).map((p) => p.id)).toEqual(["relay:x"]);
    expect(matchPlaces("", places)).toHaveLength(2);
  });
});
