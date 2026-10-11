// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The bridge between the web UI and whichever host runs it (Tauri,
 * Electron, the SwiftUI WKWebView, or the in-memory demo), with the app
 * core loaded as wasm. Screens import from here only. See README.md.
 */

export { BridgeProvider, type BridgeProviderProps } from "./BridgeProvider";
export {
  useAgentTimeline,
  useAgents,
  useBridge,
  useKeyvault,
  useMachines,
  useOnboarding,
  useSavedOnboarding,
  useSession,
  useSettings,
  useSpaceAccess,
  useSpaces,
  type AgentsHook,
  type KeyvaultHook,
  type MachinesHook,
  type OnboardingHook,
  type SessionHook,
  type SettingsHook,
  type SpacesHook,
} from "./hooks";

export type { BridgeMode, DataAdapter } from "./adapter";
export { HostError, UnsupportedOperationError, isNativeHost, isUnsupported } from "./adapter";
export { createAdapter, createDemoAdapter, createElectronAdapter, createTauriAdapter, createWebkitAdapter } from "./adapters";
export { detectMode } from "./detect";
export { loadCore, type CoreClient, type CoreParity, type CoreStatus, type ParityFlow } from "./core";
export type { Machine, HostSetupGuide, KeyvaultAppGroup, KeyvaultViews } from "./derive";
export { machineIdOf } from "./derive";
export type {
  AgentRunSummary,
  AgentState,
  AgentSummary,
  AgentsData,
  AgentTimeline,
  CreateSpaceRequest,
  KeyvaultData,
  Resource,
  RunRef,
  SessionData,
  SettingsData,
} from "./store";
export type { TranscriptItem, TranscriptStep } from "./transcript";
export { GRID_COLUMNS, iconKey, thumbnailKey, useTeleport, type TeleportHook, type TeleportSession, type TeleportStore } from "./teleport";
export { useShareSheet, type ShareHook, type ShareSession } from "./share";
export {
  agentKeyForm,
  agentKeysView,
  useAgentKeys,
  type AgentKeyFormView,
  type AgentKeyRowView,
  type AgentKeysHook,
  type AgentKeysInput,
  type AgentKeysView,
} from "./agent-keys";
export type { AgentKeyInfo, AgentKeyProvider, AgentKeysReport } from "./ops/agent-keys";
export {
  DEFAULT_SETTINGS,
  OPERATIONS,
  HOST_EVENTS,
  type AppearanceTheme,
  type HostEvent,
  type HostOperations,
  type HostSettings,
  type OpName,
  type SettingKey,
  type SettingsValues,
  type UpdateChannel,
} from "./protocol";

export type { KvAccessCommand } from "./ops/keyvault-manage";
export type * from "./contracts/spaces";
export type * from "./contracts/host";
export type * from "./contracts/keyvault";
export type * from "./contracts/agents";
export type * from "./contracts/onboarding";
export type * from "./contracts/new-space";
export type * from "./contracts/teleport";
export type * from "./contracts/share";
export type * from "./contracts/settings";
export type * from "./contracts/devices";
export type * from "./contracts/notifications";
export * from "./settings-hooks";
export { NATIVE_ONLY_STEPS, signInCodeText, type SavedOnboarding } from "./onboarding";
export { useConnectCloud, useFirstSpaceOffers, useNewSpaceRequests, useNewSpaceWizard, type ConnectCloudHook, type FirstSpaceHook, type NewSpaceWizardHook } from "./new-space-hooks";
export { createFailedText, emptyHome, type EmptyHome, type EmptyTile, type FirstSpaceOffer } from "./new-space";
export type * from "./contracts/volume";
export * from "./volume";
export { ThumbnailStore, THUMBNAIL_CAP, THUMBNAIL_REFRESH_MS, thumbnailStore, useSpaceThumbnail } from "./thumbnails";
export { useSpaceFiles, parseDropped, dropSendingText, dropSentText, sendFiles, splitDrop, type DropStatus, type SpaceFilesHook } from "./space-files";
export { agentRunLines, useDesktopCover, useSpaceDetail, type AgentRunLine, type DetailFor, type DetailReadings, type PipOutcome, type SpaceDetailHook } from "./space-detail";
export type * from "./ops/space-detail";
export { useThisMachine, type HostFailure, type HostFormView as HostSetupFormView, type ThisMachineHook } from "./this-machine";
export type { HostFormAction, HostSetupRequest } from "./ops/host-setup";
export type { TelemetrySignal } from "./ops/telemetry";
export { useStartup, type StartupHook } from "./startup";
export { READY_STARTUP, type StartupAction, type StartupPhase, type StartupState } from "./ops/startup";
