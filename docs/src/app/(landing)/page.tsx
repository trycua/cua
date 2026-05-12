import {
  ArrowRight,
  Bot,
  Box,
  Calendar,
  ChevronRight,
  Cloud,
  Code2,
  ExternalLink,
  FileCode,
  Gauge,
  Github,
  Monitor,
  Play,
  Rocket,
  Terminal,
  Workflow,
} from 'lucide-react';
import type { Metadata } from 'next';
import Link from 'next/link';

export const metadata: Metadata = {
  title: 'Cua Documentation',
  description:
    'Guides, examples, and API reference for building computer-use agents with cloud and local desktop sandboxes.',
};

const primaryLinks = [
  {
    icon: Rocket,
    title: 'Start building',
    description: 'Create a sandbox, connect an agent, and run your first desktop task.',
    href: '/cua/guide/get-started/quickstart',
  },
  {
    icon: Box,
    title: 'Set up sandboxes',
    description: 'Choose cloud or local desktops across Linux, macOS, Windows, and Android.',
    href: '/cua/guide/get-started/set-up-sandbox',
  },
  {
    icon: Bot,
    title: 'Use the Agent SDK',
    description: 'Wire models, tools, callbacks, and traces into production agent loops.',
    href: '/cua/guide/get-started/using-agent-sdk',
  },
  {
    icon: FileCode,
    title: 'Read the API',
    description: 'Reference material for the sandbox, agent, computer, and CLI surfaces.',
    href: '/cua/reference/sandbox-sdk',
  },
];

const productCards = [
  {
    icon: Bot,
    title: 'Cua',
    description: 'Agent framework and desktop sandboxes for computer-use tasks.',
    href: '/cua/guide/get-started/what-is-cua',
    links: [
      { label: 'Guide', href: '/cua/guide/get-started/what-is-cua' },
      { label: 'Examples', href: '/cua/examples' },
      { label: 'Reference', href: '/cua/reference/sandbox-sdk' },
    ],
  },
  {
    icon: Terminal,
    title: 'Cua Driver',
    description: 'Background macOS computer-use driver exposed as CLI and MCP tools.',
    href: '/cua-driver/guide/getting-started/introduction',
    links: [
      { label: 'Quickstart', href: '/cua-driver/guide/getting-started/quickstart' },
      { label: 'MCP tools', href: '/cua-driver/reference/mcp-tools' },
    ],
  },
  {
    icon: Gauge,
    title: 'Cua Bench',
    description: 'Benchmarks, tasks, traces, and RL workflows for computer-use agents.',
    href: '/cuabench/guide/getting-started/introduction',
    links: [
      { label: 'First steps', href: '/cuabench/guide/getting-started/first-steps' },
      { label: 'Tasks', href: '/cuabench/guide/fundamentals/tasks' },
    ],
  },
  {
    icon: Monitor,
    title: 'Lume',
    description: 'Local macOS VM management and automation on Apple Silicon.',
    href: '/lume/guide/getting-started/introduction',
    links: [
      { label: 'Install', href: '/lume/guide/getting-started/installation' },
      { label: 'CLI', href: '/lume/reference/cli-reference' },
    ],
  },
  {
    icon: Workflow,
    title: 'Cua-Bot',
    description: 'Co-op computer-use sessions for running agents together.',
    href: '/cuabot/guide/getting-started/introduction',
    links: [
      { label: 'Intro', href: '/cuabot/guide/getting-started/introduction' },
      { label: 'Changelog', href: '/cuabot/reference/changelog' },
    ],
  },
];

const featuredExamples = [
  {
    title: 'PDF to Form Automation',
    description: 'Fill desktop forms from local files with an agent loop.',
    href: '/cua/examples/automation/form-filling',
    image: '/docs/img/agent_gradio_ui.png',
  },
  {
    title: 'Windows App Behind VPN',
    description: 'Automate private Windows software inside a managed desktop.',
    href: '/cua/examples/platform-specific/windows-app-behind-vpn',
    image: '/docs/img/sandbox-create.png',
  },
  {
    title: 'Post-Event Contact Export',
    description: 'Move data out of browser-only workflows without brittle scripts.',
    href: '/cua/examples/automation/post-event-contact-export',
    image: '/docs/img/computer.png',
  },
  {
    title: 'Complex UI Navigation',
    description: 'Use vision models to navigate dense interfaces and recover from drift.',
    href: '/cua/examples/ai-models/gemini-complex-ui-navigation',
    image: '/docs/img/grounding-with-gemini3.gif',
  },
];

const stats = [
  { label: 'SDKs', value: 'Agent + Sandbox' },
  { label: 'Runtimes', value: 'Cloud + local' },
  { label: 'Surfaces', value: 'CLI + MCP' },
];

