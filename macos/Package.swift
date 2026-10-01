// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "Runa",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Runa", path: "Sources/Runa")
    ]
)
