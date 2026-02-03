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

// Primitive Components (added by Agent C)
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

// Composed Components (will be added by Agent D)
// export { Playground } from './components/Playground';
// export { ChatPanel } from './components/composed/ChatPanel';
// export { ChatList } from './components/composed/ChatList';
// export { ComputerList } from './components/composed/ComputerList';

// Hooks (will be added by Agent D)
// export { usePlayground } from './hooks/usePlayground';
// export { useAgentRequest } from './hooks/useAgentRequest';

// Context (will be added by Agent D)
// export { PlaygroundProvider } from './context/PlaygroundProvider';
// export { PlaygroundContext } from './context/PlaygroundContext';
