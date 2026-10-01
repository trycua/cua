// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  configureAgents,
  detectAgents,
  removeAgents,
  type AgentRow,
} from "../native/agentConfig";
import {
  reduceStorage,
  storageChoose,
  storageEdit,
  storageInitial,
  storagePress,
  storageSection,
  type DriveCacheInput,
  type DriveCheckInput,
  type DriveStorageInput,
  type StorageAction,
  type StorageInput,
  type StorageRequest,
  type StorageState,
} from "../model/driveSettings";
import { hostOs } from "../model/host";
import type { DriveMountInput } from "../model/persistent";
import type { SpaceOs } from "../model/types";
import { answerOrNull, createDriveBridge, type DriveBridge } from "../native/drive";
import type { DefaultLocation, SignInStart } from "../native/fleet";
import { storageSignals } from "../model/telemetry";
import { telemetryBridge, type TelemetryBridge, type TelemetryView } from "../native/telemetry";
import { createLoginItemBridge, type LoginItemBridge } from "../native/loginItem";
import { writeLaunchChoice, type LoginItemStatus } from "../model/loginItem";
import type { Location } from "../model/types";
import { settingsPage, type SettingsRow, type SettingsSection, type SignInPhase } from "../model/window";
import { experimentsPage, settingsWithStorage } from "../model/experiments";
import { chooseExperimentRow, useExperiments } from "../state/experiments";
import { MiddleText } from "./MiddleText";
import { SfIcon } from "./SfIcon";

/** The clipboard (the webview's). */
const writeClipboard = (text: string): Promise<void> => navigator.clipboard.writeText(text);

/** Cua's landing page; opened in the user's default browser as a fallback when
 * the native device-flow sign-in is unavailable (browser dev / tests). */
export const SIGN_IN_URL = "https://www.cua.ai";
/** Cua's sign-up page for users without an account. */
export const SIGN_UP_URL = "https://www.cua.ai/signup";

/**
 * The subset of the shell bridge the Account section drives for a real user
 * sign-in (OAuth device grant). Absent off-Tauri, where the section falls back
 * to opening the Cua site.
 */
export interface AccountAuth {
  beginSignIn: () => Promise<SignInStart>;
  signOut: () => Promise<void> | void;
  onSignedIn: (handler: (identity?: string) => void) => Promise<() => void> | (() => void);
  onSignInFailed: (handler: (reason: string) => void) => Promise<() => void> | (() => void);
  onSignedOut: (handler: () => void) => Promise<() => void> | (() => void);
}

/** The small gear glyph used by the switcher footer to open Settings. */
export function GearGlyph({ className }: { className?: string }) {
  return <SfIcon name="gearshape.fill" className={className} fallback={<GearSvg className={className} />} />;
}

function GearSvg({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      <circle cx="12" cy="12" r="3.1" fill="none" stroke="currentColor" strokeWidth="1.6" />
      <path
        d="M12 2.6l1.5 2.2 2.6-.6.5 2.6 2.5 1-.7 2.6 1.8 1.9-1.8 1.9.7 2.6-2.5 1-.5 2.6-2.6-.6L12 21.4l-1.5-2.2-2.6.6-.5-2.6-2.5-1 .7-2.6L3.8 12l1.8-1.9-.7-2.6 2.5-1 .5-2.6 2.6.6z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinejoin="round"
      />
    </svg>
  );
}

