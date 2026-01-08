// src/components/chat/OptimisticMessages.tsx
'use client';

import { useMemo } from 'react';
// @ts-expect-error - Messages is not exported publicly but we need it for the wrapper
import { Messages } from '@copilotkit/react-ui/dist/components/chat/Messages';
import { usePendingMessage } from './PendingMessageContext';

// This wrapper component adds pending (optimistic) messages to the message list
// before they're actually sent to the server
export function OptimisticMessages(props: Parameters<typeof Messages>[0]) {
  const { pendingMessage } = usePendingMessage();

  // Prepend the pending message to the actual messages
  const messagesWithPending = useMemo(() => {
    if (!pendingMessage) {
      return props.messages;
    }

    // Create a fake user message for the pending content
    const pendingUserMessage = {
      id: pendingMessage.id,
      role: 'user' as const,
      content: pendingMessage.content,
    };

    return [...props.messages, pendingUserMessage];
  }, [props.messages, pendingMessage]);

  return (
    <Messages
      {...props}
      messages={messagesWithPending}
      // Show spinner when we have a pending message (connecting to runtime)
      inProgress={props.inProgress || !!pendingMessage}
    />
  );
}
