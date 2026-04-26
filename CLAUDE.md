# CLAUDE.md · 伏羲（fuxi）

> 给未来 Claude Code 会话的工作指令。产品愿景 + 架构见 `docs/superpowers/specs/2026-04-19-伏羲-design.md`。

## 一句话

伏羲是让个人 AI agent 军团有序运行的 Rust 平台。用户只跟**玄女**（顶层 agent）对话，她调度**门客**（cc/codex/gemini-cli 门客）干活。不是工具，是平台。

## 项目根定位

- 真实路径：`/Users/e0_7/fuxi`
- 兼容符号链接：`/Users/e0_7/xihe` → `/Users/e0_7/fuxi`（旧会话/旧 shell history 不破）
- **新代码/新引用一律用 `fuxi`**。`xihe-*` 作为 crate 名已全部淘汰。

## 常用命令

```bash
# 工具链（rust-toolchain.toml 已固定 stable）
cargo fmt --all --check              # 格式检查（CI 门禁）
cargo clippy --workspace --all-targets -- -D warnings  # lint 门禁
cargo test --workspace               # 全部测试
cargo check --workspace              # 快速类型检查

# 单 crate 维度
cargo test -p fuxi-events
cargo clippy -p fuxi-a2a --all-targets -- -D warnings

# 运行二进制（当前是 placeholder）
cargo run -p fuxi-cli
```

## 语言约定

- 文档 / 注释 / 提交信息用**中文**
- 代码标识符 / 模块名 / 文件名用**英文**
- 不写"此函数做 X"式的废话注释。**只写 WHY**（隐含约束、非显而易见的取舍、bug 规避原因）

## Crate 布局

```
fuxi/
├── Cargo.toml                workspace 根
├── rust-toolchain.toml       stable, edition 2024
├── crates/
│   ├── fuxi-core/            traits + 基础类型（Agent/Runtime/Workspace/Event/Task）
│   ├── fuxi-events/          EventBus（tokio broadcast + SQLite WAL + replay）
│   ├── fuxi-a2a/             A2A v1.0 协议（types + axum server + reqwest client）
│   ├── fuxi-agent-cc/        Claude Code 门客适配器（走 cc headless）
│   ├── fuxi-firehose/        实时观察器（WebSocket + ratatui TUI）
│   └── fuxi-cli/             二进制入口 `fuxi`
└── docs/superpowers/specs/   设计文档（不删不改名）
```

## 核心公理（宪法级，改动需格外谨慎）

1. **Headless agent 不显式沟通 = 没做。** A2A 是 agent 唯一出口。直接 println 出去的文字无人看见。
2. **玄女永远有知情权，无否决权。** 抄送机制不得绕过。
3. **真实时，不轮询。** 观察组件必须订阅 EventBus，不得 poll。
4. **CLI 是工具层的唯一形态。** Agent 调用工具直接 shell，不用 MCP。
5. **SQLite 是单一真相源**（WAL 模式 + append-only 事件）。文件系统只是执行快照。
6. **借鉴 ComposioHQ 的设计智慧，不借它的代码路径。** 语言不同天然隔离。
7. **毕设不是 DDL，是顺带。** 伏羲是个人 AI agent 平台，长期用品。不要为毕设答辩时间压缩做动作；也不要把"毕设 demo 够用"当验收标准。日常使用体验才是第一目标。

## 并行工作：agent team，不是 subagent

**用 `TeamCreate` + `Agent(team_name=..., name=...)` + `SendMessage`**，不用单纯 `Agent(subagent_type=...)` 零散 spawn。

- subagent 无共享 TaskList + 无互相通信 + 主线难协调进度 → 过时模式
- agent team = `~/.claude/teams/<name>/config.json` + 共享 `~/.claude/tasks/<name>/` + teammates 用 SendMessage 互通 + 自动 idle 通知
- 典型场景：
  - 拆独立文件 / 独立模块的并行实装
  - 一人 TDD 红一人 TDD 绿（review 循环）
  - 调研 + 实装 + doc 三路并行
- 用 `TaskUpdate owner=<name>` 分派；teammate 完成 `TaskUpdate status=completed` 并 `SendMessage` 给 team-lead
- 完工后 `SendMessage shutdown_request` 到每个 teammate，再 `TeamDelete` 清残留

**决策 01**（2026-04-19 起子 cc 并行）升级为 agent team 模式——用 team 工具而非手工 tmux。

