import Foundation
import Testing
import Virtualization

@testable import lume

@Test("RunVMRequest parses shared directories correctly")
func testRunVMRequestParseSharedDirectories() async throws {
    // Create a temporary directory for testing
    let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempDir) }

    let request = RunVMRequest(
        noDisplay: true,
        sharedDirectories: [
            RunVMRequest.SharedDirectoryRequest(hostPath: tempDir.path, readOnly: true),
            RunVMRequest.SharedDirectoryRequest(hostPath: tempDir.path, readOnly: false),
            RunVMRequest.SharedDirectoryRequest(hostPath: tempDir.path, readOnly: nil)
        ],
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )

    let sharedDirs = try request.parse()
    #expect(sharedDirs.count == 3)
    #expect(sharedDirs[0].readOnly == true)
    #expect(sharedDirs[1].readOnly == false)
    #expect(sharedDirs[2].readOnly == false) // nil defaults to false
}

@Test("RunVMRequest throws error for invalid shared directory")
func testRunVMRequestInvalidSharedDirectory() async throws {
    let request = RunVMRequest(
        noDisplay: false,
        sharedDirectories: [
            RunVMRequest.SharedDirectoryRequest(hostPath: "/nonexistent/path", readOnly: false)
        ],
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )

    #expect(throws: ValidationError.self) {
        try request.parse()
    }
}

@Test("RunVMRequest parses network mode correctly")
func testRunVMRequestNetworkMode() async throws {
    let natRequest = RunVMRequest(
        noDisplay: false,
        sharedDirectories: nil,
        recoveryMode: false,
        storage: nil,
        network: "nat",
        clipboard: false
    )
    let natMode = try natRequest.parseNetworkMode()
    #expect(natMode == .nat)

    let bridgedRequest = RunVMRequest(
        noDisplay: false,
        sharedDirectories: nil,
        recoveryMode: false,
        storage: nil,
        network: "bridged",
        clipboard: false
    )
    let bridgedMode = try bridgedRequest.parseNetworkMode()
    #expect(bridgedMode == .bridged(interface: nil))

    let nilRequest = RunVMRequest(
        noDisplay: false,
        sharedDirectories: nil,
        recoveryMode: false,
        storage: nil,
        network: nil,
        clipboard: false
    )
    let nilMode = try nilRequest.parseNetworkMode()
    #expect(nilMode == nil)
}

@Test("PullRequest decoding with defaults")
func testPullRequestDecoding() async throws {
    let json = """
    {
        "image": "test-image:latest",
        "name": "test-vm"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PullRequest.self, from: data)

    #expect(request.image == "test-image:latest")
    #expect(request.name == "test-vm")
    #expect(request.registry == "ghcr.io")
    #expect(request.organization == "trycua")
    #expect(request.storage == nil)
}

@Test("PullRequest decoding with custom values")
func testPullRequestCustomValues() async throws {
    let json = """
    {
        "image": "custom-image:v1",
        "name": "custom-vm",
        "registry": "docker.io",
        "organization": "myorg",
        "storage": "/custom/path"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PullRequest.self, from: data)

    #expect(request.image == "custom-image:v1")
    #expect(request.name == "custom-vm")
    #expect(request.registry == "docker.io")
    #expect(request.organization == "myorg")
    #expect(request.storage == "/custom/path")
}

@Test("CreateVMRequest parses memory and disk size")
func testCreateVMRequestParse() async throws {
    let request = CreateVMRequest(
        name: "test-vm",
        os: "macos",
        cpu: 4,
        memory: "8GB",
        diskSize: "128GB",
        display: "1920x1080",
        ipsw: "latest",
        storage: nil,
        unattended: nil,
        network: nil
    )

    let (memory, diskSize) = try request.parse()
    #expect(memory == 8 * 1024 * 1024 * 1024)
    #expect(diskSize == 128 * 1024 * 1024 * 1024)
}

