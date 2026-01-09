'use client';

// IMPORTANT: Apply fetch patch BEFORE any CopilotKit imports
// This patches window.fetch to cache info requests and prevent the infinite loop bug
import { applyCopilotKitFetchPatch } from '@/lib/copilotkit-fetch-patch';

// Apply patch immediately on module load (browser only)
if (typeof window !== 'undefined') {
  applyCopilotKitFetchPatch();
}

import { CopilotKit } from '@copilotkit/react-core';
import { CopilotPopup } from '@copilotkit/react-ui';
import '@copilotkit/react-ui/styles.css';
import { ReactNode, useEffect, useState, useRef, memo } from 'react';

interface CopilotKitProviderProps {
  children: ReactNode;
}

// Enable CopilotKit - the fetch patch handles request deduplication
const COPILOTKIT_ENABLED = true;

// Memoized CopilotKit wrapper to prevent re-renders
const CopilotKitWrapper = memo(function CopilotKitWrapper({
  children,
}: {
  children: ReactNode;
}) {
  return (
    <CopilotKit runtimeUrl="/docs/api/copilotkit" showDevConsole={false}>
      {children}
      <CopilotPopup
        labels={{
          title: 'Cua Docs Assistant',
          initial: `Ask me anything about Cua!

This is currently an **experimental** feature.

Please refer to the source documentation for the most accurate information.`,
        }}
      />
    </CopilotKit>
  );
});

export function CopilotKitProvider({ children }: CopilotKitProviderProps) {
  const [mounted, setMounted] = useState(false);
  const mountedOnceRef = useRef(false);

  useEffect(() => {
    // Ensure we only mount once, even with React Strict Mode
    if (mountedOnceRef.current) return;
    mountedOnceRef.current = true;

    const timer = setTimeout(() => setMounted(true), 200);
    return () => clearTimeout(timer);
  }, []);

  if (!COPILOTKIT_ENABLED || !mounted) {
    return <>{children}</>;
  }

  return <CopilotKitWrapper>{children}</CopilotKitWrapper>;
}
