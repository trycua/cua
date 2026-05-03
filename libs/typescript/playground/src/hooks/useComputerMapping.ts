import { useMemo } from 'react';
import type { ComputerInfo } from '../adapters/types';
import type { Computer } from '../types';

/**
 * Maps ComputerInfo[] (adapter shape) to Computer[] (UI shape).
 *
 * The adapter exposes `agentUrl` while the UI `Computer` union expects `url`.
 * This hook performs the conversion and memoises the result so the array
 * reference stays stable across renders when the input hasn't changed.
 */
export function useComputerMapping(computerInfos: ComputerInfo[]): Computer[] {
  return useMemo(
    () =>
      computerInfos.map((c) => {
        const { agentUrl, ...rest } = c;
        return { ...rest, url: agentUrl } as Computer;
      }),
    [computerInfos]
  );
}
