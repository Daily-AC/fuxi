# 伏羲 · Fuxi

<p align="center">
  <img src="docs/assets/readme-hero.png" alt="伏羲：玄女与门客" width="800">
</p>

> 让多个 AI agent CLI（Claude Code / Codex / Gemini）以 **「玄女-门客」** 架构有序协作的 Rust 平台。

**玄女**（顶层 agent，跟用户对话）· **门客**（cc/codex/gemini 实例，干活的）· **伏羲**（平台本身）

---

## 它为什么存在

家用 + 自驱的 AI agent 编排框架，市面上要么是云原生（要 SaaS 账号），要么是 SDK fork（绑定某家 CLI 的 quirks）。伏羲反过来做：

- **不 fork 任何 CLI**——把 Claude Code / Codex CLI 当 OS 进程包装，CLI 怎么升级你怎么用
- **顶层一个长跑的"玄女" agent**——用户所有对话都跟她走，她负责派活、汇报、收尾
- **真实时事件总线 + 跨节点 sandbox + 上下文自感知**——不轮询、不 polling、不丢历史

让你的 cc 不再单线运行，让 `~/erp` 项目在家用服务器上活着，让 Claude 自己累了能告诉你"要不要重启副本"。

---

## 一眼架构

<p align="center">
  <img src="docs/assets/readme-arch.png" alt="伏羲架构示意" width="800">
</p>

```
用户 ─→ 玄女（长跑 cc） ──A2A─→ 门客（cc/codex/gemini）
         │                         │
         └─────── EventBus ←───────┘
                    │
          Firehose / PWA / TUI
                    │
                   用户
```

- **玄女**：唯一跟用户对话的 agent；不亲手干活，只派活、汇报、追问。
- **门客**：cc/codex/gemini 子进程，每个住进 git worktree 跑专项任务。
- **EventBus**：`tokio::broadcast` + SQLite WAL append-only，所有事件可订阅可回放。
- **Firehose**：实时事件渲染——TUI、WebSocket、SSE、REST 四套出口。
- **PWA**：移动端入口，4 tab（玄女 / 任务 / 通知 / 更多）+ 8 卡 hub（节点 / 项目 / 工作者 / 交付物 / 记忆 / 角色 / 更漏 / 设置）。

---

## 它能干什么

- **玄女自动派活** — `@<节点>` / `@<项目>` / `@<门客>` mention 路由
- **跨节点 sandbox** — `home` 服务器 + `macbook` 共享同 git repo，按需拉对方机器跑活
- **真实时 Firehose** — WebSocket 推送，无 polling；公理 #3「真实时不轮询」
- **PWA 移动端** — iOS/Android 装 PWA 当原生 app；通知 tab 集中收门客 review 请求 + bug 收集器 + 上下文 handoff offer
- **玄女上下文自感知**（task #8）— 跨 35% context 自动收紧，跨 45% 主动问用户「要不要重启副本」；用户回「换」她自己写 handoff，后端 kill 老副本 + spawn 新副本注入 prelude 接班
- **memory-v2 三表分流**（按 ICML 2026 论文 arXiv:2604.14004）— `oracle_facts`（甲骨：原始事实）/ `user_profile`（用户身份卡）/ `hetu_patterns`（河图心法）；低层 trace 不污染门客 prompt（=避免 negative transfer）
- **更漏 trigger** — cron / once / fs-watch / webhook 四类触发器，玄女作为 dispatch 入口
- **A2A v1.0 自实现** — Rust 生态首个完整实现（types + axum server + reqwest client）
- **Decision 13 sentinel 协议** — 门客自决何时 nudge 玄女审阅，避免攻击玄女注意力
- **Decision 21/22 工作区生命周期** — L1 read-only / L2 ephemeral / L3 持久 sandbox + 交付物五分类

---

## 快速开始

### 安装

```bash
git clone https://github.com/<your-fork>/fuxi
cd fuxi
./scripts/install.sh   # 装到 ~/.cargo/bin/fuxi
```

`fuxi` 启动时会先探测自身在 `$PATH`——不在就直接报错，**不**让 TUI 起来后才发现工具瘫痪。

### REPL 模式（开发）

```bash
fuxi              # 玄女 TUI（含 firehose 内嵌 + composer）
fuxi up           # firehose 独立面板 + REPL 一起起
```

### 家用部署（systemd 长跑）

```bash
fuxi im start --bind 127.0.0.1:9100
# 配 nginx 反代 :8443 + 通配符证书 → PWA on https://im.<your>.com:8443
```

