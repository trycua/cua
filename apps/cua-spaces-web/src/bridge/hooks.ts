// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { BridgeMode, DataAdapter } from "./adapter";
import { BridgeContext, type BridgeContextValue } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { AgentSetupRow } from "./contracts/agents";
import type { MachinesView, OnboardingMode, PermissionHint, SignInStart } from "./contracts/host";
import type {
  OnboardingAction,
  OnboardingCheckbox,
  OnboardingCopy,
  OnboardingFlowState,
  OnboardingView,
  PermissionRow,
  SandboxImage,
  OnboardingTodoId,
} from "./contracts/onboarding";
import { NO_EXPERIMENTS, type Experiments } from "./contracts/volume";
import type { KvGrant, KvListView, KvSelection, VaultAction } from "./contracts/keyvault";
import type { KvAccessCommand } from "./ops/keyvault-manage";
import { LIST_COMPLETE, type ListStatus, type Space } from "./contracts/spaces";
import { hostSetupGuide, type HostSetupGuide, type Machine } from "./derive";
import {
  firstSpaceImages,
  onboardingCopy,
  onboardingInitial,
  onboardingReduce,
  onboardingTelemetry,
  onboardingView,
  permissionRows,
  readSavedOnboarding,
  signInCodeText,
  signedInText,
  subscribeSavedOnboarding,
  writeSavedOnboarding,
  type SavedOnboarding,
} from "./onboarding";
import type { SettingKey, SettingsValues } from "./protocol";
import type {
  AgentsData,
  AgentTimeline,
  BridgeStore,
  CreateSpaceRequest,
  KeyvaultData,
  Resource,
  ResourceName,
  RunRef,
  SessionData,
  SettingsData,
} from "./store";
import type { CreateReport, ReportSend } from "./diagnostic-report";

function useBridgeContext(): BridgeContextValue {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  return ctx;
}

const LOADING: Resource<never> = { data: undefined, isLoading: true, error: null };
const noopSubscribe = () => () => {};

function useResource<T>(name: ResourceName): Resource<T> & { refresh: () => Promise<void> } {
  const { store, ready } = useBridgeContext();
  useEffect(() => {
    store?.ensure(name);
  }, [store, name]);
  const resource = useSyncExternalStore(
    store ? store.subscribe : noopSubscribe,
    () => (store ? store.get<T>(name) : (LOADING as Resource<T>)),
    () => LOADING as Resource<T>,
  );
  const shown = store && resource.data === undefined && !resource.error ? { ...resource, isLoading: true } : resource;
  return { ...shown, refresh: () => ready.then((s) => s.refresh(name)) };
}

/** Runs `f` on the store once it exists (actions called while the core loads wait). */
function useAction(): <R>(f: (s: BridgeStore) => Promise<R> | R) => Promise<R> {
  const { ready } = useBridgeContext();
  return (f) => ready.then(f);
}

/**
 * Records a Spaces app feature (a fixed name such as `agents_page_open`) once
 * each time `when` turns true, as the SwiftUI and Tauri apps do when the page
 * or sheet opens. The host and the machine's telemetry switch decide whether
 * anything is sent.
 */
export function useFeatureRecorded(feature: string, when = true): void {
  // Not an error without a provider: a screen drawn on its own records nothing.
  const ready = useContext(BridgeContext)?.ready;
  useEffect(() => {
    if (!when || !ready) return;
    void ready.then((s) => s.telemetry.track([{ type: "feature", feature }]));
  }, [ready, feature, when]);
}

/** Records one named feature now, for a click or a step rather than a page opening. */
export function useFeatureRecorder(): (feature: string) => void {
  const ready = useContext(BridgeContext)?.ready;
  return useCallback(
    (feature: string) => {
      void ready?.then((s) => s.telemetry.track([{ type: "feature", feature }]));
    },
    [ready],
  );
}

/** The host, the app core and the raw adapter. */
export function useBridge(): { mode: BridgeMode; core: CoreClient; data: DataAdapter | null } {
  const { mode, core, adapter } = useBridgeContext();
  return { mode, core, data: adapter };
}

