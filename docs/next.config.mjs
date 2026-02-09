import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  serverExternalPackages: ['pino', 'pino-pretty', 'thread-stream'],
  trailingSlash: false,
  basePath: '/docs',
  assetPrefix: '/docs',
  async rewrites() {
    return [
      {
        source: '/:path*.mdx',
        destination: '/llms.mdx/:path*',
      },
    ];
  },
  async redirects() {
    return [
      {
        source: '/',
        destination: '/docs',
        basePath: false, // Important: this bypasses the basePath
        permanent: false,
      },
      // Redirect old docs.cua.ai URLs to cua.ai/docs with 301 for SEO
      // This handles URLs that Google has indexed from the old domain
      {
        source: '/:path*',
        has: [
          {
            type: 'host',
            value: 'docs.cua.ai',
          },
        ],
        destination: 'https://cua.ai/docs/:path*',
        permanent: true, // 301 redirect to preserve SEO authority
        basePath: false,
      },
      // Redirect old api.mdx paths to SDK landing pages
      {
        source: '/cua/reference/computer-sdk/api',
        destination: '/cua/reference/computer-sdk',
        permanent: true,
      },
      {
        source: '/cua/reference/agent-sdk/api',
        destination: '/cua/reference/agent-sdk',
        permanent: true,
      },
      // Redirects for old URLs
      {
        source: '/quickstart-devs',
        destination: '/get-started/quickstart',
        permanent: true,
      },
      {
        source: '/quickstart-cli',
        destination: '/get-started/quickstart',
        permanent: true,
      },
    ];
  },
  images: {
    dangerouslyAllowSVG: true,
    remotePatterns: [
      {
        protocol: 'https',
        hostname: 'img.shields.io',
      },
      {
        protocol: 'https',
        hostname: 'starchart.cc',
      },
      {
        protocol: 'https',
        hostname: 'github.com',
      },
    ],
  },
};

export default withMDX(config);
