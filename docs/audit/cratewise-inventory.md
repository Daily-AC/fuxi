# Crate 清点 · v1.1 架构审查

> [!WARNING]
> `historical`：此审计基于 2026-04-20 附近的旧代码快照，保留用于追溯，不再代表当前 crate 清单。
> 当前架构状态以 `docs/status/now.md` 和代码为准。

> 2026-04-20 · 10 个 crate 的 pub API、依赖、模块组织速查。Agent A (Explore) 扫 pub 签名 + Cargo.toml + 模块组织，不读全文。

---

## fuxi-core

- **职责**：traits + 基础类型词汇层（无执行逻辑）
- **pub 顶层 API**：`trait Agent` / `trait Runtime` / `trait Workspace` / `trait TriggerLookup` / `struct Event` + `EventKind` / `enum TaskState` + `struct Task` / `struct AgentCard` + `AgentStatus` / `enum CoreError`
- **核心 trait**：Agent（dispatch）、Runtime（lifecycle）、Workspace（isolation）
- **依赖 fuxi-***：无
- **内部模块**：agent.rs / event.rs / id.rs / runtime.rs / task.rs / trigger_lookup.rs / workspace.rs
- **可疑点**：无

## fuxi-events

- **职责**：EventBus（tokio broadcast）+ SQLite WAL 持久化 + replay
- **pub 顶层 API**：`struct EventBus` + `EventBusConfig` / `struct EventStore` / `subscribe()` / `replay()` / `enum ReplayCursor`
- **依赖**：fuxi-core
- **内部模块**：bus.rs（broadcast）/ store.rs（SQLite）
- **可疑点**：无

## fuxi-a2a

- **职责**：A2A v1.0 wire types + HTTP JSON-RPC server/client
- **pub 顶层 API**：`trait A2AService` / `fn router<S>()` / `struct A2AClient` / wire types (`Task`, `Message`, `Part`, `Role`, `AgentCard`) / `SendTaskRequest/Response` / `mod sse`
- **依赖**：fuxi-core
- **内部模块**：client.rs / server.rs / jsonrpc.rs / sse.rs / types.rs / error.rs
- **可疑点**：**wire AgentCard 与 `fuxi_core::AgentCard` 同名不同结构**，需手工转换。下游维护负担

## fuxi-agent-cc

- **职责**：Claude Code headless 适配器（stdin/stdout + WS 反连）
- **pub 顶层 API**：`struct CcAgent` (Agent impl) / `fn spawn_claude()` / `struct CcLaunchConfig` / `parse_line()` + `translate()` / `enum CcEvent` / `struct WsChannel` / `resolve_default_model()`
- **依赖**：fuxi-core, fuxi-a2a
- **内部模块**：spawn.rs / parser.rs / config.rs / agent.rs / ws_bridge.rs
- **测试风格**：feature gate `real_cc` for E2E（避免 API 花费）
- **可疑点**：ws_bridge 有 `#[allow(dead_code)]`（薄片 H 未完）

## fuxi-firehose

- **职责**：实时观察器（WS/SSE/TUI 多路订阅）
- **pub 顶层 API**：`struct Hub` + `fn router()` / `struct EventStream` / `connect_ws()` + `connect_sse()` / `struct FirehoseApp` / `struct EventRow` / `SubscribeQuery` + `HistoryQuery`
- **依赖**：fuxi-core, fuxi-events
- **内部模块**：hub.rs / client.rs / tui.rs / error.rs
- **可疑点**：无

## fuxi-memory

- **职责**：策府 · 甲骨（S-P-O）+ 河图洛书（pattern）+ SQLite FTS5
- **pub 顶层 API**：`struct OracleStore` / `struct HetuStore` / `NewFact` + `OracleFact` / `NewPattern` + `HetuPattern` / `struct Extractor` + `ExtractorConfig` / `async fn init_schema()`
- **依赖**：fuxi-core, fuxi-events
- **内部模块**：oracle.rs / hetu.rs / extractor.rs
- **可疑点**：🟡 **extractor.rs v1 stub**，TODO(v2) spawn A2A extractor 门客抽 fact

## fuxi-orchestrator

