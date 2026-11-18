// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import SwiftUI

/// The window's whole contents: a sign-in gate, then the app shell over the
/// live store.
struct RootView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var store: BotStore

    init(model: AppModel) {
        self.model = model
        self.store = model.store
    }

    private var appearance: AppAppearance {
        AppAppearance(rawValue: model.desktopTheme) ?? .system
    }

    var body: some View {
        Group {
            if model.signedIn {
                DesktopSurface(model: model, store: store)
            } else {
                // The gate. The roster is not reachable behind it.
                SignInView(onSignIn: model.signIn)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .modifier(OpaqueBackground())
        .preferredColorScheme(appearance.colorScheme)
        .sheet(isPresented: $model.hiring) { HireSheet(model: model) }
        .sheet(isPresented: $model.creatingGroup) {
            NewGroupSheet(bots: store.bots, store: model.groups,
                          onCreated: { model.opened($0) },
                          onCancel: { model.creatingGroup = false })
        }
        .sheet(isPresented: $model.creatingSpace) {
            SpaceWizard(plan: UICapture.designPlan ?? SpacePlan(placement: SpacePlacement.configuredDefault), step: UICapture.designStep ?? .system, onCreate: { plan in _ = try await model.createSpace(plan) },
                        onCancel: { model.creatingSpace = false })
        }
    }
}

/// Paints the palette's background under everything, so no window material
/// shows through anywhere.
private struct OpaqueBackground: ViewModifier {
    @Environment(\.colorScheme) private var scheme
    func body(content: Content) -> some View {
        content.background(KoalaPalette.resolve(scheme).bg.ignoresSafeArea())
    }
}

// MARK: - Desktop

struct DesktopSurface: View {
    @ObservedObject var model: AppModel
    @ObservedObject var store: BotStore
    @Environment(\.colorScheme) private var scheme
    @StateObject private var drops: SpaceDropCoordinator

    init(model: AppModel, store: BotStore) {
        self.model = model
        self.store = store
        _drops = StateObject(wrappedValue: SpaceDropCoordinator(store: store))
    }

    var body: some View {
        if let bot = model.focusedBot, model.route == .takeover(bot.id) {
            // Tier 3: the Agent Computer takes the whole window, with an
            // explicit hand-back, and the same drop zone as the Computer pane.
            GeometryReader { g in
                DesktopShell(theme: KoalaPalette.resolve(scheme).theme,
                             canvas: CGSize(width: max(g.size.width, 1), height: max(g.size.height, 1)),
                             scrolls: true, bot: bot,
                             source: store, showRightPanel: false,
                             screen: model.screenSource, takeover: true,
                             dropZone: store.spaceID.map { drops.zone(space: $0, botID: bot.id) },
                             onToggleComputer: { model.escalate(from: bot.id) })
            }
            .padding(.top, 28)
            .clipped()
        } else {
            KoalaShell(model: model, drops: drops)
        }
    }
}

// MARK: - Creating a Bot with a first instruction

/// Creating a Bot that starts on a specific task.
///
/// The sidebar `+` is the ordinary way in and takes no prompt at all — a new
/// Bot greets you and asks what you want. This sheet is the other one: two
/// fields, because a Bot started *on something* needs the something. It is kept
/// because the mobile surface has no compose header to replace, and because
/// `agent_start` takes a prompt whether or not a human wrote it.
struct HireSheet: View {
    @ObservedObject var model: AppModel
    @State private var name = ""
    @State private var prompt = ""
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Bot").font(.system(size: 17, weight: .semibold))
            Text("A Bot is a coworker with one long-lived thread, not a chat. "
                 + "It starts work in the Space you are already attached to.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            TextField("Name, e.g. Inbox Manager", text: $name)
            TextField("What should it do first?", text: $prompt, axis: .vertical)
                .lineLimit(3...6)

            HStack {
                Spacer()
                Button("Cancel") { model.hiring = false }
                Button(working ? "Creating\u{2026}" : "Create") {
                    working = true
                    Task {
                        _ = await model.hire(name: name, prompt: prompt)
                        working = false
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(working
                          || name.trimmingCharacters(in: .whitespaces).isEmpty
                          || prompt.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .textFieldStyle(.roundedBorder)
        .padding(18)
        .frame(width: 380)
    }
}
#endif
