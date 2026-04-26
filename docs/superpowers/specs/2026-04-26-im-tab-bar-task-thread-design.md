# IM v3 · bottom tab bar + 任务=群聊 + @ 提及私聊

**日期**：2026-04-26（同日下午）
**状态**：已通过 brainstorm（用户拍板：tab bar + 任务=群组 + chip @ 提及 + 单 @ 路由 v1 简化），进入实装
**关联**：决策 14 §A 的真正实装；**supersedes** [`2026-04-26-im-task-tree-redesign-design.md`](2026-04-26-im-task-tree-redesign-design.md)（per-worker 私聊 + horizontal pager 路线在用户实测后被否决）

---

## 一句话

把上一版 `horizontal pager + per-worker 私聊页` 重做成 `bottom tab bar [玄女][任务][节点] + 任务=群聊 thread + composer @ 提及（chip 形式）`。fuxi PWA 信息架构对齐主流 IM（微信/Slack/Discord），任务被当作"群组"——所有发言（玄女 + worker + tool calls + thinking）都 inline 在该任务的 thread 里。

## 背景

[2026-04-26 第一版重设计 spec](2026-04-26-im-task-tree-redesign-design.md) 部署到 home 后用户实测撞到反直觉：

1. **horizontal pager + push modal 双层导航对手机 IM 太重** —— 主流 IM 全是 bottom tab bar，pager 是少数派 pattern
2. **per-worker 私聊页强迫"对象中心"思维** —— 但用户实际场景是"事件中心"（"查 ERP API 这件事进展如何？"），需要在一个 thread 看完玄女+鲁班+蒲松所有人发言，而不是切两次门客拼起来
3. **决策 14 §A 原本钉死的"任务=对话 thread"心智模型** —— 第一版重设计被"右滑看任务树 / 选门客 / 回玄女像 TUI"原话拐到 active target 镜像路线，背离了决策 14 初心

用户原话："这个布局我还是感觉有点反用户直觉。用起来很懵。要不我们就做成传统 IM 那种风格？就像微信那样。一个任务就是一个组。"

同步另两条用户反馈：

- **部署解耦** —— "现在部署 IM 需要把整个伏羲系统都重新构建部署吗？" → 见 [决策 17](../decisions/17-im-deploy-decoupling.md)，单独排期，本 spec 不实装
- **PWA 信息丰富度对齐 TUI** —— "里面对应的功能和信息展示都应该是完全一致的" → 任务 thread 模型自然兑现（见 §渲染规则）

## 决策

### A · 信息架构 = bottom tab bar 三 tab

```
┌──────────────────────────────────────────────────────────┐
│                                                          │
│              （主区，按 active tab 渲染）                 │
│                                                          │
├──────────────────────────────────────────────────────────┤
│   玄女          任务          节点                       │
│   ●—high       ⚒—mid         ⛁—low                       │
└──────────────────────────────────────────────────────────┘
              ↑ bottom tab bar (固定 56px)
```

- **频次顺序**：玄女（默认 + 最高频）→ 任务（中频）→ 节点（低频）
- 每 tab 占满除 tab bar 外的全屏，无 horizontal pager swipe（主流 IM 不允许 tab 间手势切换）
- tab bar 高度 56px，触控热区 ≥ 48px

### B · 任务 = 群组心智（tab 内二层导航）

任务 tab 是**两层结构**：

```
任务 tab
   │
   ├─ Layer 1：任务列表（默认）
   │     · 进行中段（按 last_active_at 降序）
   │     · 已完成段（默认折叠 sticky tail "已完成 · N 条 ▸"）
   │
   └─ Layer 2：任务 thread（Layer 1 点任务卡 push 进入）
         · 顶栏 [‹ 任务 列表]  任务 title  [⋯]
         · 任务 banner（成员列表 + 状态 + elapsed）
         · thread 内容：玄女 + 全 worker + tool calls + thinking 全 inline 按时间
         · composer 默认对玄女说，@ 提及切到具体 worker
```

任务 thread 不再分"私聊页"——这件事的所有人都在一个 thread 里。

### C · composer @ 提及 = chip 形式 + inline autocomplete

