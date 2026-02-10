import { defineConfig } from 'tsdown';

export default defineConfig([
  {
    entry: ['./src/index.ts'],
    platform: 'browser',
    dts: true,
    copy: ['./src/styles.css'],
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
