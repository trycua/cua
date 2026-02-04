// Main playground content component
// Adapted from cloud/src/website/app/components/playground/PlaygroundContent.tsx

import { AnimatePresence, motion } from 'motion/react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { EmptyStateWithInput } from './EmptyState';
import { PlaygroundLayout } from './PlaygroundLayout';
import { ChatProvider } from '../../context/ChatProvider';
import {
  useActiveChat,
  useFindDefaultModel,
  useIsChatGenerating,
  usePlayground,
} from '../../hooks/usePlayground';
import type { Chat, Computer } from '../../types';

// Re-export for convenience
export { EmptyStateWithInput } from './EmptyState';
export { ExamplePrompts, EXAMPLE_PROMPTS, type ExamplePrompt } from './ExamplePrompts';
export { PlaygroundLayout } from './PlaygroundLayout';

interface PlaygroundContentProps {
  /** Content to render when a chat is active */
  renderChatContent: (props: {
    chat: Chat;
    isGenerating: boolean;
    isMobile: boolean;
    shouldAutoSendFirstMessage: boolean;
    onAutoSendComplete: () => void;
  }) => React.ReactNode;

  /** Whether dark mode is enabled */
  isDarkMode?: boolean;
  /** Callback to toggle theme */
  onToggleTheme?: () => void;
  /** Render function for custom link */
  renderLink?: (props: {
    to: string;
    children: React.ReactNode;
    className?: string;
  }) => React.ReactNode;
  /** Custom back link URL */
  backLinkUrl?: string;
  /** Custom back link text */
  backLinkText?: string;
  /** Whether to show the preview badge */
  showPreviewBadge?: boolean;

  /** Callback for export action */
  onExportChat?: (chat: Chat) => void;
  /** Callback for replay action */
  onReplayChat?: (chat: Chat) => void;

  /** Optional loading bar component */
  loadingBar?: React.ReactNode;
  /** Optional sidebar skeleton component */
  sidebarSkeleton?: React.ReactNode;
  /** Optional VNC overlay panel */
  vncOverlayPanel?: React.ReactNode;
  /** Whether chats are loading */
  chatsLoading?: boolean;

  /** Optional settings modal */
  settingsModal?: React.ReactNode;

  /** Logo config for empty state branding */
  logo?: {
    lightSrc: string;
    darkSrc: string;
    alt: string;
  };
  /** Custom welcome message for empty state */
  welcomeMessage?: string;
  /** Custom hint message for empty state */
  selectionHint?: string;

  /** Optional telemetry callback for page view */
  onPlaygroundViewed?: () => void;
  /** Optional telemetry callback when example prompt is selected */
  onExamplePromptSelected?: (promptId: string, promptTitle: string) => void;
  /** Optional toast callback */
  onToast?: (message: string, type?: 'success' | 'error' | 'info') => void;
}

/**
 * Main content component for the playground - handles all the business logic
 */
