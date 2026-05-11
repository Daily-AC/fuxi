# jarvis-pet · 玄女桌宠 v0.4

Tauri 2.0 + Vue 3 + PixiJS v8 + macOS NSPanel。fork [VPet](https://github.com/LorisYounger/VPet)
默认人物萝莉斯（vup）作为初始立绘风格，接 fuxi-im `/api/conv` 把玄女门客的实时工作
事件映射到 6 维数值 + 8 帧呼吸循环。

## Phase 1 范围

- macOS NSPanel 透明常驻（`set_opaque(false)` + `set_has_shadow(false)`，覆盖所有 Space）
- 1 个 GraphType（Default/Nomal/1 8 帧呼吸循环，VPet fork）
- 6 维数值 store + `calMode()` 状态判定（Ill / PoorCondition / Happy / Nomal）
- fuxi-im `/api/conv` WS 客户端 + 退避重连（1s/2s/4s/.../30s cap）
- `EventKind → stats` mapper（UsageReport / WorkerHeartbeat / DeliverableAccepted / 等 6 类）
- 拖动互动（`startDragging`，左键按住任意非 debug 区域）
- debug overlay 显 6 维数值 + canvas size 诊断

## 起跑

```bash
cd apps/jarvis-pet
npm install
npm run tauri dev    # dev 模式
npm run tauri build  # 出 release .app
```

Release `.app` 输出到 `src-tauri/target/release/bundle/macos/Xuannv.app`。
推荐手动改装：

```bash
TARGET=$HOME/Applications/XuannvPet.app
SOURCE=src-tauri/target/release/bundle/macos/Xuannv.app
rm -rf "$TARGET"
cp -R "$SOURCE" "$TARGET"
codesign --verify --verbose=2 "$TARGET"
open "$TARGET"
```

跟 jarvis（药丸 v0.2，bundle id `cn.qmledmq.fuxi.xuannv`）共存——本工程 bundle id
是 `cn.qmledmq.fuxi.jarvis-pet`，TCC 权限独立。

## 不在 Phase 1 范围（留 Phase 2-4）

- BehaviorScheduler 调度器 + AnimatType 链（Touch/Sleep/Say/Listen/Think/Work…）
- 27 个其它 GraphType
- LPS manifest 完整 parser（当前 hardcode DEFAULT_SET，未走 `lpsParser.ts`）
- Sprite 多状态切换（Happy/Ill/PoorCondition 资源已 fork 在 VPet 上游，未接入）
- Live2D 路线已否决（VPet 本身是 sprite-based，gpt-image-2 自生路线也已否决）

## 资源归属

`resources/sprites/loris/` 下所有 PNG fork 自 VPet
`VPet-Simulator.Windows/mod/0000_core/pet/vup/Default/Nomal/1/`，
Apache License 2.0，作者 LorisYounger。详见
[`resources/sprites/loris/ATTRIBUTION.md`](resources/sprites/loris/ATTRIBUTION.md)。
