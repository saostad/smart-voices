// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SmartVoices",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "SmartVoices", targets: ["SmartVoices"]),
        .executable(name: "svtext", targets: ["svtext"]),
        .library(name: "SmartVoicesCore", targets: ["SmartVoicesCore"]),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-markdown.git", from: "0.5.0"),
    ],
    targets: [
        .target(
            name: "SmartVoicesCore",
            dependencies: [.product(name: "Markdown", package: "swift-markdown")]
        ),
        .executableTarget(
            name: "SmartVoices",
            dependencies: ["SmartVoicesCore"]
        ),
        .executableTarget(
            name: "svtext",
            dependencies: ["SmartVoicesCore"]
        ),
        .testTarget(
            name: "SmartVoicesCoreTests",
            dependencies: ["SmartVoicesCore"]
        ),
    ]
)
