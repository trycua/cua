'use client';

import { useEffect } from 'react';
import posthog from 'posthog-js';

export function AnalyticsTracker() {
  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      const link = target.closest('a');

      if (!link) return;

      const href = link.href;
      const text = link.textContent || link.getAttribute('aria-label') || '';

      if (href.includes('github.com/trycua')) {
        posthog.capture('github_link_clicked', {
          url: href,
          link_text: text,
          page: window.location.pathname,
        });
      }

      if (href.includes('discord.com/invite') || href.includes('discord.gg')) {
        posthog.capture('discord_link_clicked', {
          url: href,
          link_text: text,
          page: window.location.pathname,
        });
      }

      if (
        (href.includes('trycua.com') && !href.includes('trycua.com/docs')) ||
        href.includes('cua.ai')
      ) {
        posthog.capture('main_website_clicked', {
          url: href,
          link_text: text,
          page: window.location.pathname,
        });
      }

      if (link.hostname && link.hostname !== window.location.hostname) {
        if (
          href.includes('github.com/trycua') ||
          href.includes('discord.com') ||
          href.includes('trycua.com') ||
          href.includes('cua.ai')
        ) {
          return;
        }

        posthog.capture('external_link_clicked', {
          url: href,
          link_text: text,
          page: window.location.pathname,
          domain: link.hostname,
        });
      }
    };

    document.addEventListener('click', handleClick);

    return () => {
      document.removeEventListener('click', handleClick);
    };
  }, []);

  return null;
}
