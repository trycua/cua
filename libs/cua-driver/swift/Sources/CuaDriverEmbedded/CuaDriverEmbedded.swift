import Foundation

@_silgen_name("cua_driver_embedded_new")
private func cua_driver_embedded_new(_ claudeCodeCompat: Bool) -> OpaquePointer?

@_silgen_name("cua_driver_embedded_free")
private func cua_driver_embedded_free(_ driver: OpaquePointer?)

@_silgen_name("cua_driver_embedded_handle_mcp_json")
private func cua_driver_embedded_handle_mcp_json(
    _ driver: OpaquePointer?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>?

@_silgen_name("cua_driver_embedded_string_free")
private func cua_driver_embedded_string_free(_ value: UnsafeMutablePointer<CChar>?)

public enum CuaDriverEmbeddedError: Error, Equatable {
    case failedToCreateDriver
}

/// Swift wrapper for the Rust embedded cua-driver MCP runtime.
///
/// This target intentionally stays JSON-in/JSON-out at the ABI boundary.
/// Apps must link the Rust `libcua_driver_embedded` static library, dylib,
/// or xcframework in addition to depending on this Swift product.
public final class CuaDriverEmbedded {
    private let driver: OpaquePointer
    private let lock = NSLock()

    public init(claudeCodeCompat: Bool = false) throws {
        guard let driver = cua_driver_embedded_new(claudeCodeCompat) else {
            throw CuaDriverEmbeddedError.failedToCreateDriver
        }
        self.driver = driver
    }

    deinit {
        lock.lock()
        cua_driver_embedded_free(driver)
        lock.unlock()
    }

    /// Handle one JSON-RPC MCP request encoded as UTF-8 JSON.
    ///
    /// Returns `nil` for JSON-RPC notifications. GUI apps should call this
    /// from a worker queue rather than the app main thread, because some macOS
    /// tools interact with AppKit or system services.
    public func handleMCPJSON(_ requestJSON: String) -> String? {
        lock.lock()
        defer { lock.unlock() }

        return requestJSON.withCString { requestPointer in
            guard let response = cua_driver_embedded_handle_mcp_json(driver, requestPointer) else {
                return nil
            }
            defer { cua_driver_embedded_string_free(response) }
            return String(cString: response)
        }
    }
}
