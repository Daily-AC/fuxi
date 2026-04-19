# 多 agent 编排 survey · 伏羲 v1 补课蓝图

> 调研日期：2026-04-19
> 目的：用主流编排平台照一遍镜子，定伏羲 v1 该补哪些、留哪些给 v2。
> 范围：8 个对标项目 + 伏羲 `fuxi-orchestrator` 现状对比。

---

## 1. 八个项目一句话定性

| 项目 | 范式 | 核心价值（亲缘判断） |
|---|---|---|
| **ComposioHQ/agent-orchestrator**（AO） | 单 orchestrator + 插件化 session manager，8 槽（Runtime/Agent/Workspace/Tracker/SCM/Notifier/Terminal + Lifecycle） | **最直接对标**。声明式 YAML + 轮询式 lifecycle 状态机 + 独立 `recovery` 子系统（scan→validate→execute action）。orchestrator 是只读观察者，不能直接"接管对话"。 |
| **team-anya**（AgentBroker + Franky + Loid） | 按角色（Loid/Yor/Franky/Bond）各一个 Orchestrator 壳，统一 `AgentBroker` spawn；每 role 一个 `RoleInstanceManager`（watchdog/ratelimit/session DB） | 单机 DB + 事件 emit + 角色隔离 quota。`spawning = Map<id, Promise>` 防并发 spawn、cc_status 持久化恢复、spawnModeMap (resumed vs fresh) 都是"进程断了能接回来"的硬实战经验。 |
| **sia-dev/sia** | `Router` + `ForkManager` + `CCBroker` 三件套；"主 agent + specialist fork"双层树（深度硬限 2） | **最接近伏羲抄送/转交心智**。Fork 有独立 eventSessionId，完成时 `appendMerge` 把摘要回灌主流；`respawnFork` 用历史事件重建 context。这套就是伏羲 ConversationSwitch 的低阶实现。 |
| **microsoft/autogen (v0.4)** | Actor model + async message bus；AgentChat 在 Core 上做 GroupChat/SocietyOfMind 预设，speaker selector 决定谁说话 | 给"多门客自治对话"范本。v0.4 并入 Microsoft Agent Framework，项目 maintenance-only——思想借鉴，代码别押。 |
| **langchain-ai/langgraph** | Stateful graph，节点即 agent，`Command(goto, update, payload)` 统一控制流+状态更新；`langgraph-supervisor` 封装 handoff tool | 把"handoff"抽象成工具调用返回值，是个非常干净的 wire 格式：目标 + payload + 状态 patch。伏羲 ConversationSwitch 可直接借这个三元组。 |
| **All-Hands-AI/OpenHands** | EventStream pub/sub + Agent Controller loop（step(state)→Action→Runtime→Observation） + Docker runtime sandbox | EventStream + Action/Observation 对偶是业界最干净的 headless agent 抽象。v1 SDK 开始推"可组合 agent"。伏羲 EventBus 思路一致但更细粒度（30+ 事件）。 |
| **crewAI** | Role/Goal/Backstory agent + Task + Process(Sequential/Hierarchical)；manager agent 按 capability 分派 | 动态 role + task→capability 匹配的工业级案例。`allowed_agents` 白名单、manager 自动或显式指定。伏羲"招贤"要抄这套 role card schema + matcher。 |
| **smtg-ai/claude-squad** | Go TUI + tmux session 隔离 + git worktree 隔离，纯客户端编排，无 server | 跟伏羲的 `GitWorktreeWorkspace` 思路完全一致（我们走了相同的隔离路子），但它没有 orchestrator——只是并排调度。能赢它，靠的就是玄女。 |
| **Anthropic sub-agents**（官方） | 一个 operator 父 agent + 多个 narrow sub-agent（Explore/Plan + 自定义），每个独立 context window + 独立工具白名单 | 官方推"窄职能 sub-agent"的命名约定（`repo-explorer` 而非 `frontend-engineer`），`description` 字段驱动自动路由。玄女门客 profile 应该照这个规格设计。 |

