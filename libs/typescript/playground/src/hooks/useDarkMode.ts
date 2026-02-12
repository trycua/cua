import { useEffect, useState } from 'react';

/**
 * Hook to detect dark mode from multiple sources:
 * 1. Parent app's .dark class on document element (if present)
 * 2. System preference via prefers-color-scheme (fallback)
 *
 * This allows the component to work standalone OR integrate with parent themes.
 */
export function useDarkMode(): boolean {
  const [isDarkMode, setIsDarkMode] = useState(() => {
    if (typeof window === 'undefined') return false;

    // First check if parent app has set a .dark class
    if (document.documentElement.classList.contains('dark')) {
      return true;
    }

    // Fallback to system preference
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
  });

  useEffect(() => {
    if (typeof window === 'undefined') return;

    // Watch for changes to .dark class on document element
    const observer = new MutationObserver((mutations) => {
      mutations.forEach((mutation) => {
        if (mutation.attributeName === 'class') {
          const hasDarkClass = document.documentElement.classList.contains('dark');
          setIsDarkMode(hasDarkClass || window.matchMedia('(prefers-color-scheme: dark)').matches);
        }
      });
    });

    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['class'],
    });

    // Also watch for system preference changes
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = (e: MediaQueryListEvent) => {
      // Only update based on system preference if no .dark class is set
      if (!document.documentElement.classList.contains('dark')) {
        setIsDarkMode(e.matches);
      }
    };

    mediaQuery.addEventListener('change', handler);

    return () => {
      observer.disconnect();
      mediaQuery.removeEventListener('change', handler);
    };
  }, []);

  return isDarkMode;
}
