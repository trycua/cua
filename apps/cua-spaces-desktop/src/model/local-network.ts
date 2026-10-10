// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// macOS: asks for Local Network access for this app (the SwiftUI app's
// LocalNetworkPermission.swift), fire and forget.
//
// A Mac that runs macOS Spaces boots Lume VMs on vmnet (192.168.64.x), and
// its `cua daemon` (inside this app) must reach the guest. macOS blocks that
// ("No route to host") until Local Network access is allowed, and asks only
// the first time the app reaches the local network, on this Mac's own
// screen. Left alone, the prompt appears when the first Space is created,
// often from another Mac while nobody is at this one, and the create fails.
// So the app asks while the user is here: when the first run ends, when
// setup for access provides Spaces, and at launch on a Mac that already
// does. Windows and Linux have no such permission (their firewalls ask about
// listening, not about reaching the local network), so nothing is sent there.
import * as dgram from "node:dgram";
import * as os from "node:os";

export const VMNET_GATEWAY = "192.168.64.1";

const toInt = (ip: string) => ip.split(".").reduce((n, p) => ((n << 8) >>> 0) + Number.parseInt(p, 10), 0) >>> 0;
const toIp = (n: number) => [24, 16, 8, 0].map((s) => String((n >>> s) & 0xff)).join(".");

/** The subnet's first host (`192.168.1.23/24` → `192.168.1.1`); null for link-local or a subnet with no room for another host. */
export function subnetHost(ip: string, mask: string): string | null {
  const a = toInt(ip);
  const m = toInt(mask);
  if (a >>> 16 === 0xa9fe || m === 0 || (~m >>> 0) < 2) return null;
  let target = ((a & m) >>> 0) + 1;
  if (target === a) target += 1;
  return toIp(target >>> 0);
}

/** Where to send: the first active non-loopback IPv4 interface's first host (usually the router), then vmnet's gateway. */
export function targets(interfaces: NodeJS.Dict<os.NetworkInterfaceInfo[]> = os.networkInterfaces()): string[] {
  const out: string[] = [];
  for (const list of Object.values(interfaces)) {
    const v4 = (list ?? []).find((i) => i.family === "IPv4" && !i.internal);
    const host = v4 ? subnetHost(v4.address, v4.netmask) : null;
    if (host) {
      out.push(host);
      break;
    }
  }
  if (!out.includes(VMNET_GATEWAY)) out.push(VMNET_GATEWAY);
  return out;
}

/** One datagram to port 9 (discard) of each target: any datagram to a local address is what makes macOS ask; nothing has to answer. */
export class LocalNetworkPermission {
  private asked = false;
  constructor(
    private readonly platform: NodeJS.Platform = process.platform,
    private readonly send: (host: string) => void = probe,
  ) {}

  request(): void {
    if (this.asked || this.platform !== "darwin") return;
    this.asked = true;
    for (const host of targets()) this.send(host);
  }
}

function probe(host: string): void {
  const socket = dgram.createSocket("udp4");
  const close = () => {
    try {
      socket.close();
    } catch {
      // Closed already.
    }
  };
  socket.on("error", close);
  socket.send(Buffer.from([0]), 9, host, () => {});
  // Each probe lives a few seconds whatever happens; nothing waits for it.
  setTimeout(close, 3000).unref();
}
