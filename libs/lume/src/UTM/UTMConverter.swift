import Darwin
import Foundation
import Virtualization

enum UTMConversionError: Error, LocalizedError {
  case bundleNotFound(String)
  case invalidBundle(String)
  case invalidFile(String)
  case unsupportedBackend(String)
  case unsupportedVersion(Int)
  case unsupportedGuest(String)
  case unsupportedConfiguration(String)
  case missingFile(String)
  case sourceInUse(String)
  case destinationExists(String)
  case invalidVMName(String)
  case copyFailed(source: String, destination: String, reason: String)

  var errorDescription: String? {
    switch self {
    case .bundleNotFound(let path):
      return "UTM bundle not found: \(path)"
    case .invalidBundle(let reason):
      return "Invalid UTM bundle: \(reason)"
    case .invalidFile(let reason):
      return "Invalid VM file: \(reason)"
    case .unsupportedBackend(let backend):
      return "Unsupported UTM backend '\(backend)'. Only Apple macOS VMs can be imported into Lume."
    case .unsupportedVersion(let version):
      return
        "Unsupported UTM configuration version \(version). This Lume build supports version \(UTMBundleConfiguration.supportedVersion)."
    case .unsupportedGuest(let guest):
      return "Unsupported UTM guest '\(guest)'. Only Apple/macOS guests can be imported into Lume."
    case .unsupportedConfiguration(let reason):
      return "Unsupported UTM configuration: \(reason)"
    case .missingFile(let path):
      return "Required VM file not found: \(path)"
    case .sourceInUse(let path):
      return
        "VM source is in use: \(path). Fully shut down the VM before importing or exporting it."
    case .destinationExists(let path):
      return "Destination already exists: \(path)"
    case .invalidVMName(let name):
      return "Invalid VM name '\(name)'. Use a single directory name without '/'."
    case .copyFailed(let source, let destination, let reason):
      return "Could not copy \(source) to \(destination): \(reason)"
    }
  }
}

/// The version-4 UTM Apple configuration fields used by the converter.
/// Unknown UI-only plist fields are intentionally ignored.
struct UTMBundleConfiguration: Codable {
  static let supportedVersion = 4

  var backend: String
  var configurationVersion: Int
  var information: Information
  var system: System
  var virtualization: Virtualization
  var displays: [Display]
  var drives: [Drive]
  var networks: [Network]
  var serials: [Serial]

  enum CodingKeys: String, CodingKey {
    case backend = "Backend"
    case configurationVersion = "ConfigurationVersion"
    case information = "Information"
    case system = "System"
    case virtualization = "Virtualization"
    case displays = "Display"
    case drives = "Drive"
    case networks = "Network"
    case serials = "Serial"
  }

  struct Information: Codable {
    var name: String
    var iconCustom: Bool
    var uuid: UUID

    enum CodingKeys: String, CodingKey {
      case name = "Name"
      case iconCustom = "IconCustom"
      case uuid = "UUID"
    }
  }

  struct System: Codable {
    var architecture: String
    var cpuCount: Int
    /// UTM stores guest memory in MiB.
    var memorySize: Int
    var boot: Boot
    var macPlatform: MacPlatform?

    enum CodingKeys: String, CodingKey {
      case architecture = "Architecture"
      case cpuCount = "CPUCount"
      case memorySize = "MemorySize"
      case boot = "Boot"
      case macPlatform = "MacPlatform"
    }
  }

  struct Boot: Codable {
    var operatingSystem: String
    var uefiBoot: Bool

    enum CodingKeys: String, CodingKey {
      case operatingSystem = "OperatingSystem"
      case uefiBoot = "UEFIBoot"
    }

    init(operatingSystem: String, uefiBoot: Bool) {
      self.operatingSystem = operatingSystem
      self.uefiBoot = uefiBoot
    }

    init(from decoder: Decoder) throws {
      let values = try decoder.container(keyedBy: CodingKeys.self)
      operatingSystem = try values.decode(String.self, forKey: .operatingSystem)
      uefiBoot = try values.decodeIfPresent(Bool.self, forKey: .uefiBoot) ?? false
    }
  }

