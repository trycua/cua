import type { CallToolResult, Tool } from '@modelcontextprotocol/sdk/types.js';

export interface DriverAdapter {
  listTools(): Promise<Tool[]>;
  callTool(name: string, args: Record<string, unknown>): Promise<CallToolResult>;
  close(): Promise<void>;
}

export interface GeneratedCuaDriver {
  callTool(name: string, argumentsJson: string): Promise<{ rawJson: string }>;
  listToolsJson(): Promise<string>;
  metadata(): Promise<{ embedded: boolean; hostBundleId?: string; pid: number }>;
  shutdown(): Promise<void>;
  uniffiDestroy(): void;
}

export interface GeneratedCuaDriverModule {
  CuaDriver: { connect(socketPath: string | undefined): GeneratedCuaDriver };
}
