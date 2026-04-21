# HANDOFF · 伏羲 / Fuxi

> 给下一个 Claude Code 会话（或者未来的你自己）。读这一页就能接得住。
>
> **2026-04-21 起**：v1.1 阶段进度看 **`docs/handoff/v1-session4.md`**（最新接手指引）。
> 本 HANDOFF.md 是 v0.1 时代的总入口，下面 4 份必读已老但仍是项目背景。

---

## ⚠️ 优先读这 4 份（本 HANDOFF 之外）——**按顺序读**

1. **`docs/superpowers/specs/2026-04-19-v0.1-scenario.md`** — v0.1 验收场景（北极星）。所有工作围绕这个锚
2. **`docs/session-review-2026-04-19-afternoon.md`** ← **今天下午执行过程**：
   - 7 块薄片各自的设计决策链 / 踩过的坑 / 未验证的假设
   - 给 D/G 接手人的开局 checklist 和 D 架构提示
   - 第 §5 "开 D 前必检清单" 和 §7 "给 D 接手人的开局 Checklist" **是直接行动指引**
3. **`docs/session-review-2026-04-19.md`** — 昨晚凌晨过程性（composio 借鉴文件行号 / 被否决的替代方案 / 没写进 CLAUDE.md 的坑）
4. **`docs/references.md`** — 外部项目清单 + composio↔fuxi 精确映射表

**读完这 4 份 + CLAUDE.md 常见陷阱，你应该能直接接 D。**

别直接动手——先按 §5 checklist 验 baseline（跑 `cargo test --workspace` 和手动 `fuxi up` + `fuxi ping`）。

---

## 1. 你即将接手的是什么

**伏羲**：Rust 实现的个人 AI agent 编排平台。**毕设不是 ddl**，最终蓝图才是（用户 2026-04-19 下午明示）。

- 用户只跟**玄女**（顶层 agent）对话
- 玄女调度**门客**（cc / codex / gemini-cli 实例）干活
- 所有事件通过 EventBus 推送，用户实时可见，可介入
- 真实目录：`/Users/e0_7/fuxi` （legacy 软链接：`/Users/e0_7/xihe`）

**一页设计文档**（必读）：`docs/superpowers/specs/2026-04-19-伏羲-design.md`

## 当前开工分支：`feat/fuxi-v0.1`

v0.1 的 9 块薄片（A-I）**7 块已完成**（commits on `feat/fuxi-v0.1`）：

| 薄片 | 状态 | commit |
|---|---|---|
| A · 玄女 SKILL.md | ✅ | `07a3c65` |
| E · dev SKILL.md | ✅ | `07a3c65` |
| H · cc WS `--sdk-url` 反连 | ✅ | `07a3c65` |
| B · CLI 子命令（spawn/dispatch/intervene/status/list/kill） | ✅ | `8b994b9` |
| C · daemon Unix socket IPC | ✅ | `8b994b9` |
| I · 介入事件三联 | ✅ | `2175bfc` |
| F · task_blocked / task_resumed | ✅ | `37fa91a` |
| **D · REPL TUI**（`fuxi` 无参入口） | ⏳ 未做 | |
| **G · 集成测试 · 真跑一次 story** | ⏳ 未做（依赖 D） | |

门禁：fmt / clippy -D warnings / test --workspace 26 个 test block 全绿 0 failed。

**下一块开工**：薄片 D（REPL TUI）。它是用户唯一入口——做完之后用户才能真打开 `fuxi` 用。
  
见 v0.1-scenario.md §2.2 薄片 D 的设计要点 + §7 依赖关系图。

---

## 2. 现在状态（最后落 pen 时间 2026-04-19 凌晨）

```
13 commits on main · 9 个 crate 全绿 · 159 passing tests + 2 gated E2E 都真跑过
```

### Crate 地图

| Crate | 状态 | 作用 |
|---|---|---|
| `fuxi-core` | ✅ | Agent/Runtime/Workspace/Task/Event trait + 状态机 |
| `fuxi-events` | ✅ | EventBus：tokio broadcast + SQLite WAL + replay |
| `fuxi-a2a` | ✅ | A2A v1.0 subset（types + axum server + reqwest client + SSE） |
| `fuxi-agent-cc` | ✅ | Claude Code 门客适配器（`--print` stream-json 模式） |
| `fuxi-agent-codex` | ✅ | Codex CLI 门客适配器（`exec --json`） |
| `fuxi-workspace` | ✅ | git worktree 隔离 |
| `fuxi-firehose` | ✅ | WS/SSE/REST Hub + ratatui TUI |
| `fuxi-orchestrator` | ✅ | 玄女本体（spawn/dispatch/dispatch_to_any/shutdown） |
| `fuxi-cli` | ✅ | 二进制 `fuxi`：demo / up / watch 三子命令 |

### 已验证的真实运行

