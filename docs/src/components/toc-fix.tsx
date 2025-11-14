'use client';

import { useEffect } from 'react';

export function TocFix() {
  useEffect(() => {
    const fixTocOverflow = () => {
      const tocContainer = document.querySelector('#nd-toc > div');
      if (tocContainer) {
        (tocContainer as HTMLElement).style.setProperty('overflow-y', 'auto', 'important');
        (tocContainer as HTMLElement).style.setProperty('overflow-x', 'hidden', 'important');
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
