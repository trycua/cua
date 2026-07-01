import './global.css';
import { RootProvider } from 'fumadocs-ui/provider';
import { Geist, Geist_Mono, Urbanist } from 'next/font/google';
import type { ReactNode } from 'react';

// This app is a local MDX preview only — the production docs are served from the
// website. Analytics (PostHog), the cuabot assistant (CopilotKit), and the
// prompt-digest cron live on the website, so they're intentionally absent here.

const geist = Geist({
  subsets: ['latin'],
  variable: '--font-geist-sans',
});

const geistMono = Geist_Mono({
  subsets: ['latin'],
  variable: '--font-geist-mono',
});

const urbanist = Urbanist({
  subsets: ['latin'],
  weight: ['100', '300', '400', '500', '600', '700'],
  style: ['normal', 'italic'],
  variable: '--font-urbanist',
});

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html
      lang="en"
      className={`${geist.variable} ${geistMono.variable} ${urbanist.variable} font-sans`}
      suppressHydrationWarning
    >
      <head>
        <link rel="icon" href="/docs/favicon.ico" sizes="any" />
        <script
          dangerouslySetInnerHTML={{
            __html: `
              // Suppress Radix UI hydration warnings in development
              if (typeof window !== 'undefined') {
                const originalError = console.error;
                console.error = (...args) => {
                  if (args[0]?.includes?.('A tree hydrated but some attributes') ||
                      args[0]?.includes?.('Hydration failed') ||
                      (typeof args[0] === 'string' && args[0].includes('aria-controls'))) {
                    return;
                  }
                  originalError.apply(console, args);
                };
              }
            `,
          }}
        />
      </head>
      <body className="flex min-h-screen flex-col" suppressHydrationWarning>
        <RootProvider search={{ enabled: false }} theme={{ defaultTheme: 'dark' }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
