#if os(macOS)
import Foundation

@available(macOS 13.0, *)
public struct FullDiskAccessPermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .preflightSupported }
    
    public func authorizationState() -> PermissionAuthorizationState {
        // Test Full Disk Access by trying to access protected system directories
        // The most reliable test is trying to access other users' home directories or system files
        
        // Method 1: Try to access the root user's home directory
        let rootHome = "/var/root"
        if FileManager.default.isReadableFile(atPath: rootHome) {
            return .granted
        }
        
        // Method 2: Try to access system configuration files
        let systemConfigPaths = [
            "/private/etc/sudoers",
            "/Library/Application Support/com.apple.TCC/TCC.db",
            "/private/var/db/SystemPolicy"
        ]
        
        for path in systemConfigPaths {
            if FileManager.default.isReadableFile(atPath: path) {
                return .granted
            }
        }
        
        // Method 3: Try to access other users' directories if they exist
        do {
            let usersDir = "/Users"
            let usernames = try FileManager.default.contentsOfDirectory(atPath: usersDir)
            let currentUser = NSUserName()
            
            for username in usernames {
                // Skip current user, Shared, and system folders
                if username != currentUser && username != "Shared" && !username.hasPrefix(".") {
                    let otherUserHome = "/Users/\(username)"
                    // Try to access another user's home directory
                    if FileManager.default.isReadableFile(atPath: otherUserHome) {
                        do {
                            let _ = try FileManager.default.contentsOfDirectory(atPath: otherUserHome)
                            return .granted
                        } catch {
                            // If we can see the directory but can't list it, that's normal without FDA
                            continue
                        }
                    }
                }
            }
        } catch {
            // If we can't even list /Users, something is wrong
        }
        
        // Method 4: Try to access current user's protected directories that require FDA
        let protectedUserPaths = [
            "/Users/\(NSUserName())/Library/Mail",
            "/Users/\(NSUserName())/Library/Safari/Databases",
            "/Users/\(NSUserName())/Library/Messages",
            "/Users/\(NSUserName())/Library/Application Support/com.apple.sharedfilelist"
        ]
        
        for path in protectedUserPaths {
            if FileManager.default.fileExists(atPath: path) {
                do {
                    let _ = try FileManager.default.contentsOfDirectory(atPath: path)
                    return .granted
                } catch {
                    // If directory exists but we can't read it, FDA might not be granted
                    continue
                }
            }
        }
        
        // If none of the tests indicate FDA is granted, assume it's not
        return .notGranted
    }
    
    public init() {}
}
#endif