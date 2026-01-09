import {
  CopilotRuntime,
  ExperimentalEmptyAdapter,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';
import { BuiltInAgent, InMemoryAgentRunner } from '@copilotkit/runtime/v2';
import { NextRequest } from 'next/server';
import { randomUUID } from 'crypto';

/**
 * Custom agent that extends BuiltInAgent to fix issues with Anthropic and message ordering.
 *
 * Fixes:
 * 1. Filters empty assistant messages (Anthropic API requirement)
 * 2. Forces new message IDs for each conversation turn to fix message ordering
 * 3. Preserves our custom class on clone()
 */
class AnthropicSafeBuiltInAgent extends BuiltInAgent {
  private agentConfig: any;

  constructor(config: any) {
    super(config);
    this.agentConfig = config;
  }

  // Override clone() to return our custom class instead of base BuiltInAgent
  clone(): AnthropicSafeBuiltInAgent {
    return new AnthropicSafeBuiltInAgent(this.agentConfig);
  }

  run(input: any): ReturnType<BuiltInAgent['run']> {
    // Filter out empty assistant messages before passing to parent
    const filteredMessages = this.filterEmptyMessages(input.messages || []);

    // Create modified input with filtered messages
    const modifiedInput = {
      ...input,
      messages: filteredMessages,
    };

    // Generate a unique message ID for this run to fix message ordering
    // Without this, all responses use messageId: 0 and get merged together
    const uniqueMessageId = randomUUID();
    console.log('[AnthropicSafeBuiltInAgent] run() with', filteredMessages.length, 'messages, uniqueMessageId:', uniqueMessageId);

    // Get the parent observable
    const parentObservable = super.run(modifiedInput);

    // Wrap subscribe to intercept events and fix messageId
    const originalSubscribe = parentObservable.subscribe.bind(parentObservable);
    parentObservable.subscribe = (observer: any) => {
      const wrappedObserver = {
        next: (event: any) => {
          // Replace messageId for TEXT_MESSAGE_CHUNK events to ensure proper message separation
          if (event.type === 'TEXT_MESSAGE_CHUNK') {
            observer.next?.({
              ...event,
              messageId: uniqueMessageId,
            });
          } else {
            observer.next?.(event);
          }
        },
        error: (err: any) => observer.error?.(err),
        complete: () => observer.complete?.(),
      };
      return originalSubscribe(wrappedObserver);
    };

    return parentObservable;
  }

  private filterEmptyMessages(messages: any[]): any[] {
    return messages.filter((msg) => {
      // Keep all non-assistant messages
      if (msg.role !== 'assistant') {
        return true;
      }

      // Check if message has tool calls
      const hasToolCalls = msg.toolCalls && msg.toolCalls.length > 0;

      // Check if message has meaningful content
      const content = msg.content;
      let hasContent = false;

      if (typeof content === 'string') {
        hasContent = content.trim().length > 0;
      } else if (Array.isArray(content)) {
        hasContent = content.some((part: any) => {
          if (typeof part === 'string') return part.trim().length > 0;
          if (part && typeof part === 'object') {
            if (part.type === 'text') return part.text && part.text.trim().length > 0;
            if (part.type === 'tool_use' || part.type === 'tool-call') return true;
          }
          return false;
        });
      }

      // Keep if has content or tool calls
      return hasContent || hasToolCalls;
    });
  }
}

// Create a custom agent that safely handles Anthropic's message requirements
const docsAgent = new AnthropicSafeBuiltInAgent({
  maxSteps: 100,
  model: 'anthropic/claude-sonnet-4-20250514',
  prompt: `You are a helpful assistant for Cua (Computer Use Agent) and Cua-Bench documentation.
Be concise and helpful. Answer questions about the documentation accurately.

Use Cua as the name of the product and CUA for Computer Use Agent

if someone asks about cua, they are referring to Cua the product, not CUA the Computer Use Agent.

You have access to tools for searching the Cua documentation:
- search_docs: Use this to search for documentation content semantically
- sql_query: Use this for direct SQL queries on the documentation database

When answering questions about Cua, always use these tools to find accurate information from the documentation. 

politely ask the user to join the Discord server if they seem stuck or need help. in fact it would be great to mention this at the end of your interactions to help them get the most out of the product.

Discord Server invitation: https://discord.gg/MgrZyS3gcx

`,
  temperature: 0.7,
  mcpServers: [
    {
      type: 'sse',
      url: 'https://cuaai--cua-docs-mcp-web.modal.run/sse',
    },
  ],
});

// Create runtime with the agent registered as 'default'
// Cast to any to bypass rxjs version conflicts between our Observable and CopilotKit's
const runtime = new CopilotRuntime({
  agents: {
    default: docsAgent as any,
  },
  runner: new InMemoryAgentRunner(),
});

export const POST = async (req: NextRequest) => {
  const { handleRequest } = copilotRuntimeNextJSAppRouterEndpoint({
    runtime,
    serviceAdapter: new ExperimentalEmptyAdapter(),
    endpoint: '/api/copilotkit',
  });

  return handleRequest(req);
};

// GET handler for /info endpoint - returns agent metadata
export const GET = async (req: NextRequest) => {
  const { handleRequest } = copilotRuntimeNextJSAppRouterEndpoint({
    runtime,
    serviceAdapter: new ExperimentalEmptyAdapter(),
    endpoint: '/api/copilotkit',
  });

  return handleRequest(req);
};
