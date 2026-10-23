// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import {
  ELEVATED_INSTALL_SCRIPT,
  START_PATIENCE_MS,
  WindowsRuntime,
  currentUser,
  installFailedText,
  parseInstallResult,
  parseFeatures,
  parseVirtualization,
  signedByDocker,
  windowsRuntimeAdvice,
  type WindowsRuntimeFacts,
  type WindowsSystem,
} from "./windows-runtime";

const CLEAN: WindowsRuntimeFacts = {
  dockerInstalled: false, dockerRunning: false, virtualization: "on", restartPending: false,
  features: { editionId: null, virtualMachinePlatform: "unknown", hypervisorPlatform: "unknown", subsystemLinux: "unknown" },
};
const IDLE = { kind: "idle" } as const;

describe("windowsRuntimeAdvice", () => {
  it("offers the install on a clean PC (A10: no Docker Desktop)", () => {
    const a = windowsRuntimeAdvice(CLEAN, IDLE, 0);
    expect(a.reason).toMatch(/Docker Desktop, which isn't installed/);
    expect(a.fix).toMatchObject({ action: "install-docker", label: "Install Docker Desktop", busy: false });
    expect(a.fix?.note).toMatch(/administrator approval/);
    expect(a.fix?.termsUrl).toMatch(/^https:\/\/www\.docker\.com\//);
  });

  it("says virtualization is off and offers nothing to install", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, virtualization: "off" }, IDLE, 0);
    expect(a.reason).toMatch(/virtualization is turned off/);
    expect(a.reason).toMatch(/another computer/);
    expect(a.fix).toBeNull();
  });

  it("asks for a restart after the install, with no button", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, dockerInstalled: true, restartPending: true }, IDLE, 0);
    expect(a.reason).toMatch(/^Restart this PC/);
    expect(a.fix).toBeNull();
  });

  it("offers to start an installed Docker Desktop that isn't running", () => {
    const a = windowsRuntimeAdvice({ ...CLEAN, dockerInstalled: true }, IDLE, 0);
    expect(a.reason).toMatch(/installed but not running/);
    expect(a.fix).toMatchObject({ action: "start-docker", busy: false });
  });

  it("waits while Docker Desktop starts, then says where to look", () => {
    const facts = { ...CLEAN, dockerInstalled: true, dockerRunning: true };
    const soon = windowsRuntimeAdvice(facts, { kind: "starting", since: 0 }, 10_000);
    expect(soon.reason).toMatch(/is starting/);
    expect(soon.fix).toMatchObject({ action: null, busy: true });
    const late = windowsRuntimeAdvice(facts, { kind: "starting", since: 0 }, START_PATIENCE_MS + 1);
    expect(late.reason).toMatch(/finish it there/);
  });

  it("shows the download's progress and the install as busy", () => {
    expect(windowsRuntimeAdvice(CLEAN, { kind: "downloading", percent: 42 }, 0).fix).toMatchObject({ label: "Downloading Docker Desktop… 42%", busy: true, action: null });
    expect(windowsRuntimeAdvice(CLEAN, { kind: "installing" }, 0).fix).toMatchObject({ busy: true, action: null });
  });

  it("says why an install failed and offers it again", () => {
    const a = windowsRuntimeAdvice(CLEAN, { kind: "failed", message: installFailedText(new Error("cancelled")) }, 0);
    expect(a.reason).toMatch(/prompt was declined/);
    expect(a.fix?.action).toBe("install-docker");
  });
});

