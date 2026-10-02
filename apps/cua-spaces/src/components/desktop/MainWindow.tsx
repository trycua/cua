// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { FIXTURE_SPACES } from '../../model/fixtures';
import type { Space } from '../../model/types';
import {
  deleteFailedText,
  powerButton,
  mainChrome,
  sidebar as buildSidebar,
  spaceDetail,
  USAGE_REFRESH_MS,
  type DetailActionId,
  type PowerButton,
  type SidebarRow,
  type SpaceDetailView,
  type SpaceUsage,
} from '../../model/window';
import { createFileSendBridge, type FileSendBridge } from '../../native/fileSend';
import {
  type CloudPricing,
  type GpuChoice,
  createFleetBridge,
  type FleetBridge,
  type ViewerWindowRequest,
} from '../../native/fleet';
import { createTeleportBridge, type TeleportBridge } from '../../native/teleport';
import {
  createHostBridge,
  type HostBridge,
  type HostStatus,
  type OnboardingState,
} from '../../native/host';
import { createInstallerBridge, type InstallerBridge } from '../../native/installer';
import { createAgentsBridge, type AgentsBridge } from '../../native/persistent';
import { createDriveBridge, type DriveBridge } from '../../native/drive';
import { AgentsPage, DrivePage, NotificationsPage, useAgentNotifications } from './AgentsPages';
import { createDevicesBridge, type DevicesBridge } from '../../native/devices';
import { createShareBridge, type ShareBridge } from '../../native/share';
import { connectedClouds, createCloudBridge, isCloudWord, type CloudBridge, type CloudStatusWire } from '../../native/cloud';
import { ConnectCloudSheet } from '../ConnectCloudSheet';
import { ShareSheet } from '../ShareSheet';
import { hasFeature } from '../../model/spaces';
import { canRunContainer, canRunMacos, type LocalStatus } from '../../native/local';
import { createWindowDragBridge, type WindowDragBridge } from '../../native/windowDrag';
import type { PortalAction } from '../../state/portal';
import { useFleetSync } from '../../state/cloud';
import { createFromPlan } from '../../state/createSpace';
import { RadialProgress } from '../RadialProgress';
import { InstallerFlow } from '../InstallerFlow';
import { SettingsPanel } from '../SettingsPanel';
import {
  ApproveSheet,
  ConfirmMachineSheet,
  DevicesBanner,
  DevicesSection,
  EnrollSheet,
  RenameSheet,
  RevokeSheet,
  useDevices,
} from '../Devices';
import type { ApprovalPrompt, DeviceRow, UnconfirmedMachine } from '../../model/devices';
import { readMenuBar, writeMenuBar } from '../../state/settings';
import { SpaceAgentList } from '../SpaceAgentList';
import type { WriteText } from '../CopyButton';
import { FactList } from '../FactList';
import { OsIconMark } from '../OsIcon';
import { SpaceWindowList } from '../SpaceWindowList';
import TeleportDropZone from '../TeleportDropZone';
import { ThisMachinePanel } from '../ThisMachinePanel';
import { THIS_MACHINE_ID } from '../../model/host';
import { Thumbnail } from '../Thumbnail';
import { NewSpaceWizard, type CreatePlan, type SpaceHost } from './NewSpaceWizard';
import type { Experiments } from '../../model/experiments';
import { useExperiments } from '../../state/experiments';
import { createLoginItemBridge, type LoginItemBridge } from '../../native/loginItem';
import { launchPlan, readLaunchChoice, writeLaunchChoice } from '../../model/loginItem';
import { Sym } from './Sym';

export interface MainWindowProps {
  fleet?: FleetBridge;
  host?: HostBridge;
  installer?: InstallerBridge;
  teleport?: TeleportBridge;
  fileSend?: FileSendBridge;
  windowDrag?: WindowDragBridge;
  /** This device on the relay (Settings → Devices, approvals). */
  devices?: DevicesBridge;
  /** Sharing a Space with other accounts (the Share sheet). */
  share?: ShareBridge;
  /** The user's own clouds (the "Your cloud" tile and the Connect a cloud sheet). */
  cloud?: CloudBridge;
  /** Persistent agents, the Cua Volume and notifications (the daemon's tools). */
  agents?: AgentsBridge;
  /** The Cua Volume's storage, mount and cache (Settings, first run). */
  drive?: DriveBridge;
  /** Launch at login (the shell's login item; a fake in tests). */
  loginItem?: LoginItemBridge;
  /** Copy buttons' clipboard write (tests pass a fake). */
  copyText?: WriteText;
  now?: () => number;
  /** Browser preview only: show first-run onboarding without a shell. */
  forceOnboarding?: boolean;
}

type Banner = { tone: 'info' | 'error'; text: string } | null;

const SCREENSHOT_MS = 5_000;

function viewerRequest(space: Space): ViewerWindowRequest {
  return { id: space.id, name: space.name, controller: 'you', os: space.os };
}

/** The selected Space's detail (the app core's `spaces::sidebar::detail`). */
function detailOf(
  space: Space,
  usage: SpaceUsage | null | undefined,
  hostArch: string | null | undefined,
  experiments: Experiments
): SpaceDetailView {
  return spaceDetail(space, usage, hostArch, experiments);
}

