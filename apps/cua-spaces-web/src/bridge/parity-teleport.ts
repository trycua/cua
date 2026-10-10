// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The teleport picker, its grid and the Share sheet in the parity replay
 * (`parity.ts`): the core methods those screens call, answered through the
 * bridge's own functions (`teleport.ts`, `share.ts`), and the states they
 * draw, recorded for `e2e/teleport.ts` to put on the real screens.
 */

import type { CoreClient } from "./core";
import type { ShareInput, ShareSheetState, ShareSheetView } from "./contracts/share";
import type {
  OpenWindow,
  PickerFrame,
  PickerGrid,
  PickerGridPrimary,
  PickerGridTab,
  PickerGridTabItem,
  PickerState,
  RemoteWindow,
} from "./contracts/teleport";
import { shareInitial, shareReduce, shareStore, shareView } from "./share";
import type { BridgeStore } from "./store";
import {
  appGrid,
  gridPrimary,
  gridStep,
  gridTabs,
  pickerCanPlan,
  pickerConsent,
  pickerFrame,
  pickerInitial,
  pickerPlanSensitive,
  pickerProgress,
  pickerReduce,
  pickerReview,
  pickerSections,
  pickerSensitiveOptions,
  pickerStatus,
  remoteGrid,
  teleportStore,
  windowGrid,
} from "./teleport";

/** What one tab of the grid was drawn from. */
export interface GridShown {
  tab: PickerGridTab;
  spaceName: string;
  state?: PickerState;
  windows?: OpenWindow[];
  remote?: RemoteWindow[];
  query?: string;
  selected?: string | null;
}

export type TeleportCheckpoint =
  /** A picker state and what the core says it draws. */
  | { kind: "picker"; state: PickerState; frame: PickerFrame }
  | { kind: "grid-tabs"; spaceName: string; tabs: PickerGridTabItem[] }
  | { kind: "grid"; shown: GridShown; grid: PickerGrid }
  | { kind: "grid-primary"; tab: PickerGridTab; spaceName: string; grid: PickerGrid; primary: PickerGridPrimary }
  | { kind: "share"; input: ShareInput; state: ShareSheetState; view: ShareSheetView };

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export function teleportParityMethods(core: CoreClient, record: (c: TeleportCheckpoint) => void): Record<string, (a: Args) => unknown> {
  let spaceName = "";
  const picker = (state: PickerState) => {
    spaceName = state.spaceName;
    record({ kind: "picker", state, frame: pickerFrame(core, state) });
    return state;
  };
  const grid = (shown: GridShown, g: PickerGrid) => {
    record({ kind: "grid", shown, grid: g });
    return g;
  };
  return {
    "flow.initial": (a) => picker(pickerInitial(core, a.spaceName)),
    "flow.reduce": (a) => picker(pickerReduce(core, a.state, a.event)),
    "flow.sections": (a) => pickerSections(core, a.state),
    "flow.review": (a) => pickerReview(core, a.state),
    "flow.canPlan": (a) => pickerCanPlan(core, a.state),
    "flow.progress": (a) => pickerProgress(core, a.state),
    "flow.status": (a) => pickerStatus(core, a.state),
    "flow.sensitiveOptions": (a) => pickerSensitiveOptions(core, a.state),
    "flow.planSensitive": (a) => pickerPlanSensitive(core, a.state),
    "flow.consent": (a) => pickerConsent(core, a.state),
    "grid.tabs": (a) => {
      spaceName = a.spaceName;
      const tabs = gridTabs(core, a.spaceName);
      record({ kind: "grid-tabs", spaceName: a.spaceName, tabs });
      return tabs;
    },
    "grid.apps": (a) => grid({ tab: "apps", spaceName, state: a.state, windows: a.windows }, appGrid(core, a.state, a.windows)),
    "grid.windows": (a) =>
      grid({ tab: "windows", spaceName, windows: a.windows, query: a.query, selected: a.selected }, windowGrid(core, a.windows, a.query, a.selected)),
    "grid.remote": (a) =>
      grid({ tab: "space", spaceName, remote: a.windows, query: a.query, selected: a.selected }, remoteGrid(core, a.windows, a.query, a.selected)),
    "grid.step": (a) => gridStep(core, a.grid, a.selected, a.delta),
    "grid.primary": (a) => {
      const primary = gridPrimary(core, a.tab, a.spaceName, a.grid);
      record({ kind: "grid-primary", tab: a.tab, spaceName: a.spaceName, grid: a.grid, primary });
      return primary;
    },
    "share.initial": () => shareInitial(core),
    "share.reduce": (a) => shareReduce(core, a.input, a.state, a.action),
    "share.view": (a) => {
      const view = shareView(core, a.input, a.state);
      record({ kind: "share", input: a.input, state: a.state, view });
      return view;
    },
  };
}

export interface TeleportParityHandle {
  /** Puts a picker state on the screen. */
  showTeleport(state: PickerState): void;
  /** Puts one tab of the grid on the screen. */
  showTeleportGrid(shown: GridShown): void;
  /** Puts the Share sheet on the screen. */
  showShare(input: ShareInput, state: ShareSheetState): void;
}

export function teleportParityHandle(store: BridgeStore): TeleportParityHandle {
  return {
    showTeleport: (state) => teleportStore(store).show(state.spaceName, state),
    showTeleportGrid: ({ spaceName, ...g }) => teleportStore(store).showGrid(spaceName, g),
    showShare: (input, state) => shareStore(store).show(input, state),
  };
}
