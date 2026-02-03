// Chat list component
// Adapted from cloud/src/website/app/components/playground/panels/ChatList.tsx

import { useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { Calendar, EllipsisVertical, SquarePen, Trash, Download } from 'lucide-react';
import { usePlayground } from '../../hooks/usePlayground';
import type { Chat } from '../../types';
import { cn } from '../../utils/cn';

// Simple date formatting (avoiding date-fns dependency)
function formatDistanceToNow(date: Date): string {
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m`;
  if (diffHours < 24) return `${diffHours}h`;
  if (diffDays < 30) return `${diffDays}d`;
  return date.toLocaleDateString();
}

interface ChatListProps {
  /** Optional class name for styling */
  className?: string;
  /** Optional callback for creating a new chat */
  onCreateChat?: () => void;
  /** Optional callback for exporting a chat */
  onExportChat?: (chat: Chat) => void;
}

export function ChatList({ className, onCreateChat, onExportChat }: ChatListProps) {
  const { state, dispatch, adapters } = usePlayground();
  const [menuOpenId, setMenuOpenId] = useState<string | null>(null);

  const createChat = async () => {
    const newChat: Chat = {
      id: crypto.randomUUID(),
      name: 'New Chat',
      messages: [],
      created: new Date(),
      updated: new Date(),
    };

    dispatch({ type: 'ADD_CHAT', payload: newChat });
    dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: newChat.id });

    // Persist to adapter
    try {
      await adapters.persistence.saveChat(newChat);
    } catch (error) {
      console.error('Failed to save chat:', error);
    }

    onCreateChat?.();
  };

  const deleteChat = async (chat: Chat) => {
    dispatch({ type: 'DELETE_CHAT', payload: chat.id });

    // Persist to adapter
    try {
      await adapters.persistence.deleteChat(chat.id);
    } catch (error) {
      console.error('Failed to delete chat:', error);
    }
  };

  const setActiveChat = (chat: Chat | null) => {
    dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: chat?.id || null });
  };

  const sortedChats = [...state.chats].sort((a, b) => b.updated.getTime() - a.updated.getTime());

  return (
    <div className={cn('flex h-full flex-col', className)}>
      {/* Header */}
      <div className="flex items-center justify-between border-b px-4 py-3">
        <h2 className="font-semibold">Chats</h2>
        <button
          type="button"
          onClick={createChat}
          className="rounded-md border p-2 hover:bg-neutral-50 dark:hover:bg-neutral-800"
          title="New Chat"
        >
          <SquarePen className="h-4 w-4" />
        </button>
      </div>

      {/* Chat List */}
      <ul className="flex-1 overflow-auto p-2">
        <AnimatePresence>
          {sortedChats.map((chat) => (
            <motion.li
              key={chat.id}
              layout
              initial={{ opacity: 0, y: -20 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, x: -100 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              className="mb-2"
            >
              <div
                onClick={() => setActiveChat(chat)}
                className={cn(
                  'flex w-full cursor-pointer gap-2 rounded-lg border px-3 py-2 transition hover:bg-neutral-50 dark:hover:bg-neutral-800',
                  state.activeChatId === chat.id &&
                    'border-blue-500 bg-neutral-50 dark:bg-neutral-800'
                )}
              >
                <div className="flex w-full gap-2 text-left">
                  <div className="flex flex-1 flex-col gap-1 overflow-hidden">
                    <p className="truncate">{chat.name}</p>
                    <div className="flex items-center gap-1 text-xs text-neutral-500 dark:text-neutral-400">
                      <Calendar className="h-3 w-3" />
                      {formatDistanceToNow(chat.updated)} ago
                    </div>
                  </div>

                  {/* Dropdown Menu */}
                  <div className="relative">
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        setMenuOpenId(menuOpenId === chat.id ? null : chat.id);
                      }}
                      className="rounded-md p-1 hover:bg-neutral-200 dark:hover:bg-neutral-600"
                    >
                      <EllipsisVertical className="h-4 w-4" />
                    </button>

                    {menuOpenId === chat.id && (
                      <div className="absolute right-0 z-10 mt-1 w-32 rounded-md border bg-white shadow-lg dark:bg-neutral-800">
                        {onExportChat && (
                          <button
                            type="button"
                            onClick={(e) => {
                              e.stopPropagation();
                              onExportChat(chat);
                              setMenuOpenId(null);
                            }}
                            className="flex w-full items-center gap-2 px-3 py-2 text-sm hover:bg-neutral-50 dark:hover:bg-neutral-700"
                          >
                            <Download className="h-4 w-4" /> Export
                          </button>
                        )}
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            deleteChat(chat);
                            setMenuOpenId(null);
                          }}
                          className="flex w-full items-center gap-2 px-3 py-2 text-sm text-red-600 hover:bg-neutral-50 dark:text-red-400 dark:hover:bg-neutral-700"
                        >
                          <Trash className="h-4 w-4" /> Delete
                        </button>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            </motion.li>
          ))}
        </AnimatePresence>

        {sortedChats.length === 0 && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <p className="text-neutral-500 dark:text-neutral-400">
              No chats yet. Create a new chat to get started.
            </p>
          </div>
        )}
      </ul>
    </div>
  );
}
