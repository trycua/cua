// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import CuaSpaces
import SwiftUI

/// The New Space wizard: four steps, one sheet, one SDK call at the end.
///
/// Laid out the way a VM-creation assistant is: a step indicator across the
/// top, one decision per page, and Cancel / Back / Continue along the bottom,
/// with Continue becoming Create on the last page.
struct SpaceWizard: View {
    enum Step: Int, CaseIterable, Identifiable {
        case system, resources, options, summary
        var id: Int { rawValue }
        var title: String {
            switch self {
            case .system: return "System"
            case .resources: return "Resources"
            case .options: return "Options"
            case .summary: return "Summary"
            }
        }
    }

    /// What the image dropdown lists: every published image of
    /// `libs/images/sandbox-images.json`, grouped by group label, in file order.
    static var imageOptions: [(group: SandboxImageGroup, images: [SandboxImage])] {
        SandboxImages.pickerSections()
    }

    var onCreate: (SpacePlan) async throws -> Void
    var onCancel: () -> Void

    @State var plan: SpacePlan
    @State var step: Step
    @State private var working = false
    @State private var failure: String?
    @Environment(\.colorScheme) private var scheme
    private var p: KoalaPalette { .resolve(scheme) }

    /// Starts from the user's default location (`cua config set default.on`).
    init(plan: SpacePlan = SpacePlan(placement: SpacePlacement.configuredDefault), step: Step = .system,
         onCreate: @escaping (SpacePlan) async throws -> Void,
         onCancel: @escaping () -> Void) {
        _plan = State(initialValue: plan)
        _step = State(initialValue: step)
        self.onCreate = onCreate
        self.onCancel = onCancel
    }

    var body: some View {
        VStack(spacing: 0) {
            stepIndicator
                .padding(.horizontal, 24).padding(.top, 20).padding(.bottom, 16)
            Divider().overlay(p.border)
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Text(heading).font(.title2.weight(.semibold)).foregroundStyle(p.text)
                    Text(subheading).font(.body).foregroundStyle(p.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    page
                }
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout).foregroundStyle(Color(hex: 0xD93B3B))
                    .padding(.horizontal, 24).padding(.bottom, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            Divider().overlay(p.border)
            buttons.padding(.horizontal, 20).padding(.vertical, 14)
        }
        .frame(width: 640, height: 560)
        .background(p.bg)
    }

    // MARK: Step indicator

