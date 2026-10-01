import AppKit
import Observation
import SwiftUI

/// The status item and its panel. A hand-rolled panel instead of
/// MenuBarExtra: MenuBarExtra keeps its first window size, so content that
/// grows or shrinks ends up floating in empty space.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = Store()
    private var statusItem: NSStatusItem!
    private var panel: MenuPanel!
    private var host: NSView!
    private var outsideClickMonitor: Any?
    private var lastClosed = Date.distantPast
    private var contentSize = CGSize(width: 368, height: 200)

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.target = self
        statusItem.button?.action = #selector(toggle)
        statusItem.button?.imagePosition = .imageLeading
        updateStatusItem()

        panel = MenuPanel()
        panel.onEscape = { [weak self] in self?.hide() }
        let root = MenuContent { [weak self] size in self?.resize(to: size) }
            .environment(store)
        let host = NSHostingView(rootView: root)
        host.sizingOptions = []
        self.host = host
        panel.contentView = PanelBackgroundView(content: host)

        NotificationCenter.default.addObserver(
            forName: NSWindow.didResignKeyNotification, object: panel, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.hide() }
        }

        if let directory = Snapshot.directory {
            Task { await Snapshot.run(store: store, to: directory) }
        }
    }

    /// Opening the app again (Finder, Spotlight) shows the panel.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !panel.isVisible { show() }
        return false
    }

    // MARK: Panel

    @objc private func toggle() {
        if panel.isVisible {
            hide()
        } else if Date.now.timeIntervalSince(lastClosed) > 0.25 {
            // A click on the status item first takes key from the panel,
            // which closes it; don't reopen it on the same click.
            show()
        }
    }

    private func show() {
        // Measure now so the panel opens at its final size.
        host.layoutSubtreeIfNeeded()
        let size = host.fittingSize
        if size.width > 0, size.height > 0 { contentSize = size }
        layoutPanel()
        panel.alphaValue = 0
        panel.makeKeyAndOrderFront(nil)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            panel.animator().alphaValue = 1
        }
        statusItem.button?.highlight(true)
        store.isMenuOpen = true
        outsideClickMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) {
            [weak self] _ in
            MainActor.assumeIsolated { self?.hide() }
        }
    }

    private func hide() {
        guard panel.isVisible else { return }
        panel.orderOut(nil)
        lastClosed = .now
        statusItem.button?.highlight(false)
        store.isMenuOpen = false
        if let monitor = outsideClickMonitor {
            NSEvent.removeMonitor(monitor)
            outsideClickMonitor = nil
        }
    }

    /// Called by the SwiftUI content whenever its natural size changes.
    private func resize(to size: CGSize) {
        guard size.width > 0, size.height > 0, size != contentSize else { return }
        contentSize = size
        if panel.isVisible { layoutPanel() }
    }

    /// Hangs the panel just under the status item, kept on screen.
    private func layoutPanel() {
        guard let button = statusItem.button, let buttonWindow = button.window else { return }
        let anchor = buttonWindow.convertToScreen(button.convert(button.bounds, to: nil))
        let screen = (buttonWindow.screen ?? NSScreen.main)?.visibleFrame ?? .zero
        var x = anchor.midX - contentSize.width / 2
        x = min(max(x, screen.minX + 8), screen.maxX - contentSize.width - 8)
        let top = anchor.minY - 6
        panel.setFrame(NSRect(x: x, y: top - contentSize.height, width: contentSize.width, height: contentSize.height),
                       display: true)
        panel.invalidateShadow()
    }

    // MARK: Status item

    private func updateStatusItem() {
        withObservationTracking {
            apply(running: store.runningCount, trouble: store.troubleCount, ports: store.devPortCount)
        } onChange: { [weak self] in
            Task { @MainActor in self?.updateStatusItem() }
        }
    }

    private func apply(running: Int, trouble: Int, ports: Int) {
        guard let button = statusItem.button else { return }
        button.image = StatusIcon.image(alert: trouble > 0, processes: trouble > 0 ? trouble : running, ports: ports)
        button.title = ""
        var tip = "Runa: \(running) running"
        if trouble > 0 { tip += ", \(trouble) stopped" }
        tip += ports == 1 ? " · 1 port listening" : " · \(ports) ports listening"
        button.toolTip = tip
    }
}

/// A borderless, non-activating panel: it takes key for keyboard input
/// without pulling the app (or its focus ring) forward.
final class MenuPanel: NSPanel {
    var onEscape: (() -> Void)?

    init() {
        super.init(contentRect: .zero,
                   styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
                   backing: .buffered, defer: true)
        isFloatingPanel = true
        level = .statusBar
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        hidesOnDeactivate = false
        isMovable = false
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        animationBehavior = .none
    }

    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) {
        onEscape?()
    }
}
