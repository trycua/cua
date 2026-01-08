import {
  CopilotRuntime,
  ExperimentalEmptyAdapter,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';
import { BuiltInAgent, InMemoryAgentRunner } from '@copilotkit/runtime/v2';
import { NextRequest } from 'next/server';

// Create a BuiltInAgent using AI SDK with Anthropic
const docsAgent = new BuiltInAgent({
  model: 'anthropic/claude-sonnet-4-20250514',
  prompt: `You are a helpful assistant for CUA (Computer Use Agent) and CUA-Bench documentation.
Be concise and helpful. Answer questions about the documentation accurately.`,
  temperature: 0.7,
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