  struct MacPlatform: Codable {
    var hardwareModel: Data
    var machineIdentifier: Data
    var auxiliaryStoragePath: String?

    enum CodingKeys: String, CodingKey {
      case hardwareModel = "HardwareModel"
      case machineIdentifier = "MachineIdentifier"
      case auxiliaryStoragePath = "AuxiliaryStoragePath"
    }
  }

  struct Virtualization: Codable {
    var audio: Bool
    var balloon: Bool
    var entropy: Bool
    var keyboard: String
    var pointer: String
    var rosetta: Bool?
    var clipboardSharing: Bool

    enum CodingKeys: String, CodingKey {
      case audio = "Audio"
      case balloon = "Balloon"
      case entropy = "Entropy"
      case keyboard = "Keyboard"
      case pointer = "Pointer"
      case trackpad = "Trackpad"
      case rosetta = "Rosetta"
      case clipboardSharing = "ClipboardSharing"
    }

    init(
      audio: Bool,
      balloon: Bool,
      entropy: Bool,
      keyboard: String,
      pointer: String,
      rosetta: Bool?,
      clipboardSharing: Bool
    ) {
      self.audio = audio
      self.balloon = balloon
      self.entropy = entropy
      self.keyboard = keyboard
      self.pointer = pointer
      self.rosetta = rosetta
      self.clipboardSharing = clipboardSharing
    }

    init(from decoder: Decoder) throws {
      let values = try decoder.container(keyedBy: CodingKeys.self)
      audio = try values.decodeIfPresent(Bool.self, forKey: .audio) ?? false
      balloon = try values.decodeIfPresent(Bool.self, forKey: .balloon) ?? true
      entropy = try values.decodeIfPresent(Bool.self, forKey: .entropy) ?? true
      if let value = try? values.decode(String.self, forKey: .keyboard) {
        keyboard = value
      } else {
        keyboard =
          (try values.decodeIfPresent(Bool.self, forKey: .keyboard) ?? false)
          ? "Generic" : "Disabled"
      }
      if let value = try? values.decode(String.self, forKey: .pointer) {
        pointer = value
      } else if try values.decodeIfPresent(Bool.self, forKey: .trackpad) ?? false {
        pointer = "Trackpad"
      } else {
        pointer =
          (try values.decodeIfPresent(Bool.self, forKey: .pointer) ?? false)
          ? "Mouse" : "Disabled"
      }
      rosetta = try values.decodeIfPresent(Bool.self, forKey: .rosetta)
      clipboardSharing =
        try values.decodeIfPresent(Bool.self, forKey: .clipboardSharing) ?? false
    }

    func encode(to encoder: Encoder) throws {
      var values = encoder.container(keyedBy: CodingKeys.self)
      try values.encode(audio, forKey: .audio)
      try values.encode(balloon, forKey: .balloon)
      try values.encode(entropy, forKey: .entropy)
      try values.encode(keyboard, forKey: .keyboard)
      try values.encode(pointer, forKey: .pointer)
      try values.encodeIfPresent(rosetta, forKey: .rosetta)
      try values.encode(clipboardSharing, forKey: .clipboardSharing)
    }
  }

  struct Display: Codable {
    var widthPixels: Int
    var heightPixels: Int
    var pixelsPerInch: Int
    var dynamicResolution: Bool

    enum CodingKeys: String, CodingKey {
      case widthPixels = "WidthPixels"
      case heightPixels = "HeightPixels"
      case pixelsPerInch = "PixelsPerInch"
      case dynamicResolution = "DynamicResolution"
    }

    init(
      widthPixels: Int,
      heightPixels: Int,
      pixelsPerInch: Int,
      dynamicResolution: Bool
    ) {
      self.widthPixels = widthPixels
      self.heightPixels = heightPixels
      self.pixelsPerInch = pixelsPerInch
      self.dynamicResolution = dynamicResolution
    }

    init(from decoder: Decoder) throws {
      let values = try decoder.container(keyedBy: CodingKeys.self)
      widthPixels = try values.decode(Int.self, forKey: .widthPixels)
      heightPixels = try values.decode(Int.self, forKey: .heightPixels)
      pixelsPerInch = try values.decode(Int.self, forKey: .pixelsPerInch)
      dynamicResolution =
        try values.decodeIfPresent(Bool.self, forKey: .dynamicResolution) ?? true
    }
  }

