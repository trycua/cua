// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Live Spaces data for the portal: a context handing components the shell
 * bridge plus whether real data is flowing, and a hook that keeps the
 * reducer's Space list in sync with the cua SDK's Spaces registry.
 *
 * The registry (`list_spaces`) covers every location (Cua Cloud, this Mac,
 * Spaces added by address) and works without cloud credentials, so `live`
 * simply means "running in the native shell". Outside
 * it (browser dev, Vitest) the portal keeps its synthetic fixtures, which is
 * also what keeps the existing tests hermetic.
 */

import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";

import {
  composeCreates,
  createsRunning,
  isCancelledError,
  isDeleting,
  isPowering,
  isPendingCreate,
  NO_CREATES,
  reduceCreates,
  settleCreates,
  type CreateAction,
  type CreatesState,
} from "../model/creates";
import { withThisMachine } from "../model/host";
import { createsSignals } from "../model/telemetry";
import { telemetryBridge } from "../native/telemetry";
import { rowToSpace, SPACES_CONFIG, type SpaceRow } from "../model/spaces";
import type { HostStatus } from "../native/host";
import type { Location, Space, SpaceKind, SpaceOs } from "../model/types";
import {
  BUILTIN_DEFAULT_LOCATION,
  createFallbackFleetBridge,
  type DefaultLocation,
  type FleetBridge,
  type SpaceCreateConfig,
  type ViewerWindowRequest,
} from "../native/fleet";
import type { PortalAction } from "./portal";

export interface FleetContextValue {
  /** True when the native shell's Spaces registry drives the switcher. */
  live: boolean;
  bridge: FleetBridge;
}

export const FleetContext = createContext<FleetContextValue>({
  live: false,
  bridge: createFallbackFleetBridge(),
});

export function useFleet(): FleetContextValue {
  return useContext(FleetContext);
}

/** Window request for a registered Space; null for fixtures and placeholders. */
export function viewerRequestFor(space: Space): ViewerWindowRequest | null {
  if (!space.sdk) return null;
  return {
    id: space.id,
    name: space.name,
    // Running/approval Spaces are agent-driven; the PiP badge reflects that.
    controller: space.status === "approval" ? "agent" : "you",
    os: space.os,
  };
}

export interface FleetSync {
  live: boolean;
  /** Whether Cua Cloud credentials are configured in the shell. */
  fleetConfigured: boolean;
  /** OAuth client id reported by the shell, when signed in; shown in Settings. */
  clientId?: string;
  /** Signed-in user identity (email/subject), when a user session is active. */
  identity?: string;
  /** Re-read the registry and sync the reducer now. */
  refresh: () => Promise<void>;
  /** Where new Spaces go when a create names no location. */
  defaultLocation: DefaultLocation;
  /** Store a new default location (Settings). */
  setDefaultLocation: (on: Location) => Promise<DefaultLocation>;
  /** Reads the default location again (after "Make default" connected a cloud). */
  refreshDefaultLocation: () => Promise<void>;
  /** Create a Space (in the default location unless `config.on` says):
   * its row shows at once and follows the SDK's progress; resolves with its
   * id once it answers. A failure stays on the row (and rejects). */
  createSpace: (config?: SpaceCreateConfig, pendingId?: string) => Promise<string>;
  /** Removes a failed create's row. */
  dismissCreate: (pendingId: string) => void;
  /** Cancels a create still running: its row shows Cancelling until the
   * SDK stopped it and removed what it made, then goes; a cancel that
   * fails says so on the row. */
  cancelCreate: (pendingId: string) => Promise<void>;
  /** "Add Space by address"; resolves with its id. */
  addSpace: (url: string, token?: string, name?: string) => Promise<string>;
  /** Delete a created Space; a Space added by address is only forgotten.
   * `removeOnly` forgets any Space and keeps it running (Remove from List
   * for a Space in your cloud). */
  deleteSpace: (space: Space, removeOnly?: boolean) => Promise<void>;
  /** The power button: turns a Space off (`on` false) or back on. Its row
   * shows Suspending (and the like) at once; a failure shows inline. */
  setPower: (space: Space, on: boolean) => Promise<void>;
  /** The always-present "This machine" entry: the host status it reads
   * (null while it loads). The core puts it first (`host.withThisMachine`). */
  setThisMachine: (status: HostStatus | null) => void;
  /** This Mac's CPU architecture, once known (a local Space of another one
   * warns on its Architecture fact). */
  hostArch: string | null;
}

