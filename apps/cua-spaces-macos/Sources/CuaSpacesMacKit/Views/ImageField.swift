// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The New Space image field: any image reference, with the catalog's
/// presets as suggestions under it. Filtering, the highlighted row, what
/// Return picks and validation are the app core's (`AppImageFieldView`);
/// the Tauri app draws the same view.
struct ImageField: View {
    let field: AppWizardField
    let view: AppImageFieldView
    let send: (AppWizardAction) -> Void

    var body: some View {
        LabeledContent(field.label) {
            HStack(spacing: 4) {
                TextField(field.label, text: Binding(get: { view.text },
                                                     set: { send(.setImageText(text: $0)) }),
                          prompt: field.placeholder.map(Text.init))
                    .labelsHidden()
                    .multilineTextAlignment(.trailing)
                    .autocorrectionDisabled()
                    .onKeyPress(.downArrow) { send(.moveImageSuggestion(delta: 1)); return .handled }
                    .onKeyPress(.upArrow) { send(.moveImageSuggestion(delta: -1)); return .handled }
                    .onKeyPress(.return) {
                        guard view.open else { return .ignored }
                        send(.pickImageSuggestion)
                        return .handled
                    }
                    .onKeyPress(.escape) {
                        guard view.open else { return .ignored }
                        send(.dismissImageSuggestions)
                        return .handled
                    }
                    .accessibilityIdentifier("image-field")
                Button {
                    send(view.open ? .dismissImageSuggestions : .openImageSuggestions)
                } label: {
                    Image(systemName: "chevron.up.chevron.down")
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Show images")
            }
            .popover(isPresented: Binding(get: { view.open },
                                          set: { if !$0 { send(.dismissImageSuggestions) } }),
                     arrowEdge: .bottom) {
                SuggestionList(groups: view.groups, send: send)
            }
        }
        if let error = field.error {
            Text(error).foregroundStyle(.red).font(.callout)
        }
    }
}

/// The presets, grouped by catalog group, one line a row: the ref, then
/// the image's name.
private struct SuggestionList: View {
    let groups: [AppSuggestionGroup]
    let send: (AppWizardAction) -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(groups, id: \.id) { group in
                    Text(group.label)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 8)
                        .padding(.top, 6)
                        .padding(.bottom, 2)
                    ForEach(group.rows, id: \.imageRef) { row in
                        HStack(spacing: 12) {
                            Text(row.imageRef).lineLimit(1).truncationMode(.middle)
                            Spacer(minLength: 0)
                            Text(row.label)
                                .foregroundStyle(row.highlighted ? AnyShapeStyle(.white.opacity(0.85))
                                                                 : AnyShapeStyle(.secondary))
                                .lineLimit(1)
                        }
                        .foregroundStyle(row.highlighted ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(row.highlighted ? Color.accentColor : .clear,
                                    in: RoundedRectangle(cornerRadius: 5))
                        .contentShape(Rectangle())
                        .onTapGesture { send(.chooseImage(imageRef: row.imageRef)) }
                        .accessibilityAddTraits(row.highlighted ? .isSelected : [])
                    }
                }
            }
            .padding(5)
        }
        .frame(width: 420, height: height)
        .focusable(false)
    }

    /// Fits the rows, up to a scrolling maximum.
    private var height: CGFloat {
        let rows = groups.reduce(0) { $0 + $1.rows.count }
        return min(260, CGFloat(groups.count) * 24 + CGFloat(rows) * 22 + 10)
    }
}
