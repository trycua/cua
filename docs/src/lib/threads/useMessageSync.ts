// src/lib/threads/useMessageSync.ts
'use client';

import { useEffect, useRef } from 'react';
import { useCopilotChat } from '@copilotkit/react-core';
import { useThreads } from './ThreadsContext';
import { ThreadMessage } from './types';

export function useMessageSync() {
  const { visibleMessages } = useCopilotChat();
  const { activeThreadId, getActiveThread, updateThreadMessages, threads } = useThreads();
  const lastSyncedThreadId = useRef<string | null>(null);
  const isLoadingThread = useRef(false);

  // Load messages when switching to a thread - handled by CopilotKit threadId prop
  useEffect(() => {
    if (!activeThreadId) return;
    if (lastSyncedThreadId.current === activeThreadId) return;

    lastSyncedThreadId.current = activeThreadId;
    isLoadingThread.current = true;

    // Small delay to prevent immediate save after switching
    setTimeout(() => {
      isLoadingThread.current = false;
    }, 500);
  }, [activeThreadId]);

  // Save messages when they change
  useEffect(() => {
    if (!activeThreadId) return;
    if (isLoadingThread.current) return;
    if (!visibleMessages || visibleMessages.length === 0) return;

    const threadMessages: ThreadMessage[] = visibleMessages
      .filter((msg: any) => msg.role === 'user' || msg.role === 'assistant')
      .map((msg: any) => ({
        id: msg.id || crypto.randomUUID(),
        role: msg.role as 'user' | 'assistant',
        content: typeof msg.content === 'string' ? msg.content : String(msg.content || ''),
      }));

    if (threadMessages.length > 0) {
      updateThreadMessages(activeThreadId, threadMessages);
    }
  }, [visibleMessages, activeThreadId, updateThreadMessages]);
}
