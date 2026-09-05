import ArgumentParser
import Foundation

struct Export: ParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "export",
    abstract: "Export a virtual machine to another format",
    subcommands: [ExportUTM.self]
  )
}

struct ExportUTM: AsyncParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "utm",
    abstract: "Export a stopped Lume macOS VM as a UTM bundle",
    discussion: """
      Creates an independent UTM Apple/macOS .utm bundle using APFS
      copy-on-write cloning when possible. A fresh machine identifier and
      MAC address are generated so the source and export cannot collide.

      The Lume VM must be fully shut down.

      Example:
        lume export utm my-mac "~/Virtual Machines/my-mac.utm"
      """
  )

  @Argument(help: "Name of the source Lume VM", completion: .custom(completeVMName))
  var name: String

  @Argument(help: "Destination .utm bundle path", completion: .file())
  var output: Path

  @Option(name: .customLong("storage"), help: "Source Lume storage location")
  var storage: String?

  init() {}

  @MainActor
  func run() async throws {
    TelemetryClient.shared.record(event: TelemetryEvent.exportUTM)
    let source = try Home().getVMDirectory(name, storage: storage)
    let outputURL = try UTMConverter().exportUTM(from: source, to: output.url)
    print("Exported Lume VM '\(source.name)' to \(outputURL.path)")
  }
}
