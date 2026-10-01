// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { type ReactElement, type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { core } from "../core";
import { hostOs, permissionRows } from "../model/host";
import {
  initialOnboarding,
  onboardingCopy,
  onboardingView,
  reduceOnboarding,
  signedInText,
  signInCodeText,
  type OnboardingAction,
  type OnboardingState,
  type OnboardingStep,
  type OnboardingView,
  type StorageChoice,
} from "../model/onboarding";
import {
  storageChoose,
  storageEdit,
  storagePress,
  type DriveCheckInput,
  type DriveStorageInput,
  type StorageAction,
} from "../model/driveSettings";
import type { DriveMountInput } from "../model/persistent";
import type { SettingsRow } from "../model/window";
import type { SpaceOs } from "../model/types";
import { answerOrNull, createDriveBridge, type DriveBridge } from "../native/drive";
import type { SignInStart } from "../native/fleet";
import type { HostBridge, HostStatus, OnboardingMode } from "../native/host";
import type { AgentDetectReport, AgentSetupReport, InstallerBridge } from "../native/installer";
import { onboardingFinishedSignals, onboardingSignals } from "../model/telemetry";
import { telemetryBridge, type TelemetryBridge } from "../native/telemetry";
import { readMenuBar, writeMenuBar } from "../state/settings";
import { currentExperiments } from "../state/experiments";
import { writeLaunchChoice } from "../model/loginItem";
import { createLoginItemBridge, type LoginItemBridge } from "../native/loginItem";
import { Page } from "./desktop/OnboardingPage";
import { DriverPreview } from "./DriverPreview";
import { DriveMountPreview } from "./DriveMountPreview";
import { PresentationPreview } from "./PresentationPreview";
import { Onboarding } from "./Onboarding";
import { SpacesStackMark } from "./SpacesStackMark";
import { MiddleText } from "./MiddleText";
import { failedCheck, SettingsRowItem } from "./SettingsPanel";
import { PermissionRows } from "./ThisMachinePanel";

/** Sign-in hooks (the app's device-authorization sign-in). */
export interface InstallerAuth {
  beginSignIn: () => Promise<SignInStart>;
  onSignedIn: (handler: (identity?: string) => void) => Promise<() => void>;
  onSignInFailed: (handler: (reason: string) => void) => Promise<() => void>;
}

