# 架构审查 · v1 现状盘点 + Gap + Debt

> [!WARNING]
> `historical`：此审查基于 commit `126e1a1` 和旧 `feat/fuxi-v0.1` 分支，保留用于追溯，不再代表当前架构状态。
> 当前状态以 `docs/status/now.md` 和代码为准。

> **范围**：commit `126e1a1`（2026-04-20）为准，对应 `feat/fuxi-v0.1` 分支。
>
> **方法**：crate API 清点（`docs/audit/cratewise-inventory.md`）+ 事件流矩阵（`docs/audit/event-flow.md`）+ v1 蓝图对照 + 6 份 decisions + 玄女自诊 9 条 + 用户实测 3 条（`docs/v1.1-agenda.md`）。
>
> **立场**：这份文档只诊断不治疗。治疗方案在 `docs/architecture-v1.1-roadmap.md`。

---

## 1 · 架构现状（30 秒看完）

### 分层图

```
                         用户
                          ↓
             ┌──────── fuxi-cli (binary) ────────┐
             │  repl + daemon + IPC + subcommands │
             └─────────────┬──────────────────────┘
                           ↓
            ┌─── fuxi-orchestrator (编排) ───┐
            │  Fuxi + Shelf + Bridge         │
            │                                 │
            │      ┌──── Intervener ─┐        │
            └──────┤                 ├────────┘
                   ↓                 ↑
         ┌──── fuxi-agent-cc ─┐   ┌── fuxi-firehose (观察)
         │  CcAgent + WS       │   │  Hub + EventStream + TUI
         │  (codex 尚缺)       │   └──────────┬────────────
         └─────────────────────┘              │
                   ↓           ↓               │
              ┌─ fuxi-a2a ─┐  ┌── fuxi-events (EventBus) ──┐
              │  wire      │  │  broadcast + SQLite WAL     │
              └────────────┘  └─┬──────────────────────────-┘
                                ↓
                    ┌── fuxi-memory (策府) ──┐
                    │  OracleStore + HetuStore│
                    │  + extractor (v1 stub)  │
                    └─────────────────────────┘

                    ┌── fuxi-scheduler (更漏)──┐
                    │  Keeper + webhook + fs  │
                    └──────────────────────────┘

                    ┌── fuxi-skills (点将台)──┐
                    │  loader + staging       │
                    └──────────────────────────┘

         fuxi-core（traits + Event + Task 词汇层）全员引用
```

依赖方向一致、无循环。Repo 分层清晰。

### 事件流（39 个 EventKind 变体）

- `kind_tag / summarize / color_for` **三处对齐满分**（agent B 确认）
- **6 个 publisher-orphan**（定义无人发）：AgentSpawning, AgentShuttingDown, TaskDelivered, TaskCancelled, MessageSent, MessageReceived
- **9 个 subscriber-orphan**（发了无人订阅）：TriggerDispatched/Skipped/Failed, AgentInterrupted, TaskInterventionApplied, ConversationTransferred/Returned, PlatformStopping 等
- **meta.agent 缺失**：TaskCreated/TaskDispatched/TaskResumed 的 agent 字段未设——但 commit `126e1a1` 已修 TaskCreated/Dispatched 两者（agent B 扫的快照可能未覆盖新 commit，下次复查）

### v1 蓝图收敛判据对照（`architecture-v1.md §6`）

| 判据 | 达成 |
|---|---|
| 玄女真跑，用户对话，Bash 工具调 `fuxi ...` | ✅ |
| TUI Tab 切到鲁班 → 写代码 + 右栏实时 | ⚠️ 鲁班能起，但**派活后还在空闲区**bug 今天才修；实时右栏通 |
| 长期记忆（关 fuxi 再开玄女记得偏好） | ⚠️ cc `--resume` 通了（B1 session），但 oracle 里 fact 抽取 **extractor v1 stub 未动**，只能人工 `fuxi memory record` |
| 定时任务 `fuxi cron add ...` | ✅ scheduler 在，但 TriggerDispatched/Skipped/Failed 无订阅者（发了没人处理） |
| 招贤：铸牒司 → 用户审 → 新 role 可用 | ⚠️ 链路 wire 了，但**实际没跑过 E2E**（skill_staged 等事件从没被触发过） |
| 死亡恢复（kill -9 → 玄女知情） | ✅ AgentDead 事件 + bridge intervene 通了 |
| 场景 §1 的 33 事件全到 SQLite | ❓ 未做 E2E 断言；部分事件是 orphan |

