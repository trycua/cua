# Chat Threads Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add client-side conversation threads to the CUA Docs Assistant with localStorage persistence and AI-generated titles.

**Architecture:** React context manages thread state with localStorage sync. CopilotKit's native `threadId`/`setThreadId` handles thread switching. Custom components implement view toggling between thread list and chat.

**Tech Stack:** React 19, CopilotKit 1.50.x, TypeScript, localStorage, Tailwind CSS

---

## Task 1: Create Thread Types and Constants

**Files:**
- Create: `src/lib/threads/types.ts`

**Step 1: Create the types file**

```typescript
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
```

**Step 2: Verify file created**

Run: `ls -la src/lib/threads/`
Expected: `types.ts` file exists

**Step 3: Commit**

```bash
git add src/lib/threads/types.ts
git commit -m "feat(threads): add thread types and constants"
```

---

## Task 2: Create Thread Storage Utilities

**Files:**
- Create: `src/lib/threads/storage.ts`

**Step 1: Create storage utilities**

```typescript
// src/lib/threads/storage.ts
'use client';

import { Thread, ThreadsState, THREADS_STORAGE_KEY } from './types';

export function loadThreadsFromStorage(): Thread[] {
  if (typeof window === 'undefined') return [];

  try {
    const stored = localStorage.getItem(THREADS_STORAGE_KEY);
    if (!stored) return [];

    const parsed = JSON.parse(stored);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function saveThreadsToStorage(threads: Thread[]): void {
  if (typeof window === 'undefined') return;

  try {
    localStorage.setItem(THREADS_STORAGE_KEY, JSON.stringify(threads));
  } catch (error) {
    console.error('Failed to save threads to localStorage:', error);
  }
}

export function generateThreadId(): string {
  return crypto.randomUUID();
}
```

**Step 2: Verify file created**

Run: `ls -la src/lib/threads/`
Expected: `storage.ts` file exists

**Step 3: Commit**

```bash
git add src/lib/threads/storage.ts
git commit -m "feat(threads): add localStorage utilities"
```

---

## Task 3: Create Threads Context Provider

**Files:**
- Create: `src/lib/threads/ThreadsContext.tsx`

**Step 1: Create the context provider**

```typescript
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
```

**Step 2: Create barrel export**

Create `src/lib/threads/index.ts`:

```typescript
// src/lib/threads/index.ts
export * from './types';
export * from './storage';
export * from './ThreadsContext';
```

**Step 3: Verify files created**

Run: `ls -la src/lib/threads/`
Expected: `types.ts`, `storage.ts`, `ThreadsContext.tsx`, `index.ts`

**Step 4: Commit**

```bash
git add src/lib/threads/
git commit -m "feat(threads): add ThreadsContext provider with CRUD operations"
```

---

## Task 4: Create Thread List View Component

**Files:**
- Create: `src/components/chat/ThreadListView.tsx`

**Step 1: Create the thread list component**

```typescript
// src/components/chat/ThreadListView.tsx
'use client';

import { useThreads } from '@/lib/threads';
import { Plus, MessageSquare, Trash2 } from 'lucide-react';

function formatTimestamp(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const diffDays = Math.floor((now.getTime() - date.getTime()) / (1000 * 60 * 60 * 24));

  if (diffDays === 0) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  } else if (diffDays === 1) {
    return 'Yesterday';
  } else if (diffDays < 7) {
    return date.toLocaleDateString([], { weekday: 'short' });
  } else {
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }
}

function getPreview(messages: { content: string }[]): string {
  const firstUserMessage = messages.find((m) => m.content);
  if (!firstUserMessage) return 'No messages yet';
  const content = firstUserMessage.content;
  return content.length > 50 ? content.slice(0, 50) + '...' : content;
}

export function ThreadListView() {
  const { threads, createThread, switchThread, deleteThread } = useThreads();

  return (
    <div className="flex flex-col h-full bg-white dark:bg-zinc-900">
      {/* Header */}
      <div className="flex items-center justify-between p-4 border-b border-zinc-200 dark:border-zinc-700">
        <h2 className="text-lg font-semibold text-zinc-900 dark:text-zinc-100">
          Conversations
        </h2>
        <button
          onClick={() => createThread()}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 transition-colors"
        >
          <Plus className="w-4 h-4" />
          New Chat
        </button>
      </div>

      {/* Thread List */}
      <div className="flex-1 overflow-y-auto">
        {threads.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-zinc-500 dark:text-zinc-400 p-8">
            <MessageSquare className="w-12 h-12 mb-4 opacity-50" />
            <p className="text-center">No conversations yet</p>
            <p className="text-sm text-center mt-1">
              Start a new chat to get help with CUA documentation
            </p>
          </div>
        ) : (
          <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
            {threads.map((thread) => (
              <li key={thread.id} className="group">
                <button
                  onClick={() => switchThread(thread.id)}
                  className="w-full p-4 text-left hover:bg-zinc-50 dark:hover:bg-zinc-800 transition-colors"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="flex-1 min-w-0">
                      <h3 className="font-medium text-zinc-900 dark:text-zinc-100 truncate">
                        {thread.title}
                      </h3>
                      <p className="text-sm text-zinc-500 dark:text-zinc-400 truncate mt-0.5">
                        {getPreview(thread.messages)}
                      </p>
                    </div>
                    <div className="flex items-center gap-2 shrink-0">
                      <span className="text-xs text-zinc-400 dark:text-zinc-500">
                        {formatTimestamp(thread.updatedAt)}
                      </span>
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          deleteThread(thread.id);
                        }}
                        className="p-1 text-zinc-400 hover:text-red-500 opacity-0 group-hover:opacity-100 transition-opacity"
                        aria-label="Delete conversation"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
```

