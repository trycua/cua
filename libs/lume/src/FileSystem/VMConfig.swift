import ArgumentParser
import Foundation
import Virtualization

/// Represents a shared directory configuration
struct SharedDirectory: Codable {
    let hostPath: String
    let tag: String
    let readOnly: Bool

    var string: String {
        return "\(hostPath):\(tag):\(readOnly ? "ro" : "rw")"
    }
}

// MARK: - VMConfig
struct VMConfig: Codable {
    
    // MARK: - Properties
    let os: String
    private var _cpuCount: Int?
    private var _memorySize: UInt64?
    private var _diskSize: UInt64?
    private var _macAddress: String?
    private var _display: VMDisplayResolution
    private var _hardwareModel: Data?
    private var _machineIdentifier: Data?
    private var _networkMode: NetworkMode
    
    // MARK: - Initialization
    init(
        os: String,
        cpuCount: Int? = nil,
        memorySize: UInt64? = nil,
        diskSize: UInt64? = nil,
        macAddress: String? = nil,
        display: String,
        hardwareModel: Data? = nil,
        machineIdentifier: Data? = nil,
        networkMode: NetworkMode = .nat
    ) throws {
        self.os = os
        self._cpuCount = cpuCount
        self._memorySize = memorySize
        self._diskSize = diskSize
        self._macAddress = macAddress
        self._display = VMDisplayResolution(string: display) ?? VMDisplayResolution(string: "1024x768")!
        self._hardwareModel = hardwareModel
        self._machineIdentifier = machineIdentifier
        self._networkMode = networkMode
    }
    
    var display: VMDisplayResolution {
        get { _display }
        set { _display = newValue }
    }
    
    var cpuCount: Int? {
        get { _cpuCount }
        set { _cpuCount = newValue }
    }
    
    var memorySize: UInt64? {
        get { _memorySize }
        set { _memorySize = newValue }
    }
    
    var diskSize: UInt64? {
        get { _diskSize }
        set { _diskSize = newValue }
    }

    var hardwareModel: Data? {
        get { _hardwareModel }
        set { _hardwareModel = newValue }
    }

    var machineIdentifier: Data? {
        get { _machineIdentifier }
        set { _machineIdentifier = newValue }
    }

    var macAddress: String? {
        get { _macAddress }
        set { _macAddress = newValue }
    }

    var networkMode: NetworkMode {
        get { _networkMode }
        set { _networkMode = newValue }
    }
    
    mutating func setCpuCount(_ count: Int) {
        _cpuCount = count
    }
    
    mutating func setMemorySize(_ size: UInt64) {
        _memorySize = size
    }
    
    mutating func setDiskSize(_ size: UInt64) {
        _diskSize = size
    }

    mutating func setHardwareModel(_ hardwareModel: Data) {
        _hardwareModel = hardwareModel
    }

    mutating func setMachineIdentifier(_ machineIdentifier: Data) {
        _machineIdentifier = machineIdentifier
    }

    mutating func setMacAddress(_ newMacAddress: String) {
        self._macAddress = newMacAddress
    }

    mutating func setDisplay(_ newDisplay: VMDisplayResolution) {
        self._display = newDisplay
    }

    mutating func setNetworkMode(_ newNetworkMode: NetworkMode) {
        self._networkMode = newNetworkMode
    }

    // MARK: - Codable
    enum CodingKeys: String, CodingKey {
        case _cpuCount = "cpuCount"
        case _memorySize = "memorySize"
        case _diskSize = "diskSize"
        case macAddress
        case display
        case _hardwareModel = "hardwareModel"
        case _machineIdentifier = "machineIdentifier"
        case os
        case networkMode
    }
    
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        
        os = try container.decode(String.self, forKey: .os)
        _cpuCount = try container.decodeIfPresent(Int.self, forKey: ._cpuCount)
        _memorySize = try container.decodeIfPresent(UInt64.self, forKey: ._memorySize)
        _diskSize = try container.decodeIfPresent(UInt64.self, forKey: ._diskSize)
        _macAddress = try container.decodeIfPresent(String.self, forKey: .macAddress)
        _display = VMDisplayResolution(string: try container.decode(String.self, forKey: .display))!
        _hardwareModel = try container.decodeIfPresent(Data.self, forKey: ._hardwareModel)
        _machineIdentifier = try container.decodeIfPresent(Data.self, forKey: ._machineIdentifier)
        // Default to .nat for backward compatibility with existing configs
        _networkMode = try container.decodeIfPresent(NetworkMode.self, forKey: .networkMode) ?? .nat
    }
    
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        
        try container.encodeIfPresent(os, forKey: .os)
        try container.encodeIfPresent(_cpuCount, forKey: ._cpuCount)
        try container.encodeIfPresent(_memorySize, forKey: ._memorySize)
        try container.encodeIfPresent(_diskSize, forKey: ._diskSize)
        try container.encodeIfPresent(_macAddress, forKey: .macAddress)
        try container.encode(display.string, forKey: .display)
        try container.encodeIfPresent(_hardwareModel, forKey: ._hardwareModel)
        try container.encodeIfPresent(_machineIdentifier, forKey: ._machineIdentifier)
        try container.encode(_networkMode, forKey: .networkMode)
    }
}