@Test("CreateVMRequest parses network mode")
func testCreateVMRequestNetworkMode() async throws {
    let natRequest = CreateVMRequest(
        name: "test",
        os: "macos",
        cpu: 2,
        memory: "4GB",
        diskSize: "64GB",
        display: "1024x768",
        ipsw: nil,
        storage: nil,
        unattended: nil,
        network: "nat"
    )
    #expect(try natRequest.parseNetworkMode() == .nat)

    let nilRequest = CreateVMRequest(
        name: "test",
        os: "macos",
        cpu: 2,
        memory: "4GB",
        diskSize: "64GB",
        display: "1024x768",
        ipsw: nil,
        storage: nil,
        unattended: nil,
        network: nil
    )
    #expect(try nilRequest.parseNetworkMode() == .nat)
}

@Test("SetVMRequest parses sizes and display")
func testSetVMRequestParse() async throws {
    let request = SetVMRequest(
        cpu: 8,
        memory: "16GB",
        diskSize: "256GB",
        display: "2560x1440",
        storage: nil
    )

    let (memory, diskSize, display) = try request.parse()
    #expect(memory == 16 * 1024 * 1024 * 1024)
    #expect(diskSize == 256 * 1024 * 1024 * 1024)
    #expect(display?.string == "2560x1440")
}

@Test("SetVMRequest handles nil values")
func testSetVMRequestNilValues() async throws {
    let request = SetVMRequest(
        cpu: nil,
        memory: nil,
        diskSize: nil,
        display: nil,
        storage: nil
    )

    let (memory, diskSize, display) = try request.parse()
    #expect(memory == nil)
    #expect(diskSize == nil)
    #expect(display == nil)
}

@Test("CloneRequest decoding")
func testCloneRequestDecoding() async throws {
    let json = """
    {
        "name": "source-vm",
        "newName": "cloned-vm",
        "sourceLocation": "/source",
        "destLocation": "/dest"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(CloneRequest.self, from: data)

    #expect(request.name == "source-vm")
    #expect(request.newName == "cloned-vm")
    #expect(request.sourceLocation == "/source")
    #expect(request.destLocation == "/dest")
}

@Test("SetupVMRequest decoding")
func testSetupVMRequestDecoding() async throws {
    let json = """
    {
        "configPath": "/path/to/config.yaml",
        "storage": "/storage",
        "vncPort": 5900,
        "noDisplay": true,
        "debug": false,
        "debugDir": "/tmp/debug"
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(SetupVMRequest.self, from: data)

    #expect(request.configPath == "/path/to/config.yaml")
    #expect(request.configYaml == nil)
    #expect(request.storage == "/storage")
    #expect(request.vncPort == 5900)
    #expect(request.noDisplay == true)
    #expect(request.debug == false)
    #expect(request.debugDir == "/tmp/debug")
}

@Test("PushRequest decoding with defaults")
func testPushRequestDefaults() async throws {
    let json = """
    {
        "name": "my-vm",
        "imageName": "my-image",
        "tags": ["latest", "v1.0"]
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PushRequest.self, from: data)

    #expect(request.name == "my-vm")
    #expect(request.imageName == "my-image")
    #expect(request.tags == ["latest", "v1.0"])
    #expect(request.registry == "ghcr.io")
    #expect(request.organization == "trycua")
    #expect(request.storage == nil)
    #expect(request.chunkSizeMb == 512)
    #expect(request.singleLayer == false)
}

@Test("PushRequest decoding with custom values")
func testPushRequestCustomValues() async throws {
    let json = """
    {
        "name": "my-vm",
        "imageName": "my-image",
        "tags": ["v2.0"],
        "registry": "custom.io",
        "organization": "myorg",
        "storage": "/custom",
        "chunkSizeMb": 1024,
        "singleLayer": true
    }
    """
    let data = json.data(using: .utf8)!
    let request = try JSONDecoder().decode(PushRequest.self, from: data)

    #expect(request.registry == "custom.io")
    #expect(request.organization == "myorg")
    #expect(request.storage == "/custom")
    #expect(request.chunkSizeMb == 1024)
    #expect(request.singleLayer == true)
}