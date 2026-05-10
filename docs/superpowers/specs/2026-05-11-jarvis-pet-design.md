# Jarvis 桌宠 v0.3 · 设计

> 日期：2026-05-11
> 状态：design 已批，等 implementation plan
> 上一阶段：[`apps/jarvis` v0.2 大闭环（语音壳子 + GPT-SoVITS 心海音色）已 ship](../../../apps/jarvis/README.md)
> Brainstorming 路径：用户主动提「想优化成桌宠 app」→ `superpowers:brainstorming` skill 收敛 4 个核心选择

## 一句话

让玄女从「dock 上方的禅意药丸」演进为「dock 上方的水墨仙气立绘」，立绘 + 药丸双形态共存可切；MVP 只做 L1 被动反应（响应 phase 切换 + `XuannvVoiceLine`），主动陪伴留蓝图。

## 决策表（brainstorming 收敛结果）

| 维度 | 选择 | 否决项 + 理由 |
|---|---|---|
| 形象范式 | 拟人立绘（仙气线条） | 抽象药丸增强 → 不够「桌宠」感；水墨抽象生灵 → 过抽象失去人物对应；全屏环境光 → 太弱化 |
| 资产路线 | gpt-image-2 多 pose 静态成图 + IP-Adapter 一致性 | character sheet → 手切 → Spine rigging：工程量 5-10x，单人维护不现实 |
| 渲染栈 | 纯 SwiftUI Image + Canvas overlay | Live2D Cubism：商单 ¥5k-15k + 1-3 月美术；Spine：license + SDK 集成；当前不需要骨骼级动画 |
| 形态共存 | 立绘 + 药丸双窗口，设置开关切换（默认药丸） | 完全替代药丸：原 dock 上方轻量贵人调丢了；药丸主位 + 立绘客位：「桌宠常驻感」底调小 |
| 主动性 | L1 仅被动（同现药丸语义） + L2/L3/L4 蓝图入文档 | L2-L4 一次到位：scope 爆炸；L1 是最小完整切片，先 ship 体感对了再推后续 |
| MVP 范围 | B 标准桌宠：5 pose 全套 + 衣袖飘动 + 偶发微眨 | A 框架先行（1 张 idle + 灰阶替）：5 状态共一张 pose 不像桌宠；C 完整桌宠（多层 + TTS 同步）：装饰性细节多，违公理 #1「明示而非暗动」 |

## 架构

### 顶层数据流不变

```
fuxi-im /api/conv WS
        ↓
ConvEvent.voiceLine(text)
        ↓
AppState.handleConvEvent → phase = .speaking
        ↓
两个 Panel 并行监听 AppState（共享 single source of truth）
        ↓
   ┌────────────────────────┬────────────────────────┐
   ▼                        ▼                        ▼
CapsulePanel              PetPanel              Synthesizer / RemoteTTS
（uiMode == .capsule）   （uiMode == .pet）     （音频侧不动）
```

`AppState` 作为单例不动，仅新增 `Settings.uiMode: enum { capsule, pet }` 字段（默认 `.capsule`，即不主动开桌宠模式，老用户无感升级）。

### 双 Panel 协调

启动时根据 `settings.uiMode` 决定显示哪个 NSPanel；用户在设置里切换时：
1. 旧 panel `orderOut(nil)`
2. 新 panel `orderFront(nil)` + 应用 saved CGPoint（如有）
3. 写回 `Settings.uiMode` 持久化

两 panel 同时监听 `AppState.$phase`、`AppState.$ackPulse`、`AppState.$audioLevel`——切换是「视图层切换」不是「状态机切换」。

### 默认位置与拖动

- 立绘 panel 默认尺寸：280×420（@2x）
- 默认位置：dock 上方居中，沿用现 `CapsulePanel.swift::dockGap` 逻辑外推
- 可拖动：`NSPanel` 设 `isMovable = true`；用户拖完保存 `CGPoint` 到 `UserDefaults`，下次启动恢复
- menubar 图标：保留，沿用现 `menuBarIconName`；右键菜单加一行「形态：药丸 / 立绘」radio

## 新增组件

所有新文件放 `apps/jarvis/Sources/UI/Pet/` 子目录。

