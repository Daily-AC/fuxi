# Decision 15 · IM 任务 sheet → 任务树 + active target 私聊页

**日期**：2026-04-26
**状态**：已采纳（用户拍板 ABCC，spec 已 commit）
**spec**：[`docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md`](../superpowers/specs/2026-04-26-im-task-tree-redesign-design.md)

## 背景

决策 14（IM 移动端骨架）落地一周后，2026-04-26 用户在手机上看到当前 PWA 任务 sheet 的截图——9 条卡片全是 `#xxx user-turn`（玄女内部对话 task），完全没有任务进度感。同时反馈了三件事：

1. "这个任务需要重新设计"
2. "要和 TUI 的任务树那种结构"
3. "右滑看任务树，然后可以选择对应的门客然后可以回玄女就像 TUI 的交互差不多。只不过 TUI 是快捷键。"

意味着决策 14 的"任务卡片列表 + 内嵌 chat"虽方向对，但**任务呈现的信息架构 + 给特定门客发消息的通道**两块没接住 TUI 的心智模型。

## 决策（4 个轴 × ABCC 候选）

经 brainstorm 用 visual companion 推 4 屏 wireframe 给用户选，每屏 3 候选：

### 轴 1 · 输入路由模型 → A · mirror TUI active target

输入框文本根据"当前选中谁"决定送给 Xuannv 还是 Worker。

否决 B（任务树仅 inspection，输入永远回主对话）：弱化太多，跟 TUI 心智断开。

### 轴 2 · 导航壳子 → B · horizontal pager

3 页固定 `[节点][玄女][任务树]`，私聊页是 push modal 盖在 pager 上。

否决：
- A 侧抽屉 overlay：drawer 75% 宽对长任务树不够用
- C 底部 sheet 保留：跟"右滑"原话不符 + sheet 关掉丢选中态

### 轴 3 · active target 视觉指示 → C · 整页橙色识别

选门客后整页换皮，橙色顶栏 + 任务上下文 banner + 仅显示该 worker 发言。

否决：
- A 顶栏 chip + 单线程：小 chip 容易忽略导致误发
- B composer chip：同上，权重不够；用户可能没意识到自己其实在跟某门客对话

### 轴 4 · 任务树呈现密度 → C · 两级行卡片

任务卡 header + 每门客 32px+ 行 + "›" 推入箭头。

否决：
- A 密集行直搬 TUI：22px 触控目标违反 PWA 视觉公理（≥44px）
- B 任务卡 + 门客 chip：chip 28px 仍欠；任务卡 vs 门客 chip 的层级关系不直观

## 砍掉 / 没考虑过的方案

| 方案 | 否决理由 |
|---|---|
| 加一个全局 active state 镜像 TUI | mobile 路由心智里 active 就是"当前在哪页"，多一个 mutable state 重复表达 + 容易出 bug |
| codex 私聊页 disable composer | 设计阶段被 CLAUDE.md 旧措辞误导（"codex 不支持 follow-up"）。实测 codex idle 走 intervene degrade-dispatch 正常；只有 busy 才拒。统一 4xx toast 即可，不需特判 worker 适配器类型。CLAUDE.md 那行也已改 |
| TUI Roster 直接搬手机 | 行高 22px 在手指上灾难；触控目标必须 ≥44px |
| 任务 sheet 形态保留只换内容 | "右滑出树"原话排除掉 sheet；且 sheet 关掉就丢了选中状态，跟"给门客发"语义直接冲突 |
| 单 thread 永远展示玄女 + worker 混合 | TUI 数据模型是 `dialogues: HashMap<ActiveTarget, _>`——天然 per-target；归一化到单 thread 是数据降级 + 用户搞不清当前在跟谁说话 |

## 关联

- [决策 14](14-im-mobile-frontend.md)——本决策是 14 的 v1.x 延续，骨架不变（pager + 私聊 push 是更精细的"任务卡片列表 + 内嵌 chat"实现）
- [决策 04](04-intervene-idle-degrade.md)——私聊 codex 门客的"first-turn ok, busy follow-up 不 ok"行为本质就是 04 的 degrade 逻辑
- [决策 10](10-task-bound-agents.md)——任务树的"任务 → members"分组就是 task-bound 哲学的 UI 表达
- 公理 2（玄女永远有知情权）——page 2 sticky badge "✓ 抄送 N 门客" 是这条公理在 UI 层兑现
- `memory/feedback_pwa_modern_not_tui`——触控热区 + 不抄 TUI 视觉
- `memory/feedback_divide_conquer`——8 条新拆活直接派给 fuxi-im-v1 team 的 β/ε

## 实装 ID（fuxi-im-v1 team 任务表）

- β: #25 字段补齐 + filter user-turn · #27 镜像端点
- ε: #28 App shell · #29 任务树页 · #30 私聊页 · #31 sticky badge
- team-lead: #32 改 CLAUDE.md（已完）· #33 本决策（即此文）
