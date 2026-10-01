import { describe, it, expect } from 'vitest';
import AgentClient from '../src/index.js';

describe('AgentClient', () => {
  it('should create client with HTTP URL', () => {
    const client = new AgentClient('https://localhost:8000');
    expect(client).toBeDefined();
    expect(client.responses).toBeDefined();
    expect(typeof client.responses.create).toBe('function');
  });

  it('should create client with peer URL', () => {
    const client = new AgentClient('peer://test-peer-id');
    expect(client).toBeDefined();
    expect(client.responses).toBeDefined();
    expect(typeof client.responses.create).toBe('function');
  });

  it('should throw error for invalid URL', () => {
    expect(() => {
      new AgentClient('invalid://url');
    }).toThrow('Invalid URL format');
  });

  it('should have health method', async () => {
    const client = new AgentClient('https://localhost:8000');
    expect(typeof client.health).toBe('function');
  });

  it('should have disconnect method', async () => {
    const client = new AgentClient('https://localhost:8000');
    expect(typeof client.disconnect).toBe('function');
  });
});