## 工程规范

- **分支**：`feat/fuxi-*` / `fix/fuxi-*` / `chore/fuxi-*`
- **提交信息**：`type(scope): 中文摘要`，如 `feat(events): 加入 replay from cursor`
- **质量门禁**：`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` 全绿才能 merge
- **新增 migration**：SQL 文件 + schema 代码同步，缺一不可

## 常见陷阱

- 不要在 library crate 里 `unwrap()` / `panic!`——返回 `Result`。panic 只允许在 bin 里的顶层错误边界。
- 事件类型 `EventKind` 是 `#[serde(tag = "type")]` 标签联合——加新变体**必须**更新 Firehose 的渲染和 EventStore 的持久化测试。
- `tokio::sync::broadcast` 的 Receiver 从 `RecvError::Lagged` 中拿到的是"落掉了 N 条"，**不是错误终止**——继续接收还是 OK 的。订阅者处理不过来时别让它崩掉整个订阅链。
- SQLite WAL 模式在 macOS 上有偶发 `BUSY`——retry 封装进 EventStore，不往外抛。
- **macOS tempdir symlink**：`TempDir::new()` 返回 `/var/folders/...`，但 `git worktree list --porcelain` 返回 `/private/var/folders/...`。列表比对**两侧都要 `canonicalize()`**，否则匹配失败。
- **Codex model fallback 已改空串**（2026-04-20 `08358fa`）：`DEFAULT_MODEL_FALLBACK = ""` 不传 `-m`，让 codex 按登录账号自选。硬编任何具体模型都会在某种 auth 下被拒。API key 用户须 `export FUXI_CODEX_MODEL=<model>` 覆盖。
- **Codex exec follow-up 限制**（2026-04-26 修措辞，原版"不支持 follow-up"误导过 IM 私聊页设计）：codex worker **busy 时**不支持 `send_message` follow-up（`agent-codex/src/agent.rs:165` 硬返 `CoreError::Other`）。**idle 时** intervene 入口走 degrade-dispatch 是正常路径（`orchestrator/src/fuxi.rs:553-592`），起新 codex 进程跑用户的话，用户视角无感。所以上层（PWA / 玄女 dispatch / etc）**不需要特殊处理 codex 类型**——只在 4xx 时给统一 toast「门客正忙，等这轮跑完再发」即可。要"同 session 多轮续写"得换 codex `conversation` API（另一套，目前未集成）。
- **新 agent 适配器三条铁律**（S1/S2/S3 教训，commit `360a31e` 修掉的）：
  1. 必须提供 `launch_with_id(id, ...)`——不能自己 `AgentId::new()`，否则 `AgentSpawning`/`AgentReady` 属不同 id
  2. 任何事件 pump 要保证"无论怎么退出都摊回 Idle"，否则 shelf 永久 Busy
  3. "find then act" 类接口要原子（`claim_*_by_*` 命名），不能 find + set_status 两步，否则并发会双派
