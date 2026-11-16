#if os(macOS)
import SwiftUI

@available(macOS 13.0, *)
struct PermissionFlowPanelView: View {
    @ObservedObject var controller: PermissionFlowController

    var body: some View {
        content
            .padding(.top, 8)
            .padding(.bottom, 12)
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .fixedSize(horizontal: false, vertical: true)
            .background(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(.ultraThinMaterial)
                    .overlay(
                        RoundedRectangle(cornerRadius: 18, style: .continuous)
                            .stroke(.primary.opacity(0.14), lineWidth: 1)
                    )
            )
    }

    @ViewBuilder
    private var content: some View {
        if controller.isAwaitingSettingsWindow {
            loadingContent
        } else {
            VStack(alignment: .leading, spacing: 6) {
                header
                if let detail = controller.guidance?.detail {
                    Text(detail)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.leading, 21)
                }
                if controller.guidance?.showsApp ?? true, let primaryApp = controller.preferredAppURL {
                    AppDragItemView(
                        url: primaryApp,
                        localeIdentifier: controller.localeIdentifier
                    ) { isDragging in
                        controller.setPanelDragging(isDragging)
                    }
                    .frame(maxWidth: .infinity)
                }
            }
        }
    }

    /// Shown from the click until System Settings' window can be tracked.
    /// The card stays at its real content size so this state is a finished
    /// loading panel, not a clipped preview of the drag guide.
    private var loadingContent: some View {
        HStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(loadingTitle)
                .font(.system(size: 14))
                .frame(maxWidth: .infinity, alignment: .leading)
            closeButton
        }
        .frame(minHeight: 36)
    }

    private var loadingTitle: String {
        PermissionFlowLocalizer.string(
            "permission_flow.panel.opening_settings",
            defaultValue: "Opening System Settings...",
            localeIdentifier: controller.localeIdentifier
        )
    }

    /// Keeps the header logic isolated from the drag card layout.
    private var header: some View {
        HStack(alignment: .top, spacing: 3) {
            if let symbol = controller.guidance?.symbol {
                Image(systemName: symbol)
                    .font(.system(size: 15, weight: .semibold))
                    .frame(width: 18)
            } else {
                HeaderDirectionIcon(isDragging: controller.isDraggingApp)
            }
            Text(headerTitle)
                .font(.system(size: 14))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .multilineTextAlignment(.leading)
            HStack(alignment: .top, spacing: 3) {
                if controller.isSettingsFrontmost == false {
                    Button {
                        controller.reopenCurrentSettingsPane()
                    } label: {
                        Image(systemName: "gear")
                            .font(.system(size: 15, weight: .semibold))
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.primary, .secondary.opacity(0.35))
                    }
                    .buttonStyle(.borderless)
                }
                closeButton
            }
            .layoutPriority(1)
        }
    }

    private var closeButton: some View {
        Button {
            controller.closePanel(returnToPreviousApp: true)
        } label: {
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 18, weight: .semibold))
                .symbolRenderingMode(.palette)
                .foregroundStyle(.primary, .secondary.opacity(0.35))
        }
        .buttonStyle(.borderless)
    }

    /// Builds a markdown-backed localized title such as:
    /// "Drag **Example** to the list above to allow **Accessibility**."
    /// On macOS 27+, the permission name is "Device Control and Data Access".
    private var headerTitle: AttributedString {
        if let title = controller.guidance?.title {
            return (try? AttributedString(
                markdown: title,
                options: AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
            )) ?? AttributedString(title)
        }
        let localizedTemplate = PermissionFlowLocalizer.string(
            "permission_flow.panel.title",
            defaultValue: "Drag **%@** to the list above to allow **%@**.",
            localeIdentifier: controller.localeIdentifier
        )

        let markdown = String(
            format: localizedTemplate,
            locale: localizationLocale,
            appDisplayName,
            paneDisplayTitle
        )

        return (try? AttributedString(
            markdown: markdown,
            options: AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(markdown)
    }

    /// Prefers the Finder-style display name so the title reads naturally even
    /// when the URL contains a plain bundle filename.
    private var appDisplayName: String {
        guard let appURL = controller.preferredAppURL else {
            return PermissionFlowLocalizer.string(
                "permission_flow.app.this_app",
                defaultValue: "This App",
                localeIdentifier: controller.localeIdentifier
            )
        }

        return FileManager.default.displayName(atPath: appURL.path)
    }

    /// Uses the current pane's localized title so each permission can render a
    /// specific instruction in the shared panel title template.
    private var paneDisplayTitle: String {
        controller.currentPane?.localizedTitle(localeIdentifier: controller.localeIdentifier)
            ?? PermissionFlowLocalizer.string(
                "permission_flow.pane.permission",
                defaultValue: "Permission",
                localeIdentifier: controller.localeIdentifier
            )
    }

    /// Uses the explicitly injected panel locale when available.
    private var localizationLocale: Locale {
        controller.localeIdentifier.map(Locale.init(identifier:)) ?? .current
    }
}

@available(macOS 13.0, *)
private struct HeaderDirectionIcon: View {
    let isDragging: Bool

    @State private var wigglePhase = false
    @State private var scalePhase = false

    var body: some View {
        Image(systemName: "arrowshape.up.fill")
            .font(.system(size: 14, weight: .bold))
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(.tint)
            .rotationEffect(.degrees(isDragging ? 0 : (wigglePhase ? 12 : -12)))
            .offset(y: isDragging ? 0 : (wigglePhase ? -2 : 1))
            .scaleEffect(isDragging ? (scalePhase ? 1.18 : 0.88) : 1)
            .animation(
                isDragging
                    ? .easeInOut(duration: 0.68).repeatForever(autoreverses: true)
                    : .easeInOut(duration: 0.22).repeatForever(autoreverses: true),
                value: isDragging ? scalePhase : wigglePhase
            )
            .onAppear {
                wigglePhase = true
            }
            .onChange(of: isDragging) { dragging in
                if dragging {
                    scalePhase = true
                    wigglePhase = false
                } else {
                    scalePhase = false
                    wigglePhase = true
                }
            }
    }
}
#endif
