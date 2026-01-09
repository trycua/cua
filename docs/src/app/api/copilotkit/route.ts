import type { NextRequest } from 'next/server';
import {
  AnthropicAdapter,
  CopilotRuntime,
  copilotRuntimeNextJSAppRouterEndpoint,
} from '@copilotkit/runtime';

// Use AnthropicAdapter for Claude model
const serviceAdapter = new AnthropicAdapter({
  model: 'claude-sonnet-4-20250514',
});

// Create runtime
const runtime = new CopilotRuntime();

// Create the endpoint handler
// Note: Next.js strips basePath before routing, so use path without /docs prefix
const { handleRequest } = copilotRuntimeNextJSAppRouterEndpoint({
  runtime,
  serviceAdapter,
  endpoint: '/api/copilotkit',
});

// Handle POST requests
export const POST = async (req: NextRequest) => {
  return handleRequest(req);
};
