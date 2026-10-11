// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Connect a cloud": the providers with their found mark, plus "No cloud",
 * the region, project or environment, what the selected provider touches,
 * Test (creates nothing) and Connect. Every word and decision is the app
 * core's (`cloudConnect.*`, the same the SwiftUI app binds); this module
 * only types it.
 */

import { core } from '../core';

export interface CloudProviderInput {
  name: string;
  title: string;
  connected?: boolean;
  found?: boolean;
  source?: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
  label?: string;
}

export interface CloudConnectInput {
  providers: CloudProviderInput[];
}

export interface CloudCheckInput {
  name: string;
  ok: boolean;
  detail?: string;
}

export type CloudConnectState = Record<string, unknown>;

export type CloudConnectAction =
  | { type: 'select'; name: string }
  | { type: 'set-value'; text: string }
  | { type: 'set-profile'; text: string }
  | { type: 'set-make-default'; on: boolean }
  | { type: 'test' }
  | { type: 'tested'; ok: boolean; account: string; checks: CloudCheckInput[] }
  | { type: 'connect' }
  | { type: 'connected'; label: string }
  | { type: 'failed'; error: string };

export interface CloudTargetArgs {
  provider: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
}

export type CloudConnectRequest =
  | { kind: 'test'; target: CloudTargetArgs }
  | { kind: 'connect'; target: CloudTargetArgs; make_default: boolean };

export interface CloudField {
  id: string;
  label: string;
  placeholder: string;
  value: string;
}

export interface CloudConnectView {
  title: string;
  rows: { id: string; title: string; detail: string; found: boolean; selected: boolean }[];
  field: CloudField | null;
  profileField: CloudField | null;
  checks: { ok: boolean; text: string }[];
  result: string | null;
  touches: string[];
  makeDefaultLabel: string;
  makeDefault: boolean;
  testLabel: string;
  canTest: boolean;
  testHelp: string;
  connectLabel: string;
  canConnect: boolean;
  cancelLabel: string;
  error: string | null;
  done: boolean;
  request: CloudConnectRequest | null;
}

export function cloudConnectInitial(): CloudConnectState {
  return core('cloudConnect.initial', {});
}

export function reduceCloudConnect(
  input: CloudConnectInput,
  state: CloudConnectState,
  action: CloudConnectAction
): CloudConnectState {
  return core('cloudConnect.reduce', { input, state, action });
}

export function cloudConnectView(input: CloudConnectInput, state: CloudConnectState): CloudConnectView {
  return core('cloudConnect.view', { input, state });
}