**Step 2: Verify file created**

Run: `ls -la src/components/chat/`
Expected: `ThreadListView.tsx` file exists

**Step 3: Commit**

```bash
git add src/components/chat/ThreadListView.tsx
git commit -m "feat(threads): add ThreadListView component"
```

---

## Task 5: Create Custom Chat Header Component

**Files:**
- Create: `src/components/chat/ChatHeader.tsx`

**Step 1: Create the custom header**

```typescript
// src/components/chat/ChatHeader.tsx
'use client';

import { useThreads } from '@/lib/threads';
import { ArrowLeft } from 'lucide-react';
import { HeaderProps } from '@copilotkit/react-ui';

export function ChatHeader({ setOpen }: HeaderProps) {
  const { getActiveThread, setView } = useThreads();
  const activeThread = getActiveThread();

  return (
    <div className="flex items-center gap-3 p-3 border-b border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-900">
      <button
        onClick={() => setView('list')}
        className="p-1.5 text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-md transition-colors"
        aria-label="Back to conversations"
      >
        <ArrowLeft className="w-5 h-5" />
      </button>
      <h2 className="flex-1 font-medium text-zinc-900 dark:text-zinc-100 truncate">
        {activeThread?.title ?? 'New conversation'}
      </h2>
    </div>
  );
}
```

**Step 2: Verify file created**

Run: `ls -la src/components/chat/`
Expected: `ChatHeader.tsx` file exists

**Step 3: Commit**

```bash
git add src/components/chat/ChatHeader.tsx
git commit -m "feat(threads): add custom ChatHeader with back navigation"
```

---

## Task 6: Create Title Generation Hook

**Files:**
- Create: `src/lib/threads/useTitleGeneration.ts`

**Step 1: Create the title generation hook**

```typescript
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
    const userMessage = visibleMessages.find((m) => m.role === 'user');
    const assistantMessage = visibleMessages.find((m) => m.role === 'assistant' && m.content);

    if (!userMessage || !assistantMessage) return;

    // Mark as generating to prevent duplicate calls
    hasGeneratedRef.current.add(activeThreadId);

    // Generate title from first exchange
    const userContent = typeof userMessage.content === 'string'
      ? userMessage.content
      : '';
    const assistantContent = typeof assistantMessage.content === 'string'
      ? assistantMessage.content.slice(0, 200)
      : '';

    // Simple title extraction: use first ~50 chars of user message
    // This avoids an extra API call. Could be enhanced with LLM call later.
    const title = userContent.length > 40
      ? userContent.slice(0, 40).trim() + '...'
      : userContent.trim() || DEFAULT_THREAD_TITLE;

    updateThreadTitle(activeThreadId, title);
  }, [visibleMessages, activeThreadId, getActiveThread, updateThreadTitle]);
}
```

**Step 2: Update barrel export**

Update `src/lib/threads/index.ts`:

```typescript
// src/lib/threads/index.ts
export * from './types';
export * from './storage';
export * from './ThreadsContext';
export * from './useTitleGeneration';
```

**Step 3: Commit**

```bash
git add src/lib/threads/useTitleGeneration.ts src/lib/threads/index.ts
git commit -m "feat(threads): add title generation hook"
```

---

## Task 7: Create Message Sync Hook

**Files:**
- Create: `src/lib/threads/useMessageSync.ts`

**Step 1: Create the message sync hook**

```typescript
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
```

**Step 2: Update barrel export**

Update `src/lib/threads/index.ts`:

```typescript
// src/lib/threads/index.ts
export * from './types';
export * from './storage';
export * from './ThreadsContext';
export * from './useTitleGeneration';
export * from './useMessageSync';
```

**Step 3: Commit**

```bash
git add src/lib/threads/useMessageSync.ts src/lib/threads/index.ts
git commit -m "feat(threads): add message sync hook for thread persistence"
```

---

## Task 8: Create DocsAssistantChat Component

**Files:**
- Create: `src/components/chat/DocsAssistantChat.tsx`

**Step 1: Create the main chat orchestrator component**

