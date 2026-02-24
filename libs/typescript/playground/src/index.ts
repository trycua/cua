// =============================================================================
// @trycua/playground
// =============================================================================
// Reusable playground UI for Cua computer-use agents.
// Supports both local (localStorage + user-provided URLs) and cloud (CUA API) usage.

// Types
export * from './types';
export * from './adapters/types';

// Adapters
export { createLocalAdapter } from './adapters/local';
export { createCloudAdapter } from './adapters/cloud';

// Utilities
export * from './utils';

// Context & Providers
export {
  PlaygroundContext,
  ChatStateContext,
  ChatDispatchContext,
  type PlaygroundContextValue,
  type ChatContextValue,
  type PlaygroundState,
  type ChatState,
  type PlaygroundAction,
  type ChatAction,
} from './context/PlaygroundContext';
export { PlaygroundProvider } from './context/PlaygroundProvider';
export { ChatProvider } from './context/ChatProvider';

// Hooks
export {
  usePlayground,
  useChat,
  useChatDispatch,
  useActiveChat,
  useIsChatGenerating,
  useLastRequestDuration,
  useRequestStartTime,
  useFindDefaultModel,
} from './hooks/usePlayground';
export { useAgentRequest } from './hooks/useAgentRequest';
export { useDarkMode } from './hooks/useDarkMode';

// Primitive Components
export {
  ChatMessage,
  getActionDescription,
  ChatInput,
  ToolCallsGroup,
  VNCViewer,
  ThinkingIndicator,
  ThinkingCompleteAnimation,
  ScrambledText,
  CountdownTimer,
  DeferredChatsLoader,
  VMStatusBanner,
  VNCOverlayPanel,
} from './components/primitives';
export type {
  VNCViewerProps,
  VMStatusBannerProps,
  VMVersionInfo,
  VNCOverlayPanelProps,
} from './components/primitives';
export { default as VNCIframe } from './components/primitives/VNCIframe';

// Composed Components
export { ChatPanel } from './components/composed/ChatPanel';
export { ChatList } from './components/composed/ChatList';
export { ChatArea } from './components/composed/ChatArea';
export { ChatContent } from './components/composed/ChatContent';
export { ChatSidebar } from './components/composed/ChatSidebar';
export { ChatSidebarSkeleton } from './components/composed/ChatSidebarSkeleton';
export { ComputerList } from './components/composed/ComputerList';
export { EmptyStateWithInput } from './components/composed/EmptyState';
export { ExamplePrompts, EXAMPLE_PROMPTS } from './components/composed/ExamplePrompts';
export type { ExamplePrompt } from './components/composed/ExamplePrompts';
export { PlaygroundLayout } from './components/composed/PlaygroundLayout';
export { PlaygroundContent } from './components/composed/PlaygroundContent';

// Modals
export { SettingsModal } from './components/modals/SettingsModal';
export { AddSandboxModal, CustomComputerModal } from './components/modals/CustomComputerModal';
export type { AddSandboxModalProps } from './components/modals/CustomComputerModal';
export { ExportTrajectoryModal } from './components/modals';
export { ReplayTrajectoryModal } from './components/modals';

// Constants
export { SANDBOX_PRESETS, MACOS_PRESET } from './constants/sandboxPresets';
export type { SandboxPreset } from './constants/sandboxPresets';

// Trajectory Viewer
export { default as TrajectoryViewer } from './components/TrajectoryViewer';

// Telemetry
export { TelemetryProvider, usePlaygroundTelemetry } from './telemetry';
export type { TelemetryProviderProps, TelemetryFunctions } from './telemetry';

// Main Playground Component
export { Playground } from './components/Playground';

// UI Components (for advanced customization)
export * from './components/ui';