/**
 * The Cua Spaces main window: a sidebar (This machine, one section per
 * location, the Keyvault, the account line), a toolbar for the selected
 * Space, and its detail (live preview, facts, Stream, Agents, Teleport).
 * First run shows the welcome flow in the same window; New Space is a
 * step-by-step sheet. The sections, rows, words, buttons and their order are
 * the app core's (`spaces::sidebar`, `window`, `keyvault::browse`), the same
 * the SwiftUI app draws. Talks to the shell's Spaces commands directly, so
 * it works whether or not the notch portal is open.
 */
export function MainWindow({
  fleet: fleetProp,
  host: hostProp,
  installer: installerProp,
  teleport: teleportProp,
  fileSend: fileSendProp,
  windowDrag: windowDragProp,
  devices: devicesProp,
  share: shareProp,
  cloud: cloudProp,
  agents: agentsProp,
  drive: driveProp,
  loginItem: loginItemProp,
  copyText,
  now = Date.now,
  forceOnboarding = false,
}: MainWindowProps) {
  const fleet = useMemo(() => fleetProp ?? createFleetBridge(), [fleetProp]);
  const host = useMemo(() => hostProp ?? createHostBridge(), [hostProp]);
  const installer = useMemo(() => installerProp ?? createInstallerBridge(), [installerProp]);
  const teleport = useMemo(() => teleportProp ?? createTeleportBridge(), [teleportProp]);
  const fileSend = useMemo(() => fileSendProp ?? createFileSendBridge(), [fileSendProp]);
  const windowDrag = useMemo(() => windowDragProp ?? createWindowDragBridge(), [windowDragProp]);
  const devicesBridge = useMemo(() => devicesProp ?? createDevicesBridge(), [devicesProp]);
  const shareBridge = useMemo(() => shareProp ?? createShareBridge(), [shareProp]);
  const cloudBridge = useMemo(() => cloudProp ?? createCloudBridge(), [cloudProp]);
  const [cloudStatus, setCloudStatus] = useState<CloudStatusWire | null>(null);
  const [connectingCloud, setConnectingCloud] = useState(false);
  const refreshClouds = useCallback(() => {
    void cloudBridge.status().then(setCloudStatus, () => setCloudStatus(null));
  }, [cloudBridge]);
  const clouds = useMemo(() => connectedClouds(cloudStatus), [cloudStatus]);
  const agentsBridge = useMemo(() => agentsProp ?? createAgentsBridge(), [agentsProp]);
  const driveBridge = useMemo(() => driveProp ?? createDriveBridge(), [driveProp]);
  const loginItem = useMemo(() => loginItemProp ?? createLoginItemBridge(), [loginItemProp]);
  const postNotification = useCallback((title: string, body: string) => devicesBridge.notify(title, body), [devicesBridge]);
  const feed = useAgentNotifications(agentsBridge, postNotification);
  const [sharing, setSharing] = useState<Space | null>(null);
  const live = fleet.isNative;

  const [spaces, setSpaces] = useState<Space[]>(live ? [] : FIXTURE_SPACES);
  const dispatch = useCallback((action: PortalAction) => {
    if (action.type === 'sync-spaces') setSpaces(action.spaces);
  }, []);
  const sync = useFleetSync(fleet, dispatch, now);
  // Devices: read quietly while signed in; a device asking for approval
  // posts a notification and opens the approval sheet.
  const devices = useDevices(devicesBridge, Boolean(sync.identity), now);
  // The row's Deny (no sheet): revokes a still-pending device in one
  // click, or (a device due for re-verification) only dismisses it.
  const denyApproval = useCallback(
    (prompt: ApprovalPrompt) => {
      if (prompt.expired) {
        devices.dismiss(prompt.deviceId);
        return;
      }
      void devicesBridge.revoke(prompt.deviceId).then(() => void devices.refresh());
    },
    [devices, devicesBridge],
  );
  const [enrolling, setEnrolling] = useState(false);
  const [renaming, setRenaming] = useState<DeviceRow | null>(null);
  const [revoking, setRevoking] = useState<DeviceRow | null>(null);
  const [confirmingMachine, setConfirmingMachine] = useState<UnconfirmedMachine | null>(null);
  // The enroll sheet's "Sign in again": the app's sign-in, until it lands.
  const signInAndWait = useCallback(
    () =>
      new Promise<void>((resolve, reject) => {
        const offs: Array<() => void> = [];
        const done = (fn: () => void) => {
          for (const off of offs) off();
          fn();
        };
        void fleet.onSignedIn(() => done(resolve)).then((off) => offs.push(off));
        void fleet
          .onSignInFailed((reason) => done(() => reject(new Error(reason))))
          .then((off) => offs.push(off));
        fleet.beginSignIn().catch((e: unknown) => done(() => reject(e)));
      }),
    [fleet]
  );

  const [onboarding, setOnboarding] = useState<OnboardingState | null>(
    forceOnboarding ? { completed: false } : null
  );
  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null);
  const [local, setLocal] = useState<LocalStatus | null>(null);
  const [cloudPricing, setCloudPricing] = useState<CloudPricing | null>(null);
  const [gpus, setGpus] = useState<GpuChoice[] | null>(null);
  const [hosts, setHosts] = useState<SpaceHost[]>([]);
  // Settings, Experiments: what the window shows (Share, your clouds).
  const experiments = useExperiments();
  // `?view=new-space|settings|this-machine|host-setup|drive|onboarding-presentation|onboarding-drive|onboarding-done` opens that
  // view on load (the shell sets it from CUA_SPACES_START_VIEW, for demos
  // and captures), as the SwiftUI app's start views do.
  const startView =
    typeof window === 'undefined' ? null : new URLSearchParams(window.location.search).get('view');
  const [selectedId, setSelectedId] = useState<string | null>(
    startView === 'this-machine' || startView === 'host-setup' ? THIS_MACHINE_ID : null
  );
  const [query, setQuery] = useState('');
  const [wizard, setWizard] = useState(
    startView === 'new-space' || startView === 'new-space-resources'
  );
  useEffect(() => {
    if (wizard) refreshClouds();
  }, [wizard, refreshClouds]);
  // Captures: `?view=new-space-resources&image=..&on=..&disk=..` opens on
  // the Resources step (the SwiftUI app's CUA_SPACES_START_VIEW does the same).
  const [startActions] = useState(() => {
    if (startView !== 'new-space-resources') return undefined;
    const q = new URLSearchParams(window.location.search);
    const actions: ({ type: string } & Record<string, unknown>)[] = [];
    if (q.get('image')) actions.push({ type: 'choose-image', ref: q.get('image') });
    actions.push({ type: 'set-placement', placement: q.get('on') === 'cloud' ? 'cloud' : 'local' });
    actions.push({ type: 'next' });
    if (Number(q.get('disk'))) actions.push({ type: 'set-disk', diskGb: Number(q.get('disk')) });
    if (Number(q.get('cpus'))) actions.push({ type: 'set-cpus', cpus: Number(q.get('cpus')) });
    if (Number(q.get('memory')))
      actions.push({ type: 'set-memory', memoryGb: Number(q.get('memory')) });
    return actions;
  });
  const [banner, setBanner] = useState<Banner>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [view, setView] = useState<'spaces' | 'settings' | 'agents' | 'drive' | 'notifications'>(
    startView === 'settings'
      ? 'settings'
      : startView === 'drive'
        ? 'drive'
        : 'spaces'
  );
  const [menuBar, setMenuBar] = useState<boolean>(() => readMenuBar());
  const openSpace = useCallback((id: string) => {
    setSelectedId(id);
    setView('spaces');
  }, []);

  useEffect(() => {
    if (!host.isNative) return;
    let cancelled = false;
    void host
      .onboardingState()
      .then((next) => !cancelled && setOnboarding(next))
      .catch(() => {});
    void host
      .status()
      .then((next) => !cancelled && setHostStatus(next))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [host]);

  useEffect(() => {
    if (!live) return;
    sync.setThisMachine(hostStatus);
  }, [live, hostStatus, sync.setThisMachine]);

  // What a restart stops here: the Spaces this machine provides and its
  // persistent agents (Settings' note when launch at login is off).
  const [runsAgents, setRunsAgents] = useState(false);
  useEffect(() => {
    if (!loginItem.isNative) return;
    let cancelled = false;
    void agentsBridge
      .agents()
      .then((a) => !cancelled && setRunsAgents(a.length > 0))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [agentsBridge, loginItem, view]);
  const serves = useMemo(
    () => ({ providesSpaces: Boolean(hostStatus?.configured && hostStatus.provideSpaces), runsAgents }),
    [hostStatus, runsAgents]
  );

  // Launch at login, once per launch after the first run: an install that
  // never chose (its first run predates the setting) is turned on when this
  // machine provides Spaces or runs persistent agents (the core's rule).
  const launchChecked = useRef(false);
  useEffect(() => {
    if (launchChecked.current || !loginItem.isNative || !onboarding?.completed || !hostStatus) return;
    launchChecked.current = true;
    void (async () => {
      const agents = await agentsBridge.agents().catch(() => []);
      const status = await loginItem.status();
      const plan = launchPlan(
        readLaunchChoice(),
        true,
        Boolean(hostStatus.configured && hostStatus.provideSpaces) || agents.length > 0,
        status
      );
      if (plan.register) await loginItem.set(true);
      if (plan.record !== null) writeLaunchChoice(plan.record);
    })().catch(() => {});
  }, [loginItem, agentsBridge, onboarding?.completed, hostStatus]);

  // Again each time New Space opens: free space and pulled images change.
  useEffect(() => {
    if (!live) return;
    let cancelled = false;
    void fleet
      .localStatus()
      .then((next) => !cancelled && setLocal(next))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet, live, wizard]);

  // This account's cloud rates for the estimate, each time New Space opens
  // (the SDK reuses an answer for five minutes).
  useEffect(() => {
    if (!live || !wizard || !sync.fleetConfigured || !fleet.cloudPricing) return;
    let cancelled = false;
    void fleet
      .cloudPricing()
      .then((next) => !cancelled && setCloudPricing(next))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet, live, wizard, sync.fleetConfigured]);

  // The GPU option of each local runtime, each time New Space opens.
  useEffect(() => {
    if (!live || !wizard || !fleet.gpuSupport) return;
    let cancelled = false;
    void fleet
      .gpuSupport()
      .then((next) => !cancelled && setGpus(next))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet, live, wizard]);

  // Your machines that provide Spaces (the Run on menu), each time New
  // Space opens.
  useEffect(() => {
    if (!live || !wizard || !fleet.listHosts) return;
    let cancelled = false;
    void fleet
      .listHosts()
      .then((next) => !cancelled && setHosts(next))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [fleet, live, wizard]);

  // The shell (the notch's list button, the Dock) can ask for a Space or the
  // New Space sheet.
  useEffect(() => {
    if (!live) return;
    const offs: Array<() => void> = [];
    let cancelled = false;
    void import('@tauri-apps/api/event')
      .then(async ({ listen }) => {
        const a = await listen<{ spaceId?: string | null }>('main:select', (event) => {
          if (event.payload?.spaceId) openSpace(event.payload.spaceId);
        });
        const b = await listen('main:new-space', () => setWizard(true));
        const c = await listen('main:settings', () => setView('settings'));
        // The menu bar item's Cua Volume conflicts.
        const d = await listen('main:volume', () => setView('drive'));
        if (cancelled) {
          a();
          b();
          c();
          d();
        } else offs.push(a, b, c, d);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      for (const off of offs) off();
    };
  }, [live, openSpace]);

  // Sections, search and the selection fallback are the app core's.
  const sidebar = useMemo(
    () => buildSidebar(spaces, query, selectedId),
    [spaces, query, selectedId]
  );
  const byId = useMemo(() => new Map(spaces.map((s) => [s.id, s])), [spaces]);
  const selected = (sidebar.selectedId ? byId.get(sidebar.selectedId) : undefined) ?? null;
  // Memory and storage of the Space whose detail shows, at the core's low rate.
  const [usage, setUsage] = useState<{ id: string; usage: SpaceUsage } | null>(null);
  const usageId = selected && selected.sdk?.reachable ? selected.id : null;
  useEffect(() => {
    if (!usageId || !teleport.spaceUsage) return;
    let cancelled = false;
    const read = () =>
      teleport.spaceUsage?.(usageId).then(
        (u) => !cancelled && u && setUsage({ id: usageId, usage: u }),
        () => {}
      );
    void read();
    const timer = window.setInterval(read, USAGE_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [teleport, usageId]);
  const chrome = mainChrome({
    identity: sync.identity,
    cloudConfigured: sync.fleetConfigured,
    canSignIn: live,
    experiments,
  });
  // The Volume page (and its route, `main:volume`) only with the Cua Volume
  // experiment on (the core's `volumeLabel`); off, it shows the Spaces.
  const volumeShown = Boolean(chrome.volumeLabel);

  const cloudAvailable = live ? sync.fleetConfigured : true;
  const localAvailable = live ? canRunContainer(local) || canRunMacos(local) : true;

  const startCreate = useCallback(
    (plan: CreatePlan) => {
      setWizard(false);
      // The new Space's row shows at once, selected, and follows its
      // progress (the core's `spaces::creating`); a failure stays on it.
      const pendingId = `pending:${globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2)}`;
      openSpace(pendingId);
      createFromPlan(sync, plan, pendingId)
        .then((id) => {
          setSelectedId((current) => (current === pendingId ? id : current));
          if (plan.openDesktop) {
            void fleet.openSpaceWindow({
              id,
              name: plan.name || plan.image.name,
              controller: 'you',
              os: plan.image.os,
            });
          }
        })
        .catch(() => {
          // The row says why (the core's pending row keeps the error).
        });
    },
    [fleet, sync, openSpace]
  );

  // The power button: the row says Suspending (and the like) at once; a
  // failure shows inline on the row and the detail.
  const powerSpace = useCallback(
    (id: string, on: boolean) => {
      const space = byId.get(id);
      if (space) void sync.setPower(space, on);
    },
    [byId, sync]
  );

  const deleteSpace = useCallback(
    (space: Space, removeOnly = false) => {
      setConfirmDelete(null);
      sync.deleteSpace(space, removeOnly).catch((error: unknown) => {
        setBanner({
          tone: 'error',
          text: deleteFailedText(
            space.name,
            error instanceof Error ? error.message : String(error)
          ),
        });
      });
    },
    [sync]
  );

  const signIn = useCallback(() => {
    fleet.beginSignIn().catch((error: unknown) => {
      setBanner({
        tone: 'error',
        text: `Sign-in failed: ${error instanceof Error ? error.message : String(error)}`,
      });
    });
  }, [fleet]);

  if (onboarding && !onboarding.completed) {
    return (
      <div className="dw" data-view="welcome">
        <div className="ob-drag" data-tauri-drag-region />
        <main className="ob">
          <InstallerFlow
            installer={installer}
            host={host}
            drive={driveBridge}
            loginItem={loginItem}
            auth={
              fleet.isNative
                ? {
                    beginSignIn: fleet.beginSignIn,
                    onSignedIn: fleet.onSignedIn,
                    onSignInFailed: fleet.onSignInFailed,
                  }
                : undefined
            }
            installerMode={onboarding.installerMode}
            identity={sync.identity}
            onOpenExternal={(url) => void fleet.openExternal(url).catch(() => {})}
            initialStep={
              startView === 'onboarding-done'
                ? 'done'
                : startView === 'onboarding-presentation'
                  ? 'presentation'
                  : startView === 'onboarding-drive'
                    ? 'drive'
                    : undefined
            }
            onDone={(mode) => {
              setOnboarding({ ...onboarding, completed: true, mode });
              void host
                .status()
                .then(setHostStatus)
                .catch(() => {});
            }}
          />
        </main>
      </div>
    );
  }

  const detail = selected
    ? detailOf(
        selected,
        usage?.id === selected.id ? usage.usage : null,
        sync.hostArch,
        experiments
      )
    : null;
  const isHost = Boolean(detail?.isHost);
  const runAction = (id: DetailActionId, space: Space) => {
    switch (id) {
      case 'teleport':
        void fleet.openTeleportPicker(viewerRequest(space));
        break;
      case 'pip':
        void fleet.pinSpacePip(viewerRequest(space));
        break;
      case 'share':
        setSharing(space);
        break;
      case 'power': {
        const button = powerButton(space);
        if (button) powerSpace(space.id, button.turnOn);
        break;
      }
      case 'delete':
        setConfirmDelete(space.id);
        break;
      case 'open':
        void fleet.openSpaceWindow(viewerRequest(space));
        break;
      case 'cancel':
        // The row shows Cancelling, then goes (or says why the cancel failed).
        void sync.cancelCreate(space.id);
        break;
    }
  };

  return (
    <div className="dw" data-view="spaces">
      <aside className="dw-sidebar" aria-label="Spaces">
        <div className="dw-sidebar-top" data-tauri-drag-region>
          <button
            type="button"
            className="dw-icon-btn"
            aria-label={chrome.newSpaceLabel}
            title={`${chrome.newSpaceLabel} (${chrome.newSpaceShortcut})`}
            onClick={() => setWizard(true)}
          >
            <Sym name="plus" />
          </button>
        </div>
        <div className="dw-sidebar-search">
          <Sym name="magnifyingglass" size={14} />
          <input
            className="dw-input"
            type="search"
            placeholder={chrome.searchPlaceholder}
            aria-label="Search Spaces"
            value={query}
            spellCheck={false}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <nav className="dw-nav" aria-label="Sidebar">
          {sidebar.thisMachine && (
            <ul className="dw-nav-list" role="listbox" aria-label={sidebar.thisMachine.name}>
              <SpaceRowItem
                row={sidebar.thisMachine}
                selected={view === 'spaces' && selected?.id === sidebar.thisMachine.id}
                onSelect={openSpace}
              />
            </ul>
          )}
          {sidebar.sections.map((section) => (
            <div key={section.title}>
              <div className="dw-nav-section">{section.title}</div>
              <ul className="dw-nav-list" role="listbox" aria-label={section.title}>
                {section.rows.map((row) => (
                  <SpaceRowItem
                    key={row.id}
                    row={row}
                    selected={view === 'spaces' && selected?.id === row.id}
                    onSelect={openSpace}
                    onPower={powerSpace}
                  />
                ))}
              </ul>
            </div>
          ))}
          {sidebar.emptyText && !sidebar.thisMachine && (
            <p className="dw-hint dw-nav-section">{sidebar.emptyText}</p>
          )}
          <ul className="dw-nav-list" aria-label="Agents">
            {(
              [
                ['agents', 'Agents'],
                ...(chrome.volumeLabel ? [['drive', chrome.volumeLabel] as const] : []),
                ['notifications', 'Notifications'],
              ] as const
            ).map(([id, label]) => (
              <li key={id}>
                <button type="button" className="dw-row" aria-selected={view === id} onClick={() => setView(id)}>
                  {label}
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <div className="dw-sidebar-foot">
          <span className="dw-account">{chrome.account}</span>
          {chrome.signInLabel && (
            <button type="button" className="dw-btn" onClick={signIn}>
              {chrome.signInLabel}
            </button>
          )}
          <button
            type="button"
            className="dw-icon-btn"
            aria-label={chrome.settingsLabel}
            title={`${chrome.settingsLabel} (${chrome.settingsShortcut})`}
            aria-pressed={view === 'settings'}
            onClick={() => setView((v) => (v === 'settings' ? 'spaces' : 'settings'))}
          >
            <Sym name="gearshape" />
          </button>
        </div>
      </aside>

      <main className="dw-main">
        {view === 'agents' ? (
          <AgentsPage
            bridge={agentsBridge}
            thisMachine={sidebar.thisMachine?.id.startsWith('relay:') ? sidebar.thisMachine.id : null}
            now={now}
          />
        ) : view === 'drive' && volumeShown ? (
          <DrivePage bridge={agentsBridge} />
        ) : view === 'notifications' ? (
          <NotificationsPage feed={feed} bridge={agentsBridge} now={now} />
        ) : (
          <>
            <header className="dw-toolbar" data-tauri-drag-region>
              <div className="dw-toolbar-title" data-tauri-drag-region>
                {view === 'settings' ? (
                  <h1>{chrome.settingsLabel}</h1>
                ) : selected ? (
                  <h1>{selected.name}</h1>
                ) : (
                  <h1>{chrome.title}</h1>
                )}
              </div>
              {view === 'spaces' && selected && detail && detail.actions.length > 0 && (
                <div className="dw-toolbar-actions">
                  {detail.actions.map((a) =>
                    a.primary ? (
                      <button
                        key={a.id}
                        type="button"
                        className="dw-btn dw-btn-primary"
                        title={a.help}
                        disabled={!a.enabled}
                        onClick={() => runAction(a.id, selected)}
                      >
                        {a.label}
                      </button>
                    ) : !a.symbol ? (
                      <button
                        key={a.id}
                        type="button"
                        className="dw-btn"
                        title={a.help}
                        disabled={!a.enabled}
                        onClick={() => runAction(a.id, selected)}
                      >
                        {a.label}
                      </button>
                    ) : (
                      <button
                        key={a.id}
                        type="button"
                        className="dw-icon-btn"
                        aria-label={a.label}
                        title={a.help}
                        disabled={!a.enabled}
                        onClick={() => runAction(a.id, selected)}
                      >
                        {a.busy ? (
                          <span className="dw-spinner" aria-hidden="true" />
                        ) : a.symbol ? (
                          <Sym name={a.symbol} />
                        ) : (
                          a.label
                        )}
                      </button>
                    )
                  )}
                </div>
              )}
            </header>

            {view === 'spaces' && selected && detail && confirmDelete === selected.id && (
              <div className="dw-banner" role="alertdialog" aria-label={detail.confirm.title}>
                <span>
                  <strong>{detail.confirm.title}</strong> {detail.confirm.message}
                </span>
                <button type="button" className="dw-btn" onClick={() => setConfirmDelete(null)}>
                  {detail.confirm.cancelLabel}
                </button>
                {detail.confirm.removeLabel && (
                  // A Space in your cloud: forget it, keep it running there.
                  <button type="button" className="dw-btn" onClick={() => deleteSpace(selected, true)}>
                    {detail.confirm.removeLabel}
                  </button>
                )}
                <button
                  type="button"
                  className="dw-btn dw-btn-danger"
                  disabled={detail.confirm.confirmEnabled === false}
                  title={detail.confirm.disabledReason ?? undefined}
                  onClick={() => deleteSpace(selected)}
                >
                  {detail.confirm.confirmLabel}
                </button>
              </div>
            )}

            <DevicesBanner view={devices.view} onEnroll={() => setEnrolling(true)} />

            {sharing && (
              <ShareSheet
                bridge={shareBridge}
                spaceId={sharing.id}
                spaceName={sharing.name}
                signedIn={Boolean(sync.identity)}
                shareable={sharing.id.startsWith('relay:') || hasFeature(sharing, 'relay_attach')}
                onClose={() => setSharing(null)}
              />
            )}

            {banner && (
              <div
                className="dw-banner"
                data-tone={banner.tone}
                role={banner.tone === 'error' ? 'alert' : 'status'}
              >
                <span>{banner.text}</span>
                <button
                  type="button"
                  className="dw-icon-btn"
                  aria-label="Dismiss"
                  onClick={() => setBanner(null)}
                >
                  <Sym name="xmark" size={12} />
                </button>
              </div>
            )}

            <div className="dw-content">
              {view === 'settings' ? (
                <div className="dw-content-inner dw-settings">
                  <SettingsPanel
                    menuBar={menuBar}
                    onMenuBarMode={(enabled) => {
                      writeMenuBar(enabled);
                      setMenuBar(enabled);
                      void import('@tauri-apps/api/event')
                        .then(({ emit }) => emit('settings:changed', { menuBar: enabled }))
                        .catch(() => {});
                    }}
                    fleetLive={sync.fleetConfigured}
                    clientId={sync.clientId}
                    signedInIdentity={sync.identity}
                    auth={
                      fleet.isNative
                        ? {
                            beginSignIn: () => fleet.beginSignIn(),
                            signOut: () => fleet.signOut(),
                            onSignedIn: (h) => fleet.onSignedIn(h),
                            onSignInFailed: (h) => fleet.onSignInFailed(h),
                            onSignedOut: (h) => fleet.onSignedOut(h),
                          }
                        : undefined
                    }
                    onOpenExternal={(url) => void fleet.openExternal(url).catch(() => {})}
                    defaultLocation={live ? sync.defaultLocation : undefined}
                    onDefaultLocation={sync.setDefaultLocation}
                    billingStatus={fleet.billingStatus}
                    drive={driveBridge}
                    loginItem={loginItem}
                    serves={serves}
                    onShowWelcome={() => {
                      setView('spaces');
                      setOnboarding({ ...(onboarding ?? {}), completed: false });
                    }}
                    onClose={() => setView('spaces')}
                  />
                  {devicesBridge.isNative && (
                    <DevicesSection
                      view={devices.view}
                      error={devices.error}
                      signedIn={Boolean(sync.identity)}
                      nowMs={now()}
                      onEnroll={() => setEnrolling(true)}
                      onApprove={(prompt) => devices.setApproving(prompt)}
                      onDeny={denyApproval}
                      onRename={setRenaming}
                      onRevoke={setRevoking}
                      onConfirmMachine={setConfirmingMachine}
                    />
                  )}
                </div>
              ) : !selected ? (
                <div className="dw-empty">
                  <span className="dw-empty-art">
                    <Sym name="square.grid.2x2" />
                  </span>
                  <h2>{chrome.emptyTitle}</h2>
                  <button
                    type="button"
                    className="dw-btn dw-btn-primary dw-btn-lg"
                    onClick={() => setWizard(true)}
                  >
                    {chrome.emptyAction}
                  </button>
                </div>
              ) : isHost ? (
                <div className="dw-content-inner">
                  <section className="dw-card dw-card-pad dw-host">
                    <ThisMachinePanel
                      host={host}
                      identity={sync.identity}
                      onStatus={setHostStatus}
                      initialSetup={startView === 'host-setup'}
                    />
                  </section>
                </div>
              ) : (
                <SpaceDetail
                  key={selected.id}
                  space={selected}
                  detail={detail!}
                  fleet={fleet}
                  teleport={teleport}
                  fileSend={fileSend}
                  windowDrag={windowDrag}
                  copyText={copyText}
                />
              )}
            </div>
          </>
        )}
      </main>

      {/* A start view waits for the local status, so its steps can advance. */}
      {wizard && (!startActions || local) && (
        <div
          className="dw-scrim"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setWizard(false);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setWizard(false);
          }}
        >
          <NewSpaceWizard
            defaultLocation={
              sync.defaultLocation.value === 'cloud'
                ? 'cloud'
                : isCloudWord(sync.defaultLocation.value)
                  ? 'yours'
                  : 'local'
            }
            clouds={clouds}
            hosts={hosts}
            experiments={experiments}
            onConnectCloud={() => setConnectingCloud(true)}
            cloudAvailable={cloudAvailable}
            localAvailable={localAvailable}
            localReason={local?.error ?? undefined}
            localBackends={live ? (local?.backends ?? []) : undefined}
            hostArch={sync.hostArch ?? local?.hostArch}
            storage={live ? local?.storage : undefined}
            cloudPricing={live && sync.fleetConfigured ? cloudPricing : null}
            gpus={live ? gpus : null}
            onOpenExternal={(url) => void fleet.openExternal(url).catch(() => {})}
            startActions={startActions}
            onCreate={startCreate}
            onAddByAddress={async (url, token, name) => {
              const id = await sync.addSpace(url, token, name);
              openSpace(id);
            }}
            onCancel={() => setWizard(false)}
          />
        </div>
      )}
      {connectingCloud && (
        <ConnectCloudSheet
          bridge={cloudBridge}
          onConnected={() => {
            setConnectingCloud(false);
            refreshClouds();
            void sync.refreshDefaultLocation();
          }}
          onClose={() => setConnectingCloud(false)}
        />
      )}
      {enrolling && (
        <EnrollSheet
          bridge={devicesBridge}
          signIn={signInAndWait}
          onClose={() => {
            setEnrolling(false);
            void devices.refresh();
          }}
          onEnrolled={() => void devices.refresh()}
        />
      )}
      {devices.approving && (
        <ApproveSheet
          key={devices.approving.deviceId}
          bridge={devicesBridge}
          prompt={devices.approving}
          devices={devices.input?.devices ?? []}
          onClose={() => devices.setApproving(null)}
          onDone={() => {
            devices.setApproving(null);
            void devices.refresh();
          }}
        />
      )}
      {renaming && devices.view && (
        <RenameSheet
          row={renaming}
          title={devices.view.labels.renameTitle}
          confirmLabel={devices.view.labels.renameConfirm}
          cancelLabel={devices.view.labels.cancel}
          onRename={async (name) => {
            await devicesBridge.rename(renaming.id, name);
            await devices.refresh();
          }}
          onClose={() => setRenaming(null)}
        />
      )}
      {revoking?.revokeConfirm && (
        <RevokeSheet
          row={revoking}
          onRevoke={async () => {
            await devicesBridge.revoke(revoking.id);
            await devices.refresh();
          }}
          onClose={() => setRevoking(null)}
        />
      )}
      {confirmingMachine && (
        <ConfirmMachineSheet
          machine={confirmingMachine}
          onConfirm={async () => {
            await devicesBridge.confirmMachine(confirmingMachine.id);
            await devices.refresh();
          }}
          onClose={() => setConfirmingMachine(null)}
        />
      )}
      <Shortcuts onNew={() => setWizard(true)} onSettings={() => setView('settings')} />
    </div>
  );
}

function Shortcuts({ onNew, onSettings }: { onNew: () => void; onSettings: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      if (event.key.toLowerCase() === 'n') {
        event.preventDefault();
        onNew();
      } else if (event.key === ',') {
        event.preventDefault();
        onSettings();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onNew, onSettings]);
  return null;
}

/** A row's power button (the core's `PowerButton`): the `power` symbol,
 * a spinner while it runs. */
function PowerRowButton({ button, onPress }: { button: PowerButton; onPress: () => void }) {
  return (
    <button
      type="button"
      className="dw-icon-btn dw-row-power"
      aria-label={button.help}
      title={button.help}
      disabled={!button.enabled}
      data-busy={button.busy ? 'true' : undefined}
      onClick={onPress}
    >
      {button.busy ? <span className="dw-spinner" aria-hidden="true" /> : <Sym name={button.symbol} />}
    </button>
  );
}

function SpaceRowItem({
  row,
  selected,
  onSelect,
  onPower,
}: {
  row: SidebarRow;
  selected: boolean;
  onSelect: (id: string) => void;
  /** The power button was pressed: turn it on (`true`) or off. */
  onPower?: (id: string, on: boolean) => void;
}) {
  const power = row.power;
  return (
    <li className="dw-row-wrap" data-power={power && onPower ? 'true' : undefined}>
      <button
        type="button"
        role="option"
        aria-selected={selected}
        className="dw-row"
        data-dim={row.dim ? 'true' : undefined}
        data-nested={row.nested ? 'true' : undefined}
        onClick={() => onSelect(row.id)}
      >
        <OsIconMark id={row.osIcon} size={13} className="dw-row-os" />
        <span className="dw-row-name" title={row.detail || undefined}>
          {row.name}
        </span>
        {row.place && <span className="dw-row-place">{row.place}</span>}
        {row.progress != null ? (
          // Being created: the percentage and a ring, no dot.
          <>
            <span className="dw-row-trailing">{row.trailing}</span>
            <span role="img" aria-label={row.statusText}>
              <RadialProgress className="dw-row-ring" fraction={row.progress / 1000} />
            </span>
          </>
        ) : (
          <>
            {row.trailing && (
              // The create failed: why, inline.
              <span className="dw-row-trailing" data-tone="error" title={row.trailing}>
                {row.trailing}
              </span>
            )}
            <span className="dw-dot" data-status={row.status} aria-label={row.statusText} />
          </>
        )}
      </button>
      {power && onPower && (
        <PowerRowButton button={power} onPress={() => onPower(row.id, power.turnOn)} />
      )}
    </li>
  );
}

function SpaceDetail({
  space,
  detail,
  fleet,
  teleport,
  fileSend,
  windowDrag,
  copyText,
}: {
  space: Space;
  detail: SpaceDetailView;
  fleet: FleetBridge;
  teleport: TeleportBridge;
  fileSend: FileSendBridge;
  windowDrag: WindowDragBridge;
  copyText?: WriteText;
}) {
  const [shot, setShot] = useState<string | null>(null);
  const alive = useRef(true);
  const reachable = Boolean(space.sdk?.reachable);

  useEffect(() => {
    alive.current = true;
    if (!space.sdk || !reachable) return;
    const grab = () =>
      fleet
        .screenshot(space.id, 1280)
        .then((url) => alive.current && setShot(url))
        .catch(() => {});
    void grab();
    const timer = window.setInterval(grab, SCREENSHOT_MS);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [fleet, space.id, space.sdk, reachable]);

  const section = (title: string) => {
    switch (title) {
      case 'Stream':
        return (
          <div className="dw-card">
            <SpaceWindowList
              space={space}
              teleport={teleport}
              onPipDesktop={() => void fleet.pinSpacePip(viewerRequest(space))}
              onClosePipDesktop={() => void fleet.unpinSpacePip(space.id).catch(() => {})}
              onPipWindow={(win, options) =>
                void teleport
                  .streamRemoteWindow(
                    space.id,
                    space.name,
                    win.id,
                    win.appName,
                    win.title,
                    options.replica
                  )
                  .catch(() => {})
              }
            />
          </div>
        );
      case 'Agents':
        return (
          <div className="dw-card">
            <SpaceAgentList spaceId={space.id} teleport={teleport} />
          </div>
        );
      case 'Teleport':
        return (
          <TeleportDropZone
            spaceId={space.id}
            spaceName={space.name}
            fileSend={fileSend}
            windowDrag={windowDrag}
            onTeleportApp={() => void fleet.openTeleportPicker(viewerRequest(space))}
          />
        );
      default:
        return null;
    }
  };

  return (
    <div className="dw-content-inner">
      <div className={shot ? 'dw-preview dw-preview-live' : 'dw-preview'}>
        {shot ? (
          <img src={shot} alt={`${space.name} desktop`} />
        ) : space.sdk ? (
          <div className="dw-preview-empty">
            <Sym name="desktopcomputer" />
            <span>{detail.previewText}</span>
          </div>
        ) : detail.creditNotice ? (
          <div className="dw-preview-empty" role="alert">
            <span>{detail.creditNotice.text}</span>
            <button
              type="button"
              className="dw-btn dw-btn-primary"
              onClick={() => void fleet.openExternal(detail.creditNotice!.url).catch(() => {})}
            >
              {detail.creditNotice.button}
            </button>
          </div>
        ) : space.progress?.error ? (
          <div className="dw-preview-empty">
            <span>{detail.previewText}</span>
          </div>
        ) : (
          <Thumbnail
            scene={space.scene}
            status={space.status}
            statusText={space.progress?.label ?? space.detail}
            progressText={detail.progressText ?? undefined}
            fraction={space.progress ? space.progress.permille / 1000 : undefined}
          />
        )}
      </div>

      {detail.powerError && (
        // Turning it off or on failed: why, inline.
        <p className="dw-inline-error" role="alert">
          {detail.powerError}
        </p>
      )}

      <FactList facts={detail.facts} copyText={copyText} />

      {detail.sections.map((title) => (
        <section className="dw-section" aria-label={title} key={title}>
          <div className="dw-section-head">
            <h2>{title}</h2>
          </div>
          {section(title)}
        </section>
      ))}
    </div>
  );
}
