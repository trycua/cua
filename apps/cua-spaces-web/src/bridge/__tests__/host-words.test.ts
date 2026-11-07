// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";
import { hostWords, localizeHostWords } from "../host-words";
import { testCore, wasmBuilt } from "./testCore";

describe("host words: this computer in the words of its own system", () => {
  const lines = ["This Mac is almost out of space.", "Uses about 4 GB on this Mac", "Connect Claude Code on your Mac"];

  it("keeps the Mac's words on a Mac", () => {
    for (const l of lines) expect(hostWords(l, "macos")).toBe(l);
  });

  it("says This PC on Windows", () => {
    expect(hostWords(lines[0]!, "windows")).toBe("This PC is almost out of space.");
    expect(hostWords(lines[1]!, "windows")).toBe("Uses about 4 GB on this PC");
    expect(hostWords(lines[2]!, "windows")).toBe("Connect Claude Code on your PC");
  });

  it("says This computer on Linux (the app's convention)", () => {
    expect(hostWords(lines[0]!, "linux")).toBe("This computer is almost out of space.");
    expect(hostWords(lines[2]!, "linux")).toBe("Connect Claude Code on your computer");
  });

  it("leaves other Macs and text about macOS itself alone", () => {
    expect(hostWords("Sam's Mac mini", "windows")).toBe("Sam's Mac mini");
    expect(hostWords("Macs older than this Mac can't", "windows")).toBe("Macs older than this PC can't");
    expect(hostWords("Lume is not installed on this Mac", "windows")).toBe("Lume is not installed on this Mac");
    expect(hostWords("Keychain on this Mac", "windows")).toBe("Keychain on this Mac");
    expect(hostWords("this-mac", "windows")).toBe("this-mac");
  });

  it("walks a core answer: values, not keys, and never mutates it", () => {
    const answer = { "This Mac": 1, rows: [{ name: "This Mac", detail: "ok" }, 3, null], note: "on this Mac" };
    const out = localizeHostWords(answer, "windows");
    expect(out).toEqual({ "This Mac": 1, rows: [{ name: "This PC", detail: "ok" }, 3, null], note: "on this PC" });
    expect((answer.rows[0] as { name: string }).name).toBe("This Mac");
    expect(localizeHostWords(answer, "macos")).toBe(answer);
  });
});

describe("host words through the real core", () => {
  it.skipIf(!wasmBuilt)("words the core's answers for the host the shell reports", async () => {
    const core = await testCore();
    const warning = { warning: { level: "low", free: 1, total: 100, threshold: 10 } };
    const ask = (platform: string) => {
      vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
      return core.call<{ text: string }>("resources.warning", warning).text;
    };
    expect(ask("MacIntel")).toBe("This Mac is almost out of space. Delete old Spaces or snapshots.");
    expect(ask("Win32")).toBe("This PC is almost out of space. Delete old Spaces or snapshots.");
    expect(ask("Linux x86_64")).toBe("This computer is almost out of space. Delete old Spaces or snapshots.");
    vi.restoreAllMocks();
  });
});
