# v1.1 重构路线图

> **进度 · 2026-04-21**：M2 ✅（5/5）+ P2 召回 L2 ✅（commit `d5a8e02` e2e 闭环）+
> M3 命名规整 ✅（7/7，commit 至 `8be9ed1`）+ extractor 改 prompt 驱动（`f2142c5`）。
> **406 tests 绿**。下一步：M4 体验升级 + 用户验收 v1.1。
> 接手细节看 `docs/handoff/v1-session4.md`。M5 留 v1.2。
>
> 基于 `docs/architecture-audit-v1.md` 的 D1-D18 debt 列表，分 4 个 milestone 推进到 v1.1 ship。
>
> **原则**：
> 1. 先补地基桩（🔴），再清债（🟡），再改体验（🟢）
> 2. 不破坏公理 #1-#6
> 3. 破坏性 API 改动尽量打包到同一 milestone 一次吃完
> 4. 每 milestone 独立绿门禁 + 独立用户验收
> 5. TDD 硬要求（先写失败测再写实装）
> 6. 分治可用则并行 agent team（参考 Decision 01）

---

## Milestone 总览

| # | 名字 | debt | 时间 | 前置 | 产出 |
|---|---|---|---|---|---|
| **M2** ✅ | 地基修复（已完成） | D1-D5 | 1.5 session（实际 1.5） | 无 | 消息不丢 / codex 能起 / 玄女订阅事件 / 门客 GC / 策府自动抽 fact |
| **P2** ✅ | 召回（task_id + role 双入口） | — | 1 session | M2 完 | trait 通用化 + worktree 复用，cross-restart e2e 闭环（Decision 07） |
| **M3** ✅ | 命名规整 + CLI charter | D6-D12 | 1 session（实际 1） | 可与 M2 并行局部 | rename 合一次、孤儿事件清零、kill/events 补洞、CLI charter 落档 |
| **M4** | 体验升级 | D13-D14, D16, U1 | 1 session | M2 完（命令面板 要正确事件订阅） | slash 命令、/help、让贤拆除（D14 · Decision 08）、intervention 视觉差异、U1 视觉方案 A |
| **M5** | v1.2 大改 | D15, D17, D18 | 2-3 session | M2-M4 稳 | 单栏 TUI（cc 风格）+ ASCII art + Resume 真回放 |

**v1.1 = M2 + M3 + M4**（ship 条件：三者全绿 + 用户验收通过）。M5 = v1.2 独立里程碑。

---

## M2 · 地基修复（🔴）

### M2.1 · D1 · 消息黑洞修（busy 时消息丢）

**现象**：玄女/门客 busy（cc 正在 tool loop）时，用户发消息通过 `Fuxi::intervene` → `send_message` → WS 送 cc stdin，但 cc 处在当前 turn 结束前不 poll，消息被吞。

**设计**：

1. **CcAgent 加 pending_queue**：`Arc<Mutex<VecDeque<String>>>`，intervene 时**先入队**
2. **active_tx pump 结束一 turn**（`TaskStateChanged::Done`）后 drain pending_queue，按序 send_message
3. 若 queue 满（预设 32 条），drop 老的 + 发 `AgentInterrupted { reason: "queue_overflow" }` 事件让玄女知情
4. TUI 侧发送消息后立刻在对话区显示（不等 cc 回复）—— push 到 dialogue bucket + 如果 agent busy 再 push 一条 dim `System("排队中...")`

**TDD**：
- `send_message_while_busy_queues_not_drops`
- `pending_drains_after_turn_done_in_fifo_order`
- `queue_overflow_emits_interrupted_event_and_drops_oldest`

**文件影响**：`fuxi-agent-cc/src/agent.rs` + `fuxi-orchestrator/src/fuxi.rs` + `fuxi-cli/src/repl.rs`（UI pending 提示）

**退出**：用户手测：busy 时连发 3 条 → 玄女 turn 结束后全部处理 + 对话区每条有 "排队中" 提示然后 promoted 到 "已发送"。

---

### M2.2 · D2 · codex 支持

**现象**：`不能起 codex`。根因 Fuxi::spawn_worker 只路由 `WorkerKind::Cc`；`fuxi-agent-codex` crate 有骨架但没接入。

**设计**：

