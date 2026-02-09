'use client';

import { useState, useRef, useEffect } from 'react';
import Link from 'next/link';
import Image from 'next/image';
import { usePathname } from 'next/navigation';
import { cn } from 'fumadocs-ui/utils/cn';
import { SearchToggle } from 'fumadocs-ui/components/layout/search-toggle';
import { ThemeToggle } from 'fumadocs-ui/components/layout/theme-toggle';
import { ChevronsUpDown, Check, Menu } from 'lucide-react';
import { useSidebar } from 'fumadocs-ui/provider';
import LogoBlack from '@/assets/cuala-icon-black.svg';
import LogoWhite from '@/assets/cuala-icon-white.svg';
import CuaBenchLogoBlack from '@/assets/cuabench-logo-black.svg';
import CuaBenchLogoWhite from '@/assets/cuabench-logo-white.svg';
import McpBlack from '@/assets/mcp-black.svg';
import McpWhite from '@/assets/mcp-white.svg';
import LumeIconBlack from '@/assets/lume-icon-black.svg';
import LumeIconWhite from '@/assets/lume-icon-white.svg';

const docsSites = [
  {
    name: 'Cua',
    label: 'Docs',
    href: '/cua/guide/get-started/what-is-cua',
    prefix: '/cua',
    isDefault: true,
    description: 'Computer Use Agent SDK',
    logoBlack: LogoBlack,
    logoWhite: LogoWhite,
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 20,
    dropdownIconHeight: 20,
    navTabs: [
      { name: 'Guide', href: '/cua/guide/get-started/what-is-cua', prefix: '/cua/guide' },
      { name: 'Examples', href: '/cua/examples/automation/form-filling', prefix: '/cua/examples' },
      { name: 'Reference', href: '/cua/reference/computer-sdk', prefix: '/cua/reference' },
    ],
  },
  {
    name: 'Cua Bench',
    label: 'Docs',
    href: '/cuabench/guide/getting-started/introduction',
    prefix: '/cuabench',
    isDefault: false,
    description: 'Benchmarking toolkit',
    logoBlack: CuaBenchLogoBlack,
    logoWhite: CuaBenchLogoWhite,
    iconWidth: 36,
    iconHeight: 22,
    dropdownIconWidth: 30,
    dropdownIconHeight: 18,
    navTabs: [
      {
        name: 'Guide',
        href: '/cuabench/guide/getting-started/introduction',
        prefix: '/cuabench/guide',
      },
      {
        name: 'Examples',
        href: '/cuabench/guide/examples/custom-agent',
        prefix: '/cuabench/guide/examples',
      },
      {
        name: 'Reference',
        href: '/cuabench/reference/cli-reference',
        prefix: '/cuabench/reference',
      },
    ],
  },
  {
    name: 'Lume',
    label: 'Docs',
    href: '/lume/guide/getting-started/introduction',
    prefix: '/lume',
    isDefault: false,
    description: 'macOS VM CLI and Framework',
    logoBlack: LumeIconBlack,
    logoWhite: LumeIconWhite,
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 20,
    dropdownIconHeight: 20,
    navTabs: [
      {
        name: 'Guide',
        href: '/lume/guide/getting-started/introduction',
        prefix: '/lume/guide',
      },
      {
        name: 'Examples',
        href: '/lume/examples',
        prefix: '/lume/examples',
      },
      {
        name: 'Reference',
        href: '/lume/reference/cli-reference',
        prefix: '/lume/reference',
      },
    ],
  },
  {
    name: 'Cua-Bot',
    label: 'Docs',
    href: '/cuabot/guide/getting-started/introduction',
    prefix: '/cuabot',
    isDefault: false,
    description: 'Co-op computer-use for any agent',
    logoBlack: LogoBlack,
    logoWhite: LogoWhite,
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 20,
    dropdownIconHeight: 20,
    navTabs: [
      {
        name: 'Guide',
        href: '/cuabot/guide/getting-started/introduction',
        prefix: '/cuabot/guide',
      },
      {
        name: 'Reference',
        href: '/cuabot/reference',
        prefix: '/cuabot/reference',
      },
    ],
  },
];

