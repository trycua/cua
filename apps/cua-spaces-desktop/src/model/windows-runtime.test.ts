// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import {
  ELEVATED_SETUP_SCRIPT,
  WindowsRuntime,
  parseFeatures,
  parseSetupResult,
  parseVirtualization,
  setupFailedText,
  windowsRuntimeAdvice,
  type WindowsRuntimeFacts,
  type WindowsSystem,
} from "./windows-runtime";

const CLEAN: WindowsRuntimeFacts = {
  wslReady: false,
  virtualization: "on",
  restartPending: false,
  features: { editionId: null, virtualMachinePlatform: "unknown", hypervisorPlatform: "unknown", subsystemLinux: "unknown" },
};
const IDLE = { kind: "idle" } as const;

describe("windowsRuntimeAdvice", () => {
  it("offers one click of Linux support on a clean PC, never Docker Desktop", () => {
    const a = windowsRuntimeAdvice(CLEAN, IDLE);
    expect(a.reason).toMatch(/built-in Linux support \(WSL 2\)/);
    expect(a.fix).toMatchObject({ action: "setup-wsl", label: "Set up Linux support", busy: false, termsUrl: null });
    expect(a.fix?.note).toMatch(/administrator approval/);
    expect(JSON.stringify(a)).not.toMatch(/docker/i);
  });

  it("says virtualization is off and offers nothing to install", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, virtualization: "off" }, IDLE);
    expect(a.reason).toMatch(/virtualization is turned off/);
    expect(a.reason).toMatch(/another machine/);
    expect(a.fix).toBeNull();
  });

  it("asks for a restart after the setup, with no button, and says Cua Spaces carries on by itself", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, wslReady: true, restartPending: true }, IDLE);
    expect(a.reason).toMatch(/^Restart this PC/);
    expect(a.reason).toMatch(/opens again by itself/);
    expect(a.fix).toBeNull();
  });

  it("shows the setup running as a busy button", () => {
    const a = windowsRuntimeAdvice(CLEAN, { kind: "installing" });
    expect(a.fix).toMatchObject({ action: null, busy: true });
  });

  it("offers the setup again, with the reason, when it failed", () => {
    const a = windowsRuntimeAdvice(CLEAN, { kind: "failed", message: "boom" });
    expect(a.reason).toBe("boom");
    expect(a.fix?.action).toBe("setup-wsl");
  });

  it("offers the setup when WSL answers but Windows features are off", () => {
    const features = { ...CLEAN.features, virtualMachinePlatform: "disabled" as const };
    expect(windowsRuntimeAdvice({ ...CLEAN, wslReady: true, features }, IDLE).fix?.action).toBe("setup-wsl");
  });

  it("offers nothing once WSL 2 is ready: the built-in runtime takes over", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, wslReady: true }, IDLE);
    expect(a.fix).toBeNull();
  });
});

describe("parsers", () => {
  it("reads the virtualization answer", () => {
    expect(parseVirtualization("True|False")).toBe("on");
    expect(parseVirtualization("False|True")).toBe("on");
    expect(parseVirtualization("False|False")).toBe("off");
    expect(parseVirtualization("")).toBe("unknown");
  });

  it("reads the Windows features", () => {
    expect(parseFeatures('{"virtualMachinePlatform":2,"subsystemLinux":1,"editionId":"Core"}')).toMatchObject({
      virtualMachinePlatform: "disabled",
      subsystemLinux: "enabled",
      editionId: "Core",
    });
    expect(parseFeatures("nonsense").virtualMachinePlatform).toBe("unknown");
  });

  it("reads the elevated script's result", () => {
    expect(parseSetupResult("0|True")).toEqual({ code: 0, wslReady: true });
    expect(parseSetupResult("3010|False")).toEqual({ code: 3010, wslReady: false });
    expect(parseSetupResult("")).toBeNull();
  });

  it("turns on WSL 2 with no distribution and no Docker", () => {
    expect(ELEVATED_SETUP_SCRIPT).toContain("wsl.exe --install --no-distribution --web-download");
    expect(ELEVATED_SETUP_SCRIPT).not.toMatch(/docker/i);
  });

  it("words a declined prompt plainly", () => {
    expect(setupFailedText(new Error("cancelled"))).toMatch(/prompt was declined/);
    expect(setupFailedText(new Error("boom"))).toMatch(/wsl --install --no-distribution/);
  });
});

