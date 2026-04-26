# IM v2 · 任务 sheet → 任务树 + active target 私聊页 重设计

**日期**：2026-04-26
**状态**：已通过 brainstorm（用户拍板 ABCC，跳过 spec 复审），进入实装
**关联**：决策 14（IM 移动端骨架）的延续，决策 04（intervene degrade）的 UI 落地

---

## 一句话

把当前平铺卡片的"任务"sheet 重构成 TUI Roster 风格的两级行任务树；选门客触发"整页 push 私聊页"；同时把 fuxi-im PWA 的导航壳子从 BottomSheet 改成横向 pager，把 TUI 的 active target 模型 1:1 搬到手机端。

## 背景

决策 14 把 IM PWA 立起来后，到 04-26 用户实测发现：

1. **任务 sheet 信息空洞**：列表里全是 `#xxx user-turn` 内部对话条目（β #25 in_progress 修过滤）
2. **跟 TUI 的"任务树"心智模型脱节**：sheet 是平铺，TUI 是分组+成员的树
3. **缺"给特定门客发消息"通道**：TUI 里 F4→Roster→Enter→Worker 切换是日常用法，PWA 完全没对应
4. **BottomSheet + 单 Conversation 导航**与"右滑出树"原话不匹配，且 sheet 关掉就丢了选中状态

用户原话："这个任务需要重新设计 / 要和 TUI 的任务树那种结构 / 我想 ux 是右滑看任务树，然后可以选择对应的门客然后可以回玄女就像 TUI 的交互差不多。只不过 TUI 是快捷键。"

## 决策（brainstorm 已确认 ABCC）

### A · 输入路由模型 = mirror TUI active target

- 默认 page 2 composer 走 `intervene(xuannv, ...)`
- 选门客 → push 私聊页 → 该页 composer 走 `intervene(worker_id, ...)`
- pop 私聊页 = "回玄女"
- 玄女抄送由后端 A2A 层做（公理 2 不变），UI 不混合 dialogue

**实现注**：active target 不需要独立的全局 state——它隐式由"当前 page" 决定。page 2 永远 = Xuannv，私聊 page 永远 = Worker(id)。比 TUI 多一个 page 层，少一个全局 mutable state，更符合 mobile 路由心智。

### B · 导航壳子 = horizontal pager（3 页固定）

```
[1·节点]   ←→   [2·玄女主对话]   ←→   [3·任务树]
```

- 页 2 永远是玄女主对话（base camp）
- 页 1 是节点列表（沿用决策 14 既定），页 3 是任务树（新设计的核心）
- pager 顶部 dots 三点指示当前页

私聊页**不进 pager**——它是 NavigationStack 风格的 modal，从右侧 push 盖在 pager 上。

### C · 私聊页 = 整页橙色识别 + 任务上下文 banner

选门客后页面整体换皮：

- 橙色顶栏（Anthropic 橙 `#D97757`）+ "‹ 玄女" 返回按钮
- 顶栏中间 label 是门客角色名（橙色）
- 顶栏下方 task context banner：`[任务] 查 ERP API · 12s · 进行中`
- composer placeholder 橙化（`跟鲁班说...`），send 按钮橙化
- thread 内容仅显示该 worker 的活动 + 用户对该 worker 的发言

### C · 任务树呈现 = 两级行卡片

- 任务 = 卡片 header（title + elapsed + 门客数）
- 每门客 = 32px+ 行（role 加粗 + 当前 tool 副文本 + "›" 推入箭头）
- active 门客行高亮橙边（左侧 2px border + 浅橙背景）
- 默认排序：进行中段在上，按最近活动降序；已完成段在下，**默认折叠**为一行 sticky tail "已完成 · N 条 ▸"

## 信息架构图

```
┌─ Pager (horizontal swipe) ─────────────────────────────────┐
│                                                             │
│   Page 1 [节点]    Page 2 [玄女主对话]    Page 3 [任务树]    │
│                          ↑                       │          │
│                          │ ← 玄女                ▼          │
│                          │                  tap 门客行       │
│                          │                       │          │
│                  ┌───────┴───────────────────────┘          │
│                  │  push as modal (slide from right)        │
│                  ▼                                          │
│              [私聊页 · 鲁班 (active=Worker)]                │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

## 各页详细规范

### Page 2 · 玄女主对话（沿用 + 加 sticky badge）

现有的 `Conversation.tsx` + `XuannvBubble` + `UserBubble` + `Composer` 全保留。新增：

- **顶栏右侧 sticky badge**：`✓ 抄送 N 门客` —— 仅当 `tasks.in_progress.length > 0` 显示
- 点 badge → swipe 到 page 3（任务树）
- 用户感知"玄女在听 N 个门客的活"，不需翻页确认

### Page 3 · 任务树

数据源：`GET /api/tasks`（β #25 落字段后用真接口）

布局（伪结构）：

```
顶栏：[‹ 玄女]  任务树  [⋯]
pager dots：· · ●

