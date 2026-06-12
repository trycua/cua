import { ArrowRight, BookOpen, ChevronRight, Cloud, Code2, FileCode } from 'lucide-react';
import type { Metadata } from 'next';
import Link from 'next/link';

export const metadata: Metadata = {
  title: 'Cua Documentation',
  description:
    'Give every agent a cloud desktop. Built for Claude Code, Codex, OpenClaw, and computer-use agents.',
};

// Navigation card data
const navCards = [
  {
    icon: BookOpen,
    title: 'Guide',
    description: 'Set up your first agent, configure sandboxes, and learn the core concepts.',
    href: '/cua/guide/get-started/what-is-cua',
  },
  {
    icon: Code2,
    title: 'Examples',
    description: 'Form filling, contact export, VPN automation, and more — ready to copy and run.',
    href: '/cua/examples/automation/form-filling',
  },
  {
    icon: FileCode,
    title: 'API Reference',
    description: 'Sandbox, Agent, and Cloud SDK — every class, method, and parameter.',
    href: '/cua/reference/sandbox-sdk',
  },
  {
    icon: Cloud,
    title: 'Cloud',
    description: 'Managed sandboxes for production. Deploy agents at scale without managing infra.',
    href: 'https://cua.ai',
  },
];

// Featured examples data
const featuredExamples = [
  {
    title: 'Windows App Behind VPN',
    description: 'Automate legacy Windows applications securely behind VPN',
    href: '/cua/examples/platform-specific/windows-app-behind-vpn',
  },
  {
    title: 'PDF to Form Automation',
    description: 'Enhance interactions between form filling and local file systems',
    href: '/cua/examples/automation/form-filling',
  },
  {
    title: 'Contact Export',
    description: 'Export contacts from post-event platforms automatically',
    href: '/cua/examples/automation/post-event-contact-export',
  },
  {
    title: 'Complex UI Navigation',
    description: 'Navigate complex UIs with Gemini vision capabilities',
    href: '/cua/examples/ai-models/gemini-complex-ui-navigation',
  },
];

