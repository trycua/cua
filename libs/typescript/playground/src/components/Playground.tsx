// Main Playground component
// Composition of all playground pieces with slots for customization

import { useState, type ReactNode, useMemo } from 'react';
import { PlaygroundProvider } from '../context/PlaygroundProvider';
import { ChatProvider } from '../context/ChatProvider';
import { usePlayground, useIsChatGenerating, useFindDefaultModel } from '../hooks/usePlayground';
import { ChatPanel } from './composed/ChatPanel';
import { ChatList } from './composed/ChatList';
import { ComputerList } from './composed/ComputerList';
import { EmptyStateWithInput } from './composed/EmptyState';
import { VNCViewer } from './primitives/VNCViewer';
import { CustomComputerModal } from './modals/CustomComputerModal';
import type { PlaygroundAdapters } from '../adapters/types';
import type { Chat } from '../types';
import { cn } from '../utils/cn';

// =============================================================================
// Props
// =============================================================================

interface PlaygroundProps {
  /** Adapters for persistence, computer, and inference */
  adapters: PlaygroundAdapters;
  /** Optional custom sidebar component (replaces default ChatList) */
  sidebar?: ReactNode;
  /** Optional header component */
  header?: ReactNode;
  /** Optional footer component */
  footer?: ReactNode;
  /** Optional initial chats (for SSR or pre-loading) */
  initialChats?: Chat[];
  /** Whether to show the VNC viewer panel */
  showVNCViewer?: boolean;
  /** Whether to show the computer list in the sidebar */
  showComputerList?: boolean;
  /** Optional class name for the root container */
  className?: string;
  /** Logo config for branding */
  logo?: {
    lightSrc: string;
    darkSrc: string;
    alt: string;
  };
  /** Custom welcome message for empty state */
  welcomeMessage?: string;
}

// =============================================================================
// Internal Content Component (uses context)
// =============================================================================

