// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core's parity flows (libs/cua/crates/cua-spaces-app-core/parity),
 * replayed through this app's model layer: every core call a flow makes
 * goes through the same `src/model/*.ts` and `src/state/*.ts` functions the
 * components use, over the wasm core. The transcripts must equal the
 * goldens the Rust core, the Tauri shell and the SwiftUI app also match.
 */
import { describe, expect, it } from 'vitest';

import { core } from '../core';
import { flows, runFlow } from '../core/wasm/core.js';
import { initialState, reduce } from '../state/portal';
import {
  approvalView,
  kvCredentialForm,
  kvPage,
  kvPassphraseCheck,
  openApproval,
  reduceApproval,
} from './keyvault';
import {
  composeCreates,
  isDeleting,
  isPendingCreate,
  reduceCreates,
  settleCreates,
} from './creates';
import { applyDragOverlay } from './dragOverlay';
import { driverPreview, driverPreviewFrame, driverPreviewStill } from './driverPreview';
import { drivePreview, drivePreviewFrame, drivePreviewStill } from './driveMountPreview';
import {
  reduceStorage,
  storageChoose,
  storageEdit,
  storageInitial,
  storagePress,
  storageRequestText,
  storageSection,
} from './driveSettings';
import { hostFormInitial, hostFormView, reduceHostForm } from './host';
import {
  initialOnboarding,
  installedAtText,
  onboardingCopy,
  onboardingView,
  reduceOnboarding,
  replacesText,
  signedInText,
  signInCodeText,
} from './onboarding';
import {
  deleteFailedText,
  detailCopy,
  mainChrome,
  openableCount,
  pipClick,
  pipReduce,
  settingsPage,
  sidebar,
  spaceDetail,
  streamSection,
  trayMenu,
} from './window';
import { rowToSpace, type SpaceRow } from './spaces';
import { launchPlan } from './loginItem';
import { reduceShare, shareInitial, shareView } from './share';
import { cloudConnectInitial, cloudConnectView, reduceCloudConnect } from './cloudConnect';
import { chooseExperiment, experimentsChangedSignals, experimentsPage, settingsWithStorage } from './experiments';
import {
  createsSignals,
  enrollSignals,
  onboardingFinishedSignals,
  onboardingSignals,
  shareSignals,
  storageSignals,
} from './telemetry';
import {
  agentsInitial,
  agentsView,
  driveInitial,
  driveView,
  notificationsPlan,
  notificationsView,
  reduceAgents,
  reduceDrive,
} from './persistent';
import {
  appGrid,
  canPlan,
  gridPrimary,
  gridStep,
  gridTabs,
  initialPicker,
  planSensitive,
  progress,
  reducePicker,
  remoteGrid,
  sections,
  sensitiveOptions,
  windowGrid,
} from './teleportFlow';

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** Core methods as the app's model layer answers them. */
const MODEL: Record<string, (a: Args) => unknown> = {
  'spaces.rowsToSpaces': (a) => (a.rows as SpaceRow[]).map((r) => rowToSpace(r, a.now)),
  'roster.initial': (a) => {
    const { fleets: _f, ...s } = initialState(a.spaces);
    return s;
  },
  'roster.reduce': (a) => {
    const { fleets: _f, ...s } = reduce({ ...a.state, fleets: [] }, a.action);
    return s;
  },
  'creates.reduce': (a) => reduceCreates(a.state, a.action),
  'creates.compose': (a) => composeCreates(a.spaces, a.state),
  'creates.isPending': (a) => isPendingCreate(a.id),
  'creates.settle': (a) => settleCreates(a.state, a.spaces),
  'creates.isDeleting': (a) => isDeleting(a.state, a.id),
  'flow.initial': (a) => initialPicker(a.spaceName),
  'flow.reduce': (a) => reducePicker(a.state, a.event),
  'flow.sections': (a) => sections(a.state),
  'flow.canPlan': (a) => canPlan(a.state),
  'flow.progress': (a) => progress(a.state),
  'flow.sensitiveOptions': (a) => sensitiveOptions(a.state),
  'flow.planSensitive': (a) => planSensitive(a.state),
  'drag.apply': (a) => applyDragOverlay(a.state, a.event),
  'approval.open': (a) => openApproval(a.requestId),
  'approval.reduce': (a) => reduceApproval(a.overview, a.state, a.action),
  'approval.view': (a) => approvalView(a.overview, a.state),
  'keyvault.page': (a) => kvPage(a.overview, a.now),
  'keyvault.credentialForm': (a) => kvCredentialForm(a.overview),
  'keyvault.passphraseCheck': (a) => kvPassphraseCheck(a.mode, a.passphrase, a.confirm),
  'sidebar.build': (a) => sidebar(a.spaces, a.query ?? '', a.selectedId ?? null),
  'sidebar.detail': (a) => spaceDetail(a.space, a.usage, a.hostArch, a.experiments),
  'experiments.page': (a) => experimentsPage(a.experiments),
  'experiments.choose': (a) => chooseExperiment(a.experiments, a.row, a.option),
  'settings.withStorage': (a) => settingsWithStorage(a.page, a.storage, a.experiments),
  'telemetry.experimentsChanged': (a) => experimentsChangedSignals(a.before, a.after),
  'sidebar.detailCopy': () => detailCopy(),
  'sidebar.deleteFailedText': (a) => deleteFailedText(a.name, a.error),
  'sidebar.streamSection': (a) => streamSection(a.input),
  'stream.pipReduce': (a) => pipReduce(a.open, a.event),
  'stream.pipClick': (a) => pipClick(a.open, a.row),
  'grid.tabs': (a) => gridTabs(a.spaceName),
  'grid.apps': (a) => appGrid(a.state, a.windows),
  'grid.windows': (a) => windowGrid(a.windows, a.query, a.selected),
  'grid.remote': (a) => remoteGrid(a.windows, a.query, a.selected),
  'grid.primary': (a) => gridPrimary(a.tab, a.spaceName, a.grid),
  'grid.step': (a) => gridStep(a.grid, a.selected, a.delta),
  'window.chrome': (a) => mainChrome(a.input),
  'window.menu': (a) => trayMenu(a.input),
  'spaces.openableCount': (a) => openableCount(a.spaces),
  'settings.page': (a) => settingsPage(a.input),
  'loginItem.launchPlan': (a) => launchPlan(a.choice, a.onboarded, a.serves, a.status),
  'host.formInitial': () => hostFormInitial(),
  'host.formReduce': (a) => reduceHostForm(a.state, a.action),
  'host.formView': (a) => hostFormView(a.state, a.identity),
  'onboarding.initial': (a) => initialOnboarding(a.installerMode, a.identity),
  'onboarding.reduce': (a) => reduceOnboarding(a.state, a.action),
  'onboarding.view': (a) => onboardingView(a.state),
  'onboarding.copy': () => onboardingCopy(),
  'onboarding.signedInText': (a) => signedInText(a.identity),
  'onboarding.signInCodeText': (a) => signInCodeText(a.userCode),
  'onboarding.replacesText': (a) => replacesText(a.installedVersion),
  'onboarding.installedAtText': (a) => installedAtText(a.target),
  'onboarding.driverPreview': () => driverPreview(),
  'onboarding.driverPreviewFrame': (a) => driverPreviewFrame(a.tMs),
  'onboarding.driverPreviewStill': () => driverPreviewStill(),
  'onboarding.drivePreview': () => drivePreview(),
  'onboarding.drivePreviewFrame': (a) => drivePreviewFrame(a.tMs),
  'onboarding.drivePreviewStill': () => drivePreviewStill(),
  'cloudConnect.initial': () => cloudConnectInitial(),
  'cloudConnect.reduce': (a) => reduceCloudConnect(a.input, a.state, a.action),
  'cloudConnect.view': (a) => cloudConnectView(a.input, a.state),
  'share.initial': () => shareInitial(),
  'share.reduce': (a) => reduceShare(a.input, a.state, a.action),
  'share.view': (a) => shareView(a.input, a.state),
  'agents.pageInitial': () => agentsInitial(),
  'agents.pageReduce': (a) => reduceAgents(a.input, a.state, a.action),
  'agents.pageView': (a) => agentsView(a.input, a.state, a.nowMs),
  'drive.initial': () => driveInitial(),
  'drive.reduce': (a) => reduceDrive(a.state, a.action),
  'drive.view': (a) => driveView(a.input, a.state),
  'storage.initial': () => storageInitial(),
  'storage.reduce': (a) => reduceStorage(a.state, a.action),
  'storage.section': (a) => storageSection(a.input, a.state),
  'storage.press': (a) => storagePress(a.input, a.id),
  'storage.choose': (a) => storageChoose(a.id, a.option),
  'storage.edit': (a) => storageEdit(a.id, a.value),
  'storage.requestText': (a) => storageRequestText(a.request),
  'notifications.plan': (a) => notificationsPlan(a.feed, a.seenMs),
  'notifications.view': (a) => notificationsView(a.feed, a.nowMs),
  'telemetry.onboarding': (a) => onboardingSignals(a.state, a.action),
  'telemetry.onboardingFinished': (a) => onboardingFinishedSignals(a.state),
  'telemetry.creates': (a) => createsSignals(a.state, a.action, a.now),
  'telemetry.storage': (a) => storageSignals(a.input, a.state, a.action),
  'telemetry.share': (a) => shareSignals(a.input, a.state, a.action),
  'telemetry.enroll': (a) => enrollSignals(a.state, a.action),
};