1. Audit `fuxi-agent-codex` 现状（crate 存在但 trait impl 完整度未知）
2. 实现 `CodexAgent: fuxi_core::Agent`（参考 `CcAgent`）+ `spawn_codex` + `launch_with_id`
3. Fuxi::spawn_worker 增 `WorkerKind::Codex(CodexLaunchConfig)` 分支
4. SKILL.md 里 `cli` 字段支持 `codex`（已有 role="luban" cli="claude-code"，加 cli="codex"）
5. env 兼容 `FUXI_CODEX_MODEL`（CLAUDE.md 已提过 API key 用户需覆盖，ChatGPT account 需空串）

**TDD**：
- `codex_adapter_implements_agent_trait`（编译级）
- Gated E2E `FUXI_RUN_CODEX_E2E=1 cargo test`：codex 真跑一轮 helloworld

**文件影响**：`fuxi-agent-codex/*` + `fuxi-orchestrator/src/fuxi.rs` + `fuxi-cli/src/subcommands.rs`（spawn --cli codex 参数）

**退出**：`fuxi` 里说"起一个 codex 鲁班" → 玄女能 spawn + codex 真对话。

---

### M2.3 · D3 · 玄女订阅机制（反轮询）

**现象**：玄女 skill 教她 `fuxi status` poll 查门客状态 → 违反公理 #3「真实时不轮询」。

**设计方向**（两选一，需拍板）：

**方案 X · 修 skill**：改 `skills/xuannv/instructions/dispatch-protocol.md`，教她"派完活 → 等待，bridge 会自动把门客状态变化 intervene 给你（TriggerFired/TaskDone/AgentDead 三类）"。配合 bridge 已有的 intervene 通路。成本小。

**方案 Y · 加"玄女 inbox" 抽象**：专门在 fuxi-orchestrator 加 `XuannvInbox` 概念——bridge 不是 intervene 注入文本，而是入 inbox 队列。玄女 skill 里明确教她用 `fuxi inbox read` 读队列。成本中等，但把"知情权"变成一阶概念。

**推荐 X**（小改，风险低）；Y 可 v1.2 再考虑。

**TDD**：改 skill.md + 加集成测试"派活 → 完活 → 玄女通过 bridge intervene 收到通知" 端到端（已有部分，扩覆盖）。

**退出**：玄女不再调 `fuxi status` poll；日志里无 `status` 重复调用；用户等 3 分钟不烦她，她也不乱动。

---

### M2.4 · D4 · 门客 GC/TTL + spawn 去重

**现象**：`fuxi spawn` 重复起同 role 门客堆一堆，idle 也不回收。

**设计**：

1. **spawn 去重**：`Fuxi::spawn_worker` 加 role+tags 指纹；已有 idle 的同指纹门客则复用（返回旧 id），**不起新**。日志 info 级告知"复用 idle 门客 X"
2. **idle TTL**：Shelf 加 `idle_since: Option<Instant>`；Keeper 新增 idle GC tick（每 30s 扫）：
   - idle > 10 分钟 → 发 `AgentShuttingDown { reason: "idle_ttl" }` → Fuxi::shutdown_agent
   - TTL 可配置 `FUXI_IDLE_TTL_SECS=600`
3. **AgentSpawning/ShuttingDown 事件激活**：这两个变体是 publisher-orphan（agent B 报告）。spawn 开头发 `AgentSpawning`；GC 触发时发 `AgentShuttingDown`。解决 G1 一部分
4. 玄女 skill 里说明"不要频繁 spawn，去看 `fuxi list --role X` 有没有现成的"

**TDD**：
- `spawn_worker_reuses_idle_same_role`
- `idle_ttl_triggers_agent_shutting_down_after_timeout`
- `spawning_and_shutting_down_events_wire`

**退出**：连 `fuxi spawn luban` 3 次 → `fuxi list` 只有 1 个 luban。10 分钟不干活 → 自动 shutdown。

---

### M2.5 · D5 · Extractor 实装（策府自动抽 fact）

**现象**：fuxi-memory::extractor 是 stub，长期记忆靠玄女自己手调 `fuxi memory record`。

**设计**（参考 anya `chat-memory-extractor.ts`）：

1. Extractor 订阅 `TaskStateChanged { to: Done }`
2. 拿 task 的 transcript（通过 EventStore 查该 task_id 的 AgentResponded + UserPrompted）
3. **spawn 一个 A2A extractor 门客**（role="extractor"，cc headless）跑 prompt：「从这段对话抽出 subject/predicate/object 三元组 JSON list」
4. 回传 JSON → `OracleStore::insert` 逐条入库
5. 熔断：每条 task 最多抽 10 facts，避免风暴

**TDD**：
- `extractor_spawns_role_extractor_on_task_done`
- `extracted_facts_land_in_oracle_store`
- Gated E2E：跟玄女说"我爱喝冰美式" → 任务结束 → 下次 `fuxi memory query --subject user` 能读回

