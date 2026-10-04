import type { CyclopsClientLike, Sandbox } from '@trycua/fleet/node';
import type { FleetDriverClient, FleetDriverSandbox } from '../src/fleet.js';

declare const client: CyclopsClientLike;
declare const sandbox: Sandbox;

// Keep the adapter assignable from the published Fleet API, not just mocks.
const compatibleClient: FleetDriverClient = client;
const compatibleSandbox: FleetDriverSandbox = sandbox;
void compatibleClient;
void compatibleSandbox;
