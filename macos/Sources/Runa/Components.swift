import AppKit
import SwiftUI

/// A scroll view exactly as tall as its content, up to `maxHeight`.
/// A plain ScrollView has no ideal height, so a menubar window around it
/// gets a guessed size with empty space above and below.
struct FittingScrollView<Content: View>: View {
    var maxHeight: CGFloat
    @ViewBuilder var content: Content
    @State private var height: CGFloat = 0

    var body: some View {
        ScrollView {
            content.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: min(max(height, 1), maxHeight))
    }
}

struct SectionHeader<Accessory: View>: View {
    let title: String
    var count: Int?
    @ViewBuilder var accessory: Accessory

    var body: some View {
        HStack(spacing: 6) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(.secondary)
            if let count {
                Text("\(count)")
                    .font(.system(size: 10, weight: .semibold).monospacedDigit())
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(.quaternary, in: Capsule())
            }
            Spacer()
            accessory
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 6)
    }
}

extension SectionHeader where Accessory == EmptyView {
    init(title: String, count: Int? = nil) {
        self.init(title: title, count: count) { EmptyView() }
    }
}

struct StatusDot: View {
    let status: String

    var body: some View {
        ZStack {
            Circle().fill(color.opacity(0.25)).frame(width: 14, height: 14)
            Circle().fill(color).frame(width: 7, height: 7)
        }
        .help(status.capitalized)
    }

    private var color: Color {
        switch status {
        case "running": Brand.running
        case "failed": Brand.failed
        default: Brand.warning
        }
    }
}

/// "􀈕 shop › web  􀙡 main": which project a process belongs to.
struct ProjectTag: View {
    let project: Workspace

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: "folder.fill")
                .foregroundStyle(.secondary)
            Text(project.label)
                .fontWeight(.medium)
                .foregroundStyle(.primary.opacity(0.75))
            if let branch = project.branch {
                Image(systemName: "arrow.triangle.branch")
                    .foregroundStyle(.tertiary)
                    .padding(.leading, 3)
                Text(branch)
                    .foregroundStyle(.secondary)
            }
        }
        .font(.system(size: 10.5))
        .lineLimit(1)
        .truncationMode(.tail)
        .help(project.root)
    }
}

enum Finder {
    static func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

struct PortChip: View {
    @Environment(Store.self) private var store
    let port: UInt16
    @State private var hovering = false

    var body: some View {
        Button {
            store.open(port: port)
        } label: {
            Text(verbatim: ":\(port)")
                .font(.system(size: 10.5, weight: .medium).monospacedDigit())
                .padding(.horizontal, 6)
                .padding(.vertical, 1.5)
                .background(hovering ? Brand.accent.opacity(0.18) : Color.primary.opacity(0.08), in: Capsule())
                .foregroundStyle(hovering ? Brand.accent : .secondary)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("Open http://localhost:\(String(port))")
    }
}

/// A small round icon button that lights up on hover.
struct IconButton: View {
    let symbol: String
    let help: String
    var tint: Color = .primary
    var spinning = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            icon
                .font(.system(size: 11, weight: .semibold))
                .frame(width: 26, height: 26)
        }
        .buttonStyle(HoverButtonStyle(tint: tint))
        .help(help)
    }
}

extension IconButton {
    @ViewBuilder
    private var icon: some View {
        if #available(macOS 15.0, *) {
            Image(systemName: symbol).symbolEffect(.rotate, isActive: spinning)
        } else {
            Image(systemName: symbol)
        }
    }
}

struct HoverButtonStyle: ButtonStyle {
    var tint: Color = .primary
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(hovering ? tint : .secondary)
            .background(
                Circle().fill(Color.primary.opacity(configuration.isPressed ? 0.16 : hovering ? 0.09 : 0))
            )
            .contentShape(Circle())
            .onHover { hovering = $0 }
    }
}

/// Rounded highlight behind a hovered row.
struct RowBackground: ViewModifier {
    let highlighted: Bool

    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color.primary.opacity(highlighted ? 0.07 : 0))
            )
            .padding(.horizontal, 4)
            .contentShape(Rectangle())
    }
}

extension View {
    func rowBackground(_ highlighted: Bool) -> some View {
        modifier(RowBackground(highlighted: highlighted))
    }
}
