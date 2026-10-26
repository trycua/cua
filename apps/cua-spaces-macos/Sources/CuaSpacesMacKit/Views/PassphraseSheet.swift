// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// The passphrase and, at setup, its confirmation, typed into the model's
/// fields (`submitCredential` reads and clears them).
struct PassphraseFields: View {
    @Bindable var keyvault: KeyvaultModel
    let form: KvCredentialForm
    @FocusState private var focused: Bool

    var body: some View {
        SecureField(form.passphraseLabel ?? "", text: $keyvault.passphrase)
            .focused($focused)
            .onAppear { focused = true }
        if let confirm = form.confirmLabel {
            SecureField(confirm, text: $keyvault.passphraseConfirm)
        }
    }
}

/// The passphrase form as a sheet on the web UI window (`keyvault.setup`,
/// `keyvault.unlockVault`): with
/// Cancel. `done(true)` sends it and the bridge submits it, so the passphrase
/// never reaches the page (it stays in this process).
struct PassphraseSheet: View {
    @Bindable var keyvault: KeyvaultModel
    let form: KvCredentialForm
    let done: (Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(form.mode == .setup ? "Set up Keyvault" : "Unlock Keyvault")
                .font(.headline)
            Text(form.help)
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.bottom, 4)
            PassphraseFields(keyvault: keyvault, form: form)
            Text(keyvault.passphraseCheck?.hint ?? " ")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { done(false) }
                    .keyboardShortcut(.cancelAction)
                Button(form.submitLabel) { done(true) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!keyvault.canSubmitCredential)
            }
            .padding(.top, 4)
        }
        .textFieldStyle(.roundedBorder)
        .padding(20)
        .frame(width: 380)
        .onSubmit { if keyvault.canSubmitCredential { done(true) } }
    }
}
