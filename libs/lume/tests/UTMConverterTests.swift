import Darwin
import Foundation
import Testing
import Virtualization

@testable import lume

@Suite("UTM converter")
struct UTMConverterTests {
  @Test("Lume to UTM to Lume preserves boot state and refreshes identity")
  func roundTripPreservesBootStateAndRefreshesIdentity() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(
      in: root,
      name: "source-macos",
      networkMode: .bridged(interface: "en7")
    )
    let converter = UTMConverter(filesAreInUse: { _ in false })

    let bundleURL = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("round-trip")
    )
    #expect(bundleURL.pathExtension == "utm")

    let bundle = try UTMConverterFixtures.loadConfiguration(from: bundleURL)
    let platform = try #require(bundle.system.macPlatform)
    let exportedNetwork = try #require(bundle.networks.first)

    #expect(bundle.backend == "Apple")
    #expect(bundle.configurationVersion == UTMBundleConfiguration.supportedVersion)
    #expect(bundle.system.architecture == "aarch64")
    #expect(bundle.system.cpuCount == source.config.cpuCount)
    #expect(bundle.system.memorySize == 8 * 1024)
    #expect(bundle.system.boot.operatingSystem == "macOS")
    #expect(platform.hardwareModel == source.config.hardwareModel)
    #expect(platform.machineIdentifier != source.config.machineIdentifier)
    #expect(VZMacMachineIdentifier(dataRepresentation: platform.machineIdentifier) != nil)
    #expect(exportedNetwork.mode == "Bridged")
    #expect(exportedNetwork.bridgeInterface == "en7")
    #expect(exportedNetwork.macAddress != source.config.macAddress)
    #expect(VZMACAddress(string: exportedNetwork.macAddress) != nil)
    #expect(
      try Data(contentsOf: bundleURL.appendingPathComponent("Data/disk.img"))
        == source.diskData
    )
    #expect(
      try Data(contentsOf: bundleURL.appendingPathComponent("Data/AuxiliaryStorage"))
        == source.auxiliaryData
    )
    #expect(
      try UTMConverterFixtures.inode(of: bundleURL.appendingPathComponent("Data/disk.img"))
        != UTMConverterFixtures.inode(of: source.directory.diskPath.url)
    )
    #expect(
      try UTMConverterFixtures.inode(
        of: bundleURL.appendingPathComponent("Data/AuxiliaryStorage"))
        != UTMConverterFixtures.inode(of: source.directory.nvramPath.url)
    )

    let importedDirectory = VMDirectory(
      Path(root.appendingPathComponent("imported-macos").path)
    )
    try converter.importUTM(from: bundleURL, named: importedDirectory.name, to: importedDirectory)

    let importedConfig = try importedDirectory.loadConfig()
    #expect(try Data(contentsOf: importedDirectory.diskPath.url) == source.diskData)
    #expect(try Data(contentsOf: importedDirectory.nvramPath.url) == source.auxiliaryData)
    #expect(importedConfig.os == "macOS")
    #expect(importedConfig.cpuCount == source.config.cpuCount)
    #expect(importedConfig.memorySize == source.config.memorySize)
    #expect(importedConfig.diskSize == UInt64(source.diskData.count))
    #expect(importedConfig.display.string == source.config.display.string)
    #expect(importedConfig.networkMode == source.config.networkMode)
    #expect(importedConfig.hardwareModel == source.config.hardwareModel)
    #expect(importedConfig.machineIdentifier != source.config.machineIdentifier)
    #expect(importedConfig.machineIdentifier != platform.machineIdentifier)
    #expect(importedConfig.macAddress != source.config.macAddress)
    #expect(importedConfig.macAddress != exportedNetwork.macAddress)
    #expect(
      importedConfig.machineIdentifier.flatMap(VZMacMachineIdentifier.init(dataRepresentation:))
        != nil
    )
    #expect(importedConfig.macAddress.flatMap(VZMACAddress.init(string:)) != nil)
    #expect(
      try UTMConverterFixtures.inode(of: importedDirectory.diskPath.url)
        != UTMConverterFixtures.inode(of: bundleURL.appendingPathComponent("Data/disk.img"))
    )
    #expect(
      try UTMConverterFixtures.inode(of: importedDirectory.nvramPath.url)
        != UTMConverterFixtures.inode(
          of: bundleURL.appendingPathComponent("Data/AuxiliaryStorage"))
    )

    let unchangedSourceConfig = try source.directory.loadConfig()
    #expect(unchangedSourceConfig.machineIdentifier == source.config.machineIdentifier)
    #expect(unchangedSourceConfig.macAddress == source.config.macAddress)
  }

  @Test("Import rejects incompatible and unsafe UTM configurations")
  func importRejectsIncompatibleAndUnsafeConfigurations() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "rejection-source")
    let converter = UTMConverter(filesAreInUse: { _ in false })

    for rejection in ImportRejection.allCases {
      let bundleURL = try converter.exportUTM(
        from: source.directory,
        to: root.appendingPathComponent("reject-\(rejection.rawValue).utm")
      )
      var configuration = try UTMConverterFixtures.loadConfiguration(from: bundleURL)
      rejection.mutate(&configuration)
      try UTMConverterFixtures.saveConfiguration(configuration, to: bundleURL)

      let destination = VMDirectory(
        Path(root.appendingPathComponent("destination-\(rejection.rawValue)").path)
      )
      let error = try #require(
        UTMConverterFixtures.captureConversionError {
          try converter.importUTM(from: bundleURL, named: destination.name, to: destination)
        }
      )

      #expect(rejection.matches(error), "Unexpected error for \(rejection): \(error)")
      #expect(!destination.exists())
    }
  }

  @Test("Import rejects suspended state without leaving a partial VM")
  func importRejectsSuspendedStateAtomically() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "suspended-source")
    let converter = UTMConverter(filesAreInUse: { _ in false })
    let bundleURL = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("suspended.utm")
    )
    try Data([0x01]).write(
      to: bundleURL.appendingPathComponent("Data/vmstate", isDirectory: false)
    )

    let destination = VMDirectory(Path(root.appendingPathComponent("suspended-import").path))
    let error = try #require(
      UTMConverterFixtures.captureConversionError {
        try converter.importUTM(from: bundleURL, named: destination.name, to: destination)
      }
    )

    guard case .unsupportedConfiguration(let reason) = error else {
      Issue.record("Expected suspended-state rejection, got \(error)")
      return
    }
    #expect(reason.contains("suspended state"))
    #expect(!destination.exists())
    #expect(
      try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
        .allSatisfy { !$0.lastPathComponent.contains(".partial-") }
    )
  }

  @Test("In-use checks reject export and import before committing destinations")
  func inUseChecksRejectWithoutPartialDestinations() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "busy-source")
    let freeConverter = UTMConverter(filesAreInUse: { _ in false })
    let busyConverter = UTMConverter(filesAreInUse: { _ in true })

    let rejectedExportURL = root.appendingPathComponent("busy-export.utm")
    let exportError = try #require(
      UTMConverterFixtures.captureConversionError {
        _ = try busyConverter.exportUTM(from: source.directory, to: rejectedExportURL)
      }
    )
    guard case .sourceInUse(let exportSource) = exportError else {
      Issue.record("Expected source-in-use export error, got \(exportError)")
      return
    }
    #expect(exportSource == source.directory.dir.path)
    #expect(!FileManager.default.fileExists(atPath: rejectedExportURL.path))

    let importBundleURL = try freeConverter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("busy-import-source.utm")
    )
    let rejectedImport = VMDirectory(
      Path(root.appendingPathComponent("busy-import-destination").path)
    )
    let importError = try #require(
      UTMConverterFixtures.captureConversionError {
        try busyConverter.importUTM(
          from: importBundleURL, named: rejectedImport.name, to: rejectedImport)
      }
    )
    guard case .sourceInUse(let importSource) = importError else {
      Issue.record("Expected source-in-use import error, got \(importError)")
      return
    }
    #expect(importSource == importBundleURL.path)
    #expect(!rejectedImport.exists())
  }

  @Test("Advisory locks and open source files are detected")
  func realSourceUseChecksAreEnforced() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "real-lock-source")
    let configHandle = try FileHandle(forUpdating: source.directory.configPath.url)
    defer { try? configHandle.close() }
    try #require(flock(configHandle.fileDescriptor, LOCK_EX | LOCK_NB) == 0)
    defer { flock(configHandle.fileDescriptor, LOCK_UN) }

    let lockedOutput = root.appendingPathComponent("locked.utm")
    let lockError = try #require(
      UTMConverterFixtures.captureConversionError {
        _ = try UTMConverter(filesAreInUse: { _ in false }).exportUTM(
          from: source.directory,
          to: lockedOutput)
      }
    )
    guard case .sourceInUse = lockError else {
      Issue.record("Expected config lock rejection, got \(lockError)")
      return
    }
    #expect(!FileManager.default.fileExists(atPath: lockedOutput.path))

    flock(configHandle.fileDescriptor, LOCK_UN)
    let bundleURL = try UTMConverter(filesAreInUse: { _ in false }).exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("open-source.utm")
    )
    let openDisk = try FileHandle(
      forReadingFrom: bundleURL.appendingPathComponent("Data/disk.img"))
    defer { try? openDisk.close() }

    let destination = VMDirectory(Path(root.appendingPathComponent("open-source-import").path))
    let openError = try #require(
      UTMConverterFixtures.captureConversionError {
        try UTMConverter().importUTM(
          from: bundleURL,
          named: destination.name,
          to: destination)
      }
    )
    guard case .sourceInUse = openError else {
      Issue.record("Expected open file rejection, got \(openError)")
      return
    }
    #expect(!destination.exists())
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @Test("Import accepts UTM version-4 defaulted and legacy device fields")
  func importAcceptsVersion4Defaults() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "defaults-source")
    let converter = UTMConverter(filesAreInUse: { _ in false })
    let bundleURL = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("defaults.utm")
    )
    try UTMConverterFixtures.applyVersion4Defaults(to: bundleURL)

    let destination = VMDirectory(Path(root.appendingPathComponent("defaults-import").path))
    try converter.importUTM(
      from: bundleURL,
      named: destination.name,
      to: destination)

    #expect(destination.initialized())
    #expect(try Data(contentsOf: destination.diskPath.url) == source.diskData)
    #expect(try Data(contentsOf: destination.nvramPath.url) == source.auxiliaryData)
  }

  @Test("Import rejects symlink escapes and shared directories")
  func importRejectsSymlinkEscapesAndSharedDirectories() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "symlink-source")
    let converter = UTMConverter(filesAreInUse: { _ in false })

    let symlinkBundle = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("symlink.utm")
    )
    let outsideDisk = root.appendingPathComponent("outside.img")
    try Data([0x01, 0x02]).write(to: outsideDisk)
    let linkedDisk = symlinkBundle.appendingPathComponent("Data/linked.img")
    try FileManager.default.createSymbolicLink(at: linkedDisk, withDestinationURL: outsideDisk)
    var symlinkConfig = try UTMConverterFixtures.loadConfiguration(from: symlinkBundle)
    symlinkConfig.drives[0].imageName = "linked.img"
    try UTMConverterFixtures.saveConfiguration(symlinkConfig, to: symlinkBundle)

    let symlinkDestination = VMDirectory(Path(root.appendingPathComponent("symlink-import").path))
    let symlinkError = try #require(
      UTMConverterFixtures.captureConversionError {
        try converter.importUTM(
          from: symlinkBundle,
          named: symlinkDestination.name,
          to: symlinkDestination)
      }
    )
    guard case .invalidFile(let symlinkReason) = symlinkError else {
      Issue.record("Expected symlink bundle rejection, got \(symlinkError)")
      return
    }
    #expect(symlinkReason.contains("regular file"))
    #expect(!symlinkDestination.exists())

    let sharedBundle = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("shared-directory.utm")
    )
    try UTMConverterFixtures.addSharedDirectory(to: sharedBundle)
    let sharedDestination = VMDirectory(Path(root.appendingPathComponent("shared-import").path))
    let sharedError = try #require(
      UTMConverterFixtures.captureConversionError {
        try converter.importUTM(
          from: sharedBundle,
          named: sharedDestination.name,
          to: sharedDestination)
      }
    )
    guard case .unsupportedConfiguration(let sharedReason) = sharedError else {
      Issue.record("Expected shared-directory rejection, got \(sharedError)")
      return
    }
    #expect(sharedReason.contains("shared directories"))
    #expect(!sharedDestination.exists())
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @Test("Import and export reject unsupported disk image formats")
  func rejectsUnsupportedDiskFormats() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    for diskFormat in UnsupportedDiskFormat.allCases {
      let source = try UTMConverterFixtures.makeLumeVM(
        in: root,
        name: "format-source-\(diskFormat.rawValue)")
      try diskFormat.writeSignature(to: source.directory.diskPath.url)
      let converter = UTMConverter(filesAreInUse: { _ in false })
      let output = root.appendingPathComponent("format-export-\(diskFormat.rawValue).utm")
      let exportError = try #require(
        UTMConverterFixtures.captureConversionError {
          _ = try converter.exportUTM(from: source.directory, to: output)
        }
      )
      guard case .unsupportedConfiguration(let reason) = exportError else {
        Issue.record("Expected disk format rejection, got \(exportError)")
        continue
      }
      #expect(reason.contains(diskFormat.errorName))
      #expect(!FileManager.default.fileExists(atPath: output.path))
    }
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @Test("Existing destinations and invalid requested names are never overwritten")
  func existingDestinationsAndInvalidNamesAreRejected() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "destination-source")
    let converter = UTMConverter(filesAreInUse: { _ in false })
    let existingOutput = root.appendingPathComponent("existing.utm")
    try Data("keep".utf8).write(to: existingOutput)

    let exportError = try #require(
      UTMConverterFixtures.captureConversionError {
        _ = try converter.exportUTM(from: source.directory, to: existingOutput)
      }
    )
    guard case .destinationExists = exportError else {
      Issue.record("Expected existing export rejection, got \(exportError)")
      return
    }
    #expect(try Data(contentsOf: existingOutput) == Data("keep".utf8))

    let bundleURL = try converter.exportUTM(
      from: source.directory,
      to: root.appendingPathComponent("import-source.utm")
    )
    let existingImport = VMDirectory(Path(root.appendingPathComponent("existing-import").path))
    try FileManager.default.createDirectory(
      at: existingImport.dir.url,
      withIntermediateDirectories: false)
    let importError = try #require(
      UTMConverterFixtures.captureConversionError {
        try converter.importUTM(
          from: bundleURL,
          named: existingImport.name,
          to: existingImport)
      }
    )
    guard case .destinationExists = importError else {
      Issue.record("Expected existing import rejection, got \(importError)")
      return
    }

    let normalizedDestination = VMDirectory(
      Path(root.appendingPathComponent("escaped-name").path)
    )
    let nameError = try #require(
      UTMConverterFixtures.captureConversionError {
        try converter.importUTM(
          from: bundleURL,
          named: "../escaped-name",
          to: normalizedDestination)
      }
    )
    guard case .invalidVMName("../escaped-name") = nameError else {
      Issue.record("Expected invalid name rejection, got \(nameError)")
      return
    }
    #expect(!normalizedDestination.exists())
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @Test("Copy failures remove temporary output")
  func copyFailureRemovesTemporaryOutput() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let source = try UTMConverterFixtures.makeLumeVM(in: root, name: "copy-failure-source")
    let output = root.appendingPathComponent("copy-failure.utm")
    let converter = UTMConverter(filesAreInUse: { _ in
      try FileManager.default.removeItem(at: source.directory.nvramPath.url)
      return false
    })

    let error = try #require(
      UTMConverterFixtures.captureConversionError {
        _ = try converter.exportUTM(from: source.directory, to: output)
      }
    )
    guard case .copyFailed = error else {
      Issue.record("Expected copy failure, got \(error)")
      return
    }
    #expect(!FileManager.default.fileExists(atPath: output.path))
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @Test("Export validates lossless memory and source metadata")
  func exportValidatesSourceMetadata() throws {
    let root = try UTMConverterFixtures.makeTemporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    for rejection in ExportRejection.allCases {
      let source = try UTMConverterFixtures.makeLumeVM(
        in: root,
        name: "export-rejection-\(rejection.rawValue)")
      var config = source.config
      rejection.mutate(&config)
      try source.directory.saveConfig(config)

      let output = root.appendingPathComponent("export-rejected-\(rejection.rawValue).utm")
      let error = try #require(
        UTMConverterFixtures.captureConversionError {
          _ = try UTMConverter(filesAreInUse: { _ in false }).exportUTM(
            from: source.directory,
            to: output)
        }
      )
      #expect(rejection.matches(error), "Unexpected error for \(rejection): \(error)")
      #expect(!FileManager.default.fileExists(atPath: output.path))
    }
    try UTMConverterFixtures.expectNoPartialItems(in: root)
  }

  @MainActor
  @Test("Import and export UTM subcommands parse storage options")
  func commandsParseStorageOptions() throws {
    let importRoot = try Lume.parseAsRoot([
      "import", "utm", "/tmp/Test.utm", "imported", "--storage", "/tmp/vms",
    ])
    let importCommand = try #require(importRoot as? ImportUTM)
    #expect(importCommand.bundle.url.path == "/tmp/Test.utm")
    #expect(importCommand.name == "imported")
    #expect(importCommand.storage == "/tmp/vms")

    let exportRoot = try Lume.parseAsRoot([
      "export", "utm", "source", "/tmp/Test.utm", "--storage", "external",
    ])
    let exportCommand = try #require(exportRoot as? ExportUTM)
    #expect(exportCommand.name == "source")
    #expect(exportCommand.output.url.path == "/tmp/Test.utm")
    #expect(exportCommand.storage == "external")
  }
}

