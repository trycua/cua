// ChatArea component
// Adapted from cloud/src/website/app/components/playground/ChatArea.tsx

import { motion } from 'motion/react';
import { useEffect, useRef } from 'react';
import { ChatMessage } from '../primitives/ChatMessage';
import { CountdownTimer } from '../primitives/CountdownTimer';
import { ThinkingIndicator } from '../primitives/ThinkingIndicator';
import {
  useChat,
  useChatDispatch,
  useIsChatGenerating,
  useLastRequestDuration,
  useRequestStartTime,
} from '../../hooks/usePlayground';

/** Request timeout in seconds */
const REQUEST_TIMEOUT_SECONDS = 60;

interface ChatAreaProps {
  onRetry: () => void;
  onStopResponse: () => void;
  onTimeout: () => void;
}

export function ChatArea({ onRetry, onTimeout }: ChatAreaProps) {
  const chatState = useChat();
  const chatDispatch = useChatDispatch();
  const chatContainerEnd = useRef<HTMLDivElement | null>(null);

  const { id: chatId, processedMessages, chatError, retryState } = chatState;

  // Use global generating state (persists across chat switches)
  const isGenerating = useIsChatGenerating(chatId);
  const lastRequestDuration = useLastRequestDuration(chatId);
  const requestStartTime = useRequestStartTime(chatId);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (processedMessages.length) {
      setTimeout(() => chatContainerEnd.current?.scrollIntoView({ behavior: 'smooth' }), 100);
    }
  }, [processedMessages]);

  const handleRetry = () => {
    chatDispatch({ type: 'SET_ERROR', payload: null });
    onRetry();
  };

  return (
    <>
      {/* Messages area */}
      <div className="flex-1 min-h-0 overflow-y-auto p-4 pb-0 md:p-6">
        <div className="mx-auto max-w-3xl space-y-6">
          {processedMessages.length > 0 && (
            <motion.div
              key={chatId}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3 }}
              className="space-y-1"
            >
              {processedMessages.map((item, index) => {
                // Determine if this is the last non-user message in a consecutive group
                const nextMessage = processedMessages[index + 1]?.message;
                const isLastInGroup =
                  index === processedMessages.length - 1 ||
                  (nextMessage && 'role' in nextMessage && nextMessage.role === 'user');

                // Determine if this is the last user message in a consecutive group
                const currentMsg = item.message;
                const isUserMessage = 'role' in currentMsg && currentMsg.role === 'user';
                const nextMsg = processedMessages[index + 1]?.message;
                const isLastConsecutiveUserMessage =
                  isUserMessage &&
                  (index === processedMessages.length - 1 ||
                    !(nextMsg && 'role' in nextMsg && nextMsg.role === 'user'));

                // Show duration on the last assistant message when not generating
                const isLastMessage = index === processedMessages.length - 1;
                const isAssistantMessage = 'role' in currentMsg && currentMsg.role === 'assistant';
                const showDuration =
                  isLastMessage &&
                  isAssistantMessage &&
                  !isGenerating &&
                  lastRequestDuration !== null;

                return (
                  <ChatMessage
                    key={`${item.message.type}-${index}`}
                    message={item.message}
                    screenshot={item.screenshot}
                    toolCalls={item.toolCalls}
                    isLastInGroup={isLastInGroup}
                    isLastConsecutiveUserMessage={isLastConsecutiveUserMessage}
                    durationSeconds={showDuration ? lastRequestDuration : undefined}
                  />
                );
              })}

              {/* Error message display */}
              {chatError && (
                <motion.div
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  className="space-y-2"
                >
                  <div>
                    <p className="font-medium text-red-600 text-sm dark:text-red-400">
                      Request Failed
                    </p>
                    <p className="mt-1 text-neutral-700 text-sm dark:text-neutral-300">
                      {chatError.message}
                    </p>
                  </div>
                  <button
                    type="button"
                    onClick={handleRetry}
                    className="rounded-md bg-neutral-200 px-3 py-1.5 text-neutral-700 text-sm transition-colors hover:bg-neutral-300 dark:bg-neutral-800 dark:text-neutral-300 dark:hover:bg-neutral-700"
                  >
                    Try Again
                  </button>
                </motion.div>
              )}

              {/* Retry indicator */}
              {retryState && (
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  className="rounded-md bg-amber-50 px-3 py-2 dark:bg-amber-950/30"
                >
                  <p className="text-amber-700 text-sm dark:text-amber-400">
                    Connection failed. Retrying... (attempt {retryState.attempt + 1}/
                    {retryState.maxRetries})
                  </p>
                </motion.div>
              )}

              {/* Thinking indicator with bouncing dots and scrambled text */}
              {isGenerating && (
                <div className="space-y-2">
                  <ThinkingIndicator />
                  <CountdownTimer
                    key={requestStartTime ?? 0}
                    durationSeconds={REQUEST_TIMEOUT_SECONDS}
                    onTimeout={onTimeout}
                    isActive={isGenerating}
                  />
                </div>
              )}
            </motion.div>
          )}
          <div ref={chatContainerEnd} />
        </div>
      </div>
    </>
  );
}
