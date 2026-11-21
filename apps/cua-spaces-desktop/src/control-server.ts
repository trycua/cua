// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app's loopback control server: how `cua mcp` (and any other Spaces host
// using `cua_spaces::operator::ControlServerDisplay`) asks this app to present
// a Space on the operator's desktop, such as the MCP `open_space_viewer` tool.
// The macOS app (SpacesControlServer.swift) and the Tauri app (control.rs)
// serve the same protocol; without it the client reports "the Cua Spaces app
// isn't running" on Windows and Linux (#8254).
//
// It binds 127.0.0.1 on an ephemeral port, writes `{port, token}` to
// `<cua home>/spaces-control.json` (owner-only) and requires
// `Authorization: Bearer <token>` on every request. The client reads the file
// on every call, so a restart that rotates the port and the token is picked up.
//
//   POST /viewer/open  {space_id}   opens the Space's viewer window
//
// `/pip/pin`, `/pip/unpin` and `/window/stream` answer 501 until this app
// presents them for an agent; the client reports that status instead of the
// misleading "app isn't running".
import { randomBytes, timingSafeEqual } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import * as http from "node:http";
import * as path from "node:path";

export interface ControlActions {
  /** The canonical id of the Space a request names (its id, or its name), or null when there is none. */
  resolve(requested: string): string | null;
  /** Shows the Space's viewer window. */
  openViewer(spaceId: string): void;
}

export interface ControlServer {
  /** The port it listens on, once started. */
  readonly port: number;
  /** Stops listening and removes the control file, so a client says the app isn't running instead of reaching a dead port. */
  stop(): Promise<void>;
}

/** A request body past this many bytes is refused. */
const MAX_BODY = 1024 * 1024;

const NOT_YET = ["/pip/pin", "/pip/unpin", "/window/stream"];

/** `Authorization: Bearer <token>`, compared in constant time. */
export function bearerMatches(header: string | undefined, token: string): boolean {
  const given = Buffer.from(header ?? "");
  const expected = Buffer.from(`Bearer ${token}`);
  return given.length === expected.length && timingSafeEqual(given, expected);
}

/** The Space a request names: its id, else the one Space of that name (any case). */
export function resolveSpace(spaces: readonly { id: string; name: string }[], requested: string): string | null {
  const byId = spaces.find((s) => s.id === requested);
  if (byId) return byId.id;
  const wanted = requested.toLowerCase();
  const named = spaces.filter((s) => s.name.toLowerCase() === wanted);
  return named.length === 1 ? (named[0]?.id ?? null) : null;
}

/** Writes `{port, token}` with owner-only permissions from creation, by renaming a private file over any earlier one. */
export function writeControlFile(file: string, port: number, token: string): void {
  mkdirSync(path.dirname(file), { recursive: true });
  const temporary = `${file}.${randomBytes(6).toString("hex")}.tmp`;
  try {
    writeFileSync(temporary, `${JSON.stringify({ port, token })}\n`, { mode: 0o600 });
    renameSync(temporary, file);
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
}

function reply(res: http.ServerResponse, status: number, body: unknown): void {
  const text = JSON.stringify(body);
  res.writeHead(status, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(text), Connection: "close" });
  res.end(text);
}

function readBody(req: http.IncomingMessage): Promise<string | null> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_BODY) {
        resolve(null);
        req.destroy();
      } else chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", () => resolve(null));
  });
}

export function route(actions: ControlActions, method: string, url: string, body: string): { status: number; body: unknown } {
  if (method === "POST" && url === "/viewer/open") {
    let requested: unknown;
    try {
      requested = (JSON.parse(body) as { space_id?: unknown }).space_id;
    } catch {
      requested = undefined;
    }
    if (typeof requested !== "string" || requested === "") return { status: 400, body: { error: "space_id required" } };
    const id = actions.resolve(requested);
    if (id === null) return { status: 404, body: { error: `no Space ${requested}` } };
    actions.openViewer(id);
    return { status: 200, body: { ok: true } };
  }
  if (method === "POST" && NOT_YET.includes(url)) return { status: 501, body: { error: `${url} is not available in this app yet` } };
  return { status: 404, body: { error: "not found" } };
}

/**
 * Starts listening and writes the control file once the port is known. A
 * failure is logged and leaves the app running without the server: only
 * agent-driven presentation is lost.
 */
export async function startControlServer(
  file: string,
  actions: ControlActions,
  token: string = randomBytes(32).toString("hex"),
  log: (message: string) => void = (m) => console.warn(`[cua-spaces] ${m}`),
): Promise<ControlServer | null> {
  const server = http.createServer((req, res) => {
    if (!bearerMatches(req.headers.authorization, token)) return reply(res, 401, { error: "unauthorized" });
    void readBody(req).then((body) => {
      if (body === null) return reply(res, 400, { error: "request body too large" });
      try {
        const out = route(actions, req.method ?? "", req.url ?? "", body);
        reply(res, out.status, out.body);
      } catch (error) {
        log(`control request failed: ${error instanceof Error ? error.message : String(error)}`);
        reply(res, 500, { error: "failed" });
      }
    });
  });
  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;
    writeControlFile(file, port, token);
    return {
      port,
      stop: () =>
        new Promise<void>((resolve) => {
          // Only the file this server wrote: a newer app's must stay.
          try {
            if ((JSON.parse(readFileSync(file, "utf8")) as { token?: string }).token === token) rmSync(file, { force: true });
          } catch {
            // Already gone.
          }
          server.close(() => resolve());
          server.closeAllConnections();
        }),
    };
  } catch (error) {
    log(`control server disabled: ${error instanceof Error ? error.message : String(error)}`);
    server.close();
    return null;
  }
}
