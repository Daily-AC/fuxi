// swift-tools-version:5.9
//
// SwiftPM 工程文件——给 **没装完整 Xcode**（仅 CommandLineTools）的用户。
// CLT 自带 macOS SDK + SwiftUI，`swift build -c release` 即可出 binary。
// install-jarvis.sh 之后手动组 .app bundle（写 Info.plist + ad-hoc codesign + entitlements）。
//
// 装了 Xcode 的用户可用 `xcodegen generate` 走 .xcodeproj 路径，体验更完整
// （内置 IDE、SwiftUI Preview、模拟器）；两者并存不冲突。

import PackageDescription

let package = Package(
    name: "Xuannv",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Xuannv", targets: ["Xuannv"]),
    ],
    targets: [
        .executableTarget(
            name: "Xuannv",
            path: "Sources",
            sources: ["App", "Voice", "Net", "UI"]
        ),
    ]
)
