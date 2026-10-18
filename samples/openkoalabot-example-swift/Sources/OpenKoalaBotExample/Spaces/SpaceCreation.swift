// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpaces
import Foundation

// MARK: - The image list

/// One entry of `libs/images/sandbox-images.json`.
///
/// The list itself is generated (`SandboxImages.generated.swift`, from
/// `scripts/gen-images.mjs`), so the picker offers exactly what the docs and
/// the other apps offer.
struct SandboxImage: Identifiable, Hashable {
    var ref: String
    var group: String
    /// `linux`, `windows` or `macos`.
    var os: String
    var name: String
    /// `container` or `vm`.
    var variant: String
    var summary: String
    /// Whether the image runs cua-spacesd. Without it a Space has no
    /// capabilities beyond its declared services.
    var spacesd: Bool
    /// How the image runs locally (`container`, `qemu`, `lume`), `nil` when
    /// it cannot run on this machine.
    var local: String?
    /// The cloud engine (`gvisor`, `kubevirt`), `nil` when the image has no
    /// cloud variant.
    var cloud: String?
    /// `slim`, `full` (the default) or `xcode`; `nil` outside the
    /// canonical images.
    var tier: String? = nil

    var id: String { ref }

    func supports(_ placement: SpacePlacement) -> Bool {
        switch placement {
        case .cloud: return cloud != nil
        case .local: return local != nil
        }
    }

    /// What kind of machine the image is (`variant` in the catalog).
    var kind: SpaceKind { variant == "vm" ? .vm : .container }

    /// The engines this image can run on in `placement`, `auto` first. Only
    /// what the catalog says the image runs, so the wizard never offers a
    /// combination the image cannot run. Empty when it cannot run there.
    func runtimes(on placement: SpacePlacement) -> [SpaceRuntime] {
        switch placement {
        case .local:
            switch local {
            case "container": return SpaceRuntime.offered(on: .local, kind: .container)
            case "qemu": return [.auto, .qemu]
            case "lume": return [.auto, .lume]
            default: return []
            }
        case .cloud:
            guard let engine = cloud.flatMap(SpaceRuntime.init(rawValue:)) else { return [] }
            return [.auto, engine]
        }
    }
}

struct SandboxImageGroup: Identifiable, Hashable {
    var id: String
    var label: String
}

enum SandboxImages {
    /// The published images of one operating system, in file order.
    static func images(for os: SandboxOS) -> [SandboxImage] {
        published.filter { $0.os == os.rawValue }
    }

    static func image(_ ref: String) -> SandboxImage? {
        published.first { $0.ref == ref }
    }

    /// What the picker shows: each group label with its images, in file order.
    /// `os` narrows it to one system; `nil` is every published image.
    static func pickerSections(for os: SandboxOS? = nil) -> [(group: SandboxImageGroup, images: [SandboxImage])] {
        groups.compactMap { g in
            let rows = published.filter { $0.group == g.id && (os == nil || $0.os == os!.rawValue) }
            return rows.isEmpty ? nil : (g, rows)
        }
    }
}

enum SandboxOS: String, CaseIterable, Identifiable {
    case linux, windows, macos
    var id: String { rawValue }
    var label: String {
        switch self {
        case .linux: return "Linux"
        case .windows: return "Windows"
        case .macos: return "macOS"
        }
    }
    var symbol: String {
        switch self {
        case .linux: return "terminal"
        case .windows: return "pc"
        case .macos: return "macwindow"
        }
    }
    /// An OS tile is only offered when at least one published image runs it.
    var isAvailable: Bool { !SandboxImages.images(for: self).isEmpty }
}

/// Where a Space runs: the SDK's `on`.
enum SpacePlacement: String, CaseIterable, Identifiable {
    case local, cloud
    var id: String { rawValue }
    var label: String { self == .cloud ? "Cua Cloud" : "This machine" }
    var location: SpaceLocation { self == .cloud ? .cloud : .local }

    /// The user's default location (`cua config set default.on`,
    /// `CUA_DEFAULT_ON`, else `local`), which the wizard starts from.
    static var configuredDefault: SpacePlacement {
        (try? CuaSDK.configGet(key: "default.on").value) == "cloud" ? .cloud : .local
    }
}

// MARK: - The plan

/// Everything the New Space wizard collects, and the one SDK call it maps to.
struct SpacePlan: Equatable {
    var os: SandboxOS = .linux
    var imageRef: String = SandboxImages.images(for: .linux).first?.ref ?? ""
    var placement: SpacePlacement = .local
    /// `auto` unless the advanced chooser picked an engine.
    var runtime: SpaceRuntime = .auto
    var cpus: Int = SpacePlan.defaultCPUs
    var memoryGB: Int = SpacePlan.defaultMemoryGB
    var name: String = ""
    var openWhenReady: Bool = true

    static let defaultCPUs = 2
    static let defaultMemoryGB = 4
    static let cpuRange = 1...max(2, ProcessInfo.processInfo.activeProcessorCount)
    static let memoryRange = 2...max(4, Int(ProcessInfo.processInfo.physicalMemory >> 30) / 2)

    var image: SandboxImage? { SandboxImages.image(imageRef) }

