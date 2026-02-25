import { readFileSync } from 'node:fs';
import { defineConfig } from 'tsdown';

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'));

export default defineConfig([
  {
    entry: ['./src/index.ts'],
    platform: 'browser',
    dts: true,
    copy: ['./src/styles.css'],
    define: {
      __CUA_VERSION__: JSON.stringify(pkg.version),
    },
    external: [
      // React - MUST be external to avoid duplicate React instances
      'react',
      'react-dom',
      'react/jsx-runtime',
      'react/jsx-dev-runtime',
      // Animation (motion v12+) - uses React hooks
      'motion',
      'motion/react',
      // Radix UI - uses React hooks
      /^@radix-ui\/.*/,
      // lucide-react - uses React
      'lucide-react',
      // Note: lottie-react, react-markdown are BUNDLED
      // because they have ESM resolution issues with pnpm file: protocol
    ],
  },
]);