```typescript
// src/components/chat/DocsAssistantChat.tsx
'use client';

import { CopilotPopup } from '@copilotkit/react-ui';
import { useThreads, useTitleGeneration, useMessageSync } from '@/lib/threads';
import { ThreadListView } from './ThreadListView';
import { ChatHeader } from './ChatHeader';

const DOCS_INSTRUCTIONS = `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation. Be concise and helpful.`;

function ChatView() {
  // These hooks handle message persistence and title generation
  useMessageSync();
  useTitleGeneration();

  return (
    <CopilotPopup
      instructions={DOCS_INSTRUCTIONS}
      labels={{
        title: 'CUA Docs Assistant',
        initial: 'How can I help you?',
      }}
      Header={ChatHeader}
      defaultOpen={true}
      clickOutsideToClose={false}
    />
  );
}

export function DocsAssistantChat() {
  const { view, activeThreadId } = useThreads();

  // Render based on current view
  if (view === 'list' || !activeThreadId) {
    return (
      <div className="fixed bottom-4 right-4 w-[400px] h-[500px] rounded-lg shadow-xl border border-zinc-200 dark:border-zinc-700 overflow-hidden z-50">
        <ThreadListView />
      </div>
    );
  }

  return <ChatView />;
}
```

**Step 2: Create barrel export for chat components**

Create `src/components/chat/index.ts`:

```typescript
// src/components/chat/index.ts
export * from './DocsAssistantChat';
export * from './ThreadListView';
export * from './ChatHeader';
```

**Step 3: Commit**

```bash
git add src/components/chat/
git commit -m "feat(threads): add DocsAssistantChat orchestrator component"
```

---

## Task 9: Update CopilotKit Provider

**Files:**
- Modify: `src/providers/copilotkit-provider.tsx`

**Step 1: Update the provider to use threads**

Replace the entire file content:

```typescript
// src/providers/copilotkit-provider.tsx
'use client';

import { CopilotKit } from '@copilotkit/react-core';
import '@copilotkit/react-ui/styles.css';
import { ReactNode } from 'react';
import { ThreadsProvider, useThreads } from '@/lib/threads';
import { DocsAssistantChat } from '@/components/chat';

interface CopilotKitProviderProps {
  children: ReactNode;
}

function CopilotKitWithThreads({ children }: { children: ReactNode }) {
  const { activeThreadId } = useThreads();

  return (
    <CopilotKit
      runtimeUrl="/docs/api/copilotkit"
      threadId={activeThreadId ?? undefined}
    >
      {children}
      <DocsAssistantChat />
    </CopilotKit>
  );
}

export function CopilotKitProvider({ children }: CopilotKitProviderProps) {
  return (
    <ThreadsProvider>
      <CopilotKitWithThreads>
        {children}
      </CopilotKitWithThreads>
    </ThreadsProvider>
  );
}
```

**Step 2: Verify the build passes**

Run: `cd /Users/robertwendt/cua/docs && pnpm build`
Expected: Build completes without errors

**Step 3: Commit**

```bash
git add src/providers/copilotkit-provider.tsx
git commit -m "feat(threads): integrate ThreadsProvider with CopilotKit"
```

---

## Task 10: Manual Testing and Polish

**Step 1: Start dev server**

Run: `cd /Users/robertwendt/cua/docs && pnpm dev`
Expected: Server starts on http://localhost:8090

**Step 2: Test thread creation**

1. Open http://localhost:8090/docs
2. Click the chat popup
3. Verify thread list view shows "No conversations yet"
4. Click "New Chat"
5. Verify switches to chat view with back arrow

**Step 3: Test conversation persistence**

1. Send a message: "What is CUA?"
2. Wait for assistant response
3. Click back arrow to return to list
4. Verify thread appears with title (first ~40 chars of your message)
5. Click on the thread
6. Verify messages are restored

**Step 4: Test localStorage persistence**

1. Close and reopen the browser tab
2. Open chat popup
3. Verify previous conversation is still in the list

**Step 5: Test multiple threads**

1. Create 2-3 new conversations with different topics
2. Switch between them
3. Verify each maintains its own messages

**Step 6: Final commit**

```bash
git add -A
git commit -m "feat(threads): complete thread implementation with localStorage persistence"
```

---

## Summary

This implementation adds:
- Thread state management via React context
- localStorage persistence (survives browser restart)
- View toggling between thread list and chat (like messaging apps)
- Auto-generated titles from first user message
- Message sync between CopilotKit and thread storage
- Custom header with back navigation
- Thread deletion

**Files created:**
- `src/lib/threads/types.ts`
- `src/lib/threads/storage.ts`
- `src/lib/threads/ThreadsContext.tsx`
- `src/lib/threads/useTitleGeneration.ts`
- `src/lib/threads/useMessageSync.ts`
- `src/lib/threads/index.ts`
- `src/components/chat/ThreadListView.tsx`
- `src/components/chat/ChatHeader.tsx`
- `src/components/chat/DocsAssistantChat.tsx`
- `src/components/chat/index.ts`

**Files modified:**
- `src/providers/copilotkit-provider.tsx`
