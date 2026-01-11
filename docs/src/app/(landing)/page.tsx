import { ArrowRight, BookOpen, ChevronRight, Cloud, Code2, FileCode } from 'lucide-react';
import type { Metadata } from 'next';
import Image from 'next/image';
import Link from 'next/link';

export const metadata: Metadata = {
  title: 'Cua Documentation',
  description:
    'Build computer use agents that see screens, click buttons, type and run code — with Cua.',
};

// Navigation card data
const navCards = [
  {
    icon: BookOpen,
    title: 'Guide',
    description:
      'Everything you need to know to use Cua. Dive deep into all of our features and best practices.',
    href: '/cua/guide/get-started/what-is-cua',
  },
  {
    icon: Code2,
    title: 'Examples',
    description:
      'Practical examples built with Cua. Explore guided starting points for your use case.',
    href: '/cua/examples/automation/form-filling',
  },
  {
    icon: FileCode,
    title: 'API Reference',
    description:
      'Technical information about the Cua APIs. Quickly refer to basic descriptions of various programming functionalities.',
    href: '/cua/reference/computer-sdk',
  },
  {
    icon: Cloud,
    title: 'Cloud',
    description:
      'Managed cloud sandboxes for production workloads. Deploy agents at scale with our hosted infrastructure.',
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
    <div className="relative min-h-screen">
      {/* Background image - extends behind header */}
      <div className="pointer-events-none absolute inset-0 -top-14">
        <Image
          src="/docs/img/bg-dark.jpg"
          alt=""
          fill
          className="hidden object-cover object-top opacity-50 dark:block"
          priority
        />
        <Image
          src="/docs/img/bg-light.jpg"
          alt=""
          fill
          className="block object-cover object-top opacity-30 dark:hidden"
          priority
        />
        {/* Gradient overlay */}
        <div className="absolute inset-0 bg-gradient-to-b from-transparent via-fd-background/50 to-fd-background" />
      </div>

      {/* Hero Section */}
      <section className="relative overflow-hidden pb-16 pt-20 md:pb-24 md:pt-28">
        <div className="container relative mx-auto px-6 lg:px-8">
          <div className="grid items-center gap-12 lg:grid-cols-2 lg:gap-16">
            {/* Text content */}
            <div className="max-w-xl">
              {/* Badge */}
              <div className="mb-6 inline-flex items-center gap-2 rounded-lg border border-fd-border bg-fd-card/50 px-3 py-1.5 text-sm backdrop-blur-sm">
                <span className="h-2 w-2 rounded-full bg-emerald-500" />
                <span className="text-fd-muted-foreground">Open Source</span>
              </div>

              <h1
                className="text-4xl font-bold tracking-tight sm:text-5xl md:text-6xl"
                style={{ fontFamily: 'var(--font-urbanist)' }}
              >
                <span className="text-fd-foreground">Cua </span>
                <span className="text-sky-500">Documentation</span>
              </h1>

              <p className="mt-6 text-lg leading-relaxed text-fd-muted-foreground">
                Build computer use agents that see screens, click buttons, type and run code —
                without thinking about infrastructure.
              </p>

              {/* CTA Buttons */}
              <div className="mt-8 flex flex-wrap items-center gap-4">
                <Link
                  href="/cua/guide/get-started/what-is-cua"
                  className="group inline-flex items-center gap-2 rounded-xl bg-fd-foreground px-6 py-2.5 text-sm font-medium text-fd-background transition-all hover:bg-fd-foreground/90"
                >
                  Get Started
                  <ArrowRight className="h-4 w-4 transition-transform group-hover:translate-x-0.5" />
                </Link>
                <a
                  href="https://github.com/trycua/cua"
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 rounded-xl border border-fd-border bg-fd-background/50 px-6 py-2.5 text-sm font-medium backdrop-blur-sm transition-all hover:bg-fd-accent"
                >
                  <svg className="h-5 w-5" viewBox="0 0 24 24" fill="currentColor">
                    <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
                  </svg>
                  Star on GitHub
                </a>
              </div>
            </div>

            {/* Hero image */}
            <div className="relative hidden lg:block">
              <div className="relative mx-auto max-w-md">
                {/* Glow effect behind image */}
                <div className="absolute -inset-4 rounded-3xl bg-gradient-to-br from-sky-500/20 via-emerald-500/10 to-purple-500/20 blur-2xl" />
                <div className="relative overflow-hidden rounded-2xl border border-fd-border/50 bg-fd-card/30 p-4 backdrop-blur-sm">
                  <Image
                    src="/docs/img/hero.png"
                    alt="Cua Platform"
                    width={400}
                    height={400}
                    className="rounded-xl"
                    priority
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* Navigation Cards Section */}
      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {navCards.map((card) => {
            const Icon = card.icon;
            return (
              <Link
                key={card.title}
                href={card.href}
                className="group relative overflow-hidden rounded-xl border border-fd-border bg-fd-card/50 p-6 backdrop-blur-sm transition-all duration-300 hover:border-fd-primary/30 hover:bg-fd-card"
              >
                {/* Hover glow effect */}
                <div className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover:opacity-100">
                  <div className="absolute inset-0 bg-gradient-to-br from-sky-500/5 to-emerald-500/5" />
                </div>

                <div className="relative">
                  <div className="mb-4 inline-flex rounded-lg bg-gradient-to-br from-sky-500/10 to-emerald-500/10 p-3">
                    <Icon className="h-6 w-6 text-sky-500" />
                  </div>
                  <h3 className="mb-2 text-lg font-semibold">{card.title}</h3>
                  <p className="text-sm leading-relaxed text-fd-muted-foreground">
                    {card.description}
                  </p>
                  <div className="mt-4 inline-flex items-center text-sm font-medium text-sky-500 opacity-0 transition-all duration-300 group-hover:opacity-100">
                    Explore
                    <ArrowRight className="ml-1 h-4 w-4 transition-transform group-hover:translate-x-1" />
                  </div>
                </div>
              </Link>
            );
          })}
        </div>
      </section>

      {/* Featured Examples Section */}
      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="mb-8 flex items-center justify-between">
          <div>
            <h2
              className="text-2xl font-bold md:text-3xl"
              style={{ fontFamily: 'var(--font-urbanist)' }}
            >
              Featured Examples
            </h2>
            <p className="mt-2 text-fd-muted-foreground">
              Real-world examples to get you started quickly
            </p>
          </div>
          <Link
            href="/cua/examples/automation/form-filling"
            className="hidden items-center gap-1 text-sm font-medium text-fd-muted-foreground transition-colors hover:text-fd-foreground sm:inline-flex"
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
              className="group relative overflow-hidden rounded-xl border border-fd-border bg-fd-card/50 backdrop-blur-sm transition-all duration-300 hover:border-fd-primary/30 hover:bg-fd-card"
            >
              {/* Gradient header */}
              <div className="relative aspect-[16/10] overflow-hidden bg-gradient-to-br from-fd-muted to-fd-accent">
                {/* Decorative elements */}
                <div className="absolute inset-0 flex items-center justify-center">
                  <div
                    className="h-20 w-20 rounded-xl border border-fd-border/50 bg-fd-background/80 backdrop-blur-sm transition-transform duration-300 group-hover:scale-105"
                    style={{
                      boxShadow: '0 8px 32px rgba(0, 0, 0, 0.1)',
                    }}
                  >
                    <div className="flex h-full w-full items-center justify-center">
                      <Code2 className="h-8 w-8 text-fd-muted-foreground" />
                    </div>
                  </div>
                </div>
                {/* Subtle gradient overlay */}
                <div className="absolute inset-0 bg-gradient-to-t from-fd-card/80 to-transparent" />
              </div>

              <div className="p-4">
                <h3 className="font-semibold transition-colors group-hover:text-sky-500">
                  {example.title}
                </h3>
                <p className="mt-1 line-clamp-2 text-sm text-fd-muted-foreground">
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
            className="inline-flex items-center gap-1 text-sm font-medium text-fd-muted-foreground transition-colors hover:text-fd-foreground"
          >
            View all examples
            <ChevronRight className="h-4 w-4" />
          </Link>
        </div>
      </section>

      {/* Quick links section */}
      <section className="relative container mx-auto px-6 py-12 lg:px-8">
        <div className="rounded-xl border border-fd-border bg-gradient-to-br from-fd-card/50 to-fd-card p-8 backdrop-blur-sm">
          <div className="mx-auto max-w-2xl text-center">
            <h2
              className="text-xl font-bold md:text-2xl"
              style={{ fontFamily: 'var(--font-urbanist)' }}
            >
              Ready to build?
            </h2>
            <p className="mt-2 text-fd-muted-foreground">
              Start building computer-use agents in minutes with our quickstart guide.
            </p>
            <div className="mt-6 flex flex-wrap items-center justify-center gap-3">
              <Link
                href="/cua/guide/get-started/what-is-cua"
                className="inline-flex items-center gap-2 rounded-xl bg-fd-foreground px-6 py-2.5 text-sm font-medium text-fd-background transition-colors hover:bg-fd-foreground/90"
              >
                Get Started
                <ArrowRight className="h-4 w-4" />
              </Link>
              <Link
                href="/cua/reference/computer-sdk"
                className="inline-flex items-center gap-2 rounded-xl border border-fd-border px-6 py-2.5 text-sm font-medium transition-colors hover:bg-fd-accent"
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