  struct Drive: Codable {
    var identifier: String
    var imageName: String?
    var readOnly: Bool
    var nvme: Bool

    enum CodingKeys: String, CodingKey {
      case identifier = "Identifier"
      case imageName = "ImageName"
      case readOnly = "ReadOnly"
      case nvme = "Nvme"
    }

    init(identifier: String, imageName: String?, readOnly: Bool, nvme: Bool) {
      self.identifier = identifier
      self.imageName = imageName
      self.readOnly = readOnly
      self.nvme = nvme
    }

    init(from decoder: Decoder) throws {
      let values = try decoder.container(keyedBy: CodingKeys.self)
      identifier = try values.decode(String.self, forKey: .identifier)
      imageName = try values.decodeIfPresent(String.self, forKey: .imageName)
      readOnly = try values.decodeIfPresent(Bool.self, forKey: .readOnly) ?? (imageName == nil)
      nvme = try values.decodeIfPresent(Bool.self, forKey: .nvme) ?? false
    }
  }

  struct Network: Codable {
    var mode: String
    var macAddress: String
    var bridgeInterface: String?

    enum CodingKeys: String, CodingKey {
      case mode = "Mode"
      case macAddress = "MacAddress"
      case bridgeInterface = "BridgeInterface"
    }
  }

  struct Serial: Codable {
    var mode: String?

    enum CodingKeys: String, CodingKey {
      case mode = "Mode"
    }
  }
}

struct UTMConverter {
  private static let bytesPerMiB = UInt64(1_048_576)
  private static let qcowMagic = Data([0x51, 0x46, 0x49, 0xfb])
  private static let asifMagic = Data("shdw".utf8)
  private static let udifMagic = Data("koly".utf8)

  private let fileManager: FileManager
  private let filesAreInUse: ([URL]) throws -> Bool

  init(
    fileManager: FileManager = .default,
    filesAreInUse: @escaping ([URL]) throws -> Bool = UTMConverter.defaultFilesAreInUse
  ) {
    self.fileManager = fileManager
    self.filesAreInUse = filesAreInUse
  }

