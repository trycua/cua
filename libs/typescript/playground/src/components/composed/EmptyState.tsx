// Empty state component with chat input
// Adapted from cloud/src/website/app/components/playground/EmptyState.tsx

import { motion } from 'motion/react';
import { useState } from 'react';
import { ChatInput } from '../primitives/ChatInput';
import { ExamplePrompts } from './ExamplePrompts';
import { ChatProvider } from '../../context/ChatProvider';
import { useChat, useChatDispatch, usePlayground } from '../../hooks/usePlayground';
import type { Chat, Computer } from '../../types';

interface EmptyStateWithInputProps {
  /** Callback when user sends a message to create a new chat */
  onCreateAndSend: (message: string) => Promise<void>;
  /** Whether in mobile layout */
  isMobile: boolean;
  /** Draft chat for UI consistency */
  draftChat: Chat;
  /** Logo config for branding */
  logo?: {
    lightSrc: string;
    darkSrc: string;
    alt: string;
  };
  /** Custom welcome message */
  welcomeMessage?: string;
  /** Custom hint message when model/computer not selected */
  selectionHint?: string;
  /** Optional telemetry callback when example prompt is selected */
  onExamplePromptSelected?: (promptId: string, promptTitle: string) => void;
  /** Toast callback for user notifications */
  onToast?: (message: string, type?: 'success' | 'error' | 'info') => void;
}

/**
 * Empty state component with chat input - uses a draft chat for UI consistency
 */
export function EmptyStateWithInput({
  onCreateAndSend,
  isMobile,
  draftChat,
  logo,
  welcomeMessage = 'How can I help you today?',
  selectionHint = 'Make sure to select a model and sandbox!',
  onExamplePromptSelected,
  onToast,
}: EmptyStateWithInputProps) {
  return (
    <ChatProvider chat={draftChat}>
      <EmptyStateContent
        onCreateAndSend={onCreateAndSend}
        isMobile={isMobile}
        logo={logo}
        welcomeMessage={welcomeMessage}
        selectionHint={selectionHint}
        onExamplePromptSelected={onExamplePromptSelected}
        onToast={onToast}
      />
    </ChatProvider>
  );
}

interface EmptyStateContentProps {
  onCreateAndSend: (message: string) => Promise<void>;
  isMobile: boolean;
  logo?: {
    lightSrc: string;
    darkSrc: string;
    alt: string;
  };
  welcomeMessage: string;
  selectionHint: string;
  onExamplePromptSelected?: (promptId: string, promptTitle: string) => void;
  onToast?: (message: string, type?: 'success' | 'error' | 'info') => void;
}

/**
 * Inner component that uses ChatContext
 */
function EmptyStateContent({
  onCreateAndSend,
  isMobile,
  logo,
  welcomeMessage,
  selectionHint,
  onExamplePromptSelected,
  onToast,
}: EmptyStateContentProps) {
  const chatState = useChat();
  const chatDispatch = useChatDispatch();
  const { state, dispatch: playgroundDispatch } = usePlayground();
  const [isCreating, setIsCreating] = useState(false);

  const handleSendMessage = async () => {
    if (!chatState.currentInput.trim() || isCreating) return;
    setIsCreating(true);
    try {
      await onCreateAndSend(chatState.currentInput.trim());
      chatDispatch({ type: 'CLEAR_INPUT' });
    } catch (error) {
      console.error('Failed to create chat:', error);
      const message = error instanceof Error ? error.message : 'Unknown error occurred';
      onToast?.(`Failed to send message: ${message}`, 'error');
    } finally {
      setIsCreating(false);
    }
  };

  const handleSelectPrompt = (prompt: string) => {
    chatDispatch({ type: 'SET_INPUT', payload: prompt });
  };

  const handleInputChange = (value: string) => {
    chatDispatch({ type: 'SET_INPUT', payload: value });
  };

  const handleModelChange = (modelId: string) => {
    for (const provider of state.availableModels) {
      const model = provider.models.find((m) => m.id === modelId);
      if (model) {
        chatDispatch({ type: 'SET_MODEL', payload: model });
        return;
      }
    }
  };

  const handleComputerChange = (computerId: string) => {
    const computerInfo = state.computers.find((c) => c.id === computerId);
    if (computerInfo) {
      const computer: Computer = {
        id: computerInfo.id,
        name: computerInfo.name,
        url: computerInfo.agentUrl,
      };
      chatDispatch({ type: 'SET_COMPUTER', payload: computer });
      // Also update global state so VNC overlay can react to computer changes
      playgroundDispatch({ type: 'SET_CURRENT_COMPUTER', payload: computerId });
    }
  };

  // Pass through computers with all properties (including vmId, status, etc.)
  // The cloudAdapter already includes VM properties needed for status display
  const computers: Computer[] = state.computers.map((c) => ({
    ...c,
    url: c.agentUrl,
  })) as Computer[];

  return (
    <motion.div
      className="flex flex-1 flex-col items-center justify-center p-6"
      initial={{ opacity: 0, y: -20 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -40, transition: { duration: 0.2 } }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
    >
      {/* Centered welcome message and input */}
      <div className="w-full max-w-3xl">
        {/* Logo and welcome text */}
        <motion.div
          className="mb-6 flex flex-col items-center text-center"
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.3, delay: 0.1 }}
        >
          {logo && (
            <>
              <img src={logo.lightSrc} className="mb-4 h-14 w-14 dark:hidden" alt={logo.alt} />
              <img src={logo.darkSrc} className="mb-4 hidden h-14 w-14 dark:block" alt={logo.alt} />
            </>
          )}
          <p className="text-lg text-neutral-700 dark:text-neutral-300">{welcomeMessage}</p>
          {(!chatState.computer || !chatState.model) && (
            <p className="mt-2 text-neutral-500 text-sm dark:text-neutral-400">{selectionHint}</p>
          )}
        </motion.div>

        {/* Input area */}
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, delay: 0.15 }}
        >
          <ChatInput
            currentInput={chatState.currentInput}
            onInputChange={handleInputChange}
            onSendMessage={handleSendMessage}
            onStopResponse={() => {}}
            isGenerating={isCreating}
            computers={computers}
            selectedComputer={chatState.computer}
            onComputerChange={handleComputerChange}
            availableModels={state.availableModels}
            selectedModel={chatState.model}
            onModelChange={handleModelChange}
            isMobile={isMobile}
          />
        </motion.div>

        {/* Example prompts */}
        <ExamplePrompts
          onSelectPrompt={handleSelectPrompt}
          onExamplePromptSelected={onExamplePromptSelected}
        />
      </div>
    </motion.div>
  );
}
