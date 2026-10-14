import { defineAgent } from 'eve';

export default defineAgent({
  // Any vision-capable model works; desktop observations include screenshots.
  model: 'anthropic/claude-opus-5.5',
  build: {
    // The Driver SDK loads a native library, so keep it out of eve's bundle.
    externalDependencies: ['@trycua/cua-driver'],
  },
});