describe("probe parsing", () => {
  it("reads edition and feature states independently, without treating absent features as disabled", () => {
    for (const [input, expected] of [[1, "enabled"], [2, "disabled"], [3, "unknown"], [4, "unknown"], [null, "unknown"], ["2", "unknown"], [0, "unknown"]] as const) {
      expect(parseFeatures(JSON.stringify({ editionId: "Core", virtualMachinePlatform: input, hypervisorPlatform: input, subsystemLinux: input }))).toEqual({
        editionId: "Core", virtualMachinePlatform: expected, hypervisorPlatform: expected, subsystemLinux: expected,
      });
    }
    expect(parseFeatures('{"editionId":"Core"}')).toEqual({ ...CLEAN.features, editionId: "Core" });
    expect(parseFeatures('{"hypervisorPlatform":2}')).toEqual({ ...CLEAN.features, hypervisorPlatform: "disabled" });
    for (const input of ["", "not JSON", "null", "[]", "42", '{"editionId":12}']) expect(parseFeatures(input)).toEqual(CLEAN.features);
  });

  it("counts a running hypervisor as virtualization on (it hides the firmware flag)", () => {
    expect(parseVirtualization("True|False")).toBe("on");
    expect(parseVirtualization("False|True")).toBe("on");
    expect(parseVirtualization("False|False")).toBe("off");
    expect(parseVirtualization("")).toBe("unknown");
  });

  it("accepts only Docker's valid signature", () => {
    const subject = "CN=Docker Inc, O=Docker Inc, L=Palo Alto, S=California, C=US";
    expect(signedByDocker(`Valid|${subject}`)).toBe(true);
    expect(signedByDocker(`HashMismatch|${subject}`)).toBe(false);
    expect(signedByDocker("Valid|CN=Someone Else, O=Evil Corp")).toBe(false);
    expect(signedByDocker("NotSigned|")).toBe(false);
  });

  it("reads the elevated script's result", () => {
    expect(parseInstallResult("0|False|True\r\n")).toEqual({ installer: 0, addedToGroup: false, wslReady: true });
    expect(parseInstallResult("3010|True|False")).toEqual({ installer: 3010, addedToGroup: true, wslReady: false });
    expect(parseInstallResult("")).toBeNull();
  });

  it("names the signed-in user for docker-users", () => {
    expect(currentUser({ USERDOMAIN: "PC", USERNAME: "ana" })).toBe("PC\\ana");
    expect(currentUser({ USERNAME: "ana" })).toBe("ana");
    expect(currentUser({})).toBe("");
  });

  it("installs with Docker's documented silent flags and the WSL package", () => {
    expect(ELEVATED_INSTALL_SCRIPT).toContain("'install','--quiet','--accept-license','--backend=wsl-2'");
    expect(ELEVATED_INSTALL_SCRIPT).toContain("Add-LocalGroupMember -Group 'docker-users'");
    expect(ELEVATED_INSTALL_SCRIPT).toContain("wsl.exe --install --no-distribution --web-download");
  });
});

