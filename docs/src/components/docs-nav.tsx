'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { cn } from 'fumadocs-ui/utils/cn';

const tabs = [
  { name: 'Guide', href: '/cua/guide/get-started/what-is-cua', prefix: '/cua/guide' },
  { name: 'Examples', href: '/cua/examples', prefix: '/cua/examples' },
  { name: 'Reference', href: '/cua/reference/computer-sdk', prefix: '/cua/reference' },
];

export function DocsNav() {
  const pathname = usePathname();

  return (
    <nav className="flex items-center gap-4">
      {tabs.map((tab) => {
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
  );
}
