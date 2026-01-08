// src/lib/threads/ThreadsContext.tsx
'use client';

import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  ReactNode,
} from 'react';
import { Thread, ThreadMessage, ThreadsState, DEFAULT_THREAD_TITLE } from './types';
import { loadThreadsFromStorage, saveThreadsToStorage, generateThreadId } from './storage';

interface ThreadsContextValue extends ThreadsState {
  createThread: () => string;
  switchThread: (threadId: string) => void;
  updateThreadMessages: (threadId: string, messages: ThreadMessage[]) => void;
  updateThreadTitle: (threadId: string, title: string) => void;
  deleteThread: (threadId: string) => void;
  setView: (view: 'list' | 'chat') => void;
  getActiveThread: () => Thread | null;
}

const ThreadsContext = createContext<ThreadsContextValue | null>(null);

export function ThreadsProvider({ children }: { children: ReactNode }) {
  const [threads, setThreads] = useState<Thread[]>([]);
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const [view, setView] = useState<'list' | 'chat'>('list');
  const [isHydrated, setIsHydrated] = useState(false);

  // Load from localStorage on mount
  useEffect(() => {
    const loaded = loadThreadsFromStorage();
    setThreads(loaded);
    setIsHydrated(true);
  }, []);

  // Save to localStorage on change
  useEffect(() => {
    if (isHydrated) {
      saveThreadsToStorage(threads);
    }
  }, [threads, isHydrated]);

  const createThread = useCallback((): string => {
    const newThread: Thread = {
      id: generateThreadId(),
      title: DEFAULT_THREAD_TITLE,
      messages: [],
      createdAt: Date.now(),
      updatedAt: Date.now(),
    };

    setThreads((prev) => [newThread, ...prev]);
    setActiveThreadId(newThread.id);
    setView('chat');
    return newThread.id;
  }, []);

  const switchThread = useCallback((threadId: string) => {
    setActiveThreadId(threadId);
    setView('chat');
  }, []);

  const updateThreadMessages = useCallback(
    (threadId: string, messages: ThreadMessage[]) => {
      setThreads((prev) =>
        prev.map((thread) =>
          thread.id === threadId
            ? { ...thread, messages, updatedAt: Date.now() }
            : thread
        )
      );
    },
    []
  );

  const updateThreadTitle = useCallback((threadId: string, title: string) => {
    setThreads((prev) =>
      prev.map((thread) =>
        thread.id === threadId
          ? { ...thread, title, updatedAt: Date.now() }
          : thread
      )
    );
  }, []);

  const deleteThread = useCallback((threadId: string) => {
    setThreads((prev) => prev.filter((thread) => thread.id !== threadId));
    if (activeThreadId === threadId) {
      setActiveThreadId(null);
      setView('list');
    }
  }, [activeThreadId]);

  const getActiveThread = useCallback((): Thread | null => {
    return threads.find((t) => t.id === activeThreadId) ?? null;
  }, [threads, activeThreadId]);

  return (
    <ThreadsContext.Provider
      value={{
        threads,
        activeThreadId,
        view,
        createThread,
        switchThread,
        updateThreadMessages,
        updateThreadTitle,
        deleteThread,
        setView,
        getActiveThread,
      }}
    >
      {children}
    </ThreadsContext.Provider>
  );
}

export function useThreads(): ThreadsContextValue {
  const context = useContext(ThreadsContext);
  if (!context) {
    throw new Error('useThreads must be used within a ThreadsProvider');
  }
  return context;
}
