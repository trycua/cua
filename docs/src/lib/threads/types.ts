// src/lib/threads/types.ts

export interface Thread {
  id: string;
  title: string;
  messages: ThreadMessage[];
  createdAt: number;
  updatedAt: number;
}

export interface ThreadMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
}

export interface ThreadsState {
  threads: Thread[];
  activeThreadId: string | null;
  view: 'list' | 'chat';
}

export const THREADS_STORAGE_KEY = 'cua-docs-assistant-threads';

export const DEFAULT_THREAD_TITLE = 'New conversation';