进行中
┌─────────────────────────────────┐
│ 查 ERP API           12s · 2门客 │  ← 任务 header (tap = 折叠/展开 members)
├─────────────────────────────────┤
│ 鲁班                          › │  ← member 行 (tap = push 私聊)
│ grep · server/api/v1.go         │
│ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │
│ 蒲松                          › │
│ 待命                            │
└─────────────────────────────────┘

┌─────────────────────────────────┐
│ 修 multipart                3m │
├─────────────────────────────────┤
│ 鲁班#2                        › │
│ cargo build                     │
└─────────────────────────────────┘

──────────────────────────────────
已完成 · 12 条 ▸             (sticky tail, tap 展开)
```

**交互细则**：

| 元素 | 触发 | 动作 |
|---|---|---|
| 任务 header 整行 | tap | 折叠/展开 members（视觉态，不切 active） |
| member 行 | tap | push 私聊页 + 切 active=Worker(id) |
| "›" 箭头 | tap | 同 member 行 tap |
| 已完成 sticky tail | tap | 展开已完成段 |
| 长按 member | （v1 不实装，留 affordance） |

### 私聊页（modal push 在 pager 上）

数据源：`GET /api/workers/:agent_id/events?from=<cursor>` 拉历史 + `WS /api/workers/:agent_id/conv` 流式接续（**新端点，β #N5**）

布局：

```
[‹ 玄女]   鲁班   [⋯]            ← 橙色顶栏

┌─ 任务上下文 banner (橙色边) ──────┐
│ [任务] 查 ERP API · 12s · 进行中 │
└──────────────────────────────────┘

[bubble: 鲁班说 ...]              ← who 标签橙色
[bubble: 用户 ...]                ← 右侧
[tool card: ▸ Bash · grep ✓ 0.4s]  ← 折叠态
[bubble: 鲁班说 ...]
[thinking: ▸ 思考 12s]            ← 折叠态

─────────────────────────────────
[＞ 跟鲁班说...]               [发]  ← composer 橙
```

**渲染规则**：

| 块 | 来源 EventKind | 视觉 |
|---|---|---|
| 用户气泡 | `UserInterventionSent { target == 该 worker }` | 右侧 `#3A2A1E` |
| 门客气泡 | `AssistantText` (where `meta.agent == 该 worker`) | 左侧 `#2A241E` + 橙 who + 字符级流式 |
| 工具调用卡 | `ToolStarted` + `ToolFinished` 配对 | 折叠条 `Bash · grep ... ✓ 0.4s`，tap 展开看 stdout（前 20 行截断 + 全文按钮）|
| thinking | `ThinkingStarted/Done` | 灰色斜体折叠条 `▸ 思考 Ns`，tap 展开 |
| 状态变更 marker | `task_completed`、`agent_idle` | 居中 muted 行 `─ 鲁班 idle ─` |

**不渲染**：玄女对该 worker 的 `dispatch` 文本（平台动作不是对话）、其他 worker 活动、平台内部 event。

**composer 行为**：

- placeholder + send 按钮均橙色
- send → `POST /api/intervene { target: <worker_agent_id>, text }`
- send 后用户气泡立即上屏，留在私聊页（不 auto-pop）
- 4xx / err → toast `门客正忙，等这轮跑完再发`（统一文案，不区分 worker 适配器类型——codex/cc 一视同仁）

**返回**：

- 顶栏 "‹ 玄女" tap → pop 回 pager（落 page 2 玄女主对话）
- iOS 边缘左滑也 pop（NavigationStack 原生支持）
- pop 时 active 自动回 Xuannv

### Page 1 · 节点（沿用决策 14）

不变。现有 NodesSheet → 直接 promote 成 page 1（去掉 BottomSheet 容器）。

## 后端 API 契约

### 沿用（已存在）

| 端点 | 状态 |
|---|---|
| `GET /api/tasks` | β #21 ✅；β #25 in_progress 补字段 + 过滤 |
| `WS /api/conv` | γ ✅ |
| `POST /api/intervene` | β #13 ✅，已支持任意 agent_id 目标 |
| `POST /api/dispatch` | β #13 ✅ |

### 新增（β）

| 端点 | 用途 | 行为 |
|---|---|---|
| `GET /api/workers/:agent_id/events?from=<cursor>` | 私聊页打开拉历史 | 镜像 `/api/conv` 历史端点；filter `meta.agent == :agent_id`；cursor 格式与 `/api/conv` 一致 |
| `WS /api/workers/:agent_id/conv` | 私聊页流式接续 | 镜像 `/api/conv` WS；filter 同上 + 接受 `UserInterventionSent { target == :agent_id }` |

`agent_id` 路径参数为 UUID 字符串（URL safe）。

事件 filter 白名单：`AssistantText` / `ToolStarted` / `ToolFinished` / `ThinkingStarted` / `ThinkingDone` / `UserInterventionSent { target == :agent_id }` / `task_completed` / `agent_idle`

### 字段补齐（β #25 已含）

`GET /api/tasks` 每个 task 的 members 数组里每个成员需要：

- `agent_id: str`
- `role: str`（如 "鲁班"）
- `status: "busy" | "idle" | "dead"`
- `last_tool_call: str | null`（如 `"Bash · grep server/api/v1.go"`）
- `description: str | null`（task 派给 worker 时附的 description）