    private var stepIndicator: some View {
        HStack(spacing: 0) {
            ForEach(Step.allCases) { s in
                HStack(spacing: 8) {
                    ZStack {
                        Circle().fill(s.rawValue <= step.rawValue ? p.primaryFill : p.surface2)
                        if s.rawValue < step.rawValue {
                            Image(systemName: "checkmark").font(.system(size: 10, weight: .bold))
                                .foregroundStyle(p.onPrimary)
                        } else {
                            Text("\(s.rawValue + 1)").font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(s == step ? p.onPrimary : p.secondary)
                        }
                    }
                    .frame(width: 22, height: 22)
                    Text(s.title)
                        .font(.system(size: 13, weight: s == step ? .semibold : .regular))
                        .lineLimit(1).fixedSize()
                        .foregroundStyle(s == step ? p.text : p.secondary)
                }
                if s != Step.allCases.last {
                    Rectangle().fill(s.rawValue < step.rawValue ? p.text.opacity(0.5) : p.border)
                        .frame(height: 1).frame(maxWidth: .infinity).padding(.horizontal, 10)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Step \(step.rawValue + 1) of \(Step.allCases.count): \(step.title)")
    }

    private var heading: String {
        switch step {
        case .system: return "Choose a system"
        case .resources: return "Resources"
        case .options: return "Name and options"
        case .summary: return "Ready to create"
        }
    }

    private var subheading: String {
        switch step {
        case .system: return "Pick the operating system, the image, and where the Space runs."
        case .resources:
            return plan.placement == .local
                ? "How much of this machine the Space may use."
                : "Cua Cloud sizes the sandbox for the image."
        case .options: return "Spaces are addressed by name, so it must be a DNS label."
        case .summary: return "Check the details. Create makes one SDK call."
        }
    }

    @ViewBuilder private var page: some View {
        switch step {
        case .system: systemPage
        case .resources: resourcesPage
        case .options: optionsPage
        case .summary: summaryPage
        }
    }

    // MARK: 1. System

    private var systemPage: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 12) {
                ForEach(SandboxOS.allCases) { os in
                    tile(symbol: os.symbol, title: os.label,
                         caption: "\(SandboxImages.images(for: os).count) image"
                            + (SandboxImages.images(for: os).count == 1 ? "" : "s"),
                         selected: plan.os == os, enabled: os.isAvailable) {
                        plan.select(os: os)
                    }
                }
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("Image").font(.headline).foregroundStyle(p.text)
                ImagePicker(selection: Binding(get: { plan.imageRef },
                                               set: { plan.select(image: $0) }))
                    .frame(maxWidth: 360, alignment: .leading)
                if let image = plan.image {
                    Text("\(image.summary)  \(image.ref)")
                        .font(.callout).foregroundStyle(p.secondary)
                        .textSelection(.enabled)
                }
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("Where it runs").font(.headline).foregroundStyle(p.text)
                HStack(spacing: 12) {
                    ForEach(SpacePlacement.allCases) { place in
                        let ok = plan.image?.supports(place) ?? false
                        tile(symbol: place == .cloud ? "cloud" : "laptopcomputer",
                             title: place.label,
                             caption: placementCaption(place),
                             selected: plan.placement == place, enabled: ok) {
                            plan.select(placement: place)
                        }
                    }
                }
            }

            DisclosureGroup("Engine") {
                Picker("Engine", selection: Binding(get: { plan.runtime },
                                                    set: { plan.select(runtime: $0) })) {
                    ForEach(plan.runtimeOptions, id: \.self) { runtime in
                        Text(Self.runtimeLabel(runtime)).tag(runtime)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 240, alignment: .leading)
                Text("Automatic picks the safest engine \(plan.placement.label) offers for this image.")
                    .font(.callout).foregroundStyle(p.secondary)
            }
            .foregroundStyle(p.text)
        }
    }

    /// What the engine chooser shows for a runtime.
    static func runtimeLabel(_ runtime: SpaceRuntime) -> String {
        switch runtime {
        case .auto: return "Automatic"
        case .gvisor: return "gVisor container"
        case .runc: return "runc container"
        case .qemu: return "QEMU VM"
        case .lume: return "Lume VM"
        case .kubevirt: return "KubeVirt VM"
        }
    }

    private func placementCaption(_ place: SpacePlacement) -> String {
        guard let image = plan.image else { return "" }
        switch place {
        case .cloud:
            return image.cloud.map { "Metered, \($0)" } ?? "No cloud variant"
        case .local:
            return image.local.map { "Free, \(Self.localRuntimeLabel($0))" } ?? "Not available here"
        }
    }

    static func localRuntimeLabel(_ runtime: String) -> String {
        switch runtime {
        case "container": return "container"
        case "qemu": return "QEMU VM"
        case "lume": return "Lume VM"
        default: return runtime
        }
    }

    private func tile(symbol: String, title: String, caption: String, selected: Bool,
                      enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: symbol).font(.system(size: 22, weight: .regular))
                    .frame(height: 26)
                Text(title).font(.system(size: 13, weight: .semibold))
                Text(caption).font(.system(size: 11)).foregroundStyle(p.secondary)
                    .lineLimit(1)
            }
            .foregroundStyle(p.text)
            .frame(maxWidth: .infinity).padding(.vertical, 14)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(selected ? p.surface2 : p.bg))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(selected ? p.text.opacity(0.7) : p.border,
                              lineWidth: selected ? 1.5 : 1))
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .opacity(enabled ? 1 : 0.4)
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    // MARK: 2. Resources

    @ViewBuilder private var resourcesPage: some View {
        if plan.placement == .local {
            Form {
                Section {
                    LabeledContent("CPU cores") {
                        HStack {
                            Slider(value: Binding(get: { Double(plan.cpus) },
                                                  set: { plan.cpus = Int($0.rounded()) }),
                                   in: Double(SpacePlan.cpuRange.lowerBound)...Double(SpacePlan.cpuRange.upperBound),
                                   step: 1)
                            Stepper("\(plan.cpus)", value: $plan.cpus, in: SpacePlan.cpuRange)
                                .monospacedDigit().frame(width: 60)
                        }
                    }
                    LabeledContent("Memory") {
                        HStack {
                            Slider(value: Binding(get: { Double(plan.memoryGB) },
                                                  set: { plan.memoryGB = Int($0.rounded()) }),
                                   in: Double(SpacePlan.memoryRange.lowerBound)...Double(SpacePlan.memoryRange.upperBound),
                                   step: 1)
                            Stepper("\(plan.memoryGB) GB", value: $plan.memoryGB,
                                    in: SpacePlan.memoryRange)
                                .monospacedDigit().frame(width: 80)
                        }
                    }
                } footer: {
                    Text("This machine has \(ProcessInfo.processInfo.activeProcessorCount) cores and "
                         + "\(ProcessInfo.processInfo.physicalMemory >> 30) GB of memory. "
                         + "Engine: \(Self.runtimeLabel(plan.runtime)).")
                        .foregroundStyle(p.secondary)
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .frame(height: 190)
        } else {
            let runtime = plan.image?.cloud ?? ""
            VStack(alignment: .leading, spacing: 12) {
                runtimeRow("gvisor", title: "gVisor container",
                           detail: "A sandboxed container rootfs. Starts in seconds, small memory "
                               + "footprint.", current: runtime == "gvisor")
                runtimeRow("kubevirt", title: "KubeVirt VM",
                           detail: "A full virtual machine booted from a disk image. Slower to "
                               + "start, lowest input latency, runs Windows.",
                           current: runtime == "kubevirt")
                Text("The runtime comes from the image: \(plan.image?.name ?? "this image") "
                     + "runs on \(runtime == "gvisor" ? "gVisor" : "KubeVirt"). "
                     + "Cua Cloud Spaces are metered.")
                    .font(.callout).foregroundStyle(p.secondary)
            }
        }
    }

    private func runtimeRow(_ id: String, title: String, detail: String, current: Bool) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: current ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 16)).foregroundStyle(current ? p.text : p.secondary)
            VStack(alignment: .leading, spacing: 3) {
                Text("\(title)  (\(id))").font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(p.text)
                Text(detail).font(.callout).foregroundStyle(p.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .fill(current ? p.surface2 : p.bg))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(p.border))
        .opacity(current ? 1 : 0.6)
    }

    // MARK: 3. Options

    private var optionsPage: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Name").font(.headline).foregroundStyle(p.text)
                TextField("my-space", text: $plan.name)
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 320)
                    .onChange(of: plan.name) { _, new in
                        let lowered = new.lowercased()
                        if lowered != new { plan.name = lowered }
                    }
                if let e = plan.nameError, !plan.name.isEmpty {
                    Text(e).font(.callout).foregroundStyle(Color(hex: 0xD93B3B))
                }
            }
            Toggle("Open the desktop when ready", isOn: $plan.openWhenReady)
                .toggleStyle(.checkbox)
            if let warning = plan.spacesdWarning {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(Color(hex: 0xE0AE09))
                    Text(warning).font(.callout).foregroundStyle(p.text)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(12)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color(hex: 0xE0AE09).opacity(0.12)))
            }
        }
    }

    // MARK: 4. Summary

    private var summaryPage: some View {
        let rows: [(String, String)] = {
            var r: [(String, String)] = [
                ("Name", plan.name),
                ("System", plan.os.label),
                ("Image", "\(plan.image?.name ?? "") (\(plan.imageRef))"),
                ("Runs on", plan.placement.label),
            ]
            r.append(("Engine", Self.runtimeLabel(plan.runtime)))
            if plan.placement == .local {
                r.append(("CPU cores", "\(plan.cpus)"))
                r.append(("Memory", "\(plan.memoryGB) GB"))
            }
            r.append(("cua-spacesd", plan.image?.spacesd == true ? "Yes" : "No"))
            r.append(("Open when ready", plan.openWhenReady ? "Yes" : "No"))
            return r
        }()
        return VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { i, row in
                HStack(alignment: .firstTextBaseline) {
                    Text(row.0).foregroundStyle(p.secondary).frame(width: 130, alignment: .leading)
                    Text(row.1).foregroundStyle(p.text).textSelection(.enabled)
                    Spacer(minLength: 0)
                }
                .font(.system(size: 13))
                .padding(.horizontal, 14).padding(.vertical, 8)
                if i < rows.count - 1 { Divider().overlay(p.border) }
            }
        }
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(p.surface2))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(p.border))
    }

    // MARK: Buttons

    private var canContinue: Bool {
        switch step {
        case .system: return plan.systemError == nil
        case .resources: return true
        case .options: return plan.nameError == nil
        case .summary: return plan.isValid && !working
        }
    }

    private var buttons: some View {
        HStack(spacing: 10) {
            Button("Cancel", action: onCancel).keyboardShortcut(.cancelAction)
            Spacer()
            if working { ProgressView().controlSize(.small) }
            Button("Back") {
                failure = nil
                if let prev = Step(rawValue: step.rawValue - 1) { step = prev }
            }
            .disabled(step == .system || working)
            if step == .summary {
                Button("Create") { create() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canContinue)
            } else {
                Button("Continue") {
                    if step == .resources, plan.name.isEmpty {
                        plan.name = SpacePlan.suggestedName(
                            from: "\(plan.image?.name ?? plan.os.label) \(plan.placement == .local ? "local" : "cloud")")
                    }
                    if let next = Step(rawValue: step.rawValue + 1) { step = next }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!canContinue)
            }
        }
        .controlSize(.large)
    }

    private func create() {
        working = true
        failure = nil
        Task {
            do { try await onCreate(plan) } catch { failure = "\(error)" }
            working = false
        }
    }
}

/// The sandbox image dropdown: a native `Picker` over the generated list,
/// sectioned by group label.
struct ImagePicker: View {
    @Binding var selection: String

    var body: some View {
        Picker("Image", selection: $selection) {
            ForEach(SpaceWizard.imageOptions, id: \.group.id) { section in
                Section(section.group.label) {
                    ForEach(section.images) { image in
                        Text("\(image.name)  \(SandboxOS(rawValue: image.os)?.label ?? image.os)")
                            .tag(image.ref)
                    }
                }
            }
        }
        .pickerStyle(.menu)
        .labelsHidden()
    }
}
#endif