部署模板见 [`scripts/deploy-home.sh`](scripts/deploy-home.sh)（rsync + cargo + pnpm + smoke 一键跑）。

### 最小事件流验收

```bash
cargo run -p fuxi-cli -- demo "Reply with exactly: hi"
# → AgentReady(pid:...) → ThinkingStarted → ThinkingFinished
# → AgentResponded("hi") → TaskStateChanged(Delivering→Done) → exit 0
```

---

## Crate 地图

| Crate | 目的 | 状态 |
|---|---|---|
| `fuxi-core` | 核心 trait 与类型（Agent/Runtime/Workspace/Task/Event） | ✅ 可用 |
| `fuxi-events` | EventBus：tokio broadcast + SQLite WAL + replay | ✅ 可用 |
| `fuxi-a2a` | A2A v1.0 协议实现（types + axum server + reqwest client） | ✅ 可用 |
| `fuxi-agent-cc` | Claude Code 门客适配器（含 result.usage 抓取） | ✅ 可用 |
| `fuxi-agent-codex` | Codex CLI 门客适配器 | ✅ 可用 |
| `fuxi-firehose` | 实时观察器（WebSocket + SSE + REST + ratatui TUI） | ✅ 可用 |
| `fuxi-orchestrator` | 玄女编排层：spawn / dispatch / bridge / dist enqueue / xuannv_context | ✅ 可用 |
| `fuxi-workspace` | git worktree 隔离 + L1/L2/L3 三层 | ✅ 可用 |
| `fuxi-skills` | 点将台：role loader / staging / ledger | ✅ 可用 |
| `fuxi-memory` | 策府：oracle / user_profile / hetu_patterns 三表 + cangjie extractor | ✅ 可用 |
| `fuxi-scheduler` | 更漏：cron / once / fs / webhook trigger | ✅ 可用 |
| `fuxi-im` | IM API、PWA 后端、节点/任务视图、push、上传、通知 | ✅ 可用 |
| `fuxi-cli` | 二进制 `fuxi`：REPL / daemon / IM start / dist / handoff / tools | ✅ 可用 |

---

## 工程门禁

CI 全绿才能 merge：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

(cd crates/fuxi-im/web && pnpm test && pnpm typecheck && pnpm lint)
```

单 crate 调试：

```bash
cargo test -p fuxi-events
cargo run -p fuxi-cli -- --help
```

提交、分支、发布约定见 [`docs/git-workflow.md`](docs/git-workflow.md)。

---

## 公理（不可越）

伏羲有几条 **宪法级公理**——任何代码改动都必须遵守，详见 [`CLAUDE.md`](CLAUDE.md)：

1. **Headless agent 不显式沟通 = 没做。** A2A 是 agent 唯一出口；`println` 出去的字无人看见。
2. **玄女永远有知情权，无否决权。** 抄送机制不得绕过。
3. **真实时，不轮询。** 观察组件必须订阅 EventBus。
4. **CLI 是工具层的唯一形态。** Agent 调用工具直接 shell，不用 MCP。
5. **SQLite 是单一真相源**（WAL 模式 + append-only）。文件系统只是执行快照。
6. **借鉴优秀设计，不借代码路径。**
7. **毕设不是 DDL，是顺带。** 伏羲是长期使用品，不为答辩压缩动作。

---

## 文档地图

- [产品愿景 + 独创赌注](docs/superpowers/specs/2026-04-19-伏羲-design.md)
- [架构 v1 蓝图](docs/architecture-v1.md)
- [v1.1 路线图](docs/architecture-v1.1-roadmap.md)
- [Decision 决策日志](docs/decisions/) — 9 份独立决策（并行 cc / 任务树 UI / intervene 退化 / handoff scope ...）
- [Session handoff 工程日志](docs/handoff/) — 每次大改动都有一份，含 trade-off / 踩坑 / 否决方案

---

## 状态

伏羲是个人长期使用项目，承载我每天的真实业务（ERP 项目维护、家用服务器运维、多 cc 并行 PoC、毕设）。**它已经在 home 节点 systemd 长跑** + 我所有 mac 上派活都走它。

开源是顺手——欢迎 issue、欢迎 PR，但请先理解 [`CLAUDE.md`](CLAUDE.md) 的工程公理 + 看一眼最近一份 [handoff 文档](docs/handoff/) 了解当前心智再动手。

## License

MIT
