// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The refresh after an update (the SwiftUI app's UpdatesTests, agents half):
// `cua agents update` is run with the bundled `cua`, its failures read from
// the CLI's JSON, and the core says whether this launch follows an update and
// what the notice says. No process runs. Skipped when this machine's native
// directory was not built.
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { cliJson, failures, recordLaunch, refreshAfterUpdate, updateAgents, type CommandResult } from "../src/model/update-refresh";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

describe.skipIf(!built)("the refresh after an update", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-refresh-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  const ran: string[][] = [];
  const runner = (result: CommandResult) => async (_exe: string, args: string[]) => (ran.push(args), result);
  const ok: CommandResult = { status: 0, output: '{"outcomes":[]}', timedOut: false };
  beforeEach(() => (ran.length = 0));

  it("refreshes quietly", async () => {
    expect(await refreshAfterUpdate(native, "/cua", null, runner(ok))).toBeNull();
    expect(ran).toEqual([["--json", "agents", "update"]]);
  });

  it("shows one notice for a failed agents update", async () => {
    const output = JSON.stringify({ outcomes: [{ item: "cua", change: "updated", detail: "" }, { item: "cua-spaces", change: "failed", detail: "permission denied" }] }, null, 2);
    const notice = await refreshAfterUpdate(native, "/cua", null, runner({ status: 1, output, timedOut: false }));
    expect(notice).toBe("Cua Spaces updated, but could not refresh the Cua skills in your AI agents (cua-spaces: permission denied). Run `cua agents update` to try again.");
  });

  it("says when the update timed out or exited without saying why", async () => {
    expect(await updateAgents("/cua", runner({ status: null, output: "", timedOut: true }))).toBe("`cua agents update` did not finish within 2 minutes");
    expect(await updateAgents("/cua", runner({ status: 3, output: "boom", timedOut: false }))).toBe("`cua agents update` exited with status 3");
    expect(failures('{"outcomes":[{"item":"a","change":"failed","detail":""},{"item":"b","change":"failed","detail":"x"}]}')).toBe("a; b: x");
    expect(failures('{"outcomes":[]}')).toBeNull();
  });

  it("reads the CLI's pretty JSON, after other lines too", () => {
    expect(cliJson('{\n  "pid": 7\n}\n')?.pid).toBe(7);
    expect(cliJson('warning: something\n{\n  "pid": 8\n}')?.pid).toBe(8);
    expect(cliJson("no json")).toBeNull();
  });

  it("refreshes after a change of version, never on a fresh install or a relaunch", () => {
    const file = path.join(mkdtempSync(path.join(home, "launch-")), "app-settings.json");
    // A fresh install records its version and does not refresh.
    expect(recordLaunch(native, file, "0.2.0", "", false)).toBe(false);
    expect(recordLaunch(native, file, "0.2.0", "", true)).toBe(false);
    expect(recordLaunch(native, file, "0.3.0", "", true)).toBe(true);
    expect(recordLaunch(native, file, "0.3.0", "", true)).toBe(false);
    expect(native.appSettingsLoad(file).lastSeenVersion).toBe("0.3.0");
    // A launch with no record after the first run is an update from a build that kept none.
    const bare = path.join(mkdtempSync(path.join(home, "launch-")), "app-settings.json");
    writeFileSync(bare, "{}");
    expect(recordLaunch(native, bare, "0.3.0", "", true)).toBe(true);
  });

  it("takes over the Swift app's record: the same version and build is no update", () => {
    // What the Swift app wrote (`UpdateRefresh.record` with its CFBundleVersion), copied by the migration.
    const file = path.join(mkdtempSync(path.join(home, "launch-")), "app-settings.json");
    writeFileSync(file, JSON.stringify({ lastSeenVersion: "0.7.2 (0.7.2.41)" }));
    expect(recordLaunch(native, file, "0.7.2", "0.7.2.41", true)).toBe(false);
    expect(native.appSettingsLoad(file).lastSeenVersion).toBe("0.7.2 (0.7.2.41)");
    // A newer build is an update, recorded in the same form.
    expect(recordLaunch(native, file, "0.7.2", "0.7.2.42", true)).toBe(true);
    expect(native.appSettingsLoad(file).lastSeenVersion).toBe("0.7.2 (0.7.2.42)");
  });
});
