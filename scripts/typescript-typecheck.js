#!/usr/bin/env node
const { execSync } = require('child_process');

try {
  execSync('pnpm -C libs/typescript install', { stdio: 'inherit' });
  execSync('pnpm -C libs/typescript -r run typecheck', { stdio: 'inherit' });
} catch (err) {
  process.exit(1);
}
