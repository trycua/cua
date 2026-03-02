import { useEffect, useRef } from 'react';
import { usePlayground } from '../../hooks/usePlayground';
import type { Chat } from '../../types';

interface DeferredChatsLoaderProps {
  chatsPromise: Promise<Chat[]>;
}

/**
 * Component to handle deferred chat loading
 * Loads chats asynchronously and dispatches to playground context
 */
export function DeferredChatsLoader({ chatsPromise }: DeferredChatsLoaderProps) {
  const { dispatch } = usePlayground();
  const hasLoadedRef = useRef(false);

  useEffect(() => {
    // Only load once
    if (hasLoadedRef.current) return;
    hasLoadedRef.current = true;

    chatsPromise
      .then((chats) => {
        dispatch({ type: 'SET_CHATS', payload: chats });
      })
      .catch((error) => {
        console.error('Failed to load chats:', error);
        dispatch({ type: 'SET_ERROR', payload: 'Failed to load chats' });
      });
  }, [chatsPromise, dispatch]);

  return null;
}
