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
