# Jarvis 桌宠 v0.3 · L1 MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实装玄女桌宠模式 v0.3 L1：水墨仙气立绘 panel 与现有禅意药丸双形态共存，用户可切；5 状态 ↔ 5 张 gpt-image-2 生成的 pose 图，crossfade 切换 + 衣袖飘动 Canvas overlay + 偶发微眨。后端 0 改动。

**Architecture:** UI 层平行新增 `apps/jarvis/Sources/UI/Pet/` 子目录，6 个 Swift 文件 + 资产目录。`AppState` 加 `uiMode` 字段；`AppDelegate` 启动时按 mode 起 CapsulePanel 或 PetPanel；用户从设置切换。

**Tech Stack:** SwiftUI + AppKit (NSPanel) + Canvas + TimelineView。无新依赖。

**Spec ref:** `docs/superpowers/specs/2026-05-11-jarvis-pet-design.md`

---

## File Map

新增：
- `apps/jarvis/Sources/UI/Pet/PetPanel.swift` — NSPanel 容器
- `apps/jarvis/Sources/UI/Pet/PoseAssetCatalog.swift` — 资产管理 + validate
- `apps/jarvis/Sources/UI/Pet/PetPoseView.swift` — 主视图 + crossfade
- `apps/jarvis/Sources/UI/Pet/SleeveCanvasOverlay.swift` — 衣袖飘动
- `apps/jarvis/Sources/UI/Pet/BlinkCoordinator.swift` — 微眨调度
- `apps/jarvis/Sources/UI/Pet/PetSweepOverlay.swift` — ack 横扫
- `apps/jarvis/Resources/Pet/poses/manifest.json` — 资产 manifest
- `apps/jarvis/Resources/Pet/poses/{idle,listening,thinking,speaking,ack}@2x.png` — 5 张 pose 图（art track 单独走 gpt-image-2）
- `apps/jarvis/Tests/PetPoseAssetCatalogTests.swift`
- `apps/jarvis/Tests/PetBlinkCoordinatorTests.swift`
- `apps/jarvis/Tests/PetSettingsTests.swift`

修改：
- `apps/jarvis/Sources/Net/Settings.swift` — 加 `uiMode` 字段
- `apps/jarvis/Sources/App/AppState.swift` — 加 `togglePanelMode()` 方法
- `apps/jarvis/Sources/App/AppDelegate.swift` — 双 panel 协调
- `apps/jarvis/Sources/UI/PreferencesView.swift` — 加「形态」radio
- `apps/jarvis/Package.swift` — 注册 `Resources/Pet/` 为 bundle resource

---

## Critical Pre-Task Setup

**Branch:** 在 `feat/fuxi-jarvis-mac` 上直接干（无需 worktree——单人 + 文件级 isolation 足够）。

**Build cmd:** `cd apps/jarvis && swift build -c debug`

**Test cmd:** `cd apps/jarvis && swift test`

**Smoke run cmd（验证 UI 起得来）:** `cd apps/jarvis && swift run Jarvis`（手动 ctrl+C 退）

**Commit 风格：** `feat(jarvis): 描述` / `fix(jarvis): 描述` / `test(jarvis): 描述`，与现 commit history 对齐。每个 Task 完成后单独 commit。

---

## Task 1: Settings.uiMode 字段

**Files:**
- Modify: `apps/jarvis/Sources/Net/Settings.swift`
- Create: `apps/jarvis/Tests/PetSettingsTests.swift`

**No dependencies.**

- [ ] **Step 1: Write failing test for uiMode default and persistence**

Create `apps/jarvis/Tests/PetSettingsTests.swift`:

```swift
import XCTest
@testable import Jarvis

final class PetSettingsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        UserDefaults.standard.removeObject(forKey: Settings.userDefaultsKey)
    }

    func test_default_uiMode_is_capsule() {
        XCTAssertEqual(Settings.default.uiMode, .capsule)
    }

    func test_uiMode_round_trip_pet() {
        var s = Settings.default
        s.uiMode = .pet
        s.save()
        let loaded = Settings.load()
        XCTAssertEqual(loaded.uiMode, .pet)
    }

    func test_legacy_uiMode_missing_falls_back_to_capsule() {
        // 老用户升级：UserDefaults 里 SettingsCodable 没有 uiMode 字段
        struct LegacyCodable: Codable {
            var baseURL: String
            var triggerMode: String
            var hotkey: HotkeyCombo
            var ttsVoice: String
        }
        let legacy = LegacyCodable(
            baseURL: "https://im.qmledmq.cn:8443",
            triggerMode: "both",
            hotkey: HotkeyCombo(modifiers: [.control, .option], keyCode: 0x2E),
            ttsVoice: ""
        )
        let data = try! JSONEncoder().encode(legacy)
        UserDefaults.standard.set(data, forKey: Settings.userDefaultsKey)
        let loaded = Settings.load()
        XCTAssertEqual(loaded.uiMode, .capsule)
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd apps/jarvis && swift test --filter PetSettingsTests
```

Expected: FAIL with "value of type 'Settings' has no member 'uiMode'"

- [ ] **Step 3: Add uiMode field to Settings**

Edit `apps/jarvis/Sources/Net/Settings.swift`:

After `var ttsRemoteURL: String` add:

```swift
    /// UI 形态：药丸（默认，老用户无感升级）/ 立绘（桌宠模式）。
    var uiMode: UIMode

    enum UIMode: String, CaseIterable, Identifiable, Codable {
        /// 现有禅意药丸——160×32 圆角胶囊悬浮 dock 上方。
        case capsule
        /// 立绘桌宠——~280×420 pose 图 panel，仙气线条。
        case pet
        var id: String { rawValue }
        var label: String {
            switch self {
            case .capsule: return "药丸"
            case .pet: return "立绘"
            }
        }
    }
```