**概述**：骨架对了，血肉差半。

---

## 2 · Gap 矩阵（需求 vs 实现）

### 2.1 用户实测反馈 → 架构根因

| ID | 用户反馈 | 架构层根因 |
|---|---|---|
| N1 | 玄女 busy 时我发消息消失 | **fuxi-agent-cc::agent.rs::send_message** 只发 WS 消息不查 active_tx；intervene 通路在 busy 状态有黑洞（非 idle 情形 degrade 逻辑不覆盖）。消息进 cc stdin 但若 cc 当前 tool-loop 里不 poll stdin，就被吞 |
| N2 | 不能起 codex 门客 | **fuxi-agent-codex 是 P2.5 延期项**（orchestrator 注释明示）。spawn_worker 只路由 `WorkerKind::Cc`；codex 适配器 crate 存在但没接入 Fuxi::spawn_worker |
| N3 | spawn 后立即 TaskDone 抄送 | 可能是 commit `126e1a1` 补发 TaskCreated/Dispatched 时序 与 cc 的 system/init 交错；存疑需复现 |
| N4 | 门客堆 3 个不回收 | **无 GC/TTL 架构**。Shelf 只做注册，无 idle_since 计时 + 无 kill_idle_older_than policy。spawn 也不去重（同 role 多次 spawn 不合并） |
| N5 | resume 语义歧义 | CLI 命名债：`fuxi resume` = unblock Blocked task；用户直觉是"续对话"。cc 的 `--resume` 和 task 的 `Resume` 是两回事 |
| N6 | REPL 无 /help | TUI 无斜杠命令分派层（C5 命令面板 research 列了但没落地） |
| N7 | skill=role 与 claude skill 命名对冲 | **概念撞名**：伏羲把 `skill` 当作"角色包"（soul + tools + instructions），而 claude code / cursor 的 "skill" 是另一回事（模型行为注入）。目录名 `skills/` + 文件 `SKILL.md` 都撞 |
| N8 | 参数风格混乱 | 玄女工具子命令没有统一 charter：`fuxi dispatch --to` vs `fuxi intervene --id` vs `fuxi spawn --role` |
| N9 | `fuxi kill --id` 没实装 | `subcommands::run_kill` 只做 shutdown-all；无单杀路径 |
| N10 | 没有 `fuxi events` | 玄女 / 开发者 debug 事件流只能开 TUI 或手动 sqlite |
| N11 | idle intervene → dispatch 退化 | 产品语义决策（Decision 04）被部分用户认为是 "闲聊也占 task 历史"。做法没错，但 UI 上"intervention" 这个 task title 混在真 task 里不醒目 |
| N12 | TUI 太繁复 | 左右两栏 + 事件面板 + 对话 + 输入 + 状态共 5 区 30 字段，信息密度过载。cc / opencode 的"单栏 transcript"更符合当代 AI CLI 习惯 |
| N13 | 启动无 ASCII art | 纯 cosmetic，提升"平台感" |
| U1 | Agent 前缀视觉疲劳 | `▍ ` 全行挂导致"条形码"感 + mauve/teal 高饱和度 + 无消息间距 |

### 2.2 架构 gap（用户未看到但存在）

| G# | 问题 | 来源 |
|---|---|---|
| G1 | **6 个 publisher-orphan** | event-flow agent：AgentSpawning/Shutting/TaskDelivered/Cancelled/MessageSent/Received 定义了但没人发。要么删（YAGNI）要么补发布点 |
| G2 | **9 个 subscriber-orphan** | TriggerDispatched/Skipped/Failed 等事件发了没人订阅。Scheduler 岛的事件流断点：`TriggerFired` 有桥接到 xuannv，但"玄女派活后"的 `TriggerDispatched` 状态链断 |
| G3 | **Extractor v1 stub** | fuxi-memory extractor 没真正抽 fact。玄女长期记忆依赖她自己调 `fuxi memory record`，但她会忘——长期记忆实际上不 "自动"  |
| G4 | **fuxi-a2a::AgentCard ≠ fuxi_core::AgentCard** | 同名不同结构，手工转换。未来 A2A 真跑起来会踩 |
| G5 | **fuxi-cli 多处 `unwrap()`** | IPC / daemon 路径报错会直接 panic TUI。错误处理不鲁棒 |
| G6 | **intervention 不是"真介入"** | Decision 04 退化方案让 intervene idle → dispatch 新 task。这在事件流里混一堆 task_id="intervention" 的节点，违反"task 是明确工作单元"的原设计 |
| G7 | **让贤（ConversationHandoff）dead wire** | Decision 05 明确延 v1.1；event 有订阅但发起源无实体在用 |
| G8 | **玄女 skill 教她轮询** | `dispatch-protocol.md` 里教她派完活后 `fuxi status` poll 查门客状态——违反公理 #3「真实时不轮询」。她该订阅事件（但 headless agent 订阅机制没设计） |

