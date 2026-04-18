# 伏羲架构决策记录（ADR）

> 按时间顺序记录关键决策与其理由。每条 ADR 一旦 "Accepted"，除非有明确推翻理由，否则视为约束。

---

## ADR-001 · 2026-04-19 · 语言：Rust（stable · edition 2024）

**Status:** Accepted

**Context:** 平台需要长跑、低内存占用、并发协调多 agent 进程；毕业设计需要有技术深度做论文 contribution；与 team-anya（TS/Fastify）共存但**不共享代码**。

**Decision:** Rust stable, edition 2024，最低 1.85。tokio + axum + sqlx + ratatui 基础技术栈。

**Consequences:**
- 开发速度比 TypeScript 慢 2-3×，但类型/并发正确性的"地基"红利在长期占优。
- A2A 的 Rust SDK 不存在，必须自实现（同时这是毕设 contribution 的一部分）。
- 与 ComposioHQ（TS）语言隔离天然阻止"无脑抄代码"的诱惑，只借设计智慧。

---

## ADR-002 · 2026-04-19 · 协议：A2A v1.0（Google Agent2Agent）

**Status:** Accepted

**Context:** 门客与玄女、门客与门客之间需要结构化协议：任务/状态机/流式/产物/断线重连。CLI 纯文本 stdio 不足以表达这些，会逼你重新发明一次。

**Decision:** 采用 Google A2A v1.0（2026-03 稳定）作为 agent 间唯一协议。**工具层仍走 CLI/shell，不走 MCP。**

**Consequences:**
- `fuxi-a2a` 是自实现的 Rust SDK（P1 subset：types + axum server + reqwest client + SSE）。
- 每家 CLI agent（cc/codex/gemini-cli）需要一个包在 A2A 壳下的适配器——这是伏羲的核心工作之一。
- MCP 只在极端情况下考虑——用户的 strong preference 是 CLI 即工具。

---

## ADR-003 · 2026-04-19 · cc 适配器：headless stream-json（不 PTY，不 MCP）

**Status:** Accepted

**Context:** `claude` CLI 有三种可被编排的模式：
1. 交互 TTY（pty-wrap）——脆弱，格式随版本飘。
2. `claude --print --output-format stream-json`——结构化 JSON 流 in/out。
3. `claude --remote`（Remote Control，Q1 2026 引入的 REST+WS）。

**Decision:** P1 用 `stream-json` stdio。

**Reasoning:**
- stream-json 是"给程序用"的官方接口，合约明确。
- `--remote` 更优雅但文档少、可观测性尚待验证，P1 不承担探索成本。
- `--bare` 模式跳过 hooks / plugins / CLAUDE.md 自动加载，适合无污染的门客启动。

**Consequences:**
- 需要维护 cc stream-json 事件映射表（见 `/Users/e0_7/.claude/projects/-Users-e0-7-fuxi/memory/reference_cc_stream_json.md`）。
- 升级 cc 版本时要回归测试事件解析。
- **P2 可选升级**到 `--remote` 模式以拿到原生 WS 流。

---

## ADR-004 · 2026-04-19 · EventBus：tokio broadcast + SQLite WAL append-only

**Status:** Accepted

**Context:** 伏羲的独创赌注 "真实时 Firehose" 要求所有事件能实时推 + 全量留存可重放。ComposioHQ 用 5 秒轮询（被我们明确抛弃）。

**Decision:**
- 内核：`tokio::sync::broadcast` 做进程内 N-订阅 fan-out。
- 持久化：SQLite（WAL 模式）append-only 事件表，rowid 作为序号。
- 发布路径非阻塞：writer 用有界 mpsc + 背景任务消费；堆积时发 `event_store_lagged` 哨兵，**但原事件不丢**。
- Replay：按 id/时间游标拉历史，可选 `live_tail` 拼接实时流。

**Consequences:**
- 单机规模 sweet spot；跨机需 P3 引入 NATS 或 HTTP 桥接。
- SQLite 单写者约束下，高并发写入需要 writer 背靠 mpsc 缓冲。
- Replay 可能产生极短窗口的重复事件（live_tail 模式下），消费者需去重——**当前取舍，不试图在总线层解决**。

---

## ADR-005 · 2026-04-19 · ComposioHQ 关系：学设计，不抄代码

**Status:** Accepted

**Context:** ComposioHQ/agent-orchestrator（MIT, TS）在 7 插件槽抽象、状态机、事件词汇上与伏羲高度重合（~60-80%）。

**Decision:** **Path C — study patterns, greenfield in Rust**。借鉴其架构决策，不 fork、不 depend。

**Why not Path A（depend-and-extend）:** ADR-001 的 Rust 选择使此路径失效（TS↔Rust 跨语言包装成本大于收益）。

**Why not Path B（fork-and-modify）:** 保留上游修改能力不是我们要优化的目标；伏羲的定位是独立系统。

**Consequences:**
- 伏羲的独创赌注（主对话接管 / 抄送介入 / 真实时 Firehose）必须自己实装，ComposioHQ 帮不上。
- 审 ComposioHQ 的代码是持续的灵感来源，但每次引用需在 PR/commit 上明注 "Inspired by composio:path/to/file:LN-LN"。

---

## ADR-006 · 2026-04-19 · 权限模式：项目级 bypassPermissions

**Status:** Accepted

**Context:** 用户（同时是 Claude Code 协作者 + 项目所有者）明确要求本项目下无 yes/no 提示，保证 Claude 自驱开发不被打断。

**Decision:** `/Users/e0_7/fuxi/.claude/settings.local.json` 设 `"defaultMode": "bypassPermissions"`。所有 Claude 子进程启动带 `--dangerously-skip-permissions --permission-mode bypassPermissions`。

**Consequences:**
- 本项目 Claude 拥有完全权限；不适合运行在共享/生产环境。
- 如未来需做多人协作，需要切回细粒度授权。**这条记在此防忘。**
