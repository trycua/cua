// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** What a demo feature's handlers get from the demo host (`../demo.ts`). */

import type { HostEvent, OpArgs, OpName, OpResult } from "../../protocol";
import type { SpaceRow } from "../../contracts/spaces";
import type { DemoState } from "../demo";

export interface DemoContext {
  state: DemoState;
  now: () => number;
  /** Resolves after `ms` (cleared when the demo host is disposed). */
  wait: (ms: number) => Promise<void>;
  emit: (e: HostEvent) => void;
  /** The row, or a `not_found` HostError. */
  findRow: (id: string) => SpaceRow;
  /** Time per create phase and per power change (ms). */
  stepMs: number;
}

export type DemoHandlers<K extends OpName> = { [P in K]: (args: OpArgs<P>) => Promise<OpResult<P>> | OpResult<P> };
