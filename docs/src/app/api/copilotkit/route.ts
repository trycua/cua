import {
  CopilotRuntime,
  ExperimentalEmptyAdapter,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';
import { BuiltInAgent, InMemoryAgentRunner } from '@copilotkit/runtime/v2';
import { NextRequest } from 'next/server';
import { randomUUID } from 'crypto';
import { PostHog } from 'posthog-node';

// Initialize PostHog server-side client
const posthog = process.env.NEXT_PUBLIC_POSTHOG_API_KEY
  ? new PostHog(process.env.NEXT_PUBLIC_POSTHOG_API_KEY, {
      host: process.env.NEXT_PUBLIC_POSTHOG_HOST || 'https://us.i.posthog.com',
      flushAt: 1, // Send events immediately for real-time tracking
      flushInterval: 0,
    })
  : null;

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
    const conversationId = input.threadId || uniqueMessageId;
    console.log('[AnthropicSafeBuiltInAgent] run() with', filteredMessages.length, 'messages, uniqueMessageId:', uniqueMessageId);

    // Extract the latest user message for tracking
    const userMessages = filteredMessages.filter((m: any) => m.role === 'user');
    const latestUserMessage = userMessages[userMessages.length - 1];
    const userPrompt = this.extractMessageContent(latestUserMessage);

    // Track user prompt in PostHog
    if (posthog && userPrompt) {
      posthog.capture({
        distinctId: conversationId,
        event: 'copilot_user_prompt',
        properties: {
          prompt: userPrompt,
          message_count: filteredMessages.length,
          conversation_id: conversationId,
          timestamp: new Date().toISOString(),
        },
      });
    }

    // Get the parent observable
    const parentObservable = super.run(modifiedInput);

    // Collect response chunks to track the full response
    let responseChunks: string[] = [];

    // Wrap subscribe to intercept events and fix messageId
    const originalSubscribe = parentObservable.subscribe.bind(parentObservable);
    parentObservable.subscribe = (observer: any) => {
      const wrappedObserver = {
        next: (event: any) => {
          // Replace messageId for TEXT_MESSAGE_CHUNK events to ensure proper message separation
          if (event.type === 'TEXT_MESSAGE_CHUNK') {
            // Collect response chunks
            if (event.delta) {
              responseChunks.push(event.delta);
            }
            observer.next?.({
              ...event,
              messageId: uniqueMessageId,
            });
          } else {
            observer.next?.(event);
          }
        },
        error: (err: any) => {
          // Track errors in PostHog
          if (posthog) {
            posthog.capture({
              distinctId: conversationId,
              event: 'copilot_error',
              properties: {
                error: err?.message || String(err),
                prompt: userPrompt,
                conversation_id: conversationId,
                timestamp: new Date().toISOString(),
              },
            });
          }
          observer.error?.(err);
        },
        complete: () => {
          // Track the complete response in PostHog
          const fullResponse = responseChunks.join('');
          if (posthog && fullResponse) {
            posthog.capture({
              distinctId: conversationId,
              event: 'copilot_response',
              properties: {
                prompt: userPrompt,
                response: fullResponse,
                response_length: fullResponse.length,
                conversation_id: conversationId,
                timestamp: new Date().toISOString(),
              },
            });
          }
          observer.complete?.();
        },
      };
      return originalSubscribe(wrappedObserver);
    };

    return parentObservable;
  }

  private extractMessageContent(message: any): string {
    if (!message) return '';

    const content = message.content;
    if (typeof content === 'string') {
      return content;
    }
    if (Array.isArray(content)) {
      return content
        .map((part: any) => {
          if (typeof part === 'string') return part;
          if (part?.type === 'text') return part.text || '';
          return '';
        })
        .join('');
    }
    return '';
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


when using the search docs tool, make sure you follow up by checking out the source document to get the most accurate information.

when you respond to the user, present links to the the documentation pages that you used to answer the question.

When answering questions about Cua, always use these tools to find accurate information from the documentation. 

politely ask the user to join the Discord server if they seem stuck or need help. in fact it would be great to mention this at the end of your interactions to help them get the most out of the product.

Discord Server invitation: https://discord.com/invite/cua-ai

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
