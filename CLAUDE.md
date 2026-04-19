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
- **Codex model fallback**：`DEFAULT_MODEL_FALLBACK = "gpt-5.1-mini"` 在 **ChatGPT-account auth** 下会被拒 `invalid_request_error`。E2E 测试用空串让 codex 自选；API key 用户需 `FUXI_CODEX_MODEL` 覆盖。
- **Codex exec 不支持 follow-up**：`send_message` 直接返回 `CoreError::Other`。codex 门客是 spawn-per-dispatch 模式，不像 cc 那样支持同 session 续写。要多轮对话得换 codex `conversation` API（另一套）。
- **新 agent 适配器三条铁律**（S1/S2/S3 教训，commit `360a31e` 修掉的）：
  1. 必须提供 `launch_with_id(id, ...)`——不能自己 `AgentId::new()`，否则 `AgentSpawning`/`AgentReady` 属不同 id
  2. 任何事件 pump 要保证"无论怎么退出都摊回 Idle"，否则 shelf 永久 Busy
  3. "find then act" 类接口要原子（`claim_*_by_*` 命名），不能 find + set_status 两步，否则并发会双派

## 设计文档

`docs/superpowers/specs/2026-04-19-伏羲-design.md` — 宏观设计。改方向前先改这份。
