import Foundation
import ArgumentParser

extension Collection {
  subscript (safe index: Index) -> Element? {
    indices.contains(index) ? self[index] : nil
  }
}

func resolveBinaryPath(_ name: String) -> URL? {
  guard let path = ProcessInfo.processInfo.environment["PATH"] else {
    return nil
  }

  for pathComponent in path.split(separator: ":") {
    let url = URL(fileURLWithPath: String(pathComponent))
      .appendingPathComponent(name, isDirectory: false)

    if FileManager.default.fileExists(atPath: url.path) {
      return url
    }
  }

  return nil
}

// Helper function to parse size strings
func parseSize(_ input: String) throws -> UInt64 {
    let lowercased = input.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    let multiplier: Double 
    let valueString: String

    if lowercased.hasSuffix("tb") {
        multiplier = 1024 * 1024 * 1024 * 1024
        valueString = String(lowercased.dropLast(2))
    } else if lowercased.hasSuffix("gb") {
        multiplier = 1024 * 1024 * 1024
        valueString = String(lowercased.dropLast(2))
    } else if lowercased.hasSuffix("mb") {
        multiplier = 1024 * 1024
        valueString = String(lowercased.dropLast(2))
    } else if lowercased.hasSuffix("kb") {
        multiplier = 1024
        valueString = String(lowercased.dropLast(2))
    } else {
        multiplier = 1024 * 1024
        valueString = lowercased
    }

    guard let value = Double(valueString.trimmingCharacters(in: .whitespacesAndNewlines)) else {
        throw ValidationError("Malformed size input: \(input). Could not parse numeric value.")
    }

    let bytesAsDouble = (value * multiplier).rounded()
    
    guard bytesAsDouble >= 0 && bytesAsDouble <= Double(UInt64.max) else {
        throw ValidationError("Calculated size out of bounds for UInt64: \(input)")
    }

    let val = UInt64(bytesAsDouble)

    return val
}