private enum ImportRejection: String, CaseIterable {
  case backend
  case version
  case guest
  case architecture
  case uefi
  case zeroCPU
  case zeroMemory
  case unsafeDiskPath
  case nestedDiskPath
  case absoluteDiskPath
  case unsafeAuxiliaryPath
  case invalidHardwareModel
  case invalidMachineIdentifier
  case invalidMacAddress
  case missingDiskImage
  case readOnlyDisk
  case nvmeDisk
  case multipleDisks
  case multipleDisplays
  case multipleNetworks
  case serialDevice
  case unsupportedNetworkMode

  func mutate(_ configuration: inout UTMBundleConfiguration) {
    switch self {
    case .backend:
      configuration.backend = "QEMU"
    case .version:
      configuration.configurationVersion = UTMBundleConfiguration.supportedVersion + 1
    case .guest:
      configuration.system.boot.operatingSystem = "Linux"
    case .architecture:
      configuration.system.architecture = "x86_64"
    case .uefi:
      configuration.system.boot.uefiBoot = true
    case .zeroCPU:
      configuration.system.cpuCount = 0
    case .zeroMemory:
      configuration.system.memorySize = 0
    case .unsafeDiskPath:
      configuration.drives[0].imageName = "../disk.img"
    case .nestedDiskPath:
      configuration.drives[0].imageName = "nested/disk.img"
    case .absoluteDiskPath:
      configuration.drives[0].imageName = "/tmp/disk.img"
    case .unsafeAuxiliaryPath:
      configuration.system.macPlatform?.auxiliaryStoragePath = ".."
    case .invalidHardwareModel:
      configuration.system.macPlatform?.hardwareModel = Data([0x00])
    case .invalidMachineIdentifier:
      configuration.system.macPlatform?.machineIdentifier = Data([0x00])
    case .invalidMacAddress:
      configuration.networks[0].macAddress = "invalid"
    case .missingDiskImage:
      configuration.drives[0].imageName = nil
    case .readOnlyDisk:
      configuration.drives[0].readOnly = true
    case .nvmeDisk:
      configuration.drives[0].nvme = true
    case .multipleDisks:
      configuration.drives.append(configuration.drives[0])
    case .multipleDisplays:
      configuration.displays.append(configuration.displays[0])
    case .multipleNetworks:
      configuration.networks.append(configuration.networks[0])
    case .serialDevice:
      configuration.serials.append(.init(mode: "Ptty"))
    case .unsupportedNetworkMode:
      configuration.networks[0].mode = "HostOnly"
    }
  }

