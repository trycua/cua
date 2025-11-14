'use client';

import { useEffect } from 'react';

export function TocFix() {
  useEffect(() => {
    // Fix TOC overflow on mount and when content changes
    const fixTocOverflow = () => {
      const tocContainer = document.querySelector('#nd-toc > div');
      if (tocContainer) {
        (tocContainer as HTMLElement).style.overflowY = 'auto';
        (tocContainer as HTMLElement).style.overflowX = 'hidden';
      }
    };

    fixTocOverflow();

    // Also fix after a small delay to ensure DOM is ready
    const timer = setTimeout(fixTocOverflow, 100);

    return () => clearTimeout(timer);
  }, []);

  return null;
}
