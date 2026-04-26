# Decision 16 · IM 信息架构 = bottom tab bar + 任务=群聊 thread + @ 提及

**日期**：2026-04-26（同日下午）
**状态**：已采纳（用户拍板，spec 已 commit）
**spec**：[`docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md`](../superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md)
**supersedes**：[决策 15](15-im-task-tree-pager.md)（horizontal pager + per-worker 私聊页路线）

## 背景

[决策 15](15-im-task-tree-pager.md)（同日上午）的 ABCC 路线（horizontal pager + 整页橙色识别 + per-worker 私聊页）部署到 home 让用户实测后撞到反直觉。用户原话："这个布局我还是感觉有点反用户直觉。用起来很懵。要不我们就做成传统 IM 那种风格？就像微信那样。一个任务就是一个组。"

伴随两条系统性反馈：

1. **部署解耦**——"现在部署 IM 需要把整个伏羲系统都重新构建部署吗？" → 见 [决策 17](17-im-deploy-decoupling.md)
2. **PWA 跟 TUI 信息等价**——"两者都是 IM 层的，里面对应的功能和信息展示都应该是完全一致的" → 落实在 spec 的 §渲染规则白名单（与 TUI dialogue 渲染等价）

## 决策

### A · bottom tab bar 三 tab

```
[玄女]  [任务]  [节点]   ← 频次 高→低
```

主流手机 IM（微信/Telegram/iMessage/Slack/Discord）全部用 bottom tab bar，是用户已经熟悉的 muscle memory。决策 15 的 horizontal pager 是少数派 pattern，第一性认知成本高。

### B · 任务 = 群聊 thread

每个任务点进去就是一个**群聊 thread**：玄女 + 全 worker + tool calls + thinking 全 inline 按时间。这是决策 14 §A 钉死的原版心智模型——决策 15 在 brainstorm 期被"右滑看任务树 / 选门客 / 回玄女像 TUI"原话拐到了 active target 镜像路线，背离初心。

任务=群聊的优势：
- 用户场景是"事件中心"思维（"查 ERP API 这件事进展如何？"），不是"对象中心"
- 一个 thread 看完所有人发言，不需要切来切去拼信息
- 心智负担最小（人人都用过群聊）

### C · @ 提及 = chip 形式 + inline autocomplete

私聊场景**两种用一种 @ 机制覆盖**：

| 场景 | 入口 | autocomplete 范围 |
|---|---|---|
| 任务时介入特定 worker | 任务 thread composer | 该任务的成员 |
| 纯找某角色（无任务上下文）| 玄女 tab composer | 全 alive workers |

UI 完全一致，只是候选列表不同。chip 风格胜于微信 inline 蓝字——结构化、可 tap 删、跟 fuxi token-based 设计统一。

### D · 单 @ 路由 v1 简化

后端 `intervene` 是单 target，多 @ 第一个为准，其余仅作 mention 标记。toast 警示。fan-out 路由留 v1.x 之后看用户撞不撞。

## 砍掉 / 没考虑过的方案

| 方案 | 否决理由 |
|---|---|
| 保留 v1 spec 的 horizontal pager + per-worker 私聊页 | 用户实测反直觉，明确否决 |
| @ 提及用微信 inline 蓝字 | chip 风格更结构化 + 视觉一致 |
| autocomplete 弹层占屏 50% | 打断输入流，主流移动 IM 都是 inline 紧贴 |
| 玄女 tab 也允许 push per-worker 私聊页 | 用户明确否决"独立 thread"；@ 在玄女 thread 内回话已够用 |
| 部署解耦本 spec 实装 | 工作量大，单开决策 17 排期，本设计中期方向 |
| 多 @ fan-out 路由 v1 实装 | YAGNI；用户撞了再说 |
| 节点 tab 放二级菜单（v1 仅 2 tab）| 决策 12 远期分布式时节点 tab 重要，保留位 |

## 关键复用（v1 spec 已 ship 代码不全废）

ε 在决策 15 路线下已 ship 165 unit + 24 e2e 全绿（commit ee34f8d / c52c908 / 983a6bf / fda219b）。本决策的实装路径**最大化复用**：

- **保留**：`WorkerBubble` / `ToolCallCard` / `ThinkingRow` / `StatusMarkerRow` 渲染组件 → 改名 `MessageBubble` / 通用化
- **保留**：`NavigationStack` → 任务列表 → 任务 thread 的 push 仍用得着
- **改造**：`applyWorkerEvent` reducer → `applyTaskEvent`（按 task_id 而非 agent_id 过滤，混合全成员）
- **改造**：`WorkerPage` → `TaskThreadPage`（路由参数 agent_id → task_id）
- **改造**：`TasksPage` 任务卡 → tap 整卡 push thread（去除展开 members + active 高亮 + "›" 推入箭头）
- **删**：`Pager.tsx` + `XuannvPage` 顶栏 sticky badge
- **新建**：`BottomTabBar.tsx` + `MentionChip.tsx` + `MentionAutocomplete.tsx`

## 关联

- [决策 14](14-im-mobile-frontend.md) §A —— 本决策兑现其原版心智模型
- [决策 15](15-im-task-tree-pager.md) —— superseded；保留作为否决路径的历史记录
- [决策 17](17-im-deploy-decoupling.md) —— 同时由用户提出，单独排期
- 公理 2（玄女永远有知情权）—— 任务 thread 模型自然兑现（玄女抄送 inline 在 thread）
- 公理 7（毕设不是 DDL，是顺带）—— 重做 v1 spec 是为长期日常体验，不是为 demo
- `memory/feedback_pwa_modern_not_tui` —— 不抄 TUI 视觉
- `memory/feedback_team_lead_batch_dispatch` —— 本批次按"整批 spec + dep 一次给完"派给 ε
