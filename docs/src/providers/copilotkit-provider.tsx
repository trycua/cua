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
    <CopilotKit runtimeUrl="/docs/api/copilotkit" showDevConsole={false}>
      <style>{`
        .copilotKitHeader {
          display: flex;
          align-items: center;
          gap: 8px;
        }
        .copilotKitHeader::before {
          content: '';
          display: inline-block;
          width: 24px;
          height: 24px;
          background-image: url('/docs/img/cuala-icon.svg');
          background-size: contain;
          background-repeat: no-repeat;
          background-position: center;
        }
      `}</style>
      {children}
      <CopilotPopup
        instructions={DOCS_INSTRUCTIONS}
        labels={{
          title: 'CUA Docs Assistant (experimental)',
          initial: 'How can I help you?',
        }}
      />
    </CopilotKit>
  );
}
