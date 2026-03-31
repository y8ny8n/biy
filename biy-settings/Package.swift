// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "biy-settings",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "biy-settings",
            path: "Sources"
        )
    ]
)