export function PlaygroundContent({
  renderChatContent,
  isDarkMode,
  onToggleTheme,
  renderLink,
  backLinkUrl,
  backLinkText,
  showPreviewBadge,
  onExportChat,
  onReplayChat,
  loadingBar,
  sidebarSkeleton,
  vncOverlayPanel,
  chatsLoading = false,
  settingsModal,
  logo,
  welcomeMessage,
  selectionHint,
  onPlaygroundViewed,
  onExamplePromptSelected,
  onToast,
}: PlaygroundContentProps) {
  const { state, dispatch, adapters } = usePlayground();
  const activeChat = useActiveChat();
  const isActiveChatGenerating = useIsChatGenerating(activeChat?.id ?? null);
  const defaultModel = useFindDefaultModel();
  const [isMobile, setIsMobile] = useState(false);
  // Track chat IDs that need to auto-send their first message
  const pendingFirstMessageRef = useRef<Set<string>>(new Set());

  const { initialized, computers } = state;

  // Track playground page view for analytics (optional)
  useEffect(() => {
    onPlaygroundViewed?.();
  }, [onPlaygroundViewed]);

  // Check if mobile on mount
  useEffect(() => {
    const checkMobile = () => {
      setIsMobile(window.innerWidth < 768);
    };
    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  // Auto-select model for active chat if missing
  useEffect(() => {
    if (!initialized || !activeChat || activeChat.model) return;
    if (defaultModel) {
      dispatch({
        type: 'UPDATE_CHAT',
        payload: { id: activeChat.id, updates: { model: defaultModel } },
      });
    }
  }, [initialized, activeChat, defaultModel, dispatch]);

  // Create a new chat and send the first message
  const handleCreateChatWithMessage = async (message: string) => {
    // Find a running computer first, then fall back to first computer
    const runningComputer = computers.find((c) => c.status === 'running');
    const computerInfo = runningComputer ?? computers[0];

    if (!computerInfo) {
      onToast?.('Please select a sandbox to interact with.', 'error');
      return;
    }

    // Convert ComputerInfo to Computer
    const computer: Computer = {
      id: computerInfo.id,
      name: computerInfo.name,
      url: computerInfo.agentUrl,
    };

    try {
      // Create chat title from first message
      const base = message.trim().replace(/\s+/g, ' ');
      const maxLen = 60;
      const title = base.length > maxLen ? `${base.slice(0, maxLen)}...` : base || 'New Chat';

      // Create the first message
      const userMessage = {
        content: message,
        role: 'user' as const,
        type: 'message' as const,
      };

      const newChat: Chat = {
        id: crypto.randomUUID(),
        name: title,
        messages: [userMessage],
        computer,
        model: defaultModel,
        created: new Date(),
        updated: new Date(),
      };

      // Save via adapter (may return chat with server-assigned ID)
      const savedChat = await adapters.persistence.saveChat(newChat);

      // Add chat to state and set as active
      dispatch({ type: 'ADD_CHAT', payload: savedChat });
      dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: savedChat.id });

      // Mark this chat as needing to auto-send its first message
      pendingFirstMessageRef.current.add(savedChat.id);

      // Persist the first message
      adapters.persistence.saveMessages(savedChat.id, [userMessage]).catch(console.error);
    } catch (error) {
      console.error('Failed to create chat:', error);
      onToast?.('Failed to create chat', 'error');
    }
  };

  // Create a draft chat for the empty state (not persisted until first message)
  const draftChat = useMemo<Chat>(() => {
    const runningComputer = computers.find((c) => c.status === 'running');
    const computerInfo = runningComputer ?? computers[0];

    // Convert ComputerInfo to Computer if available
    const computer: Computer | undefined = computerInfo
      ? {
          id: computerInfo.id,
          name: computerInfo.name,
          url: computerInfo.agentUrl,
        }
      : undefined;

    return {
      id: 'draft',
      name: 'New Chat',
      messages: [],
      computer,
      model: defaultModel,
      created: new Date(),
      updated: new Date(),
    };
  }, [computers, defaultModel]);

  // Check if active chat needs to auto-send its first message
  const shouldAutoSend = activeChat ? pendingFirstMessageRef.current.has(activeChat.id) : false;

  const clearPendingFirstMessage = (chatId: string) => {
    pendingFirstMessageRef.current.delete(chatId);
  };

  // Render content based on whether there's an active chat
  const renderContent = () => {
    return (
      <AnimatePresence mode="wait">
        {!activeChat ? (
          <EmptyStateWithInput
            key="empty-state"
            onCreateAndSend={handleCreateChatWithMessage}
            isMobile={isMobile}
            draftChat={draftChat}
            logo={logo}
            welcomeMessage={welcomeMessage}
            selectionHint={selectionHint}
            onExamplePromptSelected={onExamplePromptSelected}
            onToast={onToast}
          />
        ) : (
          <motion.div
            key={`chat-${activeChat.id}`}
            className="flex flex-1 flex-col overflow-hidden"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.2 }}
          >
            <ChatProvider chat={activeChat} isGenerating={isActiveChatGenerating}>
              {renderChatContent({
                chat: activeChat,
                isGenerating: isActiveChatGenerating,
                isMobile,
                shouldAutoSendFirstMessage: shouldAutoSend,
                onAutoSendComplete: () => clearPendingFirstMessage(activeChat.id),
              })}
            </ChatProvider>
          </motion.div>
        )}
      </AnimatePresence>
    );
  };

  return (
    <>
      <PlaygroundLayout
        isDarkMode={isDarkMode}
        onToggleTheme={onToggleTheme}
        renderLink={renderLink}
        backLinkUrl={backLinkUrl}
        backLinkText={backLinkText}
        showPreviewBadge={showPreviewBadge}
        onExportChat={onExportChat}
        onReplayChat={onReplayChat}
        loadingBar={loadingBar}
        sidebarSkeleton={sidebarSkeleton}
        vncOverlayPanel={vncOverlayPanel}
        chatsLoading={chatsLoading}
        onToast={onToast}
      >
        {renderContent()}
      </PlaygroundLayout>

      {settingsModal}
    </>
  );
}
