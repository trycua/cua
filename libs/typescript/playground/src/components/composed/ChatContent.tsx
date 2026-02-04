// ChatContent component
// Adapted from cloud/src/website/app/components/playground/ChatContent.tsx

import { motion } from 'motion/react';
import { useEffect, useMemo, useRef } from 'react';
import { ChatArea } from './ChatArea';
import { ChatInput } from '../primitives/ChatInput';
import { ExamplePrompts, type ExamplePrompt } from './ExamplePrompts';
import {
  useChat,
  useChatDispatch,
  usePlayground,
  useIsChatGenerating,
} from '../../hooks/usePlayground';
import { useAgentRequest } from '../../hooks/useAgentRequest';
import type { Computer, VM } from '../../types';
import { getComputerId } from '../../types';
import { cn } from '../../utils/cn';

interface ChatContentProps {
  /** Whether the viewport is mobile */
  isMobile?: boolean;

  // Optional session/credits props (for cloud usage)
  hasOrg?: boolean;
  hasWorkspace?: boolean;
  hasCredits?: boolean;
  orgSlug?: string;

  // VM management callbacks
  onRestartVM?: (vm: VM) => void;
  onStartVM?: (vm: VM) => void;

  // Auto-send first message (for creating chats with pre-filled messages)
  shouldAutoSendFirstMessage?: boolean;
  onAutoSendComplete?: () => void;

  // Custom rendering
  renderLink?: (props: {
    to: string;
    children: React.ReactNode;
    className?: string;
  }) => React.ReactNode;
  renderVMStatusBanner?: (props: {
    onRestartVM?: (vm: VM) => void;
    onStartVM?: (vm: VM) => void;
    hasOrg?: boolean;
    hasWorkspace?: boolean;
    hasCredits?: boolean;
    orgSlug?: string;
    computer?: Computer;
    computers: Computer[];
  }) => React.ReactNode;

  // Logo URLs for empty state
  lightLogoUrl?: string;
  darkLogoUrl?: string;

  // Example prompts customization
  examplePrompts?: ExamplePrompt[];
  onExamplePromptSelected?: (promptId: string, promptTitle: string) => void;

  // Toast callback for user notifications
  onToast?: (message: string, type?: 'success' | 'error' | 'info') => void;

  // Custom class name
  className?: string;
}

/**
 * Chat content component that provides the main chat interface.
 * Must be used inside a ChatProvider.
 */
