import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { source } from '@/lib/source';
import type { ReactNode } from 'react';

// Stock fumadocs docs layout — sidebar tree from content/docs, no custom
// header/branding. This app is a local MDX preview; production chrome lives on
// the website.
export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout tree={source.pageTree} nav={{ title: 'Docs preview' }}>
      {children}
    </DocsLayout>
  );
}