interface SettingsPanelProps {
  /** Whether the menu-bar status item is the switcher entry point (vs the
   * notch tab). */
  menuBar: boolean;
  /** Switch the switcher entry point between the notch tab and a macOS
   * menu-bar status item. */
  onMenuBarMode: (enabled: boolean) => void;
  /** True when Cua Cloud credentials work (client credentials count). */
  fleetLive: boolean;
  /** OAuth client id, shown muted for client credentials; absent when unknown. */
  clientId?: string;
  /** Signed-in user identity (email/subject) from a device sign-in, if any. */
  signedInIdentity?: string;
  /** Native device-flow sign-in hooks; when absent the button opens the site. */
  auth?: AccountAuth;
  /** Open a URL in the user's default browser (native bridge / no-op). */
  onOpenExternal: (url: string) => void;
  /** Where new Spaces go by default, and where that came from. */
  defaultLocation?: DefaultLocation;
  /** Store a new default location (`$CUA_HOME/config.toml`). */
  onDefaultLocation?: (on: Location) => Promise<unknown>;
  /** "Show again": first run from the start. */
  onShowWelcome?: () => void;
  /** Cua Cloud billing (the Billing row): the account's status. "Manage
   * billing" opens the website's billing page with `onOpenExternal`. */
  billingStatus?: () => Promise<import("../native/fleet").BillingStatus | null>;
  /** Usage telemetry (the shell's; a fallback outside it). */
  telemetry?: TelemetryBridge;
  /** The Cua Volume's storage, mount and cache (the daemon's `drive_*` tools). */
  drive?: DriveBridge;
  /** This machine's system (the platform the app runs on). */
  os?: SpaceOs;
  /** Launch at login (the shell's login item; a fake in tests). */
  loginItem?: LoginItemBridge;
  /** What this machine runs that a restart stops (the note when it is off). */
  serves?: { providesSpaces: boolean; runsAgents: boolean };
  /** Dismiss the panel (Esc / gear toggle). */
  onClose: () => void;
}

/**
 * Settings, shown as a page of the main window (never in the notch panel),
 * with its own scroll. The sections, rows, words and states are the app
 * core's `settings::page` (Account, General, Privacy, AI agents; one line
 * per row), the same page the SwiftUI app draws; this runs what the buttons
 * and choices ask for. Keyboard events are kept local so the panel's inputs
 * never leak navigation keys to the portal's global handler.
 */
