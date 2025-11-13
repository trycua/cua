'use client';

import posthog from 'posthog-js';
import { PostHogProvider } from 'posthog-js/react';
import { useEffect } from 'react';
import { usePathname, useSearchParams } from 'next/navigation';

if (typeof window !== 'undefined') {
  const apiKey = process.env.NEXT_PUBLIC_POSTHOG_API_KEY;

  if (apiKey) {
    posthog.init(apiKey, {
      api_host: '/docs/api/posthog',
      ui_host: process.env.NEXT_PUBLIC_POSTHOG_HOST,
      person_profiles: 'always',
      capture_pageview: false,
      capture_pageleave: true,
    });
  } else {
    console.warn('[PostHog] API key not configured. Analytics will be disabled.');
  }
}

export function PHProvider({ children }: { children: React.ReactNode }) {
  return <PostHogProvider client={posthog}>{children}</PostHogProvider>;
}

export function PostHogPageView(): null {
  const pathname = usePathname();
  const searchParams = useSearchParams();

  useEffect(() => {
    if (pathname) {
      let url = window.origin + pathname;
      if (searchParams && searchParams.toString()) {
        url = url + `?${searchParams.toString()}`;
      }

      posthog.capture('$pageview', {
        $current_url: url,
      });
    }
  }, [pathname, searchParams]);

  return null;
}