  func matches(_ error: UTMConversionError) -> Bool {
    switch (self, error) {
    case (.backend, .unsupportedBackend("QEMU")):
      return true
    case (.version, .unsupportedVersion(5)):
      return true
    case (.guest, .unsupportedGuest("Linux")):
      return true
    case (.architecture, .unsupportedConfiguration(let reason)):
      return reason.contains("not Apple Silicon")
    case (.uefi, .unsupportedConfiguration(let reason)):
      return reason.contains("UEFI")
    case (.zeroCPU, .unsupportedConfiguration(let reason)):
      return reason.contains("CPU count")
    case (.zeroMemory, .unsupportedConfiguration(let reason)):
      return reason.contains("guest memory")
    case (.unsafeDiskPath, .invalidBundle(let reason)),
      (.nestedDiskPath, .invalidBundle(let reason)),
      (.absoluteDiskPath, .invalidBundle(let reason)),
      (.unsafeAuxiliaryPath, .invalidBundle(let reason)):
      return reason.contains("unsafe Data path")
    case (.invalidHardwareModel, .unsupportedConfiguration(let reason)):
      return reason == "invalid Mac hardware model"
    case (.invalidMachineIdentifier, .unsupportedConfiguration(let reason)):
      return reason == "invalid Mac machine identifier"
    case (.invalidMacAddress, .unsupportedConfiguration(let reason)):
      return reason == "invalid network MAC address"
    case (.missingDiskImage, .unsupportedConfiguration(let reason)):
      return reason.contains("external or removable")
    case (.readOnlyDisk, .unsupportedConfiguration(let reason)):
      return reason.contains("writable")
    case (.nvmeDisk, .unsupportedConfiguration(let reason)):
      return reason.contains("NVMe")
    case (.multipleDisks, .unsupportedConfiguration(let reason)):
      return reason.contains("exactly one internal boot disk")
    case (.multipleDisplays, .unsupportedConfiguration(let reason)):
      return reason.contains("one display")
    case (.multipleNetworks, .unsupportedConfiguration(let reason)):
      return reason.contains("one network adapter")
    case (.serialDevice, .unsupportedConfiguration(let reason)):
      return reason.contains("serial devices")
    case (.unsupportedNetworkMode, .unsupportedConfiguration(let reason)):
      return reason.contains("unknown network mode")
    default:
      return false
    }
  }
}