export default function LandingPage() {
  return (
    <div className="relative min-h-screen overflow-hidden bg-[var(--bg-base)] text-[var(--ink-body)]">
      <div className="pointer-events-none absolute inset-0 -top-20 bg-[radial-gradient(circle_at_top,rgba(97,188,255,0.12),transparent_30rem)]" />

      <section className="relative pb-10 pt-16 md:pb-12 md:pt-24">
        <div className="container relative mx-auto px-4 sm:px-6 lg:px-8">
          <div className="cds-hero-shell grid min-w-0 items-center gap-8 p-6 md:p-8 lg:grid-cols-[minmax(0,0.95fr)_minmax(420px,1.05fr)] lg:gap-10 lg:p-10">
            <div className="cds-hero-grid pointer-events-none" />

            <div className="relative min-w-0 max-w-2xl">
              <a
                href="https://github.com/trycua/cua"
                target="_blank"
                rel="noreferrer"
                className="github-badge mb-5 inline-flex items-center gap-2 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 text-xs text-[var(--ink-muted)] transition-colors hover:border-white/20 hover:bg-white/[0.08]"
              >
                <Github className="h-3.5 w-3.5" />
                <span>Open source</span>
                <span className="h-3 w-px bg-white/20" />
                <span>13.1k stars</span>
              </a>

              <h1
                className="not-italic text-4xl font-semibold text-[var(--ink-strong)] sm:text-5xl md:text-6xl"
                style={{ fontFamily: 'var(--font-urbanist)' }}
              >
                Cua Docs
              </h1>

              <p className="mt-5 max-w-xl text-lg leading-relaxed text-[var(--ink-body)]">
                Build computer-use agents that can see, click, type, and run code in real desktop
                sandboxes.
              </p>

              <div className="mt-7 flex flex-wrap gap-3">
                <Link
                  href="/cua/guide/get-started/quickstart"
                  className="cds-button-primary inline-flex items-center gap-2 rounded-lg px-5 py-3 text-sm font-semibold"
                >
                  <Play className="h-4 w-4" />
                  Quickstart
                </Link>
                <Link
                  href="/cua/examples"
                  className="cds-button-secondary inline-flex items-center gap-2 rounded-lg px-5 py-3 text-sm font-semibold"
                >
                  <Code2 className="h-4 w-4" />
                  Examples
                </Link>
                <a
                  href="https://cua.cal.com/f/cua-demo"
                  target="_blank"
                  rel="noreferrer"
                  className="cds-button-secondary inline-flex items-center gap-2 rounded-lg px-5 py-3 text-sm font-semibold"
                >
                  <Calendar className="h-4 w-4" />
                  Book a call
                </a>
              </div>

              <dl className="mt-8 grid max-w-xl grid-cols-1 gap-4 border-t border-white/10 pt-5 sm:grid-cols-3">
                {stats.map((stat) => (
                  <div key={stat.label}>
                    <dt className="text-xs text-[var(--ink-muted)]">{stat.label}</dt>
                    <dd className="mt-1 text-sm font-semibold text-[var(--ink-strong)]">
                      {stat.value}
                    </dd>
                  </div>
                ))}
              </dl>
            </div>

            <div className="relative min-w-0">
              <a
                href="https://cua.ai"
                target="_blank"
                rel="noreferrer"
                className="group block"
                aria-label="Open Cua Cloud"
              >
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src="/docs/img/hero.png"
                  alt="Cua cloud desktop overview"
                  className="relative aspect-[16/10] w-full max-w-full rounded-lg border border-white/10 object-cover shadow-[0_24px_70px_rgba(0,0,0,0.35)] transition-transform duration-300 group-hover:scale-[1.01]"
                />
              </a>
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
                <Link
                  href="/cua/guide/sandbox/lifecycle"
                  className="cds-mini-link group inline-flex items-center justify-between rounded-lg border border-white/10 bg-white/[0.03] px-4 py-3 text-sm text-[var(--ink-body)] hover:border-[rgba(97,188,255,0.38)] hover:text-[var(--ink-strong)]"
                >
                  Sandbox lifecycle
                  <ArrowRight className="h-4 w-4 transition-transform group-hover:translate-x-1" />
                </Link>
                <Link
                  href="/cua/guide/fundamentals/vlms"
                  className="cds-mini-link group inline-flex items-center justify-between rounded-lg border border-white/10 bg-white/[0.03] px-4 py-3 text-sm text-[var(--ink-body)] hover:border-[rgba(97,188,255,0.38)] hover:text-[var(--ink-strong)]"
                >
                  Model support
                  <ArrowRight className="h-4 w-4 transition-transform group-hover:translate-x-1" />
                </Link>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section className="relative container mx-auto px-4 py-8 sm:px-6 lg:px-8">
        <div className="mb-5 flex items-end justify-between gap-4">
          <div>
            <h2 className="text-2xl font-semibold text-[var(--ink-strong)]">Choose a path</h2>
            <p className="mt-2 text-sm text-[var(--ink-muted)]">
              The shortest route to the docs most builders need first.
            </p>
          </div>
        </div>

        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {primaryLinks.map((card) => {
            const Icon = card.icon;
            return (
              <Link key={card.title} href={card.href} className="cds-card group p-5">
                <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-lg border border-white/10 bg-[rgba(97,188,255,0.08)]">
                  <Icon className="h-5 w-5 text-[var(--brand-500)]" />
                </div>
                <h3 className="text-base font-semibold text-[var(--ink-strong)]">{card.title}</h3>
                <p className="mt-2 text-sm leading-relaxed text-[var(--ink-muted)]">
                  {card.description}
                </p>
                <span className="mt-4 inline-flex items-center gap-1 text-sm font-medium text-[var(--brand-500)]">
                  Open
                  <ArrowRight className="h-4 w-4 transition-transform group-hover:translate-x-1" />
                </span>
              </Link>
            );
          })}
        </div>
      </section>

      <section className="relative container mx-auto px-4 py-8 sm:px-6 lg:px-8">
        <div className="mb-5 flex items-end justify-between gap-4">
          <div>
            <h2 className="text-2xl font-semibold text-[var(--ink-strong)]">Browse by product</h2>
            <p className="mt-2 text-sm text-[var(--ink-muted)]">
              Each docs set keeps its guide, examples, and references scoped to the product.
            </p>
          </div>
        </div>

        <div className="grid gap-3 lg:grid-cols-5">
          {productCards.map((card) => {
            const Icon = card.icon;
            return (
              <Link key={card.title} href={card.href} className="cds-card group flex flex-col p-5">
                <div className="mb-4 flex items-center gap-3">
                  <span className="flex h-9 w-9 items-center justify-center rounded-lg border border-white/10 bg-white/[0.04]">
                    <Icon className="h-5 w-5 text-[var(--brand-500)]" />
                  </span>
                  <h3 className="font-semibold text-[var(--ink-strong)]">{card.title}</h3>
                </div>
                <p className="text-sm leading-relaxed text-[var(--ink-muted)]">
                  {card.description}
                </p>
                <div className="mt-5 flex flex-wrap gap-2">
                  {card.links.map((link) => (
                    <span
                      key={link.href}
                      className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-[var(--ink-muted)]"
                    >
                      {link.label}
                    </span>
                  ))}
                </div>
              </Link>
            );
          })}
        </div>
      </section>

      <section className="relative container mx-auto px-4 py-8 sm:px-6 lg:px-8">
        <div className="mb-5 flex items-center justify-between gap-4">
          <div>
            <h2 className="text-2xl font-semibold text-[var(--ink-strong)]">Featured examples</h2>
            <p className="mt-2 text-sm text-[var(--ink-muted)]">
              Practical workflows that show the SDKs in context.
            </p>
          </div>
          <Link
            href="/cua/examples"
            className="hidden items-center gap-1 text-sm font-medium text-[var(--ink-muted)] transition-colors hover:text-[var(--ink-strong)] sm:inline-flex"
          >
            View all
            <ChevronRight className="h-4 w-4" />
          </Link>
        </div>

        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {featuredExamples.map((example) => (
            <Link
              key={example.title}
              href={example.href}
              className="cds-card group overflow-hidden"
            >
              <div className="aspect-[16/10] overflow-hidden border-b border-white/10 bg-[var(--bg-elev-2)]">
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src={example.image}
                  alt=""
                  className="h-full w-full object-cover opacity-90 transition-transform duration-300 group-hover:scale-[1.03]"
                />
              </div>
              <div className="p-4">
                <h3 className="text-sm font-semibold text-[var(--ink-strong)] transition-colors group-hover:text-[var(--brand-500)]">
                  {example.title}
                </h3>
                <p className="mt-2 line-clamp-2 text-sm leading-relaxed text-[var(--ink-muted)]">
                  {example.description}
                </p>
              </div>
            </Link>
          ))}
        </div>

        <div className="mt-5 text-center sm:hidden">
          <Link
            href="/cua/examples"
            className="inline-flex items-center gap-1 text-sm font-medium text-[var(--ink-muted)] transition-colors hover:text-[var(--ink-strong)]"
          >
            View all examples
            <ChevronRight className="h-4 w-4" />
          </Link>
        </div>
      </section>

      <section className="relative container mx-auto px-4 py-8 pb-16 sm:px-6 lg:px-8">
        <div className="flex flex-col gap-5 rounded-lg border border-white/10 bg-[rgba(255,255,255,0.03)] px-5 py-6 md:flex-row md:items-center md:justify-between">
          <div>
            <h2 className="text-xl font-semibold text-[var(--ink-strong)]">
              Need a running cloud desktop?
            </h2>
            <p className="mt-2 text-sm text-[var(--ink-muted)]">
              Pair the docs with managed sandboxes when you are ready to scale beyond local setup.
            </p>
          </div>
          <a
            href="https://cua.ai"
            target="_blank"
            rel="noreferrer"
            className="cds-button-secondary inline-flex items-center justify-center gap-2 rounded-lg px-5 py-3 text-sm font-semibold"
          >
            <Cloud className="h-4 w-4" />
            Open Cua Cloud
            <ExternalLink className="h-4 w-4" />
          </a>
        </div>
      </section>
    </div>
  );
}
