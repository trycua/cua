// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from '../core';
import type { ConnectedCloud } from '../components/desktop/NewSpaceWizard';
import type { CloudConnectInput } from '../model/cloudConnect';
import { hasTauri } from './bridge';

/** What a cloud can run for one image family (`cloud_status` `kinds`). */
export interface CloudKindWire {
  image: string;
  kind: string;
  supported: boolean;
  reason?: string;
  machine_type?: string;
  usd_per_hour?: number;
}

/** One provider as `cloud_status` lists it (the SDK's wire record). */
export interface CloudProviderWire {
  name: string;
  title: string;
  tier: string;
  connected: boolean;
  default?: boolean;
  credentials?: { found: boolean; source?: string };
  account?: string;
  profile?: string;
  region?: string;
  zone?: string;
  project?: string;
  environment?: string;
  label?: string;
  ttl_hours?: number;
  kinds?: CloudKindWire[];
}

/** `cloud_status`. */
export interface CloudStatusWire {
  default_on?: string;
  providers: CloudProviderWire[];
}

/** One check of `cloud_test` (and of `cloud_connect`). */
export interface CloudCheckWire {
  name: string;
  ok: boolean;
  detail?: string;
}

/** `cloud_test`. */
export interface CloudTestWire {
  provider: string;
  ok: boolean;
  account?: string;
  checks: CloudCheckWire[];
}

/** Where Spaces go in the account (only names; credentials stay in the CLI). */
export interface CloudTargetWire {
  provider: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
}

/**
 * The user's own clouds (AWS, Google Cloud, Modal) through the app's
 * daemon: status, a test that creates nothing, and connect.
 */
export interface CloudBridge {
  status(): Promise<CloudStatusWire>;
  test(target: CloudTargetWire): Promise<CloudTestWire>;
  connect(target: CloudTargetWire, makeDefault: boolean): Promise<CloudProviderWire & { checks?: CloudCheckWire[] }>;
}

export function createTauriCloudBridge(): CloudBridge {
  const core = import('@tauri-apps/api/core');
  const tool = async <T>(name: string, args: Record<string, unknown>) =>
    (await core).invoke<T>('cloud_tool', { tool: name, args });
  return {
    status: () => tool('cloud_status', {}),
    test: (target) => tool('cloud_test', { ...target }),
    connect: (target, makeDefault) => tool('cloud_connect', { ...target, make_default: makeDefault }),
  };
}

export function createCloudBridge(): CloudBridge {
  if (hasTauri()) return createTauriCloudBridge();
  const unavailable = () => Promise.reject(new Error('Your cloud needs the Cua Spaces app'));
  return { status: async () => ({ providers: [] }), test: unavailable, connect: unavailable };
}

/** An in-memory account for tests: AWS found, nothing connected until Connect. */
export function createFakeCloudBridge(
  options: { failTest?: string } = {}
): CloudBridge & { calls: string[] } {
  const calls: string[] = [];
  let connected = false;
  const aws = (): CloudProviderWire => ({
    name: 'aws',
    title: 'AWS',
    tier: 'vm',
    connected,
    default: connected,
    credentials: { found: true, source: '~/.aws profile default' },
    profile: 'default',
    region: 'us-west-2',
    label: 'AWS · us-west-2',
    ttl_hours: 8,
    kinds: [
      { image: 'linux', kind: 'container', supported: true, machine_type: 't4g.medium', usd_per_hour: 0.0368 },
      { image: 'windows', kind: 'vm', supported: false, reason: 'Windows on AWS is not offered yet.' },
    ],
  });
  return {
    calls,
    status: async () => {
      calls.push('status');
      return { default_on: connected ? 'aws' : 'local', providers: [aws()] };
    },
    test: async (target) => {
      calls.push(`test:${target.provider}`);
      if (options.failTest) throw new Error(options.failTest);
      return { provider: target.provider, ok: true, account: '1', checks: [{ name: 'credentials', ok: true, detail: 'account 1' }] };
    },
    connect: async (target, makeDefault) => {
      calls.push(`connect:${target.provider}:${makeDefault}`);
      connected = true;
      return { ...aws(), checks: [{ name: 'credentials', ok: true, detail: 'account 1' }] };
    },
  };
}

/** The app core's `ConnectedCloud` rows of a `cloud_status`. */
export function connectedClouds(status: CloudStatusWire | null | undefined): ConnectedCloud[] {
  return core('cloudConnect.cloudsFromStatus', { status: status ?? {} });
}

/** The app core's `CloudConnectInput` of a `cloud_status`. */
export function connectInput(status: CloudStatusWire | null | undefined): CloudConnectInput {
  return core('cloudConnect.inputFromStatus', { status: status ?? {} });
}

/** Whether `on` (a `default.on` value) names one of the user's clouds. */
export function isCloudWord(on: string | null | undefined): boolean {
  return on ? core('cloudConnect.isCloudWord', { on }) : false;
}
