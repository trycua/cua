'use client';

import { useEffect, useState } from 'react';
import posthog from 'posthog-js';

export function CookieConsent() {
  const [isVisible, setIsVisible] = useState(false);

  useEffect(() => {
    // Check if user has already accepted cookies
    const hasAccepted = localStorage.getItem('cookie-consent');
    if (!hasAccepted) {
      setIsVisible(true);
    }
  }, []);

  const handleAccept = () => {
    localStorage.setItem('cookie-consent', 'accepted');
    setIsVisible(false);

    // Track cookie acceptance
    posthog.capture('cookie_consent_accepted', {
      page: window.location.pathname,
    });
  };

  if (!isVisible) return null;

  return (
    <div className="fixed bottom-0 left-0 right-0 z-50 bg-fd-background border-t border-fd-border shadow-lg">
      <div className="container mx-auto px-4 py-2 flex flex-col sm:flex-row items-center justify-between gap-3">
        <p className="text-xs text-fd-muted-foreground">
          This site uses cookies for website functionality, analytics, and personalized content.
        </p>
        <button
          onClick={handleAccept}
          className="px-4 py-1 text-xs bg-fd-primary text-fd-primary-foreground rounded hover:opacity-90 transition-opacity whitespace-nowrap"
        >
          Okay
        </button>
      </div>
    </div>
  );
}
