'use client';

import { CopilotKit } from '@copilotkit/react-core';
import { CopilotPopup } from '@copilotkit/react-ui';
import '@copilotkit/react-ui/styles.css';
import { ReactNode } from 'react';

const DOCS_INSTRUCTIONS = `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation. Be concise and helpful.`;

interface CopilotKitProviderProps {
  children: ReactNode;
}

export function CopilotKitProvider({ children }: CopilotKitProviderProps) {
  return (
    <CopilotKit runtimeUrl="/docs/api/copilotkit">
      {children}
      <CopilotPopup
        instructions={DOCS_INSTRUCTIONS}
        labels={{
          title: 'CUA Docs Assistant',
          initial: 'How can I help you?',
        }}
      />
    </CopilotKit>
  );
}
