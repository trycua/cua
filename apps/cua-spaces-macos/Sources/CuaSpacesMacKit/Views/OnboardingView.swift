// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// First run, laid out like the Tauri app's: Welcome centred (the stacked
/// Cua mark, title, one line, Get started), every other page in two columns
/// (title and one line on the left; the details card on the right). The
/// buttons sit in the window's bottom corners like macOS Setup Assistant:
/// Back bottom-left, Skip then the primary action bottom-right (the primary
/// in the corner), page dots centred between them. Done adds the example
/// prompts under both columns, scrolling slowly. Pages, copy, cards,
/// which buttons show and the answers are the core's (`appOnboardingView`);
/// only the placement is this view's, the same as the Tauri app's. There is
/// no command-line page, the bundled `cua` installs on first launch.
public struct OnboardingView: View {
    @Bindable var onboarding: OnboardingModel
    var onSignIn: (() async -> String?)?
    @State private var signingIn = false

    public init(onboarding: OnboardingModel, onSignIn: (() async -> String?)? = nil) {
        self.onboarding = onboarding
        self.onSignIn = onSignIn
    }

    public var body: some View {
        let v = onboarding.view
        VStack(spacing: 0) {
            Spacer(minLength: 24)
            if v.step == .welcome {
                welcome(v)
            } else {
                HStack(alignment: .center, spacing: 56) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(v.title)
                            .font(.system(size: 22, weight: .semibold))
                            .fixedSize(horizontal: false, vertical: true)
                        Text(v.lede).foregroundStyle(.secondary)
                        if v.step == .signin {
                            teamsLine
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    card(v)
                        .frame(maxWidth: .infinity)
                }
                .frame(maxWidth: 820)
                .padding(.horizontal, 40)
                if !v.prompts.isEmpty {
                    PromptTicker(prompts: v.prompts)
                        .frame(maxWidth: 820, alignment: .leading)
                        .padding(.horizontal, 40)
                        .padding(.top, 28)
                }
            }
            Spacer(minLength: 24)
            // The buttons always show: the pages give way first.
            footer(v).layoutPriority(1)
        }
        .frame(minWidth: 720, minHeight: 520)
        .task { await onboarding.installCliSilently() }
        .task { if !onboarding.state.driveChecked { await onboarding.checkDrive() } }
    }

    /// The Sign in page's Teams line: the website's waitlist (the app
    /// collects nothing).
    @ViewBuilder private var teamsLine: some View {
        let copy = onboarding.copy
        HStack(spacing: 4) {
            Text(copy.teams).foregroundStyle(.tertiary)
            if let url = URL(string: copy.teamsUrl) {
                Text("\u{00b7}").foregroundStyle(.tertiary)
                Link(copy.teamsLink, destination: url)
                    .accessibilityIdentifier("onboarding-teams")
            }
        }
        .font(.caption)
        .padding(.top, 4)
    }