---

## 2. 能力矩阵对比

| 维度 | 伏羲现状 | AO | anya | sia | autogen | langgraph | OpenHands | crewAI | claude-squad | sub-agents |
|---|---|---|---|---|---|---|---|---|---|---|
| **spawn 模型** | `spawn_worker` + `dispatch_to_any`（role→idle 复用） | YAML 声明式 + session-manager.spawn（1088 行） | `broker.spawn`（role 级 quota + promise 去重） | `broker.spawn` + `forkManager.createFork`（树深 2） | actor spawn（runtime 级） | 静态 graph 节点 | AgentHub delegate | `crew.kickoff()` 按 process 调度 | tmux 新 session | SDK `Task` 工具发起 |
| **消息路由** | `dispatch(agent_id, task)` 直连；事件回流 EventBus | `sessionManager.send`；无 orchestrator 回路 | `sendPrompt(id)` + EventEmitter | `Router.sendTo(sessionKey)` + `forkManager.sendToFork` | 异步消息 + speaker selector | `Command(goto=..., payload=...)` | EventStream pub/sub | manager → worker 直派 | 无（各 session 独立） | 父 agent 工具调用 |
| **状态机** | `ShelfStatus { Idle\|Busy\|Dead }`（薄） | `CanonicalSessionLifecycle {session, pr, runtime}` 三维 + 15+ status | per-role `state: connecting\|active\|rate_limited\|...` | ForkSession `active\|completed\|abandoned` | agent 内部 | graph state | Action/Observation 对偶 | task state | tmux alive? | Claude 内部 |
| **介入 / interrupt** | ✅ `intervene(interrupt\|append)` + 三事件（UserInterventionSent/AgentInterrupted/TaskInterventionApplied） | ❌ 无原生介入；`sessionManager.send` 直写 worker | ❌ | 部分（用户切 fork） | GroupChat 中断 | `interrupt_before` 节点 | ✅ human-in-loop action | manager 打断 worker | tmux attach 手动 | 父 agent 决定 |
| **抄送 / CC** | 🟡 设计中（InterventionProxy 未实装） | ❌（orchestrator 只读） | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **主对话权转交** | 🟡 设计中（ConversationSwitch 未实装） | ❌ | ❌ | 🟡 sia 的 fork 近似（用户手工在 fork 里对话） | 近似（speaker 切换） | ✅ Command(goto) | ✅ delegate | ✅ hierarchical manager | ❌ | ✅ 父子 context 切换 |
| **动态 role / 招贤** | ❌（必须传 `AgentProfile`） | ❌（YAML 静态） | ❌（四角色固定） | ❌（main + specialist） | 动态 agent 注册 | 动态 graph 构建 | AgentHub 注册 | ✅ role.backstory + manager 分配 | ❌ | 用户自定义 md |
| **并发门客** | `dispatch_to_any` 按 role 挑 idle；`claim_idle_by_role` 原子 | session 并发但 orchestrator 循环 30s 串行 | per-role `maxInstancesByRole` quota | CCBroker 池 | actor 天然并发 | 图内并行（send API） | 可并行 agent | sequential 串行 / hierarchical 并行 | tmux 多 session | Task 工具并行 |
| **失败恢复** | ❌（crash 后 shelf 丢失） | ✅ 独立 `recovery/`（scanner/validator/actions/logger） | ✅ DB cc_status + spawnModeMap 恢复 | ✅ forkManager.respawnFork（读 eventStream 重建 prompt） | 无（框架层） | checkpoint 持久化 | EventStream replay | 无（任务级重试） | tmux 天然持续 | session 不持久 |
| **单一真相源** | EventBus + SQLite WAL（spec 中） | metadata 文件 + SCM 探测 | SQLite (AnyaDB) | EventStore + OpsStore | 内存 actor state | LangGraph checkpointer | EventStream 持久化 | Python 内存 | 文件 | 无 |
| **门客间直通 A2A** | ❌（必须经 orchestrator） | ❌ | ❌ | ❌ | ✅ agent 直发 | ✅ handoff tool | 有限（delegate） | ✅ delegation | ❌ | ❌（父统一调度） |

