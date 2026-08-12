import './global.css';
import { RootProvider } from 'fumadocs-ui/provider';
import type { ReactNode } from 'react';

// Local MDX preview only — the production docs (and their analytics, cuabot
// assistant, search index, and custom branding) are served from the website.

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <link rel="icon" href="/docs/favicon.ico" sizes="any" />
      </head>
      <body className="flex min-h-screen flex-col" suppressHydrationWarning>
        <RootProvider search={{ enabled: false }} theme={{ defaultTheme: 'dark' }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
