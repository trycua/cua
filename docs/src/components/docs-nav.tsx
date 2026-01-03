'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { cn } from 'fumadocs-ui/utils/cn';

const tabs = [
  { name: 'Guide', href: '/guide/get-started/what-is-cua', prefix: '/guide' },
  { name: 'Examples', href: '/examples', prefix: '/examples' },
  { name: 'Reference', href: '/reference/computer-sdk', prefix: '/reference' },
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
