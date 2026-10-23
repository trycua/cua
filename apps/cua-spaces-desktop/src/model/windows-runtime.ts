// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Local Spaces on Windows run in Docker Desktop (its WSL 2 engine). A clean
 * Windows 11 PC has neither, so New Space and onboarding used to stop at a
 * red line with a docs link. This finds out what is missing and fixes what
 * it can from the app:
 *
 * - Docker Desktop not installed: downloads Docker's signed installer,
 *   checks its signature, and runs Docker's documented silent install
 *   (`install --quiet --accept-license --backend=wsl-2`) through one UAC
 *   prompt. The installer turns on WSL 2 itself.
 * - Installed but not running: starts it.
 * - Starting: says so; the page asks again until the engine answers.
 * - Needs a restart (Windows turned on WSL 2 or the installer said so): says so.
 * - Virtualization off in the firmware: nothing to install; says how to
 *   turn it on, or to use another computer.
 *
 * Everything that touches the system goes through {@link WindowsSystem}, so
 * the decisions are tested without Windows.
 */

import { createWriteStream } from "node:fs";
import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { execFile, spawn } from "node:child_process";
import { join } from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";

/** What the page's fix button does (`spaces.fixRuntime {action}`). */
export const RUNTIME_FIX_ACTIONS = ["install-docker", "start-docker", "restart-app", "open-windows-features"] as const;
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
  /** The button's label ("Install Docker Desktop"). */
  label: string;
  /** One line under the button: what happens, how long, what Windows asks. */
  note: string | null;
  /** Something runs now (download, install, start): the page polls the options until it settles. */
  busy: boolean;
  /** Terms the action accepts (Docker's), linked beside the note. */
  termsUrl: string | null;
}

/** What the probe found out about this PC. */
export interface WindowsRuntimeFacts {
  /** `Docker Desktop.exe` is installed. */
  dockerInstalled: boolean;
  /** Its process runs (the engine may still be starting). */
  dockerRunning: boolean;
  /** Hardware virtualization: on (or a hypervisor already runs), off in the firmware, or unknown. */
  virtualization: "on" | "off" | "unknown";
  /** Windows waits for a restart to finish turning on WSL 2 / the install. */
  restartPending: boolean;
  features: WindowsFeatures;
}

/** Where a fix the app started is now. */
export type FixPhase =
  | { kind: "idle" }
  | { kind: "downloading"; percent: number | null }
  | { kind: "installing" }
  | { kind: "starting"; since: number }
  | { kind: "failed"; message: string };

/** The reason and the fix New Space shows when no local runtime is ready. */
export interface WindowsRuntimeAdvice {
  reason: string;
  fix: RuntimeFix | null;
}

const DOCKER_TERMS = "https://www.docker.com/legal/docker-subscription-service-agreement/";
/** After this long "starting", say where to look instead of waiting quietly. */
export const START_PATIENCE_MS = 3 * 60_000;

/**
 * The reason and the fix for a PC where no local runtime answered.
 * `containerDetail` is the runtime doctor's word for the container engine.
 */
export function windowsRuntimeAdvice(facts: WindowsRuntimeFacts, phase: FixPhase, now: number): WindowsRuntimeAdvice {
  if (facts.virtualization === "off") {
    return {
      reason:
        "This PC can't run local Spaces: virtualization is turned off. Turn on Intel VT-x or AMD-V (SVM) in the PC's firmware (BIOS/UEFI) settings, or run Spaces on another computer.",
      fix: null,
    };
  }
  switch (phase.kind) {
    case "downloading":
      return {
        reason: "Getting Docker Desktop, which runs local Spaces on Windows.",
        fix: {
          action: null,
          label: phase.percent === null ? "Downloading Docker Desktop…" : `Downloading Docker Desktop… ${phase.percent}%`,
          note: "About 600 MB. Windows asks for administrator approval when it's in.",
          busy: true,
          termsUrl: null,
        },
      };
    case "installing":
      return {
        reason: "Installing Docker Desktop, which runs local Spaces on Windows.",
        fix: { action: null, label: "Installing Docker Desktop…", note: "Approve the Windows prompt. This takes a few minutes.", busy: true, termsUrl: null },
      };
    case "failed":
      return {
        reason: phase.message,
        fix: facts.dockerInstalled ? startFix() : installFix(),
      };
    default:
      break;
  }
  if (facts.restartPending) {
    return {
      reason: "Restart this PC to finish setting up Docker Desktop (Windows turned on WSL 2). Then open Cua Spaces again.",
      fix: null,
    };
  }
  if (!facts.dockerInstalled) {
    return {
      reason: "Local Spaces on Windows run in Docker Desktop, which isn't installed yet.",
      fix: installFix(),
    };
  }
  const disabled = [
    facts.features.virtualMachinePlatform === "disabled" ? "Virtual Machine Platform" : null,
    facts.features.subsystemLinux === "disabled" ? "Windows Subsystem for Linux" : null,
  ].filter(Boolean);
  if (disabled.length) {
    return {
      reason: `Windows features are turned off: ${disabled.join(", ")}.`,
      fix: {
        action: "open-windows-features",
        label: "Open Windows Features",
        note: "Turn on the listed features, then restart this PC and open Cua Spaces again.",
        busy: false,
        termsUrl: null,
      },
    };
  }
  if (facts.dockerRunning || phase.kind === "starting") {
    const waited = phase.kind === "starting" ? now - phase.since : 0;
    return {
      reason:
        waited > START_PATIENCE_MS
          ? "Docker Desktop is still starting. If it shows a window (terms, sign-in or an update), finish it there; this updates when its engine runs."
          : "Docker Desktop is starting. This updates when its engine runs (about a minute).",
      fix: { action: null, label: "Waiting for Docker Desktop…", note: null, busy: true, termsUrl: null },
    };
  }
  return { reason: "Docker Desktop is installed but not running.", fix: startFix() };
}

