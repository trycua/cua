// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The host operations New Space and "Connect a cloud" add. Each is an
 * existing Tauri command under a host-neutral name:
 *
 * - `spaces.createOptions`: `local_status`, `gpu_support`, `cloud_pricing`
 *   and the stored experiments (the wizard's `WizardEnv` beyond Spaces,
 *   machines, settings and the session).
 * - `spaces.add`: `add_space` ("Connect by address").
 * - `clouds.status`, `clouds.test`, `clouds.connect`: `cloud_tool` with
 *   `cloud_status`, `cloud_test` (creates nothing) and `cloud_connect`.
 *
 * The SwiftUI host answers `spaces.createOptions` with the env its native
 * sheet opens with (`NewSpaceOptions.env`, from `AppModel.newSpaceEnv`), so
 * the page's wizard runs there too, plus `macosVmsRunning`, and `pending`
 * while its launch is still starting; the native sheet is only the fallback
 * while New UI is off. The Electron shell answers the same methods.
 *
 * The Swift host routes `spaces.createOptions` (`WebUIBridge.swift`),
 * `spaces.add` (`AppModel.addByAddress`) and `clouds.*`
 * (`CloudToolRunning.cloudTool`, `WebUIBridge+Pages.swift`) under these
 * names.
 *
 * Shared files register these with one line each: `protocol.ts`
 * (HostOperations, OPERATIONS), `coverage.ts`, and the adapters (tauri,
 * webkit, demo).
 */

import type {
  CloudConnectWire,
  CloudStatusWire,
  CloudTarget,
  CloudTestWire,
  GpuChoice,
  CloudPricing,
  LocalStatus,
  NewSpaceOptions,
} from "../contracts/new-space";
import type { SpaceRow } from "../contracts/spaces";
import { readExperiments } from "./volume";

export interface NewSpaceOperations {
  "spaces.createOptions": { args: Record<string, never>; result: NewSpaceOptions };
  /** Adds a running spacesd by its address (`add_space`). */
  "spaces.add": { args: { url: string; token?: string | null; name?: string | null }; result: SpaceRow };
  "clouds.status": { args: Record<string, never>; result: CloudStatusWire };
  "clouds.test": { args: { target: CloudTarget }; result: CloudTestWire };
  "clouds.connect": { args: { target: CloudTarget; makeDefault: boolean }; result: CloudConnectWire };
}

type Name = keyof NewSpaceOperations;
type Ops = { [K in Name]: (args: NewSpaceOperations[K]["args"]) => Promise<NewSpaceOperations[K]["result"]> };

export const NEW_SPACE_OPERATIONS = [
  "spaces.createOptions",
  "spaces.add",
  "clouds.status",
  "clouds.test",
  "clouds.connect",
] as const satisfies readonly Name[];

/** `coverage.ts` rows. */
export const NEW_SPACE_COVERAGE = {
  "spaces.createOptions": {
    webkit: { methods: ["spaces.createOptions"] },
    tauri: ["local_status", "gpu_support", "cloud_pricing"],
  },
  "spaces.add": { webkit: { methods: ["spaces.add"] }, tauri: ["add_space"] },
  "clouds.status": { webkit: { methods: ["clouds.status"] }, tauri: ["cloud_tool"] },
  "clouds.test": { webkit: { methods: ["clouds.test"] }, tauri: ["cloud_tool"] },
  "clouds.connect": { webkit: { methods: ["clouds.connect"] }, tauri: ["cloud_tool"] },
} as const;

/** Arguments that take each operation down its usual path (coverage tests). */
export const NEW_SPACE_ARGS: { [K in Name]: NewSpaceOperations[K]["args"] } = {
  "spaces.createOptions": {},
  "spaces.add": { url: "studio.local:7400", token: null, name: null },
  "clouds.status": {},
  "clouds.test": { target: { provider: "aws", region: "us-west-2" } },
  "clouds.connect": { target: { provider: "aws", region: "us-west-2" }, makeDefault: false },
};

/** The Tauri app keeps its experiments in UI storage (`model/experiments.ts`). */
const EXPERIMENTS_KEY = "cua.settings.experiments";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** The Tauri shell's answers (`adapters/tauri.ts`). */
export function newSpaceTauriOps(invoke: Invoke, readUi: (key: string) => string | undefined): Ops {
  const tool = <T>(name: string, args: Record<string, unknown>) => invoke<T>("cloud_tool", { tool: name, args });
  return {
    "spaces.createOptions": async () => {
      const [local, gpus, cloudPricing] = await Promise.all([
        invoke<LocalStatus | null>("local_status").catch(() => null),
        invoke<GpuChoice[] | null>("gpu_support").catch(() => null),
        invoke<CloudPricing | null>("cloud_pricing").catch(() => null),
      ]);
      return { local: local ?? null, gpus: gpus ?? null, cloudPricing: cloudPricing ?? null, experiments: readExperiments(readUi(EXPERIMENTS_KEY)), maxCpus: null };
    },
    "spaces.add": ({ url, token, name }) => invoke<SpaceRow>("add_space", { url, token: token || null, name: name || null }),
    "clouds.status": async () => (await tool<CloudStatusWire | null>("cloud_status", {})) ?? { providers: [] },
    "clouds.test": ({ target }) => tool<CloudTestWire>("cloud_test", { ...target }),
    "clouds.connect": ({ target, makeDefault }) => tool<CloudConnectWire>("cloud_connect", { ...target, make_default: makeDefault }),
  };
}

