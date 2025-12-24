import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
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
      // Redirects for documentation restructure (PR #568)
      // Moved quickstart-devs to get-started section
      {
        source: '/quickstart-devs',
        destination: '/get-started/quickstart',
        permanent: true,
      },
      // Moved telemetry to agent section
      {
        source: '/telemetry',
        destination: '/agent/telemetry',
        permanent: true,
      },
      // Removed quickstart-cli, consolidated into main quickstart
      {
        source: '/quickstart-cli',
        destination: '/get-started/quickstart',
        permanent: true,
      },
      // Documentation restructure: 6-section organization
      // Redirect old agent-sdk paths to new agent paths
      {
        source: '/agent-sdk/:path*',
        destination: '/agent/:path*',
        permanent: true,
      },
      // Redirect old computer-sdk paths to new computer paths
      {
        source: '/computer-sdk/:path*',
        destination: '/computer/:path*',
        permanent: true,
      },
      // Redirect old macos-vm-cli-playbook/lume paths to new lume paths
      {
        source: '/macos-vm-cli-playbook/lume/:path*',
        destination: '/lume/:path*',
        permanent: true,
      },
      // Redirect old macos-vm-cli-playbook/lumier paths to new lumier paths
      {
        source: '/macos-vm-cli-playbook/lumier/:path*',
        destination: '/lumier/:path*',
        permanent: true,
      },
      // Redirect old agent-sdk/mcp-server paths to new mcp paths
      {
        source: '/agent-sdk/mcp-server/:path*',
        destination: '/mcp/:path*',
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
