// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "CodeWith",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "CodeWith", targets: ["CodeWith"]),
    ],
    targets: [
        .executableTarget(
            name: "CodeWith",
            path: "Sources/CodeWith",
            resources: [],
            swiftSettings: [
                .swiftLanguageMode(.v5)
            ]
        ),
        .testTarget(
            name: "CodeWithTests",
            dependencies: ["CodeWith"],
            path: "Tests/CodeWithTests",
            swiftSettings: [
                .swiftLanguageMode(.v5)
            ]
        ),
    ]
)
