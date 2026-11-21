// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An update check on a machine whose stdout is gone (a Windows app has no
// console): electron-updater logs through its logger, the console throws, and
// the check must still finish, with nothing fatal reported.
import { EventEmitter } from "node:events";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({
  app: { getVersion: () => "0.22.3", isPackaged: true, focus: vi.fn() },
  BrowserWindow: { getFocusedWindow: () => null, getAllWindows: () => [] },
  dialog: { showMessageBox: vi.fn(async () => ({ response: 0 })) },
}));
vi.mock("../src/settings", () => ({ readSettings: () => ({}), writeSettings: vi.fn() }));
class FakeAutoUpdater extends EventEmitter {
  logger: { info(m?: unknown): void } | null = null;
  channel = "";
  allowPrerelease = false;
  allowDowngrade = false;
  autoDownload = false;
  autoInstallOnAppQuit = false;
  setFeedURL = vi.fn();
  behaviour: () => Promise<unknown> = async () => null;
  async checkForUpdates() {
    return this.behaviour();
  }
}
const fake = new FakeAutoUpdater();
vi.mock("../src/electron-updater", () => ({ electronUpdater: async () => ({ autoUpdater: fake }) }));

import { ElectronUpdater } from "../src/updater";
import { setUpdateFailureSink, type UpdateFailureReport } from "../src/update-report";

// What crash-reporting-main's sink does with a report, minus the SDK: the handled warning's shape.
const reports: UpdateFailureReport[] = [];

const settled = async (u: ElectronUpdater) => {
  for (let i = 0; i < 50 && !u.canCheck; i++) await new Promise((r) => setImmediate(r));
};

describe("update check with a closed stdout", () => {
  const platform = Object.getOwnPropertyDescriptor(process, "platform")!;
  beforeEach(() => {
    // The report is of Windows, where the stable channel is served.
    Object.defineProperty(process, "platform", { value: "win32" });
    reports.length = 0;
    setUpdateFailureSink((r) => reports.push(r));
    fake.removeAllListeners();
    vi.spyOn(console, "info").mockImplementation(() => {
      throw Object.assign(new Error("write EPIPE"), { code: "EPIPE" });
    });
    vi.spyOn(console, "warn").mockImplementation(() => {
      throw Object.assign(new Error("write EBADF"), { code: "EBADF" });
    });
  });
  afterEach(() => {
    setUpdateFailureSink(null);
    Object.defineProperty(process, "platform", platform);
    vi.restoreAllMocks();
  });

  it("finds the update, logging through a logger that cannot throw, and reports nothing", async () => {
    fake.behaviour = async () => {
      fake.logger?.info("Found version 0.23.0");
      fake.emit("update-available", { version: "0.23.0" });
      return { downloadPromise: Promise.resolve([]) };
    };
    const u = new ElectronUpdater();
    u.checkNow();
    await settled(u);
    expect(fake.logger).not.toBeNull();
    expect(u.state.status).toBe("available");
    expect(reports).toEqual([]);
  });

  it("a real failure is a handled warning with its reason, not a fatal with no message", async () => {
    fake.behaviour = async () => {
      throw Object.assign(new Error("Cannot download https://github.com/x/latest.yml, status 404"), { statusCode: 404 });
    };
    const u = new ElectronUpdater();
    u.checkNow();
    await settled(u);
    expect(u.state.status).toBe("error");
    expect(reports).toHaveLength(1);
    expect(reports[0]?.kind).toBe("http-404");
    expect(reports[0]?.reason).toContain("404");
    expect(reports[0]?.error.message).toContain("404");
  });
});
