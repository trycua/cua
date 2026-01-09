'use client';

// IMPORTANT: Apply fetch patch BEFORE any CopilotKit imports
// This patches window.fetch to cache info requests and prevent the infinite loop bug
import { applyCopilotKitFetchPatch } from '@/lib/copilotkit-fetch-patch';

// Apply patch immediately on module load (browser only)
if (typeof window !== 'undefined') {
  applyCopilotKitFetchPatch();
}

import { CopilotKit, useRenderToolCall, useCopilotChat } from '@copilotkit/react-core';
import {
  CopilotPopup,
  AssistantMessage as DefaultAssistantMessage,
  useChatContext,
} from '@copilotkit/react-ui';
import '@copilotkit/react-ui/styles.css';
import { ReactNode, useMemo, useState, useCallback, useEffect, useRef, memo } from 'react';
import posthog from 'posthog-js';

const DOCS_INSTRUCTIONS = `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation. Be concise and helpful.

IMPORTANT: When responding to users, use clear paragraph breaks between different sections of your response. Do not run sentences together. Use proper formatting with line breaks between paragraphs.`;

interface CopilotKitProviderProps {
  children: ReactNode;
}

// Enable CopilotKit - the fetch patch handles request deduplication
const COPILOTKIT_ENABLED = true;

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
      const contentArray = message.content as unknown[];
      return {
        ...message,
        content: contentArray.map((part: unknown) => {
          if (typeof part === 'string') {
            return fixTextConcatenation(part);
          }
          if (
            part &&
            typeof part === 'object' &&
            'text' in part &&
            typeof (part as { text?: unknown }).text === 'string'
          ) {
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

  return <DefaultAssistantMessage {...rest} message={processedMessage as typeof message} />;
}

// Component to render tool call indicators
function ToolCallIndicators() {
  // Render indicator for search_docs tool
  useRenderToolCall({
    name: 'search_docs',
    description: 'Searches the CUA documentation',
    parameters: [{ name: 'query', type: 'string', description: 'Search query', required: true }],
    render: ({ status, args }) => {
      if (status === 'inProgress') {
        return (
          <div className="copilotkit-tool-indicator">
            <div className="copilotkit-tool-indicator-icon">
              <svg
                className="copilotkit-spinner"
                viewBox="0 0 24 24"
                fill="none"
                xmlns="http://www.w3.org/2000/svg"
              >
                <circle
                  className="opacity-25"
                  cx="12"
                  cy="12"
                  r="10"
                  stroke="currentColor"
                  strokeWidth="4"
                ></circle>
                <path
                  className="opacity-75"
                  fill="currentColor"
                  d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                ></path>
              </svg>
            </div>
            <span className="copilotkit-tool-indicator-text">
              Searching documentation{args?.query ? `: "${args.query}"` : '...'}
            </span>
          </div>
        );
      }
      return <></>;
    },
  });

  // Render indicator for sql_query tool
  useRenderToolCall({
    name: 'sql_query',
    description: 'Queries the documentation database',
    parameters: [{ name: 'query', type: 'string', description: 'SQL query', required: true }],
    render: ({ status }) => {
      if (status === 'inProgress') {
        return (
          <div className="copilotkit-tool-indicator">
            <div className="copilotkit-tool-indicator-icon">
              <svg
                className="copilotkit-spinner"
                viewBox="0 0 24 24"
                fill="none"
                xmlns="http://www.w3.org/2000/svg"
              >
                <circle
                  className="opacity-25"
                  cx="12"
                  cy="12"
                  r="10"
                  stroke="currentColor"
                  strokeWidth="4"
                ></circle>
                <path
                  className="opacity-75"
                  fill="currentColor"
                  d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                ></path>
              </svg>
            </div>
            <span className="copilotkit-tool-indicator-text">
              Fetching detailed documentation...
            </span>
          </div>
        );
      }
      return <></>;
    },
  });

  return null;
}

// Copy icon component - exact match of CopilotKit's internal icon
function CopyIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      fill="none"
      viewBox="0 0 24 24"
      strokeWidth="2"
      stroke="currentColor"
      width="16"
      height="16"
      style={{ minWidth: '16px', minHeight: '16px' }}
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M15.75 17.25v3.375c0 .621-.504 1.125-1.125 1.125h-9.75a1.125 1.125 0 01-1.125-1.125V7.875c0-.621.504-1.125 1.125-1.125H6.75a9.06 9.06 0 011.5.124m7.5 10.376h3.375c.621 0 1.125-.504 1.125-1.125V11.25c0-4.46-3.243-8.161-7.5-8.876a9.06 9.06 0 00-1.5-.124H9.375c-.621 0-1.125.504-1.125 1.125v3.5m7.5 10.375H9.375a1.125 1.125 0 01-1.125-1.125v-9.25m12 6.625v-1.875a3.375 3.375 0 00-3.375-3.375h-1.5a1.125 1.125 0 01-1.125-1.125v-1.5a3.375 3.375 0 00-3.375-3.375H9.75"
      />
    </svg>
  );
}

// Check icon for copied state - exact match of CopilotKit's internal icon
function CheckIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      fill="none"
      viewBox="0 0 24 24"
      strokeWidth="2"
      stroke="currentColor"
      width="16"
      height="16"
      style={{ minWidth: '16px', minHeight: '16px' }}
    >
      <path strokeLinecap="round" strokeLinejoin="round" d="M4.5 12.75l6 6 9-13.5" />
    </svg>
  );
}