function fakeSystem(state: {
  wsl?: boolean;
  hv?: string;
  features?: string;
  rebootPending?: boolean;
  result?: string;
  elevated?: { code: number; stderr?: string };
}) {
  const ran: string[] = [];
  const files = new Map<string, string>();
  const sys: WindowsSystem = {
    async powershell(script) {
      ran.push(script);
      if (script.includes("--version")) return { code: state.wsl ? 0 : 1, stdout: "", stderr: "" };
      if (script.includes("Win32_OptionalFeature")) return { code: 0, stdout: state.features ?? "{}", stderr: "" };
      if (script.includes("Win32_ComputerSystem")) return { code: 0, stdout: state.hv ?? "True|True", stderr: "" };
      if (script.includes("RebootPending")) return { code: 0, stdout: state.rebootPending ? "pending" : "clear", stderr: "" };
      if (script.includes("RunOnce")) return { code: 0, stdout: "", stderr: "" };
      if (script.includes("-Verb RunAs")) {
        const r = state.elevated ?? { code: 0 };
        const result = [...files.keys()].find((k) => k.endsWith(".result"));
        if (result && state.result !== undefined) files.set(result, state.result);
        return { code: r.code, stdout: "", stderr: r.stderr ?? "" };
      }
      return { code: 1, stdout: "", stderr: "unexpected" };
    },
    async writeFile(path, text) {
      files.set(path, text);
    },
    readFile: async (path) => files.get(path) ?? "",
    launch(path) {
      ran.push(`launch ${path}`);
    },
  };
  return { sys, ran, files };
}

const settle = () => new Promise((r) => setTimeout(r, 0));
const settleAll = async () => {
  for (let i = 0; i < 6; i++) await settle();
};

describe("WindowsRuntime", () => {
  it("reads WSL, virtualization, restart and features from the PC", async () => {
    const f = fakeSystem({ wsl: false, hv: "False|False", rebootPending: true, features: '{"virtualMachinePlatform":2}' });
    const facts = await new WindowsRuntime(f.sys, "C:\\tmp").facts();
    expect(facts).toMatchObject({ wslReady: false, virtualization: "off", restartPending: true });
    expect(facts.features.virtualMachinePlatform).toBe("disabled");
  });

  it("sets WSL 2 up through one UAC prompt and is done when WSL answers", async () => {
    const f = fakeSystem({ result: "0|True" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    expect((await w.advice()).fix?.action).toBe("setup-wsl");
    w.run("setup-wsl");
    await settleAll();
    const elevated = f.ran.filter((s) => s.includes("-Verb RunAs"));
    expect(elevated).toHaveLength(1);
    expect(elevated[0]).toContain("'-File','C:\\tmp");
    expect(f.files.get([...f.files.keys()].find((k) => k.endsWith(".ps1"))!)).toBe(ELEVATED_SETUP_SCRIPT);
    expect(f.ran.some((s) => s.includes("RunOnce"))).toBe(false);
    expect(f.ran.some((s) => s.startsWith("launch "))).toBe(false);
  });

  it("asks for a restart and registers Cua Spaces to open again when Windows turned features on", async () => {
    const f = fakeSystem({ result: "0|False" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp", "C:\\Apps\\Cua Spaces.exe");
    w.run("setup-wsl");
    await settleAll();
    const a = await w.advice();
    expect(a.reason).toMatch(/^Restart this PC/);
    expect(a.fix).toBeNull();
    const resume = f.ran.find((s) => s.includes("RunOnce"))!;
    expect(resume).toContain("CuaSpacesResume");
    expect(resume).toContain("C:\\Apps\\Cua Spaces.exe");
  });

  it("asks for a restart when the setup exits with the restart code", async () => {
    const f = fakeSystem({ result: "3010|True" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("setup-wsl");
    await settleAll();
    expect((await w.advice()).reason).toMatch(/^Restart this PC/);
  });

  it("says so when the UAC prompt is declined, and offers the setup again", async () => {
    const f = fakeSystem({ elevated: { code: 1, stderr: "This command cannot be run due to the error: The operation was canceled by the user." } });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("setup-wsl");
    await settleAll();
    const a = await w.advice();
    expect(a.reason).toMatch(/prompt was declined/);
    expect(a.fix?.action).toBe("setup-wsl");
  });

  it("reports a setup that wrote no result", async () => {
    const f = fakeSystem({});
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("setup-wsl");
    await settleAll();
    expect((await w.advice()).reason).toMatch(/Couldn't set up Linux support: the setup didn't finish/);
  });

  it("forgets the fix once a runtime answers", async () => {
    const f = fakeSystem({ elevated: { code: 1, stderr: "canceled by the user" } });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("setup-wsl");
    await settleAll();
    expect((await w.advice()).reason).toMatch(/declined/);
    w.settled();
    expect((await w.advice()).reason).not.toMatch(/declined/);
  });

  it("opens the Windows features dialog on request", async () => {
    const f = fakeSystem({});
    new WindowsRuntime(f.sys, "C:\\tmp").run("open-windows-features");
    expect(f.ran).toContain("launch optionalfeatures.exe");
  });
});