private enum ExportRejection: String, CaseIterable {
  case zeroCPU
  case unalignedMemory
  case diskSizeMismatch
  case invalidHardwareModel
  case invalidMachineIdentifier
  case invalidMacAddress

  func mutate(_ configuration: inout VMConfig) {
    switch self {
    case .zeroCPU:
      configuration.setCpuCount(0)
    case .unalignedMemory:
      configuration.setMemorySize(8 * 1024 * 1024 * 1024 + 1)
    case .diskSizeMismatch:
      configuration.setDiskSize((configuration.diskSize ?? 0) + 1)
    case .invalidHardwareModel:
      configuration.setHardwareModel(Data([0x00]))
    case .invalidMachineIdentifier:
      configuration.setMachineIdentifier(Data([0x00]))
    case .invalidMacAddress:
      configuration.setMacAddress("invalid")
    }
  }

  func matches(_ error: UTMConversionError) -> Bool {
    guard case .unsupportedConfiguration(let reason) = error else {
      return false
    }
    switch self {
    case .zeroCPU:
      return reason.contains("CPU count")
    case .unalignedMemory:
      return reason.contains("MiB")
    case .diskSizeMismatch:
      return reason.contains("disk size")
    case .invalidHardwareModel:
      return reason.contains("hardware model")
    case .invalidMachineIdentifier:
      return reason.contains("machine identifier")
    case .invalidMacAddress:
      return reason.contains("MAC address")
    }
  }
}

