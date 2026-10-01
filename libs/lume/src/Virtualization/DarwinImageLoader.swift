import Foundation
import Virtualization

/// Handles loading and validation of macOS restore images (IPSW files).
/// Provides functionality to:
/// - Fetch the latest supported macOS restore image URL
/// - Load and validate image requirements for VM creation
/// - Extract hardware model and auxiliary storage configuration
protocol ImageLoader: Sendable {
    typealias ImageRequirements = DarwinImageLoader.ImageRequirements
    func fetchLatestSupportedURL() async throws -> URL
    func loadImageRequirements(from url: URL) async throws -> ImageRequirements
    func downloadLatestImage() async throws -> Path
}

final class DarwinImageLoader: NSObject, ImageLoader, @unchecked Sendable, URLSessionDownloadDelegate {
    struct ImageRequirements: Sendable {
        let hardwareModel: Data
        let minimumSupportedCPUCount: Int
        let minimumSupportedMemorySize: UInt64
    }
    
    enum ImageError: Error {
        case invalidImage
        case unsupportedConfiguration
        case downloadFailed
    }
    
    private var lastLoggedProgress: Double = 0.0
    private var progressLogger = ProgressLogger()
    private var completionHandler: ((URL?, Error?) -> Void)?
    
    func fetchLatestSupportedURL() async throws -> URL {
        try await withCheckedThrowingContinuation { continuation in
            VZMacOSRestoreImage.fetchLatestSupported { result in
                switch result {
                case .success(let image):
                    continuation.resume(returning: image.url)
                case .failure(let error):
                    continuation.resume(throwing: error)
                }
            }
        }
    }
    
    func loadImageRequirements(from url: URL) async throws -> ImageRequirements {
        let image = try await VZMacOSRestoreImage.image(from: url)
        guard let requirements = image.mostFeaturefulSupportedConfiguration else {
            throw ImageError.unsupportedConfiguration
        }
        
        return ImageRequirements(
            hardwareModel: requirements.hardwareModel.dataRepresentation,
            minimumSupportedCPUCount: requirements.minimumSupportedCPUCount,
            minimumSupportedMemorySize: requirements.minimumSupportedMemorySize
        )
    }
    
    func downloadLatestImage() async throws -> Path {
        let url = try await fetchLatestSupportedURL()
        let tempDir = FileManager.default.temporaryDirectory
        let downloadPath = tempDir.appendingPathComponent("latest.ipsw")
        
        // Reset progress logger state
        progressLogger = ProgressLogger(threshold: 0.01)
        
        // Create a continuation to wait for download completion
        return try await withCheckedThrowingContinuation { continuation in
            let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
            let task = session.downloadTask(with: url)
            
            // Use the delegate method to handle completion
            self.completionHandler = { location, error in
                if let error = error {
                    continuation.resume(throwing: error)
                    return
                }
                
                do {
                    // Remove existing file if it exists
                    if FileManager.default.fileExists(atPath: downloadPath.path) {
                        try FileManager.default.removeItem(at: downloadPath)
                    }
                    
                    try FileManager.default.moveItem(at: location!, to: downloadPath)
                    Logger.info("Download completed and moved to: \(downloadPath.path)")
                    continuation.resume(returning: Path(downloadPath.path))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
            
            task.resume()
        }
    }
    
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        let progress = Double(totalBytesWritten) / Double(totalBytesExpectedToWrite)
        progressLogger.logProgress(current: progress, context: "Downloading IPSW")
    }
    
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        // Call the stored completion handler
        completionHandler?(location, nil)
    }
    
    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        // Call the stored completion handler with an error if it occurred
        if let error = error {
            completionHandler?(nil, error)
        }
    }
}