In `Settings.default` initializer add at the end:

```swift
        ,
        // 默认走药丸——老用户升级无感；新用户进设置切立绘。
        uiMode: .capsule
```

In `SettingsCodable` private struct, add:

```swift
    var uiMode: String?
```

In `Settings.load()`:

```swift
            ttsRemoteURL: dec?.ttsRemoteURL ?? Self.default.ttsRemoteURL,
            // 老 UserDefaults 没 uiMode → 回 capsule，无感
            uiMode: dec.flatMap { UIMode(rawValue: $0.uiMode ?? "") } ?? Self.default.uiMode
```

In `Settings.save()` `SettingsCodable` init:

```swift
            ttsRemoteURL: ttsRemoteURL,
            uiMode: uiMode.rawValue
```

- [ ] **Step 4: Run tests to verify pass**

```bash
cd apps/jarvis && swift test --filter PetSettingsTests
```

Expected: PASS 3/3

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis/Sources/Net/Settings.swift apps/jarvis/Tests/PetSettingsTests.swift
git commit -m "feat(jarvis): Settings.uiMode 字段 + 老用户兼容回退到 capsule"
```

---

## Task 2: PoseAssetCatalog + manifest

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/PoseAssetCatalog.swift`
- Create: `apps/jarvis/Resources/Pet/poses/manifest.json`
- Create: `apps/jarvis/Tests/PetPoseAssetCatalogTests.swift`
- Modify: `apps/jarvis/Package.swift` （注册 resources）

**No dependencies on other tasks**（独立模块）。

- [ ] **Step 1: Write failing test**

Create `apps/jarvis/Tests/PetPoseAssetCatalogTests.swift`:

```swift
import XCTest
@testable import Jarvis

final class PetPoseAssetCatalogTests: XCTestCase {
    func test_phase_to_pose_mapping_complete() {
        // 每个 AppState.VoicePhase 必须有 pose
        let phases: [AppState.VoicePhase] = [.idle, .listening, .sending, .waiting, .speaking]
        for p in phases {
            XCTAssertNotNil(PoseAssetCatalog.poseName(for: p), "缺 pose mapping for \(p)")
        }
    }

    func test_sending_and_waiting_share_thinking_pose() {
        XCTAssertEqual(PoseAssetCatalog.poseName(for: .sending), "thinking")
        XCTAssertEqual(PoseAssetCatalog.poseName(for: .waiting), "thinking")
    }

    func test_validate_returns_false_when_assets_missing() {
        // 单测环境下 Resources/Pet/poses 大概率没图——validate 必须返 false 不崩
        let catalog = PoseAssetCatalog()
        // 不强求 true/false——只要不抛
        _ = catalog.validate()
    }

    func test_validate_with_in_memory_bundle_all_present_returns_true() {
        // 用 mock bundle 注入 5 张 dummy 图，validate 应返 true
        let mock = MockPoseBundle(present: ["idle", "listening", "thinking", "speaking", "ack"])
        let catalog = PoseAssetCatalog(bundle: mock)
        XCTAssertTrue(catalog.validate())
    }

    func test_validate_with_one_missing_returns_false() {
        let mock = MockPoseBundle(present: ["idle", "listening", "thinking", "speaking"])  // ack 缺
        let catalog = PoseAssetCatalog(bundle: mock)
        XCTAssertFalse(catalog.validate())
    }
}

/// 测试用 bundle stub
private final class MockPoseBundle: PoseBundleSource {
    let present: Set<String>
    init(present: [String]) { self.present = Set(present) }
    func url(forPose name: String) -> URL? {
        present.contains(name) ? URL(fileURLWithPath: "/tmp/mock-\(name).png") : nil
    }
}
```

- [ ] **Step 2: Run test to verify fail**

```bash
cd apps/jarvis && swift test --filter PetPoseAssetCatalogTests
```

Expected: FAIL "cannot find PoseAssetCatalog"

- [ ] **Step 3: Implement PoseAssetCatalog**

Create dir first: `mkdir -p apps/jarvis/Sources/UI/Pet apps/jarvis/Resources/Pet/poses`

Create `apps/jarvis/Sources/UI/Pet/PoseAssetCatalog.swift`:

```swift
import Foundation
import AppKit

/// 抽象 pose 图查找入口——production 走 Bundle.module，单测可注 mock。
protocol PoseBundleSource {
    func url(forPose name: String) -> URL?
}

/// 默认 bundle source——SwiftPM target 的 Bundle.module 下找 Pet/poses/<name>@2x.png。
struct DefaultPoseBundle: PoseBundleSource {
    func url(forPose name: String) -> URL? {
        Bundle.module.url(forResource: "Pet/poses/\(name)@2x", withExtension: "png")
    }
}

/// pose 资产管理 + validate。
///
/// 5 个固定 pose name：idle / listening / thinking / speaking / ack。
/// `AppState.VoicePhase` 5 状态映射到 4 张图（sending+waiting 共用 thinking）+ ack 由 ackPulse 单独触发。
struct PoseAssetCatalog {
    static let poseNames: [String] = ["idle", "listening", "thinking", "speaking", "ack"]

    let bundle: PoseBundleSource

    init(bundle: PoseBundleSource = DefaultPoseBundle()) {
        self.bundle = bundle
    }

    /// AppState.VoicePhase → pose 图基名
    static func poseName(for phase: AppState.VoicePhase) -> String? {
        switch phase {
        case .idle:      return "idle"
        case .listening: return "listening"
        case .sending, .waiting: return "thinking"
        case .speaking:  return "speaking"
        }
    }

    /// 启动校验——5 张全在返 true，缺任何一张返 false（调用方应回退 capsule 模式）
    func validate() -> Bool {
        for name in Self.poseNames {
            if bundle.url(forPose: name) == nil {
                return false
            }
        }
        return true
    }

    /// 加载 NSImage——失败返 nil（调用方应保留前一帧不切）
    func image(for poseName: String) -> NSImage? {
        guard let url = bundle.url(forPose: poseName) else { return nil }
        return NSImage(contentsOf: url)
    }
}
```

