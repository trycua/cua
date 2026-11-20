// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space list (`spaces.*`): the roster as the core shapes it, New Space's
// options and create (the page's wizard on the app core's create), power,
// delete and cancel (the SwiftUI host's `spaces`, `createOptions`, `create`
// and `cancelCreate`).
import type { Native } from "../native/load";
import type { AppCreateSpaceArgs, AppSpace, AppWizardEnv, SpaceCreateProgress } from "../native/generated/index";
import { AppModel } from "../model/app-model";
import { RUNTIME_FIX_ACTIONS, type RuntimeFix, type RuntimeFixAction } from "../model/windows-runtime";
import { createExtras, createExtrasOf, createNetworks, kindOf, osOf, runtimeOf } from "../model/backend";
import { isCancelled } from "../model/errors";
import { cuaHomeOf, homeGrantsOf, writeHomeGrants } from "../model/home-grants";
import { bool, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { spaceReadyNote } from "./space-ready";
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
    // Another app keeps its own daemon and this app uses it (null otherwise).
    daemonNotice: model.daemonNotice,
    // Some of your computers have not answered yet (`Spaces.list_known`): the
    // page shows what is known and "Reaching N computers…", never the empty home.
    incomplete: model.listIncomplete,
    reaching: model.listReaching,
    // Your other computers' rows are the last known ones, not yet confirmed.
    stale: model.listStale,
    // Spaces your other computers are creating now: "Creating macOS on Mac mini… 40%".
    creatingElsewhere: model.creatingElsewhere,
  };
}

/**
 * The env the page's wizard runs on, in the shape of the web bridge's
 * `NewSpaceOptions` (plus `macosVmsRunning`; `pending` while the launch is
 * still starting, when every probe would wait).
 */
export function createOptions(env: AppWizardEnv, macosVmsRunning: number | null, pending: boolean, fix: RuntimeFix | null = null) {
  const out: Record<string, unknown> = {
    local: {
      available: env.localAvailable,
      backends: env.localBackends ?? [],
      error: env.localReason ?? null,
      // Windows without a runtime: what the app can do about it ("Install Docker Desktop").
      fix: env.localAvailable ? null : fix,
      hostArch: env.hostArch ?? null,
      storage: encode(env.storage),
    },
    gpus: encode(env.gpus),
    experiments: encode(env.experiments),
    maxCpus: env.maxCpus,
    env: encode(env),
    macosVmsRunning,
  };
  if (pending) out.pending = true;
  return out;
}

