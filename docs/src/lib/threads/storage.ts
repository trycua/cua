// src/lib/threads/storage.ts
'use client';

import { Thread, THREADS_STORAGE_KEY } from './types';

export function loadThreadsFromStorage(): Thread[] {
  if (typeof window === 'undefined') return [];

  try {
    const stored = localStorage.getItem(THREADS_STORAGE_KEY);
    if (!stored) return [];

    const parsed = JSON.parse(stored);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function saveThreadsToStorage(threads: Thread[]): void {
  if (typeof window === 'undefined') return;

  try {
    localStorage.setItem(THREADS_STORAGE_KEY, JSON.stringify(threads));
  } catch (error) {
    console.error('Failed to save threads to localStorage:', error);
  }
}

export function generateThreadId(): string {
  return crypto.randomUUID();
}
