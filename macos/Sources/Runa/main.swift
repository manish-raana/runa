import AppKit

// A plain AppKit entry point: everything lives in the status item, and a
// SwiftUI App would need a scene (an empty Settings window would pop up).
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    // NSApplication holds its delegate weakly; this scope keeps it alive
    // for as long as the app runs.
    withExtendedLifetime(delegate) { app.run() }
}