- **职责**：玄女编排 · 门客注册表 + spawn/dispatch/intervene + 系统事件桥
- **pub 顶层 API**：`struct Fuxi` + `FuxiConfig` / `enum WorkerKind` / `struct Shelf` + `ShelfEntry` + `ShelfStatus` / `trait Intervener` / `struct SystemEventBridge` / `spawn_worker()`
- **依赖**：fuxi-core, fuxi-events, fuxi-workspace, fuxi-agent-cc, fuxi-agent-codex
- **内部模块**：fuxi.rs / registry.rs / bridge.rs / error.rs
- **可疑点（历史）**：🔴 当时记录的 P2.5/P3 缺口包含玄女自己作 A2A server / 介入代理 / 对话权转交 / 多 CLI 适配器。当前 `codex` 已接入，剩余是 gemini/opencode 等新 adapter 与交界 e2e。

## fuxi-scheduler

- **职责**：更漏 · cron + fs watch + HTTP webhook 响应式 trigger
- **pub 顶层 API**：`struct Keeper` + `KeeperConfig` / `trait Clock` / `struct TriggerSpec` / `struct TriggerStore` + `FireRecord` / `enum FireCause` + `FireStatus` / `build_trigger_prompt()` / `new_trigger_id()` + `new_fire_id()` / `should_fire()` / `struct WebhookState` + webhook `router()`
- **依赖**：fuxi-core, fuxi-events + croner + notify
- **内部模块**：keeper.rs / watcher.rs / webhook.rs / store.rs / spec.rs / prompt.rs
- **可疑点**：无

## fuxi-skills

- **职责**：点将台 · 玉牒 loader + 招贤流程（staging→active）+ 贤士录
- **pub 顶层 API**：`load()` + `load_from_file()` / `struct LoadedSkill` / `struct SkillFrontmatter` / `skills_root()` / `stage_write()` + `approve()` + `reject()` / `list_all()` / `struct LedgerEntry` + `LedgerAction` / `render()`
- **依赖**：fuxi-core
- **内部模块**：loader.rs / staging.rs / ledger.rs / template.rs
- **可疑点**：无

## fuxi-cli

- **职责**：二进制入口 · REPL + daemon + 玄女工具子命令
- **binary crate，无 lib.rs**。主线 + 14 个 `mod`：repl / up / watch / subcommands / session / daemon / ipc / demo / memory_cmd / skill / click_registry / theme / client
- **依赖**：**全部 10 个** fuxi-* crate
- **可疑点**：🟡 多处 `#[allow(dead_code)]` + `unwrap()`（REPL/daemon/IPC 错误处理可加固）

---

## 总表

| Crate | 职责 | 依赖 |
|---|---|---|
| fuxi-core | traits 词汇 | — |
| fuxi-events | EventBus + SQLite | core |
| fuxi-a2a | A2A wire + HTTP | core |
| fuxi-workspace | git worktree 隔离 | core |
| fuxi-agent-cc | CC 适配器 | core, a2a |
| fuxi-firehose | 观察器 TUI/WS | core, events |
| fuxi-memory | 策府 SQLite FTS5 | core, events |
| fuxi-orchestrator | 玄女编排 | core, events, workspace, agent-cc |
| fuxi-scheduler | 更漏 cron + webhook | core, events |
| fuxi-skills | 点将台 loader | core |
| fuxi-cli | 二进制入口 | **全部** |

**依赖方向一致、无循环**。词汇层(core) → 事件/存储(events/memory) → 协议/适配(a2a/agent-cc) → 编排(orchestrator) → 观察(firehose) → CLI 入口。

## 最大可疑点

1. 🔴 **fuxi-orchestrator** — 历史记录里的 4 项 P2.5/P3 延期已部分收敛：`codex` adapter 已接入本地 spawn 与 dist worker adapter 路径；剩余是玄女 A2A server、介入代理、让贤、gemini/opencode 等新 adapter，以及 codex 的真实节点 e2e 覆盖。
2. 🟡 **fuxi-memory::extractor** — v1 stub 未实现，对话结束后不自动抽 fact（策府依赖人工 `fuxi memory record`）
3. 🟡 **fuxi-cli** — `allow(dead_code)` + `unwrap()` 多处，REPL/daemon/IPC 错误处理可加固
4. 🟡 **fuxi-a2a::AgentCard ≠ fuxi_core::AgentCard** — 同名结构不同定义，手工转换。未来若 A2A 成为门客间通信基础会踩