export interface SpacesHook extends Resource<Space[]> {
  refresh(): Promise<void>;
  /** Starts a create; its row shows at once (status `provisioning` with
   * `progress`) and the promise settles when the Space is ready. The image
   * reference is checked by the app core first. `pendingId`
   * (`newPendingId()`): the row's id, for a caller that follows it. */
  createSpace(request?: CreateSpaceRequest, pendingId?: string): Promise<Space>;
  /** Cancels the create whose pending row id (`pending:...`) is given. */
  cancelCreate(pendingId: string): Promise<void>;
  /** Clears a failed create's row. */
  dismissCreate(pendingId: string): Promise<void>;
  /** Try again on a failed create: the same request as a new create; the
   * new row's pending id (undefined when this page never saw the request). */
  retryCreate(pendingId: string): string | undefined;
  /** Whether Try again can run the failed create again. */
  canRetryCreate(pendingId: string): boolean;
  /** A failed create's diagnostic report, redacted, for the review sheet
   * (null when there is none). Nothing is sent. */
  createReport(pendingId: string): Promise<CreateReport | null>;
  /** Send on the review sheet: opens an email to Cua support with the kept parts. */
  sendCreateReport(send: ReportSend): Promise<void>;
  startSpace(id: string): Promise<void>;
  stopSpace(id: string): Promise<void>;
  /** Deletes the Space, or with `removeOnly` only removes it from the list. */
  deleteSpace(id: string, removeOnly?: boolean): Promise<void>;
  /** Opens the Space's desktop in its own native window. */
  openSpace(id: string): Promise<void>;
  /** The Space a finished create became, by its row's pending id. */
  createdId(pendingId: string): string | undefined;
  /** The host's last registry read failed: why the rows may be out of date
   * (the SwiftUI app's `rosterError`). Null when it worked. */
  listNotice: string | null;
  /** How far the list got: while some of the account's computers have not
   * answered (or nothing was read yet) it is not "no Spaces" (`listView`). */
  listStatus: ListStatus;
  /** The host's standing notice (another app keeps its own daemon). Null when none. */
  hostNotice: string | null;
}

/** Nothing read yet (no store). */
const NOT_LOADED: ListStatus = { ...LIST_COMPLETE, loaded: false };

export function useSpaces(): SpacesHook {
  const ctx = useBridgeContext();
  const r = useResource<Space[]>("spaces");
  // Read through the store's subscription, not off `ctx.store` in render: the compiler keeps a property read
  // of an unchanged object from an earlier render, and the page would keep "not read yet" forever.
  const store = ctx.store;
  const listStatus = useSyncExternalStore(store ? store.subscribe : noopSubscribe, () => store?.listStatus ?? NOT_LOADED, () => NOT_LOADED);
  const listNotice = useSyncExternalStore(store ? store.subscribe : noopSubscribe, () => store?.listNotice ?? null, () => null);
  const run = useAction();
  return {
    ...r,
    createSpace: (req, pendingId) => run((s) => s.createSpace(req, pendingId)),
    cancelCreate: (id) => run((s) => s.cancelCreate(id)),
    dismissCreate: (id) => run((s) => s.dismissCreate(id)),
    retryCreate: (id) => (ctx.store?.canRetryCreate(id) ? ctx.store.retryCreate(id) : undefined),
    canRetryCreate: (id) => ctx.store?.canRetryCreate(id) ?? false,
    createReport: async (id) => (ctx.store ? ctx.store.createReport(id) : null),
    sendCreateReport: (send) => run((s) => s.sendCreateReport(send)),
    startSpace: (id) => run((s) => s.setPower(id, true)),
    stopSpace: (id) => run((s) => s.setPower(id, false)),
    deleteSpace: (id, removeOnly) => run((s) => s.deleteSpace(id, removeOnly)),
    openSpace: (id) => run((s) => s.openSpace(id)),
    createdId: (id) => ctx.store?.createdId(id),
    listNotice,
    listStatus,
    hostNotice: ctx.store?.hostNotice ?? null,
  };
}