    private func welcome(_ v: AppOnboardingView) -> some View {
        VStack(spacing: 12) {
            if v.showMark, let mark = StackMark.image {
                Image(nsImage: mark)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: 120, height: 120)
                    .accessibilityHidden(true)
                    .padding(.bottom, 8)
            }
            Text(v.title).font(.system(size: 22, weight: .semibold))
            Text(v.lede).foregroundStyle(.secondary)
            Button(v.primaryLabel) { onboarding.send(.start) }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .keyboardShortcut(.defaultAction)
                .padding(.top, 12)
            if let usage = v.usage {
                // The usage-data switch and its notice, first thing: nothing
                // is sent before Welcome is left, nothing at all while off.
                VStack(spacing: 6) {
                    Toggle(usage.label, isOn: Binding(get: { usage.on }, set: { onboarding.setShareUsage($0) }))
                        .toggleStyle(.switch)
                        .controlSize(.small)
                        .disabled(!usage.enabled)
                        .help(usage.help ?? "")
                        .accessibilityIdentifier("onboarding-share-usage")
                    if let notice = v.notice {
                        // It wraps, with its link inline.
                        noticeText(notice, v)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)
                            .environment(\.openURL, OpenURLAction { url in
                                NSWorkspace.shared.open(url)
                                return .handled
                            })
                            .accessibilityIdentifier("onboarding-telemetry-notice")
                    }
                }
                .frame(maxWidth: 520)
                .padding(.top, 20)
            }
        }
        .onAppear { onboarding.shown() }
    }

    /// The right column: the page's details in a bordered card (none when
    /// the page has nothing to show, like the Tauri app).
    @ViewBuilder private func card(_ v: AppOnboardingView) -> some View {
        let body = cardContent(v)
        if hasCard(v) {
            body
                .padding(.horizontal, 20)
                .padding(.vertical, 18)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(nsColor: .controlBackgroundColor), in: .rect(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(nsColor: .separatorColor)))
        } else {
            Color.clear.frame(height: 1)
        }
    }

    private func hasCard(_ v: AppOnboardingView) -> Bool {
        switch v.step {
        case .signin: return onboarding.state.identity != nil || signingIn
        default: return true
        }
    }

    @ViewBuilder private func cardContent(_ v: AppOnboardingView) -> some View {
        let copy = onboarding.copy
        switch v.step {
        case .signin:
            if let identity = onboarding.state.identity {
                Text(appOnboardingSignedInText(identity: identity)).foregroundStyle(.secondary)
            } else if signingIn {
                Text(copy.signInWaiting).foregroundStyle(.secondary)
            }
        case .agents:
            agentsContent
        case .drive:
            if let card = v.drive {
                DriveCardView(card: card, toggle: { onboarding.send(.driveToggled(on: $0)) },
                              openSettings: { onboarding.openURL($0) },
                              chooseStorage: { id in
                                  onboarding.send(.storageChosen(choice: id == "s3" ? .s3 : id == "later" ? .later : .local))
                              },
                              editStorage: { id, value in
                                  if let a = appStorageEdit(id: id, value: value) { onboarding.send(.driveStorage(action: a)) }
                              },
                              chooseStorageRow: { id, option in
                                  if let a = appStorageChoose(id: id, option: option) {
                                      Task { await onboarding.driveStorage(a) }
                                  }
                              },
                              pressStorageRow: { id in
                                  switch id {
                                  case "s3-test": Task { await onboarding.driveStorage(.test) }
                                  case "s3-manual": onboarding.send(.driveStorage(action: .showManual(on: true)))
                                  case "s3-use-prompt": onboarding.send(.driveStorage(action: .showManual(on: false)))
                                  default: break
                                  }
                              },
                              reveal: { onboarding.reveal($0) })
                    // The agent prompt shows: watch for the bucket it sets up.
                    .task(id: onboarding.watchingStorage) {
                        while onboarding.watchingStorage, !Task.isCancelled {
                            try? await Task.sleep(for: .seconds(2))
                            await onboarding.loadDriveStorage()
                        }
                    }
                    // While macOS waits for the extension's approval, the
                    // daemon finishes the mount on its own: follow it.
                    .task(id: card.settingsUrl) {
                        while card.settingsUrl != nil, !Task.isCancelled {
                            try? await Task.sleep(for: .seconds(2))
                            await onboarding.checkDrive()
                        }
                    }
            }
        case .presentation:
            VStack(spacing: 10) {
                ForEach(v.presentations, id: \.id) { c in
                    PresentationCardView(card: c) { onboarding.pickPresentation(menuBar: c.menuBar) }
                }
            }
            .onAppear {
                if let now = onboarding.currentMenuBar?(), now != onboarding.state.menuBar {
                    onboarding.send(.presentationPicked(menuBar: now))
                }
            }
        case .mode:
            if onboarding.showingHostForm {
                HostFormView(host: onboarding.host, buttons: false)
                    .frame(height: 230)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(v.choices, id: \.label) { choice in
                        Button {
                            if choice.mode == .host { onboarding.host.openForm() } else { onboarding.send(.modeChosen(mode: .client)) }
                        } label: {
                            Text(choice.label)
                                .fontWeight(.semibold)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 16)
                                .padding(.vertical, 14)
                                .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                        .background(Color(nsColor: .windowBackgroundColor), in: .rect(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10)
                            .strokeBorder(choice.preselected ? Color.primary : Color(nsColor: .separatorColor),
                                          lineWidth: choice.preselected ? 2 : 1))
                        .accessibilityIdentifier(choice.mode == .host ? "onboarding-host" : "onboarding-client")
                    }
                }
            }
        case .done:
            VStack(alignment: .leading, spacing: 8) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                    ForEach(v.summary, id: \.label) { f in
                        GridRow {
                            Text(f.label).foregroundStyle(.secondary)
                            Text(f.value)
                                .font(f.label == "cua command" && onboarding.state.cliTarget != nil ? .body.monospaced() : .body)
                                .lineLimit(1).truncationMode(.middle)
                        }
                    }
                }
                if !onboarding.hostPermissions.isEmpty {
                    Text(copy.permissionsTitle).font(.headline).padding(.top, 8)
                    PermissionRows(rows: onboarding.hostPermissions, openLabel: copy.openSettings)
                }
                if let login = v.launchAtLogin {
                    VStack(alignment: .leading, spacing: 4) {
                        Toggle(login.label, isOn: Binding(
                            get: { login.checked },
                            set: { onboarding.send(.launchAtLoginToggled(on: $0)) }))
                            .toggleStyle(.checkbox)
                            .accessibilityIdentifier("onboarding-launch-at-login")
                        Text(login.note).font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.top, 12)
                }
            }
        default:
            EmptyView()
        }
    }

    /// Detected coding agents: which to set up and what goes in.
    @ViewBuilder private var agentsContent: some View {
        let copy = onboarding.copy
        VStack(alignment: .leading, spacing: 8) {
            if onboarding.agentOutcomes != nil {
                Text(copy.agentsDoneTitle).font(.headline)
                ForEach(onboarding.agentSummaries, id: \.line) { s in
                    Text(s.line).lineLimit(1).help(([s.text] + s.failed).joined(separator: "\n"))
                        .foregroundStyle(s.failed.isEmpty ? Color.primary : Color.red)
                }
            } else if onboarding.agentStatuses == nil {
                Text(copy.agentsLooking).foregroundStyle(.secondary)
            } else if onboarding.installedAgents.isEmpty {
                Text(copy.agentsNone).foregroundStyle(.secondary)
            } else {
                // Two columns, at most `agentsListHeight` tall (scrolling
                // past it), so a long list never pushes Back and Continue
                // off the window.
                ViewThatFits(in: .vertical) {
                    agentGrid
                    ScrollView { agentGrid }
                        .scrollIndicators(.automatic)
                }
                .frame(maxHeight: Self.agentsListHeight)
                .accessibilityIdentifier("agents-list")
                Divider()
                HStack(spacing: 16) {
                    Toggle(copy.agentsSkills, isOn: $onboarding.agentSkills).toggleStyle(.checkbox)
                    Toggle(copy.agentsMcp, isOn: $onboarding.agentMcp).toggleStyle(.checkbox)
                }
                DriverCardView(title: copy.agentsDriver, imageLabel: copy.agentsDriverImage,
                               isOn: $onboarding.agentDriver)
                    .padding(.top, 4)
            }
            if let error = onboarding.agentsError {
                Text(error).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task { if onboarding.agentStatuses == nil { await onboarding.loadAgents() } }
    }

    /// The agent list's tallest (six rows of two); longer lists scroll.
    static let agentsListHeight: CGFloat = 132

    /// The detected agents' checkboxes, in two columns.
    private var agentGrid: some View {
        LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)],
                  alignment: .leading, spacing: 6) {
            ForEach(onboarding.installedAgents, id: \.id) { a in
                Toggle(a.name, isOn: Binding(
                    get: { onboarding.agentSelection.contains(a.id) },
                    set: { on in
                        if on { onboarding.agentSelection.insert(a.id) } else { onboarding.agentSelection.remove(a.id) }
                    }))
                    .toggleStyle(.checkbox)
                    .lineLimit(1)
                    .help([a.mcpConfig ?? a.skillsDir ?? "", a.error ?? ""].filter { !$0.isEmpty }.joined(separator: " \u{b7} "))
            }
        }
    }

    /// The telemetry notice with its link inline at the end (it wraps).
    private func noticeText(_ notice: String, _ v: AppOnboardingView) -> Text {
        var s = AttributedString(notice)
        s.foregroundColor = .secondary
        if let label = v.noticeLinkLabel, let url = v.noticeLinkUrl.flatMap(URL.init(string:)) {
            var link = AttributedString(label)
            link.link = url
            s += AttributedString(" ") + link
        }
        return Text(s)
    }

    /// The bottom bar (Setup Assistant placement): Back in the bottom-left
    /// corner; Skip, then the primary action in the bottom-right corner;
    /// the page dots centred. Welcome keeps its centred Get started, so its
    /// bar has only the dots.
    private func footer(_ v: AppOnboardingView) -> some View {
        ZStack {
            dots(v)
            if v.step != .welcome {
                HStack(spacing: 12) {
                    back(v)
                    Spacer(minLength: 24)
                    trailing(v)
                }
                .controlSize(.large)
            }
        }
        .frame(height: 32)
        .padding(.horizontal, 24)
        .padding(.bottom, 20)
    }

    /// Bottom-left: Back (the host form's Back while it is open).
    @ViewBuilder private func back(_ v: AppOnboardingView) -> some View {
        if v.step == .mode, let form = onboarding.host.formView {
            Button(form.backLabel) { onboarding.host.closeForm() }
                .accessibilityIdentifier("onboarding-back")
        } else if v.canBack {
            Button(onboarding.copy.back) { onboarding.send(.back) }
                .accessibilityIdentifier("onboarding-back")
        }
    }

    /// Bottom-right: Skip, then the primary action in the corner.
    @ViewBuilder private func trailing(_ v: AppOnboardingView) -> some View {
        HStack(spacing: 12) {
            if v.step == .mode, let form = onboarding.host.formView {
                Button(form.submitLabel) { Task { await onboarding.setUpHost() } }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!form.canSubmit)
                    .accessibilityIdentifier("host-setup")
            } else {
                if skipShows(v) {
                    Button(onboarding.copy.skip) { skip(v.step) }
                        .accessibilityIdentifier("onboarding-skip")
                }
                primary(v)
            }
        }
    }

    @ViewBuilder private func primary(_ v: AppOnboardingView) -> some View {
        let copy = onboarding.copy
        switch v.step {
        case .signin where onboarding.state.identity == nil:
            if let onSignIn {
                Button(signingIn ? copy.signInWaiting : copy.signIn) {
                    signingIn = true
                    Task {
                        if let id = await onSignIn() { onboarding.send(.signedIn(identity: id)) }
                        signingIn = false
                    }
                }
                .buttonStyle(.borderedProminent).controlSize(.large).keyboardShortcut(.defaultAction)
                .disabled(signingIn)
            }
        case .agents:
            if onboarding.agentOutcomes != nil {
                Button(copy.continueLabel) { onboarding.finishAgents() }
                    .buttonStyle(.borderedProminent).controlSize(.large).keyboardShortcut(.defaultAction)
            } else if !onboarding.installedAgents.isEmpty {
                Button(onboarding.agentsBusy ? copy.agentsSettingUp : copy.agentsSetUp) {
                    Task { await onboarding.setUpAgents() }
                }
                .buttonStyle(.borderedProminent).controlSize(.large).keyboardShortcut(.defaultAction)
                .disabled(!onboarding.canSetUpAgents)
                .accessibilityIdentifier("agents-setup")
            }
        case .mode:
            EmptyView()
        case .drive:
            Button(v.primaryLabel) { Task { await onboarding.continueDrive() } }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .keyboardShortcut(.defaultAction)
                .disabled(v.drive.map { !$0.canContinue } ?? false)
                .accessibilityIdentifier("drive-continue")
        default:
            Button(v.primaryLabel) { advance(v.step) }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .keyboardShortcut(.defaultAction)
        }
    }

    private func skipShows(_ v: AppOnboardingView) -> Bool {
        switch v.step {
        case .agents: return onboarding.agentOutcomes == nil
        default: return v.canSkip
        }
    }

    private func skip(_ step: AppOnboardingStep) {
        switch step {
        case .signin: onboarding.send(.signinDone)
        case .agents: onboarding.finishAgents(skipped: true)
        default: advance(step)
        }
    }

    private func advance(_ step: AppOnboardingStep) {
        switch step {
        case .welcome: onboarding.send(.start)
        case .signin: onboarding.send(.signinDone)
        case .agents: onboarding.finishAgents(skipped: true)
        case .presentation: onboarding.send(.presentationDone)
        case .drive: Task { await onboarding.continueDrive() }
        case .mode: onboarding.send(.modeChosen(mode: .client))
        case .done: onboarding.finish()
        }
    }

    private func dots(_ v: AppOnboardingView) -> some View {
        HStack(spacing: 8) {
            ForEach(v.dots, id: \.label) { dot in
                Circle()
                    .fill(dot.current ? Color.primary : Color.secondary.opacity(0.35))
                    .frame(width: 6, height: 6)
                    .help(dot.label)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \((v.dots.firstIndex { $0.current } ?? 0) + 1) of \(v.dots.count)")
    }
}

/// One "Where should Cua Spaces show up?" card, full width (the two are
/// stacked): the mode's animated miniature (`PresentationPreview`, the
/// core's scene and frames), wide and short, over its one-line title.
struct PresentationCardView: View {
    let card: AppPresentationCard
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var still: Bool?
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            VStack(spacing: 8) {
                PresentationPreview(menuBar: card.menuBar, fixedMs: fixedMs, stillOverride: still)
                    .clipShape(.rect(cornerRadius: 6))
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(card.imageLabel)
                Text(card.title).font(.system(size: 12.5, weight: .medium))
            }
            .padding(8)
            .frame(maxWidth: .infinity)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .background(Color(nsColor: .windowBackgroundColor), in: .rect(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10)
            .strokeBorder(card.selected ? Color.accentColor : Color(nsColor: .separatorColor),
                          lineWidth: card.selected ? 2 : 1))
        .accessibilityAddTraits(card.selected ? [.isButton, .isSelected] : .isButton)
        .accessibilityIdentifier(card.id)
    }
}

