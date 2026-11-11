// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// This Mac's name as a person reads it, for the account's list of
/// computers and devices: the Sharing "Computer Name" ("Ada's MacBook
/// Pro"), unless it is really a network host name. A hosted Mac's Computer
/// Name is often its full host name ("j17-2.example-host.com"), which the
/// lists cut to "j17-2.example-…"; it keeps only its first part ("j17-2").
/// The same rule as the cua service's (`cua_host` `person_name`), so the CLI
/// and the app name a computer alike. The person can rename it afterwards.
public enum ComputerName {
    /// This Mac's name, or nil when macOS has none.
    public static var current: String? {
        Foundation.Host.current().localizedName.map(personName).flatMap { $0.isEmpty ? nil : $0 }
    }

    /// `raw` as a person reads it: kept when it has spaces or no dots; a
    /// network host name becomes its first part; an IP address stays whole.
    public static func personName(_ raw: String) -> String {
        var name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        for suffix in [".localdomain", ".local"] where name.lowercased().hasSuffix(suffix) && name.count > suffix.count {
            name = String(name.dropLast(suffix.count))
            break
        }
        if name.contains(where: \.isWhitespace) || !name.contains(".") || isIPAddress(name) { return name }
        let first = name.split(separator: ".", omittingEmptySubsequences: false).first.map(String.init) ?? ""
        return first.isEmpty ? name : first
    }

    private static func isIPAddress(_ s: String) -> Bool {
        var v4 = in_addr()
        var v6 = in6_addr()
        return inet_pton(AF_INET, s, &v4) == 1 || inet_pton(AF_INET6, s, &v6) == 1
    }
}
