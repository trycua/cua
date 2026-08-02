import { createMDX } from 'fumadocs-mdx/next';
import { networkInterfaces } from 'node:os';

const withMDX = createMDX();

const localDevOrigins = [
  'localhost',
  '127.0.0.1',
  ...Object.values(networkInterfaces())
    .flat()
    .filter((address) => address && !address.internal)
    .map((address) => address.address),
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
        destination: '/tutorials/your-first-local-sandbox',
        permanent: true,
      },
      {
        source: '/how-to-guides/sandbox/snapshots',
        destination: '/how-to-guides/sandbox/images',
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
