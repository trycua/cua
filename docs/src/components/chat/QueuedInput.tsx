// src/components/chat/QueuedInput.tsx
'use client';

import { useRef, useEffect, useState, useMemo, useCallback } from 'react';
import { useChatContext } from '@copilotkit/react-ui';

// Re-implement the essential Input logic with queuing support
// Based on CopilotKit's Input component but with message queuing when chatReady is false

interface QueuedInputProps {
  inProgress: boolean;
  onSend: (text: string) => Promise<any>;
  isVisible?: boolean;
  onStop?: () => void;
  onUpload?: () => void;
  hideStopButton?: boolean;
  chatReady?: boolean;
}

const MAX_ROWS = 6;

export function QueuedInput({
  inProgress,
  onSend,
  onStop,
  onUpload,
  hideStopButton = false,
  chatReady = false,
}: QueuedInputProps) {
  const context = useChatContext();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState('');
  const [isComposing, setIsComposing] = useState(false);
  const queuedMessageRef = useRef<string | null>(null);
  const [queuedMessageText, setQueuedMessageText] = useState<string | null>(null);

  const isQueuedMessagePending = queuedMessageText !== null;

  // When chatReady becomes true, send any queued message
  useEffect(() => {
    if (chatReady && queuedMessageRef.current !== null) {
      const messageToSend = queuedMessageRef.current;
      queuedMessageRef.current = null;
      setQueuedMessageText(null);
      onSend(messageToSend);
    }
  }, [chatReady, onSend]);

  const send = useCallback(() => {
    const trimmedText = text.trim();
    if (!trimmedText) return;
    if (inProgress) return;

    if (!chatReady) {
      // Queue the message - it will be sent when chatReady becomes true
      queuedMessageRef.current = trimmedText;
      setQueuedMessageText(trimmedText);
      setText('');
      textareaRef.current?.focus();
      return;
    }

    onSend(trimmedText);
    setText('');
    textareaRef.current?.focus();
  }, [text, inProgress, chatReady, onSend]);

  const handleDivClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest('button')) return;
    if (target.tagName === 'TEXTAREA') return;
    textareaRef.current?.focus();
  };

  const isInProgress = inProgress || isQueuedMessagePending;

  const { buttonIcon, buttonAlt } = useMemo(() => {
    if (!chatReady || isQueuedMessagePending) {
      return { buttonIcon: context.icons.spinnerIcon, buttonAlt: 'Connecting...' };
    }
    return isInProgress && !hideStopButton
      ? { buttonIcon: context.icons.stopIcon, buttonAlt: 'Stop' }
      : { buttonIcon: context.icons.sendIcon, buttonAlt: 'Send' };
  }, [isInProgress, chatReady, hideStopButton, isQueuedMessagePending, context.icons]);

  const canSend = useMemo(() => {
    // Allow sending even when chatReady is false - we'll queue it
    return !inProgress && text.trim().length > 0 && !isQueuedMessagePending;
  }, [inProgress, text, isQueuedMessagePending]);

  const canStop = useMemo(() => {
    return inProgress && !hideStopButton && chatReady;
  }, [inProgress, hideStopButton, chatReady]);

  const sendDisabled = !canSend && !canStop;

  // Auto-resize textarea
  const adjustHeight = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    const lineHeight = parseInt(getComputedStyle(textarea).lineHeight) || 20;
    const maxHeight = lineHeight * MAX_ROWS;
    textarea.style.height = `${Math.min(textarea.scrollHeight, maxHeight)}px`;
  };

  useEffect(() => {
    adjustHeight();
  }, [text]);

  return (
    <div className="copilotKitInputContainer">
      <div className="copilotKitInput" onClick={handleDivClick}>
        <textarea
          ref={textareaRef}
          placeholder={isQueuedMessagePending ? 'Sending...' : context.labels.placeholder}
          autoFocus={false}
          rows={1}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onCompositionStart={() => setIsComposing(true)}
          onCompositionEnd={() => setIsComposing(false)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !isComposing) {
              event.preventDefault();
              if (canSend) {
                send();
              }
            }
          }}
          disabled={isQueuedMessagePending}
          style={{
            resize: 'none',
            overflow: 'auto',
          }}
        />
        <div className="copilotKitInputControls">
          {onUpload && (
            <button onClick={onUpload} className="copilotKitInputControlButton">
              {context.icons.uploadIcon}
            </button>
          )}

          <div style={{ flexGrow: 1 }} />

          <button
            disabled={sendDisabled}
            onClick={isInProgress && !hideStopButton && chatReady ? onStop : send}
            data-copilotkit-in-progress={inProgress}
            data-test-id={inProgress ? 'copilot-chat-request-in-progress' : 'copilot-chat-ready'}
            className="copilotKitInputControlButton"
            aria-label={buttonAlt}
          >
            {buttonIcon}
          </button>
        </div>
      </div>
      {isQueuedMessagePending && queuedMessageText && (
        <div className="mt-2 px-3 py-2 bg-zinc-100 dark:bg-zinc-800 rounded-lg text-sm">
          <div className="text-zinc-500 dark:text-zinc-400 text-xs mb-1 flex items-center gap-1">
            <span className="inline-block w-2 h-2 bg-blue-500 rounded-full animate-pulse" />
            Sending when connected...
          </div>
          <div className="text-zinc-700 dark:text-zinc-300">{queuedMessageText}</div>
        </div>
      )}
    </div>
  );
}
