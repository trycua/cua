// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Process-level wiring (the SwiftUI app's AppEnvironment.swift): where the
// app core's state lives (`appCoreStatePaths`), this app's daemon, and the
// launch that makes the live services off
// the window's critical path. Nothing here reads the keychain or waits on
// the daemon before the window shows: the page shows the launch
// (`startup.get`) on stand-ins that wait for the services.
import { existsSync, readFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { Native } from "../native/load";
import { appCoreStatePaths } from "../migrate-swift";
import { nativeFiles } from "../native/location";
import type { CuaLike } from "../native/generated/index";
import { PendingAgentSetup, LiveAgentSetup } from "./agents";
import { AppModel } from "./app-model";
import { LiveSpacesBackend } from "./backend";
import { CloudModel } from "./cloud";
import { DaemonSupervisor, KEYCHAIN_NONINTERACTIVE_ENV } from "./daemon";
import { computerName, DevicesModel } from "./devices";
import { sdkErrorKind, words } from "./errors";
import { HostModel } from "./host";
import { KeyvaultModel } from "./keyvault";
import { LiveGate, PendingAccount, PendingSpacesBackend, type LiveServices } from "./pending";
import { LiveAccount, LiveTelemetry, hostAccount } from "./services";
import { BundledCuaKeychain, StartupModel } from "./startup";
import { withTimeout } from "./time";

/** Whether the first run finished (`onboarding.json`'s `completed`); null when unknown. */
export function readOnboardingCompleted(file: string): boolean | null {
  try {
    const completed = (JSON.parse(readFileSync(file, "utf8")) as { completed?: unknown }).completed;
    return typeof completed === "boolean" ? completed : null;
  } catch {
    return null;
  }
}

export interface EnvironmentOptions {
  native: Native;
  /** The native directory: the library, its runtime and the bundled `cua`. */
  nativeDir: string;
  /** The app's user data folder. */
  userData: string;
  version: string;
  /** Opens an https page in the user's browser (the sign-in). */
  openUrl: (url: string) => void;
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
}

export interface Environment {
  model: AppModel;
  supervisor: DaemonSupervisor | null;
  settingsPath: string;
  onboardingPath: string;
  /** The bundled `cua` (null in a build without one). */
  cua: string | null;
}

/** The broker's client (`$CUA_HOME/keyvault.sock`); null when the library cannot make one, which leaves the Keyvault unavailable. */
function keyvaultClient(native: Native) {
  try {
    return new native.KeyvaultClient(undefined);
  } catch (error) {
    console.warn(`[cua-spaces] no Keyvault client: ${words(error)}`);
    return null;
  }
}

/** The live model, its launch begun. */
export function makeModel(o: EnvironmentOptions): Environment {
  const env = o.env ?? process.env;
  const platform = o.platform ?? process.platform;
  const native = o.native;
  // The app core's files (on macOS, taken over from the SwiftUI app at first launch).
  const { settings: settingsPath, onboarding: onboardingPath } = appCoreStatePaths(o.userData);
  const cuaPath = nativeFiles(o.nativeDir, platform).cua;
  const cua = existsSync(cuaPath) ? cuaPath : null;

  // No keychain prompt from this process or the daemon it starts: a read
  // that would need one fails fast as "needs access", and the page asks.
  env[KEYCHAIN_NONINTERACTIVE_ENV] = "1";
  // This process's usage events are the app's, wait for the notice the
  // first run shows, and start with `app_launched`.
  const firstRunDue = !(readOnboardingCompleted(onboardingPath) ?? false);
  native.appTelemetryStart(o.version, firstRunDue);

  const ownership = (daemonExe: string | null, bundle: string) => native.appAboutRestartDaemon({ daemonExe: daemonExe ?? undefined, bundle });
  const supervisor = cua ? new DaemonSupervisor(cua, path.dirname(cua), ownership, env, platform) : null;
  const gate = new LiveGate<LiveServices>();
  const backend = new PendingSpacesBackend(gate);
  const account = new PendingAccount(gate);
  const telemetry = new LiveTelemetry(native);
  const host = new HostModel(native, null);
  const devices = new DevicesModel(native, () => gate.current?.devices ?? null);
  const cloud = new CloudModel(native, () => backend.current);
  const startup = new StartupModel({ kind: "starting" });
  const keyvault = new KeyvaultModel(native, keyvaultClient(native));
  const model = new AppModel({
    native,
    backend,
    startup,
    settingsPath,
    host,
    devices,
    cloud,
    account,
    telemetry,
    agentSetup: new PendingAgentSetup(gate),
    keyvault,
    servicesIn: () => gate.current !== null,
    cpus: os.availableParallelism?.() ?? os.cpus().length,
  });

  // Signed out is null (setup signs in first); anything else is the error itself.
  host.accountToken = (force) => account.accessToken(force);
  host.currentAccount = () => (gate.current?.account ? hostAccount(gate.current.account) : null);

  const services = () => makeLiveServices(native, supervisor, o.openUrl, cua);
  if (cua) startup.keychain = new BundledCuaKeychain(cua, env);
  startup.start = async () => {
    const made = await services();
    // A start run again after a failed one: the first to finish is the one handed over.
    if (!gate.resolve(made)) return;
    attach(made, model, supervisor, () => services());
  };
  startup.restart = async () => {
    await supervisor?.restart();
    // Signed in now (access given after the start): read it again.
    if (gate.current) model.attachLive();
  };
  // First launch: the bundled `cua` on PATH, silently (the SwiftUI app's first run does the same).
  if (cua && firstRunDue) startup.onReady.push(() => void installCliSilently(native, cua));
  startup.begin();
  return { model, supervisor, settingsPath, onboardingPath, cua };
}

/** Makes the live services: this app's daemon, then the SDK on it. */
export async function makeLiveServices(native: Native, supervisor: DaemonSupervisor | null, openUrl: (url: string) => void, cuaBinary: string | null = null): Promise<LiveServices> {
  const daemonError = await startDaemon(supervisor);
  try {
    const cua = await makeCua(native, supervisor);
    const auth = cua.auth();
    let devices = null;
    try {
      devices = auth.devices(undefined, computerName());
    } catch {
      devices = null;
    }
    const live = new LiveSpacesBackend(native, cua);
    return { backend: live, live, cua, account: new LiveAccount(auth, openUrl), host: new native.Host(undefined), agentSetup: new LiveAgentSetup(native, cua.agentSetup(), cuaBinary), devices, startError: null, daemonError };
  } catch (error) {
    // Say so: an empty list would look like "no Spaces".
    const message = `Could not start the cua SDK: ${words(error)}`;
    console.warn(`[cua-spaces] ${message}`);
    return { backend: null, live: null, cua: null, account: null, host: null, agentSetup: null, devices: null, startError: message, daemonError };
  }
}

/**
 * The SDK on this app's own daemon (`Cua.auto`: the daemon when it accepts
 * connections, else embedded). When another build's daemon came up in
 * between, the SDK refuses it with `DaemonNotRunning`, and one more start
 * replaces it. Cua Cloud uses the signed-in session.
 */
async function makeCua(native: Native, supervisor: DaemonSupervisor | null): Promise<CuaLike> {
  const config = native.CuaConfig.create({ fleetFromEnv: true, fleetFromSession: true });
  try {
    return native.Cua.auto(config);
  } catch (error) {
    if (sdkErrorKind(error) !== "DaemonNotRunning" || !supervisor) throw error;
    const again = await supervisor.start();
    if (again) throw new Error(`${words(error)} (${again})`);
    return native.Cua.auto(config);
  }
}

/** `cua daemon start`, once more when it failed (a daemon of the build before an update may have been slow to stop). */
export async function startDaemon(supervisor: DaemonSupervisor | null): Promise<string | null> {
  if (!supervisor) return null;
  const first = await supervisor.start();
  if (first === null) return null;
  console.log(`[cua-spaces] starting the cua daemon failed, trying again: ${first}`);
  return supervisor.start();
}

/** The live services are in: what needs them starts now. */
export function attach(services: LiveServices, model: AppModel, supervisor: DaemonSupervisor | null, remake: () => Promise<LiveServices>): void {
  model.host.host = services.host;
  if (services.live && services.cua && supervisor) {
    if (services.cua.mode() === model.native.CuaMode.Daemon) {
      supervise(services.cua, supervisor, model);
      model.reconnect = () => reconnect(model, supervisor, remake);
    } else {
      // Last resort: Spaces run in the app; say what is missing.
      model.show(
        `Cua Spaces could not start its daemon (${services.daemonError ?? "it does not answer"}). ` +
          "Spaces run inside the app for now; the Keyvault, agents and Cua Volume need the daemon. Reopen Cua Spaces to try again.",
        true,
      );
    }
  }
  model.attachLive();
  if (services.startError) model.show(services.startError, true);
}

let stopSupervision: (() => void) | null = null;

/**
 * Watches the daemon `cua` talks to (this app's, or another app's of the
 * same or a newer version it was accepted on); when it stops answering it
 * is started again and the app connects again.
 */
export function supervise(cua: CuaLike, supervisor: DaemonSupervisor, model: AppModel): void {
  stopSupervision?.();
  // The daemon this connection was made on. `Cua.auto` refuses another
  // app's daemon that is older than this app's cua, so one it accepted (of
  // the same or a newer version) is used as this app's own.
  const accepted = withTimeout(5, () => cua.info()).then((p) => (p.ok ? p.value.daemonPid : undefined));
  stopSupervision = supervisor.supervise({
    isUp: async () => {
      const probe = await withTimeout(5, () => cua.info());
      const pid = probe.ok ? probe.value.daemonPid : undefined;
      return pid !== undefined && (pid === (await accepted) || (await supervisor.isOwn(pid)));
    },
    accepted: () => accepted,
    report: (error) => {
      if (error) model.show(error, true);
      else if (model.bannerIsError) model.banner = null;
    },
    restarted: () => model.reconnectNow("the cua daemon was started again"),
  });
}

/** A new SDK client (`cua daemon start`, then `Cua.auto` again); the backend, the model and the supervisor move to it. */
export async function reconnect(model: AppModel, supervisor: DaemonSupervisor, remake: () => Promise<LiveServices>): Promise<boolean> {
  const made = await remake();
  if (!made.live || !made.cua || !(model.backend instanceof PendingSpacesBackend)) return false;
  model.backend.replace(made.live);
  model.host.host = made.host;
  model.attachLive();
  if (made.cua.mode() === model.native.CuaMode.Daemon) supervise(made.cua, supervisor, model);
  return true;
}

/** Puts the bundled `cua` on PATH: a current install, or a build without the CLI, writes nothing. */
export async function installCliSilently(native: Native, cua: string): Promise<void> {
  try {
    // The installer finds the CLI next to the "executable" it is given.
    const installer = native.AppCliInstaller.forExecutable(path.join(path.dirname(cua), "cua-spaces"));
    const plan = await installer.plan();
    if (plan.upToDate || !plan.source) return;
    const installed = await installer.install({ modifyPath: !plan.onPath });
    console.log(`[cua-spaces] installed the cua command at ${installed.target}`);
  } catch (error) {
    console.warn(`[cua-spaces] could not install the cua command: ${words(error)}`);
  }
}
