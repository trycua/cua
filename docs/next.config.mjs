import { createMDX } from 'fumadocs-mdx/next';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const withMDX = createMDX();
const docsRoot = dirname(fileURLToPath(import.meta.url));

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  turbopack: {
    root: docsRoot,
  },
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
      // Redirect old /api URLs to SDK landing pages
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
      {
        source: '/reference/cua-driver/modality-test-suite',
        destination: '/reference/cua-driver/limits',
        permanent: true,
      },
      {
        source: '/reference/cua-driver/modality-test-suite.mdx',
        destination: '/reference/cua-driver/limits',
        permanent: true,
      },
      {
        source: '/explanation/linux-and-wayland',
        destination: '/reference/cua-driver/limits',
        permanent: true,
      },
      {
        source: '/explanation/linux-and-wayland.mdx',
        destination: '/reference/cua-driver/limits',
        permanent: true,
      },
      {
        source: '/explanation/capture-and-dispatch-modalities',
        destination: '/concepts/capture-and-delivery-modalities',
        permanent: true,
      },
      {
        source: '/explanation/capture-and-dispatch-modalities.mdx',
        destination: '/concepts/capture-and-delivery-modalities',
        permanent: true,
      },
      {
        source: '/explanation/demonstrations-skills-and-trajectories',
        destination: '/concepts',
        permanent: true,
      },
      {
        source: '/explanation/demonstrations-skills-and-trajectories.mdx',
        destination: '/concepts',
        permanent: true,
      },
      {
        source: '/explanation',
        destination: '/concepts',
        permanent: true,
      },
      {
        source: '/explanation.mdx',
        destination: '/concepts',
        permanent: true,
      },
      {
        source: '/explanation/:path*.mdx',
        destination: '/concepts/:path*',
        permanent: true,
      },
      {
        source: '/explanation/:path*',
        destination: '/concepts/:path*',
        permanent: true,
      },
      {
        source: '/tutorials/run-an-agent-in-a-sandbox',
        destination: '/tutorials/your-first-cloud-sandbox',
        permanent: true,
      },
      {
        source: '/tutorials/run-an-agent-in-a-sandbox.mdx',
        destination: '/tutorials/your-first-cloud-sandbox',
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