  /// Imports a stopped UTM Apple/macOS bundle into a new Lume VM directory.
  /// The requested name is passed separately so path normalization cannot turn
  /// a parent-relative name into an apparently valid destination basename.
  func importUTM(from bundleURL: URL, named requestedName: String, to destination: VMDirectory)
    throws
  {
    try validateVMName(requestedName)
    guard destination.name == requestedName else {
      throw UTMConversionError.invalidVMName(requestedName)
    }
    guard !entryExists(at: destination.dir.url) else {
      throw UTMConversionError.destinationExists(destination.dir.path)
    }

    let source = try loadBundle(at: bundleURL)
    try validateImportConfiguration(source.configuration, rawPropertyList: source.rawPropertyList)

    guard let platform = source.configuration.system.macPlatform else {
      throw UTMConversionError.unsupportedConfiguration("missing macOS platform metadata")
    }
    _ = try validateHardwareModel(platform.hardwareModel)
    guard VZMacMachineIdentifier(dataRepresentation: platform.machineIdentifier) != nil else {
      throw UTMConversionError.unsupportedConfiguration("invalid Mac machine identifier")
    }

    guard source.configuration.drives.count == 1 else {
      throw UTMConversionError.unsupportedConfiguration(
        "expected exactly one internal boot disk; found \(source.configuration.drives.count)")
    }
    let drive = source.configuration.drives[0]
    guard let imageName = drive.imageName else {
      throw UTMConversionError.unsupportedConfiguration(
        "external or removable disks are not supported")
    }
    guard !drive.readOnly else {
      throw UTMConversionError.unsupportedConfiguration("the boot disk must be writable")
    }
    guard !drive.nvme else {
      throw UTMConversionError.unsupportedConfiguration("NVMe disks are not supported")
    }

    guard let auxiliaryStoragePath = platform.auxiliaryStoragePath else {
      throw UTMConversionError.unsupportedConfiguration(
        "missing configured auxiliary-storage file")
    }
    let diskURL = try resolveDataFile(named: imageName, in: source.dataURL)
    let auxiliaryURL = try resolveDataFile(named: auxiliaryStoragePath, in: source.dataURL)
    guard diskURL != auxiliaryURL else {
      throw UTMConversionError.invalidBundle(
        "the boot disk and auxiliary storage refer to the same file")
    }

    let stateURL = source.dataURL.appendingPathComponent("vmstate", isDirectory: false)
    if entryExists(at: stateURL) {
      throw UTMConversionError.unsupportedConfiguration(
        "the bundle contains suspended state; resume and fully shut down the UTM VM first")
    }
    if try filesAreInUse([source.configURL, diskURL, auxiliaryURL]) {
      throw UTMConversionError.sourceInUse(bundleURL.path)
    }

    try validateRawDiskImage(at: diskURL)
    _ = VZMacAuxiliaryStorage(url: auxiliaryURL)

    let display = source.configuration.displays[0]
    let network = source.configuration.networks[0]
    let networkMode: NetworkMode
    switch network.mode {
    case "Shared":
      networkMode = .nat
    case "Bridged":
      if let interface = network.bridgeInterface, interface.isEmpty {
        throw UTMConversionError.unsupportedConfiguration("bridged interface identifier is empty")
      }
      networkMode = .bridged(interface: network.bridgeInterface)
    default:
      throw UTMConversionError.unsupportedConfiguration("unknown network mode '\(network.mode)'")
    }

    guard let memoryMiB = UInt64(exactly: source.configuration.system.memorySize),
      memoryMiB <= UInt64.max / Self.bytesPerMiB
    else {
      throw UTMConversionError.unsupportedConfiguration("invalid guest memory size")
    }
    let memoryBytes = memoryMiB * Self.bytesPerMiB
    try validateCPUAndMemory(
      cpuCount: source.configuration.system.cpuCount, memorySize: memoryBytes)

    let diskSize = try regularFileSize(at: diskURL)
    let config = try VMConfig(
      os: "macOS",
      cpuCount: source.configuration.system.cpuCount,
      memorySize: memoryBytes,
      diskSize: diskSize,
      macAddress: Self.generateFreshMacAddress(excluding: network.macAddress),
      display: "\(display.widthPixels)x\(display.heightPixels)",
      hardwareModel: platform.hardwareModel,
      machineIdentifier: Self.generateFreshMachineIdentifier(excluding: platform.machineIdentifier),
      networkMode: networkMode
    )

    try createAtomically(at: destination.dir.url) { temporaryURL in
      let temporary = VMDirectory(Path(temporaryURL))
      try cloneOrCopyFile(from: diskURL, to: temporary.diskPath.url)
      try cloneOrCopyFile(from: auxiliaryURL, to: temporary.nvramPath.url)
      try temporary.saveConfig(config)
    }
  }

