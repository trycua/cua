// cua-vz <vm-dir>: runs the macOS microVM described by <vm-dir>/vz.json
// until it stops (see Package.swift). The runtime talks to it through:
//
// - <sockets>/api.sock: one JSON line per connection: {"op":"state"},
//   {"op":"pause"}, {"op":"resume"}, {"op":"save","path":<file>} (the paused
//   VM's memory and devices, Virtualization.framework's saved state) and
//   {"op":"stop"}; answered with {"ok":true,...} or {"ok":false,"error":..}.
//   It exists once the VM runs (booted, or restored and resumed).
// - <sockets>/v.sock: Firecracker's vsock convention (`CONNECT <port>\n`,
//   answered `OK <port>\n`, then the guest's stream), so the runtime's guest
//   init client is the same for both VMMs.
// - 127.0.0.1:<host> for every forward: a connection there is carried over
//   vsock to the guest init, which connects to the guest's own port.
//
// The process exits when the VM stops (the guest powered off, `stop`, or
// SIGTERM), taking its network gateway with it.

import CuaVZCore
import Darwin
import Foundation
import Virtualization

setvbuf(stdout, nil, _IOLBF, 0)
setvbuf(stderr, nil, _IONBF, 0)
signal(SIGPIPE, SIG_IGN)

func log(_ msg: String) {
    FileHandle.standardError.write(Data("cua-vz: \(msg)\n".utf8))
}

func fail(_ msg: String, code: Int32 = 1) -> Never {
    log(msg)
    exit(code)
}

/// The vsock port the guest init listens on (cua_microvm_init::PORT).
let initPort: UInt32 = 1024

guard CommandLine.arguments.count == 2 else {
    fail("usage: cua-vz <vm-dir>", code: 2)
}
let dir = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
guard chdir(dir.path) == 0 else { fail("cannot enter \(dir.path)") }
let cfg: VZConfig
do {
    cfg = try VZConfig.load(from: dir.appendingPathComponent("vz.json"))
} catch {
    fail("vz.json: \(error)", code: 2)
}
if let p = cfg.problems().first { fail(p, code: 2) }

func url(_ path: String) -> URL {
    URL(fileURLWithPath: path, relativeTo: dir).standardizedFileURL
}

func sock(_ name: String) -> String {
    url(cfg.sockets).appendingPathComponent(name).path
}

final class Host: NSObject, VZVirtualMachineDelegate {
    let queue = DispatchQueue(label: "ai.cua.vz.vm")
    var vm: VZVirtualMachine!
    var gateway: Process?
    /// The other end of an unconnected network device, drained.
    var netPeer: Int32 = -1
    var listeners: [Int32] = []

    // ------------------------------------------------------------ config

    func machineIdentifier() throws -> VZGenericMachineIdentifier {
        let file = url(cfg.machineID)
        if let d = try? Data(contentsOf: file) {
            guard let id = VZGenericMachineIdentifier(dataRepresentation: d) else {
                throw Sockets.Failure("\(file.path) is not a machine identifier")
            }
            return id
        }
        let id = VZGenericMachineIdentifier()
        try id.dataRepresentation.write(to: file, options: .atomic)
        return id
    }

