'use client';

import { CopilotKit, useRenderToolCall } from '@copilotkit/react-core';
import { CopilotPopup, AssistantMessage as DefaultAssistantMessage } from '@copilotkit/react-ui';
import '@copilotkit/react-ui/styles.css';
import { ReactNode, useMemo } from 'react';

const DOCS_INSTRUCTIONS = `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation. Be concise and helpful.

IMPORTANT: When responding to users, use clear paragraph breaks between different sections of your response. Do not run sentences together. Use proper formatting with line breaks between paragraphs.`;

interface CopilotKitProviderProps {
  children: ReactNode;
}

// Fix text concatenation by adding proper spacing between segments
function fixTextConcatenation(content: string): string {
  if (!content) return content;

  // Pattern to detect sentences that run together (period/colon followed immediately by capital letter)
  // Examples: "documentation.Based" -> "documentation.\n\nBased"
  //           "benchmarks:Let" -> "benchmarks:\n\nLet"
  let fixed = content
    // Fix period followed by capital letter with no space
    .replace(/\.([A-Z])/g, '.\n\n$1')
    // Fix colon followed by capital letter with no space (usually indicates new section)
    .replace(/:([A-Z])/g, ':\n\n$1')
    // Fix exclamation/question mark followed by capital letter with no space
    .replace(/([!?])([A-Z])/g, '$1\n\n$2');

  return fixed;
}

// Custom AssistantMessage component that fixes text concatenation
function CustomAssistantMessage(props: React.ComponentProps<typeof DefaultAssistantMessage>) {
  const { message, ...rest } = props;

  // Process the message content to fix concatenation
  const processedMessage = useMemo(() => {
    if (!message) return message;

    // Handle string content
    if (typeof message.content === 'string') {
      return {
        ...message,
        content: fixTextConcatenation(message.content),
      };
    }

    // Handle array content (multipart messages)
    if (Array.isArray(message.content)) {
      return {
        ...message,
        content: message.content.map((part: unknown) => {
          if (typeof part === 'string') {
            return fixTextConcatenation(part);
          }
          if (part && typeof part === 'object' && 'text' in part && typeof (part as { text?: unknown }).text === 'string') {
            return {
              ...part,
              text: fixTextConcatenation((part as { text: string }).text),
            };
          }
          return part;
        }),
      };
    }

    return message;
  }, [message]);

  return <DefaultAssistantMessage {...rest} message={processedMessage} />;
}

// Component to render tool call indicators
function ToolCallIndicators() {
  // Render indicator for search_docs tool
  useRenderToolCall({
    name: 'search_docs',
    description: 'Searches the CUA documentation',
    parameters: [
      { name: 'query', type: 'string', description: 'Search query', required: true },
    ],
    render: ({ status, args }) => {
      if (status === 'inProgress') {
        return (
          <div className="copilotkit-tool-indicator">
            <div className="copilotkit-tool-indicator-icon">
              <svg className="copilotkit-spinner" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4"></circle>
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
              </svg>
            </div>
            <span className="copilotkit-tool-indicator-text">
              Searching documentation{args?.query ? `: "${args.query}"` : '...'}
            </span>
          </div>
        );
      }
      return null;
    },
  });

  // Render indicator for sql_query tool
  useRenderToolCall({
    name: 'sql_query',
    description: 'Queries the documentation database',
    parameters: [
      { name: 'query', type: 'string', description: 'SQL query', required: true },
    ],
    render: ({ status }) => {
      if (status === 'inProgress') {
        return (
          <div className="copilotkit-tool-indicator">
            <div className="copilotkit-tool-indicator-icon">
              <svg className="copilotkit-spinner" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4"></circle>
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
              </svg>
            </div>
            <span className="copilotkit-tool-indicator-text">
              Fetching detailed documentation...
            </span>
          </div>
        );
      }
      return null;
    },
  });

  return null;
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

        /* Tool call indicator styles */
        .copilotkit-tool-indicator {
          display: flex;
          align-items: center;
          gap: 8px;
          padding: 8px 12px;
          margin: 8px 0;
          background: linear-gradient(135deg, #f0f9ff 0%, #e0f2fe 100%);
          border: 1px solid #bae6fd;
          border-radius: 8px;
          font-size: 13px;
          color: #0369a1;
          animation: fadeIn 0.2s ease-out;
        }

        .dark .copilotkit-tool-indicator {
          background: linear-gradient(135deg, #0c4a6e 0%, #075985 100%);
          border-color: #0369a1;
          color: #7dd3fc;
        }

        .copilotkit-tool-indicator-icon {
          display: flex;
          align-items: center;
          justify-content: center;
          width: 18px;
          height: 18px;
        }

        .copilotkit-tool-indicator-text {
          flex: 1;
          font-weight: 500;
        }

        .copilotkit-spinner {
          width: 18px;
          height: 18px;
          animation: spin 1s linear infinite;
        }

        @keyframes spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }

        @keyframes fadeIn {
          from { opacity: 0; transform: translateY(-4px); }
          to { opacity: 1; transform: translateY(0); }
        }

        /* Fix text spacing in assistant messages */
        .copilotKitAssistantMessage p {
          margin-bottom: 1em;
        }

        .copilotKitAssistantMessage p:last-child {
          margin-bottom: 0;
        }

        /* Ensure proper spacing between text blocks */
        .copilotKitAssistantMessage > div > p + p {
          margin-top: 1em;
        }
      `}</style>
      <ToolCallIndicators />
      {children}
      <CopilotPopup
        instructions={DOCS_INSTRUCTIONS}
        labels={{
          title: 'CUA Docs Assistant (experimental)',
          initial: 'How can I help you?',
        }}
        AssistantMessage={CustomAssistantMessage}
      />
    </CopilotKit>
  );
}
