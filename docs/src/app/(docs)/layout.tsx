import { CustomHeader } from '@/components/custom-header';
import { source } from '@/lib/source';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import type { ReactNode } from 'react';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <>
      <CustomHeader />
      <div className="pt-14">
        <DocsLayout
          tree={source.pageTree}
          nav={{ enabled: false }}
          searchToggle={{ enabled: false }}
          themeSwitch={{ enabled: false }}
          sidebar={{
            // Disable automatic tabs - we use header navigation instead
            tabs: false,
            // Disable collapse/expand button
            collapsible: false,
            // Add top spacing in sidebar
            banner: <div className="h-6" />,
          }}
        >
          {children}
        </DocsLayout>
      </div>
    </>
  );
}