export interface MachinesHook extends Resource<Machine[]> {
  refresh(): Promise<void>;
  /** The Machines page as every UI draws it (`machines_view::view`). */
  view: MachinesView | null;
  /** Renames a machine (`machines.rename`). */
  rename(id: string, name: string): Promise<void>;
  /** Removes a machine from the account (`machines.remove`). */
  remove(machine: string | null, device: string | null): Promise<void>;
  /** Asks the machines for their health (`machines.checkHealth`); their
   * issues show in the next view. Never throws: a check that fails shows
   * on that machine. */
  checkHealth(): Promise<void>;
  /** How another machine is set up for access, in the core's words (the
   * "Add a machine" explainer). Setup itself runs on that machine. */
  setupGuide: HostSetupGuide;
}

/** Your machines (this one first), each with the Spaces it runs. */
export function useMachines(): MachinesHook {
  const ctx = useBridgeContext();
  const setupGuide = useMemo(() => hostSetupGuide(ctx.core), [ctx.core]);
  const run = useAction();
  const r = useResource<Machine[]>("machines");
  return {
    ...r,
    setupGuide,
    view: ctx.store?.machinesView ?? null,
    rename: (id, name) => run((s) => s.renameMachine(id, name)),
    remove: (machine, device) => run((s) => s.removeMachine(machine, device)),
    checkHealth: async () => {
      await ctx.store?.checkMachinesHealth().catch(() => undefined);
    },
  };
}

export interface SettingsHook extends Resource<SettingsData> {
  refresh(): Promise<void>;
  updateSetting<K extends SettingKey>(key: K, value: SettingsValues[K]): Promise<void>;
  /** Picks an option on one of the host's own rows (the core's row id). */
  chooseSetting(row: string, option: string): Promise<void>;
  /** Settings, AI agents: reads the coding agents on this machine. */
  loadAgentRows(): Promise<void>;
  /** "Configure all detected agents". */
  configureAllAgents(): Promise<void>;
  /** An agent row's Configure or Remove (`agent:<id>`). */
  pressAgentRow(row: string): Promise<void>;
}

export function useSettings(): SettingsHook {
  const r = useResource<SettingsData>("settings");
  const run = useAction();
  return {
    ...r,
    updateSetting: (key, value) => run((s) => s.updateSetting(key, value)),
    chooseSetting: (row, option) => run((s) => s.chooseSetting(row, option)),
    loadAgentRows: () => run((s) => s.loadAgentRows()),
    configureAllAgents: () => run((s) => s.configureAllAgents()),
    pressAgentRow: (row) => run((s) => s.pressAgentRow(row)),
  };
}

export interface KeyvaultHook extends Resource<KeyvaultData> {
  refresh(): Promise<void>;
  /** Unlocks the vault: Touch ID natively, or this passphrase. */
  unlock(passphrase?: string): Promise<void>;
  /** Creates the vault; answers the recovery key to show once. */
  setUp(passphrase?: string): Promise<string | null>;
  /** Keeps an item out of unattended rules (it asks every time). */
  lockItem(itemId: string): Promise<void>;
  /** `lockItem` for several items at once. */
  lockItems(itemIds: string[]): Promise<void>;
  /** Allows items in unattended rules (the host asks for Touch ID). */
  unlockItems(itemIds: string[]): Promise<void>;
  setDisabled(disabled: boolean): Promise<void>;
  approve(requestId: string, items: string[] | null): Promise<KvGrant>;
  deny(requestId: string): Promise<void>;
  revokeGrant(id: string): Promise<void>;
  /** Shows the items' names (the host asks for Touch ID). */
  showItems(): Promise<void>;
  /** Deletes items and wipes their live copies; the host confirms first
   * (rejects with code `cancelled` when declined). */
  deleteItems(itemIds: string[]): Promise<void>;
  /** An Access row's own command. */
  runCommand(command: KvAccessCommand): Promise<void>;
  /** Hides copies (import ids) from the notch; nothing is revoked or wiped. */
  dismissAccess(imports: string[]): Promise<void>;
  /** Search, select and open groups in the vault list (the core's reducer;
   * `data.views.vault` and `data.vaultState` follow). */
  vaultAction(action: VaultAction): void;
  /** The pane for a sidebar selection (the core's `keyvault.list`). */
  pane(selection: KvSelection): KvListView | null;
}

