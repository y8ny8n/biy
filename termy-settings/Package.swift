// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "termy-settings",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "termy-settings",
            path: "Sources"
        )
    ]
)