**文件影响**：`fuxi-memory/src/extractor.rs` 补全 + 新 `skills/extractor/SKILL.md`

**退出**：跟玄女聊 3 轮偏好 → 关 fuxi 再开 → 她自己说话时会自然引用之前的偏好。

---

## M3 · 命名规整 + CLI charter（🟡）

### M3.1 · D7 · resume 重命名

- `fuxi resume` → `fuxi task unblock`（动宾明确）
- 保留 `fuxi resume` 一段 alias（deprecated warning）
- 更新 skill tool-map

### M3.2 · D8 · skill=role 命名对冲

**破坏性改名**（打包一次吃完）：
- `skills/<role>/SKILL.md` → `roles/<role>/ROLE.md`
- `fuxi skill` 子命令保留（已有）但指向 `roles/`
- `fuxi-skills` crate rename 到 `fuxi-roles`（或保 crate 名，只改目录/文件约定）
- 所有 `skill_loader` 引用随之更

**风险**：crate rename 影响 workspace Cargo.toml + 所有 use 路径。要一次 commit 全改。

### M3.3 · D6 · CLI charter

- 统一：`fuxi <resource> <verb>` 格式，`--id` 标识资源、`--to` 标识消息目的、`--role` 标识 spawn 类型
- 检查所有 subcommands.rs 中的 clap Args 符合
- 文档 `docs/cli-charter.md` 记规约

### M3.4 · D9 · A2A AgentCard 合并

- 把 `fuxi-a2a::AgentCard` 改成 `fuxi-a2a::wire::AgentCard`（显式 wire 命名）
- 加 `impl From<fuxi_core::AgentCard> for wire::AgentCard` + reverse
- 所有使用点改用 From 而不是手工字段抄

### M3.5 · D10 · fuxi-cli unwrap 清理

- grep `\.unwrap()\|panic!` 找所有 binary 里的 panic 点
- 替换 `.context("...")` + 返回 Result（main.rs 边界 Err 打印后退出，不 panic）

### M3.6 · D11 · 孤儿事件清理

- 6 个 publisher-orphan：要么补发布点（`AgentSpawning` / `AgentShuttingDown` 在 M2.4 补了），其余（`TaskDelivered` / `TaskCancelled` / `MessageSent` / `MessageReceived`）要么删（YAGNI）要么补实装
- 9 个 subscriber-orphan：`TriggerDispatched` 应被 bridge/TUI 订阅；`AgentInterrupted` 应 TUI 显示；`ConversationTransferred/Returned` 已随 D14 拆除（Decision 08）

### M3.7 · D12 · kill --id + events 子命令

- `fuxi kill --id <agent>` 实装
- `fuxi events [--tail N | --follow]` 查 SQLite（绕开 TUI）
- 这两个加入 `fuxi --help` + skill tool-map.md

---

## M4 · 体验升级（🟡🟢）

### M4.1 · U1 · 视觉方案 A（疲劳解药）

- 每消息只首行 `▍ ` + 时间戳 `HH:MM`，续行空格缩进对齐
- 饱和度降档：user sapphire 代替 teal，agent lavender 代替 mauve
- 消息间空一行
- theme 加 `user_first_line()` / `agent_first_line()` alias

**TDD**：`render_dialogue_v2_cjk_width` / `render_dialogue_v2_indent_alignment`

### M4.2 · D13 · intervention task 视觉差异

- Decision 04 退化方案保留，但 TUI 侧 task title == "intervention" 的 node 用不同 icon（`💬` 代替 `📁`）+ dim 色，和正式 task 区分

### M4.3 · D14 · 让贤决策 ✅ 拆（Decision 08 · 2026-04-21）

**已拍**：拆。理由：intervene + 抄送 + `@agent` 切 active 已覆盖所有场景；v1.1 无能主动让贤的门客（铸牒司/pm 都空位）；激活 = dead code 换形式不换本质。v1.2 真需要时重新设计 API。

详见 `docs/decisions/08-conversation-switch-removed.md`。

### M4.4 · D16 · slash/@ 命令面板 + /help

- 输入框首字符 `/` 弹浮层列 slash commands（help/clear/mouse/theme/kill/status 等）
- `@` 弹浮层列门客（@xuannv/@luban/...），选中切 active target
- **对应 N6 REPL 无 /help** + 部分替代左栏 agent 列表（M5 单栏化铺路）

---

## M5 · v1.2 大改（🟢）

### M5.1 · D15 · 单栏 TUI + 事件嵌入对话

