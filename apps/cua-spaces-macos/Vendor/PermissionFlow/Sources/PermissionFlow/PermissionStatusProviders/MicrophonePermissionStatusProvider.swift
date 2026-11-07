#if os(macOS)
import AVFoundation
import Foundation

@available(macOS 13.0, *)
public struct MicrophonePermissionStatusProvider: PermissionStatusProviding {
    public var capability: PermissionStatusCapability { .preflightSupported }

    public func authorizationState() -> PermissionAuthorizationState {
        Self.authorizationState(for: AVCaptureDevice.authorizationStatus(for: .audio))
    }

    public func requestAuthorization(
        completion: @escaping @Sendable (PermissionAuthorizationState) -> Void
    ) {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                completion(granted ? .granted : .notGranted)
            }
        case let status:
            completion(Self.authorizationState(for: status))
        }
    }

    public init() {}

    private static func authorizationState(
        for status: AVAuthorizationStatus
    ) -> PermissionAuthorizationState {
        switch status {
        case .authorized:
            .granted
        case .denied, .restricted, .notDetermined:
            .notGranted
        @unknown default:
            .unknown
        }
    }
}
#endif
