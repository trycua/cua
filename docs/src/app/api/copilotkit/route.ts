import {
  CopilotRuntime,
  ExperimentalEmptyAdapter,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';
import { BuiltInAgent, InMemoryAgentRunner } from '@copilotkit/runtime/v2';
import { NextRequest } from 'next/server';
import { Observable } from 'rxjs';

/**
 * Custom agent that extends BuiltInAgent to fix empty message content issue with Anthropic.
 *
 * The Anthropic API requires all messages to have non-empty content (except the optional
 * final assistant message). CopilotKit creates separate empty TextMessages alongside
 * ActionExecutionMessages when the assistant uses tools. These empty messages cause
 * Anthropic API errors on follow-up messages.
 *
 * This fix:
 * 1. Overrides clone() to preserve our custom class (base class clone() returns BuiltInAgent)
 * 2. Overrides run() to filter out empty assistant messages before they reach the AI SDK
 * 3. Preserves messages with tool calls or meaningful content
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

  run(input: any): Observable<any> {
    // Filter out empty assistant messages before passing to parent
    const filteredMessages = this.filterEmptyMessages(input.messages || []);

    // Create modified input with filtered messages
    const modifiedInput = {
      ...input,
      messages: filteredMessages,
    };

    return super.run(modifiedInput);
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
  prompt: `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation.
Be concise and helpful. Answer questions about the documentation accurately.

You have access to tools for searching the CUA documentation:
- search_docs: Use this to search for documentation content semantically
- sql_query: Use this for direct SQL queries on the documentation database

When answering questions about CUA, always use these tools to find accurate information from the documentation.`,
  temperature: 0.7,
  mcpServers: [
    {
      type: 'sse',
      url: 'https://cuaai--cua-docs-mcp-web.modal.run/sse',
    },
  ],
});

// Create runtime with the agent registered as 'default'
const runtime = new CopilotRuntime({
  agents: {
    default: docsAgent,
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
