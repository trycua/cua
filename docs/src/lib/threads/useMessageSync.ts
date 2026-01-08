// src/lib/threads/useMessageSync.ts
'use client';

import { useEffect, useRef } from 'react';
import { useCopilotChat } from '@copilotkit/react-core';
import { useThreads } from './ThreadsContext';
import { ThreadMessage } from './types';

export function useMessageSync() {
  const { visibleMessages, setMessages } = useCopilotChat();
  const { activeThreadId, getActiveThread, updateThreadMessages, threads } = useThreads();
  const lastSyncedThreadId = useRef<string | null>(null);
  const isLoadingThread = useRef(false);

  // Load messages when switching to a thread
  useEffect(() => {
    if (!activeThreadId) return;
    if (lastSyncedThreadId.current === activeThreadId) return;

    const thread = threads.find((t) => t.id === activeThreadId);
    if (!thread) return;

    isLoadingThread.current = true;
    lastSyncedThreadId.current = activeThreadId;

    // Convert thread messages to CopilotKit format and load
    if (thread.messages.length > 0) {
      const copilotMessages = thread.messages.map((msg) => ({
        id: msg.id,
        role: msg.role,
        content: msg.content,
      }));
      setMessages(copilotMessages as any);
    } else {
      setMessages([]);
    }

    // Small delay to prevent immediate save
    setTimeout(() => {
      isLoadingThread.current = false;
    }, 100);
  }, [activeThreadId, threads, setMessages]);

  // Save messages when they change
  useEffect(() => {
    if (!activeThreadId) return;
    if (isLoadingThread.current) return;
    if (visibleMessages.length === 0) return;

    const threadMessages: ThreadMessage[] = visibleMessages
      .filter((msg) => msg.role === 'user' || msg.role === 'assistant')
      .map((msg) => ({
        id: msg.id,
        role: msg.role as 'user' | 'assistant',
        content: typeof msg.content === 'string' ? msg.content : '',
      }));

    updateThreadMessages(activeThreadId, threadMessages);
  }, [visibleMessages, activeThreadId, updateThreadMessages]);
}