export function CustomHeader() {
  const pathname = usePathname();
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const { open: sidebarOpen, setOpen: setSidebarOpen } = useSidebar();

  // Determine current docs site based on pathname
  const currentSite =
    docsSites.find((site) => !site.isDefault && pathname.startsWith(site.prefix)) ||
    docsSites.find((site) => site.isDefault) ||
    docsSites[0];

  // Close dropdown when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  return (
    <header className="fixed top-0 left-0 right-0 z-40 border-b border-fd-border bg-fd-background/80 backdrop-blur-sm">
      <div className="container mx-auto flex h-14 items-center justify-between px-4">
        {/* Left: Logo and Nav */}
        <div className="flex items-center gap-4 md:gap-6">
          {/* Hamburger Menu Button - visible on mobile only, opens native fumadocs sidebar */}
          <button
            onClick={() => setSidebarOpen(!sidebarOpen)}
            className="inline-flex md:hidden h-9 w-9 items-center justify-center rounded-md text-fd-muted-foreground transition-colors hover:bg-fd-accent hover:text-fd-foreground"
            aria-label="Toggle sidebar"
          >
            <Menu className="h-5 w-5" />
          </button>

          {/* Docs Switcher - wraps logo, name, and label */}
          <div className="relative" ref={dropdownRef}>
            <button
              onClick={() => setIsOpen(!isOpen)}
              className="flex items-center gap-2 rounded-lg px-2 py-1.5 transition-colors hover:bg-fd-accent"
            >
              {/* Logo */}
              {currentSite.logoBlack && currentSite.logoWhite && (
                <div
                  className="relative flex h-6 w-6 shrink-0 items-center justify-center"
                >
                  <Image
                    width={currentSite.iconWidth}
                    height={currentSite.iconHeight}
                    src={currentSite.logoBlack}
                    aria-label="Logo"
                    className="block dark:hidden"
                    style={{ width: currentSite.iconWidth, height: currentSite.iconHeight }}
                    alt="Logo"
                  />
                  <Image
                    width={currentSite.iconWidth}
                    height={currentSite.iconHeight}
                    src={currentSite.logoWhite}
                    aria-label="Logo"
                    className="hidden dark:block"
                    style={{ width: currentSite.iconWidth, height: currentSite.iconHeight }}
                    alt="Logo"
                  />
                </div>
              )}
              {/* Site name and label */}
              <span className="font-semibold whitespace-nowrap" style={{ fontFamily: 'var(--font-urbanist)' }}>
                {currentSite.name}
              </span>
              <span className="text-sky-500 font-medium whitespace-nowrap">{currentSite.label}</span>
              {/* Up/down chevron */}
              <ChevronsUpDown className="h-4 w-4 text-fd-muted-foreground" />
            </button>

            {/* Dropdown menu */}
            {isOpen && (
              <div className="absolute left-0 top-full mt-1 w-64 rounded-lg border border-fd-border bg-fd-popover p-1 shadow-lg">
                {docsSites.map((site, index) => {
                  const isActive = site.name === currentSite.name;
                  return (
                    <div key={site.name}>
                      {index > 0 && <div className="mx-2 my-1 border-t border-fd-border" />}
                      <Link
                        href={site.href}
                        onClick={() => setIsOpen(false)}
                        className={cn(
                          'flex items-center gap-3 rounded-md px-3 py-2.5 transition-colors',
                          isActive
                            ? 'bg-fd-accent text-fd-foreground'
                            : 'text-fd-muted-foreground hover:bg-fd-accent hover:text-fd-foreground'
                        )}
                      >
                        {/* Site logo in dropdown */}
                        <div className="flex h-5 w-8 shrink-0 items-center justify-center">
                          {site.logoBlack && site.logoWhite ? (
                            <>
                              <Image
                                width={site.dropdownIconWidth}
                                height={site.dropdownIconHeight}
                                src={site.logoBlack}
                                aria-label={`${site.name} logo`}
                                className="block dark:hidden"
                                style={{
                                  width: site.dropdownIconWidth,
                                  height: site.dropdownIconHeight,
                                }}
                                alt={`${site.name} logo`}
                              />
                              <Image
                                width={site.dropdownIconWidth}
                                height={site.dropdownIconHeight}
                                src={site.logoWhite}
                                aria-label={`${site.name} logo`}
                                className="hidden dark:block"
                                style={{
                                  width: site.dropdownIconWidth,
                                  height: site.dropdownIconHeight,
                                }}
                                alt={`${site.name} logo`}
                              />
                            </>
                          ) : (
                            <span className="text-sm font-medium text-fd-muted-foreground">
                              {site.name.charAt(0)}
                            </span>
                          )}
                        </div>
                        <div className="flex-1">
                          <div className="font-medium text-fd-foreground">{site.name}</div>
                          <div className="text-xs text-fd-muted-foreground">{site.description}</div>
                        </div>
                        {isActive && <Check className="h-4 w-4 text-sky-500" />}
                      </Link>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {/* Navigation */}
          <nav className="hidden items-center gap-4 md:flex">
            {currentSite.navTabs.map((tab) => {
              const isActive = pathname.startsWith(tab.prefix);
              return (
                <Link
                  key={tab.name}
                  href={tab.href}
                  className={cn(
                    'text-sm font-medium transition-colors',
                    isActive
                      ? 'text-fd-foreground'
                      : 'text-fd-muted-foreground hover:text-fd-foreground'
                  )}
                >
                  {tab.name}
                </Link>
              );
            })}
          </nav>
        </div>

        {/* Right: Search, Theme, Icons */}
        <div className="flex items-center gap-2">
          <SearchToggle />

          {/* MCP - hidden on mobile */}
          <Link
            href="/cua/vibe-coding-mcp"
            className="hidden sm:inline-flex h-9 w-9 items-center justify-center rounded-md text-fd-muted-foreground transition-colors hover:bg-fd-accent hover:text-fd-foreground"
            title="Vibe Coding MCP"
          >
            <Image src={McpBlack} alt="MCP" width={20} height={20} className="block dark:hidden" />
            <Image src={McpWhite} alt="MCP" width={20} height={20} className="hidden dark:block" />
          </Link>

          <ThemeToggle />

          {/* Discord - hidden on mobile */}
          <a
            href="https://discord.com/invite/mVnXXpdE85"
            target="_blank"
            rel="noreferrer"
            className="hidden sm:inline-flex h-9 w-9 items-center justify-center rounded-md text-fd-muted-foreground transition-colors hover:bg-fd-accent hover:text-fd-foreground"
          >
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-5 w-5">
              <path d="M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z" />
            </svg>
          </a>

          {/* GitHub - hidden on mobile */}
          <a
            href="https://github.com/trycua/cua"
            target="_blank"
            rel="noreferrer"
            className="hidden sm:inline-flex h-9 w-9 items-center justify-center rounded-md text-fd-muted-foreground transition-colors hover:bg-fd-accent hover:text-fd-foreground"
          >
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-5 w-5">
              <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
            </svg>
          </a>
        </div>
      </div>
    </header>
  );
}
