#!/usr/bin/env node
const { execSync } = require('child_process');

try {
  execSync('pnpm -C libs/typescript install', { stdio: 'inherit' });
  // Build core first so its type declarations (dist/) are available
  // for dependent packages like @trycua/computer and @trycua/agent
  execSync('pnpm -C libs/typescript/core run build', { stdio: 'inherit' });
  execSync('pnpm -C libs/typescript -r run typecheck', { stdio: 'inherit' });
} catch (err) {
  process.exit(1);
}
