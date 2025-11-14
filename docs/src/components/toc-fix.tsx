'use client';

import { useEffect } from 'react';

export function TocFix() {
  useEffect(() => {
    const fixTocOverflow = () => {
      const toc = document.querySelector('#nd-toc') as HTMLElement;
      const tocContainer = document.querySelector('#nd-toc > div') as HTMLElement;

      if (toc && tocContainer) {
        // Calculate the available height for the TOC
        const topOffset = toc.style.top || 'calc(var(--fd-banner-height) + var(--fd-nav-height))';
        const maxHeight = `calc(100vh - ${topOffset})`;

        // Set max-height on the parent container
        toc.style.setProperty('max-height', maxHeight, 'important');
        toc.style.setProperty('overflow', 'hidden', 'important');

        // Make the inner container scrollable
        tocContainer.style.setProperty('overflow-y', 'auto', 'important');
        tocContainer.style.setProperty('overflow-x', 'hidden', 'important');
        tocContainer.style.setProperty('height', '100%', 'important');
      }
    };

    // Apply fix immediately
    fixTocOverflow();

    // Apply fix after delays to catch any async rendering
    const timers = [
      setTimeout(fixTocOverflow, 50),
      setTimeout(fixTocOverflow, 200),
      setTimeout(fixTocOverflow, 500),
    ];

    // Watch for DOM changes and reapply fix
    const observer = new MutationObserver(() => {
      fixTocOverflow();
    });

    const toc = document.querySelector('#nd-toc');
    if (toc) {
      observer.observe(toc, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ['class', 'style'],
      });
    }

    return () => {
      timers.forEach(clearTimeout);
      observer.disconnect();
    };
  }, []);

  return null;
}
