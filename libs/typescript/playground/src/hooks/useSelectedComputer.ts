import { useMemo } from 'react';
import type { Computer } from '../types';
import { getComputerId } from '../types';

/**
 * Resolves which computer is currently selected.
 *
 * Priority:
 *   1. The computer whose ID matches `currentComputerId` (playground-level selection).
 *   2. Falls back to `chatComputer` (per-chat selection).
 *
 * This keeps the computer dropdown in sync with the VNC viewer while
 * respecting the per-chat default when no global override exists.
 */
export function useSelectedComputer(
  currentComputerId: string | null,
  computers: Computer[],
  chatComputer: Computer | undefined
): Computer | undefined {
  return useMemo(() => {
    if (currentComputerId) {
      const match = computers.find((c) => getComputerId(c) === currentComputerId);
      if (match) {
        return match;
      }
    }
    return chatComputer;
  }, [currentComputerId, computers, chatComputer]);
}
