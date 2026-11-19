// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Local Spaces on Windows run in a runtime Cua Spaces manages itself: a small
 * WSL 2 distribution with Docker inside (cua-vmm's managed runtime), like the
 * built-in runtime on a Mac. Nothing else is installed, so New Space never
 * offers Docker Desktop (a person who wants their own engine picks it in
 * Settings, Runtimes). The one thing a clean PC may lack is WSL 2 itself; this
 * finds out and fixes it from the app:
 *
 * - WSL 2 not set up: one Windows approval prompt (UAC) runs
 *   `wsl --install --no-distribution --web-download`, which turns on the
 *   Windows parts and fetches the WSL package. No distribution and no Docker.
 * - Windows waits for a restart to finish it: says so, and registers Cua
 *   Spaces to open again by itself after the restart (RunOnce), where the
 *   built-in runtime takes over.
 * - Virtualization off in the firmware: nothing to install; says how to turn
 *   it on, or to use another computer.
 *
 * Everything that touches the system goes through {@link WindowsSystem}, so
 * the decisions are tested without Windows.
 */

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { execFile, spawn } from "node:child_process";
import { join } from "node:path";

/** What the page's fix button does (`spaces.fixRuntime {action}`). */
export const RUNTIME_FIX_ACTIONS = ["setup-wsl", "restart-app", "open-windows-features"] as const;
export type RuntimeFixAction = (typeof RUNTIME_FIX_ACTIONS)[number];

type ContractAction = import("../../../cua-spaces-web/src/bridge/contracts/new-space").RuntimeFixAction;
type Assert<T extends true> = T;
export type RuntimeFixActionsMatch = Assert<
  [Exclude<RuntimeFixAction, ContractAction> | Exclude<ContractAction, RuntimeFixAction>] extends [never] ? true : false
>;

export type FeatureState = "enabled" | "disabled" | "unknown";

export interface WindowsFeatures {
  editionId: string | null;
  virtualMachinePlatform: FeatureState;
  hypervisorPlatform: FeatureState;
  subsystemLinux: FeatureState;
}

/** A step the page can offer next to the "no local runtime" reason. */
export interface RuntimeFix {
  /** What the button runs; null while something runs, or when only the person can act (restart, firmware). */
  action: RuntimeFixAction | null;
  /** The button's label. */
  label: string;
  /** One line under the button: what happens, how long, what Windows asks. */
  note: string | null;
  /** Something runs now: the page polls the options until it settles. */
  busy: boolean;
  /** Always null: nothing here has terms to accept. Kept for the page's contract. */
  termsUrl: string | null;
}

/** What the probe found out about this PC. */
export interface WindowsRuntimeFacts {
  /** `wsl.exe --version` answers: the WSL package is there. */
  wslReady: boolean;
  /** Hardware virtualization: on (or a hypervisor already runs), off in the firmware, or unknown. */
  virtualization: "on" | "off" | "unknown";
  /** Windows waits for a restart to finish turning on WSL 2. */
  restartPending: boolean;
  features: WindowsFeatures;
}

/** Where a fix the app started is now. */
export type FixPhase = { kind: "idle" } | { kind: "installing" } | { kind: "failed"; message: string };

/** The reason and the fix New Space shows when no local runtime is ready. */
export interface WindowsRuntimeAdvice {
  reason: string;
  fix: RuntimeFix | null;
}

/**
 * The reason and the fix for a PC where no local runtime is ready.
 */
export function windowsRuntimeAdvice(facts: WindowsRuntimeFacts, phase: FixPhase): WindowsRuntimeAdvice {
  if (facts.virtualization === "off") {
    return {
      reason:
        "This PC can't run local Spaces: virtualization is turned off. Turn on Intel VT-x or AMD-V (SVM) in the PC's firmware (BIOS/UEFI) settings, or run Spaces on another machine.",
      fix: null,
    };
  }
  if (phase.kind === "installing") {
    return {
      reason: "Setting up Linux support for local Spaces.",
      fix: { action: null, label: "Setting up…", note: "Approve the Windows prompt. This takes a minute or two.", busy: true, termsUrl: null },
    };
  }
  if (phase.kind === "failed") return { reason: phase.message, fix: setupFix() };
  if (facts.restartPending) {
    return {
      reason: "Restart this PC to finish setting up Linux support. Cua Spaces opens again by itself and carries on.",
      fix: null,
    };
  }
  const off = [
    facts.features.virtualMachinePlatform === "disabled" ? "Virtual Machine Platform" : null,
    facts.features.subsystemLinux === "disabled" ? "Windows Subsystem for Linux" : null,
  ].filter(Boolean);
  if (!facts.wslReady || off.length) {
    return {
      reason: "Local Spaces run on Windows' built-in Linux support (WSL 2), which isn't set up on this PC yet.",
      fix: setupFix(),
    };
  }
  return { reason: "Cua Spaces is preparing its local runtime.", fix: null };
}

function setupFix(): RuntimeFix {
  return {
    action: "setup-wsl",
    label: "Set up Linux support",
    note: "A small Windows component. Windows asks for administrator approval and may need a restart; Cua Spaces opens again by itself afterwards.",
    busy: false,
    termsUrl: null,
  };
}

/* ---- The system ------------------------------------------------------------ */

/** The Windows calls the fixes make (faked in tests). */
export interface WindowsSystem {
  /** PowerShell, no profile; stdout, or null when it failed. */
  powershell(script: string, timeoutMs: number): Promise<{ code: number; stdout: string; stderr: string }>;
  writeFile(path: string, text: string): Promise<void>;
  readFile(path: string): Promise<string>;
  launch(path: string): void;
}

/**
 * Runs elevated (one UAC prompt): turns on WSL 2 and fetches its package, with
 * no distribution. Writes `<wsl exit code>|<wsl ready>` to `-Result`.
 */
export const ELEVATED_SETUP_SCRIPT = `param([string]$Result)
$ErrorActionPreference = 'Continue'
& wsl.exe --install --no-distribution --web-download *> $null
$code = $LASTEXITCODE
& wsl.exe --version *> $null
$ready = ($LASTEXITCODE -eq 0)
"$code|$ready" | Set-Content -LiteralPath $Result -Encoding ascii
`;

/** The elevated script's result line, or null when it never wrote one. */
export function parseSetupResult(text: string): { code: number; wslReady: boolean } | null {
  const [code, ready] = text.trim().split("|");
  if (code === undefined || code === "" || !/^-?\d+$/.test(code)) return null;
  return { code: Number(code), wslReady: ready?.toLowerCase() === "true" };
}

/** Exit codes that mean "done, restart to finish" (Windows Installer's). */
const RESTART_CODES = new Set([3010, 1641]);
/** UAC declined: `Start-Process -Verb RunAs` reports ERROR_CANCELLED. */
const CANCELLED = /cancel+ed by the user|ERROR_CANCELLED|\b1223\b/i;

/** Facts the probe reads; the virtualization answer does not change while the app runs. */
export class WindowsRuntime {
  private phase: FixPhase = { kind: "idle" };
  private virtualization: WindowsRuntimeFacts["virtualization"] | null = null;
  /** The setup asked for a restart (until this process ends, which a restart does). */
  private restartAsked = false;

  constructor(
    private readonly sys: WindowsSystem,
    private readonly tempDir: string,
    private readonly appExe: string = process.execPath,
  ) {}

  async facts(): Promise<WindowsRuntimeFacts> {
    const [wslReady, virtualization, restartPending, features] = await Promise.all([
      this.readWslReady(),
      this.readVirtualization(),
      this.restartAsked ? Promise.resolve(true) : this.readRestartPending(),
      this.phase.kind === "installing" ? Promise.resolve(parseFeatures("")) : this.readFeatures(),
    ]);
    return { wslReady, virtualization, restartPending, features };
  }

  /** The advice now. */
  async advice(): Promise<WindowsRuntimeAdvice> {
    return windowsRuntimeAdvice(await this.facts(), this.phase);
  }

  /** A runtime answers: forget the fix. */
  settled(): void {
    this.phase = { kind: "idle" };
  }

  /** Runs `action` in the background; the page sees its progress in the options. */
  run(action: RuntimeFixAction): void {
    if (this.phase.kind === "installing") return;
    switch (action) {
      case "restart-app":
        return;
      case "open-windows-features":
        this.sys.launch("optionalfeatures.exe");
        return;
      case "setup-wsl":
        void this.setup().catch((e: unknown) => {
          this.phase = { kind: "failed", message: setupFailedText(e) };
        });
        return;
      default: {
        const exhaustive: never = action;
        throw new Error(`Unknown runtime fix: ${exhaustive}`);
      }
    }
  }

  private async setup(): Promise<void> {
    this.phase = { kind: "installing" };
    // One UAC prompt for everything that needs an administrator.
    const script = join(this.tempDir, "setup-wsl.ps1");
    const result = join(this.tempDir, "setup-wsl.result");
    await this.sys.writeFile(script, ELEVATED_SETUP_SCRIPT);
    await this.sys.writeFile(result, "");
    const args = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, "-Result", result].map((a) => `'${psQuote(a)}'`).join(",");
    const r = await this.sys.powershell(
      `$ErrorActionPreference = 'Stop'; $p = Start-Process -FilePath 'powershell.exe' -ArgumentList ${args} -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode`,
      30 * 60_000,
    );
    if (CANCELLED.test(r.stderr) || CANCELLED.test(r.stdout)) throw new Error("cancelled");
    const outcome = parseSetupResult(await this.sys.readFile(result).catch(() => ""));
    if (!outcome) throw new Error(`the setup didn't finish${r.stderr.trim() ? ` (${r.stderr.trim().split(/\r?\n/)[0]})` : ""}`);
    // A restart finishes it when Windows asks, or when WSL still isn't usable
    // because Windows has features to turn on.
    if (RESTART_CODES.has(outcome.code) || !outcome.wslReady || (await this.readRestartPending())) {
      this.restartAsked = true;
      this.phase = { kind: "idle" };
      await this.resumeAfterRestart();
      return;
    }
    if (outcome.code !== 0) throw new Error(`Windows reported error ${outcome.code}`);
    this.phase = { kind: "idle" };
  }

  /** Cua Spaces opens again by itself at the person's next sign-in after the restart. */
  private async resumeAfterRestart(): Promise<void> {
    await this.sys
      .powershell(
        `New-ItemProperty -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce' -Name 'CuaSpacesResume' -Value '"${psQuote(this.appExe)}"' -PropertyType String -Force | Out-Null`,
        15_000,
      )
      .catch(() => null);
  }

  private async readWslReady(): Promise<boolean> {
    const r = await this.sys.powershell("& wsl.exe --version *> $null; exit $LASTEXITCODE", 15_000).catch(() => null);
    return r?.code === 0;
  }

  private async readFeatures(): Promise<WindowsFeatures> {
    const r = await this.sys
      .powershell(
        `$result = @{}
try { $result.editionId = (Get-ItemProperty -LiteralPath 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion' -Name EditionID -ErrorAction Stop).EditionID } catch {}
try {
  Get-CimInstance Win32_OptionalFeature -Filter "Name='VirtualMachinePlatform' OR Name='HypervisorPlatform' OR Name='Microsoft-Windows-Subsystem-Linux'" -ErrorAction Stop | ForEach-Object {
    switch ($_.Name) {
      'VirtualMachinePlatform' { $result.virtualMachinePlatform = $_.InstallState }
      'HypervisorPlatform' { $result.hypervisorPlatform = $_.InstallState }
      'Microsoft-Windows-Subsystem-Linux' { $result.subsystemLinux = $_.InstallState }
    }
  }
} catch {}
ConvertTo-Json -InputObject $result -Compress`,
        15_000,
      )
      .catch(() => null);
    return parseFeatures(r?.code === 0 ? r.stdout : "");
  }

  private async readVirtualization(): Promise<WindowsRuntimeFacts["virtualization"]> {
    if (this.virtualization) return this.virtualization;
    const r = await this.sys
      .powershell(
        '$c = Get-CimInstance Win32_ComputerSystem; $p = Get-CimInstance Win32_Processor | Select-Object -First 1; "$($c.HypervisorPresent)|$($p.VirtualizationFirmwareEnabled)"',
        15_000,
      )
      .catch(() => null);
    this.virtualization = r && r.code === 0 ? parseVirtualization(r.stdout) : "unknown";
    return this.virtualization;
  }

  /** WSL 2 not usable yet although Windows has its parts: they wait for a restart. */
  private async readRestartPending(): Promise<boolean> {
    const r = await this.sys
      .powershell(
        "if (Test-Path 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending') { 'pending' } else { 'clear' }",
        15_000,
      )
      .catch(() => null);
    return r?.stdout.trim() === "pending";
  }
}