/** The page's `SpaceCreateConfig` (the core's `CreateSpaceArgs`) as the FFI record. */
export function createArgs(native: Native, c: BridgeArgs, defaultOn: string): AppCreateSpaceArgs {
  const image = c.image;
  if (typeof image !== "string" || image === "") throw Failure.badArgs("config.image: string");
  const kind = kindOf(native, typeof c.kind === "string" ? c.kind : "");
  if (!kind) throw Failure.badArgs("config.kind: container | vm | microvm");
  const runtime = runtimeOf(native, typeof c.runtime === "string" ? c.runtime : "auto");
  if (!runtime) throw Failure.badArgs("config.runtime: auto | gvisor | runc | qemu | lume | firecracker | vz | containerization | an external runtime's name");
  const count = (k: string) => (typeof c[k] === "number" && Number.isFinite(c[k]) && (c[k] as number) >= 0 ? Math.floor(c[k] as number) : undefined);
  const text = (k: string) => (typeof c[k] === "string" && c[k] !== "" ? (c[k] as string) : undefined);
  return {
    image,
    on: text("on") ?? defaultOn,
    kind,
    runtime,
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
  const { model, events, ui, system, platform } = ctx;
  const native = model.native;

  const ids = (args: BridgeArgs): string[] => {
    const v = args.ids;
    if (!Array.isArray(v) || v.some((x) => typeof x !== "string")) throw Failure.badArgs("ids: string[]");
    return v as string[];
  };

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
      return createOptions(env, model.runningMacosVms, false, model.runtimeFix);
    },
    "spaces.fixRuntime": (args) => {
      const action = string(args, "action");
      if (!(RUNTIME_FIX_ACTIONS as readonly string[]).includes(action)) throw Failure.badArgs(`action: ${RUNTIME_FIX_ACTIONS.join(" | ")}`);
      if (!model.windowsRuntime && !model.linuxRuntime) throw Failure.unsupported("no runtime fix on this system");
      model.fixRuntime(action as RuntimeFixAction);
      return null;
    },
    // `{config, pendingId, os?, notifyWhenReady?}`. The create belongs to the app, not to the window:
    // it runs on when the window is closed, and `notifyWhenReady` posts "<Space> is ready" when it succeeds.
    "spaces.create": async (args) => {
      const pendingId = string(args, "pendingId");
      if (!native.appCreatesIsPending(pendingId)) throw Failure.badArgs("pendingId: pending:<id>");
      const config = args.config;
      if (!config || typeof config !== "object" || Array.isArray(config)) throw Failure.badArgs("config: object");
      const defaultOn = "local"; // Cua Cloud has ended
      const a = createArgs(native, config as BridgeArgs, defaultOn);
      // The New Space's Network choice goes with the create (by its id).
      const net = (config as BridgeArgs).network;
      if (typeof net === "string" && net) createNetworks.set(pendingId, net);
      // An agent home: the spawn capability and its labels (CUA-2630).
      const extras = createExtrasOf(config as BridgeArgs);
      if (Object.keys(extras).length) createExtras.set(pendingId, extras);
      // The page sends the plan's OS (the pending row's icon).
      const os = (typeof args.os === "string" ? osOf(native, args.os) : undefined) ?? native.AppSpaceOs.Unknown;
      let id: string;
      try {
        id = await model.runCreate(
          a,
          os,
          pendingId,
          (p) => events.emit("spaces.createProgress", progress(p, pendingId)),
          // The model runs it once more: the page shows "Retrying…".
          (error, errorVariant) => events.emit("spaces.createProgress", { pendingId, phase: "preparing", detail: "", retry: { error, errorVariant } }),
        );
      } catch (error) {
        if (isCancelled(error)) throw new Failure("cancelled", "cancelled");
        throw error;
      } finally {
        createNetworks.delete(pendingId);
        createExtras.delete(pendingId);
      }
      const made = model.spaces.find((s) => s.id === id);
      // A create the first run started: "<Space> is ready", window or not.
      if (args.notifyWhenReady === true) {
        const known = made && made.os !== native.AppSpaceOs.Unknown ? made.os : os;
        system?.notify(spaceReadyNote({ id, name: made?.name ?? a.name ?? id, os: String(known) }, platform));
      }
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
    // `{ids}`: each row shows Deleting until its own result; `{results}` once all are done.
    "spaces.deleteMany": async (args) => ({ results: await model.bulk(ids(args), false) }),
    "spaces.leave": async (args) => ({ results: await model.bulk(ids(args), true) }),
    // `{spaceId, name}` -> `{}`: the new name comes with the next `spaces.list`.
    "spaces.rename": async (args) => {
      const id = string(args, "spaceId");
      const s = model.spaces.find((x) => x.id === id);
      if (!s) throw Failure.notFound(`no Space ${id}`);
      await model.rename(s, string(args, "name"));
      return {};
    },
    // A Space's network, clipboard and Volume settings (the SDK's).
    "network.get": async (args) => {
      const b = model.backend;
      if (!b.network) throw Failure.unsupported("network settings");
      return await b.network(string(args, "spaceId"));
    },
    "network.set": async (args) => {
      const b = model.backend;
      if (!b.setNetwork) throw Failure.unsupported("network settings");
      const list = (k: string) => (Array.isArray(args[k]) ? (args[k] as unknown[]).filter((x): x is string => typeof x === "string") : []);
      const via = typeof args.via === "string" ? args.via : undefined;
      const applied = await b.setNetwork(
        string(args, "spaceId"),
        string(args, "network"),
        list("allow"),
        list("deny"),
        args.widen === true,
        args.tree === true,
        via,
      );
      return { applied };
    },
    "network.log": async (args) => {
      const b = model.backend;
      return b.networkLog ? await b.networkLog(string(args, "spaceId")) : [];
    },
    "network.ackNotice": async (args) => {
      await model.backend.ackNetworkNotice?.(string(args, "spaceId"));
      return {};
    },
    "spaces.setAccess": async (args) => {
      const b = model.backend;
      const word = (k: string) => (typeof args[k] === "string" ? (args[k] as string) : null);
      const [clipboard, volume] = [word("clipboard"), word("volume")];
      // An agent home's grants (CUA-2630): the file this machine's daemon reads.
      const home = homeGrantsOf(args.home);
      if (home) await writeHomeGrants(cuaHomeOf(ctx.env), string(args, "spaceId"), home);
      if (home && clipboard === null && volume === null) return {};
      if (!b.setAccess) throw Failure.unsupported("these settings");
      await b.setAccess(string(args, "spaceId"), clipboard, volume);
      return {};
    },
    // Child Spaces: the tree of Spaces, children, budgets and kept services.
    "spaces.tree": async () => {
      const b = model.backend;
      return b.spaceTree ? await b.spaceTree() : [];
    },
    "children.set": async (args) => {
      const b = model.backend;
      if (!b.setChildren) throw Failure.unsupported("child Spaces");
      await b.setChildren(string(args, "spaceId"), bool(args, "enabled"));
      return {};
    },
    "budget.get": async (args) => {
      const b = model.backend;
      if (!b.treeBudget) throw Failure.unsupported("child Spaces");
      return await b.treeBudget(string(args, "spaceId"));
    },
    "budget.set": async (args) => {
      const b = model.backend;
      if (!b.setTreeBudget) throw Failure.unsupported("child Spaces");
      const l = (args.limits ?? {}) as Record<string, unknown>;
      const n = (k: string) => (typeof l[k] === "number" && Number.isFinite(l[k]) && (l[k] as number) >= 0 ? (l[k] as number) : null);
      return await b.setTreeBudget(string(args, "spaceId"), {
        maxSpaces: n("maxSpaces"),
        maxVcpus: n("maxVcpus"),
        maxMemoryMb: n("maxMemoryMb"),
        usdPerHour: n("usdPerHour"),
        usdTotal: n("usdTotal"),
      });
    },
    "services.list": async (args) => {
      const b = model.backend;
      return b.services ? await b.services(string(args, "spaceId")) : [];
    },
    "services.control": async (args) => {
      const b = model.backend;
      if (!b.controlService) throw Failure.unsupported("services");
      const action = string(args, "action");
      if (!["start", "stop", "restart", "remove"].includes(action)) throw Failure.badArgs(`action: start, stop, restart or remove`);
      await b.controlService(string(args, "spaceId"), string(args, "name"), action);
      return {};
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
