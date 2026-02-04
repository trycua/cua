// ChatSidebar component
// Adapted from cloud/src/website/app/components/playground/ChatSidebar.tsx

import { Download, Loader2, MoreVertical, Play, Plus, Trash } from 'lucide-react';
import { useCallback, useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../ui/dropdown-menu';
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip';
import {
  useActiveChat,
  useFindDefaultModel,
  useIsChatGenerating,
  usePlayground,
} from '../../hooks/usePlayground';
import type { Chat, Computer } from '../../types';
import { cn } from '../../utils/cn';

interface ChatSidebarProps {
  /** Custom class name */
  className?: string;
  /** Callback when a chat is selected (for mobile - close sidebar after selection) */
  onChatSelect?: () => void;
  /** Callback for exporting a chat */
  onExportChat?: (chat: Chat) => void;
  /** Callback for replaying a chat */
  onReplayChat?: (chat: Chat) => void;
  /** Optional toast callback */
  onToast?: (message: string) => void;
}

export function ChatSidebar({
  className,
  onChatSelect,
  onExportChat,
  onReplayChat,
  onToast,
}: ChatSidebarProps) {
  const { state, dispatch, adapters } = usePlayground();
  const { chats, computers, activeChatId } = state;
  const activeChat = useActiveChat();
  const defaultModel = useFindDefaultModel();
  const [isCreating, setIsCreating] = useState(false);
  const [deleteConfirmChat, setDeleteConfirmChat] = useState<Chat | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  // Use global generating state (persists across chat switches)
  const isGenerating = useIsChatGenerating(activeChatId);
  const [loadingChatId, setLoadingChatId] = useState<string | null>(null);

  // Track loaded chats to avoid re-fetching
  const [loadedChatIds, setLoadedChatIds] = useState<Set<string>>(new Set());

  const createChat = useCallback(async () => {
    if (isCreating) return;
    setIsCreating(true);

    try {
      // Get first computer if available
      const computer = computers.length > 0 ? computers[0] : undefined;
      let computerAsChat: Computer | undefined;
      if (computer) {
        const { agentUrl, ...rest } = computer;
        computerAsChat = {
          ...rest,
          url: agentUrl,
        } as Computer;
      }

      // Create local chat object
      const newChat: Chat = {
        id: crypto.randomUUID(), // Temporary ID, adapter may replace it
        name: 'New Chat',
        messages: [],
        computer: computerAsChat,
        model: defaultModel,
        created: new Date(),
        updated: new Date(),
      };

      // Save via adapter (may return chat with server-assigned ID)
      const savedChat = await adapters.persistence.saveChat(newChat);

      dispatch({ type: 'ADD_CHAT', payload: savedChat });
      dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: savedChat.id });

      // Mark as loaded since it's a new empty chat
      setLoadedChatIds((prev) => new Set([...prev, savedChat.id]));
    } catch (error) {
      console.error('Failed to create chat:', error);
      const message = error instanceof Error ? error.message : 'Unknown error occurred';
      onToast?.(`Failed to create chat: ${message}`);
    } finally {
      setIsCreating(false);
    }
  }, [computers, defaultModel, dispatch, isCreating, adapters.persistence, onToast]);

  const handleDeleteConfirm = async () => {
    if (!deleteConfirmChat) return;

    setIsDeleting(true);
    try {
      await adapters.persistence.deleteChat(deleteConfirmChat.id);
      dispatch({ type: 'DELETE_CHAT', payload: deleteConfirmChat.id });
      setDeleteConfirmChat(null);
    } catch (error) {
      console.error('Failed to delete chat from DB:', error);
      const message = error instanceof Error ? error.message : 'Unknown error occurred';
      onToast?.(`Failed to delete chat: ${message}`);
    } finally {
      setIsDeleting(false);
    }
  };

  const handleChatClick = async (chat: Chat) => {
    // Set active chat immediately for responsive UI
    dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: chat.id });

    // Only fetch from DB if chat hasn't been loaded yet (cold start/page refresh).
    // During a session, messages are kept in global state via setMessages(),
    // so we don't need to re-fetch on every chat switch.
    if (!loadedChatIds.has(chat.id)) {
      setLoadingChatId(chat.id);

      const loadMessages = async () => {
        try {
          if (adapters.persistence.loadChat) {
            const fullChat = await adapters.persistence.loadChat(chat.id);
            if (fullChat) {
              dispatch({
                type: 'SET_CHAT_MESSAGES',
                payload: { id: chat.id, messages: fullChat.messages || [] },
              });
              setLoadedChatIds((prev) => new Set([...prev, chat.id]));
            }
          }
        } catch (error) {
          console.error('Failed to load chat messages:', error);
          const message = error instanceof Error ? error.message : 'Unknown error occurred';
          onToast?.(`Failed to load chat: ${message}`);
        }
      };

      await loadMessages();
      setLoadingChatId(null);
    }

    onChatSelect?.();
  };

  const handleNewChatClick = () => {
    // Don't create a new chat if the current chat is already blank
    if (activeChat && (!activeChat.messages || activeChat.messages.length === 0)) {
      return;
    }
    createChat();
    onChatSelect?.();
  };

  // Determine button disabled state and reason
  const isButtonDisabled = isGenerating || isCreating;
  const disabledReason = isGenerating ? 'Wait for the current task to complete' : null;

  const newChatButton = (
    <button
      type="button"
      onClick={handleNewChatClick}
      disabled={isButtonDisabled}
      className="mb-6 flex w-full items-center justify-center gap-2 rounded-md border border-neutral-200 bg-white px-3 py-2 text-neutral-600 text-sm hover:bg-neutral-50 hover:text-neutral-900 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
    >
      {isCreating ? (
        <>
          <Loader2 className="h-4 w-4 animate-spin" />
          Creating...
        </>
      ) : (
        <>
          <Plus className="h-4 w-4" />
          New Chat
        </>
      )}
    </button>
  );

  const sortedChats = [...chats].sort((a, b) => b.updated.getTime() - a.updated.getTime());

  return (
    <div className={cn('flex flex-col p-4', className)}>
      {disabledReason ? (
        <Tooltip>
          <TooltipTrigger asChild>{newChatButton}</TooltipTrigger>
          <TooltipContent>
            <p>{disabledReason}</p>
          </TooltipContent>
        </Tooltip>
      ) : (
        newChatButton
      )}

      <div className="space-y-1">
        <div className="mb-3 px-2 font-medium text-neutral-500 text-xs dark:text-neutral-400">
          Task History
        </div>
        {sortedChats.map((chat) => (
          <div
            key={chat.id}
            className={`group flex items-center justify-between rounded-md px-2 py-1.5 text-sm transition-colors ${
              activeChat?.id === chat.id
                ? 'bg-neutral-200 text-neutral-900 dark:bg-neutral-800 dark:text-white'
                : 'text-neutral-600 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800'
            }`}
          >
            <button
              type="button"
              onClick={() => handleChatClick(chat)}
              className="flex flex-1 items-center gap-2 truncate text-left"
            >
              {loadingChatId === chat.id && (
                <Loader2 className="h-3 w-3 flex-shrink-0 animate-spin" />
              )}
              <span className="truncate">{chat.name}</span>
            </button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="opacity-0 transition-opacity group-hover:opacity-100"
                  onClick={(e) => e.stopPropagation()}
                >
                  <MoreVertical className="h-4 w-4 text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-300" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {onReplayChat && (
                  <DropdownMenuItem
                    onClick={(e) => {
                      e.stopPropagation();
                      onReplayChat(chat);
                    }}
                  >
                    <Play className="mr-2 h-4 w-4" />
                    Replay
                  </DropdownMenuItem>
                )}
                {onExportChat && (
                  <DropdownMenuItem
                    onClick={(e) => {
                      e.stopPropagation();
                      onExportChat(chat);
                    }}
                  >
                    <Download className="mr-2 h-4 w-4" />
                    Export
                  </DropdownMenuItem>
                )}
                {(onReplayChat || onExportChat) && <DropdownMenuSeparator />}
                <DropdownMenuItem
                  onClick={(e) => {
                    e.stopPropagation();
                    setDeleteConfirmChat(chat);
                  }}
                  className="text-red-600 focus:text-red-600 dark:text-red-400 dark:focus:text-red-400"
                >
                  <Trash className="mr-2 h-4 w-4" />
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ))}

        {sortedChats.length === 0 && (
          <div className="px-2 py-4 text-center text-neutral-500 text-sm dark:text-neutral-400">
            No chats yet
          </div>
        )}
      </div>

      <Dialog
        open={!!deleteConfirmChat}
        onOpenChange={(open) => !open && setDeleteConfirmChat(null)}
      >
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Delete Chat</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete &ldquo;
              {deleteConfirmChat?.name || 'this chat'}&rdquo;? This action cannot be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              type="button"
              onClick={() => setDeleteConfirmChat(null)}
              disabled={isDeleting}
              className="rounded-md border border-neutral-200 bg-white px-4 py-2 text-neutral-700 text-sm transition-colors hover:bg-neutral-50 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-300 dark:hover:bg-neutral-700"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleDeleteConfirm}
              disabled={isDeleting}
              className="rounded-md bg-red-600 px-4 py-2 text-sm text-white transition-colors hover:bg-red-700 disabled:opacity-50"
            >
              {isDeleting ? 'Deleting...' : 'Delete'}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
