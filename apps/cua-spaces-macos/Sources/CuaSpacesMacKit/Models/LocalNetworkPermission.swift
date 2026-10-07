// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation
import Network

/// Asks macOS for Local Network access for this app (fire and forget).
///
/// Why: reaching a guest from this Mac (a Lume VM on vmnet 192.168.64.x, or
/// another machine on the LAN) needs Local Network access. macOS blocks
/// that ("No route to host") until it is allowed, and it asks only the
/// first time the app reaches the local network, at most once per boot, on
/// this Mac's own screen. The app is not even listed under System Settings
/// > Privacy & Security > Local Network until it has been asked. Left
/// alone, the prompt appears when the first Space is created, often while
/// nobody is at this Mac, and the create fails with `waiting_for_services`.
/// So every Mac asks while someone is here: at launch, and when this Mac's
/// setup becomes configured (a host that provides Spaces, or a controller
/// that only accesses other machines).
public protocol LocalNetworkPermissionRequesting: AnyObject, Sendable {
    /// Triggers the prompt if macOS has not decided yet. Never blocks.
    func request()
}

/// The live request: a short-lived UDP datagram to an address on this Mac's
/// local network (the first active IPv4 interface's subnet, and vmnet's
/// 192.168.64.1, where the VMs live). Each connection is cancelled after a
/// few seconds whatever happens; nothing waits for it. Asks once per
/// process (macOS asks at most once per boot anyway).
public final class LiveLocalNetworkPermission: LocalNetworkPermissionRequesting, @unchecked Sendable {
    private let lock = NSLock()
    private var asked = false
    private let queue = DispatchQueue(label: "com.trycua.cua-spaces.local-network")
    /// How long each probe may live.
    let lifetime: TimeInterval

    public init(lifetime: TimeInterval = 3) {
        self.lifetime = lifetime
    }

    public func request() {
        lock.lock()
        let first = !asked
        asked = true
        lock.unlock()
        guard first else { return }
        queue.async { [self] in
            for host in Self.targets() { probe(host) }
        }
    }

    private func probe(_ host: String) {
        // Port 9 (discard): any datagram to a local address is what makes
        // macOS ask; nothing has to answer.
        let connection = NWConnection(host: NWEndpoint.Host(host), port: 9, using: .udp)
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready:
                connection.send(content: Data([0]), completion: .contentProcessed { _ in })
            case .failed, .waiting:
                connection.cancel()
            default:
                break
            }
        }
        connection.start(queue: queue)
        queue.asyncAfter(deadline: .now() + lifetime) { connection.cancel() }
    }

    /// The addresses to reach: the first active non-loopback IPv4
    /// interface's first host (usually the router), then vmnet's gateway.
    static func targets() -> [String] {
        var out: [String] = []
        if let lan = firstSubnetHost() { out.append(lan) }
        if !out.contains(vmnetGateway) { out.append(vmnetGateway) }
        return out
    }

    static let vmnetGateway = "192.168.64.1"

    static func firstSubnetHost() -> String? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        for cursor in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let ifa = cursor.pointee
            let flags = Int32(ifa.ifa_flags)
            guard flags & IFF_UP != 0, flags & IFF_RUNNING != 0, flags & IFF_LOOPBACK == 0,
                  let addr = ifa.ifa_addr, addr.pointee.sa_family == sa_family_t(AF_INET),
                  let mask = ifa.ifa_netmask else { continue }
            let ip = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            let m = mask.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            if let host = subnetHost(ip: ip, mask: m) { return host }
        }
        return nil
    }

    /// The subnet's first host (`192.168.1.23/24` → `192.168.1.1`); nil for
    /// link-local or a subnet with no room for another host.
    static func subnetHost(ip: UInt32, mask: UInt32) -> String? {
        guard ip >> 16 != 0xA9FE, mask != 0, ~mask >= 2 else { return nil }
        var target = (ip & mask) + 1
        if target == ip { target += 1 }
        return [24, 16, 8, 0].map { String((target >> UInt32($0)) & 0xFF) }.joined(separator: ".")
    }
}
