import ArgumentParser
import Foundation
import Virtualization

#if canImport(DiskImageKit)
    import DiskImageKit
#endif

enum StackedDiskError: CustomNSError, LocalizedError {
    case unsupportedHost
    case missingBase(String)
    case missingLayers(String)
    case emptyComponent(String)
    case unknownLayerType(String)

    var errorDescription: String? {
        switch self {
        case .unsupportedHost:
            return
                "Stacked disks require macOS 27 (DiskImageKit) or newer on the host running lume"
        case .missingBase(let spec):
            return "Stacked disk spec is missing a base image: \(spec)"
        case .missingLayers(let spec):
            return "Stacked disk spec needs at least one layer above the base: \(spec)"
        case .emptyComponent(let spec):
            return "Stacked disk spec has an empty path component: \(spec)"
        case .unknownLayerType(let type):
            return "Unknown stacked disk layer type '\(type)' — expected 'cache' or 'overlay'"
        }
    }

    static var errorDomain: String { "StackedDiskError" }

    var errorCode: Int {
        switch self {
        case .unsupportedHost: return 1
        case .missingBase: return 2
        case .missingLayers: return 3
        case .emptyComponent: return 4
        case .unknownLayerType: return 5
        }
    }
}

struct StackedDiskLayer: Equatable {
    static func == (lhs: StackedDiskLayer, rhs: StackedDiskLayer) -> Bool {
        lhs.path.path == rhs.path.path && lhs.kind == rhs.kind
    }

    enum Kind: String {
        case cache
        case overlay
    }

    let path: Path
    let kind: Kind
}

struct StackedDiskSpec: Equatable {
    static func == (lhs: StackedDiskSpec, rhs: StackedDiskSpec) -> Bool {
        lhs.base.path == rhs.base.path && lhs.layers == rhs.layers
    }

    let base: Path
    let layers: [StackedDiskLayer]

    init(argument: String) throws {
        let parts = argument.split(separator: "+", omittingEmptySubsequences: false).map(String.init)

        guard let baseSpec = parts.first, !baseSpec.isEmpty else {
            throw StackedDiskError.missingBase(argument)
        }
        guard parts.count > 1 else {
            throw StackedDiskError.missingLayers(argument)
        }

        base = Path(baseSpec)
        layers = try parts.dropFirst().map { spec in
            let bits = spec.split(separator: "@", maxSplits: 1).map(String.init)
            guard let pathSpec = bits.first, !pathSpec.isEmpty else {
                throw StackedDiskError.emptyComponent(argument)
            }
            guard bits.count > 1 else {
                return StackedDiskLayer(path: Path(pathSpec), kind: .overlay)
            }
            guard let kind = StackedDiskLayer.Kind(rawValue: bits[1]) else {
                throw StackedDiskError.unknownLayerType(bits[1])
            }
            return StackedDiskLayer(path: Path(pathSpec), kind: kind)
        }
    }

    var paths: [Path] { [base] + layers.map(\.path) }

    var writableLayer: Path? { layers.last?.path }

    func storageDeviceConfiguration(
        cachingMode: VZDiskImageCachingMode = .automatic
    ) throws -> VZStorageDeviceConfiguration {
        #if canImport(DiskImageKit)
            guard #available(macOS 27, *) else {
                throw StackedDiskError.unsupportedHost
            }

            var image: DiskImage = try DiskImage(opening: .open(url: base.url, mode: .readOnly))
            for (index, layer) in layers.enumerated() {
                let isTop = index == layers.count - 1
                if layer.path.exists() {
                    image = try image.appending(
                        try DiskImage(
                            opening: .open(
                                url: layer.path.url, mode: isTop ? .readWrite : .readOnly)))
                } else {
                    let type: DiskImage.LayerType = layer.kind == .cache ? .cache : .overlay
                    image = try image.appending(.asifLayer(url: layer.path.url, type: type))
                }
            }

            return VZVirtioBlockDeviceConfiguration(
                attachment: try VZDiskImageStorageDeviceAttachment(
                    diskImage: image,
                    cachingMode: cachingMode,
                    synchronizationMode: VZDiskImageSynchronizationMode.fsync
                )
            )
        #else
            throw StackedDiskError.unsupportedHost
        #endif
    }
}
