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

// Prompt categorization types and helpers
type PromptCategory =
  | 'installation'
  | 'configuration'
  | 'usage'
  | 'troubleshooting'
  | 'conceptual'
  | 'api_reference'
  | 'integration'
  | 'other';

type QuestionType = 'how-to' | 'what-is' | 'why' | 'can-i' | 'where' | 'debug' | 'other';

function categorizePrompt(prompt: string): PromptCategory {
  const lowerPrompt = prompt.toLowerCase();

  if (/\b(install|setup|pip|npm|brew|download|get started|quick\s?start)\b/.test(lowerPrompt)) {
    return 'installation';
  }

  if (
    /\b(config|configure|setting|env|environment|\.env|options?|parameters?)\b/.test(lowerPrompt)
  ) {
    return 'configuration';
  }

  if (
    /\b(error|fail|issue|problem|not working|bug|crash|exception|fix|debug|broken)\b/.test(
      lowerPrompt
    )
  ) {
    return 'troubleshooting';
  }

  if (
    /\b(api|endpoint|method|function|class|interface|type|schema|parameters?)\b/.test(lowerPrompt)
  ) {
    return 'api_reference';
  }

  if (/\b(integrat|connect|webhook|third.?party|external)\b/.test(lowerPrompt)) {
    return 'integration';
  }

  if (
    /\b(what\s+is|explain|concept|understand|how\s+does|architecture|overview)\b/.test(lowerPrompt)
  ) {
    return 'conceptual';
  }

  if (/\b(how\s+to|use|using|example|tutorial|guide)\b/.test(lowerPrompt)) {
    return 'usage';
  }

  return 'other';
}

function detectQuestionType(prompt: string): QuestionType {
  const lowerPrompt = prompt.toLowerCase().trim();

  if (/^how\s+(do|can|to|would|should)/.test(lowerPrompt)) return 'how-to';
  if (/^what\s+(is|are|does)/.test(lowerPrompt)) return 'what-is';
  if (/^why\s+/.test(lowerPrompt)) return 'why';
  if (/^(can|could|is it possible)/.test(lowerPrompt)) return 'can-i';
  if (/^where\s+/.test(lowerPrompt)) return 'where';
  if (/\b(error|not working|fail|broken|debug)\b/.test(lowerPrompt)) return 'debug';

  return 'other';
}

function extractTopics(prompt: string): string[] {
  const topics: string[] = [];
  const lowerPrompt = prompt.toLowerCase();

  const topicPatterns: Record<string, RegExp> = {
    'computer-use': /\b(computer.?use|cua|cua.?agent)\b/,
    lume: /\b(lume|vm|virtual.?machine)\b/,
    mcp: /\b(mcp|model.?context|mcp.?server)\b/,
    benchmark: /\b(bench|benchmark|eval|score)\b/,
    python: /\b(python|pip|pypi)\b/,
    typescript: /\b(typescript|\.ts)\b/,
    javascript: /\b(javascript|\.js|npm)\b/,
    nodejs: /\b(node\.?js|node)\b/,
    macos: /\b(mac|macos|darwin)\b/,
    linux: /\b(linux|ubuntu|debian)\b/,
  };

  for (const [topic, pattern] of Object.entries(topicPatterns)) {
    if (pattern.test(lowerPrompt)) {
      topics.push(topic);
    }
  }

  return topics;
}

// Response analysis types and helpers
type ResponseType =
  | 'explanation'
  | 'tutorial'
  | 'code_example'
  | 'troubleshooting'
  | 'reference'
  | 'clarification'
  | 'other';

interface ResponseAnalysis {
  response_type: ResponseType;
  has_code_snippet: boolean;
  has_doc_links: boolean;
  has_steps: boolean;
  has_discord_invite: boolean;
  code_languages: string[];
}