---

## 3. 伏羲 v1 补课清单（判断优先级）

下面五项是读了上述源码后**伏羲相对主流真正缺的东西**，按 v1 必须 / v2 再说分档。

### 3.1 抄送（InterventionProxy）wire format 【v1 必须】

**判断**：这是伏羲三大独创赌注之一，且**无对标**——AO、anya、sia、autogen、langgraph、OpenHands 都没有。必须 day-1 实装，不然招牌白挂。

**wire 建议**（复用现有 EventKind，加两个变体，不新建 proxy service）：

```rust
// 新事件（加到 fuxi-core/src/event.rs）
EventKind::UserInterventionCC {
    // 用户绕过玄女直达门客时，自动向玄女发的副本
    target: AgentId,           // 门客
    orchestrator: AgentId,     // 玄女（由 orchestrator 查 shelf 得到）
    original_text: String,     // 原话
    channel: String,           // "firehose-ui" | "cli-direct" | "a2a"
    correlation_id: String,    // 关联到 UserInterventionSent 的 id
}
EventKind::OrchestratorAck {
    // 玄女读到 CC 后主动反馈（可选但推荐）
    re_intervention: String,   // CC event id
    action: String,            // "noted" | "adjusted" | "overriding"
    note: Option<String>,
}
```

**实装路径**：
1. CLI `fuxi talk <agent> <text>` 入口 → 发 `UserInterventionSent` 给 target + `UserInterventionCC` 给 orchestrator（同一 tx，保证原子）
2. A2A 入口同理——`InterventionProxy` 是**一层薄 axum middleware**，不是新组件，就在 `fuxi-a2a` 的 handler 里做双发
3. 玄女进程订阅自己的 inbox（`agent == xuannu.id`）过滤 CC 事件，决定是否发 Ack

**关键**：CC 必须是"发送即成功"的副作用，不能等玄女 ack——否则用户直连门客会被玄女 down 时卡住（违反公理 #2 "无否决权"）。

---

### 3.2 ConversationSwitch 状态切换协议 【v1 必须】

**判断**：这是锚点场景步骤 3（玄女交权 PM）和步骤 5（PM 交回玄女）的核心。langgraph 的 `Command(goto, payload)` 和 sia 的 fork 给了两套可抄思路；伏羲要综合。

**协议（三事件 + 一个 current_speaker 状态字段）**：

```rust
// 加到 Fuxi 的状态：
current_speaker: Arc<RwLock<AgentId>>,  // 默认 = 玄女 id

// 加到 EventKind：
ConversationHandoffRequested {
    from: AgentId,            // 玄女
    to: AgentId,              // 门客
    context: String,          // 交接摘要（玄女写）
    return_policy: String,    // "on_complete" | "on_cancel" | "manual"
}
ConversationHandoffAcknowledged {
    by: AgentId,              // 门客 ack，表示它已经准备好接话
}
ConversationReturned {
    from: AgentId,            // 门客
    to: AgentId,              // 玄女
    brief: String,            // 门客交出来的东西（PM 的 brief）
}
```

**wire 层只做两件事**：
1. Firehose UI 订阅 `current_speaker` 变化，切 REPL 输入焦点
2. CLI `fuxi send <text>` 不再带 `--to`，默认发给 `current_speaker`；玄女和被选中的门客共用同一个 stdin 隧道

**和 sia fork 的差异**：sia 是"创建一个新实例做子对话"，伏羲是"切换 active 的那一个"——门客本来就已经在 shelf 上；不 spawn 新进程。

---

### 3.3 门客间 A2A 直通 【v2 延后】

**现状**：`dispatch` 只能 orchestrator 发给门客；门客之间要协作必须走 orchestrator 中转。