/// Done's example prompts (the core's), one quoted line each, scrolling
/// up slowly and continuously to suggest what to ask a coding agent. Not
/// interactive: it pauses under the pointer, and with Reduce Motion it is a
/// static list of the first rows.
struct PromptTicker: View {
    let prompts: [String]
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var origin = Date()
    @State private var pausedAt: Date?

    static let rowHeight: CGFloat = 22
    static let visibleRows = 3
    /// Points per second: a row every 4 seconds (the Tauri app's too).
    static let speed: CGFloat = rowHeight / 4

    var body: some View {
        let height = Self.rowHeight * CGFloat(Self.visibleRows)
        Group {
            if reduceMotion {
                rows(Array(prompts.prefix(Self.visibleRows)))
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30, paused: pausedAt != nil)) { context in
                    let cycle = Self.rowHeight * CGFloat(prompts.count)
                    let elapsed = (pausedAt ?? context.date).timeIntervalSince(origin)
                    let offset = (CGFloat(elapsed) * Self.speed).truncatingRemainder(dividingBy: max(cycle, 1))
                    rows(prompts + prompts)
                        .offset(y: -offset)
                        .frame(height: height, alignment: .top)
                        .clipped()
                        .mask(LinearGradient(stops: [
                            .init(color: .clear, location: 0), .init(color: .black, location: 0.25),
                            .init(color: .black, location: 0.75), .init(color: .clear, location: 1),
                        ], startPoint: .top, endPoint: .bottom))
                }
                .onHover { inside in
                    if inside {
                        pausedAt = Date()
                    } else if let at = pausedAt {
                        origin += Date().timeIntervalSince(at)
                        pausedAt = nil
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Example prompts")
                .accessibilityValue(prompts.joined(separator: ", "))
            }
        }
        .frame(height: height, alignment: .top)
    }

    private func rows(_ items: [String]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, text in
                Text("\u{201C}\(text)\u{201D}")
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(height: Self.rowHeight, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The stacked Cua mark (generated by apps/cua-spaces/scripts/icons).
public enum StackMark {
    public static let image: NSImage? = {
        guard let url = ModuleResources.url(forResource: "stack-mark", withExtension: "svg") else { return nil }
        return NSImage(contentsOf: url)
    }()
}
