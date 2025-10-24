#!/usr/bin/env node
const { execSync } = require('child_process');

try {
  execSync('pnpm -r --filter "./libs/typescript/*" run typecheck', { stdio: 'inherit' });
} catch (err) {
  process.exit(1);
}
