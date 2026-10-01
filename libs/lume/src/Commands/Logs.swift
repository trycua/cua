import ArgumentParser
import Foundation

struct Logs: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "View lume serve logs",
        subcommands: [Info.self, Error.self, All.self],
        defaultSubcommand: All.self
    )
    
    // Common functionality for reading log files
    static func readLogFile(path: String, lines: Int? = nil, follow: Bool = false) -> String {
        let fileManager = FileManager.default
        
        // Check if file exists
        guard fileManager.fileExists(atPath: path) else {
            return "Log file not found at \(path)"
        }
        
        do {
            // Read file content
            let content = try String(contentsOfFile: path, encoding: .utf8)
            
            // If lines parameter is provided, return only the specified number of lines from the end
            if let lineCount = lines {
                let allLines = content.components(separatedBy: .newlines)
                let startIndex = max(0, allLines.count - lineCount)
                let lastLines = Array(allLines[startIndex...])
                return lastLines.joined(separator: "\n")
            }
            
            return content
        } catch {
            return "Error reading log file: \(error.localizedDescription)"
        }
    }
    
    // Method for tailing a log file (following new changes)
    static func tailLogFile(path: String, initialLines: Int? = 10) {
        let fileManager = FileManager.default
        
        // Check if file exists
        guard fileManager.fileExists(atPath: path) else {
            print("Log file not found at \(path)")
            return
        }
        
        do {
            // Get initial content with only the specified number of lines from the end
            var lastPosition: UInt64 = 0
            let fileHandle = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
            
            // First, print the last few lines of the file
            if let lines = initialLines {
                let content = try String(contentsOfFile: path, encoding: .utf8)
                let allLines = content.components(separatedBy: .newlines)
                let startIndex = max(0, allLines.count - lines)
                let lastLines = Array(allLines[startIndex...])
                print(lastLines.joined(separator: "\n"))
            }
            
            // Get current file size
            lastPosition = UInt64(try fileManager.attributesOfItem(atPath: path)[.size] as? UInt64 ?? 0)
            
            // Set up for continuous monitoring
            print("\nTailing log file... Press Ctrl+C to stop")
            
            // Monitor file for changes
            while true {
                // Brief pause to reduce CPU usage
                Thread.sleep(forTimeInterval: 0.5)
                
                // Get current size
                let currentSize = try fileManager.attributesOfItem(atPath: path)[.size] as? UInt64 ?? 0
                
                // If file has grown
                if currentSize > lastPosition {
                    // Seek to where we last read
                    fileHandle.seek(toFileOffset: lastPosition)
                    
                    // Read new content
                    if let newData = try? fileHandle.readToEnd() {
                        if let newContent = String(data: newData, encoding: .utf8) {
                            // Print new content without trailing newline
                            if newContent.hasSuffix("\n") {
                                print(newContent, terminator: "")
                            } else {
                                print(newContent)
                            }
                        }
                    }
                    
                    // Update position
                    lastPosition = currentSize
                }
                
                // Handle file rotation (if file became smaller)
                else if currentSize < lastPosition {
                    // File was probably rotated, start from beginning
                    lastPosition = 0
                    fileHandle.seek(toFileOffset: 0)
                    
                    if let newData = try? fileHandle.readToEnd() {
                        if let newContent = String(data: newData, encoding: .utf8) {
                            print(newContent, terminator: "")
                        }
                    }
                    
                    lastPosition = currentSize
                }
            }
        } catch {
            print("Error tailing log file: \(error.localizedDescription)")
        }
    }
    
    // MARK: - Info Logs Subcommand
    
    struct Info: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "info",
            abstract: "View info logs from the daemon"
        )
        
        @Option(name: .shortAndLong, help: "Number of lines to display from the end of the file")
        var lines: Int?
        
        @Flag(name: .shortAndLong, help: "Follow log file continuously (like tail -f)")
        var follow: Bool = false
        
        func run() throws {
            let logPath = "/tmp/lume_daemon.log"
            
            print("=== Info Logs ===")
            
            if follow {
                // Use tailing functionality to continuously monitor the log
                Logs.tailLogFile(path: logPath, initialLines: lines ?? 10)
            } else {
                // Regular one-time viewing of logs
                let content = Logs.readLogFile(path: logPath, lines: lines)
                print(content)
            }
        }
    }
    
    // MARK: - Error Logs Subcommand
    
    struct Error: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "error",
            abstract: "View error logs from the daemon"
        )
        
        @Option(name: .shortAndLong, help: "Number of lines to display from the end of the file")
        var lines: Int?
        
        @Flag(name: .shortAndLong, help: "Follow log file continuously (like tail -f)")
        var follow: Bool = false
        
        func run() throws {
            let logPath = "/tmp/lume_daemon.error.log"
            
            print("=== Error Logs ===")
            
            if follow {
                // Use tailing functionality to continuously monitor the log
                Logs.tailLogFile(path: logPath, initialLines: lines ?? 10)
            } else {
                // Regular one-time viewing of logs
                let content = Logs.readLogFile(path: logPath, lines: lines)
                print(content)
            }
        }
    }
    
    // MARK: - All Logs Subcommand
    
    struct All: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "all",
            abstract: "View both info and error logs from the daemon"
        )
        
        @Option(name: .shortAndLong, help: "Number of lines to display from the end of each file")
        var lines: Int?
        
        @Flag(name: .shortAndLong, help: "Follow log files continuously (like tail -f)")
        var follow: Bool = false
        
        // Custom implementation to tail both logs simultaneously
        private func tailBothLogs(infoPath: String, errorPath: String, initialLines: Int? = 10) {
            let fileManager = FileManager.default
            var infoExists = fileManager.fileExists(atPath: infoPath)
            var errorExists = fileManager.fileExists(atPath: errorPath)
            
            if !infoExists && !errorExists {
                print("Neither info nor error log files found")
                return
            }
            
            // Print initial content
            print("=== Info Logs ===")
            if infoExists {
                if let lines = initialLines {
                    let content = (try? String(contentsOfFile: infoPath, encoding: .utf8)) ?? ""
                    let allLines = content.components(separatedBy: .newlines)
                    let startIndex = max(0, allLines.count - lines)
                    let lastLines = Array(allLines[startIndex...])
                    print(lastLines.joined(separator: "\n"))
                }
            } else {
                print("Info log file not found")
            }
            
            print("\n=== Error Logs ===")
            if errorExists {
                if let lines = initialLines {
                    let content = (try? String(contentsOfFile: errorPath, encoding: .utf8)) ?? ""
                    let allLines = content.components(separatedBy: .newlines)
                    let startIndex = max(0, allLines.count - lines)
                    let lastLines = Array(allLines[startIndex...])
                    print(lastLines.joined(separator: "\n"))
                }
            } else {
                print("Error log file not found")
            }
            
            print("\nTailing both log files... Press Ctrl+C to stop")
            
            // Initialize file handles and positions
            var infoHandle: FileHandle? = nil
            var errorHandle: FileHandle? = nil
            var infoPosition: UInt64 = 0
            var errorPosition: UInt64 = 0
            
            // Set up file handles
            if infoExists {
                do {
                    infoHandle = try FileHandle(forReadingFrom: URL(fileURLWithPath: infoPath))
                    infoPosition = UInt64(try fileManager.attributesOfItem(atPath: infoPath)[.size] as? UInt64 ?? 0)
                } catch {
                    print("Error opening info log file: \(error.localizedDescription)")
                }
            }
            
            if errorExists {
                do {
                    errorHandle = try FileHandle(forReadingFrom: URL(fileURLWithPath: errorPath))
                    errorPosition = UInt64(try fileManager.attributesOfItem(atPath: errorPath)[.size] as? UInt64 ?? 0)
                } catch {
                    print("Error opening error log file: \(error.localizedDescription)")
                }
            }
            
            // Monitor both files for changes
            while true {
                Thread.sleep(forTimeInterval: 0.5)
                
                // Check for new content in info log
                if let handle = infoHandle {
                    do {
                        // Re-check existence in case file was deleted
                        infoExists = fileManager.fileExists(atPath: infoPath)
                        if !infoExists {
                            print("\n[Info log file was removed]")
                            infoHandle = nil
                            continue
                        }
                        
                        let currentSize = try fileManager.attributesOfItem(atPath: infoPath)[.size] as? UInt64 ?? 0
                        
                        if currentSize > infoPosition {
                            handle.seek(toFileOffset: infoPosition)
                            if let newData = try? handle.readToEnd() {
                                if let newContent = String(data: newData, encoding: .utf8) {
                                    print("\n--- New Info Log Content ---")
                                    if newContent.hasSuffix("\n") {
                                        print(newContent, terminator: "")
                                    } else {
                                        print(newContent)
                                    }
                                }
                            }
                            infoPosition = currentSize
                        } else if currentSize < infoPosition {
                            // File was rotated
                            print("\n[Info log was rotated]")
                            infoPosition = 0
                            handle.seek(toFileOffset: 0)
                            if let newData = try? handle.readToEnd() {
                                if let newContent = String(data: newData, encoding: .utf8) {
                                    print("\n--- New Info Log Content ---")
                                    print(newContent, terminator: "")
                                }
                            }
                            infoPosition = currentSize
                        }
                    } catch {
                        print("\nError reading info log: \(error.localizedDescription)")
                    }
                } else if fileManager.fileExists(atPath: infoPath) && !infoExists {
                    // File exists again after being deleted
                    do {
                        infoHandle = try FileHandle(forReadingFrom: URL(fileURLWithPath: infoPath))
                        infoPosition = 0
                        infoExists = true
                        print("\n[Info log file reappeared]")
                    } catch {
                        print("\nError reopening info log: \(error.localizedDescription)")
                    }
                }
                
                // Check for new content in error log
                if let handle = errorHandle {
                    do {
                        // Re-check existence in case file was deleted
                        errorExists = fileManager.fileExists(atPath: errorPath)
                        if !errorExists {
                            print("\n[Error log file was removed]")
                            errorHandle = nil
                            continue
                        }
                        
                        let currentSize = try fileManager.attributesOfItem(atPath: errorPath)[.size] as? UInt64 ?? 0
                        
                        if currentSize > errorPosition {
                            handle.seek(toFileOffset: errorPosition)
                            if let newData = try? handle.readToEnd() {
                                if let newContent = String(data: newData, encoding: .utf8) {
                                    print("\n--- New Error Log Content ---")
                                    if newContent.hasSuffix("\n") {
                                        print(newContent, terminator: "")
                                    } else {
                                        print(newContent)
                                    }
                                }
                            }
                            errorPosition = currentSize
                        } else if currentSize < errorPosition {
                            // File was rotated
                            print("\n[Error log was rotated]")
                            errorPosition = 0
                            handle.seek(toFileOffset: 0)
                            if let newData = try? handle.readToEnd() {
                                if let newContent = String(data: newData, encoding: .utf8) {
                                    print("\n--- New Error Log Content ---")
                                    print(newContent, terminator: "")
                                }
                            }
                            errorPosition = currentSize
                        }
                    } catch {
                        print("\nError reading error log: \(error.localizedDescription)")
                    }
                } else if fileManager.fileExists(atPath: errorPath) && !errorExists {
                    // File exists again after being deleted
                    do {
                        errorHandle = try FileHandle(forReadingFrom: URL(fileURLWithPath: errorPath))
                        errorPosition = 0
                        errorExists = true
                        print("\n[Error log file reappeared]")
                    } catch {
                        print("\nError reopening error log: \(error.localizedDescription)")
                    }
                }
            }
        }
        
        func run() throws {
            let infoLogPath = "/tmp/lume_daemon.log"
            let errorLogPath = "/tmp/lume_daemon.error.log"
            
            if follow {
                // Use custom tailing implementation for both logs
                tailBothLogs(infoPath: infoLogPath, errorPath: errorLogPath, initialLines: lines ?? 10)
            } else {
                // Regular one-time viewing of logs
                let infoContent = Logs.readLogFile(path: infoLogPath, lines: lines)
                let errorContent = Logs.readLogFile(path: errorLogPath, lines: lines)
                
                print("=== Info Logs ===")
                print(infoContent)
                print("\n=== Error Logs ===")
                print(errorContent)
            }
        }
    }
}