```
composer 输入区：
┌────────────────────────────────────────────┐
│ [● 鲁班 ✕] 帮我用 grep 看一下 API 入口     │
└────────────────────────────────────────────┘
       ↑ 圆角 chip：角色色 dot · role 名 · 小 ✕
```

**chip 视觉**：
- chip 背景：`role_color` 浅色 `rgba(229,165,71,.15)`（鲁班例）
- chip 边框：`role_color` 1px solid
- chip 内：角色色 dot（6px）+ role 名（mono 小字）+ ✕（muted 灰，tap 删除 chip）
- chip 高度 24px（紧贴 composer 文字基线）

**autocomplete 弹层**（inline 紧贴 composer 上方，从下面浮起 max-height 200px）：

```
输入 @ 触发 →
┌──────────────────────────────────────┐
│ ●  鲁班     · 在跑 grep server/...   │  ← 单选高亮
│ ●  蒲松     · 待命                    │
│ ○  鲁班#2   · 编 cargo build          │
└──────────────────────────────────────┘
       (按 last_active_at 降序，fuzzy 中文/拼音匹配)
```

### D · @ 提及路由（v1 简化版）

| 当前 tab | 默认对象（无 @）| autocomplete 范围 | 多 @ 行为 |
|---|---|---|---|
| 玄女 tab | 玄女 | 全 alive workers（不含玄女） | 第一个 @ 为准；多于 1 时 toast 警示 |
| 任务 thread | 玄女（任务发起人） | 该任务的成员（含玄女、各 worker） | 同上 |

发送 → `POST /api/intervene { target: <mentioned_or_default_agent_id>, text, mentions: [agent_ids] }`：
- backend 用 `target` 字段决定路由
- `mentions` 字段保留所有 @ 的 agent_ids（含 target 自身），供：
  - 后端记入 `UserInterventionSent.mentions`（v1 不实装通知，留 v2.x）
  - 前端历史消息渲染还原 chip 视觉

### E · 部署解耦（不本 spec 实装，链 [决策 17](../decisions/17-im-deploy-decoupling.md)）

短期：本 spec 仍走 `fuxi im start` 子命令 + systemd 全重启路径
中期（决策 17）：拆 `fuxi-im` 独立 binary + 独立 systemd unit + 通过 fuxi-a2a JSON-RPC 跟 fuxi daemon 通信

## 信息架构图（实装心智）

```
┌─ MainShell ───────────────────────────────────────────────┐
│                                                           │
│  ┌─ activeTab content ─────────────────────────────────┐  │
│  │                                                     │  │
│  │  玄女 tab：用户↔玄女 thread + composer with @       │  │
│  │            (autocomplete = all alive workers)       │  │
│  │                                                     │  │
│  │  任务 tab：                                          │  │
│  │     - Layer 1 任务列表（C 方案两级行卡片改造版）   │  │
│  │     - Layer 2 任务 thread（NavigationStack push）   │  │
│  │                                                     │  │
│  │  节点 tab：节点列表（沿用现有 NodesPage）           │  │
│  │                                                     │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
│  ┌─ BottomTabBar (56px) ───────────────────────────────┐  │
│  │  ● 玄女     ⚒ 任务     ⛁ 节点                       │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

## 各 tab 详细规范

### 玄女 tab（默认 + 最高频）

**沿用上一版的 XuannvPage 内容**（用户↔玄女主对话），改动：

- **删** `sticky badge "✓ 抄送 N 门客"`（任务 tab 自身已是这个角色，redundant）
- **加** composer @ 提及支持（chip + autocomplete，候选 = `tasksOverview.running` 中所有非玄女成员去重，按 last_active_at 降序）
- 发送行为：
  - 无 @ → `intervene(xuannv, ...)`（同现状）
  - 有 @ → `intervene(mentioned_agent_id, ...)` 但**回话也显示在玄女 thread**（不切页）
- 视觉：玄女紫 `#C4A8E8` 主色不变

### 任务 tab · Layer 1 任务列表

**沿用上一版 TasksPage 的 C 方案两级行卡片视觉**，改动：

