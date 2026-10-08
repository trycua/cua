// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Launch at login on Linux (an XDG autostart entry) and the About pane's
// machine line on each system.
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { LinuxAutostart } from "../src/model/login-item";
import { osLine } from "../src/model/updates";

describe("launch at login on Linux", () => {
  const dirs: string[] = [];
  const autostart = (exec = "/opt/Cua Spaces/cua-spaces") => {
    const d = mkdtempSync(path.join(tmpdir(), "cua-autostart-"));
    dirs.push(d);
    const opened: string[] = [];
    return { item: new LinuxAutostart(path.join(d, "autostart"), exec, (x) => opened.push(x)), opened };
  };
  afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

  it("writes and removes the entry, which starts the app quietly", () => {
    const { item, opened } = autostart();
    expect(item.status()).toBe("notRegistered");
    item.register();
    expect(item.status()).toBe("enabled");
    const text = readFileSync(item.file, "utf8");
    expect(text).toContain('Exec="/opt/Cua Spaces/cua-spaces" --hidden');
    expect(text).toContain("Name=Cua Spaces");
    item.openSystemSettings();
    expect(opened).toEqual([item.dir]);
    item.unregister();
    expect(item.status()).toBe("notRegistered");
  });

  it("reads an entry a desktop turned off as off", () => {
    const { item } = autostart("/usr/bin/cua-spaces");
    item.register();
    expect(readFileSync(item.file, "utf8")).toContain("Exec=/usr/bin/cua-spaces --hidden");
    writeFileSync(item.file, readFileSync(item.file, "utf8").replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled=false"));
    expect(item.status()).toBe("notRegistered");
  });
});

describe("the machine line for an issue report", () => {
  it("names the system as the Swift app does", () => {
    expect(osLine("darwin", "25.0.0", "arm64")).toBe("macOS 26.0 (arm64)");
    expect(osLine("darwin", "24.6.0", "x64")).toBe("macOS 15.6 (x86_64)");
    expect(osLine("darwin", "25.1.0", "arm64", "26.1.2")).toBe("macOS 26.1.2 (arm64)");
    expect(osLine("win32", "10.0.22631", "x64")).toBe("Windows 11 (x64)");
    expect(osLine("win32", "10.0.19045", "arm64")).toBe("Windows 10 (arm64)");
    expect(osLine("linux", "6.8.0-45-generic", "x64")).toBe("Linux 6.8.0 (x64)");
  });
});