function analyzeResponse(response: string): ResponseAnalysis {
  const lowerResponse = response.toLowerCase();

  // Detect code snippets (markdown code blocks)
  const codeBlockRegex = /```(\w+)?[\s\S]*?```/g;
  const codeBlocks = response.match(codeBlockRegex) || [];
  const hasCodeSnippet = codeBlocks.length > 0;

  // Extract code languages from code blocks
  const codeLanguages: string[] = [];
  for (const block of codeBlocks) {
    const langMatch = block.match(/```(\w+)/);
    if (langMatch && langMatch[1]) {
      const lang = langMatch[1].toLowerCase();
      if (!codeLanguages.includes(lang)) {
        codeLanguages.push(lang);
      }
    }
  }

  // Detect doc links
  const hasDocLinks =
    /\[.*?\]\(.*?(docs|documentation|\/docs).*?\)/i.test(response) ||
    /https?:\/\/[^\s]*docs[^\s]*/i.test(response) ||
    /trycua\.com/i.test(response);

  // Detect step-by-step instructions (numbered lists or sequential indicators)
  const hasSteps =
    /\b(step\s*\d|first,|second,|third,|finally,|then,|next,)\b/i.test(response) ||
    /^\s*\d+\.\s+/m.test(response);

  // Detect Discord invite
  const hasDiscordInvite = /discord\.com\/invite/i.test(response);

  // Determine response type (order matters - check more specific patterns first)
  let responseType: ResponseType = 'other';

  if (/\b(could\s+you\s+clarify|do\s+you\s+mean|more\s+specific)\b/.test(lowerResponse)) {
    responseType = 'clarification';
  } else if (hasSteps && hasCodeSnippet) {
    responseType = 'tutorial';
  } else if (hasCodeSnippet && !hasSteps) {
    responseType = 'code_example';
  } else if (/\b(error|fix|bug|crash|exception|resolve|try\s+this)\b/.test(lowerResponse)) {
    responseType = 'troubleshooting';
  } else if (/\b(what\s+is|means|refers\s+to|is\s+a|are\s+used\s+for)\b/.test(lowerResponse)) {
    responseType = 'explanation';
  } else if (/\b(api|method|function|parameter|returns?|accepts?)\b/.test(lowerResponse)) {
    responseType = 'reference';
  }

  return {
    response_type: responseType,
    has_code_snippet: hasCodeSnippet,
    has_doc_links: hasDocLinks,
    has_steps: hasSteps,
    has_discord_invite: hasDiscordInvite,
    code_languages: codeLanguages,
  };
}

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
    // Don't filter messages - let CopilotKit/Anthropic handle empty messages
    const messages = input.messages || [];
    const modifiedInput = {
      ...input,
      messages: messages,
    };

    // Fix message ordering - without unique IDs, responses get merged
    const uniqueMessageId = randomUUID();
    const conversationId = input.threadId || uniqueMessageId;

    const userMessages = messages.filter((m: any) => m.role === 'user');
    const latestUserMessage = userMessages[userMessages.length - 1];
    const userPrompt = this.extractMessageContent(latestUserMessage);

    const category = userPrompt ? categorizePrompt(userPrompt) : 'other';
    const questionType = userPrompt ? detectQuestionType(userPrompt) : 'other';
    const topics = userPrompt ? extractTopics(userPrompt) : [];

    if (posthog && userPrompt) {
      posthog.capture({
        distinctId: conversationId,
        event: 'copilot_user_prompt',
        properties: {
          prompt: userPrompt,
          category,
          question_type: questionType,
          topics,
          prompt_length: userPrompt.length,
          message_count: messages.length,
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
        const responseAnalysis = analyzeResponse(fullResponse);
        posthog.capture({
          distinctId: conversationId,
          event: 'copilot_response',
          properties: {
            prompt: userPrompt,
            response: fullResponse,
            // Prompt categorization
            category,
            question_type: questionType,
            topics,
            prompt_length: userPrompt.length,
            // Response analysis
            response_type: responseAnalysis.response_type,
            has_code_snippet: responseAnalysis.has_code_snippet,
            has_doc_links: responseAnalysis.has_doc_links,
            has_steps: responseAnalysis.has_steps,
            has_discord_invite: responseAnalysis.has_discord_invite,
            code_languages: responseAnalysis.code_languages,
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
          // Debug logging
          if (process.env.NODE_ENV === 'development') {
            const deltaPreview = event.delta
              ? `delta: ${String(event.delta).substring(0, 50)}...`
              : '';
            console.log('[CopilotKit] Event:', event.type, deltaPreview);
          }

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
          console.error('[CopilotKit] Error:', err?.message || String(err), err?.stack);
          if (posthog) {
            posthog.capture({
              distinctId: conversationId,
              event: 'copilot_error',
              properties: {
                error: err?.message || String(err),
                prompt: userPrompt,
                category,
                question_type: questionType,
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
  model: 'anthropic/claude-haiku-4-5-20251001',
  prompt: `You are a helpful assistant for Cua (Computer Use Agent) and Cua-Bench documentation.
Be concise and helpful. Answer questions about the documentation accurately.

Use "Cua" as the product name. "CUA" stands for Computer Use Agent.

You have access to tools for searching the Cua documentation and code:
- query_docs_db: SQL queries on the documentation database (full-text search via FTS5)
- query_docs_vectors: Semantic vector search over documentation
- query_code_db: SQL queries on the code database (full-text search, file content, versions)
- query_code_vectors: Semantic vector search over source code

When using query_docs_vectors, follow up by checking the source document for accuracy.
Include links to documentation pages in your responses.
When providing code examples, use query_code_db or query_code_vectors to ensure accuracy.

If users seem stuck, invite them to join the Discord: https://discord.com/invite/cua-ai`,
  temperature: 0.7,
  maxTokens: 8192 * 4,
  mcpServers: [
    {
      type: 'http',
      url: 'https://vk-mcp.cua.ai/mcp',
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
