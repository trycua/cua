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
