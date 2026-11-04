import { describe, expect, it, vi } from 'vitest';

const recorded: { event: string; props: Record<string, unknown> }[] = [];

vi.mock('@trycua/core', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@trycua/core')>();
  class FakeTelemetry {
    recordEvent(event: string, props: Record<string, unknown> = {}) {
      recorded.push({ event, props });
    }
    async shutdown() {}
  }
  return { ...actual, Telemetry: FakeTelemetry };
});

describe('AgentClient telemetry', () => {
  it('agent_request_error sends only the classified error type', async () => {
    const { AgentClient } = await import('../src/client.js');
    const secret = 'failed for /Users/alice/private prompt: buy alice@example.com a gift';
    const fetchMock = vi.fn(async () => {
      throw new Error(secret);
    });
    vi.stubGlobal('fetch', fetchMock);
    try {
      const client = new AgentClient('https://localhost:8000', { retries: 0 });
      await expect(
        client.responses.create({ model: 'anthropic/claude', input: 'hi' } as never)
      ).rejects.toThrow();
    } finally {
      vi.unstubAllGlobals();
    }
    const err = recorded.find((r) => r.event === 'agent_request_error');
    expect(err).toBeDefined();
    expect(err!.props).not.toHaveProperty('error_message');
    expect(typeof err!.props.error_type).toBe('string');
    expect(JSON.stringify(recorded)).not.toContain('alice');
  });
});

describe('telemetryModel', () => {
  it('keeps provider/model and coarsens paths, URLs and custom deployments', async () => {
    const { telemetryModel } = await import('../src/client.js');
    expect(telemetryModel('anthropic/claude-sonnet-4-5')).toBe('anthropic/claude-sonnet-4-5');
    expect(telemetryModel('gpt-4o')).toBe('gpt-4o');
    expect(telemetryModel('huggingface-local//Users/alice/models/x')).toBe(
      'huggingface-local/custom'
    );
    expect(telemetryModel('openai/https://llm.corp.example/v1')).toBe('custom');
    expect(telemetryModel('https://llm.corp.example/v1')).toBe('custom');
    expect(telemetryModel('~/models/mine')).toBe('custom');
    expect(telemetryModel(undefined)).toBe('unknown');
  });
});
