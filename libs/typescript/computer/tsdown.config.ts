import { readFileSync } from 'node:fs';
import { defineConfig } from 'tsdown';

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'));

export default defineConfig([
  {
    entry: ['./src/index.ts'],
    platform: 'node',
    dts: true,
    external: ['child_process', 'util'],
    define: {
      __CUA_VERSION__: JSON.stringify(pkg.version),
    },
  },
]);