export function useKeyvault(): KeyvaultHook {
  const { store } = useBridgeContext();
  const r = useResource<KeyvaultData>("keyvault");
  const run = useAction();
  return {
    ...r,
    unlock: (p) => run((s) => s.unlockKeyvault(p)),
    setUp: (p) => run((s) => s.setUpKeyvault(p)),
    lockItem: (id) => run((s) => s.setUnattended([id], false)),
    lockItems: (ids) => run((s) => s.setUnattended(ids, false)),
    unlockItems: (ids) => run((s) => s.setUnattended(ids, true)),
    setDisabled: (d) => run((s) => s.setKeyvaultDisabled(d)),
    approve: (id, items) => run((s) => s.approve(id, items)),
    deny: (id) => run((s) => s.deny(id)),
    revokeGrant: (id) => run((s) => s.revokeGrant(id)),
    showItems: () => run((s) => s.showKeyvaultItems()),
    deleteItems: (ids) => run((s) => s.deleteKeyvaultItems(ids)),
    runCommand: (c) => run((s) => s.runKeyvaultCommand(c)),
    dismissAccess: (imports) => run((s) => s.dismissKeyvaultAccess(imports)),
    vaultAction: (action) => store?.vaultAction(action),
    pane: (selection) => store?.keyvaultPane(selection) ?? null,
  };
}

/** The Spaces a Keyvault sign-in is live in (the core's `keyvault.signedInSpaces`:
 * a live delivered copy is in them), for the "Signed in" badge, and each one's
 * Access row (`accessKey`) to bring forward. Empty without the core or a vault. */
export function useSpaceAccess(spaces: readonly Space[]): { signedIn: ReadonlySet<string>; accessKey(spaceId: string): string | null } {
  const { core } = useBridgeContext();
  const { data } = useResource<KeyvaultData>("keyvault");
  const overview = data?.overview;
  return useMemo(() => {
    const none = { signedIn: new Set<string>(), accessKey: () => null };
    if (!overview || overview.availability !== "ready" || !overview.deliveries.length) return none;
    const now = Date.now();
    const ids = core.tryCall<string[]>("keyvault.signedInSpaces", { overview, now, dismissed: [], spaces }) ?? [];
    return {
      signedIn: new Set(ids),
      accessKey: (spaceId: string) => {
        const space = spaces.find((s) => s.id === spaceId);
        return (space && core.tryCall<string | null>("keyvault.spaceAccessKey", { overview, now, space })) || null;
      },
    };
  }, [core, overview, spaces]);
}

export interface SessionHook extends Resource<SessionData> {
  refresh(): Promise<void>;
  /** Starts the browser sign-in; `session.signIn` tracks it until the host
   * reports signed in (or failed). */
  signIn(opts?: { code?: boolean }): Promise<SignInStart>;
  /** Stops waiting for the browser. */
  cancelSignIn(): void;
  signOut(): Promise<void>;
  /** `launchAtLogin`: Done's checkbox, where the host applies it. */
  completeOnboarding(mode: OnboardingMode, launchAtLogin?: boolean): Promise<void>;
  openExternal(url: string): Promise<void>;
}

export function useSession(): SessionHook {
  const r = useResource<SessionData>("session");
  const run = useAction();
  return {
    ...r,
    signIn: (opts) => run((s) => s.signIn(opts)),
    cancelSignIn: () => void run((s) => s.cancelSignIn()),
    signOut: () => run((s) => s.signOut()),
    completeOnboarding: (mode, launchAtLogin) => run((s) => s.completeOnboarding(mode, launchAtLogin)),
    openExternal: (url) => run((s) => s.openExternal(url)),
  };
}