- [ ] **Step 4: Add Resources to Package.swift**

Edit `apps/jarvis/Package.swift`. Change `executableTarget`:

```swift
        .executableTarget(
            name: "Jarvis",
            dependencies: [
                .product(name: "WhisperKit", package: "argmax-oss-swift"),
                .product(name: "RealTimeCutVADLibrary", package: "RealTimeCutVADLibrary"),
            ],
            path: "Sources",
            sources: ["App", "Voice", "Net", "UI"],
            resources: [
                // pose 资产 —— gpt-image-2 生的 PNG 放这。validate 失败时 jarvis
                // 自动回退 capsule 模式，不阻塞编译。
                .copy("../Resources/Pet"),
            ],
            linkerSettings: [
                // ... 保持原 linkerSettings 不变
            ]
        ),
```

⚠️ **注意**：SwiftPM `path: "Sources"` 限定了 source 根，resources 路径需相对该根 → `../Resources/Pet`。验证一下 build 不爆。如果 SwiftPM 拒绝 `..` 路径，改方案：把 Resources 移到 `apps/jarvis/Sources/Resources/Pet/` 下并改回 `.copy("Resources/Pet")`。

- [ ] **Step 5: Create empty manifest placeholder**

Create `apps/jarvis/Resources/Pet/poses/manifest.json`:

```json
{
  "version": 1,
  "poses": [
    {"name": "idle",      "file": "idle@2x.png",      "anchor": [0.5, 0.5]},
    {"name": "listening", "file": "listening@2x.png", "anchor": [0.5, 0.5]},
    {"name": "thinking",  "file": "thinking@2x.png",  "anchor": [0.5, 0.5]},
    {"name": "speaking",  "file": "speaking@2x.png",  "anchor": [0.5, 0.5]},
    {"name": "ack",       "file": "ack@2x.png",       "anchor": [0.5, 0.5]}
  ]
}
```

（manifest 当前只是占位 + 给 art track 的人参照命名；后续如果要 anchor 微调可读它，MVP PetPoseView 先不读。）

- [ ] **Step 6: Run tests to verify pass**

```bash
cd apps/jarvis && swift test --filter PetPoseAssetCatalogTests
```

Expected: PASS 5/5

- [ ] **Step 7: Commit**

```bash
git add apps/jarvis/Sources/UI/Pet/PoseAssetCatalog.swift \
        apps/jarvis/Resources/Pet/poses/manifest.json \
        apps/jarvis/Tests/PetPoseAssetCatalogTests.swift \
        apps/jarvis/Package.swift
git commit -m "feat(jarvis): PoseAssetCatalog + manifest + Resources/Pet 注册"
```

---

## Task 3: BlinkCoordinator

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/BlinkCoordinator.swift`
- Create: `apps/jarvis/Tests/PetBlinkCoordinatorTests.swift`

**No dependencies.**

- [ ] **Step 1: Write failing test**

Create `apps/jarvis/Tests/PetBlinkCoordinatorTests.swift`:

```swift
import XCTest
@testable import Jarvis

@MainActor
final class PetBlinkCoordinatorTests: XCTestCase {
    func test_random_interval_in_range() {
        // 随机 100 次都在 4-7s
        for _ in 0..<100 {
            let interval = BlinkCoordinator.randomInterval()
            XCTAssertGreaterThanOrEqual(interval, 4.0)
            XCTAssertLessThanOrEqual(interval, 7.0)
        }
    }

    func test_blink_trigger_increments() {
        let c = BlinkCoordinator()
        let before = c.blinkTrigger
        c.fireForTesting()
        XCTAssertEqual(c.blinkTrigger, before + 1)
    }
}
```

- [ ] **Step 2: Run test to verify fail**

```bash
cd apps/jarvis && swift test --filter PetBlinkCoordinatorTests
```

- [ ] **Step 3: Implement BlinkCoordinator**

Create `apps/jarvis/Sources/UI/Pet/BlinkCoordinator.swift`:

```swift
import Foundation
import Combine

/// 偶发微眨的调度器——4-7s 随机间隔触发一个 0.15s blink。
/// PetPoseView 监听 `$blinkTrigger` `.onChange` 触发渐隐线 overlay。
///
/// 用 Timer + RunLoop.main——不引 Combine `Timer.publish` 是因为它 cancel 时
/// 需 store cancellable，复杂度对这点功能不值。
@MainActor
final class BlinkCoordinator: ObservableObject {
    /// 自增计数器——每次眨眼 +1。SwiftUI .onChange 监听其值变化触发 overlay 重建。
    @Published private(set) var blinkTrigger: Int = 0

    private var timer: Timer?

