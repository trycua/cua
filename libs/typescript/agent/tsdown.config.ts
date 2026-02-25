import { readFileSync } from 'node:fs';
import { defineConfig } from 'tsdown';

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'));

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['module'],
  platform: 'browser',
  dts: true,
  clean: true,
  // Remove if we don't need to support including the library via '<script/>' tags.
  // noExternal bundles this list of libraries within the final 'dist'
  noExternal: ['peerjs'],
  define: {
    __CUA_VERSION__: JSON.stringify(pkg.version),
  },
});
