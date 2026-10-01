// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { baseUrlFor, endpointFromLocation, mediaSocketUrl, spacesdUrl } from "./api";
import { isAppBundle } from "./viewer";

const store = () => {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), m };
};

describe("endpoint", () => {
  it("derives the base from the page in every mode", () => {
    expect(baseUrlFor("http://127.0.0.1:3211/viewer/")).toBe("http://127.0.0.1:3211/");
    expect(baseUrlFor("http://127.0.0.1:3211/viewer/index.html?x=1#t")).toBe("http://127.0.0.1:3211/");
    expect(baseUrlFor("https://gw.example/api/signed-svc/tok/viewer/")).toBe("https://gw.example/api/signed-svc/tok/");
    expect(baseUrlFor("http://localhost:8211/s/local%3Adev/viewer/")).toBe("http://localhost:8211/s/local%3Adev/");
  });

  it("moves the ticket from the fragment to session storage and scrubs it", () => {
    const s = store();
    let scrubbed = "";
    const href = "http://127.0.0.1:3211/viewer/#ticket=v1.a.b&files=%2Fhome%2Fcua";
    const e = endpointFromLocation({ href, hash: "#ticket=v1.a.b&files=%2Fhome%2Fcua", search: "" }, s, (u) => (scrubbed = u));
    expect(e.ticket).toBe("v1.a.b");
    expect(e.params.get("files")).toBe("/home/cua");
    expect(e.params.has("ticket")).toBe(false);
    expect(scrubbed).toBe("http://127.0.0.1:3211/viewer/#files=%2Fhome%2Fcua");
    // A reload without the fragment finds it again.
    const again = endpointFromLocation({ href: scrubbed, hash: "#files=%2Fhome%2Fcua", search: "" }, s);
    expect(again.ticket).toBe("v1.a.b");
  });

  it("resolves the media socket and file URLs against the base", () => {
    expect(mediaSocketUrl("https://gw.example/api/signed-svc/tok/", "/media?ticket=abc")).toBe("wss://gw.example/api/signed-svc/tok/media?ticket=abc");
    expect(mediaSocketUrl("http://127.0.0.1:3211/", "/media?ticket=abc")).toBe("ws://127.0.0.1:3211/media?ticket=abc");
    expect(spacesdUrl("http://h/s/x/", "/files/a?sig=1")).toBe("http://h/s/x/files/a?sig=1");
  });
});

describe("drops", () => {
  it("recognizes app bundles like the Spaces app does", () => {
    expect(isAppBundle("Safari.app")).toBe(true);
    expect(isAppBundle("firefox.desktop")).toBe(true);
    expect(isAppBundle("Word.lnk")).toBe(true);
    expect(isAppBundle("notes.txt")).toBe(false);
    expect(isAppBundle("app.tsx")).toBe(false);
  });
});
