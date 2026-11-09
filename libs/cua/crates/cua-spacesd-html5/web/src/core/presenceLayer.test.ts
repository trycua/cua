// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { drawsOwnCursor, PresenceLayer } from "./presenceLayer";

describe("drawsOwnCursor", () => {
  it("draws yours only when everything holds", () => {
    const all = { joined: true, live: true, width: 10, height: 10, inside: true };
    expect(drawsOwnCursor(all)).toBe(true);
    for (const off of [{ joined: false }, { live: false }, { width: 0 }, { height: 0 }, { inside: false }]) {
      expect(drawsOwnCursor({ ...all, ...off })).toBe(false);
    }
  });
});

describe("PresenceLayer system cursor", () => {
  function stage() {
    const host = document.createElement("div");
    const surface = document.createElement("canvas");
    host.appendChild(surface);
    document.body.appendChild(host);
    let size = { width: 1000, height: 500 };
    surface.getBoundingClientRect = () =>
      ({ left: 0, top: 0, ...size, right: size.width, bottom: size.height, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    const state = { me: true as boolean, live: true as boolean };
    const layer = new PresenceLayer({
      surface,
      host,
      view: () => null,
      me: () => (state.me ? { id: "me", color: "#3cb44b" } : null),
      live: () => state.live,
      interactive: true,
      requestFrame: () => 0,
      cancelFrame: () => {},
    });
    const own = () => host.querySelector(".presence-cursor-me") as HTMLElement | null;
    const drawn = () => own() !== null && own()!.style.display !== "none";
    return { surface, layer, state, own, drawn, resize: (w: number, h: number) => (size = { width: w, height: h }) };
  }

  it("hides the system cursor exactly while yours is drawn", () => {
    const s = stage();
    s.layer.pointerAt(100, 100);
    expect(s.drawn()).toBe(true);
    expect(s.surface.style.cursor).toBe("none");

    // Mouse exit.
    s.layer.pointerAt(null, null);
    expect(s.drawn()).toBe(false);
    expect(s.surface.style.cursor).toBe("");

    // Stream lost while hovering.
    s.layer.pointerAt(100, 100);
    s.state.live = false;
    s.layer.render();
    expect(s.drawn()).toBe(false);
    expect(s.surface.style.cursor).toBe("");
    s.state.live = true;
    s.layer.render();
    expect(s.surface.style.cursor).toBe("none");

    // Presence gone.
    s.state.me = false;
    s.layer.render();
    expect(s.drawn()).toBe(false);
    expect(s.surface.style.cursor).toBe("");
    s.state.me = true;

    // A zero-size stream.
    s.layer.pointerAt(100, 100);
    s.resize(0, 0);
    s.layer.render();
    expect(s.surface.style.cursor).toBe("");
    s.resize(1000, 500);

    // The window loses focus.
    s.layer.pointerAt(100, 100);
    expect(s.surface.style.cursor).toBe("none");
    window.dispatchEvent(new Event("blur"));
    expect(s.drawn()).toBe(false);
    expect(s.surface.style.cursor).toBe("");

    // Teardown.
    s.layer.pointerAt(100, 100);
    s.layer.destroy();
    expect(s.surface.style.cursor).toBe("");
  });

  it("keeps the system cursor before you have joined", () => {
    const s = stage();
    s.state.me = false;
    s.layer.pointerAt(100, 100);
    expect(s.drawn()).toBe(false);
    expect(s.surface.style.cursor).toBe("");
    s.layer.destroy();
  });
});
