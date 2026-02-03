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
} from './components/primitives';
export type { VNCViewerProps } from './components/primitives';

// Composed Components
export { ChatPanel } from './components/composed/ChatPanel';
export { ChatList } from './components/composed/ChatList';
export { ComputerList } from './components/composed/ComputerList';

// Modals
export { SettingsModal } from './components/modals/SettingsModal';
export { CustomComputerModal } from './components/modals/CustomComputerModal';

// Main Playground Component
export { Playground } from './components/Playground';