  /// Exports a fully initialized, stopped Lume macOS VM as an independent UTM bundle.
  func exportUTM(from source: VMDirectory, to requestedOutputURL: URL) throws -> URL {
    let outputURL =
      requestedOutputURL.pathExtension.lowercased() == "utm"
      ? requestedOutputURL
      : requestedOutputURL.appendingPathExtension("utm")
    guard !entryExists(at: outputURL) else {
      throw UTMConversionError.destinationExists(outputURL.path)
    }
    guard !source.provisioningPath.exists(), !source.hasResizeMarker() else {
      throw UTMConversionError.unsupportedConfiguration(
        "the Lume VM is provisioning or has an incomplete disk-resize transaction")
    }

    let configSize = try regularFileSize(at: source.configPath.url)
    guard configSize > 0 else {
      throw VMError.notInitialized(source.name)
    }
    let diskSize = try regularFileSize(at: source.diskPath.url)
    _ = try regularFileSize(at: source.nvramPath.url)

    let configLock: FileHandle
    do {
      configLock = try FileHandle(forUpdating: source.configPath.url)
    } catch {
      throw VMError.notInitialized(source.name)
    }
    defer { try? configLock.close() }
    guard flock(configLock.fileDescriptor, LOCK_EX | LOCK_NB) == 0 else {
      throw UTMConversionError.sourceInUse(source.dir.path)
    }
    defer { flock(configLock.fileDescriptor, LOCK_UN) }

    if try filesAreInUse([source.diskPath.url, source.nvramPath.url]) {
      throw UTMConversionError.sourceInUse(source.dir.path)
    }

    let config = try source.loadConfig()
    guard config.os == "macOS" else {
      throw UTMConversionError.unsupportedGuest(config.os)
    }
    guard let hardwareModelData = config.hardwareModel else {
      throw UTMConversionError.unsupportedConfiguration("missing Mac hardware model")
    }
    _ = try validateHardwareModel(hardwareModelData)
    guard let originalMachineIdentifier = config.machineIdentifier,
      VZMacMachineIdentifier(dataRepresentation: originalMachineIdentifier) != nil
    else {
      throw UTMConversionError.unsupportedConfiguration("invalid or missing Mac machine identifier")
    }
    guard let originalMacAddress = config.macAddress,
      VZMACAddress(string: originalMacAddress) != nil
    else {
      throw UTMConversionError.unsupportedConfiguration("invalid or missing network MAC address")
    }
    guard let cpuCount = config.cpuCount, let memorySize = config.memorySize else {
      throw UTMConversionError.unsupportedConfiguration("missing CPU or memory configuration")
    }
    try validateCPUAndMemory(cpuCount: cpuCount, memorySize: memorySize)
    guard memorySize % Self.bytesPerMiB == 0 else {
      throw UTMConversionError.unsupportedConfiguration(
        "guest memory must be an exact whole number of MiB for UTM")
    }
    guard let configuredDiskSize = config.diskSize, configuredDiskSize == diskSize else {
      throw UTMConversionError.unsupportedConfiguration(
        "configured disk size does not match disk.img")
    }
    guard config.display.width > 0, config.display.height > 0 else {
      throw UTMConversionError.unsupportedConfiguration("invalid display dimensions")
    }

    try validateRawDiskImage(at: source.diskPath.url)
    _ = VZMacAuxiliaryStorage(url: source.nvramPath.url)

    guard let memoryMiB = Int(exactly: memorySize / Self.bytesPerMiB) else {
      throw UTMConversionError.unsupportedConfiguration("guest memory size is too large")
    }
    let bundleConfig = UTMBundleConfiguration(
      backend: "Apple",
      configurationVersion: UTMBundleConfiguration.supportedVersion,
      information: .init(name: source.name, iconCustom: false, uuid: UUID()),
      system: .init(
        architecture: "aarch64",
        cpuCount: cpuCount,
        memorySize: memoryMiB,
        boot: .init(operatingSystem: "macOS", uefiBoot: false),
        macPlatform: .init(
          hardwareModel: hardwareModelData,
          machineIdentifier: Self.generateFreshMachineIdentifier(
            excluding: originalMachineIdentifier),
          auxiliaryStoragePath: "AuxiliaryStorage")
      ),
      virtualization: .init(
        audio: true,
        balloon: false,
        entropy: true,
        keyboard: "Mac",
        pointer: "Trackpad",
        rosetta: false,
        clipboardSharing: true
      ),
      displays: [
        .init(
          widthPixels: config.display.width,
          heightPixels: config.display.height,
          pixelsPerInch: 80,
          dynamicResolution: true)
      ],
      drives: [
        .init(
          identifier: UUID().uuidString,
          imageName: "disk.img",
          readOnly: false,
          nvme: false)
      ],
      networks: [makeUTMNetwork(from: config.networkMode, excluding: originalMacAddress)],
      serials: []
    )

    try createAtomically(at: outputURL) { temporaryURL in
      let dataURL = temporaryURL.appendingPathComponent("Data", isDirectory: true)
      try fileManager.createDirectory(at: dataURL, withIntermediateDirectories: false)
      try cloneOrCopyFile(
        from: source.diskPath.url,
        to: dataURL.appendingPathComponent("disk.img", isDirectory: false))
      try cloneOrCopyFile(
        from: source.nvramPath.url,
        to: dataURL.appendingPathComponent("AuxiliaryStorage", isDirectory: false))

      let encoder = PropertyListEncoder()
      encoder.outputFormat = .xml
      let plist = try encoder.encode(bundleConfig)
      try plist.write(
        to: temporaryURL.appendingPathComponent("config.plist", isDirectory: false),
        options: .atomic)
    }
    return outputURL
  }