- **cc 反连 --sdk-url 被 Clash TUN 吞**：本机 VPN（Clash/Surge TUN 模式）把 127.0.0.1 也代理走，cc 反连 WS 的 SYN 被拦，表现是 30s timeout。`spawn_claude` 已注入 `NO_PROXY=127.0.0.1,localhost`；**手动起 claude 也要加**（并行 cc team 脚本时踩过）。sia/src/core/cc-process.ts:666-667 同坑同解。
- **起子 cc 必清 `CLAUDECODE*` env**：父 cc（我主线）起子 cc 时子进程继承 env 会触发嵌套检测静默卡死。`agent-cc/spawn.rs` 已做；主线 Bash 起 `claude -p` 也要 `env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_NO_FLICKER -u CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS -u CLAUDE_CODE_EXECPATH`。
- **TUI stderr 重定向必须在 `init_tracing` 之前**（2026-04-20 踩，`main.rs` 修过）：否则 fuxi 启动期（hub/daemon/spawn）的 tracing 打到原 fd 2，alt screen 遮住，退 TUI 后一把冒出来满屏。在 `main()` 开头就 `dup2(file, 2)`。
- **`intervene` Idle 自动退化 dispatch**（2026-04-20 玄女自诊断）：cc Idle 状态下 `active_tx=None`，`send_message` 发 WS 消息响应被 drop。`Fuxi::intervene` 入口检 shelf status，Idle → `dispatch(Task::new("intervention", text))`。见 `docs/decisions/04`。
- **加 EventKind 新变体**必同步 5 处（再强调一次）：`events/store.rs::kind_tag` + `firehose/hub.rs::kind_tag` + `firehose/tui.rs::summarize + color_for` + 持久化测试。clippy `-D warnings` 会一次性报三处。
- **Cargo.lock cherry-pick 后常坏**：多 teammate 改 Cargo.toml 时 lock 自动合并失败。直接 `rm Cargo.lock && cargo build` 重生，比 manual 解快且稳。
- **parser 双发 bug**（2026-04-20 `074ab2e`）：cc stream-json 里 assistant 的文本在 `AssistantText` 和末尾 `ResultSuccess` 会被发两次，老 parser 两处都翻 `AgentResponded` 让 TUI 重复显示。现已加 `TranslateState.responded_this_turn` 去重。**下次改 parser 或加新 EventKind 时留意"同一信息被翻译两次"**。
- **TUI Submit::Xuannv 必须走 `intervene` 不走 `dispatch`**（2026-04-20 `860c377`）：每次 Enter 直接 `Fuxi::dispatch(xuannv, Task::new("user-turn"))` 会堆僵尸 task（idle 无 degrade，busy 无 queue）。正解：`Fuxi::intervene(xuannv, false, text)`——idle 自动 degrade 单 dispatch（Decision 04），busy 入 pending queue（M2.1）。Decision 04 的 degrade title 现在统一为 `"user-turn"`。
- **dispatch pump terminal 不能立即 break**（2026-04-20 `860c377`）：M2.1 drain 在 terminal 事件后把 pending queue 塞 cc 触发新 turn，旧 pump 看到 terminal 立即 break 让 rx drop → 新响应无 receiver 被丢。fix：`Fuxi::dispatch` pump 看 terminal 后**不**break，用 500ms timeout 等；新事件来重置 `saw_terminal`；超时才退。
- **shutdown_agent 必须豁免玄女**（2026-04-20 `1e6db4e`）：`IdleGcTask` 10 分钟 idle 会误杀 xuannv（她对 GC 是普通 agent）。治本：`Fuxi::shutdown_agent` 开头比对 `xuannv_id()`，命中 warn + 返 Ok 静默 noop。**任何新 shutdown 路径（`fuxi kill --id` / 将来的 worker pool rebalance）都从这个方法走，不绕旁路**。只有 `Fuxi::shutdown()`（整体下线）能关玄女。
- **spawn 语义是"新建"，去重不塞在 spawn_worker**（2026-04-20 `fbba2ec`）：用户"起三个鲁班"就真起三个。复用职责在 `dispatch_to_any(role)`（`claim_idle_by_role` 原子），GC 负责回收。
- **fuxi-memory 不能依赖 fuxi-orchestrator**（循环依赖风险）。需要调 orchestrator 的 pattern：trait 定义在 memory，impl adapter 放 **fuxi-cli**（顶层依赖全部 crate）。参考 `fuxi-cli/src/extractor_hook.rs::FuxiExtractorSpawner`。

## 决策 + 过程文档

- `docs/architecture-v1.md` — v1 蓝图（**改方向先改这份**）
- `docs/architecture-audit-v1.md` — v1.1 审查：架构现状 + Gap 矩阵 + Debt D1-D18
- `docs/architecture-v1.1-roadmap.md` — M2-M5 路线图（M2 已完，M3-M5 待推）
- `docs/audit/cratewise-inventory.md` + `docs/audit/event-flow.md` — 审查基础材料
- `docs/decisions/` — 9 份独立决策（01 并行 cc / 02 soul-first skill / 03 任务树 UI / 04 intervene 退化 / 05 让贤保留·被 08 override / 06 文化命名 / 07 P2 召回 scope / 08 让贤拆除 / **09 TUI 照抄 opencode 12 条**）
- `docs/handoff/v1-session5.md` — **最新** 开工指引（M4-REDUX 已 ship · Batch C 待开）
- `docs/handoff/v1-session4.md` — 上一份（保留）
- `docs/handoff/v1-session2.md` — 上一份（保留）
- `docs/session-review-2026-04-*.md` — 过程性记录（为什么这么做 / 踩坑 / 否决方案）
- `docs/superpowers/specs/2026-04-19-伏羲-design.md` — 最早的宏观设计
