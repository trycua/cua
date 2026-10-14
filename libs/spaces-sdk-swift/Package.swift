// swift-tools-version: 5.9
import PackageDescription

// A thin Swift overlay on the generated cua SDK (`libs/cua/swift`, product
// `Cua`). Every Spaces call goes through `CuaSDK.Spaces` / `CuaSDK.Space` —
// embedded (`Cua.embedded`) or through a running `cua daemon`
// (`Cua.connect`) — so there is one implementation of every tool (the Rust
// `cua-spaces` crate). This package keeps the app-shaped API OpenKoalaBots was
// built against: typed ids, run snapshots, rosters and consent-typed
// teleport. The SwiftUI stream views fed by `SpaceStreamSession` frames
// (`CuaSpacesStreaming`) are source-available (FSL-1.1-MIT) and live in
// `libs/spaces-app-swift`.
let package = Package(
    name: "CuaSpacesSDK",
    platforms: [.macOS(.v14)],
    products: [
        // The protocol surface: Spaces, agent runs, rosters, files.
        .library(name: "CuaSpaces", targets: ["CuaSpaces"]),
        // Terminal scrollback -> structured UI, and importing it is the act
        // of consent. FRICTION.md §19/§20 say every app otherwise writes this
        // parser and they all guess differently. Every event it produces says
        // `isInferred == true`; `CuaSpaces` alone never mints a `.question` or
        // a `.toolUse` the agent did not offer. Optional and dependency-free,
        // so an app that only needs the protocol surface pays for no VT
        // emulator.
        .library(name: "CuaSpacesTranscript", targets: ["CuaSpacesTranscript"]),
    ],
    dependencies: [
        .package(name: "Cua", path: "../cua/swift"),
    ],
    targets: [
        .target(name: "CuaSpaces", dependencies: [.product(name: "Cua", package: "Cua")],
                path: "Sources/CuaSpaces"),
        // The VT emulator and the parser take terminal bytes and give back a
        // document; ScrollbackClassifier turns that into CuaSpaces events, so
        // this target does depend on CuaSpaces. The protocol library never
        // grows a VT emulator, because this one is opt-in.
        .target(name: "CuaSpacesTranscript", dependencies: ["CuaSpaces"],
                path: "Sources/CuaSpacesTranscript",
                exclude: ["README.md"]),
        .executableTarget(name: "cast-render", dependencies: ["CuaSpacesTranscript"],
                          path: "Sources/cast-render"),
        .testTarget(name: "CuaSpacesTranscriptTests",
                    dependencies: ["CuaSpacesTranscript"],
                    path: "Tests/CuaSpacesTranscriptTests",
                    resources: [.copy("Goldens")]),
        .testTarget(name: "CuaSpacesTests",
                    dependencies: ["CuaSpaces", "CuaSpacesTranscript",
                                   .product(name: "Cua", package: "Cua")],
                    path: "Tests/CuaSpacesTests"),
    ]
)
