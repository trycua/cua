// src/components/chat/PendingMessageContext.tsx
'use client';

import { createContext, useContext, useState, useCallback, ReactNode } from 'react';

interface PendingMessage {
  id: string;
  content: string;
  timestamp: number;
}

interface PendingMessageContextType {
  pendingMessage: PendingMessage | null;
  setPendingMessage: (message: PendingMessage | null) => void;
  clearPendingMessage: () => void;
}

const PendingMessageContext = createContext<PendingMessageContextType | null>(null);

export function PendingMessageProvider({ children }: { children: ReactNode }) {
  const [pendingMessage, setPendingMessage] = useState<PendingMessage | null>(null);

  const clearPendingMessage = useCallback(() => {
    setPendingMessage(null);
  }, []);

  return (
    <PendingMessageContext.Provider
      value={{
        pendingMessage,
        setPendingMessage,
        clearPendingMessage,
      }}
    >
      {children}
    </PendingMessageContext.Provider>
  );
}

export function usePendingMessage() {
  const context = useContext(PendingMessageContext);
  if (!context) {
    throw new Error('usePendingMessage must be used within PendingMessageProvider');
  }
  return context;
}
