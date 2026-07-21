import Foundation

struct DetachedRunResult: Sendable {
  let processIdentifier: Int32
  let logURL: URL
}

enum DetachedRunError: LocalizedError {
  case executableUnavailable
  case launchFailed(String)

  var errorDescription: String? {
    switch self {
    case .executableUnavailable:
      return "Cannot locate the Lume executable for detached launch."
    case .launchFailed(let message):
      return "Failed to launch the detached VM process: \(message)"
    }
  }
}

enum DetachedVMRunner {
  static func launch(
    vmName: String,
    logPath: String?,
    arguments: [String] = CommandLine.arguments
  ) throws -> DetachedRunResult {
    guard let executableURL = Bundle.main.executableURL else {
      throw DetachedRunError.executableUnavailable
    }

    let logURL = resolvedLogURL(vmName: vmName, customPath: logPath)
    let fileManager = FileManager.default
    try fileManager.createDirectory(
      at: logURL.deletingLastPathComponent(),
      withIntermediateDirectories: true
    )
    if !fileManager.fileExists(atPath: logURL.path) {
      guard fileManager.createFile(atPath: logURL.path, contents: nil) else {
        throw DetachedRunError.launchFailed("Cannot create log file at \(logURL.path)")
      }
    }

    let logHandle = try FileHandle(forWritingTo: logURL)
    try logHandle.seekToEnd()

    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/nohup")
    process.arguments = [executableURL.path] + childArguments(from: arguments)
    process.currentDirectoryURL = URL(
      fileURLWithPath: FileManager.default.currentDirectoryPath,
      isDirectory: true
    )
    var environment = ProcessInfo.processInfo.environment
    environment["NSUnbufferedIO"] = "YES"
    process.environment = environment
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = logHandle
    process.standardError = logHandle

    do {
      try process.run()
    } catch {
      try? logHandle.close()
      throw DetachedRunError.launchFailed(error.localizedDescription)
    }
    try? logHandle.close()

    return DetachedRunResult(
      processIdentifier: process.processIdentifier,
      logURL: logURL
    )
  }

  static func childArguments(from arguments: [String]) -> [String] {
    var childArguments: [String] = []
    var iterator = Array(arguments.dropFirst()).makeIterator()
    while let argument = iterator.next() {
      switch argument {
      case "--detach":
        continue
      case "--log-file":
        _ = iterator.next()
      default:
        if !argument.hasPrefix("--log-file=") {
          childArguments.append(argument)
        }
      }
    }
    return childArguments
  }

  static func defaultLogURL(vmName: String) -> URL {
    let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
    let safeName = String(
      vmName.unicodeScalars.map { allowed.contains($0) ? Character(String($0)) : "_" }
    )
    return FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Logs/lume", isDirectory: true)
      .appendingPathComponent("\(safeName).log")
  }

  private static func resolvedLogURL(vmName: String, customPath: String?) -> URL {
    guard let customPath else { return defaultLogURL(vmName: vmName) }
    let expanded = NSString(string: customPath).expandingTildeInPath
    if expanded.hasPrefix("/") {
      return URL(fileURLWithPath: expanded)
    }
    return URL(
      fileURLWithPath: expanded,
      relativeTo: URL(
        fileURLWithPath: FileManager.default.currentDirectoryPath,
        isDirectory: true
      )
    ).standardizedFileURL
  }
}