  private func loadBundle(at bundleURL: URL) throws -> (
    configuration: UTMBundleConfiguration,
    configURL: URL,
    dataURL: URL,
    rawPropertyList: [String: Any]
  ) {
    var isDirectory: ObjCBool = false
    guard fileManager.fileExists(atPath: bundleURL.path, isDirectory: &isDirectory),
      isDirectory.boolValue
    else {
      throw UTMConversionError.bundleNotFound(bundleURL.path)
    }

    let configURL = bundleURL.appendingPathComponent("config.plist", isDirectory: false)
    let dataURL = bundleURL.appendingPathComponent("Data", isDirectory: true)
    _ = try regularFileSize(at: configURL)
    try requireDirectoryWithoutSymlink(at: dataURL)

    do {
      let plistData = try Data(contentsOf: configURL)
      guard
        let rawPropertyList = try PropertyListSerialization.propertyList(
          from: plistData,
          options: [],
          format: nil
        ) as? [String: Any]
      else {
        throw UTMConversionError.invalidBundle("config.plist is not a dictionary")
      }
      guard let backend = rawPropertyList["Backend"] as? String else {
        throw UTMConversionError.invalidBundle("missing or invalid Backend")
      }
      guard backend == "Apple" else {
        throw UTMConversionError.unsupportedBackend(backend)
      }
      guard let version = rawPropertyList["ConfigurationVersion"] as? Int else {
        throw UTMConversionError.invalidBundle("missing or invalid ConfigurationVersion")
      }
      guard version == UTMBundleConfiguration.supportedVersion else {
        throw UTMConversionError.unsupportedVersion(version)
      }

      let configuration = try PropertyListDecoder().decode(
        UTMBundleConfiguration.self,
        from: plistData)
      return (configuration, configURL, dataURL, rawPropertyList)
    } catch let error as UTMConversionError {
      throw error
    } catch {
      throw UTMConversionError.invalidBundle(error.localizedDescription)
    }
  }

  private func validateImportConfiguration(
    _ configuration: UTMBundleConfiguration,
    rawPropertyList: [String: Any]
  ) throws {
    guard configuration.backend == "Apple" else {
      throw UTMConversionError.unsupportedBackend(configuration.backend)
    }
    guard configuration.configurationVersion == UTMBundleConfiguration.supportedVersion else {
      throw UTMConversionError.unsupportedVersion(configuration.configurationVersion)
    }
    guard configuration.system.architecture == "aarch64" else {
      throw UTMConversionError.unsupportedConfiguration(
        "architecture '\(configuration.system.architecture)' is not Apple Silicon")
    }
    guard configuration.system.boot.operatingSystem == "macOS" else {
      throw UTMConversionError.unsupportedGuest(configuration.system.boot.operatingSystem)
    }
    guard !configuration.system.boot.uefiBoot else {
      throw UTMConversionError.unsupportedConfiguration("UEFI boot is not valid for a macOS guest")
    }
    guard configuration.system.macPlatform != nil else {
      throw UTMConversionError.unsupportedConfiguration("missing macOS platform metadata")
    }
    guard configuration.displays.count == 1 else {
      throw UTMConversionError.unsupportedConfiguration(
        "Lume currently supports one display; found \(configuration.displays.count)")
    }
    let display = configuration.displays[0]
    guard display.widthPixels > 0, display.heightPixels > 0, display.pixelsPerInch > 0 else {
      throw UTMConversionError.unsupportedConfiguration(
        "invalid display dimensions or pixel density")
    }
    guard configuration.networks.count == 1 else {
      throw UTMConversionError.unsupportedConfiguration(
        "Lume currently supports one network adapter; found \(configuration.networks.count)")
    }
    guard VZMACAddress(string: configuration.networks[0].macAddress) != nil else {
      throw UTMConversionError.unsupportedConfiguration("invalid network MAC address")
    }
    guard configuration.serials.isEmpty else {
      throw UTMConversionError.unsupportedConfiguration("serial devices are not supported")
    }
    if let sharedDirectories = rawPropertyList["SharedDirectory"] {
      guard let sharedDirectories = sharedDirectories as? [Any] else {
        throw UTMConversionError.invalidBundle("SharedDirectory is not an array")
      }
      guard sharedDirectories.isEmpty else {
        throw UTMConversionError.unsupportedConfiguration(
          "shared directories must be removed before import")
      }
    }
  }

