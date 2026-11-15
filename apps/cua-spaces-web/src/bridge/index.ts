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
  useFeatureRecorded,
  useFeatureRecorder,
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

export { useAgentDetail, type AgentDetailHook } from "./agent-detail";
export type { BridgeMode, DataAdapter } from "./adapter";
export { HostError, UnsupportedOperationError, isNativeHost, isUnsupported } from "./adapter";
export { createAdapter, createDemoAdapter, createElectronAdapter, createTauriAdapter, createWebkitAdapter } from "./adapters";
export { detectMode } from "./detect";
export { loadCore, type CoreClient, type CoreParity, type CoreStatus, type ParityFlow } from "./core";
export type { Machine, HostSetupGuide, KeyvaultAppGroup, KeyvaultViews } from "./derive";
export { harnessName, listView, machineIdOf, sharesDesktop } from "./derive";
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
export { newPendingId } from "./store";
export type { TranscriptItem, TranscriptStep } from "./transcript";
export { authorView, exportCardView } from "./thread-cards";
export type { AuthorView, CardCommitRow, CardFileRow, ExportCardView, Square } from "./thread-cards";
export {
  AUTOMATION_PANE,
  FULL_DISK_ACCESS_PANE,
  GRID_COLUMNS,
  iconKey,
  setupBlocked,
  setupProfile,
  thumbnailKey,
  useTeleport,
  type TeleportHook,
  type TeleportSession,
  type TeleportSetup,
  type TeleportStore,
} from "./teleport";
export { useShareSheet, type ShareHook, type ShareSession } from "./share";
export { useShareRemovalNote, type ShareRemovalNoteHook } from "./removal-note";
export {
  agentKeyForm,
  agentKeysView,
  useAgentKeys,
  useKeysRecheck,
  type AgentKeyFormView,
  type AgentKeyRowView,
  type AgentKeysHook,
  type AgentKeysInput,
  type AgentKeysView,
} from "./agent-keys";
export type { AgentKeyInfo, AgentKeyProvider, AgentKeysReport, KeyChoices } from "./ops/agent-keys";
export {
  connectionRows,
  useConnections,
  type ConnectionsHook,
  type ConnectionReport,
  type ConnectionRow,
  type ConnectionRowView,
  type ConnectResult,
} from "./connections";
export {
  DEFAULT_SETTINGS,
  OPERATIONS,
  HOST_EVENTS,
  type AppearanceTheme,
  type HostEvent,
  type HostOperations,
  type AppTarget,
  type CliInfo,
  type GlobalShortcut,
  type HostSettings,
  type OpName,
  type SettingKey,
  type SettingsValues,
  type AgentMenuPrefs,
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
export { hasNotch, passedSteps, signInCodeText, type SavedOnboarding } from "./onboarding";
export { signInFailedFromError, signInFailedPhase, signInFooter, type FailedPhase, type SignInFooterInput } from "./sign-in";
export {
  useCloudActions,
  useConnectCloud,
  useFirstPrompts,
  useFirstSpaceOffers,
  useNewSpaceRequests,
  useNewSpaceWizard,
  useRuntimeFix,
  type CloudActionsHook,
  type ConnectCloudHook,
  type FirstSpaceHook,
  type NewSpaceWizardHook,
  type RuntimeFixHook,
} from "./new-space-hooks";
export { runCreateFix, type CreateFixActions } from "./create-retry";
export {
  createFailedText,
  emptyHome,
  type EmptyHome,
  type EmptyTile,
  type FirstPrompts,
  type FirstSpaceOffer,
  type PromptCap,
  type PromptExample,
  type PromptSuggestion,
} from "./new-space";
export type * from "./contracts/volume";
export * from "./volume";
export type { GithubAccountRow, GithubDetail, GithubRow, GithubSourceLine, GithubTone, GithubView } from "./contracts/github";
export { githubView, useGithub, type GithubHook } from "./github";
export type { GithubReport, GithubHost, GithubAccount, GithubSource } from "./ops/github";
export type * from "./contracts/ci-runners";
export * from "./ci-runners";
export type * from "./contracts/prepare";
export * from "./prepare-core";
export { ThumbnailStore, THUMBNAIL_CAP, THUMBNAIL_REFRESH_MS, thumbnailStore, useSpaceThumbnail } from "./thumbnails";
export { useWindowDropTarget } from "./window-drop";
export { useSpaceFiles, parseDropped, dropSendingText, dropSentText, sendFiles, splitDrop, type DropStatus, type SpaceFilesHook } from "./space-files";
export {
  agentRunLines,
  useDesktopCover,
  useSpaceDetail,
  type AgentRunLine,
  type DetailFor,
  type DetailReadings,
  type PipOutcome,
  type SpaceDetailHook,
} from "./space-detail";
export type * from "./ops/space-detail";
export { isNeedsProof, useThisMachine, type HostFailure, type HostFormView as HostSetupFormView, type ThisMachineHook } from "./this-machine";
export type { HostFormAction, HostSetupRequest } from "./ops/host-setup";
export type { TelemetrySignal } from "./ops/telemetry";
export { useStartup, type StartupHook } from "./startup";
export { useLinkNotice, TURN_OFF_FAILED, type LinkNoticeHook } from "./link-notice";
export type { TelemetryLinkNotice } from "./ops/telemetry";
export { defaultTeam, eligibleTeams, teamName, useTeamChoices, type TeamChoices } from "./teams";
export { joinFailureText, useJoinWithCode, type JoinHook, type JoinPhase } from "./join";
export { displayPairingCode, normalizePairingCode, pairingLinkRelay } from "./pairing-code";
export {
  BUY_PRO_URL,
  BUY_TEAMS_URL,
  expiresInText,
  FINISHING_SETUP,
  PAIRING_POLL_MS,
  pairingUnsupported,
  planFailure,
  usePairingCode,
  type PairingHook,
  type PairingPhase,
} from "./pairing";
export { READY_STARTUP, type StartupAction, type StartupPhase, type StartupState } from "./ops/startup";
export { NO_TIPS, sendTipsEvent, tipsState, useTips, type TipsHook } from "./tips";
export { useNotchPins, type NotchPins } from "./notch-pins";
export { CHECKLIST_STEPS, useChecklist, type ChecklistHook, type ChecklistRow, type ChecklistState, type ChecklistStep, type ChecklistView } from "./checklist";
export {
  useRelayUsage,
  useRelayUsageView,
  usePlanLimitNotice,
  RELAY_USAGE_FOCUS_MS,
  RELAY_USAGE_POLL_MS,
  type PlanCard,
  type PlanLink,
  type RelayAllowanceNotice,
  type RelayUsage,
  type RelayUsageView,
} from "./relay-usage";
export { environmentTag, useEnvironmentTag, type EnvironmentTag } from "./environment-tag";
export {
  resolveDefaultSpace,
  useSpacesDefault,
  type DefaultCandidate,
  type DefaultSpace,
  type DefaultSpaceReason,
  type DefaultSpaceStep,
  type SpacesDefaultHook,
  type SpacesSettings,
  type SpacesSettingsInput,
} from "./spaces-settings";

export { dataWords, linkEnds, needsRelaySetup, useLinks, useShareSettings, type LinksHook, type ShareSettingsHook } from "./links";
export {
  pointerChange,
  pointerSettings,
  sinceText,
  useDriver,
  useDriverMachines,
  DRIVER_POLL_MS,
  POINTER_STYLES,
  type DriverActivity,
  type DriverApp,
  type DriverConfig,
  type DriverHook,
  type DriverLiveTest,
  type DriverMachine,
  type DriverPermission,
  type DriverStatus,
  type DriverUpdate,
  type PointerSettings,
} from "./driver";
export {
  chatKey,
  mergeChats,
  useAgentChats,
  useAgentDm,
  type AgentChat,
  type AgentChats,
  type AgentChatSpace,
  type AgentDm,
  type ChatAttempt,
  type ChatNeed,
  type ChatTone,
  type DmItem,
} from "./agents-dm";
export {
  foldThread,
  readSeqFrom,
  useSpaceThread,
  type SpaceThread,
  type SpaceThreadStatus,
  type ThreadAnswer,
  type ThreadItem,
  type ThreadQuote,
} from "./thread";
export {
  DEFAULT_AGENT_RIGHTS,
  isMissingThreadTool,
  threadOpAvailable,
  type AgentMember,
  type JoinCode,
  type JoinCodeRow,
  type JoinCodeState,
  type JoinRequest,
  type JoinMethod,
  type ChildNote,
  type LiveAgent,
  type LiveChild,
  type MemberEvent,
  type MemberIdentity,
  type MemberRef,
  type NeedsYou,
  type AgentRight,
  type AgentRights,
  type RightHolders,
  type TaskState,
  type ThreadAttachment,
  type ThreadAuthor,
  type ThreadEntry,
  type ThreadFailure,
  type ThreadView,
  type ThreadPage,
  type ThreadRole,
} from "./ops/thread";
export { computerSignInAgentOf, type ComputerSignInAgent, type ComputerSignInState } from "./ops/agent-sign-in";
export {
  agentOf as threadAgentOf,
  folderLabel,
  onlyAgents,
  resumeContext,
  THREAD_AGENTS,
  type PastMessage,
  type PastThread,
  type RecentThread,
  type ThreadAgent,
} from "./ops/agent-threads";
export type { LinkAccess, LinkCreate, ShareLanding, ShareLink, ShareSettings } from "./ops/links";
export {
  useResources,
  hostResources,
  spaceDiskBytes,
  spaceResources,
  RESOURCES_HIDDEN_MS,
  RESOURCES_VISIBLE_MS,
  type ComputerResources,
  type DiskOnHost,
  type DiskWarning,
  type GuestUsage,
  type ResourcesSnapshot,
  type SharedBase,
  type SpaceResources,
} from "./resources";
export {
  resetUpsellMemory,
  upgradeLink,
  usePlanLimitTelemetry,
  useQuiet,
  useTrackPlanClick,
  useUpsellCard,
  useUpsellCardAvailable,
  useUpsellGate,
  useUpsellPrompt,
  type PlanFacts,
  type PlanLimitSurface,
  type PlanLimitTelemetry,
  type UpsellCard,
  type UpsellCardReason,
  type UpsellCardState,
  type UpsellGate,
  type UpsellPrompt,
  type UpsellSurface,
} from "./upsell";
// Clears the overrides of retired flags once, on load.
import "./flags";
export type { CreateReport, ReportPart, ReportSend } from "./diagnostic-report";
export { createReport, reportMailto } from "./diagnostic-report";
export {
  TEAMMATES_KEY,
  UPSELL_DISMISSED_KEY,
  firstRunCliView,
  cliMissing,
  cliPresent,
  useCliInstall,
  type CliInstallHook,
  noteTeammates,
  readFirstRun,
  resetFirstRunCache,
  useFirstRunSaved,
  writeFirstRun,
  type FirstRunSaved,
} from "./first-run";
export type * from "./contracts/first-run";
// The new Spaces page's decisions (CUA-2204): menus, bulk delete and leave,
// tips, the Pro banner and the account menu.
export { accountMenu, bulkConfirm, bulkDone, bulkProgress, chooseSettings, proBanner, spaceMenu, tipsReduce, tipsView } from "./spaces-page";
export type * from "./contracts/spaces-page";
export * from "./connectors";
export * from "./profile-card";
