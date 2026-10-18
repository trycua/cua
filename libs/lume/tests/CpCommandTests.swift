import Foundation
import Testing

@testable import lume

@Test("cp parses VM:PATH references as remote endpoints")
func cpParsesRemoteEndpoints() {
    #expect(Cp.parseEndpoint("my-vm:/tmp/file") == .remote(vm: "my-vm", path: "/tmp/file"))
    #expect(Cp.parseEndpoint("my-vm:relative/path") == .remote(vm: "my-vm", path: "relative/path"))
    // Empty remote path (defaults to the guest home directory) is still remote.
    #expect(Cp.parseEndpoint("my-vm:") == .remote(vm: "my-vm", path: ""))
}

@Test("cp keeps host paths local even when they contain a colon")
func cpKeepsHostPathsLocal() {
    #expect(Cp.parseEndpoint("/tmp/file") == .local("/tmp/file"))
    #expect(Cp.parseEndpoint("./build.zip") == .local("./build.zip"))
    // A colon after a path separator is a local path, matching scp.
    #expect(Cp.parseEndpoint("/tmp/x:y") == .local("/tmp/x:y"))
    #expect(Cp.parseEndpoint("./a:b") == .local("./a:b"))
}

@Test("scp file-copy arguments target the host → guest direction")
func scpUploadArguments() throws {
    let client = SystemSSHClient(host: "192.168.64.24")
    let endpoint = client.remoteScpEndpoint(path: "/tmp/dest file.txt")
    let arguments = client.fileCopyArguments(
        sources: ["/tmp/source.txt"],
        destination: endpoint,
        recursive: false
    )
    let separator = try #require(arguments.firstIndex(of: "--"))

    #expect(!arguments.contains("-r"))
    #expect(arguments.contains("StrictHostKeyChecking=no"))
    #expect(arguments.contains("ServerAliveInterval=15"))
    // Remote path is shell-quoted so spaces survive the guest shell.
    #expect(endpoint == "lume@192.168.64.24:'/tmp/dest file.txt'")
    #expect(Array(arguments[(separator + 1)...]) == ["/tmp/source.txt", endpoint])
}

@Test("scp file-copy arguments carry -r and a port override for recursive copies")
func scpRecursiveArgumentsWithPort() throws {
    let client = SystemSSHClient(
        host: "192.168.64.24",
        port: 2222,
        user: "lume",
        password: "secret"
    )
    let endpoint = client.remoteScpEndpoint(path: "/tmp/out.log")
    let arguments = client.fileCopyArguments(
        sources: [endpoint],
        destination: "/tmp/local.log",
        recursive: true
    )
    let separator = try #require(arguments.firstIndex(of: "--"))

    #expect(arguments.contains("-r"))
    #expect(arguments.contains("-P"))
    #expect(arguments.contains("2222"))
    // guest → host: the remote endpoint is the source, the host path the target.
    #expect(Array(arguments[(separator + 1)...]) == [endpoint, "/tmp/local.log"])
    #expect(endpoint == "lume@192.168.64.24:'/tmp/out.log'")
}
