// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The loopback control server `cua mcp`'s open_space_viewer talks to (#8254):
// the control file, the bearer, and the routes, over real HTTP as the Rust client sends them.
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { bearerMatches, resolveSpace, startControlServer, type ControlServer } from "../src/control-server";

const spaces = [
  { id: "sp_1", name: "Build box" },
  { id: "sp_2", name: "Scratch" },
  { id: "sp_3", name: "scratch" },
];

describe("the control server", () => {
  const dirs: string[] = [];
  const stops: ControlServer[] = [];
  afterEach(async () => {
    await Promise.all(stops.splice(0).map((s) => s.stop()));
    dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true }));
  });

  async function start() {
    const dir = mkdtempSync(path.join(tmpdir(), "cua-control-"));
    dirs.push(dir);
    const file = path.join(dir, "spaces-control.json");
    const opened: string[] = [];
    const server = await startControlServer(file, { resolve: (r) => resolveSpace(spaces, r), openViewer: (id) => void opened.push(id) }, "tok", () => {});
    expect(server).not.toBeNull();
    stops.push(server!);
    const post = (route: string, body: unknown, bearer: string | null = "tok") =>
      fetch(`http://127.0.0.1:${server!.port}${route}`, {
        method: "POST",
        headers: { "content-type": "application/json", ...(bearer ? { authorization: `Bearer ${bearer}` } : {}) },
        body: JSON.stringify(body),
      });
    return { file, opened, server: server!, post };
  }

  it("writes the port and token where the client looks, and removes them on stop", async () => {
    const { file, server } = await start();
    expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ port: server.port, token: "tok" });
    if (process.platform !== "win32") expect(statSync(file).mode & 0o777).toBe(0o600);
    await server.stop();
    expect(existsSync(file)).toBe(false);
  });

  it("opens the viewer of a Space named by id or by name", async () => {
    const { opened, post } = await start();
    expect((await post("/viewer/open", { space_id: "sp_1" })).status).toBe(200);
    expect(await (await post("/viewer/open", { space_id: "BUILD BOX" })).json()).toEqual({ ok: true });
    expect(opened).toEqual(["sp_1", "sp_1"]);
  });

  it("refuses without the bearer, and says what is wrong otherwise", async () => {
    const { opened, post } = await start();
    expect((await post("/viewer/open", { space_id: "sp_1" }, null)).status).toBe(401);
    expect((await post("/viewer/open", { space_id: "sp_1" }, "other")).status).toBe(401);
    expect((await post("/viewer/open", {})).status).toBe(400);
    const missing = await post("/viewer/open", { space_id: "nope" });
    expect(missing.status).toBe(404);
    expect(await missing.json()).toEqual({ error: "no Space nope" });
    // Two Spaces share a name: not a guess.
    expect((await post("/viewer/open", { space_id: "Scratch" })).status).toBe(404);
    expect((await post("/elsewhere", {})).status).toBe(404);
    expect(opened).toEqual([]);
  });

  it("says picture in picture is not available yet instead of that the app isn't running", async () => {
    const { post } = await start();
    for (const route of ["/pip/pin", "/pip/unpin", "/window/stream"]) {
      const r = await post(route, { space_id: "sp_1" });
      expect(r.status).toBe(501);
      expect(((await r.json()) as { error: string }).error).toContain(route);
    }
  });

  it("leaves a newer app's control file alone when it stops", async () => {
    const { file, server } = await start();
    const newer = path.join(path.dirname(file), "newer.json");
    expect(newer).not.toBe(file);
    const { writeControlFile } = await import("../src/control-server");
    writeControlFile(file, 1, "newer-token");
    await server.stop();
    expect(JSON.parse(readFileSync(file, "utf8")).token).toBe("newer-token");
  });

  it("compares the bearer exactly", () => {
    expect(bearerMatches("Bearer abc", "abc")).toBe(true);
    expect(bearerMatches("Bearer abd", "abc")).toBe(false);
    expect(bearerMatches("Bearer abcd", "abc")).toBe(false);
    expect(bearerMatches(undefined, "abc")).toBe(false);
  });

  it("runs without a control file when it cannot listen or write", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cua-control-"));
    dirs.push(dir);
    // A file where the folder should be: the control file can't be written.
    const blocked = path.join(dir, "blocked");
    const { writeFileSync } = await import("node:fs");
    writeFileSync(blocked, "");
    const logs: string[] = [];
    const server = await startControlServer(path.join(blocked, "spaces-control.json"), { resolve: () => null, openViewer: () => {} }, "t", (m) => logs.push(m));
    expect(server).toBeNull();
    expect(logs[0]).toContain("control server disabled");
  });
});
