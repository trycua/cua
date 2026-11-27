import Darwin
import Foundation
import Testing

@testable import lume

private func socketPair() -> (host: Int32, guest: Int32) {
    var fds: [Int32] = [0, 0]
    #expect(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    return (fds[0], fds[1])
}

private func readAll(_ fd: Int32) -> String {
    var data = Data()
    var buf = [UInt8](repeating: 0, count: 256)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { break }
        data.append(contentsOf: buf[0..<n])
    }
    return String(decoding: data, as: UTF8.self)
}

@Suite("GuestTimeService")
struct GuestTimeServiceTests {
    @Test("A reply is one versioned line with seconds and nanoseconds")
    func replyFormat() {
        let time = timespec(tv_sec: 1_700_000_000, tv_nsec: 5)
        #expect(GuestTimeService.reply(for: time) == "time-v1 1700000000 5\n")
    }

    @Test("Serving writes the current time and nothing else")
    func servesCurrentTime() {
        let (host, guest) = socketPair()
        defer { close(guest) }
        let before = GuestTimeService.currentTime().tv_sec
        #expect(GuestTimeService.serve(fd: host))
        close(host)
        let after = GuestTimeService.currentTime().tv_sec

        let line = readAll(guest)
        #expect(line.hasSuffix("\n"))
        let fields = line.dropLast().split(separator: " ")
        #expect(fields.count == 3)
        #expect(fields.first == "time-v1")
        let seconds = Int(fields[1]) ?? -1
        let nanoseconds = Int(fields[2]) ?? -1
        #expect(seconds >= before && seconds <= after)
        #expect((0..<1_000_000_000).contains(nanoseconds))
    }

    @Test("Serving uses the clock it is given")
    func usesGivenClock() {
        let (host, guest) = socketPair()
        defer { close(guest) }
        #expect(GuestTimeService.serve(fd: host, now: { timespec(tv_sec: 42, tv_nsec: 999_999_999) }))
        close(host)
        #expect(readAll(guest) == "time-v1 42 999999999\n")
    }

    @Test("A guest that already closed its end does not raise SIGPIPE")
    func closedPeer() {
        let (host, guest) = socketPair()
        close(guest)
        // A plain write here would raise SIGPIPE and kill the test process.
        #expect(!GuestTimeService.serve(fd: host))
        close(host)
    }

    @Test("A guest that never reads cannot block the host")
    func fullSendBuffer() {
        let (host, guest) = socketPair()
        defer {
            close(host)
            close(guest)
        }
        // Fill the host's send buffer so the next send would block.
        _ = fcntl(host, F_SETFL, fcntl(host, F_GETFL) | O_NONBLOCK)
        var chunk = [UInt8](repeating: 0, count: 4096)
        while send(host, &chunk, chunk.count, MSG_NOSIGNAL) > 0 {}
        #expect(!GuestTimeService.serve(fd: host))
    }

    @Test("Ports 0 and 0xFFFFFFFF are rejected")
    func portRange() {
        #expect(!GuestTimeService.isValidPort(0))
        #expect(!GuestTimeService.isValidPort(UInt32.max))
        #expect(GuestTimeService.isValidPort(1))
        #expect(GuestTimeService.isValidPort(5000))
        #expect(GuestTimeService.isValidPort(UInt32.max - 1))
    }

    @Test("The time service adds a socket device; a resume follows the saved state")
    func socketDeviceSelection() {
        #expect(VM.usesSocketDevice(forwards: [], guestTimePort: 5000, hasSavedState: false, savedStateHasVsock: false))
        #expect(VM.usesSocketDevice(forwards: [], guestTimePort: 5000, hasSavedState: true, savedStateHasVsock: true))
        #expect(!VM.usesSocketDevice(forwards: [], guestTimePort: 5000, hasSavedState: true, savedStateHasVsock: false))
        #expect(!VM.usesSocketDevice(forwards: [], guestTimePort: nil, hasSavedState: false, savedStateHasVsock: false))
        // A state saved with the device keeps it on resume, so the restore
        // configuration matches, even when this run asks for neither.
        #expect(VM.usesSocketDevice(forwards: [], guestTimePort: nil, hasSavedState: true, savedStateHasVsock: true))
    }

    @Test("The run command validates --time-vsock")
    func runCommandValidation() throws {
        let run = try Run.parse(["my-vm", "--time-vsock", "5000"])
        #expect(run.timeVsock == 5000)
        #expect(throws: (any Error).self) { try Run.parse(["my-vm", "--time-vsock", "0"]).validate() }
        #expect(throws: (any Error).self) { try Run.parse(["my-vm", "--time-vsock", "-1"]) }
        #expect(try Run.parse(["my-vm"]).timeVsock == nil)
    }

    @Test("The HTTP run request validates timeVsockPort")
    func runRequestValidation() throws {
        func request(_ json: String) throws -> RunVMRequest {
            try JSONDecoder().decode(RunVMRequest.self, from: Data(json.utf8))
        }
        #expect(try request("{}").validatedTimeVsockPort() == nil)
        #expect(try request("{\"timeVsockPort\":5000}").validatedTimeVsockPort() == 5000)
        #expect(throws: (any Error).self) { try request("{\"timeVsockPort\":0}").validatedTimeVsockPort() }
    }
}
