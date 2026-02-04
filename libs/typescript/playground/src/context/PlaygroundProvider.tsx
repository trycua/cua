// Provider for playground state management
// Adapted from cloud/src/website/app/providers/PlaygroundProvider.tsx

import { useReducer, useEffect, useMemo, type ReactNode } from 'react';
import {
  PlaygroundContext,
  type PlaygroundState,
  type PlaygroundAction,
} from './PlaygroundContext';
import type { PlaygroundAdapters } from '../adapters/types';
import type { Chat } from '../types';

// =============================================================================
// Initial State
// =============================================================================

const initialState: PlaygroundState = {
  chats: [],
  activeChatId: null,
  computers: [],
  currentComputerId: null,
  availableModels: [],
  selectedModel: null,
  isLoading: true,
  error: null,
  generatingChatIds: new Set(),
  requestStartTimes: new Map(),
  lastRequestDurations: new Map(),
  initialized: false,
  showSettings: false,
  isSidebarCollapsed: false,
};

// =============================================================================
// Reducer
// =============================================================================

function playgroundReducer(state: PlaygroundState, action: PlaygroundAction): PlaygroundState {
  switch (action.type) {
    case 'SET_CHATS':
      return { ...state, chats: action.payload };

    case 'ADD_CHAT':
      return { ...state, chats: [...state.chats, action.payload] };

    case 'UPDATE_CHAT': {
      const updatedChats = state.chats.map((chat) =>
        chat.id === action.payload.id ? { ...chat, ...action.payload.updates } : chat
      );
      return { ...state, chats: updatedChats };
    }

    case 'DELETE_CHAT': {
      const filteredChats = state.chats.filter((c) => c.id !== action.payload);
      const newActiveChatId =
        state.activeChatId === action.payload
          ? filteredChats.length > 0
            ? [...filteredChats].sort((a, b) => b.updated.getTime() - a.updated.getTime())[0].id
            : null
          : state.activeChatId;
      return {
        ...state,
        chats: filteredChats,
        activeChatId: newActiveChatId,
      };
    }

    case 'SET_ACTIVE_CHAT_ID':
      return { ...state, activeChatId: action.payload };

    case 'SET_CHAT_MESSAGES': {
      const updatedChats = state.chats.map((chat) =>
        chat.id === action.payload.id ? { ...chat, messages: action.payload.messages } : chat
      );
      return { ...state, chats: updatedChats };
    }

    case 'SET_COMPUTERS':
      return { ...state, computers: action.payload };

    case 'SET_CURRENT_COMPUTER':
      return { ...state, currentComputerId: action.payload };

    case 'SET_MODELS':
      return { ...state, availableModels: action.payload };

    case 'SET_SELECTED_MODEL':
      return { ...state, selectedModel: action.payload };

    case 'SET_LOADING':
      return { ...state, isLoading: action.payload };

    case 'SET_ERROR':
      return { ...state, error: action.payload };

    case 'SET_CHAT_GENERATING': {
      const newGeneratingChatIds = new Set(state.generatingChatIds);
      const newRequestStartTimes = new Map(state.requestStartTimes);
      const newLastRequestDurations = new Map(state.lastRequestDurations);

      if (action.payload.generating) {
        newGeneratingChatIds.add(action.payload.chatId);
        newRequestStartTimes.set(action.payload.chatId, Date.now());
      } else {
        newGeneratingChatIds.delete(action.payload.chatId);
        const startTime = newRequestStartTimes.get(action.payload.chatId);
        if (startTime) {
          const durationSeconds = (Date.now() - startTime) / 1000;
          newLastRequestDurations.set(action.payload.chatId, durationSeconds);
        }
        newRequestStartTimes.delete(action.payload.chatId);
      }

      return {
        ...state,
        generatingChatIds: newGeneratingChatIds,
        requestStartTimes: newRequestStartTimes,
        lastRequestDurations: newLastRequestDurations,
      };
    }

    case 'RESET_REQUEST_START_TIME': {
      const newRequestStartTimes = new Map(state.requestStartTimes);
      newRequestStartTimes.set(action.payload.chatId, Date.now());
      return {
        ...state,
        requestStartTimes: newRequestStartTimes,
      };
    }

    case 'SET_INITIALIZED':
      return { ...state, initialized: action.payload };

    case 'SET_SHOW_SETTINGS':
      return { ...state, showSettings: action.payload };

    case 'TOGGLE_SIDEBAR':
      return { ...state, isSidebarCollapsed: !state.isSidebarCollapsed };

    default:
      return state;
  }
}

// =============================================================================
// Provider Props
// =============================================================================

interface PlaygroundProviderProps {
  adapters: PlaygroundAdapters;
  children: ReactNode;
  /** Optional initial chats (for SSR or pre-loading) */
  initialChats?: Chat[];
}

// =============================================================================
// Provider Component
// =============================================================================

export function PlaygroundProvider({ adapters, children, initialChats }: PlaygroundProviderProps) {
  const [state, dispatch] = useReducer(playgroundReducer, {
    ...initialState,
    chats: initialChats || [],
  });

  // Initialize from adapters
  useEffect(() => {
    async function initialize() {
      try {
        const [chats, computers, models] = await Promise.all([
          initialChats ? Promise.resolve(initialChats) : adapters.persistence.loadChats(),
          adapters.computer.listComputers(),
          adapters.inference.getAvailableModels(),
        ]);

        console.log('[PlaygroundProvider] Initialized:', {
          chats: chats.length,
          computers: computers.length,
          models: models.length,
          computersData: computers,
        });

        dispatch({ type: 'SET_CHATS', payload: chats });
        dispatch({ type: 'SET_COMPUTERS', payload: computers });
        dispatch({ type: 'SET_MODELS', payload: models });

        // Set defaults
        if (computers.length > 0) {
          dispatch({ type: 'SET_CURRENT_COMPUTER', payload: computers[0].id });
        }
        if (models.length > 0 && models[0].models.length > 0) {
          dispatch({ type: 'SET_SELECTED_MODEL', payload: models[0].models[0].id });
        }

        dispatch({ type: 'SET_LOADING', payload: false });
        dispatch({ type: 'SET_INITIALIZED', payload: true });
      } catch (error) {
        dispatch({
          type: 'SET_ERROR',
          payload: error instanceof Error ? error.message : 'Failed to initialize',
        });
        dispatch({ type: 'SET_LOADING', payload: false });
      }
    }

    initialize();
  }, [adapters, initialChats]);

  // Memoize context value to prevent unnecessary re-renders
  const contextValue = useMemo(
    () => ({
      adapters,
      state,
      dispatch,
    }),
    [adapters, state]
  );

  return <PlaygroundContext.Provider value={contextValue}>{children}</PlaygroundContext.Provider>;
}
