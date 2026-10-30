// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if os(iOS)
import Cua
import CuaBotsCore
import CuaBotsPhone
import CuaBotsRemote
import SwiftUI
import UserNotifications

/// The iPhone app. Sign in with a pairing link from Cua Bots on the Mac
/// (`cuabots://relay?machine=...` over the relay, or
/// `cuabots://direct?url=...&token=...` on the same network).
@main
struct CuaBotsPhoneApp: App {
    @StateObject private var model = PhoneModel()
    @AppStorage("pairing") private var pairing = ""
    @State private var error: String?

    var body: some Scene {
        WindowGroup {
            Group {
                if model.bots.isEmpty {
                    PairView(link: $pairing, error: error) { Task { await connect() } }
                } else {
                    PhoneRoot(model: model)
                }
            }
            .task {
                _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
                model.onNotification = { n, bot in
                    let c = UNMutableNotificationContent()
                    c.title = n.title
                    c.body = n.body
                    c.threadIdentifier = bot.id
                    UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: n.id, content: c, trigger: nil))
                }
                if !pairing.isEmpty { await connect() }
            }
            .preferredColorScheme(AppAppearance.scheme)
            .onOpenURL { url in
                pairing = url.absoluteString
                Task { await connect() }
            }
        }
    }

    func connect() async {
        do {
            let cua = try Cua.embedded()
            // Relay links use the account token of the signed-in Cua account.
            let token = try? await cua.auth().accessToken(force: false)
            guard let endpoint = RemoteEndpoint.parse(pairing, accountToken: token) else {
                error = "That isn't a Cua Bots pairing link."
                return
            }
            model.attach(try await RemoteBot.connect(endpoint, cua: cua), label: endpoint.label)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

struct PairView: View {
    @Binding var link: String
    var error: String?
    var connect: () -> Void

    var body: some View {
        VStack(spacing: 18) {
            KoalaAvatar(AvatarConfig(color: .cloud, eyes: .star, ears: .scalloped)).frame(width: 110, height: 110)
            Text("Cua Bots").font(.title.weight(.semibold))
            Text("Open Cua Bots on your Mac, choose a bot, then Pair iPhone. Or paste its link here.")
                .multilineTextAlignment(.center).foregroundStyle(.secondary)
            TextField("cuabots://…", text: $link).textFieldStyle(.roundedBorder).autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            Button("Connect", action: connect).buttonStyle(.borderedProminent).disabled(link.isEmpty)
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .padding(28)
    }
}
#endif
