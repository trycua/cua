// Context for playground state management
// Adapted from cloud/src/website/app/contexts/ChatContext.tsx and cloud/src/website/app/providers/PlaygroundProvider.tsx

import { createContext, type Dispatch } from 'react';
import type { PlaygroundAdapters, ComputerInfo } from '../adapters/types';
import type {
  Chat,
  Model,
  ModelProvider,
  AgentMessage,
  ChatError,
  ProcessedMessage,
  RetryState,
  Computer,
} from '../types';

// =============================================================================
// Playground State
// =============================================================================

/**
 * Global playground state.
 * Manages chats, computers, models, and UI state.
 */
export interface PlaygroundState {
  // Chats
  chats: Chat[];
  activeChatId: string | null;

  // Computers
  computers: ComputerInfo[];
  currentComputerId: string | null;

  // Models
  availableModels: ModelProvider[];
  selectedModel: string | null;

  // Loading & Error
  isLoading: boolean;
  error: string | null;

  // Generation state (tracks which chats have active agent requests)
  generatingChatIds: Set<string>;
  requestStartTimes: Map<string, number>;
  lastRequestDurations: Map<string, number>;

  // UI state
  initialized: boolean;
  showSettings: boolean;
  isSidebarCollapsed: boolean;
}

// =============================================================================
// Chat State (per-chat context)
// =============================================================================

/**
 * Per-chat state for the active chat.
 * Manages input, messages, and chat-specific UI state.
 */
export interface ChatState {
  id: string;
  name: string;
  messages: AgentMessage[];
  model?: Model;
  computer?: Computer;
  currentInput: string;
  waitingForResponse: boolean;
  chatError: ChatError | null;
  processedMessages: ProcessedMessage[];
  retryState: RetryState | null;
}

// =============================================================================
// Action Types
// =============================================================================

export type PlaygroundAction =
  // Chat actions
  | { type: 'SET_CHATS'; payload: Chat[] }
  | { type: 'ADD_CHAT'; payload: Chat }
  | { type: 'UPDATE_CHAT'; payload: { id: string; updates: Partial<Chat> } }
  | { type: 'DELETE_CHAT'; payload: string }
  | { type: 'SET_ACTIVE_CHAT_ID'; payload: string | null }
  | { type: 'SET_CHAT_MESSAGES'; payload: { id: string; messages: AgentMessage[] } }
  // Computer actions
  | { type: 'SET_COMPUTERS'; payload: ComputerInfo[] }
  | { type: 'SET_CURRENT_COMPUTER'; payload: string | null }
  // Model actions
  | { type: 'SET_MODELS'; payload: ModelProvider[] }
  | { type: 'SET_SELECTED_MODEL'; payload: string }
  // Loading & Error actions
  | { type: 'SET_LOADING'; payload: boolean }
  | { type: 'SET_ERROR'; payload: string | null }
  // Generation state actions
  | { type: 'SET_CHAT_GENERATING'; payload: { chatId: string; generating: boolean } }
  | { type: 'RESET_REQUEST_START_TIME'; payload: { chatId: string } }
  // UI state actions
  | { type: 'SET_INITIALIZED'; payload: boolean }
  | { type: 'SET_SHOW_SETTINGS'; payload: boolean }
  | { type: 'TOGGLE_SIDEBAR' };

export type ChatAction =
  | { type: 'SET_INPUT'; payload: string }
  | { type: 'SET_MODEL'; payload: Model }
  | { type: 'SET_COMPUTER'; payload: Computer }
  | { type: 'ADD_MESSAGE'; payload: AgentMessage }
  | { type: 'SET_MESSAGES'; payload: AgentMessage[] }
  | { type: 'APPEND_MESSAGES'; payload: AgentMessage[] }
  | { type: 'SET_WAITING'; payload: boolean }
  | { type: 'SET_ERROR'; payload: ChatError | null }
  | { type: 'SET_RETRY_STATE'; payload: RetryState | null }
  | { type: 'UPDATE_NAME'; payload: string }
  | { type: 'CLEAR_INPUT' }
  | { type: 'RESET_CHAT'; payload: Chat };

// =============================================================================
// Context Values
// =============================================================================

export interface PlaygroundContextValue {
  adapters: PlaygroundAdapters;
  state: PlaygroundState;
  dispatch: Dispatch<PlaygroundAction>;
}

export interface ChatContextValue {
  state: ChatState;
  dispatch: Dispatch<ChatAction>;
}

// =============================================================================
// Contexts
// =============================================================================

export const PlaygroundContext = createContext<PlaygroundContextValue | null>(null);
export const ChatStateContext = createContext<ChatState | null>(null);
export const ChatDispatchContext = createContext<Dispatch<ChatAction> | null>(null);