- `cargo run -p fuxi-cli -- demo "Reply with exactly: hi" --quiet` 跑通全链路
  → AgentSpawning → AgentReady → Thinking → AgentResponded("hi") → Done → AgentDead
- Codex E2E：`FUXI_RUN_CODEX_E2E=1 cargo test -p fuxi-agent-codex --test real_codex_smoke -- --ignored --nocapture` 跑通

---

## 3. 下一步候选（未决，按价值排）

用户醒来后可以挑任意一条：

1. **抄送式介入（InterventionProxy）** · P2 独创赌注之一。用户直接对门客说话时玄女自动收副本。需要：
   - `fuxi-cli` 加 `intervene <agent-id> "message"` 子命令
   - orchestrator 的 `dispatch` 路径加一个 "cc-to-orchestrator" hook，把用户发来的 intervention 作为额外事件推进 bus
2. **主对话权转交（ConversationSwitch）** · 另一个独创赌注。玄女把"当前跟用户主对话"的身份切到某门客（典型：PM 接管需求澄清）。设计需要新模块 `fuxi-conversation` 或 orchestrator 扩展。
3. **gemini/opencode 适配器** · 模板已经成熟（参照 fuxi-agent-cc / fuxi-agent-codex），每个 1-3 天。opencode 自带 HTTP serve 反而更简单——不走 stream-json subprocess，直接 HTTP 请求。
4. **fuxi-cli 上玄女的 dispatch_to_any 演示锚点场景**：多角色门客接力（用户说"开发 IM"→ PM 门客对话 → Dev 门客动工）。这个会真的让锚点场景第一次跑通。
5. **测试覆盖率补齐到 1.4× 对标 ComposioHQ**（当前 ~1.1×）。
6. **事件 `cc_system_other` 噪声**：当前靠 `--quiet` 过，也可以在 parser 层丢掉（下一次 cc 版本升级时一起看）。

**用户"最想看"的下一步**：问他。锚点场景第一次跑通（路径 4）是对毕设最直观的 wow。

---

## 4. 用户怎么想事情（最重要的部分）

下面内容在 `~/.claude/projects/-Users-e0-7-fuxi/memory/*.md` 里都有，新 session 开机时会被自动加载。但值得在这里再强调：

- **用户管什么**：产品意图、宏观链路、命名品味、路径选择（fork/依赖/自造）。
- **Claude 管什么**：语言/库/schema/分层/命名约定/测试策略——自己拍板，**不要问他**。
- **希望被反驳**：他的直觉不对时直说+给理由。讨好式陪跑他厌恶。
- **零 ceremonies**：
  - 不写 plan / planning 文档
  - spec 写进文件给路径，不 inline dump 长设计
  - 不分节反复要 "这一节对吗？"
- **权限**：项目内 `bypassPermissions` 已生效，不要再弹 yes/no
- **关键原话**（要记进骨子里）：
  - "没有啥歧义就一直往后推吧"
  - "工程性的决策你不该问我"
  - "agent 的能力是高于我的……我倒想让 cc 也好 codex 也罢来反驳我"
  - "我花钱买订阅让你特么纸上谈兵的吗？"

---

## 5. 骨架约束（改动前看一眼）

### 公理（CLAUDE.md 里有完整版）

1. **Headless agent 不显式沟通 = 没做**。所有 lifecycle / state change 必须过 EventBus。
2. **玄女永远有知情权，无否决权**。抄送不得绕过。
3. **真实时不轮询**。观察组件订阅 bus，不 poll。
4. **CLI 是工具层唯一形态，不用 MCP**。
5. **SQLite 是单一真相源**（WAL append-only）。文件系统只是快照。
6. **借鉴 ComposioHQ 的设计智慧，不借代码**。语言隔离（TS vs Rust）天然保护。

### 命名

- 产品：**伏羲**  / crate 前缀：**`fuxi-*`**
- 顶层 agent：**玄女**
- 员工 agent：**门客**
- 禁止重新出现 `xihe-*` 做 crate 名。

### 技术栈

- Rust stable edition 2024（`rust-toolchain.toml` 固定）
- tokio · axum · sqlx SQLite · tokio-tungstenite · ratatui · clap · tracing
- A2A v1.0（Google Agent2Agent）自实现（Rust 无现成 SDK）

---

## 6. 常用命令

```bash
cd /Users/e0_7/fuxi

# 日常开发门禁
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

# 单 crate 维度
cargo test -p fuxi-orchestrator
cargo clippy -p fuxi-events --all-targets -- -D warnings

# 跑 demo（真调 claude，花 $0.05 左右）
cargo run -p fuxi-cli -- demo "Reply with exactly: hi" --quiet
cargo run -p fuxi-cli -- demo --tui     # 带 ratatui 仪表盘

# 跑 gated E2E
FUXI_RUN_CC_E2E=1 cargo test -p fuxi-agent-cc --test real_cc_smoke -- --ignored --nocapture
FUXI_RUN_CODEX_E2E=1 cargo test -p fuxi-agent-codex --test real_codex_smoke -- --ignored --nocapture

# 本机已装的 agent CLI：claude / codex / opencode / gemini
```

