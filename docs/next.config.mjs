import { createMDX } from 'fumadocs-mdx/next';
import { readFileSync } from 'node:fs';
import { hostname, networkInterfaces } from 'node:os';

// Old slug -> current slug, owned by the content tree (also read by docs.cua.ai).
const MOVED = JSON.parse(
  readFileSync(new URL('./content/docs/redirects.json', import.meta.url), 'utf8')
).redirects;

function current(path) {
  let slug = path.replace(/^\/+|\/+$/g, '');
  for (let hops = 0; hops < 8 && Object.hasOwn(MOVED, slug); hops++) {
    slug = MOVED[slug];
  }
  return `/${slug}`;
}

import { generateDocModules } from './scripts/gen-doc-modules.mjs';

// One lazy import per page (see scripts/gen-doc-modules.mjs); regenerated on
// every dev/build start. Restart the dev server after adding a page.
generateDocModules();

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
  // Static generation workers: pinned to what a 4-vCPU CI runner uses, so
  // the build's memory (`pnpm build:budget`, 6 GiB) does not depend on the
  // machine's core count. DOCS_BUILD_CPUS overrides it.
  experimental: {
    cpus: Number(process.env.DOCS_BUILD_CPUS) || 3,
  },
  reactStrictMode: true,
  trailingSlash: false,
  basePath: '/docs',
  assetPrefix: '/docs',
  allowedDevOrigins: [...new Set(localDevOrigins)],
  async redirects() {
    const legacy = [
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
        source: '/reference/cua-env-driver',
        destination: '/reference/cua-spacesd',
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
        destination: '/start-here/create-a-space-with-the-sdk',
        permanent: true,
      },
      {
        source: '/tutorials/your-first-local-sandbox',
        destination: '/start-here/create-a-space-with-the-sdk',
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
        source: '/how-to-guides/sandbox/create-pool-with-python',
        destination: '/how-to-guides/sandbox/create-fleet-capacity',
        permanent: true,
      },
      {
        source: '/how-to-guides/sandbox/create-pool-with-typescript',
        destination: '/how-to-guides/sandbox/create-fleet-capacity',
        permanent: true,
      },
      {
        source: '/how-to-guides/skills/record-a-demonstration',
        destination: '/how-to-guides',
        permanent: true,
      },
    ].map((r) => (r.basePath === false ? r : { ...r, destination: current(r.destination) }));
    const moved = Object.keys(MOVED).map((slug) => ({
      source: `/${slug}`,
      destination: current(slug),
      permanent: true,
    }));
    const seen = new Set(legacy.map((r) => r.source));
    return [...legacy, ...moved.filter((r) => !seen.has(r.source) && r.source !== r.destination)];
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