function PlaygroundContentInternal({
  sidebar,
  header,
  footer,
  showVNCViewer = true,
  showComputerList = false,
  className,
  logo,
  welcomeMessage,
}: Omit<PlaygroundProps, 'adapters' | 'initialChats'>) {
  const { state, dispatch, adapters } = usePlayground();
  const [showCustomComputerModal, setShowCustomComputerModal] = useState(false);
  const defaultModel = useFindDefaultModel();

  const activeChat = state.activeChatId
    ? state.chats.find((c) => c.id === state.activeChatId)
    : null;

  const isGenerating = useIsChatGenerating(activeChat?.id ?? null);

  // Get VNC URL for current computer
  const currentComputer = state.computers.find((c) => c.id === state.currentComputerId);
  const vncUrl = currentComputer?.vncUrl;

  // Create a draft chat for the empty state UI
  const draftChat = useMemo<Chat>(
    () => ({
      id: 'draft',
      name: 'New Chat',
      messages: [],
      model: defaultModel,
      created: new Date(),
      updated: new Date(),
    }),
    [defaultModel]
  );

  // Handle creating a new chat and sending the first message
  const handleCreateAndSend = async (
    message: string,
    selectedModel?: Chat['model'],
    selectedComputer?: Chat['computer']
  ) => {
    // Use model/computer from empty state picker, or fall back to defaults
    const model = selectedModel ?? defaultModel;
    const computer =
      selectedComputer ??
      (currentComputer
        ? { id: currentComputer.id, name: currentComputer.name, url: currentComputer.agentUrl }
        : undefined);

    const newChat: Chat = {
      id: crypto.randomUUID(),
      name: message.slice(0, 50) || 'New Chat',
      messages: [],
      model,
      computer,
      created: new Date(),
      updated: new Date(),
    };

    try {
      // Save to adapter first to get the server-assigned ID
      const savedChat = await adapters.persistence.saveChat(newChat);

      dispatch({ type: 'ADD_CHAT', payload: savedChat });
      dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: savedChat.id });

      // The message will be sent by ChatPanel once it's active
      // Store the initial message to be sent
      // Note: This is handled by the ChatPanel/ChatProvider
    } catch (error) {
      console.error('Failed to create chat:', error);
    }
  };

  // Create a new chat without a message
  const handleCreateChat = async () => {
    const newChat: Chat = {
      id: crypto.randomUUID(),
      name: 'New Chat',
      messages: [],
      model: defaultModel,
      created: new Date(),
      updated: new Date(),
    };

    try {
      const savedChat = await adapters.persistence.saveChat(newChat);
      dispatch({ type: 'ADD_CHAT', payload: savedChat });
      dispatch({ type: 'SET_ACTIVE_CHAT_ID', payload: savedChat.id });
    } catch (error) {
      console.error('Failed to create chat:', error);
    }
  };

  // Show loading state
  if (state.isLoading) {
    return (
      <div className={cn('flex h-screen items-center justify-center', className)}>
        <div className="text-center">
          <div className="mb-4 h-8 w-8 animate-spin rounded-full border-4 border-neutral-300 border-t-neutral-900"></div>
          <p className="text-neutral-500">Loading...</p>
        </div>
      </div>
    );
  }

  // Show error state
  if (state.error) {
    return (
      <div className={cn('flex h-screen items-center justify-center', className)}>
        <div className="text-center">
          <p className="text-red-600">{state.error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className={cn('flex h-screen', className)}>
      {/* Sidebar slot - for custom sidebar or default */}
      {sidebar ?? (
        <aside className="flex w-64 flex-col border-r bg-white dark:bg-neutral-900">
          <ChatList onCreateChat={handleCreateChat} />
          {showComputerList && (
            <>
              <div className="border-t" />
              <ComputerList
                className="flex-1"
                onAddComputer={() => setShowCustomComputerModal(true)}
              />
            </>
          )}
        </aside>
      )}

      {/* Main content */}
      <main className="flex flex-1 flex-col">
        {header}

        <div className="flex flex-1 overflow-hidden">
          {/* Chat panel */}
          <div className={cn('flex-1', showVNCViewer && 'max-w-[50%]')}>
            {activeChat ? (
              <ChatProvider chat={activeChat} isGenerating={isGenerating}>
                <ChatPanel />
              </ChatProvider>
            ) : (
              <EmptyStateWithInput
                onCreateAndSend={handleCreateAndSend}
                isMobile={false}
                draftChat={draftChat}
                logo={logo}
                welcomeMessage={welcomeMessage}
              />
            )}
          </div>

          {/* VNC viewer */}
          {showVNCViewer && (
            <div className="w-1/2 border-l">
              {vncUrl ? (
                <VNCViewer src={vncUrl} />
              ) : (
                <div className="flex h-full items-center justify-center text-neutral-500">
                  No computer selected
                </div>
              )}
            </div>
          )}
        </div>

        {footer}
      </main>

      {/* Custom Computer Modal */}
      <CustomComputerModal
        isOpen={showCustomComputerModal}
        onClose={() => setShowCustomComputerModal(false)}
      />
    </div>
  );
}

// =============================================================================
// Main Playground Component
// =============================================================================

/**
 * Main Playground component.
 * Provides a full-featured chat interface with VNC viewer.
 *
 * @example
 * ```tsx
 * import { Playground, createLocalAdapter } from '@trycua/playground';
 *
 * const adapters = createLocalAdapter({
 *   computerServerUrl: 'http://localhost:8443',
 *   providerApiKeys: { anthropic: 'sk-...' },
 * });
 *
 * function App() {
 *   return <Playground adapters={adapters} />;
 * }
 * ```
 */
export function Playground({ adapters, initialChats, ...props }: PlaygroundProps) {
  return (
    <PlaygroundProvider adapters={adapters} initialChats={initialChats}>
      <PlaygroundContentInternal {...props} />
    </PlaygroundProvider>
  );
}