export function ChatContent({
  isMobile = false,
  hasOrg = true,
  hasWorkspace = true,
  hasCredits = true,
  orgSlug,
  onRestartVM,
  onStartVM,
  shouldAutoSendFirstMessage,
  onAutoSendComplete,
  renderLink,
  renderVMStatusBanner,
  lightLogoUrl = '/cua_logo_black_new.svg',
  darkLogoUrl = '/cua_logo_white_new.svg',
  examplePrompts,
  onExamplePromptSelected,
  onToast,
  className,
}: ChatContentProps) {
  const { handleSendMessage, handleStopResponse, handleTimeout, handleRetry } = useAgentRequest();
  const { state, dispatch: playgroundDispatch } = usePlayground();
  const chatState = useChat();
  const chatDispatch = useChatDispatch();
  const hasAutoSentRef = useRef(false);

  const { processedMessages, computer, model, currentInput, id: chatId } = chatState;
  const isGenerating = useIsChatGenerating(chatId);

  const handleSelectPrompt = (prompt: string) => {
    chatDispatch({ type: 'SET_INPUT', payload: prompt });
  };

  // Auto-send first message when chat is created from empty state
  useEffect(() => {
    if (shouldAutoSendFirstMessage && !hasAutoSentRef.current) {
      hasAutoSentRef.current = true;
      // Use handleRetry which sends the existing messages to the agent
      handleRetry();
      onAutoSendComplete?.();
    }
  }, [shouldAutoSendFirstMessage, handleRetry, onAutoSendComplete]);

  const hasMessages = processedMessages.length > 0;

  // Convert ComputerInfo[] to Computer[] for ChatInput
  const computers: Computer[] = state.computers.map((c) => {
    const { agentUrl, ...rest } = c;
    return {
      ...rest,
      url: agentUrl,
    } as Computer;
  });

  // Determine selected computer: prefer currentComputerId from playground state, fallback to chat's computer
  // This ensures the Select stays in sync with the VNC viewer
  const selectedComputerForInput = useMemo(() => {
    if (state.currentComputerId) {
      const currentComputer = computers.find((c) => getComputerId(c) === state.currentComputerId);
      if (currentComputer) {
        return currentComputer;
      }
    }
    return computer;
  }, [state.currentComputerId, computers, computer]);

  // Handle model change
  const handleModelChange = (modelId: string) => {
    for (const provider of state.availableModels) {
      const foundModel = provider.models.find((m) => m.id === modelId);
      if (foundModel) {
        chatDispatch({ type: 'SET_MODEL', payload: foundModel });
        return;
      }
    }
  };

  // Handle computer change
  const handleComputerChange = (computerId: string) => {
    const computerInfo = state.computers.find((c) => c.id === computerId);
    if (computerInfo) {
      const { agentUrl, ...rest } = computerInfo;
      const chatComputer: Computer = {
        ...rest,
        url: agentUrl,
      } as Computer;
      chatDispatch({ type: 'SET_COMPUTER', payload: chatComputer });
      // Also update global state so VNC overlay can react to computer changes
      playgroundDispatch({ type: 'SET_CURRENT_COMPUTER', payload: computerId });

      // Warn if selecting an offline computer
      if (computerInfo.status !== 'running') {
        onToast?.(
          'Selected sandbox is not running. Please start it before sending messages.',
          'error'
        );
      }
    }
  };

  // Handle input change
  const handleInputChange = (value: string) => {
    chatDispatch({ type: 'SET_INPUT', payload: value });
  };

  // Render VM status banner if provided
  const vmStatusBanner = renderVMStatusBanner?.({
    onRestartVM,
    onStartVM,
    hasOrg,
    hasWorkspace,
    hasCredits,
    orgSlug,
    computer,
    computers,
  });

  // Centered layout for empty chat (no messages yet)
  if (!hasMessages) {
    return (
      <motion.div
        className={cn('flex flex-1 flex-col items-center justify-center p-6', className)}
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: -40, transition: { duration: 0.2 } }}
        transition={{ duration: 0.3, ease: 'easeOut' }}
      >
        <div className="w-full max-w-3xl">
          {/* Logo and welcome text */}
          <motion.div
            className="mb-6 flex flex-col items-center text-center"
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ duration: 0.3, delay: 0.1 }}
          >
            <img src={lightLogoUrl} className="mb-4 h-14 w-14 dark:hidden" alt="Logo" />
            <img src={darkLogoUrl} className="mb-4 hidden h-14 w-14 dark:block" alt="Logo" />
            <p className="text-lg text-neutral-700 dark:text-neutral-300">
              How can I help you today?
            </p>
            {(!computer || !model) && (
              <p className="mt-2 text-neutral-500 text-sm dark:text-neutral-400">
                Make sure to select a model and sandbox!
              </p>
            )}
          </motion.div>

          {/* Input area */}
          <motion.div
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0.15 }}
          >
            {vmStatusBanner}
            <ChatInput
              currentInput={currentInput}
              onInputChange={handleInputChange}
              onSendMessage={handleSendMessage}
              onStopResponse={handleStopResponse}
              isGenerating={isGenerating}
              computers={computers}
              selectedComputer={selectedComputerForInput}
              onComputerChange={handleComputerChange}
              availableModels={state.availableModels}
              selectedModel={model}
              onModelChange={handleModelChange}
              hasMessages={hasMessages}
              isMobile={isMobile}
              hasOrg={hasOrg}
              hasWorkspace={hasWorkspace}
              hasCredits={hasCredits}
              orgSlug={orgSlug}
              renderLink={renderLink}
            />
          </motion.div>

          {/* Example prompts */}
          <ExamplePrompts
            onSelectPrompt={handleSelectPrompt}
            onExamplePromptSelected={onExamplePromptSelected}
            prompts={examplePrompts}
          />
        </div>
      </motion.div>
    );
  }

  // Normal layout with messages and input at bottom
  return (
    <div className={cn('flex flex-1 flex-col', className)}>
      <ChatArea
        onRetry={handleRetry}
        onStopResponse={handleStopResponse}
        onTimeout={handleTimeout}
      />

      <div className="flex-shrink-0 p-6">
        <div className="mx-auto max-w-3xl">
          {vmStatusBanner}
          <ChatInput
            currentInput={currentInput}
            onInputChange={handleInputChange}
            onSendMessage={handleSendMessage}
            onStopResponse={handleStopResponse}
            isGenerating={isGenerating}
            computers={computers}
            selectedComputer={selectedComputerForInput}
            onComputerChange={handleComputerChange}
            availableModels={state.availableModels}
            selectedModel={model}
            onModelChange={handleModelChange}
            hasMessages={hasMessages}
            isMobile={isMobile}
            hasOrg={hasOrg}
            hasWorkspace={hasWorkspace}
            hasCredits={hasCredits}
            orgSlug={orgSlug}
            renderLink={renderLink}
          />
        </div>
      </div>
    </div>
  );
}