export interface AgentsHook extends Resource<AgentsData> {
  refresh(): Promise<void>;
  /** Stops the run, saves the agent's home and holds its routines. */
  pauseAgent(name: string): Promise<void>;
  resumeAgent(name: string): Promise<void>;
  /** The coding agents on this machine and what cua set up for each. */
  agentSetup(): Promise<AgentSetupRow[]>;
  /** Adds the cua skills and MCP server to `agents`, or to every installed agent. */
  configureAgents(agents?: string[]): Promise<AgentSetupRow[]>;
}

/** Persistent agents and the runs in your running Spaces. */
export function useAgents(): AgentsHook {
  const r = useResource<AgentsData>("agents");
  const run = useAction();
  return {
    ...r,
    pauseAgent: (name) => run((s) => s.pauseAgent(name)),
    resumeAgent: (name) => run((s) => s.resumeAgent(name)),
    agentSetup: () => run((s) => s.agentSetup()),
    configureAgents: (agents) => run((s) => s.configureAgents(agents ?? null)),
  };
}

const NO_TIMELINE: AgentTimeline = {
  items: [],
  status: null,
  phase: "",
  caughtUp: false,
  isLoading: true,
  error: null,
  unsupported: false,
  lastEventMs: null,
};

/**
 * One run's conversation, read as it is written while the component is
 * mounted (`agents.events` with a cursor, folded like cua-agents'
 * `Transcript`). `null` watches nothing.
 */
export function useAgentTimeline(run: RunRef | null): AgentTimeline {
  const { store } = useBridgeContext();
  const spaceId = run?.spaceId;
  const runId = run?.runId;
  useEffect(() => {
    if (!store || spaceId === undefined || runId === undefined) return;
    return store.watchTimeline({ spaceId, runId });
  }, [store, spaceId, runId]);
  return useSyncExternalStore(
    store ? store.subscribe : noopSubscribe,
    () => (store && spaceId !== undefined && runId !== undefined ? store.timeline({ spaceId, runId }) : undefined) ?? NO_TIMELINE,
    () => NO_TIMELINE,
  );
}

/** The saved first-run progress: `state` (null before it starts) and
 * whether the user skipped the rest for now. */
export function useSavedOnboarding(): SavedOnboarding {
  return useSyncExternalStore(subscribeSavedOnboarding, readSavedOnboarding, readSavedOnboarding);
}

/** How often the permission rows are read again while one is still to grant (ms). */
export const PERMISSION_POLL_MS = 2_000;

/** The panes in `shown` that `hints` no longer lists as missing. */
export function grantedPermissionRows(shown: Record<string, string>, hints: PermissionHint[]): { id: string; title: string }[] {
  const missing = new Set(hints.filter((p) => !p.granted).map((p) => p.id));
  return Object.entries(shown)
    .filter(([id]) => !missing.has(id))
    .map(([id, title]) => ({ id, title }));
}

export interface OnboardingHook {
  /** The core's flow state; null until the session has loaded. */
  state: OnboardingFlowState | null;
  /** The page as the core draws it (the pages this host passes through left out). */
  view: OnboardingView | null;
  copy: OnboardingCopy;
  /** Sign in's line: signed in as, or what to do in the browser. */
  signInText: string | null;
  /** macOS panes still to grant for this machine (explainers only: the
   * grant itself happens in System Settings). */
  permissions: PermissionRow[];
  /** Panes this page showed that are granted now (ticked, not dropped). */
  grantedPermissions: { id: string; title: string }[];
  /** This machine is already set up for access (the host choice is real). */
  hostConfigured: boolean;
  /** One image per OS for the first Space. */
  images: SandboxImage[];
  /** Done's "Launch at login" checkbox, where the host applies it (the
   * Electron shell; the core lays it out on Done). */
  launchAtLogin: OnboardingCheckbox | null;
  /** Sends an action through the core. The usage-data switch and the Menu
   * bar choice also write the machine's setting (telemetry, `menuBar`). */
  send(action: OnboardingAction): void;
  /** A fix on Done's to-do list: opens its page (Cua Volume turns its
   * experiment on first); leaving the page comes back to Done. */
  fixTodo(id: OnboardingTodoId): Promise<void>;
  /** Leaves the flow for now; it resumes on the same page. */
  skip(): void;
  /** Done: records the choice with the host and forgets the saved progress. */
  finish(): Promise<void>;
  /** Starts again from Welcome. */
  restart(): void;
}

