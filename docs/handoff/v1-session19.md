# Handoff · v1 · Session 19 → 20 开工指引

> 本 session（2026-05-25 → 26 凌晨）核心 = **Phase 0「artifact ref-only」ship + 部署 home**，同时
> 跟用户讨论清楚 **Phase 1「Topic 一等公民」** 设计共识。新会话从 Phase 1 起手。
>
> 上一份 handoff：`docs/handoff/v1-session18.md`（玄女上下文水位 + handoff 机制全 ship）。
> 本次新会话**不依赖** session18 的细节，但 [[reference_home_deploy]] / CLAUDE.md 的部署流程公理已踩稳。

---

## 1 · Phase 0 已 ship（HEAD `2530612` on main，home 已部署）

### 改动

| 模块 | 内容 |
|---|---|
| `fuxi-core/event.rs` | 新加 `ArtifactRef { path, summary, byte_size }` + `maybe_dump` / `maybe_dump_default` / `summarize_for_artifact_ref` helper（500 字阈值、200 字 summary 截断） |
| `EventKind::AgentResponded` | 加 `artifact_ref: Option<ArtifactRef>`（带 `#[serde(default)]` + `skip_serializing_if = None` 向后兼容） |
| `fuxi-agent-cc/parser.rs` | AssistantText / ResultSuccess 翻 AgentResponded 时调 `ArtifactRef::maybe_dump_default`：长产出落档 `~/.fuxi/artifacts/<task_id>/turn-<ts>-<short>.md`，payload text 改填 summary |
| `fuxi-agent-codex/parser.rs` | codex AgentMessage 翻 AgentResponded 同行为 |
| `fuxi-cli/subcommands.rs::event_summary` | print 带 artifact_ref 的 AgentResponded 时显示 `summary · artifact=<path>` 让玄女知道完整在哪 |
| `fuxi-firehose/tui.rs` | TUI 加 `[a]` 标记表示该 event 带 artifact |
| 22 个文件 47 处 callsite 修 | `EventKind::AgentResponded` 加字段是 breaking change，所有构造点加 `artifact_ref: None`，解构点加 `, ..` |

### 测试

- `fuxi-core`：5 个新单测覆盖 dump 落档 / short skip / summary 截断 / `artifact_ref` serde round-trip（含 None skip-serialize + 老 wire 反兼容）
- `fuxi-agent-cc`：2 个 e2e translate path 测试
- `fuxi-agent-codex`：1 个 e2e 测试
- 全 workspace fmt + clippy（`-D warnings`）+ test（除 main 上已有的 dist:: / a2a roundtrip 预存在 502/JSON-RPC 失败外）全绿

### 部署状态

- HEAD `2530612` on `origin/main`
- home rsync + cargo build release 完，`sudo systemctl restart fuxi-im.service` 已重启
- 新玄女副本 `agent-d328c4d4-a467-4741-aed2-512321460d83`，启动 log 干净
- 旧 5 个 cangjie/luban 全回收（fuxi 重启常态，按需 dispatch 时再起）
- **下次用户跟玄女对话时，长产出会自动走 ref-only**——验收点：`ls ~/.fuxi/artifacts/` 有目录 + `fuxi events --task X` 看 AgentResponded 那行带 `· artifact=` 后缀

---

## 2 · Phase 1 设计共识（用户已拍板）

### 用户暴露的真痛点

5/22 用户原话：「最近用 fuxi 很费劲。你听不懂我要干啥，输出一大坨，上下文污染」  
5/23 用户主动提议：「我跟你聊新话题但要聊很长时间，门客做完事件中间打断怎么处理？是不是需要加一个话题功能？」

**Phase 0 治了「输出一大坨」的一半（context 烧速），Phase 1 治多话题打断 + cc context 单线性污染**。

### 行业方案研究结论

- **LangGraph thread_id**：thread_id 是 checkpointer 主键，每个 thread 独立 state，跨 thread 走 `Store` namespaced key——thread = 数据库主键不是 UI 概念
- **Claude Code subagent**：父子只两个 string（prompt in / final out）严格隔离，**fuxi 当前把 worker 中间事件灌回玄女破了这个原则**——Phase 0 已部分治
- **Anthropic multi-agent research**：subagent 写外部存储 + 回轻量 ref，避免 "game of telephone"——已是 Phase 0 落地的设计
- **OpenAI Assistants 单 thread 串行 run** = 反例，**就是 fuxi 当前痛点**，不要学
- **Slack threads**：「把 reply 从 channel 抽出来是最有意义的改动」（设计师原话）——门客事件**默认不进主聊**，进对应 topic
- **ChatGPT/Claude.ai/Slack/Telegram/Cursor**：桌面 left sidebar list 是默认范式；移动端抽屉，主屏 100% 给对话
- **Telegram Topics vs Discord threads**：入口必须一级（不能藏消息底下），不允许多级嵌套

