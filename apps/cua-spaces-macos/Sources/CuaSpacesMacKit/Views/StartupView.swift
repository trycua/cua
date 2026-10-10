// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI

/// The launch, until the live services are in: "Starting Cua…", or what
/// the keychain needs ("Allow Keychain access", "Waiting for Keychain
/// access…"). The words and buttons are `StartupModel.copy`, which New UI's
/// startup screen shows too.
struct StartupView: View {
    let startup: StartupModel
    @State private var confirmingSignIn = false

    var body: some View {
        let copy = startup.copy
        VStack(spacing: 14) {
            if copy.actions.isEmpty {
                ProgressView().controlSize(.regular)
            } else {
                Image(systemName: "key.fill")
                    .font(.system(size: 30, weight: .regular))
                    .foregroundStyle(.secondary)
            }
            Text(copy.title)
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
            if !copy.body.isEmpty {
                Text(copy.body)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: 440)
            }
            if !copy.actions.isEmpty {
                HStack(spacing: 10) {
                    ForEach(Array(copy.actions.enumerated()), id: \.offset) { index, action in
                        button(action, primary: index == 0)
                    }
                }
                .padding(.top, 6)
            }
        }
        .padding(40)
        .frame(minWidth: 520, maxWidth: .infinity, minHeight: 360, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("startup")
        .confirmationDialog("Sign in again?", isPresented: $confirmingSignIn) {
            Button("Sign in again") { startup.act(.signInAgain) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This removes the saved sign-in from this Mac's keychain. Nothing else is deleted. "
                 + "You'll sign in again in your browser.")
        }
    }

    @ViewBuilder
    private func button(_ action: StartupModel.Action, primary: Bool) -> some View {
        let label = Self.label(action)
        if primary {
            Button(label) { press(action) }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        } else {
            Button(label) { press(action) }
                .buttonStyle(.bordered)
        }
    }

    private func press(_ action: StartupModel.Action) {
        if action == .signInAgain {
            confirmingSignIn = true
        } else {
            // The keychain prompt shows over the app that asked: be in front.
            NSApp.activate()
            startup.act(action)
        }
    }

    static func label(_ action: StartupModel.Action) -> String {
        switch action {
        case .allowAccess: return "Allow access"
        case .tryAgain: return "Try again"
        case .signInAgain: return "Sign in again"
        }
    }
}
