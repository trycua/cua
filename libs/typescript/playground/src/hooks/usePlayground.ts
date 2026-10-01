// Hook to access playground context
// Provides access to adapters, state, and dispatch

import { useContext, type Dispatch } from 'react';
import {
  PlaygroundContext,
  ChatStateContext,
  ChatDispatchContext,
  type PlaygroundContextValue,
  type ChatState,
  type ChatAction,
} from '../context/PlaygroundContext';

/**
 * Hook to access the playground context.
 * Must be used within a PlaygroundProvider.
 *
 * @returns PlaygroundContextValue with adapters, state, and dispatch
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const { adapters, state, dispatch } = usePlayground();
 *   // ...
 * }
 * ```
 */
export function usePlayground(): PlaygroundContextValue {
  const context = useContext(PlaygroundContext);
  if (!context) {
    throw new Error('usePlayground must be used within a PlaygroundProvider');
  }
  return context;
}

/**
 * Hook to access the chat state.
 * Must be used within a ChatProvider.
 */
export function useChat(): ChatState {
  const context = useContext(ChatStateContext);
  if (!context) {
    throw new Error('useChat must be used within a ChatProvider');
  }
  return context;
}

/**
 * Hook to access the chat dispatch.
 * Must be used within a ChatProvider.
 */
export function useChatDispatch(): Dispatch<ChatAction> {
  const context = useContext(ChatDispatchContext);
  if (!context) {
    throw new Error('useChatDispatch must be used within a ChatProvider');
  }
  return context;
}

/**
 * Hook to get the active chat from state.
 */
export function useActiveChat() {
  const { state } = usePlayground();
  if (!state.activeChatId) return null;
  return state.chats.find((c) => c.id === state.activeChatId) ?? null;
}

/**
 * Hook to check if a chat is currently generating.
 */
export function useIsChatGenerating(chatId: string | null): boolean {
  const { state } = usePlayground();
  if (!chatId) return false;
  return state.generatingChatIds.has(chatId);
}

/**
 * Hook to get the last request duration for a chat.
 */
export function useLastRequestDuration(chatId: string | null): number | null {
  const { state } = usePlayground();
  if (!chatId) return null;
  return state.lastRequestDurations.get(chatId) ?? null;
}

/**
 * Hook to get the request start time for a chat.
 */
export function useRequestStartTime(chatId: string | null): number | null {
  const { state } = usePlayground();
  if (!chatId) return null;
  return state.requestStartTimes.get(chatId) ?? null;
}

/**
 * Hook to find the default model from available models.
 */
export function useFindDefaultModel() {
  const { state } = usePlayground();
  const { availableModels, selectedModel } = state;

  // Try selected model first
  if (selectedModel) {
    for (const provider of availableModels) {
      const model = provider.models.find((m) => m.id === selectedModel);
      if (model) return model;
    }
  }

  // Fall back to first available model
  if (availableModels.length > 0 && availableModels[0].models.length > 0) {
    return availableModels[0].models[0];
  }

  return undefined;
}
