// Renders the runa app icon (macos/Resources/AppIcon.icns).
// Run: swift macos/scripts/make-icon.swift
import AppKit

let pulse: [CGPoint] = [
    CGPoint(x: 0.00, y: 0.56), CGPoint(x: 0.27, y: 0.56), CGPoint(x: 0.37, y: 0.30),
    CGPoint(x: 0.50, y: 0.82), CGPoint(x: 0.62, y: 0.18), CGPoint(x: 0.73, y: 0.56),
    CGPoint(x: 1.00, y: 0.56),
]

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    let s = CGFloat(px) / 1024
    // Flip to y-down so the shared pulse points read the same as in the app.
    ctx.translateBy(x: 0, y: CGFloat(px))
    ctx.scaleBy(x: s, y: -s)

    // Apple's icon grid: an 824pt squircle centred on a 1024pt canvas.
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let squircle = NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 12), blur: 28, color: NSColor.black.withAlphaComponent(0.35).cgColor)
    NSColor(white: 0.12, alpha: 1).setFill()
    squircle.fill()
    ctx.restoreGState()

    // Graphite body, lighter at the top.
    ctx.saveGState()
    squircle.addClip()
    let body = NSGradient(colors: [NSColor(white: 0.30, alpha: 1), NSColor(white: 0.10, alpha: 1)])!
    body.draw(in: tile, angle: 90)
    // Soft cool glow behind the pulse.
    let glow = NSGradient(colors: [NSColor(red: 0.35, green: 0.70, blue: 1.0, alpha: 0.28),
                                   NSColor(red: 0.35, green: 0.70, blue: 1.0, alpha: 0)])!
    glow.draw(fromCenter: CGPoint(x: 512, y: 540), radius: 0, toCenter: CGPoint(x: 512, y: 540), radius: 380, options: [])
    ctx.restoreGState()

    // Thin bevel highlight.
    NSColor.white.withAlphaComponent(0.14).setStroke()
    let rim = NSBezierPath(roundedRect: tile.insetBy(dx: 2, dy: 2), xRadius: 183, yRadius: 183)
    rim.lineWidth = 4
    rim.stroke()

    // The pulse: a white-to-cyan stroke with a glow.
    let area = CGRect(x: 220, y: 300, width: 584, height: 424)
    let path = CGMutablePath()
    for (i, p) in pulse.enumerated() {
        let point = CGPoint(x: area.minX + p.x * area.width, y: area.minY + p.y * area.height)
        i == 0 ? path.move(to: point) : path.addLine(to: point)
    }
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: 40, color: NSColor(red: 0.4, green: 0.8, blue: 1, alpha: 0.9).cgColor)
    ctx.addPath(path)
    ctx.setLineWidth(58)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    let stroke = NSGradient(colors: [.white, NSColor(red: 0.55, green: 0.86, blue: 1.0, alpha: 1)])!
    stroke.draw(in: area.insetBy(dx: -40, dy: -40), angle: 0)
    ctx.restoreGState()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    try! render(size).write(to: iconset.appendingPathComponent("icon_\(size)x\(size).png"))
    try! render(size * 2).write(to: iconset.appendingPathComponent("icon_\(size)x\(size)@2x.png"))
}
let resources = root.appendingPathComponent("Resources")
try! FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
try! render(1024).write(to: resources.appendingPathComponent("AppIcon.png"))
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", resources.appendingPathComponent("AppIcon.icns").path]
try! task.run()
task.waitUntilExit()
print("Wrote \(resources.path)/AppIcon.icns")
