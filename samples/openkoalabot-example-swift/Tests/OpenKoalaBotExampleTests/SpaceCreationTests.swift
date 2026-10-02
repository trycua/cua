// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The image list, the New Space wizard's plan, and the one SDK call it makes.
@Suite final class SandboxImageListTests {

    /// `libs/images/sandbox-images.json`, found from this file so the test
    /// reads the real shared list and not a copy.
    static let jsonURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()   // OpenKoalaBotExampleTests
        .deletingLastPathComponent()   // Tests
        .deletingLastPathComponent()   // the package
        .deletingLastPathComponent()   // samples
        .deletingLastPathComponent()   // the repo
        .appendingPathComponent("libs/images/sandbox-images.json")

    struct JSONImage: Decodable {
        var ref, group, os, name, variant, summary: String
        var spacesd: Bool
        var local: String?
        var cloud: String?
        var published: Bool
    }
    struct JSONGroup: Decodable { var id, label: String }
    struct JSONFile: Decodable { var groups: [JSONGroup]; var images: [JSONImage] }

    func load() throws -> JSONFile {
        try JSONDecoder().decode(JSONFile.self, from: Data(contentsOf: Self.jsonURL))
    }

    @Test func testTheGeneratedListIsExactlyThePublishedJSONEntriesInOrder() throws {
        let json = try load()
        let published = json.images.filter(\.published)
        XCTAssertFalse(published.isEmpty)
        XCTAssertEqual(SandboxImages.published.map(\.ref), published.map(\.ref))
        XCTAssertEqual(SandboxImages.published.map(\.name), published.map(\.name))
        XCTAssertEqual(SandboxImages.published.map(\.group), published.map(\.group))
        XCTAssertEqual(SandboxImages.published.map(\.os), published.map(\.os))
        XCTAssertEqual(SandboxImages.published.map(\.variant), published.map(\.variant))
        XCTAssertEqual(SandboxImages.published.map(\.summary), published.map(\.summary))
        XCTAssertEqual(SandboxImages.published.map(\.spacesd), published.map(\.spacesd))
        XCTAssertEqual(SandboxImages.published.map { $0.local ?? "-" }, published.map { $0.local ?? "-" })
        XCTAssertEqual(SandboxImages.published.map { $0.cloud ?? "-" }, published.map { $0.cloud ?? "-" })
    }

    @Test func testUnpublishedEntriesAreNeverListed() throws {
        let hidden = Set(try load().images.filter { !$0.published }.map(\.ref))
        XCTAssertTrue(SandboxImages.published.allSatisfy { !hidden.contains($0.ref) })
    }

    @Test func testGroupLabelsComeFromTheJSON() throws {
        let json = try load()
        let used = Set(json.images.filter(\.published).map(\.group))
        let expected = json.groups.filter { used.contains($0.id) }
        XCTAssertEqual(SandboxImages.groups.map(\.id), expected.map(\.id))
        XCTAssertEqual(SandboxImages.groups.map(\.label), expected.map(\.label))
    }

    @Test func testThePickerOffersExactlyThePublishedList() throws {
        let json = try load()
        let options = SpaceWizard.imageOptions
        XCTAssertEqual(options.flatMap { $0.images.map(\.ref) },
                       json.images.filter(\.published).map(\.ref))
        let labels = Dictionary(uniqueKeysWithValues: json.groups.map { ($0.id, $0.label) })
        for section in options {
            XCTAssertEqual(section.group.label, labels[section.group.id])
            XCTAssertTrue(section.images.allSatisfy { $0.group == section.group.id })
        }
    }
}

/// Records what `Create` asked the SDK for.
final class FakeSpaceCreator: SpaceCreating {
    var requests: [SpaceCreateRequest] = []
    var result = "local:fake-space"
    func createSpace(_ request: SpaceCreateRequest) async throws -> String {
        requests.append(request)
        return result
    }
}

@Suite final class SpacePlanTests {

    private func image(local: String) -> SandboxImage? {
        SandboxImages.published.first { $0.local == local }
    }

    @Test func testCloudPlanCreatesInTheCloudAndLetsTheImageDecideTheEngine() async throws {
        var plan = SpacePlan()
        let img = try #require(SandboxImages.published.first { $0.cloud == "gvisor" })
        plan.select(image: img.ref)
        plan.select(placement: .cloud)
        plan.name = "my-space"
        let fake = FakeSpaceCreator()
        let id = try await SpaceCreationFlow.create(plan, with: fake)
        XCTAssertEqual(id, "local:fake-space")
        XCTAssertEqual(fake.requests, [SpaceCreateRequest(
            on: .cloud, kind: img.kind, runtime: .auto, image: img.ref, name: "my-space",
            cpus: nil, memoryMB: nil, spacesd: img.spacesd)])
    }

    @Test func testTheEngineChooserOffersOnlyWhatTheImageRunsThere() throws {
        var plan = SpacePlan()
        let vm = try #require(SandboxImages.published.first { $0.cloud == "kubevirt" })
        plan.select(image: vm.ref)
        plan.select(placement: .cloud)
        XCTAssertEqual(plan.runtimeOptions, [.auto, .kubevirt])
        plan.select(runtime: .gvisor)
        XCTAssertEqual(plan.runtime, .auto, "gVisor cannot run a VM image")
        plan.select(runtime: .kubevirt)
        plan.name = "vm"
        XCTAssertEqual(plan.request?.runtime, .kubevirt)
        XCTAssertEqual(plan.request?.kind, .vm)

        // Moving to a location that does not offer the engine resets it.
        plan.select(placement: .local)
        XCTAssertEqual(plan.runtime, .auto)
        XCTAssertFalse(plan.runtimeOptions.contains(.kubevirt))

        if let container = SandboxImages.published.first(where: { $0.local == "container" }) {
            plan.select(image: container.ref)
            plan.select(placement: .local)
            XCTAssertEqual(plan.runtimeOptions, [.auto, .gvisor, .runc])
            XCTAssertFalse(plan.runtimeOptions.contains(.qemu))
        }
    }

