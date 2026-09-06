#!/usr/bin/env node
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { UniffiDaemonAdapter } from './adapter.js';
import { createCuaMcpServer } from './server.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const componentRoot = resolve(packageRoot, '..');
const adapter = await UniffiDaemonAdapter.connect({
  modulePath:
    process.env.CUA_DRIVER_TYPESCRIPT_MODULE ?? resolve(componentRoot, 'typescript/dist/index.js'),
  ...(process.env.CUA_DRIVER_SOCKET ? { socketPath: process.env.CUA_DRIVER_SOCKET } : {}),
});
const server = await createCuaMcpServer({
  adapter,
  skillRoot: resolve(componentRoot, 'rust/Skills/cua-driver'),
});
const close = async (): Promise<void> => {
  await server.close();
  await adapter.close();
};
process.once('SIGINT', () => void close());
process.once('SIGTERM', () => void close());
await server.connect(new StdioServerTransport());