    /// 启动后台调度——AppDelegate 切到 pet 模式时调；切回 capsule 时 stop。
    func start() {
        scheduleNext()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    /// 单测用 trigger（绕开 Timer，直接 increment）
    func fireForTesting() {
        blinkTrigger &+= 1
    }

    static func randomInterval() -> TimeInterval {
        Double.random(in: 4.0...7.0)
    }

    private func scheduleNext() {
        let interval = Self.randomInterval()
        let t = Timer(timeInterval: interval, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self = self else { return }
                self.blinkTrigger &+= 1
                self.scheduleNext()
            }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

```bash
cd apps/jarvis && swift test --filter PetBlinkCoordinatorTests
```

Expected: PASS 2/2

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis/Sources/UI/Pet/BlinkCoordinator.swift \
        apps/jarvis/Tests/PetBlinkCoordinatorTests.swift
git commit -m "feat(jarvis): BlinkCoordinator 4-7s 随机微眨调度"
```

---

## Task 4: SleeveCanvasOverlay

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/SleeveCanvasOverlay.swift`

**No dependencies.** No tests（pure SwiftUI Canvas 视觉 overlay，单测意义低；smoke 验证靠手动 `swift run Jarvis`）。

- [ ] **Step 1: Implement**

Create `apps/jarvis/Sources/UI/Pet/SleeveCanvasOverlay.swift`:

```swift
import SwiftUI

/// 衣袖飘动效果——三层 sin 叠加画一道淡墨气韵线。
///
/// 视觉语言：
/// - 主基调跟 ZenStyle.inkTeal 同色
/// - 三层 sin 振幅 / 频率 / 相位错开形成"自然"飘动
/// - amplitudeBoost 0~1，listening 时让 PetPoseView 透传 audioLevel 让它跟麦克风电平耦合
/// - 透明度 0.5，避免抢主体 pose
struct SleeveCanvasOverlay: View {
    let amplitudeBoost: Double  // 0~1，外部调制（idle 给 0，listening 给 audioLevel）
    let scheme: ColorScheme

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            Canvas { ctx, size in
                let now = timeline.date.timeIntervalSinceReferenceDate
                let baseColor = ZenStyle.inkTeal(scheme).opacity(0.5)

                // 三层错开
                for layer in 0..<3 {
                    let layerPhase = Double(layer) * 1.7
                    let layerFreq = 0.5 + Double(layer) * 0.3
                    let layerAmp = (4.0 + Double(layer) * 2.0) * (1.0 + amplitudeBoost)
                    var path = Path()
                    let yMid = size.height * (0.65 + Double(layer) * 0.05)
                    let step: CGFloat = 4
                    var x: CGFloat = 0
                    path.move(to: CGPoint(x: 0, y: yMid))
                    while x < size.width {
                        let phase = Double(x) / 40.0 + now * layerFreq + layerPhase
                        let y = yMid + sin(phase) * layerAmp
                        path.addLine(to: CGPoint(x: x, y: y))
                        x += step
                    }
                    ctx.stroke(path, with: .color(baseColor), lineWidth: 0.8)
                }
            }
        }
        .allowsHitTesting(false)
    }
}
```

- [ ] **Step 2: Verify build**

```bash
cd apps/jarvis && swift build -c debug
```

Expected: build SUCCESS

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis/Sources/UI/Pet/SleeveCanvasOverlay.swift
git commit -m "feat(jarvis): SleeveCanvasOverlay 三层 sin 衣袖飘动"
```

---

## Task 5: PetSweepOverlay

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/PetSweepOverlay.swift`

**No dependencies.** 直接复用现 `SweepOverlay` 视觉语言。

- [ ] **Step 1: Implement**

Create `apps/jarvis/Sources/UI/Pet/PetSweepOverlay.swift`:

```swift
import SwiftUI

/// 立绘 ack 横扫 overlay——同 CapsuleStateView 的 SweepOverlay 视觉语言，
/// 但适配立绘尺寸（更宽，扫得更慢）。
///
/// 由 PetPoseView 监听 AppState.ackPulse `.id(state.ackPulse)` 触发 onAppear 重建，
/// 与 earcon 200ms × 1.5 = 300ms 视觉同步（立绘大要扫得久点才看得到）。
struct PetSweepOverlay: View {
    let scheme: ColorScheme
    @State private var progress: Double = 0
    @State private var visible = false

    static let sweepDuration: Double = 0.3  // 立绘比药丸扫得慢

    var body: some View {
        GeometryReader { geo in
            let lineHeight: CGFloat = 2.0
            let xPos = geo.size.width * progress
            Rectangle()
                .fill(ZenStyle.inkTeal(scheme))
                .frame(width: geo.size.width * 0.4, height: lineHeight)
                .position(x: xPos, y: geo.size.height / 2)
                .opacity(visible ? 0.7 : 0.0)
        }
        .onAppear {
            withAnimation(.easeIn(duration: Self.sweepDuration / 2)) {
                visible = true
            }
            withAnimation(.linear(duration: Self.sweepDuration)) {
                progress = 1.0
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.sweepDuration / 2) {
                withAnimation(.easeOut(duration: Self.sweepDuration / 2)) {
                    visible = false
                }
            }
        }
        .allowsHitTesting(false)
    }
}
```

- [ ] **Step 2: Verify build**

```bash
cd apps/jarvis && swift build -c debug
```

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis/Sources/UI/Pet/PetSweepOverlay.swift
git commit -m "feat(jarvis): PetSweepOverlay ack 墨笔横扫（立绘版 300ms）"
```

---

## Task 6: PetPoseView

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/PetPoseView.swift`

**Depends on:** Task 2 (PoseAssetCatalog), Task 3 (BlinkCoordinator), Task 4 (SleeveCanvasOverlay), Task 5 (PetSweepOverlay).

无单元测试（SwiftUI 视图集成靠 smoke run 验证）。

- [ ] **Step 1: Implement**

Create `apps/jarvis/Sources/UI/Pet/PetPoseView.swift`:

```swift
import SwiftUI
import AppKit

/// 立绘桌宠主视图——监听 AppState.phase + ackPulse + audioLevel，
/// 渲染对应 pose 图 + 衣袖飘动 + 偶发微眨 + ack 横扫。
///
/// 资产缺失时（PoseAssetCatalog.image == nil）显示半透明 placeholder——
/// 不让窗口空白，给用户 visual cue 资产没装好。
struct PetPoseView: View {
    @ObservedObject var state: AppState
    @StateObject var blink = BlinkCoordinator()
    @Environment(\.colorScheme) private var scheme

    private let catalog = PoseAssetCatalog()

    var body: some View {
        ZStack {
            // 主 pose 图层 —— crossfade transition
            poseImage
                .id(state.phase)  // phase 变 → 整个 view 重建触发 transition
                .transition(.opacity.animation(.easeInOut(duration: 0.25)))

            // 衣袖飘动（idle/listening/thinking/speaking 都有；ack 期间不渲染避撞）
            SleeveCanvasOverlay(
                amplitudeBoost: state.phase == .listening ? state.audioLevel : 0,
                scheme: scheme
            )

            // 偶发微眨——仅 idle/listening 显示（thinking 闭目不眨，speaking 不闲）
            if state.phase == .idle || state.phase == .listening {
                BlinkLineOverlay(scheme: scheme)
                    .id(blink.blinkTrigger)
            }

            // ack 横扫——key by ackPulse 触发 onAppear 重建
            if state.ackPulse > 0 {
                PetSweepOverlay(scheme: scheme)
                    .id(state.ackPulse)
            }
        }
        .frame(width: 280, height: 420)
        .onAppear { blink.start() }
        .onDisappear { blink.stop() }
        .animation(.easeInOut(duration: 0.25), value: state.phase)
    }

    @ViewBuilder
    private var poseImage: some View {
        let poseName = PoseAssetCatalog.poseName(for: state.phase) ?? "idle"
        if let nsImg = catalog.image(for: poseName) {
            Image(nsImage: nsImg)
                .resizable()
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
        } else {
            // 资产缺失 placeholder——半透明叉 + label，让用户一眼看出问题
            ZStack {
                Rectangle()
                    .fill(ZenStyle.paper(scheme).opacity(0.3))
                VStack(spacing: 8) {
                    Image(systemName: "questionmark.square.dashed")
                        .font(.system(size: 48, weight: .light))
                        .foregroundStyle(ZenStyle.inkTeal(scheme).opacity(0.5))
                    Text("立绘资产未就绪")
                        .font(.caption)
                        .foregroundStyle(ZenStyle.inkTeal(scheme).opacity(0.5))
                    Text("（\(poseName)@2x.png 缺失）")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// 微眨一闪而过的渐隐线——0.15s 在双眼区横过。
/// 简化做法：在视图顶 1/3 位置画一条水平线 alpha 1→0。
private struct BlinkLineOverlay: View {
    let scheme: ColorScheme
    @State private var visible: Bool = false

    var body: some View {
        GeometryReader { geo in
            let y = geo.size.height * 0.32  // 假定脸部在视图上 1/3 处（与 art track prompt 对齐）
            Rectangle()
                .fill(ZenStyle.inkTeal(scheme))
                .frame(width: geo.size.width * 0.18, height: 0.8)
                .position(x: geo.size.width * 0.5, y: y)
                .opacity(visible ? 0.6 : 0.0)
        }
        .onAppear {
            withAnimation(.easeIn(duration: 0.06)) {
                visible = true
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) {
                withAnimation(.easeOut(duration: 0.07)) {
                    visible = false
                }
            }
        }
        .allowsHitTesting(false)
    }
}
```

- [ ] **Step 2: Verify build**

```bash
cd apps/jarvis && swift build -c debug
```

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis/Sources/UI/Pet/PetPoseView.swift
git commit -m "feat(jarvis): PetPoseView 主视图 + 资产缺失 placeholder"
```

---

## Task 7: PetPanel

**Files:**
- Create: `apps/jarvis/Sources/UI/Pet/PetPanel.swift`

**Depends on:** Task 6 (PetPoseView), Task 1 (Settings.uiMode for default position UserDefaults key)。

无单元测试（NSPanel 集成层，靠 smoke run + 手动拖动验证）。

- [ ] **Step 1: Implement**

Create `apps/jarvis/Sources/UI/Pet/PetPanel.swift`:

```swift
import AppKit
import SwiftUI
import OSLog

/// 立绘桌宠悬浮窗——~280×420 NSPanel，accessory 模式。
///
/// 关键约束：
/// - `.borderless + .nonactivatingPanel` —— 不抢焦点
/// - `.floating` level —— 浮在 dock 上方
/// - `collectionBehavior = [.canJoinAllSpaces, .stationary]`
/// - 可拖动：CGPoint 写 UserDefaults，下次启动恢复
/// - 屏幕分辨率变 / Space 切换 → 检测当前位置是否还在屏内，否则重置
@MainActor
final class PetPanel: NSPanel {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "pet-panel")
    private let state: AppState
    private var hostingView: NSHostingView<PetPoseView>?

    static let panelWidth: CGFloat = 280
    static let panelHeight: CGFloat = 420
    static let dockGap: CGFloat = 12
    static let positionKey = "cn.qmledmq.fuxi.xuannv.petPanel.origin"

    init(state: AppState) {
        self.state = state
        let rect = NSRect(x: 0, y: 0, width: Self.panelWidth, height: Self.panelHeight)
        super.init(
            contentRect: rect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        hidesOnDeactivate = false
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        isOpaque = false
        backgroundColor = .clear
        hasShadow = false  // 立绘自带 alpha，再加阴影会出怪框
        isExcludedFromWindowsMenu = true
        isMovable = true
        isMovableByWindowBackground = true  // 拖任意位置都可移

        let host = NSHostingView(rootView: PetPoseView(state: state))
        host.frame = rect
        host.autoresizingMask = [.width, .height]
        contentView = host
        hostingView = host

        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSApplication.didChangeScreenParametersNotification,
            object: nil
        )
        NSWorkspace.shared.notificationCenter.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSWorkspace.activeSpaceDidChangeNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleDidMove),
            name: NSWindow.didMoveNotification,
            object: self
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// 右键弹「设置 / 切回药丸 / 退出」—— PetPanel 复用 CapsulePanel 的菜单语义
    override func rightMouseDown(with event: NSEvent) {
        let menu = NSMenu()

        let settingsItem = NSMenuItem(
            title: "设置…",
            action: #selector(AppDelegate.openSettings),
            keyEquivalent: ","
        )
        settingsItem.keyEquivalentModifierMask = [.command]
        settingsItem.target = NSApp.delegate
        menu.addItem(settingsItem)

        menu.addItem(NSMenuItem.separator())

        let switchItem = NSMenuItem(
            title: "切回药丸",
            action: #selector(AppDelegate.switchToCapsule),
            keyEquivalent: ""
        )
        switchItem.target = NSApp.delegate
        menu.addItem(switchItem)

        menu.addItem(NSMenuItem.separator())

        let quitItem = NSMenuItem(
            title: "退出玄女",
            action: #selector(AppDelegate.quitApp),
            keyEquivalent: "q"
        )
        quitItem.keyEquivalentModifierMask = [.command]
        quitItem.target = NSApp.delegate
        menu.addItem(quitItem)

        NSMenu.popUpContextMenu(menu, with: event, for: contentView ?? NSView())
    }

    /// show—— restore saved CGPoint，否则 dock 上方居中
    func show() {
        if let saved = loadSavedOrigin(), originIsOnScreen(saved) {
            setFrameOrigin(saved)
        } else {
            repositionAboveDock()
        }
        orderFront(nil)
        logger.notice("pet panel shown at \(self.frame.debugDescription, privacy: .public)")
    }

    @objc private func handleScreenChange() {
        // 屏幕变了——若当前位置不在任何屏 visibleFrame 内，重置到 dock 上方
        if !originIsOnScreen(frame.origin) {
            repositionAboveDock()
        }
    }

    @objc private func handleDidMove(_ note: Notification) {
        // 用户拖完——存 UserDefaults
        let p = frame.origin
        let dict: [String: Double] = ["x": Double(p.x), "y": Double(p.y)]
        UserDefaults.standard.set(dict, forKey: Self.positionKey)
    }

    private func loadSavedOrigin() -> NSPoint? {
        guard let dict = UserDefaults.standard.dictionary(forKey: Self.positionKey),
              let x = dict["x"] as? Double,
              let y = dict["y"] as? Double
        else { return nil }
        return NSPoint(x: x, y: y)
    }

    /// 检测 origin 是否落在任意 NSScreen 的 visibleFrame 内（左下角点）
    private func originIsOnScreen(_ origin: NSPoint) -> Bool {
        for screen in NSScreen.screens {
            if screen.visibleFrame.contains(origin) {
                return true
            }
        }
        return false
    }

    private func repositionAboveDock() {
        guard let screen = NSScreen.main else {
            logger.warning("无主屏，跳过 reposition")
            return
        }
        let visible = screen.visibleFrame
        let x = visible.midX - Self.panelWidth / 2
        let y = visible.minY + Self.dockGap
        setFrameOrigin(NSPoint(x: x, y: y))
    }
}
```

- [ ] **Step 2: Verify build**

```bash
cd apps/jarvis && swift build -c debug
```

⚠️ AppDelegate.switchToCapsule 还没定义，build 会 fail。这个 selector 在 Task 8 加。**临时**：注释掉「切回药丸」menu item 让 Task 7 build 通过；Task 8 完成后 uncomment。或者：Task 7 直接跳到 Task 8 一起 commit。

**推荐**：把 Task 7 的 commit 跟 Task 8 合并——`switchToCapsule` selector 跟 PetPanel 强耦合。

- [ ] **Step 3: 暂不 commit，等 Task 8 一起**

---

## Task 8: AppDelegate / AppState 双 panel 协调

**Files:**
- Modify: `apps/jarvis/Sources/App/AppState.swift`
- Modify: `apps/jarvis/Sources/App/AppDelegate.swift`

**Depends on:** Task 1 (Settings.uiMode), Task 7 (PetPanel)。

- [ ] **Step 1: AppState 加 togglePanelMode 方法**

Edit `apps/jarvis/Sources/App/AppState.swift`. After `enterIdle()` 方法加：

```swift
    /// 切 UI 形态——AppDelegate 依此 swap NSPanel。
    /// 持久化到 UserDefaults，重启后保留。
    func setUIMode(_ mode: Settings.UIMode) {
        guard settings.uiMode != mode else { return }
        settings.uiMode = mode
        settings.save()
        logger.notice("uiMode 切到 \(mode.rawValue, privacy: .public)")
    }
```

- [ ] **Step 2: AppDelegate 加 panel 协调**

Edit `apps/jarvis/Sources/App/AppDelegate.swift`：

替换 `private var capsulePanel: CapsulePanel?` 为：

```swift
    private var capsulePanel: CapsulePanel?
    private var petPanel: PetPanel?
```

替换 `applicationDidFinishLaunching` 末段（`let panel = CapsulePanel...` 起）为：

```swift
        // 按 settings.uiMode 起对应 panel
        capsulePanel = CapsulePanel(state: AppState.shared)
        petPanel = PetPanel(state: AppState.shared)
        applyUIMode(AppState.shared.settings.uiMode)

        installStatusItem()
        settingsController = SettingsWindowController(state: AppState.shared)
        logger.notice("玄女 ready (uiMode=\(AppState.shared.settings.uiMode.rawValue, privacy: .public))")
```

加新方法 `applyUIMode`：

```swift
    /// 切换 panel 显示——隐当前，显新。capsule 始终保留实例（轻量），不销毁。
    func applyUIMode(_ mode: Settings.UIMode) {
        // 资产缺失保护：用户切 pet 但资产没装好 → 强制 capsule + 设置回写 + 用户 alert
        if mode == .pet {
            let catalog = PoseAssetCatalog()
            if !catalog.validate() {
                logger.warning("立绘资产校验失败，强制回 capsule")
                AppState.shared.setUIMode(.capsule)
                showAssetMissingAlert()
                applyUIMode(.capsule)
                return
            }
        }

        switch mode {
        case .capsule:
            petPanel?.orderOut(nil)
            capsulePanel?.show()
        case .pet:
            capsulePanel?.orderOut(nil)
            petPanel?.show()
        }
    }

    private func showAssetMissingAlert() {
        let alert = NSAlert()
        alert.messageText = "立绘资产未就绪"
        alert.informativeText = "Resources/Pet/poses/ 下缺 5 张 pose 图，已暂时回到药丸模式。资产装好后重启 App 再切。"
        alert.alertStyle = .warning
        alert.addButton(withTitle: "好")
        alert.runModal()
    }

    /// PetPanel 右键菜单「切回药丸」入口
    @objc func switchToCapsule() {
        AppState.shared.setUIMode(.capsule)
        applyUIMode(.capsule)
    }

    /// CapsulePanel 右键菜单「切到立绘」入口（在 CapsulePanel rightMouseDown 加 menu item）
    @objc func switchToPet() {
        AppState.shared.setUIMode(.pet)
        applyUIMode(.pet)
    }
```

- [ ] **Step 3: CapsulePanel 加「切到立绘」menu item**

Edit `apps/jarvis/Sources/UI/CapsulePanel.swift`，在 `rightMouseDown` 里 `menu.addItem(NSMenuItem.separator())` 之前加：

```swift
        let switchItem = NSMenuItem(
            title: "切到立绘",
            action: #selector(AppDelegate.switchToPet),
            keyEquivalent: ""
        )
        switchItem.target = NSApp.delegate
        menu.addItem(switchItem)

        menu.addItem(NSMenuItem.separator())
```

（保留原有的「设置」和「退出」item，只在中间插一个「切到立绘」+ separator。）

- [ ] **Step 4: Verify build**

```bash
cd apps/jarvis && swift build -c debug
```

Expected: SUCCESS

- [ ] **Step 5: Commit Task 7 + Task 8 一起**

```bash
git add apps/jarvis/Sources/UI/Pet/PetPanel.swift \
        apps/jarvis/Sources/App/AppState.swift \
        apps/jarvis/Sources/App/AppDelegate.swift \
        apps/jarvis/Sources/UI/CapsulePanel.swift
git commit -m "feat(jarvis): PetPanel + AppDelegate 双 panel 协调 + 右键互切"
```

---

## Task 9: PreferencesView 形态 radio

**Files:**
- Modify: `apps/jarvis/Sources/UI/PreferencesView.swift`

**Depends on:** Task 1 (uiMode), Task 8 (applyUIMode)。

- [ ] **Step 1: 加「形态」tab 或并入「连接」**

简化方案：并入 connectionTab 顶部。Edit `apps/jarvis/Sources/UI/PreferencesView.swift`:

替换 `private var connectionTab: some View` 整段：

```swift
    private var connectionTab: some View {
        Form {
            Picker("形态", selection: $draft.uiMode) {
                ForEach(Settings.UIMode.allCases) { m in
                    Text(m.label).tag(m)
                }
            }
            .pickerStyle(.radioGroup)

            Divider()

            TextField("fuxi-im 地址", text: $draft.baseURL)
                .textFieldStyle(.roundedBorder)
            TextField("Pair Token", text: $draft.pairToken)
                .textFieldStyle(.roundedBorder)
            HStack {
                Text("当前状态：")
                Text(state.connectionStatus).foregroundStyle(.secondary)
            }
            Text("PWA 设置里生成 token，粘贴这里，App 自动重连。立绘需先在 apps/jarvis/Resources/Pet/poses/ 装 5 张 PNG。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
```

- [ ] **Step 2: 改 onChange 触发 panel swap**

把现 `.onChange(of: draft) { _, new in ... }` 替换为：

```swift
        .onChange(of: draft) { old, new in
            new.save()
            state.settings = new
            state.fuxiClient?.updateSettings(new)
            state.hotkey?.install(combo: new.hotkey)
            state.reloadWake()
            // uiMode 变了让 AppDelegate swap panel
            if old.uiMode != new.uiMode {
                if let delegate = NSApp.delegate as? AppDelegate {
                    delegate.applyUIMode(new.uiMode)
                }
            }
        }
```

- [ ] **Step 3: Verify build + 手测**

```bash
cd apps/jarvis && swift build -c debug
cd apps/jarvis && swift run Jarvis
# 手动：右键药丸 → 设置 → 切「立绘」radio → 应弹「资产未就绪」alert（因为 PNG 还没装）→ 切回药丸
```

Expected: 切换 radio 弹 alert + 自动回 capsule。

- [ ] **Step 4: Commit**

```bash
git add apps/jarvis/Sources/UI/PreferencesView.swift
git commit -m "feat(jarvis): PreferencesView 加「形态」radio + 实时切 panel"
```

---

## Task 10: Smoke verify + 端到端检查

**Files:**
- 仅运行测试 + 手测，无代码改动

**Depends on:** Task 1-9 全部完成。

- [ ] **Step 1: Run all tests**

```bash
cd apps/jarvis && swift test
```

Expected: 所有现有 + 新增测试全 PASS。

- [ ] **Step 2: Build release**

```bash
cd apps/jarvis && swift build -c release
```

Expected: SUCCESS

- [ ] **Step 3: Smoke run**

```bash
cd apps/jarvis && swift run Jarvis &
sleep 5
# 验证：dock 上方应出现药丸（默认 capsule 模式）
# 右键药丸 → 看到「切到立绘」+「设置」+「退出」
# 点设置 → 形态 radio 显示「药丸 ●」「立绘 ○」，default uiMode = capsule 正确
pkill -f "swift run Jarvis"
```

⚠️ smoke 不能在 ssh / headless 跑——需要本地 mac GUI 会话。如果 worker 是远端 agent 跑这步，跳过 step 3 让用户自测。

- [ ] **Step 4: 写 README 更新**

Edit `apps/jarvis/README.md`，在末尾加段「v0.3 · 立绘桌宠模式」简介，说明：
- 怎么切（设置 / 右键菜单）
- 资产装在哪
- 默认是 capsule，老用户无感升级

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis/README.md
git commit -m "docs(jarvis): README 加 v0.3 立绘桌宠模式说明"
```

---

## Art Track（与代码工程并行 · gpt-image-2）

**By:** 一个独立 worker 走 `gpt-image-2` skill。**不阻塞 Task 1-10**——资产缺失时 PoseAssetCatalog 自动让 jarvis 回 capsule 模式。

### A1: Reference sheet 出图

- [ ] 调 `gpt-image-2` skill，prompt（可在迭代里调）：

```
东方水墨风格 character reference sheet ·
主题：九天玄女上古女神 · 仙气素纱衣袖 ·
隐身处理（侧影/背影为主，不正面露脸）·
黑灰墨色调为主，朱砂只点睛配饰 ·
透明背景 · multi-angle（正侧背三视图）·
衣饰特写细节 · 无表情包风格 · 无萌系日漫元素 ·
工笔写意 · 高对比留白
```

输出存：`apps/jarvis/Resources/Pet/poses/ref-sheet.png`（仅参考，不进 catalog）

- [ ] **由用户人工 ack 气质**——在 PoseAssetCatalog 单测之外。worker 把 ref-sheet 路径报给 team-lead，team-lead 把图片给用户看。**用户不 ack 不出后续 5 张**（避免 5 张全废）。

### A2: 5 张 pose 图

只在 A1 用户 ack 后做。

- [ ] 5 个 prompt（每张独立 gpt-image-2 调用 + IP-Adapter ref-sheet 一致性）：

```
基于 [ref-sheet.png] 一致角色，姿态：
  idle      → 侧身静立，垂目，双手交于身前，衣袖自然下垂
  listening → 微微侧首聆听，目光抬起，发丝轻扬
  thinking  → 背手负后或抚袖沉吟，闭目低首
  speaking  → 半身正面，唇微启，一手轻舒
  ack       → 半身后仰一拍微微颔首
透明背景 · 同 ref 风格 · 280×420 输出（@2x = 560×840）
```

- [ ] 输出存：`apps/jarvis/Resources/Pet/poses/{idle,listening,thinking,speaking,ack}@2x.png`

- [ ] commit + manifest 更新（如有 anchor 调整）

```bash
git add apps/jarvis/Resources/Pet/poses/*.png
git commit -m "art(jarvis): 玄女桌宠 5 pose + ref sheet (gpt-image-2)"
```

---

## Self-Review 通过项

✅ Spec 5 个新 component 全有对应 Task（PetPanel/PoseAssetCatalog/PetPoseView/SleeveCanvasOverlay/BlinkCoordinator/PetSweepOverlay = 6 个，全有）
✅ Spec 双 panel 协调要求 → Task 8 covered
✅ Spec 资产缺失回退 → Task 8 + Task 6 covered
✅ Spec 测试策略 → Task 1/2/3 全 TDD 测试，UI 视图层（4/5/6/7）走 smoke run
✅ Spec L1 范围严格控制 → 不动后端，不动 EventKind，不动 fuxi-im
✅ 无 placeholder（每步都有完整代码）
✅ 类型一致性：`Settings.UIMode` / `AppState.VoicePhase` / `PoseAssetCatalog.poseName(for:)` 跨 task 命名对齐

## 依赖图（给 team-lead 派活参考）

```
T1 ─┐                    ┌─ T8 ─ T9 ─┐
    ├─ (independent)     │           │
T2 ─┤                    │           ├─ T10 (smoke)
T3 ─┤                    │           │
T4 ─┼── T6 ─── T7 ──────┘           │
T5 ─┘                                │
                                     │
A1 ── (user ack) ── A2 ──────────────┘
```

可并行 group：
- Group α: T1 → T8 → T9
- Group β: T2 → T6 → T7（T6 还需 T3/T4/T5 完成）
- Group γ: T3, T4, T5（彼此独立）
- Group δ (art): A1 → 用户 ack → A2

最少 worker 配置：3 个工程 ε（α/β/γ 各一） + 1 art ε。
