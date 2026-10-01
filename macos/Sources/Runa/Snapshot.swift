import AppKit
import SwiftUI

/// `Runa --snapshot <dir>` renders the menu's screens to PNGs, in light
/// and dark mode, then quits. For checking layout without opening the menu.
@MainActor
enum Snapshot {
    static var directory: String? {
        let args = CommandLine.arguments
        guard let index = args.firstIndex(of: "--snapshot"), index + 1 < args.count else { return nil }
        return args[index + 1]
    }

    static func run(store: Store, to directory: String) async {
        // Let the first refresh land.
        try? await Task.sleep(for: .seconds(3))
        renderStatusItem(store: store, to: "\(directory)/menubar.png")
        for (suffix, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            await render(OverviewView { _ in }, store: store, appearance: appearance,
                         to: "\(directory)/overview-\(suffix).png", settle: 1)
            if let name = store.processes.first(where: \.running)?.name ?? store.processes.first?.name {
                await render(ProcessDetailView(name: name) {}, store: store, appearance: appearance,
                             to: "\(directory)/detail-\(suffix).png", settle: 3)
            }
        }
        exit(0)
    }

    private static func render(_ view: some View, store: Store, appearance: NSAppearance.Name,
                               to path: String, settle seconds: Double) async {
        // A wallpaper-like backdrop so the glass has something to show.
        let backdrop = LinearGradient(colors: [Color(red: 0.16, green: 0.42, blue: 0.68),
                                               Color(red: 0.52, green: 0.66, blue: 0.80)],
                                      startPoint: .top, endPoint: .bottom)
        let host = NSHostingView(rootView: view.environment(store)
            .modifier(PanelChrome())
            // Stand-in for the AppKit glass, which can't render off-screen.
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: Brand.cornerRadius, style: .continuous))
            .padding(24)
            .background(backdrop))
        host.appearance = NSAppearance(named: appearance)
        let window = NSWindow(contentRect: NSRect(x: -4000, y: -4000, width: 416, height: 400),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: appearance)
        window.backgroundColor = .windowBackgroundColor
        window.contentView = host
        window.orderFrontRegardless()
        // Size to content twice: once to load, once after async content lands.
        for _ in 0..<2 {
            window.setContentSize(host.fittingSize)
            try? await Task.sleep(for: .seconds(seconds / 2))
        }
        window.setContentSize(host.fittingSize)
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
        host.cacheDisplay(in: host.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        window.orderOut(nil)
    }

    /// The menu bar item at 2x, on light and dark strips like the real bar.
    private static func renderStatusItem(store: Store, to path: String) {
        let icon = StatusIcon.image(alert: store.troubleCount > 0,
                                    processes: store.troubleCount > 0 ? store.troubleCount : store.runningCount,
                                    ports: store.devPortCount)
        let pad: CGFloat = 8
        let size = NSSize(width: icon.size.width + pad * 2, height: (icon.size.height + pad) * 2)
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * 2), pixelsHigh: Int(size.height * 2),
                                   bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        for (index, (background, tint)) in [(NSColor(white: 0.93, alpha: 1), NSColor.black),
                                             (NSColor(white: 0.15, alpha: 1), NSColor.white)].enumerated() {
            let strip = NSRect(x: 0, y: CGFloat(index) * size.height / 2, width: size.width, height: size.height / 2)
            background.setFill()
            strip.fill()
            // Template images are drawn in the bar's color: tint a copy.
            let tinted = NSImage(size: icon.size, flipped: false) { rect in
                icon.draw(in: rect)
                tint.set()
                rect.fill(using: .sourceAtop)
                return true
            }
            tinted.draw(in: NSRect(x: pad, y: strip.minY + pad / 2, width: icon.size.width, height: icon.size.height))
        }
        NSGraphicsContext.restoreGraphicsState()
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }
}
