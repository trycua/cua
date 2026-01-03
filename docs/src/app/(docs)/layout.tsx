import { baseOptions } from '@/app/layout.config';
import { source } from '@/lib/source';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { Compass, Blocks, BookOpen } from 'lucide-react';
import type { ReactNode } from 'react';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout
      tree={source.pageTree}
      {...baseOptions}
      sidebar={{
        tabs: [
          {
            title: 'Guide',
            description: 'Learn how to build with Cua',
            url: '/guide',
            icon: (
              <span className="flex items-center justify-center">
                <Compass className="size-4" />
              </span>
            ),
          },
          {
            title: 'Examples',
            description: 'Real-world examples',
            url: '/examples',
            icon: (
              <span className="flex items-center justify-center">
                <Blocks className="size-4" />
              </span>
            ),
          },
          {
            title: 'Reference',
            description: 'CLI tools and API reference',
            url: '/reference',
            icon: (
              <span className="flex items-center justify-center">
                <BookOpen className="size-4" />
              </span>
            ),
          },
        ],
      }}
    >
      {children}
    </DocsLayout>
  );
}
