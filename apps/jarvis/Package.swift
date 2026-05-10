// swift-tools-version:5.9
//
// SwiftPM 工程文件——给 **没装完整 Xcode**（仅 CommandLineTools）的用户。
// CLT 自带 macOS SDK + SwiftUI，`swift build -c release` 即可出 binary。
// install-jarvis.sh 之后手动组 .app bundle（写 Info.plist + ad-hoc codesign + entitlements）。
//
// 装了 Xcode 的用户可用 `xcodegen generate` 走 .xcodeproj 路径，体验更完整
// （内置 IDE、SwiftUI Preview、模拟器）；两者并存不冲突——SwiftPM target 名 = xcodegen 名 =
// `Jarvis`，testTarget 名 `JarvisTests` 跟 project.yml 对齐。

import PackageDescription

let package = Package(
    name: "Jarvis",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Jarvis", targets: ["Jarvis"]),
    ],
    dependencies: [
        // WhisperKit（argmax-oss-swift 仓库 product 名为 WhisperKit）：替 SFSpeech zh-CN
        .package(url: "https://github.com/argmaxinc/argmax-oss-swift.git", from: "1.0.0"),
        // RealTimeCutVADLibrary：Silero v5 ONNX + WebRTC APM，替手写 trailing timer
        .package(url: "https://github.com/helloooideeeeea/RealTimeCutVADLibrary.git", from: "1.0.14"),
    ],
    targets: [
        .executableTarget(
            name: "Jarvis",
            dependencies: [
                .product(name: "WhisperKit", package: "argmax-oss-swift"),
                .product(name: "RealTimeCutVADLibrary", package: "RealTimeCutVADLibrary"),
            ],
            path: "Sources",
            sources: ["App", "Voice", "Net", "UI"],
            linkerSettings: [
                // RealTimeCutVADCXXLibrary.framework 是 binaryTarget xcframework，加载时 dyld
                // 用 `@rpath/...` 找；ad-hoc 装到 .app/Contents/MacOS 后没 default rpath，
                // 必须显式埋一条「相对 binary 上一级 ../Frameworks」的 rpath 才能起来。
                // 否则每次 cp binary 都要手 `install_name_tool -add_rpath ...` 兜，不可持续。
                .unsafeFlags([
                    "-Xlinker", "-rpath",
                    "-Xlinker", "@executable_path/../Frameworks",
                    // swift run 调试场景（直接跑 .build/release/Jarvis 而非 .app）时，
                    // framework 在同目录下；多埋一条让两种场景都能起。
                    "-Xlinker", "-rpath",
                    "-Xlinker", "@executable_path",
                    // 以及 SwiftPM build dir 内的兜底（.build/release/）。
                    "-Xlinker", "-rpath",
                    "-Xlinker", "@loader_path/../Frameworks",
                ]),
            ]
        ),
        .testTarget(
            name: "JarvisTests",
            dependencies: ["Jarvis"],
            path: "Tests"
        ),
    ]
)