### 用户拍板的决策

| # | 问题 | 用户答 |
|---|---|---|
| 决策 1 | 玄女 cc 进程怎么对应 topic？ | **fuxi 重建 prelude，每切 topic 重启 cc**（不依赖 cc resume，复用现有 xuannv_handoff 机制） |
| 决策 2 | Phase 0 ref-only 是否独立先 ship？ | **先 ship**（已完成，HEAD `2530612`） |
| 决策 3 | pin topic 到顶？ | ❌ 第一版不做，Phase 2 |
| 决策 4 | 移动端 sidebar vs 底 tab？ | ✅ **左滑抽屉**，主屏 100% 给对话 |
| 决策 5 | 门客 inbox 在 PWA 暴露？ | ✅ **暴露但默认折叠**，透明优于黑盒 |
| 决策 6 | topic 标题谁起？ | 第一版用户输入，玄女可建议；归档不删 |
| 决策 7 | 跨 topic broadcast 阈值？ | 进 inbox 的 milestone：`deliverable_produced` / `agent_dead` / `error` / `agent_request_review`，其他 `task_*`/`tool_call_*` 不出本 topic |

---

## 3 · Phase 1 实施骨架（三件事可拆开）

### 3.1 数据层：`topic_id` 进一等公民

- 新表 `topics(id, title, created_at, last_active_at, pinned, archived_at)` 在 `fuxi-im` 的 `im.db`（或可考虑放 `events.db` 让 controller 视角统一——待定）
- `events.db.events` / `tasks` / `im.db.messages` 都加 `topic_id` 列
- 老数据全归默认 topic `"general"`
- **必带 `#[serde(default)]`**（CLAUDE.md 公理：加字段必默认兼容老持久化，`Project.host_nodes` / `Task.project_id` 那两次踩坑反回归单测在 fuxi-core，照样兜）

### 3.2 切 topic = 重启 cc + prelude 重建

**复用现有 `xuannv_handoff` 机制**（CLAUDE.md 已验证路径走得通）：

1. 切前：当前 cc 进程优雅退（emit `AgentShuttingDown { reason: "topic_switch" }` 走 `Fuxi::shutdown_xuannv_for_handoff` 绕豁免）
2. fuxi 拉新 topic 的：① 最近 N 条对话（默认 50）② 该 topic 进行中的 task 状态摘要 ③ 跨 topic inbox 简报
3. 拼成 ≤ 1500 字 prelude → 起新 cc 进程，prelude 作为 `--append-system-prompt` 或 first user message
4. 玄女接到：「✻ 切到 <topic-title>，上文如下：...」继续对话

### 3.3 路由层：玄女只收 current topic 事件

- `SystemEventBridge`（`crates/fuxi-orchestrator/src/bridge.rs`）加 `current_topic_id` filter
- 非 current topic 的 worker task 事件 → 攒进「玄女 inbox」（fuxi 内存结构 + DB 持久化）
- 切回该 topic 时 prelude 把 inbox 该 topic 的部分 surface
- 公理 2 知情权保持：跨 topic 关键 milestone（决策 7 阈值）默认进 inbox，玄女**主动**查（`fuxi events --topic X`）而非被动注入

### 3.4 UI

- PWA 桌面 left sidebar topic list（240px 常驻）：title / 未读 badge / 最近预览 / pin
- 移动 PWA 左滑抽屉
- 「所有进行中话题」聚合页（Slack All Threads 范式兜底）
- ❌ 不做 project dashboard（ChatGPT 2025 反例）
- ❌ 不做顶 tab（玄女 5/23 自己否过：横向滚动比垂直差）

---

## 4 · 关键代码定位（让新会话不重复摸代码）

