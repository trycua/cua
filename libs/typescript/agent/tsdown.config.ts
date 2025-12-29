import { defineConfig } from 'tsdown';

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['module'],
  platform: 'browser',
  dts: true,
  clean: true,
  // Remove if we don't need to support including the library via '<script/>' tags.
  // noExternal bundles this list of libraries within the final 'dist'
  noExternal: ['peerjs'],
});
