import { defineConfig } from 'tsdown';

// One self-contained ESM file (dist/index.js) plus its types (dist/index.d.ts),
// so the build can be copied into any project straight from the repository.
// Types are emitted in a second pass: bundling them in the same pass splits a
// shared runtime helper into its own chunk.
export default defineConfig([
  { entry: ['./src/index.ts'], platform: 'neutral', dts: false },
  { entry: ['./src/index.ts'], platform: 'neutral', dts: { emitDtsOnly: true }, clean: false },
]);
