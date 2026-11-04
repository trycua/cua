import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'happy-dom',
    // Tests must never send real telemetry.
    env: { CUA_TELEMETRY: '0', DO_NOT_TRACK: '1' },
  },
});
