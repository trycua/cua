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
