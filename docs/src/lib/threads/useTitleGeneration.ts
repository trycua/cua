// src/lib/threads/useTitleGeneration.ts
'use client';

import { useEffect, useRef } from 'react';
import { useCopilotChat } from '@copilotkit/react-core';
import { useThreads } from './ThreadsContext';
import { DEFAULT_THREAD_TITLE } from './types';

export function useTitleGeneration() {
  const { visibleMessages } = useCopilotChat();
  const { activeThreadId, getActiveThread, updateThreadTitle } = useThreads();
  const hasGeneratedRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (!activeThreadId) return;

    const activeThread = getActiveThread();
    if (!activeThread) return;

    // Skip if title already generated for this thread
    if (hasGeneratedRef.current.has(activeThreadId)) return;
    if (activeThread.title !== DEFAULT_THREAD_TITLE) return;

    // Check if we have at least one user message and one assistant response
    const userMessage = visibleMessages?.find((m: any) => m.role === 'user');
    const assistantMessage = visibleMessages?.find((m: any) => m.role === 'assistant' && m.content);

    if (!userMessage || !assistantMessage) return;

    // Mark as generating to prevent duplicate calls
    hasGeneratedRef.current.add(activeThreadId);

    // Generate title from first exchange
    const userContent = typeof (userMessage as any).content === 'string'
      ? (userMessage as any).content
      : '';

    // Simple title extraction: use first ~50 chars of user message
    const title = userContent.length > 40
      ? userContent.slice(0, 40).trim() + '...'
      : userContent.trim() || DEFAULT_THREAD_TITLE;

    updateThreadTitle(activeThreadId, title);
  }, [visibleMessages, activeThreadId, getActiveThread, updateThreadTitle]);
}
