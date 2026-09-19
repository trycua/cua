import { afterEach, describe, expect, it, vi } from 'vitest';
import { FleetComputer, OSType, ProviderType } from '../../src';
import { InterfaceFactory } from '../../src/interface/factory';

const fleetMocks = vi.hoisted(() => ({
  createFleetClient: vi.fn(),
}));

vi.mock('@trycua/fleet/node', () => ({
  createFleetClient: fleetMocks.createFleetClient,
  CyclopsTokenProviderConfigurationBuilder: class {
    private configuration: Record<string, unknown> = {};

    baseUrl(value: string) {
      this.configuration.baseUrl = value;
      return this;
    }

    poolPollIntervalMs(value: bigint) {
      this.configuration.poolPollIntervalMs = value;
      return this;
    }

    poolPollLimit(value: number) {
      this.configuration.poolPollLimit = value;
      return this;
    }

    claimPollIntervalMs(value: bigint) {
      this.configuration.claimPollIntervalMs = value;
      return this;
    }

    claimPollLimit(value: number) {
      this.configuration.claimPollLimit = value;
      return this;
    }

    build() {
      return this.configuration;
    }
  },
  CreateClaimRequestBuilder: class {
    private request: Record<string, unknown> = {};

    pool(value: unknown) {
      this.request.pool = value;
      return this;
    }

    build() {
      return this.request;
    }
  },
}));

afterEach(() => {
  vi.restoreAllMocks();
  fleetMocks.createFleetClient.mockReset();
});

describe('FleetComputer', () => {
  it('claims a sandbox, connects through the Fleet proxy, and releases it', async () => {
    const pool = { metadata: { namespace: 'keiki' } };
    const claim = { metadata: { name: 'claim-1' } };
    const sandbox = { namespace: 'keiki', name: 'sandbox-1', services: ['server'] };
    const client = {
      getPool: vi.fn().mockResolvedValue(pool),
      createClaim: vi.fn().mockResolvedValue(claim),
      waitClaim: vi.fn().mockResolvedValue(sandbox),
      deleteClaim: vi.fn().mockResolvedValue(undefined),
      uniffiDestroy: vi.fn(),
    };
    fleetMocks.createFleetClient.mockResolvedValue(client);

    const iface = {
      waitForReady: vi.fn().mockResolvedValue(undefined),
      disconnect: vi.fn(),
    };
    const interfaceSpy = vi
      .spyOn(InterfaceFactory, 'createInterfaceForOS')
      .mockReturnValue(iface as never);

    const computer = new FleetComputer({
      name: 'keiki-task',
      osType: OSType.LINUX,
      vmProvider: ProviderType.FLEET,
      accessToken: 'secret-token',
      poolName: 'keiki-pool',
      namespace: 'keiki',
      serviceName: 'server',
    });

    await computer.run();

    expect(computer.getVMProviderType()).toBe(ProviderType.FLEET);
    expect(client.getPool).toHaveBeenCalledWith('keiki-pool');
    expect(client.createClaim).toHaveBeenCalledWith({ pool });
    expect(interfaceSpy).toHaveBeenCalledWith(OSType.LINUX, 'sandbox-1', undefined, undefined, {
      wsUrl: 'wss://run.cua.ai/api/svc/keiki/sandbox-1-server/ws',
      headers: { Authorization: 'Bearer secret-token' },
    });
    expect(computer.ip).toBe('wss://run.cua.ai/api/svc/keiki/sandbox-1-server/ws');

    await computer.stop();

    expect(iface.disconnect).toHaveBeenCalledOnce();
    expect(client.deleteClaim).toHaveBeenCalledWith(claim);
    expect(client.uniffiDestroy).toHaveBeenCalledOnce();
  });

  it('releases a claim when interface initialization fails', async () => {
    const claim = { metadata: { name: 'claim-1' } };
    const client = {
      getPool: vi.fn().mockResolvedValue({ metadata: { namespace: 'keiki' } }),
      createClaim: vi.fn().mockResolvedValue(claim),
      waitClaim: vi
        .fn()
        .mockResolvedValue({ namespace: 'keiki', name: 'sandbox-1', services: ['server'] }),
      deleteClaim: vi.fn().mockResolvedValue(undefined),
      uniffiDestroy: vi.fn(),
    };
    fleetMocks.createFleetClient.mockResolvedValue(client);
    vi.spyOn(InterfaceFactory, 'createInterfaceForOS').mockReturnValue({
      waitForReady: vi.fn().mockRejectedValue(new Error('not ready')),
      disconnect: vi.fn(),
    } as never);

    const computer = new FleetComputer({
      name: 'keiki-task',
      osType: OSType.LINUX,
      accessToken: 'secret-token',
      poolName: 'keiki-pool',
    });

    await expect(computer.run()).rejects.toThrow('Failed to initialize Fleet computer');
    expect(client.deleteClaim).toHaveBeenCalledWith(claim);
    expect(client.uniffiDestroy).toHaveBeenCalledOnce();
  });
});