    /// Pick an OS: the image follows (first of that OS), and the placement
    /// moves to one the image supports.
    mutating func select(os: SandboxOS) {
        self.os = os
        if image?.os != os.rawValue, let first = SandboxImages.images(for: os).first {
            select(image: first.ref)
        }
    }

    mutating func select(image ref: String) {
        imageRef = ref
        guard let image else { return }
        os = SandboxOS(rawValue: image.os) ?? os
        if !image.supports(placement) {
            placement = image.supports(.cloud) ? .cloud : .local
        }
        keepRuntimeValid()
    }

    mutating func select(placement: SpacePlacement) {
        guard image?.supports(placement) ?? false else { return }
        self.placement = placement
        keepRuntimeValid()
    }

    /// The engines the advanced chooser offers: what this image runs where
    /// the plan says, `auto` first.
    var runtimeOptions: [SpaceRuntime] { image?.runtimes(on: placement) ?? [] }

    mutating func select(runtime: SpaceRuntime) {
        guard runtimeOptions.contains(runtime) else { return }
        self.runtime = runtime
    }

    private mutating func keepRuntimeValid() {
        if !runtimeOptions.contains(runtime) { runtime = .auto }
    }

    // MARK: Validation

    /// A Space name is a DNS label: 1 to 63 of `a-z`, `0-9` and `-`, not
    /// starting or ending with `-`.
    static func isDNSLabel(_ s: String) -> Bool {
        guard (1...63).contains(s.count), s.first != "-", s.last != "-" else { return false }
        return s.unicodeScalars.allSatisfy {
            ("a"..."z").contains($0) || ("0"..."9").contains($0) || $0 == "-"
        }
    }

    /// Lowercase, spaces to hyphens, everything else dropped.
    static func suggestedName(from s: String) -> String {
        var out = ""
        for ch in s.lowercased() {
            if ch.isASCII, ch.isLetter || ch.isNumber { out.append(ch) }
            else if ch == " " || ch == "-" || ch == "_" || ch == "." {
                if !out.isEmpty, out.last != "-" { out.append("-") }
            }
        }
        while out.last == "-" { out.removeLast() }
        return String(out.prefix(63))
    }

    var nameError: String? {
        if name.isEmpty { return "Give the Space a name." }
        return Self.isDNSLabel(name)
            ? nil
            : "Use lowercase letters, digits and hyphens (a DNS label, at most 63 characters)."
    }

    var systemError: String? {
        guard let image else { return "Choose an image." }
        if image.os != os.rawValue { return "\(image.name) is not a \(os.label) image." }
        if !image.supports(placement) {
            return placement == .cloud
                ? "\(image.name) has no Cua Cloud variant. Run it on this machine."
                : "\(image.name) cannot run on this machine. Run it in Cua Cloud."
        }
        if !runtimeOptions.contains(runtime) {
            return "\(image.name) does not run on \(runtime.rawValue) in \(placement.label)."
        }
        return nil
    }

    var isValid: Bool { systemError == nil && nameError == nil }

    /// Shown on the Options step when the image has no cua-spacesd.
    var spacesdWarning: String? {
        guard let image, !image.spacesd else { return nil }
        return "\(image.name) does not run cua-spacesd yet, so the Space starts with no "
            + "capabilities: no screen, shell or files. Bots cannot work in it."
    }

    /// The call `Create` makes. The image reference goes as is: the SDK
    /// reads the image's manifest to pick the variant and, with `auto`, the
    /// engine. CPUs and memory apply to local Spaces only.
    var request: SpaceCreateRequest? {
        guard isValid, let image else { return nil }
        return SpaceCreateRequest(
            on: placement.location, kind: image.kind, runtime: runtime,
            image: image.ref, name: name,
            cpus: placement == .local ? cpus : nil,
            memoryMB: placement == .local ? memoryGB * 1024 : nil,
            spacesd: image.spacesd)
    }
}

/// One Space creation, in SDK terms: `Spaces.create(options:)`
/// (`create_space`). `on: .local` is free; `on: .cloud` is metered.
struct SpaceCreateRequest: Equatable {
    var on: SpaceLocation
    var kind: SpaceKind
    var runtime: SpaceRuntime
    var image: String
    var name: String
    var cpus: Int?
    var memoryMB: Int?
    var spacesd: Bool
}

/// `Create`, minus the UI: validate the plan, then make exactly one SDK call.
enum SpaceCreationFlow {
    static func create(_ plan: SpacePlan, with creator: SpaceCreating) async throws -> String {
        guard let request = plan.request else {
            throw SpaceCreationError.invalidPlan(plan.systemError ?? plan.nameError ?? "invalid plan")
        }
        return try await creator.createSpace(request)
    }
}

/// The part of a Spaces client that can create a Space. Separate from
/// `SpacesClient` so the app's everyday path cannot create one by accident.
protocol SpaceCreating: AnyObject {
    /// Create the Space and return its id once the SDK reports it.
    func createSpace(_ request: SpaceCreateRequest) async throws -> String
}

enum SpaceCreationError: Error, CustomStringConvertible {
    case invalidPlan(String)
    case noSpacesRuntime
    case noSpaceID

    var description: String {
        switch self {
        case .invalidPlan(let why): return why
        case .noSpacesRuntime:
            return "Creating a Space needs the cua SDK. The app is offline (demo client)."
        case .noSpaceID: return "The SDK did not return a Space id."
        }
    }
}
