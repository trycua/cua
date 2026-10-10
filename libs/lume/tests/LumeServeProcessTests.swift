import Darwin
import Foundation
import Testing

@testable import lume

@Test("A lume serve command line yields its API port")
func serveCommandLinesYieldThePort() {
    #expect(LumeServeProcess.apiPort(fromArguments: ["/usr/local/bin/lume", "serve"]) == 7777)
    #expect(
        LumeServeProcess.apiPort(fromArguments: ["lume", "serve", "--port", "7778"]) == 7778)
    #expect(LumeServeProcess.apiPort(fromArguments: ["lume", "serve", "--port=9000"]) == 9000)
}

@Test("Other lume commands are not a server")
func otherCommandsAreNotAServer() {
    #expect(
        LumeServeProcess.apiPort(fromArguments: ["lume", "run", "vm", "--no-display"]) == nil)
    #expect(LumeServeProcess.apiPort(fromArguments: ["lume"]) == nil)
    // The MCP server talks over stdio and has no HTTP API to call.
    #expect(LumeServeProcess.apiPort(fromArguments: ["lume", "serve", "--mcp"]) == nil)
    #expect(LumeServeProcess.apiPort(fromArguments: ["lume", "serve", "--port", "x"]) == nil)
}

@Test("KERN_PROCARGS2 buffers split into arguments")
func procArgsBuffersSplit() {
    var argc = Int32(4)
    var buffer = withUnsafeBytes(of: &argc) { Array($0) }
    buffer += Array("/opt/lume/lume".utf8) + [0, 0, 0, 0]
    for argument in ["lume", "serve", "--port", "7777"] {
        buffer += Array(argument.utf8) + [0]
    }
    buffer += Array("HOME=/Users/x".utf8) + [0]
    #expect(
        LumeServeProcess.parseProcArgs(buffer) == ["lume", "serve", "--port", "7777"])
}

@Test("This test process's own command line is readable and not a server")
func ownCommandLineIsReadable() {
    let arguments = LumeServeProcess.arguments(ofProcess: getpid())
    #expect(arguments?.isEmpty == false)
    #expect(LumeServeProcess.apiPort(ofProcess: getpid()) == nil)
}
