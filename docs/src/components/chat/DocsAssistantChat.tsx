// src/components/chat/DocsAssistantChat.tsx
'use client';

import { CopilotPopup } from '@copilotkit/react-ui';
import { useThreads, useTitleGeneration, useMessageSync } from '@/lib/threads';
import { ThreadListView } from './ThreadListView';
import { ChatHeader } from './ChatHeader';
import { QueuedInput } from './QueuedInput';
import { PendingMessageProvider } from './PendingMessageContext';
import { OptimisticMessages } from './OptimisticMessages';

const DOCS_INSTRUCTIONS = `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation. Be concise and helpful.`;

function ChatView() {
  // These hooks handle message persistence and title generation
  useMessageSync();
  useTitleGeneration();

  return (
    <PendingMessageProvider>
      <CopilotPopup
        instructions={DOCS_INSTRUCTIONS}
        labels={{
          title: 'CUA Docs Assistant',
          initial: 'How can I help you?',
        }}
        Header={ChatHeader}
        Input={QueuedInput}
        Messages={OptimisticMessages}
        defaultOpen={true}
        clickOutsideToClose={false}
      />
    </PendingMessageProvider>
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
