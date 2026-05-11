# 贾维斯（Jarvis）· 玄女语音壳子

伏羲平台的 macOS 语音前端。唤醒/热键 → 中文听写 → 玄女 → TTS 念回应。文字记录全量同步到 PWA / 手机 IM。

## 状态

- v0.1（当前）：骨架 + 全局热键 + Apple SFSpeechRecognizer 听写 + AVSpeech TTS + fuxi-im HTTP/WS 联通
- v0.2 计划：Picovoice Porcupine 唤醒词「玄女」、自定义热键 UI、推送通知

## 启动

```bash
brew install xcodegen   # 没装的话
cd apps/jarvis
xcodegen generate
open Jarvis.xcodeproj
```

Xcode 内：
1. Signing & Capabilities → 选 "Sign to Run Locally"（个人本地用 ad-hoc 签即可，按 fuxi 项目 `feedback_macos_gatekeeper_codesign` 教训）
2. Product → Run（⌘R）

首次启动系统会弹三次权限：
- 麦克风（必给）
- 语音识别（必给）
- 辅助功能（用全局热键时给——系统设置 → 隐私与安全性 → 辅助功能）

## 配置

菜单栏 mic 图标 → 设置：

- **连接** · `fuxi-im 地址` 默认 `http://127.0.0.1:9100`；远端家用部署填 `https://im.qmledmq.cn:8443`。`Pair Token` 在 PWA 设置面板生成
- **语音** · TTS voice identifier，留空走系统默认中文女声
- **唤醒** · 热键 / 唤醒词 / 两者皆可。唤醒词需 Picovoice access key（个人 plan 免费）

## 默认热键

⌥⇧Space 进入听写。再按一次结束并发送，或停顿 1.5s 自动发。

## 链路

```
按热键/唤醒
    │
    ▼
SFSpeechRecognizer (zh-CN, on-device) ─→ 文字
    │
    ▼ POST /api/intervene  body={"text":"[语音] 用户说的话","mode":"append"}
fuxi-im
    │
    ▼ 玄女 prompt 看到 "[语音]" 前缀 → 决定哪句要 say
    │
    ▼ 玄女执行 Bash: fuxi xuannv say "..."
    │
    ▼ daemon publish XuannvVoiceLine（meta.agent=xuannv_id）
    │
    ▼ /api/conv WS 透传
贾维斯收 voiceLine → AVSpeechSynthesizer 念出口
```

## 开发约束

- macOS 14+（Sonoma）。MenuBarExtra / on-device zh-CN STT 都需要这个版本
- 中文注释、英文标识符（按 fuxi 项目约定）
- 没有 sandbox（个人工具，全局热键 / Accessibility / 自由网络都要）
- 涉及 secret 的字段（pair token / picovoice key）走 Keychain，绝不进 UserDefaults

## 测试

```bash
xcodebuild test -project Jarvis.xcodeproj -scheme Jarvis
```

或在 Xcode 内 ⌘U。

> 仅装 CommandLineTools（无完整 Xcode）的环境跑不了 `swift test`——XCTest framework 不在 CLT bundle 里。Pet 模块（PetSettings / PetPoseAssetCatalog / PetBlinkCoordinator）测试代码已就位，跑前确认 `xcode-select -p` 指向 `/Applications/Xcode.app/Contents/Developer`。

## 已知陷阱

- **Clash TUN 模式吞 127.0.0.1**：FuxiClient 已注入 `connectionProxyDictionary = [:]`（按 fuxi `feedback_macos_gatekeeper_codesign` 同款防御），手动 curl 测试也加 `--noproxy '*'`
- **首次中文 STT 慢**：on-device zh-CN 模型首次会下载，可能需要 1-2 分钟。System Settings → General → Keyboard → Dictation 提前打开下载更稳
- **全局热键被前台 app 抢**：addGlobalMonitorForEvents 不能 consume 事件——某些应用（如 Xcode 调试中）会抢同款组合键。换 `addGlobalMonitorForEvents` → Carbon `RegisterEventHotKey` 可独占，但要桥 C API

## v0.3 · 立绘桌宠模式

v0.3 起新增「立绘」形态——水墨仙气 280×420 panel，5 状态对应 5 张 pose 图（idle / listening / thinking / speaking / ack）+ 衣袖 Canvas 飘动 + 偶发微眨。和老「药丸」形态共存，用户可切。

**默认仍走药丸**——老用户升级无感，v0.2 持久化的 UserDefaults 没 `uiMode` 字段会回退到 capsule。

**怎么切**：
- 设置 → 「连接」标签顶部「形态」radio → 选「立绘」/「药丸」实时生效
- 右键药丸 → 「切到立绘」/ 右键立绘 → 「切回药丸」

**资产装哪**：`apps/jarvis/Sources/Resources/Pet/poses/{idle,listening,thinking,speaking,ack}@2x.png`（路径在 SwiftPM `Sources/Resources/` 下而非顶层 `Resources/`——SwiftPM `path: "Sources"` 限制下走 `.copy("Resources/Pet")` 才能让 `Bundle.module` 找到）。资产缺失时切「立绘」会自动弹 alert 并回到药丸——不会让 App 黑屏。资产由 art track 单独走 gpt-image-2 出图（参考 `docs/superpowers/plans/2026-05-11-jarvis-pet-implementation.md` Art Track 段的 prompt）。

**架构**：UI 层 `apps/jarvis/Sources/UI/Pet/` 子目录 6 个文件（PetPanel / PetPoseView / PoseAssetCatalog / SleeveCanvasOverlay / BlinkCoordinator / PetSweepOverlay），后端零改动；状态机仍走 `AppState.VoicePhase` 现成 5 状态。
