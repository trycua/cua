// Mock MCP server for testing Gemini CLI integration
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { ListToolsRequestSchema, CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js';

const server = new Server({
  name: 'mock-cua-driver',
  version: '1.0.0'
}, {
  capabilities: {
    tools: {}
  }
});

// Mock tools
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      {
        name: 'screenshot',
        description: 'Take a screenshot',
        inputSchema: {
          type: 'object',
          properties: {}
        }
      },
      {
        name: 'click',
        description: 'Click at coordinates',
        inputSchema: {
          type: 'object',
          properties: {
            x: { type: 'number', description: 'X coordinate' },
            y: { type: 'number', description: 'Y coordinate' }
          },
          required: ['x', 'y']
        }
      },
      {
        name: 'type_text',
        description: 'Type text',
        inputSchema: {
          type: 'object',
          properties: {
            text: { type: 'string', description: 'Text to type' }
          },
          required: ['text']
        }
      }
    ]
  };
});

// Mock tool execution
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  console.error(`[Mock Server] Executing tool: ${name} with args: ${JSON.stringify(args)}`);

  return {
    content: [{
      type: 'text',
      text: `Mock result for ${name}: Success!`
    }]
  };
});

// Start server
const transport = new StdioServerTransport();
await server.connect(transport);

console.error('[Mock Server] Ready');
