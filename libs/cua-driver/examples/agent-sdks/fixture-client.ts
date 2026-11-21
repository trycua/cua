/** Bounded HTTP verification for the native desktop example. */

import { setTimeout as delay } from 'node:timers/promises';

async function withDeadline<T>(
  timeoutMs: number,
  label: string,
  operation: (signal: AbortSignal) => Promise<T>
): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await operation(controller.signal);
  } catch (error) {
    if (controller.signal.aborted) {
      throw new Error(`${label} timed out`, { cause: error });
    }
    throw error;
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
}

export async function resetFixture(fixture: string, timeoutMs: number): Promise<void> {
  await withDeadline(timeoutMs, 'fixture reset', async (signal) => {
    const response = await fetch(`${fixture}/reset`, { method: 'POST', signal });
    if (!response.ok) throw new Error(`fixture reset failed: ${response.status}`);
    await response.arrayBuffer();
  });
}

export async function waitForSubmission(
  fixture: string,
  token: string,
  timeoutMs: number
): Promise<void> {
  // One deadline covers headers, body reads, and every polling interval.
  await withDeadline(timeoutMs, 'fixture submission verification', async (signal) => {
    while (true) {
      const response = await fetch(`${fixture}/state`, { signal });
      if (!response.ok) throw new Error(`fixture state failed: ${response.status}`);
      const state = (await response.json()) as { submitted: string | null };
      if (state.submitted === token) return;
      await delay(100, undefined, { signal });
    }
  });
}