| 文件 | 职责 | 估算行数 |
|---|---|---|
| `PetPanel.swift` | NSPanel 容器，accessory 模式，拖动 + 位置记忆 | ~150 |
| `PoseAssetCatalog.swift` | 5 张 PNG 资产 + manifest（pose name → bundle path → anchor 点）+ `validate()` 启动校验 | ~80 |
| `PetPoseView.swift` | SwiftUI 主视图，监听 `AppState.phase` crossfade 切 pose；叠 SleeveCanvasOverlay 与 BlinkCoordinator overlay | ~120 |
| `SleeveCanvasOverlay.swift` | TimelineView + Canvas，sin 三层叠加画衣袖飘动；`amplitude` 属性可被 audioLevel 调制 | ~70 |
| `BlinkCoordinator.swift` | `ObservableObject`，4-7s 随机间隔 `@Published var blinkTrigger: Int`，PetPoseView 监听触发 0.15s 渐隐线 | ~50 |
| `PetSweepOverlay.swift` | 复刻现 `SweepOverlay`，由 `ackPulse` 触发 200ms 墨笔横扫，叠在 pose 之上 | ~50 |

资产文件 `apps/jarvis/Resources/Pet/poses/`：
- `idle@2x.png`
- `listening@2x.png`
- `thinking@2x.png`
- `speaking@2x.png`
- `ack@2x.png`
- `manifest.json`（mapping + anchor 点 + 推荐 size）

## State → pose 映射

| AppState.phase | pose 文件 | overlay |
|---|---|---|
| `.idle` | `idle@2x.png` | SleeveCanvasOverlay 慢飘 + BlinkCoordinator 微眨 |
| `.listening` | `listening@2x.png` | 同 idle，sleeve amplitude 跟 audioLevel 调制 |
| `.sending` / `.waiting` | `thinking@2x.png` | SleeveCanvasOverlay 慢飘，无微眨（沉思中眼睛闭着） |
| `.speaking` | `speaking@2x.png` | SleeveCanvasOverlay 中频飘动 |
| ackPulse 触发 | 沿用上一帧 pose | PetSweepOverlay 200ms 横扫，与 earcon 同步 |

## 资产生成 pipeline

走 `gpt-image-2` skill。两步：

**Step 1 · reference sheet**（一次出齐定形象）。Prompt 起手草稿：

> 东方水墨风格 character reference sheet · 主题：九天玄女上古女神 · 仙气素纱衣袖 · 隐身处理（侧影/背影为主，不正面露脸）· 黑灰墨色调为主 · 朱砂只点睛配饰 · 透明背景 · multi-angle（正侧背三视图）· 衣饰特写细节 · 无表情包风格 · 无萌系日漫元素 · 工笔写意

**Step 2 · 5 张 pose**，每张用 reference sheet 做 IP-Adapter 一致性。Prompt 模板：

```
基于 [reference] 一致角色 · 姿态：<idle|listening|thinking|speaking|ack>
描述：
  idle      → 侧身静立，垂目，双手交于身前，衣袖自然下垂
  listening → 微微侧首聆听，目光抬起，发丝轻扬
  thinking  → 背手负后或抚袖沉吟，闭目低首
  speaking  → 半身正面，唇微启，一手轻舒
  ack       → 半身后仰一拍微微颔首
透明背景 · 同 reference 风格 · 280×420 输出
```

**验收门**：第 1 张 idle 出来后必须人工确认气质对了（用户终端 ack）才批量出剩 4 张。否则风格漂移、5 张 pose 不一致 → 整套作废重生。

## 错误处理 / 回退

| 故障 | 行为 |
|---|---|
| 资产 PNG 缺失（任何一张） | `PoseAssetCatalog.validate()` 启动检测失败 → log error + 强制 `Settings.uiMode = .capsule` 落库 + 弹一次提示 sheet「立绘资产缺失，已切回药丸模式」 |
| pose 图加载失败（运行时单张） | 该 phase 显示前一帧 pose 不切（避免空白），log warning |
| NSPanel 创建失败 | 沿用现有 `CapsulePanel` 失败处理，回退到 capsule |
| 用户拖动到屏幕外 | 启动时检测 saved CGPoint 不在任何 NSScreen 内 → 重置到 dock 上方默认位置 |

## 测试策略（TDD 必须先行）

