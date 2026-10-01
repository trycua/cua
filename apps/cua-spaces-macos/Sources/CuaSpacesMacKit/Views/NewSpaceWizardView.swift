// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// New Space, as a step-by-step assistant: System, Resources, Options,
/// Summary. Every tile, rule, label and the plan are the core's
/// (`appWizardView`); this sheet renders them.
public struct NewSpaceWizardView: View {
    @Bindable var wizard: WizardModel
    let onCreate: (AppCreatePlan) -> Void
    let onAdd: (String, String?, String?) async throws -> Void
    let onCancel: () -> Void
    /// "Connect a cloud": the sheet it opens (nil: the link does not show).
    let cloud: CloudModel?

    public init(wizard: WizardModel, onCreate: @escaping (AppCreatePlan) -> Void,
                onAdd: @escaping (String, String?, String?) async throws -> Void,
                onCancel: @escaping () -> Void, cloud: CloudModel? = nil) {
        self.wizard = wizard
        self.onCreate = onCreate
        self.onAdd = onAdd
        self.onCancel = onCancel
        self.cloud = cloud
    }

    public var body: some View {
        let v = wizard.view
        VStack(spacing: 0) {
            header(v)
            Divider()
            Group {
                if v.mode == .address {
                    address(v)
                } else {
                    switch v.step {
                    case 0: system(v)
                    case 1: resources(v)
                    case 2: options(v)
                    default: summary(v)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            Divider()
            footer(v).padding(14)
        }
        .frame(width: 560, height: 520)
        .sheet(isPresented: Binding(get: { cloud?.showing ?? false },
                                    set: { if !$0 { cloud?.showing = false } })) {
            if let cloud { ConnectCloudSheet(cloud: cloud) }
        }
    }

    private func header(_ v: AppWizardView) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(v.title).font(.title2.weight(.semibold))
            Spacer()
            if v.mode == .create {
                HStack(spacing: 6) {
                    ForEach(Array(v.steps.enumerated()), id: \.offset) { _, step in
                        Text(step.label)
                            .font(.callout)
                            .foregroundStyle(step.state == .current ? .primary : .secondary)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("wizard-steps")
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
    }

    /// The System step: the core's fields in its order. Everything before
    /// the first advanced field, then "Advanced", then the rest.
    private func system(_ v: AppWizardView) -> some View {
        let main = Array(v.fields.prefix { !$0.advanced })
        let advanced = v.fields.filter(\.advanced)
        let rest = v.fields.drop { !$0.advanced }.filter { !$0.advanced }
        return Form {
            Section {
                ForEach(main, id: \.id) { systemRow($0, v) }
            }
            Section {
                if !advanced.isEmpty {
                    DisclosureGroup(v.labels.advanced, isExpanded: Binding(
                        get: { v.advanced }, set: { _ in wizard.send(.toggleAdvanced) })) {
                        ForEach(advanced, id: \.id) { systemRow($0, v) }
                    }
                }
                ForEach(rest, id: \.id) { systemRow($0, v) }
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder private func systemRow(_ field: AppWizardField, _ v: AppWizardView) -> some View {
        switch field.id {
        case "os":
            Picker(field.label, selection: Binding(
                get: { v.image.os },
                set: { wizard.send(.chooseOs(os: $0)) })) {
                ForEach(v.osTiles, id: \.id) { tile in
                    Text(tile.title).tag(osOf(tile.id)).help(tile.detail)
                }
            }
            .pickerStyle(.segmented)
        case "image":
            ImageField(field: field, view: v.imageField, send: wizard.send)
        case "placement":
            // One menu of every machine that can host the Space, a divider
            // between This Mac, your machines and your clouds.
            Picker(field.label, selection: Binding(
                get: { v.placementId },
                set: { wizard.send(.choosePlacement(on: $0)) })) {
                ForEach(Array(v.placements.enumerated()), id: \.element.id) { index, option in
                    if index > 0, v.placements[index - 1].group != option.group { Divider() }
                    Text(option.label)
                        .tag(option.id)
                        .selectionDisabled(!option.enabled)
                        .help(option.detail)
                }
            }
            .pickerStyle(.menu)
            .accessibilityIdentifier("wizard-run-on")
            if let error = field.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
        case "kind":
            Picker(field.label, selection: Binding(
                get: { v.image.variant },
                set: { wizard.send(.chooseKind(kind: $0)) })) {
                ForEach(v.kindTiles, id: \.id) { tile in
                    Text(tile.title)
                        .tag(tile.id == "vm" ? AppSpaceKind.vm : AppSpaceKind.container)
                        .selectionDisabled(!tile.enabled)
                }
            }
        case "runtime":
            Picker(field.label, selection: Binding(
                get: { v.runtime },
                set: { wizard.send(.setRuntime(runtime: $0)) })) {
                ForEach(v.runtimes, id: \.value) { r in Text(r.label).tag(runtimeOf(r.value)) }
            }
            .disabled(!v.runtimeEnabled)
        case "connect-cloud":
            if let cloud {
                Button(field.label) { cloud.open() }
                    .buttonStyle(.link)
                    .accessibilityIdentifier("wizard-connect-cloud")
            }
        case "connect-by-address":
            Button(field.label) { wizard.send(.showAddress) }
                .buttonStyle(.link)
        default:
            EmptyView()
        }
    }

    private func field(_ v: AppWizardView, _ id: String) -> AppWizardField? {
        v.fields.first { $0.id == id }
    }

    private func label(_ v: AppWizardView, _ id: String) -> String { field(v, id)?.label ?? "" }

    private func prompt(_ v: AppWizardView, _ id: String) -> Text? { field(v, id)?.placeholder.map(Text.init) }

    private func resources(_ v: AppWizardView) -> some View {
        Form {
            Section {
                resourceSliders(v)
            }
            if !v.resourceFacts.isEmpty {
                Section {
                    ForEach(v.resourceFacts, id: \.id) { fact in
                        LabeledContent(fact.label) {
                            HStack(spacing: 4) {
                                Text(fact.value).textSelection(.enabled)
                                if let symbol = fact.symbol {
                                    Image(systemName: symbol)
                                        .foregroundStyle(.orange)
                                        .help(fact.help ?? "")
                                        .accessibilityLabel(fact.help ?? "")
                                }
                            }
                        }
                        .accessibilityIdentifier("wizard-fact-\(fact.id)")
                    }
                }
            }
            if let error = v.resourcesError {
                Text(error).foregroundStyle(.red).font(.callout)
                    .accessibilityIdentifier("wizard-resources-error")
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder private func resourceSliders(_ v: AppWizardView) -> some View {
        // In your cloud the machine type sets the size: no sliders.
        if field(v, "cpus") != nil {
        LabeledContent(label(v, "cpus")) {
            HStack {
                Slider(value: Binding(get: { Double(v.cpus) },
                                      set: { wizard.send(.setCpus(cpus: UInt32($0.rounded()))) }),
                       in: Double(v.minCpus)...Double(max(v.maxCpus, v.minCpus + 1)), step: 1)
                Text(v.cpusText).monospacedDigit().frame(width: 64, alignment: .trailing)
            }
        }
        LabeledContent(label(v, "memory")) {
            HStack {
                Slider(value: Binding(get: { Double(v.memoryGb) },
                                      set: { wizard.send(.setMemory(memoryGb: UInt32($0.rounded()))) }),
                       in: Double(v.minMemoryGb)...Double(v.maxMemoryGb), step: 1)
                Text(v.memoryText).monospacedDigit().frame(width: 64, alignment: .trailing)
            }
        }
        }
        if v.diskEditable {
            LabeledContent(label(v, "disk")) {
                HStack {
                    Slider(value: Binding(get: { Double(v.diskGb) },
                                          set: { wizard.send(.setDisk(diskGb: UInt32($0.rounded()))) }),
                           // Continuous: a GB step over hundreds of GB draws a solid tick rail.
                           in: Double(v.minDiskGb)...Double(max(v.maxDiskGb, v.minDiskGb + 1)))
                        .help(v.diskHelp ?? "")
                    Text(v.diskText).monospacedDigit().frame(width: 64, alignment: .trailing)
                }
            }
            if v.diskNote != nil || v.diskResetLabel != nil {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    if let note = v.diskNote {
                        Text(note).font(.callout).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    if let reset = v.diskResetLabel {
                        Button(reset) { wizard.send(.resetDisk) }
                            .controlSize(.small)
                            .accessibilityIdentifier("wizard-disk-reset")
                    }
                }
            }
        }
        if let gpu = v.gpu { gpuRow(gpu) }
        if let price = v.price {
            Text(price).foregroundStyle(.secondary).monospacedDigit()
                .accessibilityIdentifier("wizard-price")
        }
    }

    /// The GPU checkbox with a small "Learn more"; where this machine
    /// cannot, disabled with the reason (tooltip and one quiet line).
    @ViewBuilder private func gpuRow(_ gpu: AppGpuRow) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Toggle(gpu.label, isOn: Binding(get: { gpu.on }, set: { wizard.send(.setGpu(on: $0)) }))
                .toggleStyle(.checkbox)
                .disabled(!gpu.enabled)
                .help(gpu.reason ?? "")
                .accessibilityIdentifier("wizard-gpu")
            Spacer(minLength: 8)
            if let raw = gpu.learnMoreUrl, let url = URL(string: raw) {
                Link(gpu.learnMoreLabel, destination: url)
                    .font(.callout)
                    .accessibilityIdentifier("wizard-gpu-learn-more")
            }
        }
        if !gpu.enabled, let reason = gpu.reason {
            Text(reason).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                .accessibilityIdentifier("wizard-gpu-reason")
        }
    }

    private func options(_ v: AppWizardView) -> some View {
        Form {
            TextField(label(v, "name"), text: Binding(get: { v.name }, set: { wizard.send(.setName(name: $0)) }),
                      prompt: prompt(v, "name"))
            if let error = field(v, "name")?.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
            Toggle(label(v, "open-when-ready"), isOn: Binding(
                get: { v.openWhenReady }, set: { wizard.send(.setOpenWhenReady(on: $0)) }))
            if let note = v.streamNote {
                Text(note).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }

    private func summary(_ v: AppWizardView) -> some View {
        Form {
            ForEach(v.summary, id: \.label) { fact in
                LabeledContent(fact.label, value: fact.value)
            }
        }
        .formStyle(.grouped)
        .accessibilityIdentifier("wizard-summary")
    }

    private func address(_ v: AppWizardView) -> some View {
        Form {
            TextField(label(v, "address"), text: Binding(get: { wizard.state.address.url },
                                                        set: { wizard.send(.setAddress(url: $0)) }),
                      prompt: prompt(v, "address"))
            SecureField(label(v, "token"), text: Binding(get: { wizard.state.address.token },
                                                        set: { wizard.send(.setToken(token: $0)) }),
                        prompt: prompt(v, "token"))
            TextField(label(v, "address-name"), text: Binding(get: { wizard.state.address.name },
                                                             set: { wizard.send(.setAddressName(name: $0)) }),
                      prompt: prompt(v, "address-name"))
            if let price = v.price {
                Text(price).foregroundStyle(.secondary).accessibilityIdentifier("wizard-price")
            }
            if let error = v.address.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder private func footer(_ v: AppWizardView) -> some View {
        HStack {
            // While the image suggestions are open, Escape and Return are theirs.
            let menuOpen = v.mode == .create && v.imageField.open
            Button(v.labels.cancel, action: onCancel).keyboardShortcut(menuOpen ? nil : .cancelAction)
            Spacer()
            if v.mode == .address {
                Button(v.labels.back) { wizard.send(.hideAddress) }
                Button(v.address.submitLabel) {
                    Task { await wizard.submitAddress(onAdd) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!v.address.canSubmit)
            } else {
                if v.showBack { Button(v.labels.back) { wizard.send(.back) } }
                if v.step < 3 {
                    Button(v.primaryLabel) { wizard.send(.next) }
                        .keyboardShortcut(menuOpen ? nil : .defaultAction)
                        .disabled(!v.canContinue)
                } else {
                    Button(v.primaryLabel) { onCreate(v.plan) }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.glassProminent)
                }
            }
        }
    }

    private func osOf(_ id: String) -> AppSpaceOs {
        switch id {
        case "macos": return .macos
        case "windows": return .windows
        default: return .linux
        }
    }

    private func runtimeOf(_ word: String) -> AppRuntime {
        switch word {
        case "gvisor": return .gvisor
        case "runc": return .runc
        case "qemu": return .qemu
        case "lume": return .lume
        case "kubevirt": return .kubevirt
        default: return .auto
        }
    }
}
