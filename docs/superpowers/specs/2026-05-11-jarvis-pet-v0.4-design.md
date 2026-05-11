# Jarvis 桌宠 v0.4 · 设计

> 日期：2026-05-11
> 状态：design 等用户 ack 后开 implementation plan
> 替换：v0.3「PetPanel + 立绘 PNG crossfade」（已废弃，archive/jarvis-v0.3-pose 留档）
> Brainstorming 路径：v0.3 实测后用户反馈"贴图 crossfade 不是真桌宠"→ GitHub 实测 5 个最受欢迎桌宠 repo → 用户拍板抄 VPet（虚拟桌宠模拟器，6.1k stars）+ BongoCat 架构（Tauri 2.0，20.8k stars）

## 一句话

把玄女做成 VPet 那种"会动会反应有性格"的真桌宠：Tauri 2.0 跨平台壳子 + PixiJS sprite 动画引擎 + VPet 抄来的 28 GraphType × 4 AnimatType × 4 ModeType 行为状态机 + 6 维数值养成系统（映射玄女工作状态）+ 接现 jarvis Swift native 当 IPC sidecar 保留 mac vpio AEC/WhisperKit 语音优势 + 第一方 MOD 系统给第三方角色包扩展。

## 决策表（用户拍板锁定）

| 维度 | 选择 | 否决项 + 理由 |
|---|---|---|
| 技术栈 | **Tauri 2.0 + Vue 3 + TS + Vite + PixiJS v8** | 抄 BongoCat（实测 20.8k stars 跨平台事实标杆）。Live2D 路线否决——VPet 本身是 sprite-based 不是 Live2D，且 sprite 资产用 gpt-image-2 帧帧生比 .moc3 商单美术好控成本 |
| 行为/动画系统 | **抄 VPet 全集**：28 GraphType × 4 AnimatType × 4 ModeType | 5 GraphType 极简版（v0.3 教训）→ 不像桌宠；12 GraphType 中等 → 用户明确要 all |
| 接语音 | **接，走 IPC sidecar 桥到现 jarvis Swift native** | Web Speech API 重做 → 丢 mac vpio AEC / Silero VAD / WhisperKit native 优势；不接 → 失去玄女语音对话能力 |
| 资产生成节奏 | **先生 1 个 GraphType（Default 6 帧）给用户 ack 体感** → 再批量铺 27 个 | 全量并行 → 风险高，万一第 1 个就失败浪费 token；串行慢 → 太慢 |
| MOD 系统 | **Phase 3 上**——LPS 元数据 + 资产目录扫描 + UI 入口 | 不上 → 失去 VPet 的开放生态优势；Phase 1 上 → 拖慢 MVP |
| 删 v0.3 vs 留 | **删立绘（apps/jarvis/Sources/UI/Pet/ + 资产 + 设置项），保留药丸 + 语音壳子** | archive/jarvis-v0.3-pose 分支留档完整历史 |

## 架构

### 系统全景

```
┌──────────────────────────────────────────────────────────────┐
│  apps/jarvis-pet/  (Tauri 2.0 桌宠 app · 跨平台 mac/win/linux) │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │ Vue 3 + TS + UnoCSS frontend                            │ │
│  │  ├── PixiJS sprite renderer + atlas loader              │ │
│  │  ├── Behavior scheduler（VPet 状态机抄过来）              │ │
│  │  ├── 数值 store（Pinia · 6 维）                          │ │
│  │  └── fuxi adapter（REST + WS · meta.agent=xuannv 流）    │ │
│  └────────────────────────┬────────────────────────────────┘ │
│                           │ Tauri invoke / event             │
│  ┌────────────────────────▼────────────────────────────────┐ │
│  │ Rust backend (src-tauri/)                               │ │
│  │  ├── tauri_nspanel macOS NSPanel + transparent          │ │
│  │  ├── tauri-plugin-macos-input-monitor 全局键鼠 hook      │ │
│  │  ├── voice_ipc（连本地 jarvis sidecar）                  │ │
│  │  └── asset_pipeline（LPS 元数据扫描 + MOD 加载）          │ │
│  └────────────────┬─────────────────────┬───────────────────┘ │
└───────────────────┼─────────────────────┼─────────────────────┘
                    │                     │
                    │ HTTP/WS             │ localhost IPC
                    ▼                     ▼
              fuxi-im backend       apps/jarvis (Swift sidecar)
              /api/conv WS          ├── vpio AEC
              /api/tasks            ├── Silero VAD
              /api/nodes            ├── WhisperKit STT
              UsageReport           ├── RemoteTTSProvider (GPT-SoVITS)
              （需新加 push）        └── wake-server client
```