export default function LandingPage() {
  return (
    <div className="relative min-h-screen overflow-hidden bg-[var(--bg-base)] text-[var(--ink-body)]">
      <div className="pointer-events-none absolute inset-0 -top-14">
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_top,rgba(97,188,255,0.14),transparent_32rem)]" />
      </div>

      <section className="relative overflow-hidden pb-16 pt-20 md:pb-24 md:pt-28">
        <div className="container relative mx-auto px-6 lg:px-8">
          <div className="cds-hero-shell grid items-center gap-10 px-8 py-10 lg:grid-cols-2 lg:gap-16 lg:px-12 lg:py-14">
            <div className="cds-hero-grid pointer-events-none" />
            <div className="relative max-w-xl">
              <a
                href="https://github.com/trycua/cua"
                target="_blank"
                rel="noreferrer"
                className="github-badge mb-6 inline-flex items-center gap-2 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 text-xs uppercase tracking-widest text-[var(--ink-muted)] transition-colors hover:border-white/20 hover:bg-white/[0.08]"
              >
                <svg
                  aria-hidden="true"
                  viewBox="0 0 16 16"
                  className="h-3.5 w-3.5"
                  fill="currentColor"
                >
                  <path d="M8 0C3.58 0 0 3.58 0 8a8 8 0 0 0 5.47 7.59c.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.5-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.56 7.56 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
                </svg>
                <span>Open source</span>
                <span className="h-3 w-px bg-white/20" />
                <span className="flex items-center gap-1">
                  <svg
                    aria-hidden="true"
                    viewBox="0 0 16 16"
                    className="h-3 w-3"
                    fill="currentColor"
                  >
                    <path d="M8 .25a.75.75 0 0 1 .673.418l1.882 3.815 4.21.612a.75.75 0 0 1 .416 1.279l-3.046 2.97.719 4.192a.751.751 0 0 1-1.088.791L8 12.347l-3.766 1.98a.75.75 0 0 1-1.088-.79l.72-4.194L.818 6.374a.75.75 0 0 1 .416-1.28l4.21-.611L7.327.668A.75.75 0 0 1 8 .25Zm0 2.445L6.615 5.5a.75.75 0 0 1-.564.41l-3.097.45 2.24 2.184a.75.75 0 0 1 .216.664l-.528 3.084 2.769-1.456a.75.75 0 0 1 .698 0l2.77 1.456-.53-3.084a.75.75 0 0 1 .216-.664l2.24-2.183-3.096-.45a.75.75 0 0 1-.564-.41L8 2.694Z" />
                  </svg>
                  13.1k
                </span>
              </a>

              <h1
                className="not-italic text-4xl font-semibold text-[var(--ink-strong)] sm:text-5xl md:text-6xl"
                style={{ fontFamily: 'var(--font-urbanist)', letterSpacing: '-0.035em' }}
              >
                Cua Docs
              </h1>

              <p className="mt-6 bg-gradient-to-r from-[var(--ink-body)] to-[var(--ink-muted)] bg-clip-text text-lg leading-relaxed text-transparent">
                Give every agent a cloud desktop. Built for Claude Code, Codex, OpenClaw, and
                computer-use agents.
              </p>

              <div className="cta-row mt-8 flex flex-wrap gap-4">
                <Link
                  href="/cua/guide/get-started/what-is-cua"
                  className="cds-button-primary inline-flex items-center gap-2 rounded-[10px] px-6 py-3 text-[0.95rem] font-semibold"
                >
                  Get started
                </Link>
                <a
                  href="https://cua.cal.com/f/cua-demo"
                  target="_blank"
                  rel="noreferrer"
                  className="cds-button-secondary inline-flex items-center gap-2 rounded-[10px] px-6 py-3 text-[0.95rem] font-semibold"
                >
                  Book a call
                </a>
                <a
                  href="https://cua.ai"
                  target="_blank"
                  rel="noreferrer"
                  className="cds-button-secondary inline-flex items-center gap-2 rounded-[10px] px-6 py-3 text-[0.95rem] font-semibold"
                >
                  Try Cua Cloud
                </a>
              </div>
            </div>

            <div className="relative hidden lg:block">
              <a
                href="https://cua.ai"
                target="_blank"
                rel="noreferrer"
                className="group relative block"
              >
                <div className="absolute -inset-6 bg-[radial-gradient(circle,rgba(97,188,255,0.18),rgba(88,200,190,0.06),transparent_72%)] blur-2xl transition-opacity duration-300 group-hover:opacity-150" />
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src="/docs/img/hero.png"
                  alt="Cua Platform"
                  className="relative w-full rounded-[10px] transition-all duration-300 group-hover:scale-[1.02] group-hover:shadow-[0_8px_40px_rgba(97,188,255,0.25)]"
                />
              </a>
            </div>
          </div>
        </div>
      </section>

      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {navCards.map((card) => {
            const Icon = card.icon;
            return (
              <Link
                key={card.title}
                href={card.href}
                className="cds-card group relative overflow-hidden p-6 backdrop-blur-sm transition-all duration-300"
              >
                <div className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover:opacity-100">
                  <div className="absolute inset-0 bg-gradient-to-br from-[rgba(97,188,255,0.09)] to-[rgba(88,200,190,0.06)]" />
                </div>

                <div className="relative">
                  <div className="mb-4 inline-flex rounded-2xl border border-white/10 bg-[rgba(97,188,255,0.08)] p-3">
                    <Icon className="h-6 w-6 text-[var(--brand-500)]" />
                  </div>
                  <h3 className="mb-2 text-lg font-semibold text-[var(--ink-strong)]">
                    {card.title}
                  </h3>
                  <p className="text-sm leading-relaxed text-[var(--ink-muted)]">
                    {card.description}
                  </p>
                  <div className="mt-4 inline-flex items-center text-sm font-medium text-[var(--brand-500)] opacity-0 transition-all duration-300 group-hover:opacity-100">
                    Explore
                    <ArrowRight className="ml-1 h-4 w-4 transition-transform group-hover:translate-x-1" />
                  </div>
                </div>
              </Link>
            );
          })}
        </div>
      </section>

      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="mb-8 flex items-center justify-between">
          <div>
            <h2
              className="text-2xl font-bold text-[var(--ink-strong)] md:text-3xl"
              style={{ fontFamily: 'var(--font-urbanist)' }}
            >
              Featured Examples
            </h2>
            <p className="mt-2 text-[var(--ink-muted)]">From form filling to VPN automation</p>
          </div>
          <Link
            href="/cua/examples/automation/form-filling"
            className="hidden items-center gap-1 text-sm font-medium text-[var(--ink-muted)] transition-colors hover:text-[var(--ink-strong)] sm:inline-flex"
          >
            View all
            <ChevronRight className="h-4 w-4" />
          </Link>
        </div>

        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {featuredExamples.map((example) => (
            <Link
              key={example.title}
              href={example.href}
              className="cds-card group relative overflow-hidden backdrop-blur-sm transition-all duration-300"
            >
              <div className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover:opacity-100">
                <div className="absolute inset-0 bg-gradient-to-br from-[rgba(97,188,255,0.09)] to-[rgba(88,200,190,0.06)]" />
              </div>

              <div className="relative aspect-[16/10] overflow-hidden bg-gradient-to-br from-[var(--bg-elev-2)] via-[rgba(97,188,255,0.08)] to-[var(--bg-elev-1)]">
                <div className="absolute inset-0 flex items-center justify-center">
                  <div
                    className="h-20 w-20 rounded-2xl border border-white/10 bg-[rgba(7,8,10,0.82)] backdrop-blur-sm transition-transform duration-300 group-hover:scale-105"
                    style={{
                      boxShadow: '0 16px 40px rgba(0, 0, 0, 0.32)',
                    }}
                  >
                    <div className="flex h-full w-full items-center justify-center">
                      <Code2 className="h-8 w-8 text-[var(--brand-500)]" />
                    </div>
                  </div>
                </div>
                <div className="absolute inset-0 bg-gradient-to-t from-[rgba(10,12,16,0.88)] to-transparent" />
              </div>

              <div className="relative p-4">
                <h3 className="font-semibold text-[var(--ink-strong)] transition-colors group-hover:text-[var(--brand-500)]">
                  {example.title}
                </h3>
                <p className="mt-1 line-clamp-2 text-sm text-[var(--ink-muted)]">
                  {example.description}
                </p>
              </div>
            </Link>
          ))}
        </div>

        {/* Mobile "View all" link */}
        <div className="mt-6 text-center sm:hidden">
          <Link
            href="/cua/examples/automation/form-filling"
            className="inline-flex items-center gap-1 text-sm font-medium text-[var(--ink-muted)] transition-colors hover:text-[var(--ink-strong)]"
          >
            View all examples
            <ChevronRight className="h-4 w-4" />
          </Link>
        </div>
      </section>

      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="cds-card p-8 backdrop-blur-sm">
          <div className="mx-auto max-w-2xl text-center">
            <h2
              className="text-xl font-bold text-[var(--ink-strong)] md:text-2xl"
              style={{ fontFamily: 'var(--font-urbanist)' }}
            >
              Ready to build?
            </h2>
            <p className="mt-2 text-[var(--ink-muted)]">
              Go from zero to a running agent in under five minutes.
            </p>
            <div className="mt-6 flex flex-wrap items-center justify-center gap-3">
              <Link
                href="/cua/guide/get-started/what-is-cua"
                className="cds-button-primary inline-flex items-center gap-2 rounded-[10px] px-6 py-3 text-[0.95rem] font-semibold"
              >
                Get started
              </Link>
              <Link
                href="/cua/reference/sandbox-sdk"
                className="cds-button-secondary inline-flex items-center gap-2 rounded-[10px] px-6 py-3 text-[0.95rem] font-semibold"
              >
                API Reference
              </Link>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
