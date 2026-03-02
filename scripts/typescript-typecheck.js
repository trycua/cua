#!/usr/bin/env node
const { execSync } = require('child_process');
const { readFileSync, writeFileSync, existsSync } = require('fs');
const { join } = require('path');

// Allow pnpm version mismatch (root uses 9.x, workspace packages declare 10.x)
const env = { ...process.env, COREPACK_ENABLE_STRICT: '0' };

// Save lockfile before install so we can restore it after — pnpm install
// may rewrite the lockfile format when the pnpm version differs, which
// causes pre-commit to report "files were modified by this hook".
const lockfilePath = join(__dirname, '..', 'libs', 'typescript', 'pnpm-lock.yaml');
const lockfileBefore = existsSync(lockfilePath) ? readFileSync(lockfilePath) : null;

try {
  execSync('pnpm -C libs/typescript install', { stdio: 'inherit', env });
  // Build core first so its type declarations (dist/) are available
  // for dependent packages like @trycua/computer and @trycua/agent
  execSync('pnpm -C libs/typescript/core run build', { stdio: 'inherit', env });
  execSync('pnpm -C libs/typescript -r run typecheck', { stdio: 'inherit', env });
} catch (err) {
  process.exit(1);
} finally {
  // Restore lockfile to avoid pre-commit "files were modified" failure
  if (lockfileBefore !== null) {
    writeFileSync(lockfilePath, lockfileBefore);
  }
}