/** `HypervisorPresent|VirtualizationFirmwareEnabled`. A running hypervisor
 * (Hyper-V, WSL 2) hides the firmware flag, so it counts as on. */
export function parseVirtualization(stdout: string): WindowsRuntimeFacts["virtualization"] {
  const [hypervisor, firmware] = stdout
    .trim()
    .split("|")
    .map((s) => s.trim().toLowerCase());
  if (hypervisor === "true" || firmware === "true") return "on";
  if (firmware === "false") return "off";
  return "unknown";
}

export function parseFeatures(stdout: string): WindowsFeatures {
  let raw: Record<string, unknown> = {};
  try {
    const value: unknown = JSON.parse(stdout);
    if (value && typeof value === "object" && !Array.isArray(value)) raw = value as Record<string, unknown>;
  } catch {}
  const state = (value: unknown): FeatureState => (value === 1 ? "enabled" : value === 2 ? "disabled" : "unknown");
  return {
    editionId: typeof raw.editionId === "string" ? raw.editionId : null,
    virtualMachinePlatform: state(raw.virtualMachinePlatform),
    hypervisorPlatform: state(raw.hypervisorPlatform),
    subsystemLinux: state(raw.subsystemLinux),
  };
}

export function setupFailedText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw === "cancelled") return "Linux support wasn't set up: the Windows prompt was declined. Try again and choose Yes.";
  return `Couldn't set up Linux support: ${raw}. Try again, or run \`wsl --install --no-distribution\` in an administrator PowerShell and restart.`;
}

const psQuote = (s: string) => s.replace(/'/g, "''");

/* ---- The real system ------------------------------------------------------- */

export function nodeWindowsSystem(): WindowsSystem {
  const exec = (file: string, args: string[], timeoutMs: number) =>
    new Promise<{ code: number; stdout: string; stderr: string }>((resolve) => {
      execFile(file, args, { timeout: timeoutMs, windowsHide: true, maxBuffer: 1024 * 1024 }, (error, stdout, stderr) => {
        const code = error ? (typeof (error as { code?: unknown }).code === "number" ? (error as { code: number }).code : 1) : 0;
        resolve({ code, stdout: String(stdout), stderr: String(stderr) });
      });
    });
  return {
    powershell: (script, timeoutMs) => exec("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], timeoutMs),
    async writeFile(path, text) {
      await mkdir(join(path, ".."), { recursive: true });
      await writeFile(path, text, "utf8");
    },
    readFile: (path) => readFile(path, "utf8"),
    launch(path) {
      const child = spawn(path, [], { detached: true, stdio: "ignore", windowsHide: false });
      child.on("error", () => undefined);
      child.unref();
    },
  };
}