**判断**：**v1 不要做**。理由：
- 锚点场景里 PM→Dev 的"传 brief"本来就该由玄女中转（玄女要留档 + 可能调整）；直通反而破坏世界模型一致
- autogen/langgraph 的直通是因为它们没有"上帝视角"这个角色；伏羲有玄女，就该以她为中心辐射
- 真正要门客直通的场景（比如 Dev-A 问 Dev-B 技术问题）v1 可以用"玄女读到后转发"的方式兜底

v2 再加 A2A 直通时的最小改动：`Agent` trait 加 `async fn peer_send(&self, target: AgentId, msg: &str)`，实际 wire 上还是走 orchestrator 的路由表——只是 API 看起来像直通。

---

### 3.4 招贤：动态 role 冷启动 【v1 做最小骨架，v2 完善】

**判断**：crewAI 的 role card（role/goal/backstory/allowed_agents）是工业已验证的 schema；autogen 的 SocietyOfMind 给了"agent 自己招人"的思路。伏羲要在 v1 有**最小骨架**：

```rust
// fuxi-core/src/agent.rs 加
pub struct RoleTemplate {
    pub role: String,            // "pm" / "dev-frontend" / "qa"
    pub description: String,     // 给玄女看的：这人干啥
    pub system_prompt: String,   // agent 侧系统 prompt
    pub cli: String,             // "cc" | "codex" | "gemini"
    pub capabilities: Vec<String>,  // 标签：["electron", "react", "pr-review"]
    pub max_concurrent: usize,
}

// fuxi-orchestrator 加
async fn hire(&self, template: &RoleTemplate, task_hint: &str) -> Result<AgentId>;
```

**v1 骨架**：
- `RoleTemplate` 从 `~/.fuxi/roles/*.toml` 加载（玄女和用户都能改）
- `hire` = spawn_worker 但 profile 自动从 template 展开
- 不做 capability matcher（crewAI 的 manager 自动分配）——v1 由玄女的 prompt 自己挑

**v2 做**：capability 向量 + 任务描述 embedding → cosine 选人；自动从 GitHub/官方 registry 拉 role template（类似 sub-agents 的 md 生态）。

---

### 3.5 失败恢复 【v1 做进程级 + 状态机收尾；v2 做 replay】

**判断**：这是当前 `fuxi-orchestrator` 最大短板。`dispatch` 的 pump task 里已经有"退出必回 Idle"的保证（`fuxi.rs:247`），但：
- 门客进程 OOM / kill -9 时，`AgentDead` 事件从哪发？—— cc 适配器的 stdout EOF 要能触发
- 进程外重启（fuxi 自己被 kill）整个 shelf 丢了
- 跟 sia 的 `respawnFork` 比差了一个数量级

**v1 必做**：
1. **心跳+死亡检测**：`ShelfEntry` 加 `last_seen: Instant`；`Fuxi::tick()` 每 5s 扫一遍，`last_seen > 60s` 发 `AgentDead { cause: "heartbeat timeout" }`，状态摊到 Dead（不 remove，保留 card）
2. **cc stdout EOF → AgentDead**：`fuxi-agent-cc` 的 pump 里 `ChildStdout::read` 返回 0 字节就发事件；当前薄片假设进程一直活
3. **dispatch 失败时门客状态**：现在 launch 失败会回滚 worktree 并发 AgentDead——checked；但 dispatch 到一半门客崩了还没覆盖，pump 的 `is_terminal` 匹配里加 `AgentDead` 分支（现有代码里有，但需要从子 rx 能流出来才有效）

**v1 选做（建议做）**：
4. **重启恢复**：抄 anya 的 cc_status 表——SQLite 持久化 `(agent_id, role, cli, worktree_path, last_task_id, status)`。重启时 scan 表：worktree 还在且有心跳 → 尝试 attach；worktree 孤儿 → destroy。这不是完整 replay，只是"下次启动不从零"。

**v2 做**：完整 event replay（从 EventBus SQLite cursor 重建世界模型 + 尝试把活着的 cc 进程重新 attach 回 shelf）。参考 AO 的 `recovery/` 子目录分 scanner/validator/executor 三层是好范本。

---

## 4. 建议开发优先级