**目标**：cc 风格 transcript，事件 + 消息 + 工具调用混在同一滚动区。

**影响面**：
- 左栏砍掉（任务树收进 `@` 命令面板 + 状态栏简短显示）
- 右栏砍掉（任务元信息收进 `/task` slash 命令展开卡）
- 中栏对话区 = 唯一主体，滚动历史 + 事件嵌入
- ClickRegistry 重设计适配单栏
- 对应 architecture-v1.md §M1.4 要 override 重写

**成本**：2-3 session。最大破坏性改动。

### M5.2 · D17 · 启动 ASCII art

- `fuxi` 启动 banner：`figlet` 风格"伏羲" / 三阳 / 卦象线条（用 Unicode block 字符）
- 1 秒后淡出或 Enter 跳过

### M5.3 · D18 · Resume 真回放 dialogue

- 持久化 `dialogues: HashMap<ActiveTarget, VecDeque<DialogueLine>>` 到 SQLite
- 启动时按 xuannv_session_id 回放
- 可能需要新 `dialogue_lines` 表

---

## 并行拆法（给未来 agent team）

**M2 各子项独立**，可并行：
- 子 agent 1: M2.1 消息队列
- 子 agent 2: M2.2 codex
- 子 agent 3: M2.4 GC/TTL
- 主线: M2.3 skill（skill.md 文档性工作）+ M2.5 extractor

**M3 整体破坏性**，单线做（cherry-pick 风险高时）。

**M4** 可拆：U1 + D13（UI）/ D14（让贤）/ D16（命令面板）三路并行。

**M5** 必须单线：单栏重写牵涉全 repl.rs。

---

## 风险 + 延后候选

### 风险

| 风险 | 应对 |
|---|---|
| M2.5 extractor 要 spawn cc headless，API 开销 | 加 `FUXI_EXTRACTOR_ENABLED` env，默认 on，用户可关 |
| M3.2 skill→roles rename 破坏 `~/.fuxi/skills/` 用户已有数据 | 加 migration：启动时自动 mv skills/ → roles/ 并备份 |
| M5.1 单栏改动影响太大，卡住 v1.2 | 保留 M1.4 三栏作 fallback，FUXI_TUI_LAYOUT=classic\|transcript 切换 |
| 玄女 skill 改订阅机制后她 "不知道 怎么等"  | skill 改动前写明 "等待时做什么" 指引：空闲时刷新记忆、整理 oracle、回顾 |

### 延后候选（v2+）

- ~~G7 让贤完全激活~~ → M4.3 拍板"拆"，已移除（Decision 08）。v1.2 如有铸牒司场景再重新设计
- G4 A2A 双 AgentCard 合并 → 若 A2A 不成为主通信路则延
- M5.3 Resume 回放 → 需要新表 + 性能考量，可 v2

---

## ship 判据（v1.1）

M2 + M3 + M4 全绿 + 下列用户验收全过：

- [ ] busy 时连发 3 条消息全收到
- [ ] `fuxi spawn codex luban` 能起 + 派活
- [ ] 玄女不再 poll `fuxi status`（日志验证）
- [ ] 连开 fuxi 5 次 spawn 3 次 → list 只 1 个（去重）；10 分钟不用 → 自动回收
- [ ] 跟玄女聊喜好 → 关闭 fuxi → 开新会话她能自然引用（extractor 通）
- [ ] `fuxi task unblock` 替代 `fuxi resume`
- [ ] `fuxi kill --id` 单杀
- [ ] 对话视觉方案 A 应用后连看 30 秒不累
- [ ] `/help` 有
- [ ] `@agent` 切 active 有

---

## 附录：如果用户想重排优先级

按**价值 / 成本**二维：

| 高价值 低成本 | 先做 |
|---|---|
| M4.1 U1 视觉 | 0.5 天，立竿见影 |
| M2.3 skill 订阅 | 0.5 天，公理归位 |
| M2.1 消息队列 | 1 天，用户痛点 |
| M3.7 kill/events | 0.5 天，开发效率 |

| 高价值 高成本 | 攻坚 |
|---|---|
| M2.5 Extractor | 1-2 天，长期记忆真落地 |
| M5.1 单栏 TUI | 2-3 天 |

| 低价值 低成本 | 顺手 |
|---|---|
| M3.6 清孤儿事件 | 配合 M3.2 一把 |
| M5.2 ASCII art | 0.3 天 |

| 低价值 高成本 | 延后 |
|---|---|
| G4 AgentCard 合并 |  |
| M5.3 Resume 回放 |  |
