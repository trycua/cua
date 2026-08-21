import ArgumentParser
import Foundation

struct Import: ParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "import",
    abstract: "Import a virtual machine from another format",
    subcommands: [ImportUTM.self]
  )
}

struct ImportUTM: AsyncParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "utm",
    abstract: "Import a stopped UTM Apple/macOS VM into Lume",
    discussion: """
      Creates an independent Lume VM from a UTM .utm bundle. The raw disk,
      auxiliary storage, and hardware model are preserved as a matched set.
      A fresh machine identifier and MAC address are generated.

      The UTM VM must be fully shut down, not suspended.

      Example:
        lume import utm "~/Virtual Machines/MyMac.utm" my-mac
      """
  )

  @Argument(help: "Path to the source .utm bundle", completion: .file())
  var bundle: Path

  @Argument(help: "Name for the imported Lume VM")
  var name: String

  @Option(name: .customLong("storage"), help: "Destination Lume storage location")
  var storage: String?

  init() {}

  @MainActor
  func run() async throws {
    TelemetryClient.shared.record(event: TelemetryEvent.importUTM)
    let destination = try Home().getVMDirectory(name, storage: storage)
    try UTMConverter().importUTM(from: bundle.url, named: name, to: destination)
    print("Imported UTM VM as Lume VM '\(destination.name)' at \(destination.dir.path)")
  }
}
