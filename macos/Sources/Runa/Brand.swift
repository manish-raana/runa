import AppKit
import SwiftUI

/// Native palette: the user's accent color plus system status colors, so
/// the panel matches every other part of macOS in light and dark mode.
enum Brand {
    static let accent = Color.accentColor
    static let running = Color(nsColor: .systemGreen)
    static let failed = Color(nsColor: .systemRed)
    static let warning = Color(nsColor: .systemOrange)
    static let cpu = Color.accentColor
    static let memory = Color(nsColor: .systemTeal)

    /// Matches the panel's AppKit background (see PanelBackgroundView).
    static let cornerRadius: CGFloat = 16
}

/// Size and focus behavior shared by every screen of the panel. The
/// background itself is native AppKit glass, drawn by PanelBackgroundView.
struct PanelChrome: ViewModifier {
    func body(content: Content) -> some View {
        content
            .frame(width: 360)
            .fixedSize(horizontal: false, vertical: true)
            .focusEffectDisabled()
    }
}

/// A grouped block of rows, like a section in System Settings.
struct Card: ViewModifier {
    func body(content: Content) -> some View {
        content
            .padding(.vertical, 4)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color.primary.opacity(0.045))
            )
            .padding(.horizontal, 10)
    }
}

extension View {
    func card() -> some View { modifier(Card()) }

    /// Glass buttons on macOS 26, bordered ones before.
    @ViewBuilder
    func glassButtonStyle() -> some View {
        if #available(macOS 26.0, *) {
            buttonStyle(.glass)
        } else {
            buttonStyle(.bordered)
        }
    }
}

/// The panel's background: Liquid Glass on macOS 26, the popover material
/// before. The rounded, clipped layer gives the window a rounded shadow.
final class PanelBackgroundView: NSView {
    init(content: NSView) {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = Brand.cornerRadius
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true

        let background: NSView
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = Brand.cornerRadius
            glass.contentView = content
            background = glass
        } else {
            let effect = NSVisualEffectView()
            effect.material = .popover
            effect.blendingMode = .behindWindow
            effect.state = .active
            content.frame = effect.bounds
            content.autoresizingMask = [.width, .height]
            effect.addSubview(content)
            background = effect
        }
        background.frame = bounds
        background.autoresizingMask = [.width, .height]
        addSubview(background)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}