/** A Windows that answers from `state`, recording what was run. */
function fakeSystem(state: {
  installed?: boolean;
  running?: boolean;
  hv?: string;
  features?: string;
  featureFailure?: "exit" | "throw";
  rebootPending?: boolean;
  signature?: string;
  result?: string;
  elevated?: { code: number; stderr?: string };
}) {
  const ran: string[] = [];
  const files = new Map<string, string>();
  const sys: WindowsSystem = {
    exists: async () => state.installed ?? false,
    running: async () => state.running ?? false,
    async powershell(script) {
      ran.push(script);
      if (script.includes("Win32_OptionalFeature")) {
        if (state.featureFailure === "throw") throw new Error("unavailable");
        if (state.featureFailure === "exit") return { code: 1, stdout: state.features ?? "", stderr: "unavailable" };
        if (state.features !== undefined) return { code: 0, stdout: state.features, stderr: "" };
      }
      if (script.includes("Win32_ComputerSystem")) return { code: 0, stdout: state.hv ?? "True|True", stderr: "" };
      if (script.includes("RebootPending")) return { code: 0, stdout: state.rebootPending ? "pending" : "clear", stderr: "" };
      if (script.includes("Get-AuthenticodeSignature")) return { code: 0, stdout: state.signature ?? "Valid|CN=Docker Inc, O=Docker Inc", stderr: "" };
      if (script.includes("-Verb RunAs")) {
        const r = state.elevated ?? { code: 0 };
        const result = [...files.keys()].find((k) => k.endsWith(".result"));
        if (result && state.result !== undefined) files.set(result, state.result);
        return { code: r.code, stdout: "", stderr: r.stderr ?? "" };
      }
      return { code: 1, stdout: "", stderr: "unexpected" };
    },
    async download(_url, _to, progress) {
      ran.push("download");
      progress(50);
    },
    async writeFile(path, text) {
      files.set(path, text);
    },
    readFile: async (path) => files.get(path) ?? "",
    launch(path) {
      ran.push(`launch ${path}`);
      state.running = true;
    },
  };
  return { sys, ran, files };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("WindowsRuntime", () => {
  it("offers Windows Features for disabled Virtual Machine Platform on Home", async () => {
    const f = fakeSystem({ installed: true, features: JSON.stringify({ editionId: "Core", virtualMachinePlatform: 2 }) });
    const a = await new WindowsRuntime(f.sys, "C:\\tmp").advice();
    expect(a.reason).toContain("Virtual Machine Platform");
    expect(a.fix).toMatchObject({ action: "open-windows-features", label: "Open Windows Features", busy: false, termsUrl: null });
    expect(a.fix?.note).toContain("restart this PC");
    expect(JSON.stringify(a)).not.toContain("Hyper-V");
  });

  it("refreshes feature facts and preserves Docker, restart and active-install precedence across editions", async () => {
    for (const editionId of ["Core", "CoreSingleLanguage", "Professional", "Enterprise"]) {
      for (const feature of ["virtualMachinePlatform", "hypervisorPlatform", "subsystemLinux"]) {
        const state = { installed: true, features: JSON.stringify({ editionId, [feature]: 2 }) };
        const f = fakeSystem(state);
        const w = new WindowsRuntime(f.sys, "C:\\tmp");
        expect((await w.facts()).features).toMatchObject({ editionId, [feature]: "disabled" });
        const a = await w.advice();
        expect(a.fix?.action).toBe(feature === "hypervisorPlatform" ? "start-docker" : "open-windows-features");
        if (feature === "subsystemLinux") expect(a.reason).toContain("Windows Subsystem for Linux");
        const facts = await w.facts();
        const alternatives = [
          windowsRuntimeAdvice({ ...facts, dockerInstalled: false }, IDLE, 0),
          windowsRuntimeAdvice({ ...facts, restartPending: true }, IDLE, 0),
          windowsRuntimeAdvice(facts, { kind: "downloading", percent: 10 }, 0),
          windowsRuntimeAdvice(facts, { kind: "installing" }, 0),
        ] as const;
        expect(alternatives[0].fix?.action).toBe("install-docker");
        expect(alternatives[1].reason).toMatch(/^Restart this PC/);
        expect(alternatives[1].fix).toBeNull();
        expect(alternatives.slice(2).every((a) => a.fix?.busy)).toBe(true);
        expect(JSON.stringify([a, ...alternatives])).not.toContain("Hyper-V");
        state.features = JSON.stringify({ editionId, [feature]: 1 });
        expect((await w.facts()).features).toMatchObject({ [feature]: "enabled" });
        expect((await w.advice()).fix?.action).toBe("start-docker");
      }
    }
  });

  it("returns unknown facts on failed or malformed probes", async () => {
    for (const featureFailure of [undefined, "exit", "throw"] as const) {
      const f = fakeSystem({ installed: true, features: "invalid JSON", featureFailure });
      const w = new WindowsRuntime(f.sys, "C:\\tmp");
      expect((await w.facts()).features).toEqual(CLEAN.features);
      expect((await w.advice()).fix?.action).toBe("start-docker");
    }
  });

  it("opens only Windows Features without starting an install or changing phase", async () => {
    const f = fakeSystem({ installed: true });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("open-windows-features");
    await settle();
    expect(f.ran).toEqual(["launch optionalfeatures.exe"]);
    f.sys.running = async () => false;
    expect((await w.advice()).fix?.action).toBe("start-docker");
  });

  it("skips feature probing while a download or install is active", async () => {
    const f = fakeSystem({ features: '{"virtualMachinePlatform":2}' });
    let finishDownload!: () => void;
    let finishInstall!: () => void;
    f.sys.download = () => new Promise<void>((resolve) => { finishDownload = resolve; });
    const powershell = f.sys.powershell;
    f.sys.powershell = (script, ms) => script.includes("-Verb RunAs") ? new Promise((resolve) => { finishInstall = () => resolve({ code: 0, stdout: "", stderr: "" }); }) : powershell(script, ms);
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    expect((await w.facts()).features.virtualMachinePlatform).toBe("disabled");
    f.ran.length = 0;
    w.run("install-docker");
    expect((await w.advice()).fix?.label).toContain("Downloading");
    expect(f.ran.some((s) => s.includes("Win32_OptionalFeature"))).toBe(false);
    finishDownload();
    await settle();
    expect((await w.advice()).fix?.label).toContain("Installing");
    expect(f.ran.some((s) => s.includes("Win32_OptionalFeature"))).toBe(false);
    finishInstall();
    await settle();
  });

  it("installs, then starts Docker Desktop when no restart is needed", async () => {
    const f = fakeSystem({ result: "0|False|True" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp", () => 1_000);
    expect((await w.advice()).fix?.action).toBe("install-docker");
    w.run("install-docker");
    await settle();
    await settle();
    await settle();
    expect(f.ran).toContain("download");
    const elevated = f.ran.find((s) => s.includes("-Verb RunAs"))!;
    expect(elevated).toContain("'-File','C:\\tmp");
    expect(f.files.get([...f.files.keys()].find((k) => k.endsWith(".ps1"))!)).toBe(ELEVATED_INSTALL_SCRIPT);
    expect(f.ran.some((s) => s.startsWith("launch ") && s.endsWith("Docker Desktop.exe"))).toBe(true);
    f.sys.exists = async () => true;
    expect((await w.advice()).fix).toMatchObject({ busy: true, label: "Waiting for Docker Desktop…" });
    w.settled();
  });

  it("asks for a restart when Windows turned WSL 2 on", async () => {
    const f = fakeSystem({ result: "0|False|False" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("install-docker");
    for (let i = 0; i < 5; i++) await settle();
    f.sys.exists = async () => true;
    const a = await w.advice();
    expect(a.reason).toMatch(/^Restart this PC/);
    expect(f.ran.some((s) => s.startsWith("launch "))).toBe(false);
  });

  it("asks for a restart when the user just joined docker-users", async () => {
    const f = fakeSystem({ result: "0|True|True" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("install-docker");
    for (let i = 0; i < 5; i++) await settle();
    expect((await w.advice()).reason).toMatch(/^Restart this PC/);
  });

  it("refuses an installer not signed by Docker", async () => {
    const f = fakeSystem({ signature: "Valid|CN=Mallory, O=Mallory" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("install-docker");
    for (let i = 0; i < 5; i++) await settle();
    expect((await w.advice()).reason).toMatch(/isn't signed by Docker/);
    expect(f.ran.some((s) => s.includes("-Verb RunAs"))).toBe(false);
  });

  it("says so when the UAC prompt is declined", async () => {
    const f = fakeSystem({ elevated: { code: 1, stderr: "This command cannot be run due to the error: The operation was canceled by the user." } });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    w.run("install-docker");
    for (let i = 0; i < 5; i++) await settle();
    const a = await w.advice();
    expect(a.reason).toMatch(/prompt was declined/);
    expect(a.fix?.action).toBe("install-docker");
  });

  it("starts an installed Docker Desktop", async () => {
    const f = fakeSystem({ installed: true });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    expect((await w.advice()).fix?.action).toBe("start-docker");
    w.run("start-docker");
    expect(f.ran.some((s) => s.startsWith("launch "))).toBe(true);
    expect((await w.advice()).fix?.busy).toBe(true);
  });

  it("reads virtualization once", async () => {
    const f = fakeSystem({ hv: "False|False" });
    const w = new WindowsRuntime(f.sys, "C:\\tmp");
    expect((await w.advice()).reason).toMatch(/virtualization is turned off/);
    await w.advice();
    expect(f.ran.filter((s) => s.includes("Win32_ComputerSystem"))).toHaveLength(1);
  });
});

describe("nodeWindowsSystem.download", () => {
  it("writes every byte (no chunk lost before the file opens) and reports progress", async () => {
    const { mkdtemp, readFile } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const { nodeWindowsSystem } = await import("./windows-runtime");
    const chunks = Array.from({ length: 64 }, (_, i) => new Uint8Array(4096).fill(i));
    const total = chunks.length * 4096;
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (const ch of chunks) c.enqueue(ch);
        c.close();
      },
    });
    const fetchImpl = (async () => new Response(body, { headers: { "content-length": String(total) } })) as unknown as typeof fetch;
    const dir = await mkdtemp(join(tmpdir(), "w2-dl-"));
    const seen: (number | null)[] = [];
    await nodeWindowsSystem(fetchImpl).download("https://x/i.exe", join(dir, "i.exe"), (p) => seen.push(p));
    expect((await readFile(join(dir, "i.exe"))).length).toBe(total);
    expect(seen.at(-1)).toBe(100);
  });

  it("refuses a short download", async () => {
    const { mkdtemp } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const { nodeWindowsSystem } = await import("./windows-runtime");
    const fetchImpl = (async () => new Response(new Uint8Array(10), { headers: { "content-length": "20" } })) as unknown as typeof fetch;
    const dir = await mkdtemp(join(tmpdir(), "w2-dl-"));
    await expect(nodeWindowsSystem(fetchImpl).download("https://x/i.exe", join(dir, "i.exe"), () => undefined)).rejects.toThrow(/incomplete/);
  });
});