function installFix(): RuntimeFix {
  return {
    action: "install-docker",
    label: "Install Docker Desktop",
    note: "About 600 MB. Windows asks for administrator approval (Windows PowerShell) and may need a restart. Installing accepts Docker's subscription terms; Docker Desktop is free for personal use and small businesses.",
    busy: false,
    termsUrl: DOCKER_TERMS,
  };
}

function startFix(): RuntimeFix {
  return { action: "start-docker", label: "Start Docker Desktop", note: null, busy: false, termsUrl: null };
}

/* ---- The system ------------------------------------------------------------ */

/** The Windows calls the fixes make (faked in tests). */
export interface WindowsSystem {
  exists(path: string): Promise<boolean>;
  /** Image names of running processes, lower case. */
  running(image: string): Promise<boolean>;
  /** PowerShell, no profile; stdout, or null when it failed. */
  powershell(script: string, timeoutMs: number): Promise<{ code: number; stdout: string; stderr: string }>;
  download(url: string, to: string, progress: (percent: number | null) => void): Promise<void>;
  writeFile(path: string, text: string): Promise<void>;
  readFile(path: string): Promise<string>;
  launch(path: string): void;
}

/**
 * Runs elevated (one UAC prompt): Docker Desktop's documented silent install
 * with the WSL 2 backend, the signed-in user into docker-users, then the WSL
 * package when `wsl --version` says it is missing. Writes
 * `<installer exit>|<added to docker-users>|<wsl ready>` to `-Result`.
 */
export const ELEVATED_INSTALL_SCRIPT = `param([string]$Installer, [string]$User, [string]$Result)
$ErrorActionPreference = 'Continue'
$added = $false
$wsl = $false
$p = Start-Process -FilePath $Installer -ArgumentList 'install','--quiet','--accept-license','--backend=wsl-2' -Wait -PassThru
$code = $p.ExitCode
if ($code -eq 0 -or $code -eq 3010 -or $code -eq 1641) {
  if ($User) {
    $member = Get-LocalGroupMember -Group 'docker-users' -ErrorAction SilentlyContinue | Where-Object { $_.Name -ieq $User }
    if (-not $member) {
      try { Add-LocalGroupMember -Group 'docker-users' -Member $User -ErrorAction Stop; $added = $true } catch { }
    }
  }
  & wsl.exe --version *> $null
  if ($LASTEXITCODE -ne 0) {
    & wsl.exe --install --no-distribution --web-download *> $null
    & wsl.exe --version *> $null
  }
  $wsl = ($LASTEXITCODE -eq 0)
}
"$code|$added|$wsl" | Set-Content -LiteralPath $Result -Encoding ascii
`;

/** The elevated script's result line, or null when it never wrote one. */
export function parseInstallResult(text: string): { installer: number; addedToGroup: boolean; wslReady: boolean } | null {
  const [code, added, wsl] = text.trim().split("|");
  if (code === undefined || code === "" || !/^-?\d+$/.test(code)) return null;
  return { installer: Number(code), addedToGroup: added?.toLowerCase() === "true", wslReady: wsl?.toLowerCase() === "true" };
}

