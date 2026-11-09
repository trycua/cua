import Darwin
import Foundation
import Testing

@testable import lume

private struct OwnerProbe: RunLockProbe {
    let pids: [pid_t]?
    func lockHolderPIDs(ofFileAt path: String) -> [pid_t]? { pids }
}

struct RunOwnerTests {
    @Test func identityAndPersistence() throws {
        let owner = try #require(RunOwner.current())
        #expect(owner.matches)
        #expect(RunOwner.current(0) == nil)
        #expect(RunOwner.current(-1) == nil)
        #expect(!RunOwner(pid: owner.pid, seconds: owner.seconds,
            microseconds: owner.microseconds + 1).matches)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        try owner.save(url)
        #expect(RunOwner.load(url) == owner)
        let successor = RunOwner(pid: owner.pid, seconds: owner.seconds + 1,
            microseconds: owner.microseconds)
        try successor.save(url)
        owner.remove(url)
        #expect(RunOwner.load(url) == successor)
        successor.remove(url)
        #expect(RunOwner.load(url) == nil)
        try Data("{}".utf8).write(to: url)
        #expect(RunOwner.load(url) == nil)
        try RunOwner(pid: -1, seconds: 1, microseconds: 0).save(url)
        #expect(RunOwner.load(url) == nil)
    }

    @Test func childOwnerLosesAuthorityAfterUnlock() throws {
        let config = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let record = config.appendingPathExtension("owner")
        try Data().write(to: config)
        defer {
            try? FileManager.default.removeItem(at: config)
            try? FileManager.default.removeItem(at: record)
        }
        let ready = Pipe()
        let release = Pipe()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/perl")
        process.arguments = ["-e", """
            $| = 1;
            open(my $file, '<', $ARGV[0]) or die $!;
            flock($file, 6) or die $!;
            print '1';
            sysread(STDIN, my $byte, 1);
            flock($file, 8) or die $!;
            print '1';
            sleep 60;
            """, config.path]
        process.standardOutput = ready
        process.standardInput = release
        try process.run()
        let child = process.processIdentifier
        defer {
            kill(child, SIGKILL)
            process.waitUntilExit()
        }
        _ = ready.fileHandleForReading.readData(ofLength: 1)
        let owner = try #require(RunOwner.current(child))
        try owner.save(record)
        let probe = OwnerProbe(pids: [getpid(), child])
        #expect(owner.verified(record: record, config: config, probe: probe))
        #expect(!owner.verified(record: record, config: config, probe: OwnerProbe(pids: nil)))
        let recycled = RunOwner(pid: child, seconds: owner.seconds + 1,
            microseconds: owner.microseconds)
        try recycled.save(record)
        #expect(!recycled.verified(record: record, config: config, probe: probe))
        try owner.save(record)
        release.fileHandleForWriting.write(Data([1]))
        _ = ready.fileHandleForReading.readData(ofLength: 1)
        #expect(owner.matches)
        #expect(!owner.verified(record: record, config: config, probe: probe))
        owner.remove(record)
        #expect(!owner.verified(record: record, config: config, probe: probe))
    }

    @Test func readersDoNotProveLockOwnership() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data().write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let reader = try FileHandle(forReadingFrom: url)
        defer { try? reader.close() }
        #expect(RunOwner.lockHeld(url) == false)
        #expect(flock(reader.fileDescriptor, LOCK_EX | LOCK_NB) == 0)
        #expect(RunOwner.lockHeld(url) == true)
        flock(reader.fileDescriptor, LOCK_UN)
        #expect(RunOwner.lockHeld(url) == false)
        let owner = try #require(RunOwner.current())
        let record = url.appendingPathExtension("owner")
        defer { try? FileManager.default.removeItem(at: record) }
        try owner.save(record)
        #expect(!owner.verified(record: record, config: url,
            probe: OwnerProbe(pids: [getpid(), getpid() + 1])))
        #expect(!owner.verified(record: record, config: url, probe: OwnerProbe(pids: nil)))
    }
}
