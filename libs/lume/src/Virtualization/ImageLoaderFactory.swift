import Foundation

/// Protocol defining a factory for creating image loaders based on the image type
protocol ImageLoaderFactory {
    /// Creates an appropriate ImageLoader based on the image path or type
    func createImageLoader() -> ImageLoader
}

/// Default implementation of ImageLoaderFactory that creates appropriate loaders based on image type
final class DefaultImageLoaderFactory: ImageLoaderFactory {
    func createImageLoader() -> ImageLoader {
        // For now, we only support Darwin images
        // In the future, this can be extended to support other OS types
        // by analyzing the image path or having explicit OS type parameter
        return DarwinImageLoader()
    }
} 