export function SettingsPanel({
  menuBar,
  onMenuBarMode,
  fleetLive,
  clientId,
  signedInIdentity,
  auth,
  onOpenExternal,
  defaultLocation,
  onDefaultLocation,
  onShowWelcome,
  billingStatus: readBilling,
  telemetry: telemetryProp,
  drive: driveProp,
  os,
  loginItem: loginItemProp,
  serves,
  onClose,
}: SettingsPanelProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const telemetry = useMemo(() => telemetryProp ?? telemetryBridge(), [telemetryProp]);
  const experiments = useExperiments();
  const drive = useMemo(() => driveProp ?? createDriveBridge(), [driveProp]);
  const storage = useStorage(drive, os ?? hostOs());

  const [locationError, setLocationError] = useState<string | null>(null);
  const [locationBusy, setLocationBusy] = useState(false);
  const chooseLocation = useCallback(
    (on: Location) => {
      if (!onDefaultLocation || locationBusy || defaultLocation?.value === on) return;
      setLocationBusy(true);
      setLocationError(null);
      void Promise.resolve(onDefaultLocation(on))
        .catch((error: unknown) => setLocationError(error instanceof Error ? error.message : String(error)))
        .finally(() => setLocationBusy(false));
    },
    [onDefaultLocation, locationBusy, defaultLocation?.value],
  );

  // Privacy: the usage-telemetry switch (the same setting as `cua telemetry
  // off`). Opening Settings also counts as seeing the first-run notice.
  const [telemetryView, setTelemetryView] = useState<TelemetryView | null>(null);
  const [telemetryError, setTelemetryError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void telemetry
      .acknowledgeNotice()
      .then((v) => {
        if (!cancelled) setTelemetryView(v);
      })
      .catch((e: unknown) => {
        if (!cancelled) setTelemetryError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [telemetry]);
  const setTelemetry = useCallback(
    (on: boolean) => {
      setTelemetryError(null);
      telemetry
        .setEnabled(on)
        .then(setTelemetryView)
        .catch((e: unknown) => setTelemetryError(e instanceof Error ? e.message : String(e)));
    },
    [telemetry],
  );

  // General: launch at login. The toggle shows what the system holds, read
  // again after every change; the change saves the user's choice.
  const loginItem = useMemo(() => loginItemProp ?? createLoginItemBridge(), [loginItemProp]);
  const [loginStatus, setLoginStatus] = useState<LoginItemStatus | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void loginItem
      .status()
      .then((s) => !cancelled && setLoginStatus(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [loginItem]);
  const setLaunchAtLogin = useCallback(
    (on: boolean) => {
      if (loginBusy) return;
      setLoginBusy(true);
      setLoginError(null);
      writeLaunchChoice(on);
      loginItem
        .set(on)
        .then(setLoginStatus)
        .catch((e: unknown) => {
          setLoginError(e instanceof Error ? e.message : String(e));
          void loginItem.status().then(setLoginStatus).catch(() => {});
        })
        .finally(() => setLoginBusy(false));
    },
    [loginItem, loginBusy],
  );

  // AI agents: the SDK's agent onboarding (cua skills + the cua MCP server in
  // each installed coding agent). Detection runs on mount (read-only).
  const [agentRows, setAgentRows] = useState<AgentRow[] | null>(null);
  const [agentsBusy, setAgentsBusy] = useState(false);
  const [agentPending, setAgentPending] = useState<string[]>([]);
  const [agentsError, setAgentsError] = useState<string | null>(null);

  const mergeRows = useCallback((rows: AgentRow[]) => {
    setAgentRows((prev) => {
      const next = [...(prev ?? [])];
      for (const r of rows) {
        const i = next.findIndex((x) => x.agent === r.agent);
        if (i >= 0) next[i] = r;
        else next.push(r);
      }
      return next;
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    void detectAgents()
      .then((rows) => {
        if (!cancelled) setAgentRows(rows);
      })
      .catch((error) => {
        if (!cancelled) setAgentsError(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const agentAction = useCallback(
    async (agent: string, action: (agents: string[]) => Promise<AgentRow[]>) => {
      setAgentPending((p) => [...p, agent]);
      setAgentsError(null);
      try {
        mergeRows(await action([agent]));
      } catch (error) {
        setAgentsError(String(error));
      } finally {
        setAgentPending((p) => p.filter((a) => a !== agent));
      }
    },
    [mergeRows],
  );

  const configureAll = useCallback(async () => {
    setAgentsBusy(true);
    setAgentsError(null);
    try {
      mergeRows(await configureAgents());
    } catch (error) {
      setAgentsError(String(error));
    } finally {
      setAgentsBusy(false);
    }
  }, [mergeRows]);

  // Device-flow sign-in state. `identity` is seeded from the prop (a session
  // restored on startup) and then driven by the shell's auth events.
  const [phase, setPhase] = useState<SignInPhase>({ kind: "idle" });
  const [identity, setIdentity] = useState<string | undefined>(signedInIdentity);
  useEffect(() => {
    setIdentity(signedInIdentity);
  }, [signedInIdentity]);

  useEffect(() => {
    if (!auth) return;
    const unsubs: Array<() => void> = [];
    const track = (pending: Promise<() => void> | (() => void)) => {
      void Promise.resolve(pending)
        .then((unsub) => {
          if (unsub) unsubs.push(unsub);
        })
        .catch(() => {});
    };
    track(
      auth.onSignedIn((id) => {
        setIdentity(id ?? "Signed in to Cua");
        setPhase({ kind: "idle" });
      }),
    );
    track(auth.onSignInFailed((message) => setPhase({ kind: "failed", message })));
    track(
      auth.onSignedOut(() => {
        setIdentity(undefined);
        setPhase({ kind: "idle" });
      }),
    );
    return () => {
      for (const unsub of unsubs) unsub();
    };
  }, [auth]);

  const beginSignIn = useCallback(async () => {
    // No native device flow (browser dev / tests): fall back to the Cua site.
    if (!auth) {
      onOpenExternal(SIGN_IN_URL);
      return;
    }
    setPhase({ kind: "starting" });
    try {
      const { userCode, verificationUri } = await auth.beginSignIn();
      void verificationUri;
      setPhase({ kind: "waiting", userCode: userCode ?? null });
    } catch (error) {
      setPhase({
        kind: "failed",
        message: error instanceof Error ? error.message : "Could not start sign-in",
      });
    }
  }, [auth, onOpenExternal]);

  // Billing (signed in): the credit left; "Manage billing" is the website.
  const [billingStatus, setBillingStatus] = useState<import("../native/fleet").BillingStatus | null>(null);
  useEffect(() => {
    if (!identity || !readBilling) {
      setBillingStatus(null);
      return;
    }
    let cancelled = false;
    void readBilling()
      .then((s) => !cancelled && setBillingStatus(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [readBilling, identity]);

  const signOut = useCallback(() => {
    setIdentity(undefined);
    setPhase({ kind: "idle" });
    void Promise.resolve(auth?.signOut()).catch(() => {});
  }, [auth]);

  // Move focus into the panel on open so Escape (and the panel's key trapping)
  // is caught here rather than reaching the portal's global handler underneath.
  useEffect(() => {
    panelRef.current?.focus({ preventScroll: true });
  }, []);

  // The Account section reflects the USER session (a device sign-in). Env
  // client-credentials keep Cua Cloud working, but they're a machine key, not
  // a sign-in: they show as "Signed in via API key" beside "Sign in to Cua".
  const page = settingsPage({
    identity: identity ?? null,
    apiKeyClient: !identity && fleetLive ? (clientId ?? null) : null,
    signIn: phase,
    canSignOut: Boolean(auth) && Boolean(identity),
    menuBar,
    defaultLocation: defaultLocation?.value === "cloud" ? "cloud" : "local",
    locationLockedBy: defaultLocation?.source === "env" ? (defaultLocation.env ?? "CUA_DEFAULT_ON") : null,
    telemetry: telemetryView
      ? {
          enabled: telemetryView.enabled,
          lockedBy:
            telemetryView.sourceKind !== "config" && telemetryView.sourceKind !== "default" ? telemetryView.source : null,
        }
      : null,
    agents: agentRows,
    agentsBusy,
    agentsPending: agentPending,
    billing: billingStatus,
    loginItem: loginStatus
      ? {
          status: loginStatus,
          busy: loginBusy,
          error: loginError,
          providesSpaces: serves?.providesSpaces ?? false,
          runsAgents: serves?.runsAgents ?? false,
        }
      : null,
    experiments,
  });
  const rowById = new Map((agentRows ?? []).map((r) => [`agent:${r.agent}`, r]));

  const press = (row: SettingsRow) => {
    switch (row.id) {
      case "account":
        signOut();
        return;
      case "sign-in-code":
        setPhase({ kind: "idle" });
        return;
      case "sign-in":
        void beginSignIn();
        return;
      case "welcome":
        onShowWelcome?.();
        return;
      case "billing":
      case "teams":
        if (row.linkUrl) onOpenExternal(row.linkUrl);
        return;
    }
    const agent = rowById.get(row.id);
    if (agent) void agentAction(agent.agent, agent.configured ? removeAgents : configureAgents);
  };

  const choose = (row: SettingsRow, id: string) => {
    if (row.id.startsWith("experiment:")) chooseExperimentRow(row.id, id, telemetry);
    else if (row.id === "notch") onMenuBarMode(id === "hide");
    else if (row.id === "default-location") chooseLocation(id as Location);
    else if (row.id === "telemetry") setTelemetry(id === "on");
    else if (row.id === "launch-at-login") setLaunchAtLogin(id === "on");
  };

  // Storage goes after General while the Cua Volume experiment is on (the
  // core's rule; its rows run the core's storage actions), and
  // Experiments last (the SwiftUI app shows it as its own tab).
  const sections: SettingsSection[] = [
    ...settingsWithStorage(page, storage.section, experiments).sections,
    ...experimentsPage(experiments).sections,
  ];

  const sectionError = (id: string) =>
    id === "general" ? locationError : id === "privacy" ? telemetryError : id === "agents" ? agentsError : null;

  return (
    <div
      className="settings-panel dw-settings-panel"
      role="dialog"
      aria-label={page.title}
      tabIndex={-1}
      ref={panelRef}
      // A click on the panel's own chrome must not fall through to the portal's
      // click-off collapse handler beneath the overlay.
      onMouseDown={(event) => event.stopPropagation()}
      // Keep every key local: Escape closes the panel, and no key leaks to the
      // portal's global navigation/shortcut handler beneath the overlay.
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          onClose();
          return;
        }
        event.stopPropagation();
      }}
    >
      {sections.map((section) => {
        const error = sectionError(section.id);
        const isStorage = section.id === "storage";
        return (
          <section className="st-group" aria-label={section.title} key={section.id}>
            <div className="st-head">
              <h3 className="st-title">{section.title}</h3>
              {section.button && (
                <button
                  type="button"
                  className="dw-btn dw-btn-quiet"
                  data-owns-enter
                  disabled={!section.buttonEnabled}
                  title={section.buttonHelp ?? undefined}
                  onClick={() => (isStorage ? storage.send({ type: "save" }) : void configureAll())}
                >
                  {section.button}
                </button>
              )}
            </div>
            <div className="st-rows">
              {section.rows.map((row) => (
                <SettingsRowItem
                  key={row.id}
                  row={row}
                  onPress={isStorage ? storage.press : press}
                  onChoose={isStorage ? storage.choose : choose}
                  onEdit={storage.edit}
                  onOpenExternal={onOpenExternal}
                />
              ))}
            </div>
            {error && (
              <p className="st-error" role="alert">
                {error}
              </p>
            )}
          </section>
        );
      })}
    </div>
  );
}

/** One Settings row (also the first run's Volume page bucket rows). */
export function SettingsRowItem({
  row,
  onPress,
  onChoose,
  onEdit,
  onOpenExternal,
}: {
  row: SettingsRow;
  onPress: (row: SettingsRow) => void;
  onChoose: (row: SettingsRow, id: string) => void;
  onEdit: (row: SettingsRow, value: string) => void;
  onOpenExternal: (url: string) => void;
}) {
  switch (row.kind) {
    case "field":
    case "secret":
      return (
        <div className="st-row" data-dim={!row.enabled ? "true" : undefined} title={row.help ?? undefined}>
          <span className="st-label">{row.label}</span>
          <input
            className="dw-input st-field"
            type={row.kind === "secret" ? "password" : "text"}
            aria-label={row.label}
            placeholder={row.placeholder ?? undefined}
            value={row.value ?? ""}
            disabled={!row.enabled}
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            data-owns-enter
            onChange={(event) => onEdit(row, event.target.value)}
          />
        </div>
      );
    case "error":
      return (
        <p className="st-error" role="alert">
          {row.label}
        </p>
      );
    case "note":
      return (
        <p className="st-note">
          {row.label}{" "}
          {row.linkUrl && (
            <button type="button" className="dw-link" data-owns-enter onClick={() => onOpenExternal(row.linkUrl!)}>
              {row.linkLabel}
            </button>
          )}
        </p>
      );
    case "toggle": {
      const on = row.options.find((o) => o.id === "on")?.active ?? false;
      return (
        <div className="st-row" title={row.help ?? undefined}>
          <span className="st-label">{row.label}</span>
          <button
            type="button"
            role="switch"
            className="kv-switch"
            data-owns-enter
            aria-checked={on}
            aria-label={row.label}
            disabled={!row.enabled}
            onClick={() => onChoose(row, on ? "off" : "on")}
          />
        </div>
      );
    }
    case "choice":
      return (
        <div className="st-row" title={row.help ?? undefined}>
          <span className="st-label">{row.label}</span>
          <div className="st-segmented" role="radiogroup" aria-label={row.label}>
            {row.options.map((o) => (
              <button
                key={o.id}
                type="button"
                role="radio"
                data-owns-enter
                aria-checked={o.active}
                data-active={o.active}
                disabled={!row.enabled}
                onClick={() => onChoose(row, o.id)}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>
      );
    case "prompt":
      return (
        <div className="st-prompt">
          <div className="st-row">
            <span className="st-label">{row.label}</span>
            {row.button && (
              <button
                type="button"
                className="dw-btn"
                data-owns-enter
                onClick={() => void writeClipboard(row.value ?? "").catch(() => {})}
              >
                {row.button}
              </button>
            )}
          </div>
          <textarea className="st-prompt-text" aria-label={row.label} readOnly rows={5} value={row.value ?? ""} />
        </div>
      );
    case "link":
      return (
        <div className="st-row st-link-row">
          <button type="button" className="dw-link st-link" data-owns-enter disabled={!row.enabled} onClick={() => onPress(row)}>
            {row.label}
          </button>
        </div>
      );
    case "text": {
      // A path (the help is its full form) shortens in the middle.
      const path = Boolean(row.help && row.value && (row.id === "fs-path" || row.id === "mount-path"));
      return (
        <div
          className="st-row"
          data-dim={!row.button && !row.enabled ? "true" : undefined}
          title={path ? undefined : (row.help ?? undefined)}
        >
          <span className="st-label" role={row.id === "sign-in-code" ? "status" : undefined}>
            {row.label}
          </span>
          {row.value && path && <MiddleText className="st-value" text={row.value} title={row.help} />}
          {row.value && !path && (
            <span className={`st-value${row.id === "sign-in-code" ? " st-code" : ""}`} aria-live={row.id === "sign-in-code" ? "polite" : undefined}>
              {row.value}
            </span>
          )}
          {row.button && (
            <button type="button" className="dw-btn" data-owns-enter disabled={!row.enabled} onClick={() => onPress(row)}>
              {row.button}
            </button>
          )}
        </div>
      );
    }
  }
}

/** How often Storage asks again while macOS waits for the extension. */
export const STORAGE_APPROVAL_POLL_MS = 2000;
/** How often Storage looks for the bucket the user's agent sets up. */
export const STORAGE_PROMPT_POLL_MS = 2000;

/** A `volume_storage_set` that threw, as its answer. */
export function failedCheck(detail: string): DriveCheckInput {
  return { ok: false, reachable: false, authorized: false, versioning: false, detail, applied: false };
}

/**
 * Settings' Storage section: reads `volume_storage`, `volume_mount_status`
 * and `volume_cache_stats` (each null when the daemon cannot answer: the
 * core then shows one honest disabled line) and runs what the core asks
 * for. The S3 keys typed here stay in this state and only ever travel in
 * `volume_storage_set`'s arguments.
 */
function useStorage(drive: DriveBridge, os: SpaceOs) {
  const [input, setInput] = useState<StorageInput>({ os, home: null, storage: null, mount: null, cache: null });
  const [state, setState] = useState<StorageState>(storageInitial);
  const stateRef = useRef(state);
  stateRef.current = state;
  const inputRef = useRef(input);
  inputRef.current = input;

  const sendRef = useRef<(action: StorageAction | null) => void>(() => {});
  const load = useCallback(async () => {
    const [storage, mount, cache] = await Promise.all([
      answerOrNull<DriveStorageInput>(drive.call, "volume_storage"),
      answerOrNull<DriveMountInput>(drive.call, "volume_mount_status"),
      answerOrNull<DriveCacheInput>(drive.call, "volume_cache_stats"),
    ]);
    setInput((i) => ({ ...i, storage, mount, cache }));
    // Through `send`: a bucket the user's agent set up is adopted from here.
    if (storage) sendRef.current({ type: "loaded", storage });
  }, [drive]);

  useEffect(() => {
    let cancelled = false;
    void drive
      .home()
      .then((home) => !cancelled && setInput((i) => ({ ...i, home })))
      .catch(() => {});
    void load();
    return () => {
      cancelled = true;
    };
  }, [drive, load]);

  useEffect(() => setInput((i) => (i.os === os ? i : { ...i, os })), [os]);

  // Mounting, or macOS waiting for the extension's approval: ask again
  // every 2 s so the row follows once the user allows it.
  const waiting = input.mount?.state === "needs_approval" || input.mount?.state === "mounting";
  useEffect(() => {
    if (!waiting) return;
    const id = setInterval(() => {
      void answerOrNull<DriveMountInput>(drive.call, "volume_mount_status").then((mount) =>
        setInput((i) => ({ ...i, mount })),
      );
    }, STORAGE_APPROVAL_POLL_MS);
    return () => clearInterval(id);
  }, [waiting, drive]);

  const run = useCallback(
    async (request: StorageRequest) => {
      // Through the ref, so a reload right after sees this answer.
      const settle = (action: StorageAction) => {
        // A storage choice saved, the Finder volume on or off, Open in
        // Finder: the usage events the app core derives.
        telemetryBridge().recordSignals(storageSignals(inputRef.current, stateRef.current, action));
        const n = reduceStorage(stateRef.current, action);
        stateRef.current = n;
        setState(n);
      };
      try {
        switch (request.kind) {
          case "test":
            settle({ type: "checked", check: (await drive.call("volume_storage_set", { ...request.update })) as DriveCheckInput });
            break;
          case "save":
            settle({ type: "saved", check: (await drive.call("volume_storage_set", { ...request.update })) as DriveCheckInput });
            break;
          case "mount":
            await drive.call("volume_mount");
            settle({ type: "done" });
            break;
          case "unmount":
            await drive.call("volume_unmount");
            settle({ type: "done" });
            break;
          case "reveal":
            await drive.reveal(request.path);
            settle({ type: "done" });
            break;
          case "open-url":
            await drive.openSettings(request.url);
            settle({ type: "done" });
            break;
          case "set-cache":
            await drive.call("volume_cache_set", { capacity_bytes: request.capacity_bytes });
            settle({ type: "done" });
            break;
          case "clear-cache":
            await drive.call("volume_cache_clear");
            settle({ type: "done" });
            break;
          case "adopt":
            // The bucket the user's agent set up, with its saved keys.
            try {
              settle({ type: "adopted", check: (await drive.call("volume_storage_set", { ...request.update })) as DriveCheckInput });
            } catch (e) {
              settle({ type: "adopted", check: failedCheck(e instanceof Error ? e.message : String(e)) });
            }
            break;
        }
      } catch (e) {
        settle({ type: "failed", error: e instanceof Error ? e.message : String(e) });
      }
      await load();
    },
    [drive, load],
  );

  const send = useCallback(
    (action: StorageAction | null) => {
      if (!action) return;
      const before = stateRef.current;
      const next = reduceStorage(before, action);
      stateRef.current = next;
      setState(next);
      if (!before.busy && next.busy && next.request) void run(next.request);
    },
    [run],
  );
  sendRef.current = send;

  // While the section shows the agent prompt, look for the bucket the
  // user's agent sets up every 2 s.
  const section = storageSection(input, state);
  const prompting = section.rows.some((r) => r.id === "s3-prompt");
  useEffect(() => {
    if (!prompting) return;
    const id = setInterval(() => {
      void answerOrNull<DriveStorageInput>(drive.call, "volume_storage").then((storage) => {
        if (!storage) return;
        setInput((i) => ({ ...i, storage }));
        sendRef.current({ type: "loaded", storage });
      });
    }, STORAGE_PROMPT_POLL_MS);
    return () => clearInterval(id);
  }, [prompting, drive]);

  return {
    section,
    send,
    press: (row: SettingsRow) => send(storagePress(inputRef.current, row.id)),
    choose: (row: SettingsRow, option: string) => send(storageChoose(row.id, option)),
    edit: (row: SettingsRow, value: string) => send(storageEdit(row.id, value)),
  };
}