/** `DOMAIN\\user` for docker-users (the signed-in user, not the approving administrator). */
export function currentUser(env: NodeJS.ProcessEnv = process.env): string {
  const user = env.USERNAME ?? "";
  if (!user) return "";
  return env.USERDOMAIN ? `${env.USERDOMAIN}\\${user}` : user;
}

export const DOCKER_INSTALLER_URL = "https://desktop.docker.com/win/main/amd64/Docker%20Desktop%20Installer.exe";

export function dockerDesktopExe(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.ProgramFiles ?? "C:\\Program Files", "Docker", "Docker", "Docker Desktop.exe");
}

/** Exit codes that mean "done, restart to finish" (Windows Installer's and Docker's). */
const RESTART_CODES = new Set([3010, 1641]);
/** UAC declined: `Start-Process -Verb RunAs` reports ERROR_CANCELLED. */
const CANCELLED = /cancel+ed by the user|ERROR_CANCELLED|\b1223\b/i;

/** Facts the probe reads; the virtualization answer does not change while the app runs. */
export class WindowsRuntime {
  private phase: FixPhase = { kind: "idle" };
  private virtualization: WindowsRuntimeFacts["virtualization"] | null = null;
  /** The installer asked for a restart (until this process ends, which a restart does). */
  private restartAsked = false;

  constructor(
    private readonly sys: WindowsSystem,
    private readonly tempDir: string,
    private readonly now: () => number = Date.now,
  ) {}

  async facts(): Promise<WindowsRuntimeFacts> {
    const exe = dockerDesktopExe();
    const [dockerInstalled, dockerRunning, virtualization, restartPending, features] = await Promise.all([
      this.sys.exists(exe),
      this.sys.running("Docker Desktop.exe"),
      this.readVirtualization(),
      this.restartAsked ? Promise.resolve(true) : this.readRestartPending(),
      this.phase.kind === "downloading" || this.phase.kind === "installing" ? Promise.resolve(parseFeatures("")) : this.readFeatures(),
    ]);
    return { dockerInstalled, dockerRunning, virtualization, restartPending, features };
  }

  /** The advice now; a finished start (the engine answered) is the caller's to see. */
  async advice(): Promise<WindowsRuntimeAdvice> {
    const facts = await this.facts();
    // A start that ended with Docker Desktop gone (quit, crashed): offer it again.
    if (this.phase.kind === "starting" && !facts.dockerRunning && this.now() - this.phase.since > 20_000) this.phase = { kind: "idle" };
    return windowsRuntimeAdvice(facts, this.phase, this.now());
  }

  /** The engine answers: forget the fix. */
  settled(): void {
    this.phase = { kind: "idle" };
  }

  /** Runs `action` in the background; the page sees its progress in the options. */
  run(action: RuntimeFixAction): void {
    if (this.phase.kind === "downloading" || this.phase.kind === "installing") return;
    switch (action) {
      case "restart-app":
        return;
      case "start-docker":
        this.start();
        return;
      case "open-windows-features":
        this.sys.launch("optionalfeatures.exe");
        return;
      case "install-docker":
        void this.install().catch((e: unknown) => {
          this.phase = { kind: "failed", message: installFailedText(e) };
        });
        return;
      default: {
        const exhaustive: never = action;
        throw new Error(`Unknown runtime fix: ${exhaustive}`);
      }
    }
  }

  private start(): void {
    this.sys.launch(dockerDesktopExe());
    this.phase = { kind: "starting", since: this.now() };
  }