## 跟在飞任务的关系

| 任务 | 现状 | 处理 |
|---|---|---|
| #16 ε PWA v2 重做 | in_progress | **本设计是 #16 的延续，close** |
| #18 ε markdown + 附件 + 历史预加载 | 代码已落 | **mark completed** |
| #20 ε 阶段 4 任务 sheet + 节点 sheet | sheet 形态被淘汰 | **close** |
| #23 β multipart 400 | 与本设计无关 | **不动**，β 继续修 |
| #25 β 任务 sheet 过滤 + 字段对齐 | 仍需要 | **保留**（改 title 成 "GET /api/tasks 字段补齐 + filter user-turn"） |
| #26 ε 任务 sheet 卡片信息密度对齐 | completed | sheet 淘汰但 codepath 部分能复用进新树 |

## 拆活（agent team `fuxi-im-v1`）

### ε 端

| ID | subject | description |
|---|---|---|
| #N1 | 重构 App shell 成 horizontal pager + NavigationStack | BottomSheet + 单 Conversation 改成 3 页 pager（[节点·玄女·任务树]）+ NavigationStack 容纳私聊 push；pager dots 指示器；iOS-style swipe 边缘 pop |
| #N2 | 任务树页（page 3）实装 C 方案两级行卡片 | 进行中/已完成分段；已完成默认折叠 sticky tail；任务 header 折叠/展开；门客行 tap → push 私聊 + 切 active=Worker(id)；接 `GET /api/tasks` |
| #N3 | 私聊页 modal 实装 C 方案橙色识别 + 任务 banner | 橙色整页；任务上下文 banner；工具调用/thinking 折叠卡；composer 橙化；send → /api/intervene；4xx toast；接 `WS /api/workers/:id/conv` + history 端点 |
| #N4 | page 2 玄女主对话顶栏 sticky badge "✓ 抄送 N 门客" | tap badge = swipe 到任务树页 |

### β 端

| ID | subject | description |
|---|---|---|
| #N5 | 镜像端点 `/api/workers/:agent_id/{events,conv}` | history GET + WS stream，filter by `meta.agent`；事件 kind 白名单见 spec |
| (#25) | 改 title 成 "GET /api/tasks 字段补齐 + filter user-turn" | 已 in_progress，沿用，加 last_tool_call/description 字段 |

### 杂

| ID | subject | description |
|---|---|---|
| #N7 | 改 CLAUDE.md codex follow-up 注释 | 准确措辞为「codex worker busy 时不支持 send_message follow-up；idle 走 intervene degrade-dispatch 是正常路径」 |
| #N8 | 写 docs/decisions/15-im-task-tree-pager.md | 决策记录，引本 spec |

## 实装顺序

1. β 开 #N5（前端干等）
2. ε 同时开 #N1（shell 重构，无后端依赖）
3. β #25 / #N5 落地后 ε 接 #N2 / #N3 / #N4
4. ζ 一发 `./deploy/im/install.sh --apply` 部署

预计：β 半天，ε 一到两天。

## 视觉一致性（决策 14 + memory/feedback_pwa_modern_not_tui）

继续遵守：

- 暖暗底 `#1F1E1B`，Anthropic 橙 accent `#D97757`，奶白文字 `#F5F1E8`
- 角色色：玄女紫 `#C4A8E8`、鲁班琥珀 `#E5A547`、蒲松绿 `#A0C277`
- **不用 emoji**、**不用 Unicode block 装饰**（▎┌─└→… 全 TUI 语言，PWA 不抄）
- 触控热区 ≥ 44px（A 方案密集行被否决就是因为这条）
- 等宽字体仅 code block / 工具输出 / agent_id

## 砍掉的方案 / 否决理由

| 方案 | 否决理由 |
|---|---|
| 侧抽屉 overlay（导航 A） | drawer 75% 宽对长任务树不够用 |
| 底部上拉 sheet 保留（导航 C） | 跟用户原话"右滑"不符 + sheet 关掉丢选中态 |
| 顶栏 chip 单线程（active 指示 A）| 小 chip 容易忽略导致误发 |
| composer chip 单线程（active 指示 B）| 同上，chip 视觉权重不够 |
| 密集行直搬 TUI（树密度 A） | 22px 触控目标违反 PWA 视觉公理 |
| 任务卡 + 门客 chip（树密度 B）| chip 28px 仍欠；任务卡 vs 门客 chip 层级关系不直观 |
| 给 codex 私聊页加 disable banner | 误判：idle codex 走 dispatch degrade 没问题；只有 busy 时 follow-up 不行；统一 toast 即可 |

## 关联

- 决策 04（intervene idle 自动 degrade）—— 私聊 codex 门客的"first-turn ok, busy follow-up 不 ok"行为本质
- 决策 14（IM 移动端骨架）—— 本 spec 是其 v1.x 延续
- 公理 2（玄女永远有知情权）—— page 2 sticky badge 是 UI 层兑现
- memory/feedback_pwa_modern_not_tui —— 触控热区 + 不抄 TUI 视觉
