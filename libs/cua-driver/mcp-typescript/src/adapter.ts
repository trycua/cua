import { pathToFileURL } from 'node:url';
import type { CallToolResult, Tool } from '@modelcontextprotocol/sdk/types.js';
import type { DriverAdapter, GeneratedCuaDriver, GeneratedCuaDriverModule } from './types.js';

interface DaemonToolDefinition {
  annotations?: Tool['annotations'];
  capabilities?: unknown;
  description?: string;
  destructive?: boolean;
  idempotent?: boolean;
  input_schema?: Tool['inputSchema'];
  name?: string;
  open_world?: boolean;
  output_schema?: Tool['outputSchema'];
  read_only?: boolean;
  risk?: unknown;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function mapDaemonInventory(value: unknown): Tool[] {
  const definitions = isObject(value) && Array.isArray(value.tools) ? value.tools : undefined;
  if (!definitions) throw new Error('Cua Driver returned an invalid tool inventory');

  return definitions.map((raw) => {
    const definition = raw as DaemonToolDefinition;
    if (!definition.name || !definition.input_schema) {
      throw new Error('Cua Driver returned an incomplete tool definition');
    }
    const tool: Tool & Record<string, unknown> = {
      name: definition.name,
      description: definition.description ?? '',
      inputSchema: definition.input_schema,
      annotations: definition.annotations ?? {
        readOnlyHint: definition.read_only ?? false,
        destructiveHint: definition.destructive ?? false,
        idempotentHint: definition.idempotent ?? false,
        openWorldHint: definition.open_world ?? false,
      },
    };
    if (definition.output_schema) tool.outputSchema = definition.output_schema;
    if (definition.capabilities !== undefined) tool.capabilities = definition.capabilities;
    if (definition.risk !== undefined) tool.risk = definition.risk;
    return tool;
  });
}

export class UniffiDaemonAdapter implements DriverAdapter {
  constructor(private readonly driver: GeneratedCuaDriver) {}

  static async connect(options: {
    modulePath: string;
    socketPath?: string;
  }): Promise<UniffiDaemonAdapter> {
    const module = (await import(
      pathToFileURL(options.modulePath).href
    )) as GeneratedCuaDriverModule;
    const driver = module.CuaDriver.connect(options.socketPath);
    const metadata = await driver.metadata();
    if (metadata.embedded) {
      driver.uniffiDestroy();
      throw new Error('expected the canonical Cua Driver daemon, got an embedded runtime');
    }
    return new UniffiDaemonAdapter(driver);
  }

  async listTools(): Promise<Tool[]> {
    return mapDaemonInventory(JSON.parse(await this.driver.listToolsJson()));
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<CallToolResult> {
    const result = await this.driver.callTool(name, JSON.stringify(args));
    const raw = JSON.parse(result.rawJson) as unknown;
    if (!isObject(raw) || !Array.isArray(raw.content)) {
      throw new Error(`Cua Driver returned an invalid result for ${name}`);
    }
    return raw as CallToolResult;
  }

  async close(): Promise<void> {
    await this.driver.shutdown();
    this.driver.uniffiDestroy();
  }
}
