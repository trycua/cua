// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { limitsFor, plainJoinError, plainPairingError, stripInternalPrefixes } from "../friendly-errors";

let warn: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
});
afterEach(() => warn.mockRestore());

describe("plain host errors", () => {
  it("strips the command's and relay's prefixes", () => {
    expect(stripInternalPrefixes("cua: relay: disk full")).toBe("Disk full");
    expect(stripInternalPrefixes("host setup failed at register: oops")).toBe("Oops");
    expect(stripInternalPrefixes("")).toBe("");
  });

  it("joins: a bad code is one plain line, and the raw text goes to the log", () => {
    const raw = "cua: unauthenticated: host setup failed at register: relay: pairing code is not valid";
    const text = plainJoinError(new Error(raw));
    expect(text).toBe("That code didn't work. It may be used, expired or mistyped. Make a new one in Add a computer.");
    expect(warn).toHaveBeenCalledWith(expect.any(String), raw);
  });

  it("joins: other failures lose the prefixes", () => {
    expect(plainJoinError(new Error("cua: host setup failed at install: disk is full"))).toBe("Disk is full");
    expect(plainJoinError(new Error("cua host setup timed out"))).toMatch(/took too long/);
  });

  it("pairing: an unregistered computer is told to set up first", () => {
    const text = plainPairingError(new Error("not found: relay: no such device"));
    expect(text).toBe("This computer isn't set up for access yet. Set it up first, then add computers.");
  });

  it("pairing: other failures keep a plain reason", () => {
    expect(plainPairingError(new Error("relay: quota exceeded"))).toBe("Couldn't make a link: Quota exceeded.");
    expect(plainPairingError(new Error(""))).toBe("Couldn't make a link.");
  });
});

describe("the limits line per system", () => {
  const line = "Up to 4 Spaces \u00b7 2 macOS VMs";
  it("keeps the macOS VMs on a Mac and drops them elsewhere, with no license wording", () => {
    expect(limitsFor(line, true)).toBe(line);
    expect(limitsFor(line, false)).toBe("Up to 4 Spaces");
    expect(limitsFor("Up to 4 Spaces \u00b7 1 macOS VM", false)).toBe("Up to 4 Spaces");
    expect(limitsFor("No Space limit", false)).toBe("No Space limit");
    expect(`${limitsFor(line, true)}${limitsFor(line, false)}`).not.toMatch(/license|Apple/i);
  });
});