---

## 3 · Debt 分级

### 🔴 必须改（正确性 / 安全 / 阻塞用户）

| # | Debt | 对应 |
|---|---|---|
| D1 | 消息黑洞（busy 时消息丢） | N1 |
| D2 | codex 不能起 | N2，G2（scheduler 孤儿事件相关） |
| D3 | 玄女 skill 教轮询违反公理 #3 | G8 |
| D4 | 无 GC/TTL 门客堆积 | N4 |
| D5 | Extractor 不抽 fact | G3（长期记忆谎称） |

### 🟡 应该改（一致性 / 可维护性）

| # | Debt | 对应 |
|---|---|---|
| D6 | CLI 参数 charter 缺失 | N8 |
| D7 | resume 命名歧义 | N5 |
| D8 | skill=role 与 claude skill 撞名 | N7 |
| D9 | A2A AgentCard 双定义 | G4 |
| D10 | fuxi-cli unwrap() 不鲁棒 | G5 |
| D11 | 孤儿事件清理（6+9 个） | G1, G2 |
| D12 | `fuxi kill --id` / `fuxi events` 补洞 | N9, N10 |
| D13 | intervention 退化 task title | G6（UI 侧小修即可） |
| D14 | ~~让贤 wire 是否拆除还是激活~~ ✅ 已拆（Decision 08 · 2026-04-21） | G7（原 Decision 05 说 v1.1 决定） |

### 🟢 Nice to have（体验）

| # | Debt | 对应 |
|---|---|---|
| D15 | TUI 单栏化 + 事件嵌入对话 | N12, U1 |
| D16 | REPL `/help` + slash/@ 命令面板 | N6 |
| D17 | 启动 ASCII art | N13 |
| D18 | Resume 真回放 dialogue history | B4 |

---

## 4 · 外部参考借鉴点（按 debt 对应）

| Debt | 借鉴对象 | 借什么 |
|---|---|---|
| D1 消息黑洞 | cc's stdin queue | cc 自己在 busy 时 stdin 不丢消息，而是排队下一 turn；看 `ink` 的 message queue |
| D2 codex 支持 | 已有 `fuxi-agent-codex` crate 骨架 | 补齐 `WorkerKind::Codex` 路由；codex 的 auth（ChatGPT account vs API key）要处理 |
| D3 玄女订阅机制 | ComposioHQ 的 event trigger | agent headless 订阅本质是 webhook 回调 → 用 bus subscribe 给"唤醒模板"动态生成 prompt 注入 |
| D4 GC/TTL | anya `ForkManager` | idle_since + TTL 清理；spawn 去重按 role+tag |
| D5 Extractor | anya `chat-memory-extractor.ts` | 对话结束后 async 跑 cc headless extractor 抽 fact（蓝图 §M1.1 原本要做） |
| D7 resume 命名 | git `git stash pop` 语义 | 动宾明确：`fuxi task unblock` |
| D8 skill 命名 | 已存在的方案可选：`roles/` / `personas/` / `characters/` | 避开 claude skill |
| D15 单栏 + 事件嵌入 | claude code / opencode / codex | 看 `ink` 的 React DSL 对应 ratatui 怎么实现 |
| D16 slash 命令 | Warp / Raycast / cc `/compact` 等 | tui-textarea 上层浮层 |

---

## 5 · 审查总结

**好消息**：
- 骨架对了（无循环依赖、事件系统对齐、公理清晰、命名体系立住）
- 6 个已修 commit 覆盖用户第一轮测试的所有痛点

**坏消息**：
- **骨架对 ≠ 血肉通**。C3/C4 合完但 E2E 场景没跑——extractor/招贤链路是 wire 不是实装
- 新用户痛点（N1/N4/G8）都指向**架构缺口**不是 bug：消息队列、GC、订阅机制在 v1 蓝图里未明确提及
- v1 设计时对"多门客同时活着"的 lifecycle 考虑不足（spawn/TTL/去重/kill 单点）

**核心判断**：**不是危房，但基础桩差几根**。D1-D5 必须先补，再谈 TUI 大改。路线图在下一份文档。
