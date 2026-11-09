// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Logging must never take the app down. A packaged Windows app has no
// console, and a launched-from-the-shell app can lose its terminal: stdout
// and stderr then fail on write (EPIPE, EBADF, EIO, EAGAIN on a pipe, a destroyed stream). That
// error is emitted on the stream, and an 'error' event nobody listens for is
// an uncaught exception: Sentry reported it as a fatal, message-less "Error"
// (CUA-SPACES-DESKTOP-T, -V, -W, -X) from electron-updater's default logger,
// which is `console`. Sentry's own uncaught-exception handler then calls
// console.error, which fails the same way: 14 events in 14 seconds on a
// Linux machine (-N, -P, -Q). Three guards: a listener that swallows that class of
// stream error, console methods that cannot throw, and a logger for
// electron-updater built on them.

/** Write failures of a stream whose far end is gone. They say nothing about the app. */
const STREAM_WRITE_CODES = new Set(["EPIPE", "EBADF", "EIO", "EAGAIN", "ENOTCONN", "ECONNRESET", "ERR_STREAM_DESTROYED", "ERR_STREAM_WRITE_AFTER_END"]);

/** The class of error a closed stdout or stderr raises: a stream write failure, by code or by a failed `write` syscall. */
export function isStreamWriteError(error: unknown): boolean {
  if (error === null || typeof error !== "object") return false;
  const { code, syscall } = error as { code?: unknown; syscall?: unknown };
  return (typeof code === "string" && STREAM_WRITE_CODES.has(code)) || syscall === "write";
}

interface StdioStream {
  on(event: "error", listener: (error: Error) => void): unknown;
  listeners?(event: string): unknown[];
}

const GUARD = Symbol.for("cua.stdioGuard");

/**
 * Listens for 'error' on stdout and stderr and drops stream write errors. Any
 * other error is thrown again, so it is as uncaught as it was. Safe to call twice.
 */
export function guardStdio(proc: { stdout?: StdioStream | null; stderr?: StdioStream | null } = process): void {
  for (const stream of [proc.stdout, proc.stderr]) {
    const marked = stream as unknown as Record<symbol, unknown> | null | undefined;
    if (!stream || !marked || marked[GUARD]) continue;
    marked[GUARD] = true;
    stream.on("error", (error: Error) => {
      if (isStreamWriteError(error)) return;
      throw error;
    });
  }
}

type LogMethod = "log" | "info" | "warn" | "error" | "debug" | "trace";
const LOG_METHODS: LogMethod[] = ["log", "info", "warn", "error", "debug", "trace"];

/** Replaces the console's logging methods with ones that swallow whatever writing throws. Safe to call twice. */
export function protectConsole(target: Partial<Record<LogMethod, (...args: unknown[]) => void>> = console): void {
  for (const name of LOG_METHODS) {
    const original = target[name];
    if (typeof original !== "function" || (original as unknown as Record<symbol, unknown>)[GUARD]) continue;
    const safe = (...args: unknown[]): void => {
      try {
        original.apply(target, args);
      } catch {
        // A log line is never worth an exception.
      }
    };
    (safe as unknown as Record<symbol, unknown>)[GUARD] = true;
    target[name] = safe;
  }
}

/** The logger electron-updater gets: it never throws and never touches a stream that may be closed unguarded. */
export const safeLogger = {
  info: (message?: unknown): void => safely("info", message),
  warn: (message?: unknown): void => safely("warn", message),
  error: (message?: unknown): void => safely("error", message),
  debug: (message?: unknown): void => safely("debug", message),
};

function safely(method: "info" | "warn" | "error" | "debug", message: unknown): void {
  try {
    console[method](message);
  } catch {
    // Dropped, like any log line the console cannot take.
  }
}

/** Everything above, on the real process: first thing in the main process. */
export function protectLogging(): void {
  guardStdio();
  protectConsole();
}