/** How often a running create's bar moves (4 Hz, as in the SwiftUI app). */
export const CREATE_TICK_MS = 250;

function uid(): string {
  return globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
}

export function useFleetSync(
  fleet: FleetBridge,
  dispatch: (action: PortalAction) => void,
  now: () => number,
): FleetSync {
  const live = fleet.isNative;
  const [fleetConfigured, setFleetConfigured] = useState(false);
  const [clientId, setClientId] = useState<string | undefined>(undefined);
  const [identity, setIdentity] = useState<string | undefined>(undefined);
  const [defaultLocation, setDefaultLocationState] = useState<DefaultLocation>(BUILTIN_DEFAULT_LOCATION);
  /**
   * Spaces being created (the app core's `spaces::creating`): each shows the
   * instant its create starts, follows the SDK's create progress, and gives
   * way to the registry's row once it is ready. A failed one stays, with
   * its error, until dismissed.
   */
  const createsRef = useRef<CreatesState>(NO_CREATES);
  /** This Mac's CPU architecture (a pending create's platform, the
   * emulation warning), once the shell said. */
  const [hostArch, setHostArch] = useState<string | null>(null);
  const hostArchRef = useRef<string | null>(null);
  /** A create is running: the progress timer ticks. */
  const [creating, setCreating] = useState(false);
  /** The last good registry rows, so a failed refresh never blanks the list. */
  const rowsRef = useRef<SpaceRow[]>([]);
  /** "This machine": always first in the live roster, once the App set it
   * (`undefined` until then). */
  const thisMachineRef = useRef<HostStatus | null | undefined>(undefined);

  const publish = useCallback(() => {
    const timestamp = now();
    let spaces = rowsRef.current.map((row) => rowToSpace(row, timestamp));
    if (live && thisMachineRef.current !== undefined) spaces = withThisMachine(spaces, thisMachineRef.current, timestamp);
    dispatch({ type: "sync-spaces", spaces: composeCreates(spaces, createsRef.current) });
  }, [dispatch, now, live]);

  const sendCreate = useCallback(
    (action: CreateAction) => {
      // A create started, reached ready or failed: the usage events the app
      // core derives (the SwiftUI app sends the same ones).
      telemetryBridge().recordSignals(createsSignals(createsRef.current, action, now()));
      createsRef.current = reduceCreates(createsRef.current, action);
      setCreating(createsRunning(createsRef.current));
      publish();
    },
    [publish, now],
  );

  const setThisMachine = useCallback(
    (status: HostStatus | null) => {
      thisMachineRef.current = status;
      if (live) publish();
    },
    [live, publish],
  );

  const refresh = useCallback(async () => {
    try {
      rowsRef.current = await fleet.listSpaces();
      // Finished creates the registry lists, finished deletes it dropped
      // (by id: the timestamp does not matter).
      createsRef.current = settleCreates(
        createsRef.current,
        rowsRef.current.map((row) => rowToSpace(row, 0)),
      );
    } finally {
      publish();
    }
  }, [fleet, publish]);

  /** Show the new Space's row while `create` runs (following its
   * progress), then hand over to the registry's row. */
  const withPendingRow = useCallback(
    async (
      pendingId: string,
      name: string,
      os: SpaceOs,
      provider: Location | "relay",
      create: () => Promise<SpaceRow>,
      image?: string,
      kind?: SpaceKind,
      gpu?: boolean,
    ) => {
      sendCreate({
        type: "start",
        id: pendingId,
        name,
        os,
        provider,
        now: now(),
        image: image ?? null,
        kind: kind ?? null,
        hostArch: hostArchRef.current,
        gpu: gpu ?? false,
      });
      let row: SpaceRow;
      try {
        row = await create();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        // Cancelled (here or by another client, `cua spaces cancel`): the
        // row goes; it did not fail.
        sendCreate(
          isCancelledError(message)
            ? { type: "cancel-done", id: pendingId }
            : { type: "fail", id: pendingId, error: message },
        );
        throw error;
      }
      sendCreate({ type: "finish", id: pendingId, spaceId: row.id });
      await refresh().catch(() => {});
      // The registry lists it now: the pending row has already given way.
      sendCreate({ type: "dismiss", id: pendingId });
      return row.id;
    },
    [now, sendCreate, refresh],
  );

  // The SDK's create progress, for every pending row.
  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void Promise.resolve(
      fleet.onCreateProgress?.((p) =>
        sendCreate({
          type: "progress",
          id: p.pendingId,
          phase: p.phase,
          fraction: p.fraction ?? null,
          now: now(),
          bytesDone: p.bytesDone ?? null,
          bytesTotal: p.bytesTotal ?? null,
          bytesPerSecond: p.bytesPerSecond ?? null,
        }),
      ),
    )
      .then((u) => {
        if (cancelled) u?.();
        else unsub = u;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unsub?.();
    };
  }, [fleet, sendCreate, now]);

  // While a create runs, time moves its bar within a phase that reports no
  // fraction (the app core computes it from `now`, as the SwiftUI app does).
  useEffect(() => {
    if (!creating) return;
    const timer = window.setInterval(() => sendCreate({ type: "tick", now: now() }), CREATE_TICK_MS);
    return () => window.clearInterval(timer);
  }, [creating, sendCreate, now]);

  useEffect(() => {
    let cancelled = false;
    void Promise.resolve(fleet.hostArch?.())
      .then((arch) => {
        if (!arch || cancelled) return;
        hostArchRef.current = arch;
        setHostArch(arch);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet]);

  const applyStatus = useCallback(() => {
    return fleet
      .status()
      .then((status) => {
        setFleetConfigured(status.configured);
        setClientId(status.clientId);
        setIdentity(status.identity);
      })
      .catch(() => {});
  }, [fleet]);

  useEffect(() => {
    void applyStatus();
  }, [applyStatus]);

  useEffect(() => {
    let cancelled = false;
    void Promise.resolve(fleet.defaultLocation?.())
      .then((d) => {
        if (d && !cancelled) setDefaultLocationState(d);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet]);

  const refreshDefaultLocation = useCallback(async () => {
    const d = await Promise.resolve(fleet.defaultLocation?.()).catch(() => undefined);
    if (d) setDefaultLocationState(d);
  }, [fleet]);

  const setDefaultLocation = useCallback(
    async (on: Location) => {
      const next = await fleet.setDefaultLocation(on);
      setDefaultLocationState(next);
      return next;
    },
    [fleet],
  );

  // Re-sync when the user signs in or out (device grant).
  useEffect(() => {
    const unsubs: Array<() => void> = [];
    let cancelled = false;
    const track = (pending: Promise<() => void> | undefined) => {
      void Promise.resolve(pending)
        .then((unsub) => {
          if (!unsub) return;
          if (cancelled) unsub();
          else unsubs.push(unsub);
        })
        .catch(() => {});
    };
    track(
      fleet.onSignedIn?.((id) => {
        if (id) setIdentity(id);
        void applyStatus();
        void refresh().catch(() => {});
      }),
    );
    track(
      fleet.onSignedOut?.(() => {
        setIdentity(undefined);
        void applyStatus();
      }),
    );
    // Any window (or the MCP-driven control path) changing the registry.
    track(fleet.onSpacesChanged?.(() => void refresh().catch(() => {})));
    return () => {
      cancelled = true;
      for (const unsub of unsubs) unsub();
    };
  }, [fleet, applyStatus, refresh]);

  // Poll the registry while live.
  useEffect(() => {
    if (!live) return;
    void refresh().catch((error: unknown) => {
      console.error("[Cua Spaces] registry refresh failed", error);
    });
    const timer = window.setInterval(() => {
      void refresh().catch(() => {});
    }, SPACES_CONFIG.refreshMs);
    return () => window.clearInterval(timer);
  }, [live, refresh]);

  const createSpace = useCallback(
    (config?: SpaceCreateConfig, pendingId?: string) => {
      const on = config?.on ?? defaultLocation.value;
      const macos = config?.runtime === "lume" || /\/macos[:-]/.test(config?.image ?? "");
      const windows = /\/windows[:-]/.test(config?.image ?? "");
      const id = pendingId ?? `pending:${uid()}`;
      const kind = config?.kind === "container" || config?.kind === "vm" ? config.kind : undefined;
      return withPendingRow(
        id,
        config?.name ?? "",
        macos ? "macos" : windows ? "windows" : "linux",
        // A Space in the user's own cloud lists as a relay machine.
        on === "local" || on === "cloud" ? on : "relay",
        () => fleet.createSpace(config, id),
        config?.image,
        kind,
        Boolean(config?.gpu),
      );
    },
    [fleet, withPendingRow, defaultLocation.value],
  );

  const dismissCreate = useCallback((pendingId: string) => sendCreate({ type: "dismiss", id: pendingId }), [sendCreate]);

  const cancelCreate = useCallback(
    async (pendingId: string) => {
      if (!fleet.cancelCreate) return;
      sendCreate({ type: "cancel-start", id: pendingId });
      try {
        await fleet.cancelCreate(pendingId);
      } catch (error) {
        sendCreate({
          type: "cancel-fail",
          id: pendingId,
          error: error instanceof Error ? error.message : String(error),
        });
        return;
      }
      sendCreate({ type: "cancel-done", id: pendingId });
    },
    [fleet, sendCreate],
  );

  const addSpace = useCallback(
    async (url: string, token?: string, name?: string) => {
      const row = await fleet.addSpace(url, token, name);
      await refresh().catch(() => {});
      return row.id;
    },
    [fleet, refresh],
  );

  const deleteSpace = useCallback(
    async (space: Space, removeOnly = false) => {
      // A failed create is only removed from the list.
      if (isPendingCreate(space.id)) {
        sendCreate({ type: "dismiss", id: space.id });
        return;
      }
      if (!space.sdk) return;
      // The row shows Deleting at once (registry probes during the delete
      // cannot undo it); a second Delete does nothing.
      if (isDeleting(createsRef.current, space.id)) return;
      sendCreate({ type: "delete-start", id: space.id, now: now() });
      try {
        if (removeOnly) await fleet.removeSpace(space.id);
        else await fleet.deleteSpace(space.id);
      } catch (error) {
        sendCreate({ type: "delete-fail", id: space.id });
        throw error;
      }
      // Gone now, even while the registry still lists it.
      sendCreate({ type: "delete-done", id: space.id });
      await refresh().catch(() => {});
    },
    [fleet, refresh, sendCreate, now],
  );

  const setPower = useCallback(
    async (space: Space, on: boolean) => {
      if (!fleet.setSpacePower || isPowering(createsRef.current, space.id)) return;
      sendCreate({ type: "power-start", id: space.id, on, now: now() });
      try {
        await fleet.setSpacePower(space.id, on);
      } catch (error) {
        sendCreate({
          type: "power-fail",
          id: space.id,
          error: error instanceof Error ? error.message : String(error),
        });
        return;
      }
      // The row says so until the registry shows it on (or off).
      sendCreate({ type: "power-done", id: space.id });
      await refresh().catch(() => {});
    },
    [fleet, refresh, sendCreate, now],
  );

  return {
    live,
    fleetConfigured,
    clientId,
    identity,
    refresh,
    defaultLocation,
    refreshDefaultLocation,
    setDefaultLocation,
    createSpace,
    dismissCreate,
    cancelCreate,
    addSpace,
    deleteSpace,
    setPower,
    setThisMachine,
    hostArch,
  };
}
