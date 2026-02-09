#!/usr/bin/env node
const { execSync } = require('child_process');

// Allow pnpm version mismatch (root uses 9.x, workspace packages declare 10.x)
const env = { ...process.env, COREPACK_ENABLE_STRICT: '0' };

try {
  execSync('pnpm -C libs/typescript install', { stdio: 'inherit', env });
  // Build core first so its type declarations (dist/) are available
  // for dependent packages like @trycua/computer and @trycua/agent
  execSync('pnpm -C libs/typescript/core run build', { stdio: 'inherit', env });
  execSync('pnpm -C libs/typescript -r run typecheck', { stdio: 'inherit', env });
} catch (err) {
  process.exit(1);
}