| 主题 | 文件 | 行号 / 关键函数 |
|---|---|---|
| EventKind 定义 | `crates/fuxi-core/src/event.rs` | L186-652 `enum EventKind`；`ArtifactRef` 在 L100+ |
| 加 EventKind 变体必同步 6 处 | - | `fuxi-events/src/store.rs::kind_tag@342` · `fuxi-firehose/src/hub.rs::kind_tag@284` · `fuxi-firehose/src/tui.rs::summarize@377 / color_for@625` · `fuxi-cli/src/subcommands.rs::event_summary@601` · round-trip 测试 `event.rs@654+` |
| agent-cc parser 翻 final | `crates/fuxi-agent-cc/src/parser.rs` | L476 `AssistantText`（流式） · L562 `ResultSuccess`（终态）· `TranslateState.responded_this_turn` 是双发 bug 修补点 |
| agent-codex parser 翻 final | `crates/fuxi-agent-codex/src/parser.rs` | L355 `AgentMessage`（codex 一次一段）· `state.last_agent_message` |
| SystemEventBridge 注入玄女 prompt | `crates/fuxi-orchestrator/src/bridge.rs` | L349 `build_task_done_prompt`（兜底） · L382 `build_request_review_prompt` · L770 TaskStateChanged→Done 注入 · L1437 「AgentResponded 默认 silent」注释 |
| xuannv_handoff 切玄女机制 | `crates/fuxi-cli/src/xuannv_handoff.rs` + `crates/fuxi-orchestrator/src/fuxi.rs::shutdown_xuannv_for_handoff` | 已 ship（session18），prelude 模式可复用 |
| IM notifications / issue 工作流 | `crates/fuxi-im/src/notifications.rs` | NewNotification / IssueStatus / link_fix / close |
| IM conversations / messages 表 | `crates/fuxi-im/src/conv_store.rs` + migrations | 玄女主线 `scope="xuannv"`，子线 `scope="task:<task_id>"` — Phase 1 加 `scope="topic:<topic_id>"` 或者重设计 |
| PWA 主体 | `crates/fuxi-im/web/src/views/` | sidebar 加在 `App.tsx` / `views/pages/` 下 |

---

## 5 · 推荐起点：先做数据层 + 路由层（后端独立可 ship）

新会话起手按这个顺序：

1. **建 `feat/fuxi-topic-first-class` 分支**，从 `origin/main`（HEAD `2530612`）起
2. **TDD 红**：在 fuxi-core 加 `pub struct TopicId(Uuid)`、`pub struct TopicMeta { ... }`，写 round-trip + legacy 反兼容测试
3. **EventKind/Task/Message 加 `topic_id: Option<TopicId>`**（带 `#[serde(default)]`），扫 callsite 修（cargo check workspace 报错位置即清单——参考 Phase 0 时 sweeping agent 模式，可委派给 sub-agent 跑机械改动）
4. **新表 `topics`**：决定放 `im.db` 还是 `events.db`（建议 `im.db`，因为 topic 是用户视角概念，跟 conversations/messages 同 db 便于 join；后端事件流只需 topic_id FK 不需要 join）
5. **`Fuxi::switch_topic(new_topic_id)`** 在 orchestrator：内部走 `shutdown_xuannv_for_handoff` + 起新 cc + 注 prelude
6. **`fuxi topic new/switch/list/archive` CLI**
7. **`SystemEventBridge` 加 topic filter + inbox**——这一步影响行为，要带 e2e 覆盖
8. PWA 前端 sidebar——可并行 spawn 一个前端 agent 做，主线干后端
9. 部署 home 实测（参考本 handoff §1 部署流程）

**别动**：CLAUDE.md 5/11 那条 cc session-id bug 决策——`session_id: None, resume_session_id: None` 保持四处一律 None，不要因 Phase 1 改回 strict resume，那条路径已死。

---

## 6 · 反公理 / 边界提醒

- **不要做 sub-topic**：Slack 设计师明确否决多级嵌套
- **不要在移动端做双栏**：主屏 100% 给对话是 5 个产品共识
- **不要做 project dashboard 聚合页**（ChatGPT 2025 反例）
- **不要为切 topic 引入 cc `--resume`**（CLAUDE.md 5/11 session-id 死路）
- **新加 EventKind 变体必同步 6 处** + 反回归 round-trip 单测（`task #8 实测踩过 UsageReport/XuannvContextWatermark/XuannvHandoffWritten 三变体撞了 6 处编译错`）
- **Cargo.lock cherry-pick 后常坏**：多 teammate 改 Cargo.toml 时 lock 自动合并失败。直接 `rm Cargo.lock && cargo build` 重生
- **玄女主聊 conv `48dbc201-...`** 是历史载体，不要重设计 conv_store schema 时丢老消息——`#[serde(default)]` 兼容老 `scope="xuannv"` 必须保留