/** The first run, page by page, from the app core (see `onboarding.ts`). */
export function useOnboarding(): OnboardingHook {
  const { core, adapter, store, mode } = useBridgeContext();
  const { data: session, completeOnboarding } = useSession();
  const { data: settings, updateSetting } = useSettings();
  const saved = useSavedOnboarding();
  const [hints, setHints] = useState<{ permissions: PermissionHint[]; configured: boolean } | null>(null);

  const identity = session?.signedIn ? (session.fleet.identity ?? null) : null;
  const ctx = { menuBar: settings?.values.menuBar ?? false, observe: store ? store.trackOnboarding.bind(store) : undefined };
  const state = saved.state;

  // Finished here: don't start over while the page is still up.
  const finished = useRef(false);

  // Start (or resume) once the session says who is signed in.
  useEffect(() => {
    if (!state && session && !finished.current) writeSavedOnboarding({ state: onboardingInitial(core, identity), skipped: saved.skipped });
  }, [state, session, core, identity, saved.skipped]);

  // Welcome's switch starts from the machine's setting.
  useEffect(() => {
    if (state && !state.telemetry && settings?.telemetry) {
      const next = onboardingReduce(core, state, { type: "telemetry-loaded", telemetry: onboardingTelemetry(settings.telemetry) }, ctx);
      writeSavedOnboarding({ ...readSavedOnboarding(), state: next });
    }
  });

  // Follow the account: a sign-in anywhere shows here, a sign-out clears it.
  useEffect(() => {
    if (!state || !session) return;
    if (identity && state.identity !== identity) {
      writeSavedOnboarding({ ...readSavedOnboarding(), state: onboardingReduce(core, state, { type: "signed-in", identity }, ctx) });
    } else if (!session.signedIn && state.identity) {
      // The core has no signed-out action: its identity is only what Done
      // and Sign in show, so drop it.
      writeSavedOnboarding({ ...readSavedOnboarding(), state: { ...state, identity: null } });
    }
  });

  // This machine's permissions (hosts without `host.status` show none). Read
  // again every few seconds, and when the window is back in front, while any is
  // still to grant: macOS's Settings is another window, and a grant there
  // must tick here without a restart (CUA-1765).
  const pending = hints ? hints.permissions.some((p) => !p.granted) : false;
  const readHints = useRef<() => void>(() => undefined);
  useEffect(() => {
    if (!adapter) return;
    let live = true;
    readHints.current = () => {
      adapter
        .call("host.status", {})
        .then((h) => live && setHints({ permissions: h.permissions ?? [], configured: h.configured }))
        .catch(() => live && setHints((cur) => cur ?? { permissions: [], configured: false }));
    };
    readHints.current();
    return () => {
      live = false;
    };
  }, [adapter]);
  useEffect(() => {
    if (!pending) return;
    const again = () => readHints.current();
    const timer = setInterval(again, PERMISSION_POLL_MS);
    window.addEventListener("focus", again);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", again);
    };
  }, [pending]);
  // The panes this page has shown, so one that is granted now shows ticked instead of vanishing.
  const [shown, setShown] = useState<Record<string, string>>({});
  useEffect(() => {
    if (!hints) return;
    const missing = hints.permissions.filter((p) => !p.granted);
    if (missing.some((p) => !(p.id in shown))) setShown((cur) => ({ ...cur, ...Object.fromEntries(missing.map((p) => [p.id, p.label])) }));
  }, [hints, shown]);

  // Done counts the coding agents set up before (from the CLI or
  // Settings), not only the ones this run set up.
  const onDone = state?.step === "done";
  useEffect(() => {
    if (!onDone || !adapter) return;
    let live = true;
    adapter.call("agents.setup", {}).then(
      (rows) => {
        const names = rows.filter((r) => r.installed && r.configured).map((r) => r.name);
        const cur = readSavedOnboarding().state;
        const known = Array.isArray(cur?.agents) ? (cur.agents as string[]) : [];
        if (!live || !cur || names.every((n) => known.includes(n))) return;
        writeSavedOnboarding({ ...readSavedOnboarding(), state: onboardingReduce(core, cur, { type: "agents-detected", configured: names }, ctx) });
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onDone, adapter, core]);

  const drawn = state ? onboardingView(core, state) : null;
  // No switch until the host has said what the setting is: an "on" it
  // could not write would not be true.
  const view = drawn && !settings?.telemetry ? { ...drawn, usage: null } : drawn;
  const signIn = session?.signIn;
  const signInText = !state
    ? null
    : state.identity
      ? signedInText(core, state.identity)
      : signIn?.kind === "waiting"
        ? signInCodeText(core, signIn.userCode)
        : signIn?.kind === "failed"
          ? signIn.message
          : null;

  const send = (action: OnboardingAction) => {
    const current = readSavedOnboarding().state;
    if (!current) return;
    // Skipping Sign in leaves the browser login behind: stop waiting for it.
    if (action.type === "signin-done") store?.cancelSignIn();
    const next = onboardingReduce(core, current, action, ctx);
    // The switch writes the setting at once, so what it shows is what the
    // machine does even if the flow is skipped from Welcome. (The SwiftUI
    // app writes it on leaving Welcome; the web can be left from any page.)
    if (action.type === "usage-data-toggled" && next.telemetry?.enabled === action.on && settings?.telemetry) {
      void updateSetting("telemetry", action.on);
    }
    // The Menu bar choice is the "Spaces tab in the notch" setting, written
    // at once as the SwiftUI page does.
    if (action.type === "presentation-picked" && next.menuBar === action.menuBar && settings && settings.values.menuBar !== action.menuBar) {
      void updateSetting("menuBar", action.menuBar);
    }
    writeSavedOnboarding({ ...readSavedOnboarding(), state: next });
  };

  return {
    state,
    view,
    copy: onboardingCopy(core),
    signInText,
    permissions: hints ? permissionRows(core, hints.permissions) : [],
    grantedPermissions: hints ? grantedPermissionRows(shown, hints.permissions) : [],
    hostConfigured: hints?.configured ?? false,
    images: firstSpaceImages(core),
    launchAtLogin: mode === "electron" ? (view?.launchAtLogin ?? null) : null,
    send,
    fixTodo: async (id) => {
      const current = readSavedOnboarding().state;
      const experiments = (current?.experiments ?? null) as Experiments | null;
      // Cua Volume is an experiment today: "Turn on" turns it on first.
      if (id === "volume" && adapter && !experiments?.cuaVolume) {
        const now = { ...NO_EXPERIMENTS, ...(await adapter.call("experiments.get", {}).catch(() => ({}))), cuaVolume: true };
        await adapter.call("experiments.set", { experiments: now });
        send({ type: "experiments-loaded", experiments: now });
      }
      send({ type: "todo-chosen", id });
    },
    skip: () => {
      const current = readSavedOnboarding().state;
      if (current) store?.trackOnboardingSkipped(current);
      store?.cancelSignIn();
      writeSavedOnboarding({ ...readSavedOnboarding(), skipped: true });
    },
    finish: async () => {
      finished.current = true;
      const last = readSavedOnboarding().state;
      if (last) store?.trackOnboardingFinished(last);
      const done = readSavedOnboarding().state;
      // Done's checkbox (ticked unless unticked), where this host draws it.
      const login = mode === "electron" && view?.launchAtLogin ? done?.launchAtLogin !== false : undefined;
      await completeOnboarding(done?.mode ?? "client", login);
      writeSavedOnboarding(null);
    },
    restart: () => {
      finished.current = false;
      writeSavedOnboarding({ state: onboardingInitial(core, identity), skipped: false });
    },
  };
}
