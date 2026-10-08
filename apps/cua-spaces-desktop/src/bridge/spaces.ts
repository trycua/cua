// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space list (`spaces.*`): the roster as the core shapes it, New Space's
// options and create (the page's wizard on the app core's create), power,
// delete and cancel (the SwiftUI host's `spaces`, `createOptions`, `create`
// and `cancelCreate`).
import type { Native } from "../native/load";
import type { AppCreateSpaceArgs, AppRuntime, AppSpace, AppWizardEnv, SpaceCreateProgress } from "../native/generated/index";
import { AppModel } from "../model/app-model";
import { kindOf, osOf } from "../model/backend";
import { isCancelled } from "../model/errors";
import { bool, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { encode } from "./value";

export function spacesState(model: AppModel) {
  return {
    loaded: model.loaded,
    selectedId: model.selectedSpaceId,
    sidebar: encode(model.sidebar),
    spaces: model.spaces.map((space) => ({ space: encode(space), detail: encode(model.detail(space)), deleting: model.isDeleting(space.id) })),
    statusLine: model.statusLine,
    // Why the list may be out of date (null when the last read worked).
    rosterError: model.rosterError,
  };
}

/**
 * The env the page's wizard runs on, in the shape of the web bridge's
 * `NewSpaceOptions` (plus `macosVmsRunning`; `pending` while the launch is
 * still starting, when every probe would wait).
 */
export function createOptions(env: AppWizardEnv, macosVmsRunning: number | null, pending: boolean) {
  const out: Record<string, unknown> = {
    local: {
      available: env.localAvailable,
      backends: env.localBackends ?? [],
      error: env.localReason ?? null,
      hostArch: env.hostArch ?? null,
      storage: encode(env.storage),
    },
    gpus: encode(env.gpus),
    cloudPricing: encode(env.cloudPricing),
    experiments: encode(env.experiments),
    maxCpus: env.maxCpus,
    env: encode(env),
    macosVmsRunning,
  };
  if (pending) out.pending = true;
  return out;
}

const RUNTIMES = ["auto", "gvisor", "runc", "qemu", "lume", "kubevirt"] as const;

/** The page's `SpaceCreateConfig` (the core's `CreateSpaceArgs`) as the FFI record. */
export function createArgs(native: Native, c: BridgeArgs, defaultOn: string): AppCreateSpaceArgs {
  const image = c.image;
  if (typeof image !== "string" || image === "") throw Failure.badArgs("config.image: string");
  const kind = kindOf(native, typeof c.kind === "string" ? c.kind : "");
  if (!kind) throw Failure.badArgs("config.kind: container | vm");
  const runtime = (c.runtime ?? "auto") as string;
  if (!(RUNTIMES as readonly string[]).includes(runtime)) throw Failure.badArgs("config.runtime: auto | gvisor | runc | qemu | lume | kubevirt");
  const count = (k: string) => (typeof c[k] === "number" && Number.isFinite(c[k]) && (c[k] as number) >= 0 ? Math.floor(c[k] as number) : undefined);
  const text = (k: string) => (typeof c[k] === "string" && c[k] !== "" ? (c[k] as string) : undefined);
  return {
    image,
    on: text("on") ?? defaultOn,
    kind,
    runtime: runtime as AppRuntime,
    name: text("name"),
    cpus: count("cpus"),
    memoryMb: count("memoryMb"),
    diskGb: count("diskGb"),
    spacesd: typeof c.spacesd === "boolean" ? c.spacesd : true,
    gpu: text("gpu"),
  };
}

/** The SDK's create progress as the bridge's `CreateProgress`. */
export function progress(p: SpaceCreateProgress, pendingId: string) {
  return {
    pendingId,
    phase: p.phase,
    detail: p.detail,
    fraction: p.fraction ?? null,
    bytesDone: p.bytesDone === undefined ? null : Number(p.bytesDone),
    bytesTotal: p.bytesTotal === undefined ? null : Number(p.bytesTotal),
    bytesPerSecond: p.bytesPerSecond ?? null,
  };
}

export function spacesMethods(ctx: BridgeContext): Handlers {
  const { model, events, ui } = ctx;
  const native = model.native;

  const space = (args: BridgeArgs): AppSpace => {
    const id = string(args, "id");
    const s = model.spaces.find((x) => x.id === id);
    if (!s) throw Failure.notFound(`no Space ${id}`);
    return s;
  };

  return {
    "spaces.list": () => spacesState(model),
    "spaces.open": (args) => {
      const s = space(args);
      ui.openSpace(s.id, s.name, String(s.os));
      return null;
    },
    "spaces.createOptions": async () => {
      if (!model.servicesIn) return createOptions(model.knownNewSpaceEnv(), null, true);
      const env = await model.newSpaceEnv();
      return createOptions(env, model.runningMacosVms, false);
    },
    "spaces.create": async (args) => {
      const pendingId = string(args, "pendingId");
      if (!native.appCreatesIsPending(pendingId)) throw Failure.badArgs("pendingId: pending:<id>");
      const config = args.config;
      if (!config || typeof config !== "object" || Array.isArray(config)) throw Failure.badArgs("config: object");
      const defaultOn = model.settings.defaultLocation === native.AppLocation.Cloud ? "cloud" : "local";
      const a = createArgs(native, config as BridgeArgs, defaultOn);
      // The page sends the plan's OS (the pending row's icon).
      const os = (typeof args.os === "string" ? osOf(native, args.os) : undefined) ?? native.AppSpaceOs.Unknown;
      let id: string;
      try {
        id = await model.runCreate(a, os, pendingId, (p) => events.emit("spaces.createProgress", progress(p, pendingId)));
      } catch (error) {
        if (isCancelled(error)) throw new Failure("cancelled", "cancelled");
        throw error;
      }
      const made = model.spaces.find((s) => s.id === id);
      if (made) return encode(made);
      return { id, name: a.name ?? id, os, status: "running", detail: "", lastUsedAt: Number(AppModel.nowMs()) };
    },
    "spaces.setPower": (args) => {
      const s = space(args);
      model.setPower(s, bool(args, "on"));
      return spacesState(model);
    },
    "spaces.delete": (args) => {
      const s = space(args);
      model.delete(s, args.removeOnly === true);
      return spacesState(model);
    },
    "spaces.cancelCreate": (args) => {
      const id = string(args, "pendingId");
      const pending = model.creates.pending.find((p) => p.id === id);
      let state: string;
      if (pending && pending.error === undefined && pending.spaceId === undefined) {
        if (!pending.cancelling) model.cancelCreate(id);
        state = "cancelled";
      } else if (pending && pending.spaceId !== undefined) {
        state = "already_created";
      } else {
        state = model.spaces.some((s) => s.id === id) && !id.startsWith("pending:") ? "already_created" : "not_creating";
      }
      return { id, state, message: "" };
    },
  };
}