// Helper function to extract messages from the DOM
function extractMessagesFromDOM(): string {
  // Find the CopilotKit messages container
  const messagesContainer = document.querySelector('.copilotKitMessages');
  if (!messagesContainer) {
    console.log('No messages container found');
    return '';
  }

  const messages: string[] = [];

  // Find all message elements - CopilotKit uses specific class patterns
  const allElements = messagesContainer.querySelectorAll(
    '[class*="copilotKitUserMessage"], [class*="copilotKitAssistantMessage"]'
  );

  allElements.forEach((element) => {
    const className = element.className;
    const isUser = className.includes('copilotKitUserMessage');
    const isAssistant = className.includes('copilotKitAssistantMessage');

    if (isUser || isAssistant) {
      const role = isUser ? '**User:**' : '**Assistant:**';
      // Get text content, excluding action buttons
      const textContent = element.textContent?.trim() || '';
      if (textContent) {
        messages.push(`${role}\n\n${textContent}`);
      }
    }
  });

  return messages.join('\n\n---\n\n');
}

// Custom Header component with copy button
function CustomHeader() {
  const { setOpen, icons, labels } = useChatContext();
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    const markdown = extractMessagesFromDOM();

    if (!markdown) {
      console.log('No messages to copy');
      return;
    }

    try {
      await navigator.clipboard.writeText(markdown);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error('Failed to copy:', err);
    }
  }, []);

  return (
    <div className="copilotKitHeader">
      <div style={{ flex: 1 }}>{labels.title}</div>
      <div className="copilotKitHeaderControls">
        <button
          onClick={handleCopy}
          className="copilotkit-copy-button"
          title="Copy chat as markdown"
          aria-label="Copy chat as markdown"
        >
          {copied ? <CheckIcon /> : <CopyIcon />}
        </button>
        <button
          onClick={() => setOpen(false)}
          className="copilotKitHeaderCloseButton"
          aria-label="Close"
        >
          {icons.headerCloseIcon}
        </button>
      </div>
    </div>
  );
}