- **任务卡 header tap → push Layer 2 任务 thread**（不再"折叠展开 members"）
- members 行**仅作 inspection 不可点**——成员的发言全在 thread 里看
- 删 active 高亮（per-worker 私聊概念已去）
- 删 "›" 推入箭头（tap 整卡进 thread，不需要 affordance）
- 进行中段排序、已完成 sticky tail 不变

### 任务 tab · Layer 2 任务 thread（核心新增）

**心智参照微信群聊**：所有人在一个 thread，按时间排，按 who 标签区分。

#### 顶栏

```
[‹ 列表]   查 ERP API   [⋯]
```

- "‹ 列表" tap → pop 回 Layer 1
- iOS 边缘左滑 pop 支持（NavigationStack 已有）
- 中间 title = task.title

#### 任务 banner（顶栏下方紧贴）

```
┌──────────────────────────────────────────────────┐
│ ● 进行中 · 12s · 2门客                            │
│ 鲁班 · grep ▎蒲松 · 待命                          │
└──────────────────────────────────────────────────┘
```

- 状态 dot + 状态名 + elapsed + 门客数
- 第二行：成员列表（不可 tap），每个 = `role · last_tool_call`（截断到 12 字符）

#### thread 内容（核心）

按时间 ASC 排，事件 → 渲染规则：

| EventKind | 渲染 | who 标签色 |
|---|---|---|
| `UserInterventionSent { target, text }` | 用户气泡（右侧）+ 显示 chip 还原（如果有 mentions） | 用户气泡无 who 标签 |
| `AgentResponded` (agent=玄女) | 玄女气泡 + who="玄女" | 玄女紫 `#C4A8E8` |
| `AgentResponded` (agent=worker) | worker 气泡 + who="鲁班" | 角色色（鲁班琥珀 `#E5A547`、蒲松绿 `#A0C277`、etc）|
| `ToolCallStarted` + `ToolCallFinished` | 折叠卡 `[tool] · args · ✓ 0.4s`，tap 展开 stdout | tool card 用工具的 agent 颜色 |
| `ThinkingStarted` + `ThinkingFinished` | 折叠条 `▸ 思考 12s` | 灰斜体 |
| `TaskStateChanged{Done|Cancelled|Delivering}` | 居中 muted marker `─ 任务完成 ─` 或 `─ 已取消 ─` | muted |
| `agent_idle` | 居中 muted marker `─ 鲁班 idle ─` | muted |

**白名单原则**：跟 TUI dialogue 渲染器**保持等价**——TUI 显示什么，PWA 任务 thread 也显示。这是用户 #3 反馈"PWA 跟 TUI 信息一样丰富"的兑现。

#### composer

- 默认 `placeholder = "对玄女说..."`（紫色）
- 输 `@` → autocomplete 弹层，候选 = 该任务所有成员（玄女 + 各 worker）
- 选 worker → chip 化 + placeholder 改 `placeholder = "对鲁班说..."`（角色色）
- chip ✕ 删除 → 默认回玄女
- send → `POST /api/intervene { target, text, mentions }`
- 4xx → toast `门客正忙，等这轮跑完再发`

### 节点 tab

**沿用上一版 NodesPage** —— 节点列表 / 离线时心智。一个 tab 一个页面，没二层导航。

## composer @ 机制详细规范

### 触发

输入框检测到 `@` 字符（半角）→ 立即弹 autocomplete。中文输入法不触发（避免误弹）。

### autocomplete 弹层

- 位置：紧贴 composer 上方，从下面浮起
- max-height: 200px，超出 scroll
- 候选项：每条 ≥ 44px 触控
- 渲染：`[● role · last_tool_call/活动]`
- 选中：tap / 上下键 + Enter
- 取消：Esc / 输入框点其他位置 / 删除已输的 `@`

### chip 化

- 选中候选 → composer 文字里 `@` + 输入文本被替换成 chip（结构化），光标跳到 chip 后
- chip 是不可分割的 token，光标只能在 chip 前后，不能在 chip 内部

### 发送时序列化

文本 + chip 序列化成消息：
```json
{
  "target": "<first_mention_agent_id_or_default>",
  "text": "帮我用 grep 看一下 API 入口",
  "mentions": ["<agent_id>", ...]
}
```