  private async install(): Promise<void> {
    this.phase = { kind: "downloading", percent: null };
    const file = join(this.tempDir, "DockerDesktopInstaller.exe");
    await this.sys.download(DOCKER_INSTALLER_URL, file, (percent) => {
      this.phase = { kind: "downloading", percent };
    });
    const sig = await this.sys.powershell(
      `$s = Get-AuthenticodeSignature -LiteralPath '${psQuote(file)}'; "$($s.Status)|$($s.SignerCertificate.Subject)"`,
      60_000,
    );
    if (!signedByDocker(sig.stdout)) throw new Error(`the downloaded installer isn't signed by Docker (${sig.stdout.trim() || sig.stderr.trim()})`);
    this.phase = { kind: "installing" };
    // One UAC prompt for everything that needs an administrator: Docker's
    // silent install, this user in docker-users (an administrator who isn't
    // this user may approve the prompt) and the WSL package (Windows 11
    // ships only its stub; Docker Desktop can't start without it).
    const script = join(this.tempDir, "install-docker.ps1");
    const result = join(this.tempDir, "install-docker.result");
    await this.sys.writeFile(script, ELEVATED_INSTALL_SCRIPT);
    await this.sys.writeFile(result, "");
    const args = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, "-Installer", file, "-User", currentUser(), "-Result", result]
      .map((a) => `'${psQuote(a)}'`)
      .join(",");
    const r = await this.sys.powershell(
      `$ErrorActionPreference = 'Stop'; $p = Start-Process -FilePath 'powershell.exe' -ArgumentList ${args} -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode`,
      30 * 60_000,
    );
    if (CANCELLED.test(r.stderr) || CANCELLED.test(r.stdout)) throw new Error("cancelled");
    const outcome = parseInstallResult(await this.sys.readFile(result).catch(() => ""));
    if (!outcome) throw new Error(`the installer didn't finish${r.stderr.trim() ? ` (${r.stderr.trim().split(/\r?\n/)[0]})` : ""}`);
    if (outcome.installer !== 0 && !RESTART_CODES.has(outcome.installer)) throw new Error(`the installer exited with code ${outcome.installer}`);
    // A restart finishes it when the installer asks, when Windows has
    // features to turn on (WSL 2), or when this user just joined docker-users.
    if (RESTART_CODES.has(outcome.installer) || outcome.addedToGroup || !outcome.wslReady || (await this.readRestartPending())) {
      this.restartAsked = true;
      this.phase = { kind: "idle" };
      return;
    }
    this.start();
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
        "$c = Get-CimInstance Win32_ComputerSystem; $p = Get-CimInstance Win32_Processor | Select-Object -First 1; \"$($c.HypervisorPresent)|$($p.VirtualizationFirmwareEnabled)\"",
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
  const [hypervisor, firmware] = stdout.trim().split("|").map((s) => s.trim().toLowerCase());
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

export function signedByDocker(stdout: string): boolean {
  const [status, subject = ""] = stdout.trim().split("|");
  return status === "Valid" && /\bO="?Docker Inc/i.test(subject);
}

export function installFailedText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw === "cancelled") return "Docker Desktop wasn't installed: the Windows prompt was declined. Try again and choose Yes.";
  return `Couldn't install Docker Desktop: ${raw}. Try again, or install it from https://docs.docker.com/desktop/setup/install/windows-install/.`;
}

const psQuote = (s: string) => s.replace(/'/g, "''");

/* ---- The real system ------------------------------------------------------- */

export function nodeWindowsSystem(fetchImpl: typeof fetch = fetch): WindowsSystem {
  const exec = (file: string, args: string[], timeoutMs: number) =>
    new Promise<{ code: number; stdout: string; stderr: string }>((resolve) => {
      execFile(file, args, { timeout: timeoutMs, windowsHide: true, maxBuffer: 1024 * 1024 }, (error, stdout, stderr) => {
        const code = error ? (typeof (error as { code?: unknown }).code === "number" ? ((error as { code: number }).code) : 1) : 0;
        resolve({ code, stdout: String(stdout), stderr: String(stderr) });
      });
    });
  return {
    exists: async (path) => (await stat(path).catch(() => null)) !== null,
    running: async (image) => {
      const r = await exec("tasklist.exe", ["/FI", `IMAGENAME eq ${image}`, "/FO", "CSV", "/NH"], 10_000);
      return r.stdout.toLowerCase().includes(`"${image.toLowerCase()}"`);
    },
    powershell: (script, timeoutMs) => exec("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], timeoutMs),
    async download(url, to, progress) {
      const part = `${to}.part`;
      await mkdir(join(to, ".."), { recursive: true });
      await rm(part, { force: true });
      const res = await fetchImpl(url, { redirect: "follow" });
      if (!res.ok || !res.body) throw new Error(`download failed (HTTP ${res.status})`);
      const total = Number(res.headers.get("content-length")) || 0;
      let got = 0;
      let last = -1;
      // Progress counts in the pipeline itself: a 'data' listener on the
      // source would start it flowing before the file is open and drop chunks.
      const count = new Transform({
        transform(chunk: Buffer, _enc, done) {
          got += chunk.length;
          const pct = total ? Math.floor((got / total) * 100) : null;
          if (pct !== last) {
            last = pct ?? -1;
            progress(pct);
          }
          done(null, chunk);
        },
      });
      await pipeline(Readable.fromWeb(res.body as import("node:stream/web").ReadableStream), count, createWriteStream(part));
      if (total && got !== total) throw new Error(`download incomplete (${got} of ${total} bytes)`);
      await rename(part, to);
    },
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
