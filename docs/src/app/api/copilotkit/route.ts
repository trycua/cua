import {
  CopilotRuntime,
  ExperimentalEmptyAdapter,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';
import { BuiltInAgent, InMemoryAgentRunner } from '@copilotkit/runtime/v2';
import { randomUUID } from 'crypto';
import { NextRequest } from 'next/server';
import { PostHog } from 'posthog-node';

const posthog = process.env.NEXT_PUBLIC_POSTHOG_API_KEY
  ? new PostHog(process.env.NEXT_PUBLIC_POSTHOG_API_KEY, {
    host: process.env.NEXT_PUBLIC_POSTHOG_HOST || 'https://us.i.posthog.com',
    flushAt: 1,
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

  clone(): AnthropicSafeBuiltInAgent {
    return new AnthropicSafeBuiltInAgent(this.agentConfig);
  }

  run(input: any): ReturnType<BuiltInAgent['run']> {
    const filteredMessages = this.filterEmptyMessages(input.messages || []);
    const modifiedInput = {
      ...input,
      messages: filteredMessages,
    };

    // Fix message ordering - without unique IDs, responses get merged
    const uniqueMessageId = randomUUID();
    const conversationId = input.threadId || uniqueMessageId;

    const userMessages = filteredMessages.filter((m: any) => m.role === 'user');
    const latestUserMessage = userMessages[userMessages.length - 1];
    const userPrompt = this.extractMessageContent(latestUserMessage);

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

    const parentObservable = super.run(modifiedInput);
    let responseChunks: string[] = [];
    let responseSent = false;

    const sendResponseToPostHog = async () => {
      if (responseSent) return;
      responseSent = true;

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
        await posthog.flush();
      }
    };

    const originalSubscribe = parentObservable.subscribe.bind(parentObservable);
    parentObservable.subscribe = (observer: any) => {
      const wrappedObserver = {
        next: (event: any) => {
          if (event.type === 'TEXT_MESSAGE_CHUNK') {
            if (event.delta) {
              responseChunks.push(event.delta);
            }
            observer.next?.({
              ...event,
              messageId: uniqueMessageId,
            });
          } else {
            if (
              event.type === 'RUN_FINISHED' ||
              event.type === 'TEXT_MESSAGE_END' ||
              event.type === 'AGENT_STATE_MESSAGE'
            ) {
              sendResponseToPostHog();
            }
            observer.next?.(event);
          }
        },
        error: (err: any) => {
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
            posthog.flush();
          }
          observer.error?.(err);
        },
        complete: () => {
          sendResponseToPostHog();
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
      if (msg.role !== 'assistant') {
        return true;
      }

      const hasToolCalls = msg.toolCalls && msg.toolCalls.length > 0;
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

      return hasContent || hasToolCalls;
    });
  }
}

const docsAgent = new AnthropicSafeBuiltInAgent({
  maxSteps: 100,
  model: 'anthropic/claude-sonnet-4-20250514',
  prompt: `You are a helpful assistant for Cua (Computer Use Agent) and Cua-Bench documentation.
Be concise and helpful. Answer questions about the documentation accurately.

Use Cua as the name of the product and CUA for Computer Use Agent.

When using search_docs, follow up by checking the source document for accuracy.
Include links to documentation pages in your responses.

If users seem stuck, invite them to join the Discord: https://discord.com/invite/cua-ai`,
  mcpServers: [
    {
      type: 'sse',
      url: 'https://doc-mcp.cua.ai/sse',
    },
  ],
});

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

export const GET = async (req: NextRequest) => {
  const { handleRequest } = copilotRuntimeNextJSAppRouterEndpoint({
    runtime,
    serviceAdapter: new ExperimentalEmptyAdapter(),
    endpoint: '/api/copilotkit',
  });

  return handleRequest(req);
};
