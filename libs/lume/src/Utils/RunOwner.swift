import Darwin
import Foundation

struct RunOwner: Codable, Equatable {
    let pid: pid_t
    let seconds: UInt64
    let microseconds: UInt64

    static func current(_ pid: pid_t = getpid()) -> RunOwner? {
        guard pid > 0 else { return nil }
        var info = proc_bsdinfo()
        let size = MemoryLayout<proc_bsdinfo>.stride
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(size)) == Int32(size)
        else { return nil }
        return RunOwner(pid: pid, seconds: info.pbi_start_tvsec, microseconds: info.pbi_start_tvusec)
    }

    var matches: Bool { pid > 0 && Self.current(pid) == self }

    static func load(_ url: URL) -> RunOwner? {
        guard let data = try? Data(contentsOf: url),
            let owner = try? JSONDecoder().decode(Self.self, from: data), owner.pid > 0,
            owner.seconds > 0, owner.microseconds < 1_000_000 else { return nil }
        return owner
    }

    func save(_ url: URL) throws {
        try JSONEncoder().encode(self).write(to: url, options: .atomic)
    }

    func remove(_ url: URL) {
        if Self.load(url) == self { try? FileManager.default.removeItem(at: url) }
    }

    static func lockHeld(_ url: URL) -> Bool? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        if flock(handle.fileDescriptor, LOCK_EX | LOCK_NB) == 0 {
            flock(handle.fileDescriptor, LOCK_UN)
            return false
        }
        return errno == EWOULDBLOCK ? true : nil
    }

    func verified(record: URL, config: URL, probe: any RunLockProbe) -> Bool {
        guard pid != getpid(), Self.load(record) == self, matches,
            probe.recordedOwnerHasFile(pid: pid, path: config.path) == true,
            Self.lockHeld(config) == true else { return false }
        return matches && Self.load(record) == self
    }
}