    @Test func testLocalPlansSendTheImageAsIsWithCpusAndMemory() async throws {
        for local in ["container", "qemu", "lume"] {
            guard let img = image(local: local) else { continue }
            var plan = SpacePlan()
            plan.select(image: img.ref)
            plan.select(placement: .local)
            plan.cpus = 3
            plan.memoryGB = 6
            plan.name = "box-\(local)"
            let fake = FakeSpaceCreator()
            _ = try await SpaceCreationFlow.create(plan, with: fake)
            XCTAssertEqual(fake.requests, [SpaceCreateRequest(
                on: .local, kind: img.kind, runtime: .auto, image: img.ref, name: "box-\(local)",
                cpus: 3, memoryMB: 6 * 1024, spacesd: img.spacesd)])
        }
    }

    @Test func testAnImageWithNoCloudVariantCannotBeCreatedInTheCloud() throws {
        guard let mac = SandboxImages.published.first(where: { $0.cloud == nil }) else { return }
        var plan = SpacePlan(placement: .cloud)
        plan.select(image: mac.ref)
        XCTAssertEqual(plan.placement, .local)
        plan.select(placement: .cloud)
        XCTAssertEqual(plan.placement, .local, "a cloud:null image must not switch to Cua Cloud")
        plan.placement = .cloud
        plan.name = "mac"
        XCTAssertNotNil(plan.systemError)
        XCTAssertNil(plan.request)
    }

    @Test func testLabelsAreCuaCloudAndThisMachine() {
        XCTAssertEqual(SpacePlacement.cloud.label, "Cua Cloud")
        XCTAssertEqual(SpacePlacement.local.label, "This machine")
        XCTAssertEqual(SpacePlacement.cloud.location, .cloud)
        XCTAssertEqual(SpacePlacement.local.location, .local)
    }

    @Test func testPickingAnOSPicksItsFirstImage() {
        var plan = SpacePlan()
        for os in SandboxOS.allCases where os.isAvailable {
            plan.select(os: os)
            XCTAssertEqual(plan.imageRef, SandboxImages.images(for: os).first?.ref)
            XCTAssertTrue(plan.image?.supports(plan.placement) ?? false)
        }
    }

    @Test func testAnInvalidNameMakesNoCall() async {
        var plan = SpacePlan()
        plan.name = "Not A Label"
        let fake = FakeSpaceCreator()
        await XCTAssertThrowsErrorAsync(try await SpaceCreationFlow.create(plan, with: fake))
        XCTAssertTrue(fake.requests.isEmpty)
    }

    @Test func testDNSLabels() {
        XCTAssertTrue(SpacePlan.isDNSLabel("a"))
        XCTAssertTrue(SpacePlan.isDNSLabel("ubuntu-24-04"))
        XCTAssertFalse(SpacePlan.isDNSLabel(""))
        XCTAssertFalse(SpacePlan.isDNSLabel("-a"))
        XCTAssertFalse(SpacePlan.isDNSLabel("a-"))
        XCTAssertFalse(SpacePlan.isDNSLabel("Upper"))
        XCTAssertFalse(SpacePlan.isDNSLabel("dots.not.ok"))
        XCTAssertFalse(SpacePlan.isDNSLabel(String(repeating: "a", count: 64)))
        XCTAssertEqual(SpacePlan.suggestedName(from: "Ubuntu 24.04 VM local"), "ubuntu-24-04-vm-local")
    }

    @Test func testImagesWithoutSpacesdWarn() {
        for img in SandboxImages.published {
            var plan = SpacePlan()
            plan.select(image: img.ref)
            XCTAssertEqual(plan.spacesdWarning != nil, !img.spacesd)
        }
    }
}

private func XCTAssertThrowsErrorAsync<T>(_ body: @autoclosure () async throws -> T,
                                          sourceLocation: SourceLocation = #_sourceLocation) async {
    do {
        _ = try await body()
        Issue.record("expected an error", sourceLocation: sourceLocation)
    } catch {}
}

/// The window is named for the app, and every icon-only control has a name.
@Suite final class ShellNamingTests {
    @Test func testTheWindowIsCalledOpenKoalaBots() {
        XCTAssertEqual(AppIdentity.name, "OpenKoalaBots")
        XCTAssertEqual(SignInView.title, "OpenKoalaBots")
    }

    @Test func testEveryIconOnlyControlHasADistinctLabel() {
        XCTAssertTrue(ShellLabels.all.allSatisfy {
            !$0.trimmingCharacters(in: .whitespaces).isEmpty
        })
        XCTAssertEqual(Set(ShellLabels.all).count, ShellLabels.all.count)
    }

    @Test func testTheNoStreamStateSaysSo() {
        XCTAssertTrue(ShellLabels.noLiveScreenBody.contains("not attached"))
    }
}