  private func validateHardwareModel(_ data: Data) throws -> VZMacHardwareModel {
    guard let model = VZMacHardwareModel(dataRepresentation: data) else {
      throw UTMConversionError.unsupportedConfiguration("invalid Mac hardware model")
    }
    guard model.isSupported else {
      throw UTMConversionError.unsupportedConfiguration(
        "the Mac hardware model is not supported on this host")
    }
    return model
  }

  private func validateCPUAndMemory(cpuCount: Int, memorySize: UInt64) throws {
    guard cpuCount >= VZVirtualMachineConfiguration.minimumAllowedCPUCount,
      cpuCount <= VZVirtualMachineConfiguration.maximumAllowedCPUCount
    else {
      throw UTMConversionError.unsupportedConfiguration(
        "CPU count \(cpuCount) is outside the supported range \(VZVirtualMachineConfiguration.minimumAllowedCPUCount)...\(VZVirtualMachineConfiguration.maximumAllowedCPUCount)"
      )
    }
    guard memorySize >= VZVirtualMachineConfiguration.minimumAllowedMemorySize,
      memorySize <= VZVirtualMachineConfiguration.maximumAllowedMemorySize,
      memorySize % Self.bytesPerMiB == 0
    else {
      throw UTMConversionError.unsupportedConfiguration(
        "guest memory size is outside the Virtualization.framework range or is not MiB-aligned")
    }
  }

  private func makeUTMNetwork(
    from mode: NetworkMode,
    excluding originalMacAddress: String
  ) -> UTMBundleConfiguration.Network {
    switch mode {
    case .nat:
      return .init(
        mode: "Shared",
        macAddress: Self.generateFreshMacAddress(excluding: originalMacAddress),
        bridgeInterface: nil)
    case .bridged(let interface):
      return .init(
        mode: "Bridged",
        macAddress: Self.generateFreshMacAddress(excluding: originalMacAddress),
        bridgeInterface: interface)
    }
  }

  private func validateVMName(_ name: String) throws {
    guard !name.isEmpty,
      name != ".",
      name != "..",
      !name.contains("/"),
      !name.utf8.contains(0)
    else {
      throw UTMConversionError.invalidVMName(name)
    }
  }

  private func resolveDataFile(named name: String, in dataURL: URL) throws -> URL {
    guard !name.isEmpty,
      name != ".",
      name != "..",
      name == URL(fileURLWithPath: name).lastPathComponent
    else {
      throw UTMConversionError.invalidBundle("unsafe Data path '\(name)'")
    }

    let standardizedDataURL = dataURL.standardizedFileURL
    let url = standardizedDataURL.appendingPathComponent(name, isDirectory: false)
      .standardizedFileURL
    guard url.deletingLastPathComponent() == standardizedDataURL else {
      throw UTMConversionError.invalidBundle("unsafe Data path '\(name)'")
    }
    _ = try regularFileSize(at: url)
    return url
  }

  private func requireDirectoryWithoutSymlink(at url: URL) throws {
    var info = stat()
    let result: Int32 = url.withUnsafeFileSystemRepresentation { path in
      guard let path else { return -1 }
      return lstat(path, &info)
    }
    guard result == 0 else {
      throw UTMConversionError.missingFile(url.path)
    }
    guard (info.st_mode & S_IFMT) == S_IFDIR else {
      throw UTMConversionError.invalidBundle("not a real directory: \(url.path)")
    }
  }

  private func regularFileSize(at url: URL) throws -> UInt64 {
    var info = stat()
    let result: Int32 = url.withUnsafeFileSystemRepresentation { path in
      guard let path else { return -1 }
      return lstat(path, &info)
    }
    guard result == 0 else {
      throw UTMConversionError.missingFile(url.path)
    }
    guard (info.st_mode & S_IFMT) == S_IFREG, info.st_size > 0 else {
      throw UTMConversionError.invalidFile("not a non-empty regular file: \(url.path)")
    }
    return UInt64(info.st_size)
  }