---

## 7. 必读顺序（新 session 开局）

1. **`OVERNIGHT-2026-04-19.md`** — 夜里做了什么，一眼看
2. **`docs/superpowers/specs/2026-04-19-伏羲-design.md`** — 设计总图
3. **`CLAUDE.md`** — 项目规范 + 常用命令 + 公理
4. **`docs/architecture-decisions.md`** — 6 条 ADR（语言/协议/cc 适配/EventBus/ComposioHQ 关系/权限）
5. **`README.md`** — 对外的快速上手
6. `~/.claude/projects/-Users-e0-7-fuxi/memory/MEMORY.md` — memory 索引（会自动加载）

---

## 8. 已踩过的坑（避免重蹈）

- **`claude --bare` 会跳过 keychain** → 默认不加，否则用户的订阅 auth 丢。`cc_system_*` 噪声用 `--quiet` 过滤或在 parser 里丢即可。
- **目录 `/Users/e0_7/xihe` 是符号链接** 指向 `/Users/e0_7/fuxi`。用 canonical 路径 `fuxi`；legacy 链接保留让旧 session 不破。memory 目录同理：`~/.claude/projects/-Users-e0-7-xihe` 软链接到 `...-fuxi`。
- **后台 agent 运行期间 `git add -A` 会捕获半成品** → 用精确文件列表或等 agent 完成后再 commit。
- **`cargo clippy --workspace` 在子 agent 还没写完新 crate 时会失败** → 改用 `-p <specific>` 或等完工。
- **macOS tempdir `/var/folders/...` vs `/private/var/folders/...`**：git worktree list 报前者，canonicalize 才能对齐。
- **codex ChatGPT-account auth 拒绝 `gpt-5.1-mini`**：fuxi-agent-codex 的 `DEFAULT_MODEL_FALLBACK` 对 API-key 好使；ChatGPT-account 用户需要 `FUXI_CODEX_MODEL` 覆盖或留空让 codex 自选。
- **Rust edition 2024 的 `if let ... && ... ` 需要新编译器**；rust-toolchain.toml 固定 stable，OK。

---

## 9. 上次 code review 发现的（已修，**但记得**）

reviewer 找出 3 个必修 bug（已在 `360a31e` 修掉），但值得新 session 知道它们为什么重要：

- **S1 · Agent ID 双生**：如果在新 agent 适配器（gemini/opencode）里再写一个 `launch(profile, cfg)` 自己生 id，而不是接受 `launch_with_id(id, ...)`，同样的 bug 会复现。**新适配器必须提供 `launch_with_id`**。
- **S2 · pump 状态泄露**：任何新加的"事件 republish pump"都要保证"无论怎么退出都摊回 Idle"。orchestrator 那条 pattern 要复制。
- **S3 · TOCTOU race in `dispatch_to_any`**：加新的 "find then act" 接口时用 `claim_*_by_*` 这种原子命名，别回到 find + set_status 两步。

---

## 10. 还没决的事

- **玄女的 profile / prompt 内容**（role→prompt 映射表）——这是角色层，P2.5 再做。
- **云 burst 触发策略**（手动 vs 自动判断负载）——P3 再决。
- **A2A Rust SDK 是否开源回馈社区**——P4 可选。
- **opencode 适配器走 HTTP 而非 stream-json**——技术上更干净，用户愿意接受"两种门客形态"就做。
- **PM 门客直接对话用户**（ConversationSwitch）的具体 UI：REPL 切换提示符？TUI 分栏？还是都保留？——产品层问题，问用户。

---

## 11. 不要做的事

- 不要再写 plan / planning 文档
- 不要把设计 inline dump 到 chat
- 不要问用户工程细节
- 不要用 `xihe-*` 做新 crate 名
- 不要默认 `claude --bare`
- 不要在 orchestrator 的 pump 里 poll（公理 #3）
- 不要在新 agent 适配器里 `AgentId::new()`（S1 教训）
- 不要为了"未来扩展"留空类型 / 空方法（YAGNI；参考 `WorkerDeps` 被删的教训）

---

## 12. 如果用户说"继续开发"

走这个默认路径：

1. 读 §7 的必读顺序
2. 问用户：「接上次停在 P2 的 orchestrator + 两个门客适配器完工。下一步你想先推哪个？（抄送介入 / 主对话权转交 / 多角色锚点场景演示 / gemini 适配器 / opencode 适配器）」
3. 他选了就开干——可以并行 dispatch 子 agent。参考本次 overnight 的节奏：每个 crate 一个 agent、明确门禁（fmt+clippy -D+test）、回来后 code review 一轮、修 bug、commit 里程碑。

---

**用户原话签收**："一起加油"。
