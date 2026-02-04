// Chat panel component
// Adapted from cloud/src/website/app/components/playground/panels/ChatPanel.tsx

import { useEffect, useRef, useState, type FormEvent } from 'react';
import { motion } from 'motion/react';
import { StopCircle, Send } from 'lucide-react';
import {
  usePlayground,
  useChat,
  useChatDispatch,
  useIsChatGenerating,
} from '../../hooks/usePlayground';
import { useAgentRequest } from '../../hooks/useAgentRequest';
import { ChatMessage } from '../primitives/ChatMessage';
import { ToolCallsGroup } from '../primitives/ToolCallsGroup';
import { ThinkingIndicator } from '../primitives/ThinkingIndicator';
import type { Computer } from '../../types';
import { cn } from '../../utils/cn';

interface ChatPanelProps {
  /** Optional class name for styling */
  className?: string;
  /** Optional callback when model changes */
  onModelChange?: (modelId: string) => void;
  /** Optional callback when computer changes */
  onComputerChange?: (computerId: string) => void;
}

export function ChatPanel({ className, onModelChange, onComputerChange }: ChatPanelProps) {
  const { state } = usePlayground();
  const chatState = useChat();
  const chatDispatch = useChatDispatch();
  const { handleSendMessage, handleStopResponse } = useAgentRequest();

  const isGenerating = useIsChatGenerating(chatState.id);
  const chatContainerEnd = useRef<HTMLDivElement | null>(null);
  const [stopping, setStopping] = useState(false);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (chatState.processedMessages.length > 0) {
      setTimeout(() => chatContainerEnd.current?.scrollIntoView({ behavior: 'smooth' }), 100);
    }
  }, [chatState.processedMessages.length]);

  const onSend = (e: FormEvent) => {
    e.preventDefault();
    if (!chatState.currentInput.trim()) return;
    setStopping(false);
    handleSendMessage();
  };

  const onStop = () => {
    setStopping(true);
    handleStopResponse();
  };

  const handleModelChange = (modelId: string) => {
    // Find the model from available models
    for (const provider of state.availableModels) {
      const model = provider.models.find((m) => m.id === modelId);
      if (model) {
        chatDispatch({ type: 'SET_MODEL', payload: model });
        onModelChange?.(modelId);
        return;
      }
    }
  };

  const handleComputerChange = (computerId: string) => {
    // Find the computer from available computers
    // Note: We need to map from ComputerInfo to Computer type
    const computer = state.computers.find((c) => c.id === computerId);
    if (computer) {
      // Convert ComputerInfo to Computer format for the chat state
      const chatComputer: Computer = {
        id: computer.id,
        name: computer.name,
        url: computer.agentUrl,
      };
      chatDispatch({ type: 'SET_COMPUTER', payload: chatComputer });
      onComputerChange?.(computerId);
    }
  };

  return (
    <div className={cn('flex h-full flex-col bg-white dark:bg-neutral-900', className)}>
      {/* Header with model and computer selects */}
      <div className="flex items-center justify-center gap-3 border-b px-4 py-3">
        {/* Model Select */}
        <select
          value={chatState.model?.id || ''}
          onChange={(e) => handleModelChange(e.target.value)}
          className="w-1/2 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-800"
        >
          <option value="">Select a Model</option>
          {state.availableModels.map((provider) => (
            <optgroup key={provider.name} label={provider.name}>
              {provider.models.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.name}
                </option>
              ))}
            </optgroup>
          ))}
        </select>

        {/* Computer Select */}
        <select
          value={state.currentComputerId || ''}
          onChange={(e) => handleComputerChange(e.target.value)}
          className="w-1/2 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-800"
        >
          <option value="">Select a Computer</option>
          {state.computers.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </div>

      {/* Chat Container */}
      <div className="flex-1 overflow-auto px-6 pt-10 pb-2">
        <div className="flex flex-col items-center">
          <p className="text-neutral-700 dark:text-neutral-300">How can I help you today?</p>

          <motion.div
            key={chatState.id}
            className="mt-8 flex w-full flex-col gap-4"
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -20 }}
            transition={{ duration: 0.3, ease: 'easeInOut' }}
          >
            {chatState.processedMessages.length === 0
              ? (!chatState.computer || !chatState.model) && (
                  <p className="mx-auto text-center text-neutral-500">
                    Select a model and computer to get started.
                  </p>
                )
              : chatState.processedMessages.map((item, index) => {
                  if (item.toolCalls && item.toolCalls.length > 0) {
                    return (
                      <div key={`group-${index}`}>
                        <ChatMessage message={item.message} screenshot={item.screenshot} />
                        <ToolCallsGroup toolCalls={item.toolCalls} />
                      </div>
                    );
                  }
                  return (
                    <ChatMessage
                      key={`msg-${index}`}
                      message={item.message}
                      screenshot={item.screenshot}
                    />
                  );
                })}

            {/* Generating indicator */}
            {isGenerating && (
              <div className="flex w-fit items-center gap-2 rounded-md border bg-neutral-50 p-2 dark:bg-neutral-800">
                {!stopping && (
                  <button
                    type="button"
                    onClick={onStop}
                    className="rounded-md border p-1 hover:bg-neutral-100 dark:hover:bg-neutral-700"
                    title="Stop the Agent"
                  >
                    <StopCircle className="h-5 w-5" />
                  </button>
                )}
                <p>{!stopping ? 'Processing' : 'Stopping'}</p>
                <ThinkingIndicator />
              </div>
            )}

            {/* Error display */}
            {chatState.chatError && (
              <div className="rounded-md border border-red-300 bg-red-50 p-3 text-red-800 dark:border-red-800 dark:bg-red-900/20 dark:text-red-300">
                <p className="font-medium">Error</p>
                <p className="text-sm">{chatState.chatError.message}</p>
              </div>
            )}

            <div ref={chatContainerEnd} />
          </motion.div>
        </div>
      </div>

      {/* Input Area */}
      <form onSubmit={onSend} className="flex items-end gap-2 border-t p-4">
        <input
          type="text"
          placeholder="Enter your prompt here"
          value={chatState.currentInput}
          onChange={(e) => chatDispatch({ type: 'SET_INPUT', payload: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              onSend(e);
            }
          }}
          className="flex-1 rounded-md border border-neutral-300 bg-white px-4 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-800"
          disabled={isGenerating}
        />
        <button
          type="submit"
          disabled={isGenerating || !chatState.currentInput.trim()}
          className="rounded-md bg-neutral-900 p-2 text-white hover:bg-neutral-700 disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-neutral-300"
          title="Send your prompt"
        >
          <Send className="h-5 w-5" />
        </button>
      </form>
    </div>
  );
}
