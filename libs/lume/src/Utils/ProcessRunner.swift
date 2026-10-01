import Foundation

/// Protocol for process execution
protocol ProcessRunner {
    func run(executable: String, arguments: [String]) throws
}

class DefaultProcessRunner: ProcessRunner {
    func run(executable: String, arguments: [String]) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        try process.run()
    }
}