### 双 app 共存模型

- **apps/jarvis-pet/**（新）：UI + 行为 + 数值 + MOD —— 跨平台主壳
- **apps/jarvis/**（已存在，v0.3 立绘已删）：药丸 + 语音工程层 —— mac only sidecar
- 用户视角：可以单独开 jarvis 用药丸 + 语音；可以单独开 jarvis-pet 用桌宠（无语音 fallback）；同时开两个时 jarvis-pet 通过 localhost IPC 调 jarvis 的语音能力（mac 用户体验最佳）
- 跨平台：win/linux 用户只能装 jarvis-pet（没语音 native），但 fuxi-im /api/tts 远程音色仍可用

### voice IPC 协议

jarvis Swift app 启动时绑 localhost 端口（如 `127.0.0.1:9303`），暴露 HTTP REST + WS：

```
GET  /api/sidecar/health     → {ok, vad_ready, stt_ready, tts_ready, wake_mode}
POST /api/sidecar/listen     → 启动 STT 听写一段（流式 WS 返中间结果 + final）
POST /api/sidecar/say        → body:{text, voice, rate} 触发 TTS（系统 / 远端）
WS   /api/sidecar/wake       → 推送 wake event {type:"wake", method:"remote"|"fallback"}
WS   /api/sidecar/audio_level → 实时 mic 电平 (60Hz)，桌宠 listening 状态用来调动画
```

jarvis-pet 启动时探测 `127.0.0.1:9303`，可达则启用"接语音"模式；不可达则降级"纯桌宠"模式。

## 行为/数值系统（VPet 抄过来 + 玄女特化）

### 6 维数值定义

| 字段 | VPet 含义 | 玄女含义 | 信号源 | 范围 | 衰减 / 恢复 |
|---|---|---|---|---|---|
| `strength` 体力 | 行动能力 | 工作能力余量 | `1 - inflight/max_concurrency` (WorkerHeartbeat) | 0-100 | 玄女空闲时回 +0.5/min |
| `strengthFood` 饱腹 | 吃饱否 | context 余量 | `100 - 100*UsageReport.pct` | 0-100 | 不衰减；玄女让贤后归 100 |
| `strengthDrink` 口渴 | 喝水否 | 用户互动新鲜度 | 距上次 UserPrompted 时间衰减 | 0-100 | 时间衰减 -2/min；UserPrompted 回 100 |
| `feeling` 心情 | 心情值 | 任务接纳率 | `(DeliverableAccepted + Done) / Created × 100` | 0-100 | 持续累积加权平均 |
| `health` 健康 | 病痛 | 系统稳定 | `100 - (AgentDead + WorkerStaleSwept) / worker_total × 100` | 0-100 | 7 天滚动窗口 |
| `likability` 好感 | 跟主人亲密度 | 用户互动累计 | UserPrompted 总数 + DeliverableAccepted 总数 | 累加 ∞ | 不衰减 |
| `money` 金钱 | 财富 | 累积交付 | Σ DeliverableProduced 数 | 累加 ∞ | 不衰减 |

ModeType 触发规则（抄 VPet `CalMode()`）：
```
Ill:           health ≤ 30
PoorCondition: health ≤ 60 OR feeling ≤ 45
Happy:         feeling ≥ 90 (考虑好感度加成)
Normal:        其他
```

### GraphType 28 种（按 VPet 命名 + 玄女语义）

| GraphType | VPet 原义 | 玄女语义 | 触发 |
|---|---|---|---|
| `Default` | 呼吸 idle | 静默呼吸 | 一切空闲态默认 |
| `Idle` | 空闲随机动作 | 望窗外 / 抚琴 / 整衣 / 翻书 | 调度器 15s tick 加权随机 |
| `Touch_Head` | 摸头 | 摸头反应 | 鼠标点头部区 |
| `Touch_Body` | 摸身 | 拖动身体 | 鼠标按住拖动 |
| `Sleep` | 睡 | 入定 / 闭目 | strength<25 OR 长 idle |
| `Say` | 说话 | 说话 | XuannvVoiceLine 收到 |
| `Listen` | 听 | 聆听 | 用户启动语音输入（IPC 通知） |
| `Think` | 思 | 思考 | ThinkingStarted 收到 |
| `Work_Start/Loop/End` | 工作起循结 | 工作（task in flight）| TaskDispatched/agent busy |
| `Achievement` | 成就 | 任务完工喜悦 | DeliverableAccepted |
| `Tired` | 累 | 上下文水位高 | strengthFood < 35 |
| `Critical` | 临界 | 让贤前夕 | XuannvContextWatermark.action == handoff_offer |
| `Switch_Up` | 状态升 | mode 升级过渡 | ModeType 上调 |
| `Switch_Down` | 状态降 | mode 降级过渡 | ModeType 下调 |
| `Switch_Hunger` | 饿了 | strengthFood 降到阈值 | strengthFood crosses 50 |
| `Switch_Thirsty` | 渴了 | strengthDrink 降到阈值 | strengthDrink crosses 30 |
| `Raised_Static` | 提起静 | 拖到屏幕外暂停 | Drag 进 hidden 区 |
| `Raised_Dynamic` | 提起动 | 拖动中挣扎 | active drag |
| `Move` | 移动 | 自由游走 | 长 idle 时随机移动 |
| `Common` | 通用 | 通用 fallback | 缺资产时降级 |
| `StateONE/TWO` | 状态变体 | 备用 idle 池 | Idle 子集 |
| `SideHide_Left_Main/Rise` | 屏边藏左主/起 | 拖到屏左侧消隐 | edge snap |
| `SideHide_Right_Main/Rise` | 屏边藏右主/起 | 拖到屏右侧消隐 | edge snap |

### AnimatType 4 档（事件链）

抄 VPet：
- `Single` —— 一次播完
- `A_Start` —— 接近 / 起势
- `B_Loop` —— 循环 / 持续
- `C_End` —— 结束 / 收势

互动事件（如 Touch_Head）走 `A_Start → B_Loop → C_End → 回 Default`。

### 调度器（事件驱动 + 加权随机）

抄 VPet `MainLogic.cs`：

```typescript
class BehaviorScheduler {
  // 15s tick - 主循环
  tick() {
    // 1. 数值衰减/恢复
    this.applyDecay();
    // 2. ModeType 重算
    const newMode = this.calMode();
    if (newMode !== this.mode) {
      this.playSwitch(newMode);
      this.mode = newMode;
    }
    // 3. 加权随机选下一个 idle 动作（10 级权重抄 VPet）
    const action = this.pickWeighted([
      { graph: 'Default', weight: 30 },
      { graph: 'Idle', weight: 25 },
      { graph: 'StateONE', weight: 15 },
      { graph: 'Move', weight: 10 },
      { graph: 'Sleep', weight: this.strength < 25 ? 50 : 5 },
      // ...
    ]);
    this.play(action);
  }

  // 事件驱动（fuxi 事件触发，跳出 idle）
  onFuxiEvent(ev: FuxiEvent) {
    switch (ev.kind) {
      case 'XuannvVoiceLine': this.play({ graph: 'Say' }); break;
      case 'ThinkingStarted': this.play({ graph: 'Think' }); break;
      case 'TaskDispatched': this.play({ graph: 'Work_Start' }); break;
      case 'DeliverableAccepted':
        this.play({ graph: 'Achievement' });
        this.feeling += 5;
        break;
      // ...
    }
  }

  // 用户互动（鼠标/键盘）
  onTouch(area: 'head' | 'body') {
    this.playChain(`Touch_${area}_A_Start`, 'B_Loop', 'C_End');
    this.feeling += 1;
    this.strength -= 1;
  }
}
```

## Sprite 资产系统

### 文件命名约定（抄 VPet）

```
apps/jarvis-pet/resources/sprites/xuannv/      # default character
├── default/
│   ├── default_001_120.png    # 第 1 帧，显示 120ms
│   ├── default_002_120.png
│   └── ... (6-10 帧 idle 呼吸循环)
├── idle/
│   ├── idle_001_150.png       # 第 1 帧，150ms
│   └── ...
├── touch_head/
│   ├── happy/                 # ModeType variant
│   │   ├── a_start_001_100.png
│   │   ├── b_loop_001_120.png
│   │   └── c_end_001_100.png
│   └── normal/...
├── say/
│   └── ...
└── manifest.lps               # VPet 风格 LPS 元数据
```

`manifest.lps` 格式（抄 VPet `info.lps`）：

```
[character]
name: 玄女
author: fuxi
version: 0.4

[pnganimation]
graph: Default
animat: Single
mode: Normal
path: ./default
loop: true

[pnganimation]
graph: Touch_Head
animat: A_Start
mode: Happy
path: ./touch_head/happy
loop: false
next: Touch_Head_B_Loop_Happy
```

### gpt-image-2 资产生成 pipeline

**Phase 1 验证**：先生 1 个 GraphType (Default · 6 帧 · Normal mode)，让用户 ack 体感。
- 单帧 prompt：基于 ref-sheet 玄女角色，姿态"侧身静立呼吸 phase {1..6}/6"——**关键**：每帧 prompt 强调"承接前帧动作"，靠 IP-Adapter 一致性
- 第 6 帧应该跟第 1 帧首尾相接，能无缝循环

**Phase 2-4 批量**：4 个 art ε 并行，每个吃 1 个 GraphType；monitor `persistent: true` 不超时；team-lead 每 1 小时 poll 一次进度

**总规模**：28 GraphType × 平均 1.5 ModeType × 平均 1 AnimatType × 6 帧 ≈ 250 帧
- 单帧 ~150s × 250 = 10.4 小时（4 路并行 ≈ 2.5 小时）
- 成本估算：250 × ¥0.15 = ¥38

## 互动系统

| 互动 | 检测 | 反应 |
|---|---|---|
| 鼠标点头部 | PixiJS hit test on head sprite area | `Touch_Head_A_Start` → `B_Loop` → `C_End`；feeling +1 |
| 鼠标点身体 | hit test on body area | `Touch_Body` 链；feeling +0.5 |
| 拖动 | mousedown + drag delta > 5px | `Raised_Dynamic` loop；strength -1 |
| 拖到屏边 | drag end at screen edge < 50px | `SideHide_*_Rise` |
| hover 跟头 | mouse move | head sprite 微调 rotation 跟鼠标朝向 |
| 双击 | 200ms 内 2 次 click | 弹设置面板 |
| 右键 | rightclick | 弹「设置 / 喂咖啡 / 切回药丸 / 退出」菜单 |

## MOD 系统（Phase 3）

第三方角色包目录约定：
```
~/.fuxi/jarvis-pet/mods/<mod_id>/
├── character.lps      # 元数据：name / author / version / 描述
├── manifest.lps       # 同 sprite manifest
└── sprites/
    ├── default/...
    ├── idle/...
    └── ...
```

UI 入口：设置 → MOD → 列出已装 + 「导入 .zip」按钮 → 切换当前角色。

## 现 jarvis Swift 改动（最小化）

apps/jarvis/ 这边为了当 sidecar 需要做的事：

1. **加 `LocalSidecarServer`**：新文件 `Sources/Net/SidecarServer.swift`，启动 axum-style HTTP/WS server on `127.0.0.1:9303`（用 `Network` framework 或第三方 swift-nio），暴露上述 `/api/sidecar/*` 路由
2. **复用现有 Recognizer / Synthesizer / RemoteTTSProvider / RemoteWakeClient**——不重写，只 adapter 层
3. **AppState 不改**——SidecarServer 只读 AppState 状态 + 调 AppState 方法

## fuxi-im 后端改动（最小化）

仅一项必需改动：

**新增 `UsageReport` 推送到 `/api/conv` WS**——目前 conv.rs 只过滤 `meta.agent==xuannv`，但 UsageReport 走 EventBus 不走 conv stream。需要：
- 选项 A（轻）：在 publish UsageReport 时显式 set `meta.agent = Some(xuannv_id)`，不改 conv.rs filter 哲学
- 选项 B（重）：conv.rs 加白名单允许 `EventKind::UsageReport`

推荐选项 A——与 conv.rs 注释"按 agent id 过滤，不维护白名单"哲学保持一致。

## 测试策略（TDD）

### 前端（Vitest + Vue Test Utils）
- `BehaviorScheduler.spec.ts`: 加权随机分布、ModeType 切换条件、事件触发响应
- `数值Mapper.spec.ts`: WorkerHeartbeat → strength 公式、UsageReport → strengthFood 公式
- `SpriteRenderer.spec.ts`: 帧序列加载、loop 循环、frame 时间精度
- `ManifestLoader.spec.ts`: LPS 解析

### 后端（Rust + cargo test）
- `nspanel.rs`: 透明 / always-on-top / 鼠标穿透 切换正确
- `voice_ipc.rs`: 连不上 sidecar 的 graceful degrade
- `asset_pipeline.rs`: MOD 目录扫描 + 缺资产 fallback

### E2E（Playwright + tauri-driver）
- 启动 → 桌宠出现 → 拖动到屏边 → 双击弹设置
- 模拟 fuxi 事件 → 桌宠对应反应

## 阶段化交付（Phase 1 → 4）

### Phase 1 · MVP "桌宠骨架"（~10 工作 session）
- Tauri 壳子 + macOS NSPanel + 透明 + alwaysOnTop
- PixiJS sprite renderer + LPS manifest loader
- 1 个 GraphType（Default · 6 帧 · Normal mode）
- fuxi adapter（REST + WS 接 /api/conv）
- 6 维数值 store + 简单 mapper
- 拖动互动
- **art**：1 个 GraphType（6 帧）由 1 个 art ε 出
- **不接语音**——纯桌宠 fallback 模式

**MVP 验收**：玄女桌宠在 mac 屏幕悬浮，呼吸 idle，可拖动，收到 XuannvVoiceLine 时切换到 Say sprite（如果 Say 资产已就绪），数值随 fuxi 事件实时更新（debug overlay 显示）。

### Phase 2 · 行为系统全 + 12 GraphType（~15 session）
- BehaviorScheduler 调度器完整实现
- AnimatType 事件链（A→B→C）
- ModeType 切换 + Switch 动画
- 加权随机 idle 池
- Touch_Head/Body 互动
- **art**：补 11 个 GraphType（按 user 优先级）

### Phase 3 · 接语音 + MOD 系统（~10 session）
- apps/jarvis 加 LocalSidecarServer
- jarvis-pet voice_ipc 模块连接
- Listen / Say sprite 接 audio_level 调动画
- MOD 系统：LPS 扫描 + UI 入口 + zip 导入
- **art**：补剩余 GraphType（Sleep/Work/Achievement/etc）

### Phase 4 · 跨平台 + 完整 28 GraphType（~10 session）
- Tauri windows/linux build 验证
- 全 28 GraphType 资产铺齐
- 边缘 polish：SideHide / Move / Raised
- 性能 profile + 内存优化

## 不在范围

- ❌ Live2D / Spine 骨骼动画（VPet 路是 sprite，不引重 SDK）
- ❌ 用 jarvis-pet 替代药丸（药丸保留作为语音壳子 + sidecar）
- ❌ Steam 创意工坊集成（MOD 走本地目录 + 手动 import）
- ❌ Web 部署（Tauri 是 native，不出 PWA 版）

## 关键 risk

1. **资产生成时间**：250 帧 × 150s = 10 小时（4 路并行 2.5 小时）。Phase 1 先 6 帧验证管线
2. **PixiJS 与 Tauri WebView 透明窗口冲突**：mac WebKit 已知 issue（#8255），focus 切换重绘异常。需 `macOSPrivateApi: true`
3. **sidecar IPC 复杂度**：jarvis 端要新加 HTTP server，jarvis-pet 端要 fallback 处理 sidecar 不可达
4. **MOD 资产质量参差**：第三方包可能资产不全 → asset_pipeline 必须 graceful degrade（缺哪个 GraphType 就不播）

## 资料引用

- VPet 源：[github.com/LorisYounger/VPet](https://github.com/LorisYounger/VPet)（6.1k stars）
  - `VPet-Simulator.Core/Handle/GameSave.cs` 数值定义
  - `GraphInfo.cs` 28×4×4 状态机
  - `MainLogic.cs` 调度器
  - `PNGAnimation.cs + PetLoader.cs` sprite 帧系统 + LPS 元数据
- BongoCat 源：[github.com/ayangweb/BongoCat](https://github.com/ayangweb/BongoCat)（20.8k stars）
  - `src-tauri/src/core/setup/macos.rs` tauri_nspanel 用法
  - `src/utils/live2d.ts` PixiJS 集成 pattern
- Tauri 2.0 + 桌宠最佳实践：[crabnebula.dev/blog/building-a-desktop-pet-with-tauri](https://crabnebula.dev/blog/building-a-desktop-pet-with-tauri/)
- pixi-live2d-display（备选 Live2D 渲染，未采用）：[github.com/guansss/pixi-live2d-display](https://github.com/guansss/pixi-live2d-display)
- CartoonAlive（未来可能用来把 ref-sheet rig 成 Live2D 升级路径）：[arxiv.org/html/2507.17327v1](https://arxiv.org/html/2507.17327v1)
