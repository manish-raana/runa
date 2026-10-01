import AppKit
import SwiftUI

/// The runa mark: a heartbeat pulse, for "keep your processes alive".
/// Points are in a unit square, y pointing down.
enum Pulse {
    static let points: [CGPoint] = [
        CGPoint(x: 0.00, y: 0.56),
        CGPoint(x: 0.27, y: 0.56),
        CGPoint(x: 0.37, y: 0.30),
        CGPoint(x: 0.50, y: 0.82),
        CGPoint(x: 0.62, y: 0.18),
        CGPoint(x: 0.73, y: 0.56),
        CGPoint(x: 1.00, y: 0.56),
    ]

    static func path(in rect: CGRect) -> Path {
        var path = Path()
        for (index, point) in points.enumerated() {
            let p = CGPoint(x: rect.minX + point.x * rect.width, y: rect.minY + point.y * rect.height)
            index == 0 ? path.move(to: p) : path.addLine(to: p)
        }
        return path
    }
}

struct PulseShape: Shape {
    func path(in rect: CGRect) -> Path { Pulse.path(in: rect) }
}

/// The app icon in miniature: a graphite squircle with a bright pulse.
struct AppLogo: View {
    var size: CGFloat = 22

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.235, style: .continuous)
            .fill(LinearGradient(colors: [Color(white: 0.27), Color(white: 0.11)],
                                 startPoint: .top, endPoint: .bottom))
            .overlay(
                PulseShape()
                    .stroke(LinearGradient(colors: [.white, Color(red: 0.55, green: 0.85, blue: 1.0)],
                                           startPoint: .leading, endPoint: .trailing),
                            style: StrokeStyle(lineWidth: max(1.4, size * 0.075), lineCap: .round, lineJoin: .round))
                    .padding(.horizontal, size * 0.16)
                    .padding(.vertical, size * 0.22)
            )
            .overlay(
                RoundedRectangle(cornerRadius: size * 0.235, style: .continuous)
                    .strokeBorder(.white.opacity(0.12), lineWidth: 0.5)
            )
            .frame(width: size, height: size)
    }
}

enum StatusIcon {
    /// The whole menu bar item as one template image, so the counts and the
    /// port glyph follow the menu bar's tint: `[pulse] 3  [network] 5`.
    static func image(alert: Bool, processes: Int, ports: Int) -> NSImage {
        let glyph = image(alert: alert)
        let font = NSFont.monospacedDigitSystemFont(ofSize: 12.5, weight: .medium)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.black]
        let processText = processes > 0 ? NSAttributedString(string: "\(processes)", attributes: attributes) : nil
        let portText = ports > 0 ? NSAttributedString(string: "\(ports)", attributes: attributes) : nil
        let portGlyph = NSImage(systemSymbolName: "network", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 11, weight: .semibold))

        var width: CGFloat = 18
        if let processText { width += 3 + processText.size().width }
        if let portText, let portGlyph { width += 7 + portGlyph.size.width + 2 + portText.size().width }
        let height: CGFloat = 18

        let image = NSImage(size: NSSize(width: ceil(width), height: height), flipped: false) { _ in
            glyph.draw(in: NSRect(x: 0, y: 0, width: 18, height: 18))
            var x: CGFloat = 18
            func drawText(_ text: NSAttributedString) {
                let size = text.size()
                text.draw(at: NSPoint(x: x, y: (height - size.height) / 2))
                x += size.width
            }
            if let processText {
                x += 3
                drawText(processText)
            }
            if let portText, let portGlyph {
                x += 7
                let size = portGlyph.size
                portGlyph.draw(in: NSRect(x: x, y: (height - size.height) / 2, width: size.width, height: size.height))
                x += size.width + 2
                drawText(portText)
            }
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Runa: \(processes) processes, \(ports) ports"
        return image
    }

    /// An 18pt template glyph: the pulse inside a rounded square, with a
    /// dot cut out at the corner when something is down.
    static func image(alert: Bool) -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: true) { _ in
            let frame = NSRect(x: 1.5, y: 2, width: 15, height: 14)
            let box = NSBezierPath(roundedRect: frame, xRadius: 4, yRadius: 4)
            box.lineWidth = 1.4
            box.stroke()

            let inner = frame.insetBy(dx: 2.6, dy: 3.2)
            let pulse = NSBezierPath()
            for (index, point) in Pulse.points.enumerated() {
                let p = NSPoint(x: inner.minX + point.x * inner.width, y: inner.minY + point.y * inner.height)
                index == 0 ? pulse.move(to: p) : pulse.line(to: p)
            }
            pulse.lineWidth = 1.4
            pulse.lineCapStyle = .round
            pulse.lineJoinStyle = .round
            pulse.stroke()

            if alert {
                // Clear a ring around the badge so it reads on any wallpaper.
                NSGraphicsContext.current?.compositingOperation = .clear
                NSBezierPath(ovalIn: NSRect(x: 10.5, y: 0, width: 7.5, height: 7.5)).fill()
                NSGraphicsContext.current?.compositingOperation = .sourceOver
                NSBezierPath(ovalIn: NSRect(x: 11.75, y: 1.25, width: 5, height: 5)).fill()
            }
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = alert ? "Runa: a process is down" : "Runa"
        return image
    }
}