// Generate a session-based conversation ID
function getConversationId(): string {
  if (typeof window === 'undefined') return '';
  let id = sessionStorage.getItem('copilot_conversation_id');
  if (!id) {
    id = `conv_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
    sessionStorage.setItem('copilot_conversation_id', id);
  }
  return id;
}

// Type for CopilotKit message (subset of properties we need)
interface CopilotMessage {
  id: string;
  role?: string;
  content?: string | unknown;
}

// Wrapper component that has access to chat context for feedback tracking
function CopilotPopupWithFeedback() {
  const { visibleMessages } = useCopilotChat();
  const [conversationId, setConversationId] = useState('');

  useEffect(() => {
    setConversationId(getConversationId());
  }, []);

  const getMessageContent = useCallback((msg: CopilotMessage): string | null => {
    if (msg.content) {
      return typeof msg.content === 'string' ? msg.content : JSON.stringify(msg.content);
    }
    return null;
  }, []);

  const findPrecedingUserPrompt = useCallback(
    (assistantMessageId: string) => {
      const messageIndex = visibleMessages.findIndex(
        (m: CopilotMessage) => m.id === assistantMessageId
      );
      if (messageIndex <= 0) return null;

      // Look backwards for the most recent user message
      for (let i = messageIndex - 1; i >= 0; i--) {
        const msg = visibleMessages[i] as CopilotMessage;
        if (msg.role === 'user') {
          return getMessageContent(msg);
        }
      }
      return null;
    },
    [visibleMessages, getMessageContent]
  );

  const handleThumbsUp = useCallback(
    (message: CopilotMessage) => {
      const prompt = findPrecedingUserPrompt(message.id);
      const response = getMessageContent(message);

      posthog.capture('copilot_feedback', {
        vote: 'up',
        response,
        prompt,
        conversation_id: conversationId,
        timestamp: new Date().toISOString(),
      });
    },
    [findPrecedingUserPrompt, getMessageContent, conversationId]
  );

  const handleThumbsDown = useCallback(
    (message: CopilotMessage) => {
      const prompt = findPrecedingUserPrompt(message.id);
      const response = getMessageContent(message);

      posthog.capture('copilot_feedback', {
        vote: 'down',
        response,
        prompt,
        conversation_id: conversationId,
        timestamp: new Date().toISOString(),
      });
    },
    [findPrecedingUserPrompt, getMessageContent, conversationId]
  );

  return (
    <CopilotPopup
      instructions={DOCS_INSTRUCTIONS}
      labels={{
        title: 'CUA Docs Assistant',
        initial: `Ask me anything about CUA!

This is currently an **experimental** feature.

Please refer to the source documentation for the most accurate information.`,
      }}
      AssistantMessage={CustomAssistantMessage}
      Header={CustomHeader}
      onThumbsUp={handleThumbsUp}
      onThumbsDown={handleThumbsDown}
    />
  );
}

// CSS styles for the CopilotKit customizations
const customStyles = `
  .copilotKitHeader {
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 1.125rem;
    padding: 0.75rem 1rem;
    height: auto;
    min-height: 56px;
  }
  .copilotKitHeader::before {
    content: '';
    display: inline-block;
    width: 40px;
    height: 40px;
    background-image: url('/docs/img/cuala-icon.svg');
    background-size: contain;
    background-repeat: no-repeat;
    background-position: center;
    flex-shrink: 0;
  }

  .dark .copilotKitHeader::before {
    filter: invert(1);
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
  .copilotKitAssistantMessage {
    line-height: 1.6;
  }

  .copilotKitAssistantMessage p {
    margin-bottom: 0.75em;
  }

  .copilotKitAssistantMessage p:last-child {
    margin-bottom: 0;
  }

  /* Ensure proper spacing between text blocks */
  .copilotKitAssistantMessage > div > p + p {
    margin-top: 0.75em;
  }

  /* Better spacing for list items and headers */
  .copilotKitAssistantMessage ul,
  .copilotKitAssistantMessage ol {
    margin: 0.75em 0;
  }

  .copilotKitAssistantMessage li {
    margin-bottom: 0.25em;
  }

  .copilotKitAssistantMessage h1,
  .copilotKitAssistantMessage h2,
  .copilotKitAssistantMessage h3 {
    margin-top: 1em;
    margin-bottom: 0.5em;
  }

  /* Header controls container */
  .copilotKitHeaderControls {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  /* Custom copy button styles */
  .copilotkit-copy-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    background: transparent;
    border-radius: 6px;
    cursor: pointer;
    color: var(--copilot-kit-secondary-contrast-color, #666);
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .copilotkit-copy-button:hover {
    background-color: var(--copilot-kit-separator-color, rgba(0, 0, 0, 0.08));
    color: var(--copilot-kit-secondary-contrast-color, #333);
  }

  .copilotkit-copy-button:active {
    background-color: var(--copilot-kit-separator-color, rgba(0, 0, 0, 0.12));
  }

  .dark .copilotkit-copy-button {
    color: rgba(255, 255, 255, 0.7);
  }

  .dark .copilotkit-copy-button:hover {
    background-color: rgba(255, 255, 255, 0.1);
    color: rgba(255, 255, 255, 0.9);
  }

  /* Hide regenerate button but keep copy and thumbs up/down for feedback */
  /* Target by aria-label (multiple possible labels) */
  .copilotKitMessageControls button[aria-label="Regenerate"],
  .copilotKitMessageControls button[aria-label="regenerate"],
  .copilotKitMessageControls button[title="Regenerate"],
  .copilotKitMessageControls button[title="regenerate"],
  /* Target by position (first button in the controls) */
  .copilotKitAssistantMessage .copilotKitMessageControls > button:first-child,
  [class*="copilotKitAssistantMessage"] [class*="MessageControls"] > button:first-child,
  [class*="copilotKitAssistantMessage"] [class*="Controls"] > button:first-child,
  /* Target buttons containing refresh/regenerate SVG icons (lucide RefreshCw icon class) */
  .copilotKitMessageControls button:has(svg.lucide-refresh-cw),
  .copilotKitMessageControls button:has([class*="refresh"]),
  .copilotKitMessageControls button:has([class*="Refresh"]),
  /* Additional fallback targeting first button in any flex container inside assistant messages */
  [class*="AssistantMessage"] [class*="Controls"] button:first-of-type {
    display: none !important;
  }

  /* Hide download button in code blocks (first button), keep copy button */
  .copilotKitCodeBlockToolbarButtons button:first-child {
    display: none !important;
  }

  /* Fix corner radius consistency - all corners should match window's 0.75rem */
  @media (min-width: 640px) {
    .copilotKitHeader {
      border-top-left-radius: 0.75rem;
      border-top-right-radius: 0.75rem;
    }
  }

  /* Hide "Powered by CopilotKit" text and reduce bottom spacing */
  .poweredBy {
    display: none !important;
  }

  .poweredByContainer {
    padding: 0.75rem !important;
    min-height: 0 !important;
  }

  /* Match chat input radius to window */
  .copilotKitInput {
    border-radius: 0.75rem;
  }

  /* Hide chat on mobile devices */
  @media (max-width: 768px) {
    .copilotKitPopup,
    [class*="copilotKitPopup"] {
      display: none !important;
    }
  }
`;

// Memoized CopilotKit wrapper to prevent re-renders
const CopilotKitWrapper = memo(function CopilotKitWrapper({ children }: { children: ReactNode }) {
  return (
    <CopilotKit runtimeUrl="/docs/api/copilotkit" showDevConsole={false}>
      <style>{customStyles}</style>
      <ToolCallIndicators />
      {children}
      <CopilotPopupWithFeedback />
    </CopilotKit>
  );
});

export function CopilotKitProvider({ children }: CopilotKitProviderProps) {
  const [mounted, setMounted] = useState(false);
  const mountedOnceRef = useRef(false);

  useEffect(() => {
    // Ensure we only mount once, even with React Strict Mode
    if (mountedOnceRef.current) return;
    mountedOnceRef.current = true;

    const timer = setTimeout(() => setMounted(true), 200);
    return () => clearTimeout(timer);
  }, []);

  if (!COPILOTKIT_ENABLED || !mounted) {
    return <>{children}</>;
  }

  return <CopilotKitWrapper>{children}</CopilotKitWrapper>;
}
