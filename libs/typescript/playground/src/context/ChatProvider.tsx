// Per-chat context provider
// Adapted from cloud/src/website/app/contexts/ChatContext.tsx

import { useReducer, useEffect, useRef, type ReactNode } from 'react';
import {
  ChatStateContext,
  ChatDispatchContext,
  type ChatState,
  type ChatAction,
} from './PlaygroundContext';
import { usePlayground } from '../hooks/usePlayground';
import { processMessagesForRendering } from '../utils/messageProcessing';
import type { Chat } from '../types';

// =============================================================================
// Create Initial State
// =============================================================================

function createInitialState(chat: Chat): ChatState {
  return {
    id: chat.id,
    name: chat.name,
    messages: chat.messages || [],
    model: chat.model,
    computer: chat.computer,
    currentInput: '',
    waitingForResponse: false,
    chatError: null,
    processedMessages: processMessagesForRendering(chat.messages || []),
    retryState: null,
  };
}

// =============================================================================
// Reducer
// =============================================================================

function chatReducer(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case 'SET_INPUT':
      return { ...state, currentInput: action.payload };

    case 'SET_MODEL':
      return { ...state, model: action.payload };

    case 'SET_COMPUTER':
      return { ...state, computer: action.payload };

    case 'ADD_MESSAGE': {
      const newMessages = [...state.messages, action.payload];
      return {
        ...state,
        messages: newMessages,
        processedMessages: processMessagesForRendering(newMessages),
      };
    }

    case 'SET_MESSAGES': {
      return {
        ...state,
        messages: action.payload,
        processedMessages: processMessagesForRendering(action.payload),
      };
    }

    case 'APPEND_MESSAGES': {
      const newMessages = [...state.messages, ...action.payload];
      return {
        ...state,
        messages: newMessages,
        processedMessages: processMessagesForRendering(newMessages),
      };
    }

    case 'SET_WAITING':
      return { ...state, waitingForResponse: action.payload };

    case 'SET_ERROR':
      return { ...state, chatError: action.payload };

    case 'SET_RETRY_STATE':
      return { ...state, retryState: action.payload };

    case 'UPDATE_NAME':
      return { ...state, name: action.payload };

    case 'CLEAR_INPUT':
      return { ...state, currentInput: '' };

    case 'RESET_CHAT':
      return createInitialState(action.payload);

    default:
      return state;
  }
}

// =============================================================================
// Provider Props
// =============================================================================

interface ChatProviderProps {
  chat: Chat;
  children: ReactNode;
  isGenerating?: boolean;
}

// =============================================================================
// Provider Component
// =============================================================================

export function ChatProvider({ chat, children, isGenerating = false }: ChatProviderProps) {
  const [state, dispatch] = useReducer(chatReducer, chat, (c) => ({
    ...createInitialState(c),
    waitingForResponse: isGenerating,
  }));

  const { dispatch: playgroundDispatch } = usePlayground();

  // Reset state when chat changes (different chat selected)
  useEffect(() => {
    if (chat.id !== state.id) {
      dispatch({ type: 'RESET_CHAT', payload: chat });
    }
  }, [chat.id, state.id, chat]);

  // Sync model and computer from chat prop when they become available
  useEffect(() => {
    if (chat.model && !state.model) {
      dispatch({ type: 'SET_MODEL', payload: chat.model });
    }
  }, [chat.model, state.model]);

  useEffect(() => {
    if (chat.computer && !state.computer) {
      dispatch({ type: 'SET_COMPUTER', payload: chat.computer });
    }
  }, [chat.computer, state.computer]);

  // Sync messages from global state when they change
  useEffect(() => {
    if (chat.id === state.id && chat.messages && chat.messages.length > state.messages.length) {
      dispatch({ type: 'SET_MESSAGES', payload: chat.messages });
    }
  }, [chat.id, chat.messages, state.id, state.messages.length]);

  // Track previous values to avoid unnecessary syncs
  const prevModelRef = useRef(state.model);
  const prevComputerRef = useRef(state.computer);
  const prevNameRef = useRef(state.name);

  // Sync chat metadata back to global store only when values actually change
  useEffect(() => {
    const modelChanged = prevModelRef.current !== state.model;
    const computerChanged = prevComputerRef.current !== state.computer;
    const nameChanged = prevNameRef.current !== state.name;

    if (modelChanged || computerChanged || nameChanged) {
      prevModelRef.current = state.model;
      prevComputerRef.current = state.computer;
      prevNameRef.current = state.name;

      playgroundDispatch({
        type: 'UPDATE_CHAT',
        payload: {
          id: state.id,
          updates: {
            name: state.name,
            model: state.model,
            computer: state.computer,
            updated: new Date(),
          },
        },
      });
    }
  }, [state.name, state.model, state.computer, state.id, playgroundDispatch]);

  return (
    <ChatStateContext.Provider value={state}>
      <ChatDispatchContext.Provider value={dispatch}>{children}</ChatDispatchContext.Provider>
    </ChatStateContext.Provider>
  );
}
