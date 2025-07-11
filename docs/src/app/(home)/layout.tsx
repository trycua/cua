import { baseOptions } from '@/app/layout.config';
import { source } from '@/lib/source';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { CodeXml, Home } from 'lucide-react';
import type { ReactNode } from 'react';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout
      tree={source.pageTree}
      sidebar={{
        tabs: [
          {
            url: '/home',
            title: 'Home',
            icon: <Home className="ml-1" />,
          },
          {
            url: '/api',
            title: 'API Reference',
            icon: <CodeXml className="ml-1" />,
          },
        ],
      }}
      {...baseOptions}>
      {children}
    </DocsLayout>
  );
}
