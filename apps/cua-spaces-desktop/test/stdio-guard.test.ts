// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";
import { isStdioWriteEvent } from "../src/crash-reporting";
import { guardStdio, isStreamWriteError, protectConsole, safeLogger } from "../src/stdio-guard";

const codeError = (code: string) => Object.assign(new Error(`write ${code}`), { code, syscall: "write" });

describe("guardStdio", () => {
  it("swallows a closed stream's write errors on stdout and stderr, and only that class", () => {
    const stdout = new PassThrough();
    const stderr = new PassThrough();
    guardStdio({ stdout, stderr });
    guardStdio({ stdout, stderr }); // twice adds nothing
    expect(stdout.listenerCount("error")).toBe(1);
    for (const code of ["EPIPE", "EBADF", "EIO", "ERR_STREAM_DESTROYED"]) {
      expect(() => stdout.emit("error", codeError(code))).not.toThrow();
      expect(() => stderr.emit("error", codeError(code))).not.toThrow();
    }
    // A different error is still loud.
    expect(() => stdout.emit("error", new Error("boom"))).toThrow("boom");
    expect(() => stdout.emit("error", Object.assign(new Error("nope"), { code: "ENOSPC" }))).toThrow("nope");
  });

  it("a destroyed stream written to does not become an uncaught error", async () => {
    const stdout = new PassThrough();
    guardStdio({ stdout, stderr: null });
    stdout.destroy();
    expect(() => stdout.write("late")).not.toThrow();
    await new Promise((r) => setImmediate(r));
  });

  it("knows the class", () => {
    expect(isStreamWriteError(codeError("EPIPE"))).toBe(true);
    expect(isStreamWriteError(new Error("EPIPE"))).toBe(false);
    expect(isStreamWriteError(null)).toBe(false);
  });
});

describe("protectConsole and safeLogger", () => {
  it("log calls cannot throw", () => {
    const fake = {
      log: vi.fn(() => {
        throw codeError("EPIPE");
      }),
      warn: vi.fn(),
    } as Record<"log" | "warn", (...a: unknown[]) => void>;
    protectConsole(fake);
    expect(() => fake.log("x")).not.toThrow();
    fake.warn("y");
    expect(fake.warn).toBeDefined();
    const wrapped = fake.log;
    protectConsole(fake);
    expect(fake.log).toBe(wrapped);
  });

  it("the updater logger swallows a console that throws", () => {
    const spy = vi.spyOn(console, "info").mockImplementation(() => {
      throw codeError("EBADF");
    });
    expect(() => safeLogger.info("Found version 1.2.3")).not.toThrow();
    spy.mockRestore();
  });
});

describe("isStdioWriteEvent", () => {
  const frame = (fn: string, file: string) => ({ function: fn, filename: file });
  const closedStdout = {
    exception: {
      values: [
        {
          type: "Error",
          value: "",
          stacktrace: {
            frames: [
              frame("Socket._write", "node:internal/net"),
              frame("writeOrBuffer", "node:internal/streams/writable"),
              frame("Writable.write", "node:internal/streams/writable"),
              frame("[kWriteToConsole]", "node:internal/console/constructor"),
              frame("console.info", "node:internal/console/constructor"),
              frame("NsisUpdater.onUpdateAvailable", "app:///node_modules/electron-updater/out/AppUpdater.js"),
            ],
          },
        },
      ],
    },
  };

  it("matches the console-to-closed-stdout stack that paged as T, V, W and X", () => {
    expect(isStdioWriteEvent(closedStdout)).toBe(true);
  });

  it("matches an EPIPE error raised by a stream write", () => {
    const e = { exception: { values: [{ type: "Error", value: "write EPIPE", stacktrace: { frames: [frame("Socket._write", "node:internal/net")] } }] } };
    expect(isStdioWriteEvent(e)).toBe(true);
  });

  it("matches Sentry's own handler failing on the same stream (the Linux issue P)", () => {
    const p = {
      exception: {
        values: [
          {
            type: "Error",
            value: "",
            stacktrace: {
              frames: [
                frame("afterWriteDispatched", "node:internal/stream_base_commons"),
                frame("Socket._write", "node:net"),
                frame("Writable.write", "node:internal/streams/writable"),
                frame("console.error", "node:internal/console/constructor"),
              ],
            },
          },
        ],
      },
    };
    expect(isStdioWriteEvent(p)).toBe(true);
  });

  it("keeps real errors: no stream write, or a write without console or code", () => {
    expect(
      isStdioWriteEvent({
        exception: { values: [{ type: "TypeError", value: "x is undefined", stacktrace: { frames: [frame("check", "app:///updater.js")] } }] },
      }),
    ).toBe(false);
    expect(
      isStdioWriteEvent({ exception: { values: [{ type: "Error", value: "EPIPE", stacktrace: { frames: [frame("check", "app:///updater.js")] } }] } }),
    ).toBe(false);
    expect(
      isStdioWriteEvent({
        exception: { values: [{ type: "Error", value: "disk full", stacktrace: { frames: [frame("Writable.write", "node:internal/streams/writable")] } }] },
      }),
    ).toBe(false);
    expect(isStdioWriteEvent({ message: "hi" })).toBe(false);
  });
});