**v0.2 冲刺（锚点场景必须）**：
1. **抄送 InterventionProxy**（3.1）—— 2 天，事件层 + CLI 双发 + 玄女侧订阅
2. **ConversationSwitch**（3.2）—— 3 天，`current_speaker` + 三事件 + Firehose 焦点切
3. **cc 进程死亡检测**（3.5 必做项 1+2）—— 1 天，EOF 和心跳补齐

这三项搞定 = 锚点场景（IM 项目）端到端能跑 = 毕设核心 demo 能录。

**v0.3 扩能**：
4. **RoleTemplate 骨架**（3.4 v1 部分）—— 2 天，`hire()` + toml 加载
5. **重启恢复（cc_status 表）**（3.5 v1 选做）—— 3 天，SQLite schema + scan + attach
6. **codex/gemini 门客** —— 已有 codex 适配器，补 gemini

**v0.4 及以后**：
- 门客间直通 A2A（3.3）
- capability matcher（3.4 v2）
- 完整 event replay（3.5 v2）
- Web 仪表盘（spec 中已列 P3）

---

## 5. 关键判断总结

1. **伏羲真正的护城河是 InterventionProxy + ConversationSwitch**。八个项目里没有一个同时具备这两个——AO/anya/sia 都是 orchestrator 只读或者没有 orchestrator；autogen/langgraph 有 handoff 但没有"抄送给上位观察者"。**这两个必须 v1 吃下**。
2. **"门客间直通"不是短板，是设计选择**。玄女中心辐射 ≠ bug，别被 autogen 带偏。
3. **失败恢复是硬伤**，但 sia 的 `respawnFork` + anya 的 cc_status 给了现成范本，抄就行——不用自己想。
4. **动态 role** 先做 template 壳子就行。crewAI 式的 capability 匹配留到 v2——否则玄女 prompt 里让她自己选反而更灵活，不要过早自动化。
5. **claude-squad 证明了 tmux+worktree 路线能活**，但它卡在"没有上帝视角"——这正是伏羲的立足点。不要去卷 TUI 美观（ratatui 够用即可），卷玄女的智商。
6. **AO 的 lifecycle 30s 轮询是反面教材**，伏羲的 EventBus 推送架构本来就赢了半个身位——坚持公理 #3。

---

## 附录：核心源码锚点

- 伏羲 orchestrator 主体：`/Users/e0_7/fuxi/crates/fuxi-orchestrator/src/fuxi.rs`
- AO lifecycle 轮询：`/Users/e0_7/_refs/agent-orchestrator/packages/core/src/lifecycle-manager.ts:1997`
- AO recovery 子系统：`/Users/e0_7/_refs/agent-orchestrator/packages/core/src/recovery/`
- anya AgentBroker.spawn：`/Users/e0_7/team-anya/apps/server/src/broker/agent-broker.ts:221`
- anya Franky cc_status 恢复：`/Users/e0_7/team-anya/apps/server/src/franky/franky-orchestrator.ts:196-253`
- sia ForkManager（respawn 参照）：`/Users/e0_7/sia-dev/sia/src/core/fork-manager.ts`
- sia Router（sessionKey 路由 + fork 持久化）：`/Users/e0_7/sia-dev/sia/src/core/router.ts`

## 附录：外部来源

- [LangGraph Multi-Agent Supervisor (handoff tools / Command 类型)](https://reference.langchain.com/python/langgraph-supervisor)
- [AutoGen v0.4 Actor-Model Architecture](https://newsletter.victordibia.com/p/a-friendly-introduction-to-the-autogen)
- [OpenHands Runtime + EventStream](https://docs.openhands.dev/openhands/usage/architecture/runtime)
- [CrewAI Hierarchical Process](https://docs.crewai.com/en/learn/hierarchical-process)
- [Claude Squad 架构（tmux + worktree + Bubble Tea）](https://deepwiki.com/smtg-ai/claude-squad)
- [Anthropic Sub-agents 官方 pattern](https://code.claude.com/docs/en/sub-agents)