文本里 chip 位置用占位标记 `​` + agent_id（前端渲染时还原）。后端不解析占位，只用 `mentions` 数组。

### 多 @ 处理（v1 简化）

- composer 允许放多个 chip
- 发送时取 `mentions[0]` 作为 target，其余仅作 mention 标记
- 多于 1 个 chip 时 send 前 toast 警示：`fuxi 当前只发给第一个 @ 的角色，其余仅作引用`
- v1.x 之后看用户撞不撞需求再实装多目标路由

## 后端 API 契约

### 沿用（已存在）

| 端点 | 状态 |
|---|---|
| `GET /api/tasks` | ✅（含 members + last_tool_call + description + status 三态） |
| `WS /api/conv` | ✅（玄女 thread） |
| `POST /api/intervene` | ✅，本 spec 加 `mentions: [agent_ids]` 字段 |
| `GET /api/workers/:id/events`, `WS /api/workers/:id/conv` | ✅，但**本 spec 后不再使用**（per-worker 私聊概念去除）。代码暂留，留作 v2.x 后续 / 备用 |

### 新增（β）

| 端点 | 用途 | 行为 |
|---|---|---|
| `GET /api/tasks/:task_id/events?from=<cursor>&limit=N` | 任务 thread 打开拉历史 | filter: `meta.task_id == :task_id` AND kind ∈ 白名单 |
| `WS /api/tasks/:task_id/conv?from=<cursor>` | 任务 thread 流式接续 | 同上 filter |

事件 filter 白名单：`UserInterventionSent` / `AgentResponded` / `ToolCallStarted` / `ToolCallFinished` / `ThinkingStarted` / `ThinkingFinished` / `TaskStateChanged` / `agent_idle`（玄女 + 全 worker 同 task_id 都通过）

注：`agent_idle` 不走 EventBus（shelf 状态），可由 task_completed 推或前端简化不渲染。

### intervene 字段扩展

`POST /api/intervene` 请求 body 增加可选字段：

```typescript
{
  target: string;        // agent_id (uuid)
  text: string;
  attachments?: ...;     // 沿用
  mentions?: string[];   // NEW: 所有 @ 的 agent_ids（含 target）
}
```

后端不强制 `mentions[0] == target`（前端保证）。后端把 `mentions` 写进发出去的 `UserInterventionSent` event 里，供历史消息渲染时恢复 chip。

## 跟前一版 spec 的差异

| 维度 | v1 spec（2026-04-26-im-task-tree-redesign）| v2 spec（本文）|
|---|---|---|
| 导航壳子 | horizontal pager 3 页 + push modal | bottom tab bar 3 tab + Layer 2 push（仅任务 tab） |
| 选 worker 后 | push 私聊页（橙色整页 + 任务 banner） | **不存在** —— 任务 thread 内 @ 提及代替 |
| active target | per-page 隐式（page 2=玄女，私聊页=worker） | **去除** —— composer 路由由 @ chip 显式决定 |
| 玄女主对话 sticky badge | "✓ 抄送 N 门客" | **删** —— 任务 tab 自身已是这个角色 |
| 任务 thread | per-worker 私聊（每 worker 独立 thread） | per-task 群聊（mix 全成员） |
| 信息架构源头 | TUI active target 1:1 镜像 | 微信群聊 + Slack @ 提及 |

## 拆活（agent team `fuxi-im-v1`）

### ε 端

| ID | subject | 复用前一版 |
|---|---|---|
| #N1' | App shell · 删 Pager + 改 BottomTabBar 三 tab | 删 `Pager.tsx`、保留 `NavigationStack`（任务 list→thread 仍 push） |
| #N2' | 任务 tab Layer 1 任务列表（点卡 push thread）| 改造现 TasksPage（去除 active 高亮 + 去除 "›" + members 行变 inspection-only） |
| #N3' | 任务 tab Layer 2 任务 thread（mix 全成员）| 改造现 WorkerPage：路由参数 agent_id → task_id；reducer 处理 task_id 全员 events；删任务 banner 单 task 限制 |
| #N4' | composer @ chip + autocomplete 组件 | 全新；玄女 tab 和任务 thread 共用 |
| #N5' | 玄女 tab 改造 · 删 sticky badge + 加 @ chip composer | 改 XuannvPage；调用 #N4' 组件 |