private enum UnsupportedDiskFormat: String, CaseIterable {
  case qcow2
  case asif
  case udif

  var errorName: String {
    rawValue.uppercased()
  }

  func writeSignature(to url: URL) throws {
    let handle = try FileHandle(forWritingTo: url)
    defer { try? handle.close() }
    switch self {
    case .qcow2:
      try handle.write(contentsOf: Data([0x51, 0x46, 0x49, 0xfb]))
    case .asif:
      try handle.write(contentsOf: Data("shdw".utf8))
    case .udif:
      let size = try handle.seekToEnd()
      try handle.seek(toOffset: size - 512)
      try handle.write(contentsOf: Data("koly".utf8))
    }
  }
}

private enum UTMConverterFixtures {
  struct LumeVM {
    let directory: VMDirectory
    let config: VMConfig
    let diskData: Data
    let auxiliaryData: Data
  }

  enum FixtureError: Error {
    case invalidHardwareModelFixture
  }

  static func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent(
      "UTMConverterTests-\(UUID().uuidString)",
      isDirectory: true
    )
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
    return url
  }

  static func makeLumeVM(
    in root: URL,
    name: String,
    networkMode: NetworkMode = .nat
  ) throws -> LumeVM {
    let directory = VMDirectory(Path(root.appendingPathComponent(name).path))
    try FileManager.default.createDirectory(
      at: directory.dir.url,
      withIntermediateDirectories: false
    )

    let diskData = Data((0..<16_384).map { UInt8(($0 * 31) % 251) })
    let auxiliaryData = Data((0..<4_096).map { UInt8(($0 * 17) % 253) })
    try diskData.write(to: directory.diskPath.url)
    try auxiliaryData.write(to: directory.nvramPath.url)

    let hardwareModel = try validHardwareModelData()
    let machineIdentifier = VZMacMachineIdentifier().dataRepresentation
    let config = try VMConfig(
      os: "macOS",
      cpuCount: 6,
      memorySize: 8 * 1024 * 1024 * 1024,
      diskSize: UInt64(diskData.count),
      macAddress: "02:00:00:00:00:11",
      display: "1920x1080",
      hardwareModel: hardwareModel,
      machineIdentifier: machineIdentifier,
      networkMode: networkMode
    )
    try directory.saveConfig(config)
    return LumeVM(
      directory: directory,
      config: config,
      diskData: diskData,
      auxiliaryData: auxiliaryData
    )
  }

  static func loadConfiguration(from bundleURL: URL) throws -> UTMBundleConfiguration {
    let data = try Data(
      contentsOf: bundleURL.appendingPathComponent("config.plist", isDirectory: false)
    )
    return try PropertyListDecoder().decode(UTMBundleConfiguration.self, from: data)
  }

  static func saveConfiguration(
    _ configuration: UTMBundleConfiguration,
    to bundleURL: URL
  ) throws {
    let encoder = PropertyListEncoder()
    encoder.outputFormat = .xml
    let data = try encoder.encode(configuration)
    try data.write(
      to: bundleURL.appendingPathComponent("config.plist", isDirectory: false),
      options: .atomic
    )
  }

  static func captureConversionError(_ operation: () throws -> Void) -> UTMConversionError? {
    do {
      try operation()
      Issue.record("Expected UTM conversion to fail")
      return nil
    } catch let error as UTMConversionError {
      return error
    } catch {
      Issue.record("Expected UTMConversionError, got \(error)")
      return nil
    }
  }

  static func inode(of url: URL) throws -> UInt64 {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return try #require((attributes[.systemFileNumber] as? NSNumber)?.uint64Value)
  }

  static func addSharedDirectory(to bundleURL: URL) throws {
    try updateRawConfiguration(in: bundleURL) { plist in
      plist["SharedDirectory"] = [
        ["Path": "/tmp/shared", "ReadOnly": true]
      ]
    }
  }

  static func applyVersion4Defaults(to bundleURL: URL) throws {
    try updateRawConfiguration(in: bundleURL) { plist in
      var system = try #require(plist["System"] as? [String: Any])
      var boot = try #require(system["Boot"] as? [String: Any])
      boot.removeValue(forKey: "UEFIBoot")
      system["Boot"] = boot
      plist["System"] = system

      var displays = try #require(plist["Display"] as? [[String: Any]])
      displays[0].removeValue(forKey: "DynamicResolution")
      plist["Display"] = displays

      var drives = try #require(plist["Drive"] as? [[String: Any]])
      drives[0].removeValue(forKey: "ReadOnly")
      drives[0].removeValue(forKey: "Nvme")
      plist["Drive"] = drives

      var virtualization = try #require(plist["Virtualization"] as? [String: Any])
      virtualization["Keyboard"] = true
      virtualization["Pointer"] = true
      virtualization["Trackpad"] = true
      virtualization.removeValue(forKey: "ClipboardSharing")
      plist["Virtualization"] = virtualization
    }
  }

  static func expectNoPartialItems(in root: URL) throws {
    let items = try FileManager.default.contentsOfDirectory(
      at: root,
      includingPropertiesForKeys: nil)
    #expect(items.allSatisfy { !$0.lastPathComponent.hasPrefix(".partial-") })
  }

  private static func updateRawConfiguration(
    in bundleURL: URL,
    update: (inout [String: Any]) throws -> Void
  ) throws {
    let configURL = bundleURL.appendingPathComponent("config.plist")
    let data = try Data(contentsOf: configURL)
    var plist = try #require(
      try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any]
    )
    try update(&plist)
    let updated = try PropertyListSerialization.data(
      fromPropertyList: plist,
      format: .xml,
      options: 0)
    try updated.write(to: configURL, options: .atomic)
  }

  private static func validHardwareModelData() throws -> Data {
    let encoded =
      "YnBsaXN0MDDTAQIDBAQFXxAZRGF0YVJlcHJlc2VudGF0aW9uVmVyc2lvbl8QD1BsYXRmb3JtVmVyc2lvbl8QEk1pbmltdW1TdXBwb3J0ZWRPUxACowYHBxANEAAIDys9UlRYWgAAAAAAAAEBAAAAAAAAAAgAAAAAAAAAAAAAAAAAAABc"
    guard let data = Data(base64Encoded: encoded),
      VZMacHardwareModel(dataRepresentation: data) != nil
    else {
      throw FixtureError.invalidHardwareModelFixture
    }
    return data
  }
}
