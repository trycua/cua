import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import {
  CallToolRequestSchema,
  ListResourcesRequestSchema,
  ListToolsRequestSchema,
  ReadResourceRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import {
  buildSkillInstructions,
  listSkillResources,
  LOAD_SKILL_TOOL,
  loadSkill,
  loadSkillTool,
  readSkillResource,
} from './skill-pack.js';
import type { DriverAdapter } from './types.js';

export async function createCuaMcpServer(options: {
  adapter: DriverAdapter;
  skillRoot: string;
}): Promise<Server> {
  const driverTools = await options.adapter.listTools();
  const driverToolNames = new Set(driverTools.map((tool) => tool.name));
  if (driverToolNames.has(LOAD_SKILL_TOOL))
    throw new Error(`reserved tool collision: ${LOAD_SKILL_TOOL}`);

  const server = new Server(
    { name: 'cua-driver-mcp-typescript', version: '0.0.0' },
    {
      capabilities: { tools: {}, resources: {} },
      instructions: await buildSkillInstructions(options.skillRoot),
    }
  );
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [...driverTools, loadSkillTool],
  }));
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const args = request.params.arguments ?? {};
    if (request.params.name === LOAD_SKILL_TOOL) return loadSkill(options.skillRoot, args);
    if (!driverToolNames.has(request.params.name)) {
      return {
        content: [{ type: 'text', text: `Unknown tool: ${request.params.name}` }],
        isError: true,
      };
    }
    return options.adapter.callTool(request.params.name, args);
  });
  server.setRequestHandler(ListResourcesRequestSchema, async () => ({
    resources: await listSkillResources(options.skillRoot),
  }));
  server.setRequestHandler(ReadResourceRequestSchema, async (request) => ({
    contents: [await readSkillResource(options.skillRoot, request.params.uri)],
  }));
  return server;
}