function host(method: string, args: string): string {
  const a = JSON.parse(args) as Args;
  const viaModel = MODEL[method];
  return JSON.stringify(viaModel ? viaModel(a) : core(method, a));
}

describe('app core parity flows (webview)', () => {
  const all = JSON.parse(flows()) as { name: string; flow: string; golden: string }[];

  it('runs every flow', () => {
    expect(all.map((f) => f.name)).toEqual([
      'create-space',
      'create-resources',
      'create-gpu',
      'create-cancel',
      'teleport-review',
      'teleport-sign-ins',
      'keyvault-approve-deny',
      'keyvault-unlock',
      'main-window',
      'notch',
      'notch-drag-trigger',
      'provisioning',
      'stream-section',
      'space-facts',
      'picker-grid',
      'delete-space',
      'space-power',
      'create-progress',
      'devices',
      'driver-card',
      'share-sheet',
      'agents-page',
      'drive-page',
      'menu-count',
      'drive-onboarding',
      'drive-storage',
      'notifications',
      'about',
      'your-cloud',
      'telemetry-funnel',
      'launch-at-login',
      'experiments',
      'placement-picker',
    ]);
  });

  for (const f of all) {
    it(`${f.name} matches its golden transcript`, () => {
      const transcript = JSON.parse(runFlow(f.name, f.flow, host));
      expect(transcript).toEqual(JSON.parse(f.golden));
    });
  }
});