| 测试 | 验证什么 |
|---|---|
| `PoseAssetCatalogTests` | 5 张 PNG 都能从 bundle 加载 + manifest 与 phase enum 一一映射 + `validate()` 缺失场景返回 false |
| `PetPoseViewTests` | phase change → 资产 path 切换正确（用 ViewInspector 或 SwiftUI snapshot） |
| `BlinkCoordinatorTests` | 随机间隔落在 4-7s 范围内（用 mock clock，跑 100 次取分布） |
| `PetPanelDragTests` | 拖动后 CGPoint 写入 UserDefaults；屏幕外检测重置 |
| 反回归 `WireEventTests` | 现有 `XuannvVoiceLine` 解析路径不动 |
| 反回归 `CapsulePanel` 视觉 | 老药丸模式视觉 100% 不变 |

新测试文件放 `apps/jarvis/Tests/Pet*Tests.swift`。

## 现有代码影响面

需要改动的文件（最小化）：

| 文件 | 改动 |
|---|---|
| `apps/jarvis/Sources/App/AppDelegate.swift` | 启动时根据 `settings.uiMode` 决定起 CapsulePanel 还是 PetPanel；切换时协调两者 |
| `apps/jarvis/Sources/App/AppState.swift` | 加 `Settings.uiMode` 字段；加 `togglePanelMode()` 方法 |
| `apps/jarvis/Sources/UI/PreferencesView.swift` | 新增「形态」radio 选项 |
| `apps/jarvis/Sources/UI/CapsulePanel.swift` | 不动 |
| `apps/jarvis/Sources/UI/CapsuleStateView.swift` | 不动 |
| `apps/jarvis/Sources/UI/ZenStyle.swift` | 复用其颜色 token，不改；PetPoseView 直接 import |
| `apps/jarvis/Package.swift` | 加 `Resources/Pet/` 资源目录 |
| `apps/jarvis/Resources/Pet/poses/*` | 5 张 PNG + manifest（资产由用户在 brainstorming 后单独走 gpt-image-2 生） |

后端（`crates/`）**完全不动**。L1 范围内不需要新 EventKind，不需要改 fuxi-im。

## 蓝图 · L2/L3/L4 主动性升级（未来阶段）

以下不在 MVP 范围，仅记录方向，待 L1 体感 OK 后再 brainstorming 拆。

### L2 · task/context 事件反应

桌宠监听更多 EventKind 触发 emote pose（需要新增资产）：

| 事件 | 候选 emote pose |
|---|---|
| `TaskCompleted`（玄女自完任务）| `done@2x.png`（轻颔首微笑） |
| `AgentRequestReview`（门客求审）| `surprised@2x.png`（侧目挑眉） |
| `XuannvContextWatermark`（上下文高位）| `weary@2x.png`（收袖低头） |
| `XuannvHandoffWritten`（玄女让贤）| `farewell@2x.png`（背影远去） |

工程改动：fuxi-im /api/conv WS 把这些 EventKind 推到 jarvis（现在只推 XuannvVoiceLine，加白名单即可）；jarvis 加 `EmoteCoordinator` 短时切换 pose 后回 idle。

### L3 · 时间 / Schedule 主动出现

接 fuxi `Schedule` / `CronCreate`：玄女按 cron 主动 emit voice line。UI 侧：弱化 wake earcon → 直接 PetPanel 半透明渐入 + voice line 显示。需「打扰预算」机制（每小时不超过 N 次）。

### L4 · 全面同伴（闲聊 / 心情 / 主动观察）

L3 + 闲聊 / 心情报告 / 看气候 / 看 task 队列状态主动评论。需要：拉气候模型、Schedule 编排、对话引擎扩展。公理 #1 限制下需精准设计「该不该开口」决策树。

## 不在范围

- ❌ Live2D / Spine SDK 集成
- ❌ 双形态实时同时显示（用户只能二选一）
- ❌ pose 图骨骼级动画 / 唇启 TTS 同步
- ❌ 主动陪伴（L2/L3/L4 全部）
- ❌ 后端任何改动
- ❌ pose 资产生成自动化（gpt-image-2 调用进 jarvis）—— 资产生成是一次性人工流程

## 资产产出节奏

资产生成与代码工程**并行**而非串行：

- 工程组：先把 PoseAssetCatalog `validate()` 失败回退路径打通（资产空时也能编译运行 → capsule 模式），再依次实装 PetPanel / PetPoseView / overlays
- 美术组（人工 + gpt-image-2）：先出 reference sheet → 用户确认气质 → 出 5 张 pose → 入 `Resources/Pet/poses/`
- 整合：5 张 pose 到位后切到 PetPanel 验收

这样工程不被资产阻塞，资产不被工程阻塞。