    func networkAttachment() throws -> VZNetworkDeviceAttachment {
        guard let net = cfg.network else {
            var fds: [Int32] = [0, 0]
            guard socketpair(AF_UNIX, SOCK_DGRAM, 0, &fds) == 0 else {
                throw Sockets.Failure("socketpair")
            }
            netPeer = fds[1]
            let peer = fds[1]
            let t = Thread {
                var buf = [UInt8](repeating: 0, count: 65536)
                while read(peer, &buf, buf.count) > 0 {}
            }
            t.name = "no network"
            t.start()
            return VZFileHandleNetworkDeviceAttachment(
                fileHandle: FileHandle(fileDescriptor: fds[0], closeOnDealloc: true))
        }
        let gv = sock("gv.sock")
        unlink(gv)
        let p = Process()
        p.executableURL = url(net.gateway)
        p.arguments = ["-config", url(net.config).path]
        p.currentDirectoryURL = dir
        let logURL = dir.appendingPathComponent("netgw.log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        let logFH = try FileHandle(forWritingTo: logURL)
        logFH.seekToEndOfFile()
        p.standardOutput = logFH
        p.standardError = logFH
        p.standardInput = FileHandle.nullDevice
        try p.run()
        gateway = p
        let deadline = Date().addingTimeInterval(10)
        while !FileManager.default.fileExists(atPath: gv) {
            if !p.isRunning { throw Sockets.Failure("the network gateway exited (see netgw.log)") }
            if Date() > deadline { throw Sockets.Failure("the network gateway did not listen within 10 s") }
            usleep(2_000)
        }
        let fd = try Sockets.datagram(local: sock("net.sock"), remote: gv)
        // vfkit's greeting: the gateway learns the peer's address from it.
        guard Sockets.writeAll(fd, Data("VFKT".utf8)) else {
            throw Sockets.Failure("network gateway greeting")
        }
        return VZFileHandleNetworkDeviceAttachment(
            fileHandle: FileHandle(fileDescriptor: fd, closeOnDealloc: true))
    }

    func makeConfiguration() throws -> VZVirtualMachineConfiguration {
        let c = VZVirtualMachineConfiguration()
        c.cpuCount = cfg.cpus
        c.memorySize = cfg.memoryMB << 20
        let platform = VZGenericPlatformConfiguration()
        platform.machineIdentifier = try machineIdentifier()
        c.platform = platform
        let boot = VZLinuxBootLoader(kernelURL: url(cfg.kernel))
        boot.initialRamdiskURL = url(cfg.initrd)
        boot.commandLine = cfg.cmdline
        c.bootLoader = boot
        c.storageDevices = try cfg.disks.map { d in
            // `.cached`: the default caching corrupts ext4 in Linux guests
            // (as Lume and UTM found).
            VZVirtioBlockDeviceConfiguration(
                attachment: try VZDiskImageStorageDeviceAttachment(
                    url: url(d.path), readOnly: d.readOnly, cachingMode: .cached,
                    synchronizationMode: .fsync))
        }
        let net = VZVirtioNetworkDeviceConfiguration()
        guard let mac = VZMACAddress(string: cfg.mac) else {
            throw Sockets.Failure("bad MAC address \(cfg.mac)")
        }
        net.macAddress = mac
        net.attachment = try networkAttachment()
        c.networkDevices = [net]
        c.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
        c.socketDevices = [VZVirtioSocketDeviceConfiguration()]
        c.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
        let console = VZVirtioConsoleDeviceSerialPortConfiguration()
        let consoleURL = url(cfg.console)
        if !FileManager.default.fileExists(atPath: consoleURL.path) {
            FileManager.default.createFile(atPath: consoleURL.path, contents: nil)
        }
        let out = try FileHandle(forWritingTo: consoleURL)
        out.seekToEndOfFile()
        console.attachment = VZFileHandleSerialPortAttachment(
            fileHandleForReading: nil, fileHandleForWriting: out)
        c.serialPorts = [console]
        try c.validate()
        try c.validateSaveRestoreSupport()
        return c
    }

    // ------------------------------------------------------------ VM calls

    /// Runs `body` on the VM's queue and waits for its completion.
    func call(_ body: @escaping (@escaping (Error?) -> Void) -> Void) -> Error? {
        let done = DispatchSemaphore(value: 0)
        var err: Error?
        queue.async {
            body { e in
                err = e
                done.signal()
            }
        }
        done.wait()
        return err
    }

    func result(_ cb: @escaping (Error?) -> Void) -> (Result<Void, Error>) -> Void {
        { r in
            if case .failure(let e) = r { cb(e) } else { cb(nil) }
        }
    }

    var state: String {
        queue.sync {
            switch vm.state {
            case .stopped: return "stopped"
            case .running: return "running"
            case .paused: return "paused"
            case .error: return "error"
            case .starting: return "starting"
            case .pausing: return "pausing"
            case .resuming: return "resuming"
            case .stopping: return "stopping"
            case .saving: return "saving"
            case .restoring: return "restoring"
            @unknown default: return "unknown"
            }
        }
    }

    /// A stream to the guest's vsock `port`, or `nil`.
    func vsock(_ port: UInt32) -> VZVirtioSocketConnection? {
        let done = DispatchSemaphore(value: 0)
        var conn: VZVirtioSocketConnection?
        queue.async {
            guard let dev = self.vm.socketDevices.first as? VZVirtioSocketDevice else {
                done.signal()
                return
            }
            dev.connect(toPort: port) { r in
                if case .success(let c) = r { conn = c }
                done.signal()
            }
        }
        done.wait()
        return conn
    }

    // ------------------------------------------------------------ servers

    /// `CONNECT <port>` → `OK <port>`, then the guest's stream.
    func serveVsock(_ c: Int32) {
        guard let line = Sockets.readLine(c, limit: 64), let port = parseConnect(line),
            let conn = vsock(port)
        else {
            close(c)
            return
        }
        guard Sockets.writeAll(c, Data("OK \(port)\n".utf8)) else {
            conn.close()
            close(c)
            return
        }
        Sockets.splice(c, conn.fileDescriptor) {
            conn.close()
            close(c)
        }
    }

    /// A published port: the init connects to the guest's own port.
    func serveForward(_ c: Int32, guest: UInt16) {
        var one: Int32 = 1
        setsockopt(c, IPPROTO_TCP, TCP_NODELAY, &one, 4)
        guard let conn = vsock(initPort) else {
            close(c)
            return
        }
        let fd = conn.fileDescriptor
        let req = Data("{\"op\":\"forward\",\"port\":\(guest)}\n".utf8)
        guard Sockets.writeAll(fd, req), let reply = Sockets.readLine(fd),
            reply.contains("\"ok\":true")
        else {
            conn.close()
            close(c)
            return
        }
        Sockets.splice(c, fd) {
            conn.close()
            close(c)
        }
    }

    func serveControl(_ c: Int32) {
        defer { close(c) }
        guard let line = Sockets.readLine(c),
            let req = try? JSONDecoder().decode(ControlRequest.self, from: Data(line.utf8))
        else {
            Sockets.writeAll(c, ControlReply(ok: false, error: "bad request").line())
            return
        }
        let started = Date()
        let ms = { Int(Date().timeIntervalSince(started) * 1000) }
        var reply: ControlReply
        switch req.op {
        case "state":
            reply = ControlReply(ok: true, state: state)
        case "pause":
            if state == "paused" {
                reply = ControlReply(ok: true, state: "paused")
            } else if let e = call({ cb in self.vm.pause(completionHandler: self.result(cb)) }) {
                reply = ControlReply(ok: false, error: "pause: \(e.localizedDescription)")
            } else {
                reply = ControlReply(ok: true, state: state, ms: ms())
            }
        case "resume":
            if state == "running" {
                reply = ControlReply(ok: true, state: "running")
            } else if let e = call({ cb in self.vm.resume(completionHandler: self.result(cb)) }) {
                reply = ControlReply(ok: false, error: "resume: \(e.localizedDescription)")
            } else {
                reply = ControlReply(ok: true, state: state, ms: ms())
            }
        case "save":
            guard let path = req.path else {
                reply = ControlReply(ok: false, error: "save needs a path")
                break
            }
            if state != "paused" {
                reply = ControlReply(ok: false, error: "save needs a paused VM (it is \(state))")
            } else if let e = call({ cb in
                self.vm.saveMachineStateTo(url: url(path), completionHandler: cb)
            }) {
                reply = ControlReply(ok: false, error: "save: \(e.localizedDescription)")
            } else {
                reply = ControlReply(ok: true, state: state, ms: ms())
            }
        case "stop":
            Sockets.writeAll(c, ControlReply(ok: true, state: "stopping").line())
            shutdownAndExit(0)
        default:
            reply = ControlReply(ok: false, error: "unknown op \(req.op)")
        }
        Sockets.writeAll(c, reply.line())
    }

    // ------------------------------------------------------------ lifecycle

    func run() {
        let configuration: VZVirtualMachineConfiguration
        do {
            configuration = try makeConfiguration()
        } catch {
            cleanup()
            fail("configuration: \(error)")
        }
        vm = VZVirtualMachine(configuration: configuration, queue: queue)
        queue.sync { vm.delegate = self }
        let started = Date()
        if let saved = cfg.restore {
            if let e = call({ cb in self.vm.restoreMachineStateFrom(url: url(saved), completionHandler: cb) }) {
                cleanup()
                fail("restore \(saved): \(e.localizedDescription)", code: 3)
            }
            let restored = Date()
            if let e = call({ cb in self.vm.resume(completionHandler: self.result(cb)) }) {
                cleanup()
                fail("resume after restore: \(e.localizedDescription)", code: 3)
            }
            log(
                "restored in \(Int(restored.timeIntervalSince(started) * 1000)) ms, "
                    + "resumed in \(Int(Date().timeIntervalSince(restored) * 1000)) ms")
        } else {
            if let e = call({ cb in self.vm.start(completionHandler: self.result(cb)) }) {
                cleanup()
                fail("start: \(e.localizedDescription)")
            }
            log("started in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
        }
        do {
            for f in cfg.forwards {
                let fd = try Sockets.listenLoopback(f.host)
                listeners.append(fd)
                let guest = f.guest
                Sockets.serve(fd, name: "port \(f.host)") { c in self.serveForward(c, guest: guest) }
            }
            let v = try Sockets.listenUnix(sock("v.sock"))
            listeners.append(v)
            Sockets.serve(v, name: "vsock") { c in self.serveVsock(c) }
            // Last: its presence tells the runtime the VM runs.
            let api = try Sockets.listenUnix(sock("api.sock"))
            listeners.append(api)
            Sockets.serve(api, name: "api") { c in self.serveControl(c) }
        } catch {
            log("\(error)")
            shutdownAndExit(1)
        }
    }

    func cleanup() {
        for fd in listeners { close(fd) }
        for name in ["api.sock", "v.sock", "net.sock", "gv.sock"] { unlink(sock(name)) }
        if let p = gateway, p.isRunning {
            p.terminate()
            p.waitUntilExit()
        }
    }

    func shutdownAndExit(_ code: Int32) -> Never {
        if vm != nil, state != "stopped" {
            _ = call { cb in self.vm.stop(completionHandler: cb) }
        }
        cleanup()
        exit(code)
    }

    func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        log("the guest stopped")
        DispatchQueue.global().async { self.cleanup(); exit(0) }
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        log("the VM stopped: \(error.localizedDescription)")
        DispatchQueue.global().async { self.cleanup(); exit(1) }
    }
}

let host = Host()
let signalSources: [DispatchSourceSignal] = [SIGTERM, SIGINT, SIGHUP].map { sig in
    signal(sig, SIG_IGN)
    let src = DispatchSource.makeSignalSource(signal: sig, queue: .global())
    src.setEventHandler { host.shutdownAndExit(0) }
    src.resume()
    return src
}
DispatchQueue.global().async { host.run() }
dispatchMain()
