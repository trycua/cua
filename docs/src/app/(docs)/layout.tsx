import { ScopedDocsLayout } from '@/components/scoped-docs-layout';
import { source } from '@/lib/source';
import type { ReactNode } from 'react';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <div className="pt-14">
      <ScopedDocsLayout tree={source.pageTree}>{children}</ScopedDocsLayout>
    </div>
  );
}
