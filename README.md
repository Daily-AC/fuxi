# 伏羲 · Fuxi

> 让个人 AI agent 军团得以有序运行的平台。

**玄女**（顶层 agent）· **门客**（cc/codex/gemini-cli 实例）· **伏羲**（平台）

产品意图、独创赌注、与 ComposioHQ / team-anya 的关系：见 [`docs/superpowers/specs/2026-04-19-伏羲-design.md`](docs/superpowers/specs/2026-04-19-伏羲-design.md)。

---

## 开发者怎么上手

```bash
# 安装 toolchain（rust-toolchain.toml 固定 stable edition 2024）
rustup show

# 全工程门禁
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

# 单 crate
cargo test -p fuxi-events
cargo run -p fuxi-cli -- --help
```

## Crate 地图（P1 当前状态）

| Crate | 目的 | 状态 |
|---|---|---|
| `fuxi-core` | 核心 trait 与类型（Agent/Runtime/Workspace/Task/Event） | ✅ 完工（4/4） |
| `fuxi-events` | EventBus：tokio broadcast + SQLite WAL + replay | ✅ 完工（8/8） |
| `fuxi-a2a` | A2A v1.0 协议实现（types + axum server + reqwest client） | ✅ 完工（10/10） |
| `fuxi-agent-cc` | Claude Code 门客适配器（headless stream-json） | ✅ 完工（40/40 + 1 gated E2E 跑通） |
| `fuxi-firehose` | 实时观察器（WebSocket + SSE + REST + ratatui TUI） | ✅ 完工（30/30） |
| `fuxi-cli` | 二进制 `fuxi`——`demo` / `up` / `watch` | ✅ 完工（2/2） |

**工作区合计：94 passing tests + 1 ignored（E2E gated by `FUXI_RUN_CC_E2E=1`）。**

P1 已跑通 "玄女 spawn cc 门客 → 用户看到事件流" 的锚点场景最小切片：

```bash
cargo run -p fuxi-cli -- demo "Reply with exactly: hi"
# → AgentReady(pid:...) → ThinkingStarted → ThinkingFinished
# → AgentResponded("hi") → TaskStateChanged(Delivering→Done) → exit 0
```

## 架构一眼

```
用户 ─→ 玄女（顶层 cc） ─A2A→ 门客（cc/codex/gemini-cli）
                │                   │
                └────── EventBus ←──┘
                          │
                    Firehose（TUI/Web）
                          │
                         用户
```

三个独创赌注：
1. **主对话权转交**——玄女可把对话交给某个门客（如 PM 直接跟用户聊需求）。
2. **抄送式介入**——用户绕过玄女直接对门客说话时，玄女自动拿到副本。
3. **真实时 Firehose**——WebSocket 推送 + 转场级颗粒度，替代轮询。

## 参考与血统

- **ComposioHQ/agent-orchestrator** 是架构参考——借 7 插件槽分类法、事件词汇表、状态机设计，**不 fork 不 depend**（语言隔离）。
- **team-anya**（用户的既有 TS 项目）提供 `channel_send` 公理、append-only 事件日志、角色 profile 分层组装经验。

## License

MIT