export type InstallerStep = OnboardingStep;

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** What an agent setup did for one agent (the app core's summary). */
export function summarizeAgent(
  report: AgentSetupReport,
  agentId: string,
  name = agentId,
): { text: string; failed: string[]; line: string } {
  return core("agents.setupSummary", { outcomes: report.outcomes, agent: agentId, name });
}

/** How often the Cua Volume page asks again while macOS waits for approval. */
export const DRIVE_APPROVAL_POLL_MS = 2000;
/** How often the page looks for the bucket the user's agent sets up. */
export const DRIVE_PROMPT_POLL_MS = 2000;

/**
 * The graphical installer (first run of the app): welcome, sign in, agent
 * onboarding (the same cua-agent-setup that `cua auth login` runs), where
 * Cua Spaces shows up (notch and menu bar, or menu bar only), the Cua Volume
 * in Finder (off unless ticked; skipped where it cannot mount), then the
 * "access other machines" / "unattended access" choice, then done. The
 * bundled `cua` CLI is installed silently on first launch. Every write asks first and shows its exact path.
 * The pages, their words and the answers are the app core's
 * (`onboarding::*`).
 */
export function InstallerFlow({
  installer,
  host,
  auth,
  identity: initialIdentity,
  installerMode,
  initialStep = "welcome",
  onOpenExternal,
  drive: driveProp,
  os: osProp,
  loginItem: loginItemProp,
  telemetry: telemetryProp,
  onDone,
}: {
  installer: InstallerBridge;
  host: HostBridge;
  /** The daemon's Cua Volume tools (`volume_mount_status`, `volume_mount`, `volume_unmount`). */
  drive?: DriveBridge;
  /** This machine's system (the platform the app runs on). */
  os?: SpaceOs;
  /** Launch at login (Done's checkbox; a fake in tests). */
  loginItem?: LoginItemBridge;
  auth?: InstallerAuth;
  identity?: string;
  installerMode?: OnboardingMode | null;
  initialStep?: InstallerStep;
  /** Opens a website page in the browser (the Teams waitlist). */
  onOpenExternal?: (url: string) => void;
  /** Usage telemetry (the shell's; nothing outside it). */
  telemetry?: TelemetryBridge;
  onDone: (mode: OnboardingMode) => void;
}) {
  // Settings, Experiments decide the pages (Cua Volume's only with it on).
  const [state, setState] = useState<OnboardingState>(() =>
    reduceOnboarding(
      { ...initialOnboarding(installerMode ?? null, initialIdentity ?? null), step: initialStep },
      { type: "experiments-loaded", experiments: currentExperiments() },
    ),
  );
  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null);
  const telemetry = useMemo(() => telemetryProp ?? telemetryBridge(), [telemetryProp]);
  // Every step goes through here: the core's reducer, and the usage events
  // the core says the step means (the SwiftUI app sends the same ones).
  const stateRef = useRef(state);
  const send = (action: OnboardingAction) => {
    const before = stateRef.current;
    stateRef.current = reduceOnboarding(before, action);
    setState(stateRef.current);
    const signals = onboardingSignals(before, action);
    if (before.step === "welcome" && stateRef.current.step !== "welcome") {
      // Leaving Welcome: its usage-data switch decides before anything is
      // sent (the core derives nothing while Welcome shows).
      const on = onboardingView(before).usage?.on ?? true;
      void telemetry
        .welcomeLeft(on)
        .catch(() => undefined)
        .then(() => telemetry.recordSignals(signals));
    } else {
      telemetry.recordSignals(signals);
    }
  };
  useEffect(() => {
    if (initialIdentity) send({ type: "signed-in", identity: initialIdentity });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialIdentity]);

  const view = onboardingView(state);
  // Welcome's usage-data switch starts from the machine's setting (the
  // same one as Settings, Privacy and `cua telemetry off`).
  useEffect(() => {
    void telemetry
      .status()
      .then((t) => {
        const locked = t.sourceKind !== "config" && t.sourceKind !== "default";
        send({ type: "telemetry-loaded", telemetry: { enabled: t.enabled, lockedBy: locked ? t.source : null } });
      })
      .catch(() => {});
    // Once per flow.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const copy = useMemo(() => onboardingCopy(), []);
  const drive = useMemo(() => driveProp ?? createDriveBridge(), [driveProp]);
  const loginItem = useMemo(() => loginItemProp ?? createLoginItemBridge(), [loginItemProp]);
  const os = osProp ?? hostOs();
  // Done: the checkbox is the user's choice; the app registers (or not)
  // as the main window shows. A failure shows in Settings, General.
  const finish = () => {
    telemetry.recordSignals(onboardingFinishedSignals(state));
    if (loginItem.isNative) {
      const on = state.launchAtLogin ?? true;
      writeLaunchChoice(on);
      void loginItem.set(on).catch(() => {});
    }
    onDone(state.mode ?? "client");
  };

  // First launch asks what the volume's mount can do here and where its
  // files live (null when the daemon cannot answer: the page says so and
  // Continue moves on).
  // The home folder, so paths show as `~/...`.
  const home = useMemo(() => drive.home().catch(() => null), [drive]);
  const checkDrive = useMemo(
    () => () =>
      Promise.all([
        answerOrNull<DriveMountInput>(drive.call, "volume_mount_status"),
        answerOrNull<DriveStorageInput>(drive.call, "volume_storage"),
        home,
      ]).then(([status, storage, homeDir]) => {
        if (storage) send({ type: "drive-storage-loaded", storage, home: homeDir });
        send({ type: "drive-checked", os, status });
      }),
    [drive, os, home],
  );
  useEffect(() => {
    void checkDrive();
  }, [checkDrive]);
  // While macOS waits for the extension's approval, ask again every 2 s.
  const awaitingApproval = Boolean(view.drive?.settingsUrl);
  useEffect(() => {
    if (!awaitingApproval) return;
    const id = setInterval(() => void checkDrive(), DRIVE_APPROVAL_POLL_MS);
    return () => clearInterval(id);
  }, [awaitingApproval, checkDrive]);
  // The bucket's Test connection, or Continue's save: `volume_storage_set`
  // with the core's update (the keys travel only here).
  const storageRequest = state.storage?.request ?? null;
  const storageRunning = useRef(false);
  useEffect(() => {
    if (!storageRequest || storageRunning.current) return;
    if (storageRequest.kind !== "test" && storageRequest.kind !== "save" && storageRequest.kind !== "adopt") return;
    storageRunning.current = true;
    const kind = storageRequest.kind;
    const answer = (check: DriveCheckInput): OnboardingAction =>
      kind === "test"
        ? { type: "drive-storage", action: { type: "checked", check } }
        : kind === "adopt"
          ? { type: "drive-storage", action: { type: "adopted", check } }
          : { type: "drive-storage-saved", check };
    void drive
      .call("volume_storage_set", { ...storageRequest.update })
      .then((check: DriveCheckInput) => send(answer(check)))
      .catch((err: unknown) => {
        const error = message(err);
        send(
          kind === "test"
            ? { type: "drive-storage", action: { type: "failed", error } }
            : answer(failedCheck(error)),
        );
      })
      .finally(() => {
        storageRunning.current = false;
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageRequest, drive]);
  const storageAct = (action: StorageAction | null) => {
    if (action) send({ type: "drive-storage", action });
  };
  // While the card shows the agent prompt, look for the bucket the user's
  // agent sets up every 2 s; the core adopts it.
  const prompting = Boolean(view.drive?.storageRows.some((r) => r.id === "s3-prompt"));
  useEffect(() => {
    if (!prompting) return;
    const id = setInterval(() => {
      void Promise.all([answerOrNull<DriveStorageInput>(drive.call, "volume_storage"), home]).then(
        ([storage, homeDir]) => storage && send({ type: "drive-storage-loaded", storage, home: homeDir }),
      );
    }, DRIVE_PROMPT_POLL_MS);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prompting, drive, home]);

  // Continue with the box and the daemon disagreeing: mount or unmount.
  const running = useRef(false);
  useEffect(() => {
    const request = state.driveRequest;
    if (!request || running.current) return;
    running.current = true;
    const tool = request === "mount" ? "volume_mount" : "volume_unmount";
    void drive
      .call(tool)
      .then(async (answer) => {
        const status =
          answer !== null && typeof answer === "object" && !Array.isArray(answer)
            ? (answer as DriveMountInput)
            : await answerOrNull<DriveMountInput>(drive.call, "volume_mount_status");
        send(status ? { type: "drive-mounted", status } : { type: "drive-failed", error: String(answer) });
      })
      .catch((err: unknown) => send({ type: "drive-failed", error: message(err) }))
      .finally(() => {
        running.current = false;
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.driveRequest, drive]);

  // First launch installs the bundled `cua` silently (no page): onto the
  // plan's target, adding its bin dir to the shell profile when it is not
  // on PATH. Done shows where it went.
  useEffect(() => {
    let cancelled = false;
    void installer
      .cliPlan()
      .then(async (plan) => {
        if (plan.upToDate) return plan;
        if (!plan.source) return null;
        return installer.installCli({ modifyPath: !plan.onPath });
      })
      .then((plan) => {
        if (!cancelled && plan?.upToDate !== false && plan) send({ type: "cli-installed", target: plan.target });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // Once per first run.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [installer]);
  // The presentation page starts on the current setting.
  useEffect(() => {
    if (state.step === "presentation" && state.menuBar !== readMenuBar())
      send({ type: "presentation-picked", menuBar: readMenuBar() });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.step]);
  const back = view.canBack ? (
    <button type="button" className="secondary-button" data-owns-enter onClick={() => send({ type: "back" })}>
      {copy.back}
    </button>
  ) : null;

  let body: ReactElement;
  switch (view.step) {
    case "welcome":
      body = (
        <>
          <h2 className="onboarding-title">{view.title}</h2>
          <p className="host-setup-lede">{view.lede}</p>
          <footer className="shelf-footer create-cloud-actions">
            <button type="button" className="primary-button" data-owns-enter onClick={() => send({ type: "start" })}>
              {view.primaryLabel}
            </button>
          </footer>
          {view.usage && (
            <label className="onboarding-usage" title={view.usage.help ?? undefined}>
              <input
                type="checkbox"
                role="switch"
                checked={view.usage.on}
                disabled={!view.usage.enabled}
                onChange={(e) => send({ type: "usage-data-toggled", on: e.target.checked })}
              />
              {view.usage.label}
            </label>
          )}
          {view.notice && (
            <p className="onboarding-telemetry-notice" data-testid="telemetry-notice">
              {view.notice}{" "}
              {view.noticeLinkUrl && (
                <a href={view.noticeLinkUrl} target="_blank" rel="noreferrer">
                  {view.noticeLinkLabel}
                </a>
              )}
            </p>
          )}
        </>
      );
      break;
    case "signin":
      body = (
        <SignInStep
          view={view}
          back={back}
          auth={auth}
          identity={state.identity ?? undefined}
          onSignedIn={(identity) => identity && send({ type: "signed-in", identity })}
          onNext={() => send({ type: "signin-done" })}
          onOpenExternal={onOpenExternal}
        />
      );
      break;
    case "agents":
      body = (
        <AgentsStep
          view={view}
          back={back}
          installer={installer}
          onNext={(configured) => send({ type: "agents-done", configured })}
        />
      );
      break;
    case "presentation":
      body = (
        <PresentationStep
          view={view}
          back={back}
          onPick={(menuBar) => {
            writeMenuBar(menuBar);
            void import("@tauri-apps/api/event")
              .then(({ emit }) => emit("settings:changed", { menuBar }))
              .catch(() => {});
            send({ type: "presentation-picked", menuBar });
          }}
          onNext={() => send({ type: "presentation-done" })}
        />
      );
      break;
    case "drive":
      body = view.drive ? (
        <DriveStep
          view={view}
          back={back}
          onToggle={(on) => send({ type: "drive-toggled", on })}
          onOpenSettings={(url) => void host.openSettings(url).catch(() => {})}
          onChooseStorage={(choice) => send({ type: "storage-chosen", choice })}
          onStorageEdit={(row, value) => storageAct(storageEdit(row.id, value))}
          onStorageChoose={(row, option) => storageAct(storageChoose(row.id, option))}
          onStoragePress={(row) =>
            storageAct(
              storagePress(
                { os, home: null, storage: state.driveStorage ?? null, mount: state.driveStatus ?? null, cache: null },
                row.id,
              ),
            )
          }
          onOpenExternal={(url) => void host.openSettings(url).catch(() => {})}
          onNext={() => send({ type: "drive-continue" })}
        />
      ) : (
        <></>
      );
      break;
    case "mode":
      body = (
        <Onboarding
          host={host}
          view={view}
          back={back}
          identity={state.identity ?? undefined}
          onDone={(chosen, status) => {
            setHostStatus(status ?? null);
            send({ type: "mode-chosen", mode: chosen });
          }}
        />
      );
      break;
    case "done": {
      const perms = hostStatus ? permissionRows(hostStatus) : [];
      body = (
        <Page
          title={view.title}
          lede={view.lede}
          below={view.prompts.length > 0 ? <PromptTicker prompts={view.prompts} /> : undefined}
          primary={
            <button
              type="button"
              className="primary-button"
              data-owns-enter
              onClick={finish}
            >
              {view.primaryLabel}
            </button>
          }
        >
          <dl className="this-machine-facts">
            {view.summary.map((f) => (
              <div key={f.label}>
                <dt>{f.label}</dt>
                <dd>{f.label === "cua command" && state.cliTarget ? <code>{f.value}</code> : f.value}</dd>
              </div>
            ))}
          </dl>
          <PermissionRows
            title={copy.permissionsTitle}
            rows={perms}
            openLabel={copy.openSettings}
            onOpen={(url) => void host.openSettings(url)}
          />
          {view.launchAtLogin && loginItem.isNative && (
            <>
              <label className="create-cloud-toggle">
                <input
                  type="checkbox"
                  checked={view.launchAtLogin.checked}
                  onChange={(event) => send({ type: "launch-at-login-toggled", on: event.target.checked })}
                />
                <span>{view.launchAtLogin.label}</span>
              </label>
              <p className="st-note">{view.launchAtLogin.note}</p>
            </>
          )}
        </Page>
      );
      break;
    }
  }

  return (
    <section
      className="shelf create-cloud onboarding installer"
      aria-label="Set up Cua"
      data-step={view.step}
      data-layout={view.step === "welcome" ? "center" : "split"}
    >
      {view.showMark && (
        <span className="ob-app-icon">
          <SpacesStackMark size={120} />
        </span>
      )}
      {body}
      <ol className="installer-steps installer-dots" aria-label="Setup steps">
        {view.dots.map((d) => (
          <li key={d.step} aria-current={d.current ? "step" : undefined} aria-label={d.label} title={d.label} />
        ))}
      </ol>
    </section>
  );
}

/**
 * Done's example prompts (the core's), one quoted line each, scrolling up
 * slowly and continuously to suggest what to ask a coding agent. Not
 * interactive: CSS pauses it under the pointer, and with reduced motion it
 * is a static list of the first rows (the duplicate that makes the loop
 * seamless is hidden).
 */
function PromptTicker({ prompts }: { prompts: string[] }) {
  const row = (text: string) => `\u201C${text}\u201D`;
  return (
    <div className="ob-ticker" role="list" aria-label="Example prompts">
      <div className="ob-ticker-track" style={{ animationDuration: `${prompts.length * 4}s` }}>
        {prompts.map((p) => (
          <div key={p} className="ob-ticker-row" role="listitem">
            {row(p)}
          </div>
        ))}
        {prompts.map((p) => (
          <div key={`again-${p}`} className="ob-ticker-row ob-ticker-again" aria-hidden="true">
            {row(p)}
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * Where Cua Spaces shows up: two cards side by side, each a real capture of
 * the app in that mode (the core's cards). Picking one sets the same setting
 * as Settings' "Spaces tab in the notch".
 */
function PresentationStep({
  view,
  back,
  onPick,
  onNext,
}: {
  view: OnboardingView;
  back: ReactNode;
  onPick: (menuBar: boolean) => void;
  onNext: () => void;
}) {
  return (
    <Page
      title={view.title}
      lede={view.lede}
      back={back}
      primary={
        <button type="button" className="primary-button" data-owns-enter onClick={onNext}>
          {view.primaryLabel}
        </button>
      }
    >
      <div className="ob-presentations" role="radiogroup" aria-label={view.title}>
        {view.presentations.map((c) => (
          <button
            key={c.id}
            type="button"
            role="radio"
            aria-checked={c.selected}
            className="ob-presentation"
            data-selected={c.selected ? "true" : undefined}
            data-owns-enter
            onClick={() => onPick(c.menuBar)}
          >
            <span className="ob-presentation-picture" role="img" aria-label={c.imageLabel}>
              <PresentationPreview menuBar={c.menuBar} />
            </span>
            <span className="ob-presentation-title">{c.title}</span>
          </button>
        ))}
      </div>
    </Page>
  );
}

/**
 * The Cua Volume page: the miniature over one checkbox line (the AI agents
 * page's card), a muted note when the daemon cannot mount here or macOS
 * waits for approval, and Open System Settings then. Continue mounts or
 * unmounts first when the box and the daemon disagree (the core decides).
 */
function DriveStep({
  view,
  back,
  onToggle,
  onOpenSettings,
  onChooseStorage,
  onStorageEdit,
  onStorageChoose,
  onStoragePress,
  onOpenExternal,
  onNext,
}: {
  view: OnboardingView;
  back: ReactNode;
  onToggle: (on: boolean) => void;
  onOpenSettings: (url: string) => void;
  onChooseStorage: (choice: StorageChoice) => void;
  onStorageEdit: (row: SettingsRow, value: string) => void;
  onStorageChoose: (row: SettingsRow, option: string) => void;
  onStoragePress: (row: SettingsRow) => void;
  onOpenExternal: (url: string) => void;
  onNext: () => void;
}) {
  const card = view.drive!;
  return (
    <Page
      title={view.title}
      lede={view.lede}
      back={back}
      primary={
        <button
          type="button"
          className="primary-button"
          data-owns-enter
          disabled={card.busy || !card.canContinue}
          onClick={onNext}
        >
          {view.primaryLabel}
        </button>
      }
    >
      <div className="installer-driver installer-drive">
        {/* The bucket's rows take the miniature's place, so the page fits. */}
        {card.storageRows.length === 0 && (
          <span className="installer-driver-picture" role="img" aria-label={card.imageLabel}>
            <DriveMountPreview />
          </span>
        )}
        {card.storageTitle && card.storageOptions.length > 0 && (
          <div className="st-row installer-drive-storage">
            <span className="st-label">{card.storageTitle}</span>
            <div className="st-segmented" role="radiogroup" aria-label={card.storageTitle}>
              {card.storageOptions.map((o) => (
                <button
                  key={o.id}
                  type="button"
                  role="radio"
                  data-owns-enter
                  aria-checked={o.active}
                  data-active={o.active}
                  disabled={card.busy}
                  onClick={() => onChooseStorage(o.id)}
                >
                  {o.label}
                </button>
              ))}
            </div>
          </div>
        )}
        {card.storedIn && (
          <p className="st-note ob-path-line">
            <MiddleText text={card.storedIn} title={card.storedPath} />
          </p>
        )}
        {card.mountedAt && (
          <p className="st-note ob-path-line">
            <MiddleText text={card.mountedAt} title={card.mountedPath} />
          </p>
        )}
        {card.storageRows.length > 0 && (
          <div className="st-rows installer-drive-bucket" aria-label={card.storageTitle ?? undefined}>
            {card.storageRows.map((row) => (
              <SettingsRowItem
                key={row.id}
                row={row}
                onPress={onStoragePress}
                onChoose={onStorageChoose}
                onEdit={onStorageEdit}
                onOpenExternal={onOpenExternal}
              />
            ))}
          </div>
        )}
        {card.storageNote && <p className="st-note">{card.storageNote}</p>}
        <label className="create-cloud-toggle">
          <input
            type="checkbox"
            checked={card.checked}
            disabled={!card.enabled}
            onChange={(event) => onToggle(event.target.checked)}
          />
          <span>{card.label}</span>
        </label>
        {card.note && <p className="st-note">{card.note}</p>}
        {card.error && (
          <p className="create-cloud-error" role="alert">
            {card.error}
          </p>
        )}
        {card.settingsUrl && card.settingsLabel && (
          <div>
            <button
              type="button"
              className="secondary-button"
              data-owns-enter
              onClick={() => onOpenSettings(card.settingsUrl!)}
            >
              {card.settingsLabel}
            </button>
          </div>
        )}
      </div>
    </Page>
  );
}

function SignInStep({
  view,
  back,
  auth,
  identity,
  onSignedIn,
  onNext,
  onOpenExternal,
}: {
  view: OnboardingView;
  back: ReactNode;
  auth?: InstallerAuth;
  identity?: string;
  onSignedIn: (identity?: string) => void;
  onNext: () => void;
  onOpenExternal?: (url: string) => void;
}) {
  const copy = onboardingCopy();
  const [code, setCode] = useState<SignInStart | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [signedIn, setSignedIn] = useState(false);
  const done = Boolean(identity) || signedIn;

  useEffect(() => {
    if (!auth) return;
    let offOk: (() => void) | undefined;
    let offFail: (() => void) | undefined;
    let cancelled = false;
    void auth
      .onSignedIn((who) => {
        onSignedIn(who);
        setSignedIn(true);
        setWaiting(false);
      })
      .then((off) => (cancelled ? off() : (offOk = off)));
    void auth
      .onSignInFailed((reason) => {
        setError(reason);
        setWaiting(false);
      })
      .then((off) => (cancelled ? off() : (offFail = off)));
    return () => {
      cancelled = true;
      offOk?.();
      offFail?.();
    };
  }, [auth, onSignedIn]);

  const begin = () => {
    if (!auth || waiting) return;
    setError(null);
    setWaiting(true);
    auth
      .beginSignIn()
      .then(setCode)
      .catch((err: unknown) => {
        setError(message(err));
        setWaiting(false);
      });
  };

  return (
    <Page
      title={view.title}
      lede={view.lede}
      back={back}
      skip={
        view.canSkip && !done ? (
          <button type="button" className="secondary-button" data-owns-enter onClick={onNext}>
            {copy.skip}
          </button>
        ) : null
      }
      primary={
        done ? (
          <button type="button" className="primary-button" data-owns-enter onClick={onNext}>
            {view.primaryLabel}
          </button>
        ) : auth ? (
          <button type="button" className="primary-button" data-owns-enter disabled={waiting} onClick={begin}>
            {waiting ? copy.signInWaiting : copy.signIn}
          </button>
        ) : null
      }
    >
      {done ? <p className="host-setup-lede">{signedInText(identity)}</p> : null}
      {!done && code && (
        <p className="host-setup-lede" data-testid="signin-code">
          {signInCodeText(code.userCode)}
        </p>
      )}
      {error && (
        <p className="create-cloud-error" role="alert">
          {error}
        </p>
      )}
      {onOpenExternal && (
        <p className="onboarding-teams" data-testid="onboarding-teams">
          {copy.teams}{" \u00b7 "}
          <button type="button" className="dw-link" data-owns-enter onClick={() => onOpenExternal(copy.teamsUrl)}>
            {copy.teamsLink}
          </button>
        </p>
      )}
    </Page>
  );
}

function AgentsStep({
  view,
  back,
  installer,
  onNext,
}: {
  view: OnboardingView;
  back: ReactNode;
  installer: InstallerBridge;
  onNext: (configured: string[]) => void;
}) {
  const copy = onboardingCopy();
  const [report, setReport] = useState<AgentDetectReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [skills, setSkills] = useState(true);
  const [mcp, setMcp] = useState(true);
  const [driver, setDriver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<AgentSetupReport | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    installer
      .detectAgents()
      .then((next) => {
        if (cancelled) return;
        setReport(next);
        setSelected(new Set(next.agents.filter((a) => a.installed).map((a) => a.id)));
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(message(err));
      });
    return () => {
      cancelled = true;
    };
  }, [installer, attempt]);

  const installed = useMemo(() => report?.agents.filter((a) => a.installed) ?? [], [report]);
  const canSetup = selected.size > 0 && (skills || mcp || driver) && !busy;

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const setup = () => {
    if (!canSetup) return;
    setBusy(true);
    setError(null);
    const agents = installed.map((a) => a.id).filter((id) => selected.has(id));
    installer
      .setupAgents({ agents, skills, mcp, driver })
      .then((next) => {
        setResult(next);
        setBusy(false);
      })
      .catch((err: unknown) => {
        setError(message(err));
        setBusy(false);
      });
  };

  if (result) {
    const agents = installed.filter((a) => selected.has(a.id));
    const summaries = agents.map((a) => ({ agent: a, s: summarizeAgent(result, a.id, a.name) }));
    const configured = summaries.filter(({ s }) => s.failed.length === 0).map(({ agent }) => agent.name);
    return (
      <Page
        title={copy.agentsDoneTitle}
        lede={copy.agentsDoneLede}
        back={back}
        primary={
          <button type="button" className="primary-button" data-owns-enter onClick={() => onNext(configured)}>
            {copy.continueLabel}
          </button>
        }
      >
        <ul className="installer-agents" aria-label="Setup results">
          {summaries.map(({ agent, s }) => (
            <li key={agent.id} className={s.failed.length ? "installer-agent failed" : "installer-agent"}>
              <span className="installer-agent-name" title={s.text}>
                {s.line}
              </span>
              {s.failed.map((f) => (
                <span key={f} className="create-cloud-error">
                  {f}
                </span>
              ))}
            </li>
          ))}
        </ul>
      </Page>
    );
  }

  return (
    <Page
      title={view.title}
      lede={view.lede}
      back={back}
      skip={
        view.canSkip ? (
          <button type="button" className="secondary-button" data-owns-enter onClick={() => onNext([])}>
            {copy.skip}
          </button>
        ) : null
      }
      primary={
        error && !report ? (
          <button type="button" className="primary-button" data-owns-enter onClick={() => setAttempt((n) => n + 1)}>
            {copy.tryAgain}
          </button>
        ) : installed.length > 0 ? (
          <button type="button" className="primary-button" data-owns-enter disabled={!canSetup} onClick={setup}>
            {busy ? copy.agentsSettingUp : copy.agentsSetUp}
          </button>
        ) : null
      }
    >
      {!report && !error && <p className="host-setup-lede">{copy.agentsLooking}</p>}
      {report && installed.length === 0 && <p className="host-setup-lede">{copy.agentsNone}</p>}
      {installed.length > 0 && (
        <fieldset className="installer-agents" aria-label="Detected agents">
          {installed.map((a) => (
            <label key={a.id} className="create-cloud-toggle installer-agent">
              <input type="checkbox" checked={selected.has(a.id)} onChange={() => toggle(a.id)} />
              <span
                className="installer-agent-name"
                title={[a.mcpConfig ?? a.skillsDir, a.cuaConfigured ? "already connected" : "", a.error ?? ""]
                  .filter(Boolean)
                  .join(" · ")}
              >
                {a.name}
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {installed.length > 0 && (
        <div className="installer-options">
          <label className="create-cloud-toggle">
            <input type="checkbox" checked={skills} onChange={(event) => setSkills(event.target.checked)} />
            <span title={report?.skills.map((s) => s.name).join(", ")}>{copy.agentsSkills}</span>
          </label>
          <label className="create-cloud-toggle">
            <input type="checkbox" checked={mcp} onChange={(event) => setMcp(event.target.checked)} />
            <span>{copy.agentsMcp}</span>
          </label>
        </div>
      )}
      {installed.length > 0 && (
        <div className="installer-driver">
          <span className="installer-driver-picture" role="img" aria-label={copy.agentsDriverImage}>
            <DriverPreview />
          </span>
          <label className="create-cloud-toggle">
            <input type="checkbox" checked={driver} onChange={(event) => setDriver(event.target.checked)} />
            <span>{copy.agentsDriver}</span>
          </label>
        </div>
      )}
      {error && (
        <p className="create-cloud-error" role="alert">
          {error}
        </p>
      )}
    </Page>
  );
}
