import type { IncomingMessage } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocketServer } from 'ws';
import { Computer, HostComputer } from '../../src';
import { OSType } from '../../src/types';

/**
 * These tests stand up a throwaway WebSocket server that mimics the Computer
 * Server's `/ws` endpoint, then assert that the host-mode Computer connects to
 * it over plain `ws://` with no authentication headers and can round-trip
 * commands. No VM, cloud, or API key is involved.
 */
describe('Host Computer Server', () => {
  let server: WebSocketServer;
  let port: number;
  let lastHeaders: IncomingMessage['headers'] | undefined;

  function platformOSType(): OSType {
    switch (process.platform) {
      case 'win32':
        return OSType.WINDOWS;
      case 'darwin':
        return OSType.MACOS;
      default:
        return OSType.LINUX;
    }
  }

  beforeEach(async () => {
    lastHeaders = undefined;
    server = new WebSocketServer({ port: 0 });
    await new Promise<void>((resolve) => server.once('listening', () => resolve()));
    port = (server.address() as AddressInfo).port;

    server.on('connection', (socket, req: IncomingMessage) => {
      lastHeaders = req.headers;
      socket.on('message', (data) => {
        const msg = JSON.parse(data.toString());
        if (msg.command === 'get_screen_size') {
          socket.send(JSON.stringify({ success: true, size: { width: 1920, height: 1080 } }));
        } else {
          socket.send(JSON.stringify({ success: true, echo: msg }));
        }
      });
    });
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  it('constructs in host mode without an apiKey', () => {
    const computer = new Computer({ useHostComputerServer: true, port });
    expect(computer).toBeInstanceOf(Computer);
  });

  it('defaults osType to the current platform in host mode', () => {
    const computer = new Computer({ useHostComputerServer: true, port });
    expect(computer.getOSType()).toBe(platformOSType());
  });

  it('reports the configured host as its ip', () => {
    const computer = new Computer({ useHostComputerServer: true, host: '127.0.0.1', port });
    expect(computer.ip).toBe('127.0.0.1');
  });

  it('connects to the local server without auth headers and round-trips a command', async () => {
    const computer = new Computer({ useHostComputerServer: true, port, osType: OSType.LINUX });
    await computer.run();
    try {
      const size = await computer.interface.getScreenSize();
      expect(size).toEqual({ width: 1920, height: 1080 });
      // Host mode must not send cloud authentication headers.
      expect(lastHeaders?.['x-api-key']).toBeUndefined();
      expect(lastHeaders?.['x-vm-name']).toBeUndefined();
    } finally {
      await computer.disconnect();
    }
  });

  it('HostComputer is a Computer wired for host mode', async () => {
    const computer = new HostComputer({ port, osType: OSType.LINUX });
    expect(computer).toBeInstanceOf(Computer);
    await computer.run();
    try {
      const size = await computer.interface.getScreenSize();
      expect(size).toEqual({ width: 1920, height: 1080 });
    } finally {
      await computer.disconnect();
    }
  });

  it('throws a helpful error when neither apiKey nor host mode is given', () => {
    // @ts-expect-error — intentionally invalid: cloud mode requires apiKey.
    expect(() => new Computer({ osType: OSType.LINUX, name: 'x' })).toThrow(/apiKey/);
  });
});
