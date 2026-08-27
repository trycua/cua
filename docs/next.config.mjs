import { createMDX } from 'fumadocs-mdx/next';
import { hostname, networkInterfaces } from 'node:os';

const withMDX = createMDX();

const localDevOrigins = [
  'localhost',
  '127.0.0.1',
  ...Object.values(networkInterfaces())
    .flat()
    .filter((address) => address && !address.internal)
    .map((address) => address.address),
  // networkInterfaces() yields addresses only, so reaching the preview by name
  // — a Tailscale MagicDNS host, a .local name — otherwise 403s on /_next/*
  // and the page loads unhydrated: server-rendered HTML, no working sidebar.
  hostname(),
  // `**` matches any depth; a plain `*` matches a single segment and so would
  // miss MagicDNS names like <host>.<tailnet>.ts.net.
  '**.ts.net',
  '*.local',
];

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  trailingSlash: false,
  basePath: '/docs',
  assetPrefix: '/docs',
  allowedDevOrigins: [...new Set(localDevOrigins)],
  async redirects() {
    return [
      {
        source: '/',
        destination: '/docs',
        basePath: false,
        permanent: false,
      },
      {
        source: '/cuabench',
        destination: '/concepts/what-is-cua-bench',
        permanent: true,
      },
      {
        source: '/tutorials/your-first-cua-driver-python-app',
        destination: '/how-to-guides/driver/use-sdk-in-process',
        permanent: true,
      },
      {
        source: '/tutorials/your-first-cua-driver-typescript-app',
        destination: '/how-to-guides/driver/use-sdk-in-process',
        permanent: true,
      },
      {
        source: '/tutorials/verify-a-desktop-action-with-cua-driver',
        destination: '/how-to-guides/driver/verify-a-desktop-action',
        permanent: true,
      },
      {
        source: '/tutorials/your-first-cloud-sandbox',
        destination: '/tutorials/your-first-cloud-fleet',
        permanent: true,
      },
      {
        source: '/tutorials/your-first-local-sandbox',
        destination: '/tutorials/your-first-cloud-fleet',
        permanent: true,
      },
      {
        source: '/how-to-guides/sandbox/snapshots',
        destination: '/how-to-guides/sandbox/images',
        permanent: true,
      },
      {
        source: '/how-to-guides/sandbox/lifecycle',
        destination: '/concepts/sandbox-lifecycle',
        permanent: true,
      },
      {
        source: '/how-to-guides/fleets/configure-run-cua-fleets',
        destination: '/how-to-guides/sandbox/configure-pool-with-terraform',
        permanent: true,
      },
      {
        source: '/how-to-guides/(fleets)/configure-run-cua-fleets',
        destination: '/how-to-guides/sandbox/configure-pool-with-terraform',
        permanent: true,
      },
      {
        source: '/how-to-guides/skills/record-a-demonstration',
        destination: '/how-to-guides',
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
        hostname: 'github.com',
      },
    ],
  },
};

export default withMDX(config);