### β 端

| ID | subject |
|---|---|
| #N6' | 加 `/api/tasks/:task_id/{events,conv}` 镜像端点（按 task_id 过滤）|
| #N7' | `POST /api/intervene` 加 `mentions: [agent_ids]` 字段 + 写入 `UserInterventionSent.mentions` |

### 杂

| ID | subject |
|---|---|
| #N8' | 写 `docs/decisions/16-im-tab-bar-task-thread.md`（决策 16）|
| #N9' | 写 `docs/decisions/17-im-deploy-decoupling.md`（决策 17，仅排期不实装）|
| #N10' | 老 spec `2026-04-26-im-task-tree-redesign-design.md` 顶部加 `状态：superseded by 2026-04-26-im-tab-bar-task-thread-design.md`|

## 实装顺序

1. β 开 #N6' + #N7'（前端不阻塞）
2. ε 开 #N4' composer @ chip 组件（基础组件，所有 tab 都依赖）
3. ε #N1' shell 改造 + #N2' 任务列表改造（可并行 #N4'）
4. ε #N3' 任务 thread + #N5' 玄女 tab 改造（依赖 #N4'）
5. ζ 完整 rust+web 部署
6. team-lead 写决策 16 + 17 + 改老 spec 状态

## 关闭老的 follow-up

- **#34**（私聊页支持多 task tab）→ **作废**：私聊页本身去除，本设计不存在该问题
- **#35**（ToolCallCard stdout 截断 + 全文按钮）→ **保留**：任务 thread 仍渲染 ToolCallCard，仍适用

## 视觉一致性（沿用前一版 + 决策 14）

- 暖暗底 `#1F1E1B`，Anthropic 橙 accent `#D97757`
- 角色色：玄女紫 `#C4A8E8` / 鲁班琥珀 `#E5A547` / 蒲松绿 `#A0C277`
- chip 用**角色色**而非橙色 accent —— 一眼区分 @ 谁
- autocomplete 弹层用**inline 紧贴 composer**，max-height 200px
- 不用 emoji / Unicode block 装饰 / shadow / gradient / glassmorphism
- 触控热区 ≥ 44px（tab bar 项 ≥ 48px）

## 否决方案 / 反驳点

| 方案 | 否决理由 |
|---|---|
| 保留 horizontal pager（v1 spec）| 双层导航对手机 IM 太重；主流 IM 全是 bottom tab bar |
| per-worker 私聊页（v1 spec）| 强迫"对象中心"；用户实际是"事件中心"思维 |
| @ 提及 inline 蓝字（微信风格）| chip 风格更结构化、tap 删更安全；fuxi 视觉语言用 chip 跟 token-based 设计统一 |
| autocomplete 全屏底部 sheet（占 50% 屏）| 打断输入流；主流移动 IM 都是 inline 紧贴 composer |
| 多 @ 全部路由（fan-out）| 后端 intervene 是单 target；fan-out 涉及多 target 编排，v1.x 不必复杂化 |
| 玄女 tab 也允许点 worker 进私聊页 | 用户明确否决"per-worker 独立 thread"；@ 在玄女 thread 内回话已够用 |
| 节点 tab 放二级菜单（v1 仅 2 tab）| 决策 12 远期分布式时节点 tab 重要；保留位 |

## 关联

- 决策 14（IM 移动端骨架 §A）—— 本 spec 兑现 §A "任务卡片列表 + 内嵌 chat thread" 原版心智
- 决策 16（NEW · 任务=群聊模型）—— 记录 brainstorm 路径 + 否决路径
- 决策 17（NEW · IM 部署解耦排期）—— 用户 #2 反馈的中期方向
- `memory/feedback_pwa_modern_not_tui` —— 触控热区 + 不抄 TUI 视觉
- `memory/feedback_team_lead_batch_dispatch` —— 本批次按"整批 spec + dep 一次给完"模式派给 ε
