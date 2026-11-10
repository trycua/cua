// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { StreamRow } from "@/bridge";
import { DEFAULT_PREFS, SPACE_LAYOUT_STORAGE_KEY } from "@/lib/space-layout";
import { useSpaceLayoutStore } from "@/stores/space-layout";

import { MembersPanel } from "./members-panel";
import type { Roster } from "./roster";

const roster = { people: [{ id: "p1", label: "Dillon", me: true }], agents: [] } as unknown as Roster;
const desktop = { id: "desktop", kind: "desktop", label: "Desktop", help: "", actions: [], icon: { kind: "os", id: "os-ubuntu" } } as unknown as StreamRow;

function panel() {
  return render(
    <MembersPanel
      roster={roster}
      facts={[{ label: "Image", value: "linux" }] as never}
      onMore={() => {}}
      windows={[]}
      desktop={desktop}
      onStream={() => {}}
      folded={false}
      onFold={() => {}}
    />,
  );
}

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  useSpaceLayoutStore.setState({ ...DEFAULT_PREFS });
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const toggle = (id: string) => document.querySelector<HTMLButtonElement>(`[data-section-toggle="${id}"]`)!;

describe("<MembersPanel> sections", () => {
  it("opens every section, each behind a button that says whether it is open", () => {
    panel();
    for (const id of ["people", "stream", "details"]) {
      expect(toggle(id).tagName).toBe("BUTTON");
      expect(toggle(id).getAttribute("aria-expanded")).toBe("true");
      expect(document.getElementById(toggle(id).getAttribute("aria-controls")!)).not.toBeNull();
    }
    expect(document.querySelector("[data-members]")).not.toBeNull();
    expect(document.querySelector("[data-stream-desktop]")).not.toBeNull();
    expect(document.querySelector("[data-rail-fact]")).not.toBeNull();
  });

  it("folds and opens each section on its own, with the keyboard too", () => {
    panel();
    fireEvent.click(toggle("stream"));
    expect(toggle("stream").getAttribute("aria-expanded")).toBe("false");
    expect(document.querySelector("[data-stream-desktop]")).toBeNull();
    expect(document.querySelector("[data-members]")).not.toBeNull();
    expect(document.querySelector("[data-rail-fact]")).not.toBeNull();
    fireEvent.click(toggle("details"));
    expect(document.querySelector("[data-rail-fact]")).toBeNull();
    expect(document.querySelector("[data-call-more]")).toBeNull();
    fireEvent.click(toggle("people"));
    expect(document.querySelector("[data-members]")).toBeNull();
    // Only the three headings are left, and the panel's own controls still work.
    expect(screen.getByLabelText("Hide people")).toBeTruthy();
    fireEvent.click(toggle("people"));
    expect(document.querySelector("[data-members]")).not.toBeNull();
    // A button: Enter and Space are the browser's, and it takes focus.
    toggle("details").focus();
    expect(document.activeElement).toBe(toggle("details"));
  });

  it("has no icon on a heading: the whole row is the control, filled on hover and on keyboard focus", () => {
    panel();
    for (const id of ["people", "stream", "details"]) {
      const b = toggle(id);
      expect(b.querySelector("svg")).toBeNull();
      expect(b.className).toContain("cursor-pointer");
      expect(b.className).toContain("w-full");
      expect(b.className).toContain("focus-visible:bg-accent");
    }
    // Stream and Details fill from edge to edge of the panel; In this Space's row takes the hover.
    expect(toggle("stream").className).toContain("hover:bg-accent");
    expect(toggle("stream").closest("h2")!.className).toContain("-mx-3");
    expect(toggle("people").closest("div")!.className).toContain("hover:bg-accent");
  });

  it("stacks folded headings tight and never reserves a column for a hover-only button", () => {
    panel();
    fireEvent.click(toggle("stream"));
    fireEvent.click(toggle("details"));
    // A folded section adds no margin or padding under its heading.
    for (const sel of ["[data-call-stream]", "[data-call-details]"]) {
      const sec = document.querySelector<HTMLElement>(sel)!;
      expect(sec.className.split(" ")).not.toContain("pb-2");
      expect(sec.className.split(" ")).not.toContain("pb-3");
      expect(sec.querySelector("h2")!.className.split(" ")).not.toContain("mb-1");
    }
    // The first heading row has the same height as the others (h-8).
    expect(document.querySelector("[data-members-panel] > div")!.className).toContain("h-8");
  });

  it("remembers what is folded", () => {
    panel();
    fireEvent.click(toggle("details"));
    expect(JSON.parse(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)!).state.folded).toEqual({ people: false, children: false, stream: false, details: true });
    cleanup();
    panel();
    expect(toggle("details").getAttribute("aria-expanded")).toBe("false");
    expect(document.querySelector("[data-rail-fact]")).toBeNull();
  });
});

describe("Child Spaces section heading", () => {
  it("uses the same quiet heading: no icon, a full-row button that folds the list", async () => {
    const { ChildSpacesSection } = await import("@/components/space-detail/child-spaces-section");
    expect(typeof ChildSpacesSection).toBe("function");
    const { SectionHeading } = await import("./section-heading");
    render(
      <SectionHeading id="children" folded={false} bodyId="section-children">
        Child Spaces
      </SectionHeading>,
    );
    const b = toggle("children");
    expect(b.querySelector("svg")).toBeNull();
    expect(b.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(b);
    expect(useSpaceLayoutStore.getState().folded.children).toBe(true);
  });
});