  private func validateRawDiskImage(at url: URL) throws {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }

    let prefix = try handle.read(upToCount: 4) ?? Data()
    if prefix == Self.qcowMagic {
      throw UTMConversionError.unsupportedConfiguration("QCOW2 disk images are not supported")
    }
    if prefix == Self.asifMagic {
      throw UTMConversionError.unsupportedConfiguration("ASIF disk images are not supported")
    }

    let size = try handle.seekToEnd()
    if size >= 512 {
      try handle.seek(toOffset: size - 512)
      let suffixMagic = try handle.read(upToCount: 4) ?? Data()
      if suffixMagic == Self.udifMagic {
        throw UTMConversionError.unsupportedConfiguration("UDIF disk images are not supported")
      }
    }
  }

  private func entryExists(at url: URL) -> Bool {
    var info = stat()
    return url.withUnsafeFileSystemRepresentation { path in
      guard let path else { return false }
      return lstat(path, &info) == 0
    }
  }

  private func createAtomically(at destinationURL: URL, populate: (URL) throws -> Void) throws {
    guard !entryExists(at: destinationURL) else {
      throw UTMConversionError.destinationExists(destinationURL.path)
    }
    let parentURL = destinationURL.deletingLastPathComponent()
    var isDirectory: ObjCBool = false
    guard fileManager.fileExists(atPath: parentURL.path, isDirectory: &isDirectory),
      isDirectory.boolValue
    else {
      throw UTMConversionError.missingFile(parentURL.path)
    }

    let temporaryURL = parentURL.appendingPathComponent(
      ".partial-\(UUID().uuidString)",
      isDirectory: true)
    try fileManager.createDirectory(at: temporaryURL, withIntermediateDirectories: false)
    do {
      try populate(temporaryURL)
      guard !entryExists(at: destinationURL) else {
        throw UTMConversionError.destinationExists(destinationURL.path)
      }
      try fileManager.moveItem(at: temporaryURL, to: destinationURL)
    } catch {
      try? fileManager.removeItem(at: temporaryURL)
      throw error
    }
  }

  private func cloneOrCopyFile(from sourceURL: URL, to destinationURL: URL) throws {
    guard !entryExists(at: destinationURL) else {
      throw UTMConversionError.destinationExists(destinationURL.path)
    }
    if Darwin.clonefile(sourceURL.path, destinationURL.path, 0) == 0 {
      return
    }

    let cloneError = errno
    try? fileManager.removeItem(at: destinationURL)
    do {
      try fileManager.copyItem(at: sourceURL, to: destinationURL)
    } catch {
      throw UTMConversionError.copyFailed(
        source: sourceURL.path,
        destination: destinationURL.path,
        reason: "clonefile errno \(cloneError); copy failed: \(error.localizedDescription)")
    }
  }

  private static func generateFreshMacAddress(excluding original: String) -> String {
    var generated: String
    repeat {
      generated = VZMACAddress.randomLocallyAdministered().string
    } while generated.caseInsensitiveCompare(original) == .orderedSame
    return generated
  }

  private static func generateFreshMachineIdentifier(excluding original: Data) -> Data {
    var generated: Data
    repeat {
      generated = VZMacMachineIdentifier().dataRepresentation
    } while generated == original
    return generated
  }

  static func defaultFilesAreInUse(_ urls: [URL]) throws -> Bool {
    for url in urls {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
      process.arguments = ["-t", url.path]
      let output = Pipe()
      process.standardOutput = output
      process.standardError = Pipe()
      try process.run()
      let data = try output.fileHandleForReading.readToEnd() ?? Data()
      process.waitUntilExit()
      if process.terminationStatus == 0 && !data.isEmpty {
        return true
      }
      if process.terminationStatus != 0 && process.terminationStatus != 1 {
        throw UTMConversionError.invalidBundle(
          "could not determine whether \(url.path) is in use")
      }
    }
    return false
  }
}
