# Session Review · 2026-04-19 下午（v0.1 执行阶段）

> [!WARNING]
> `historical`：此文档为 2026-04-19 下午的过程记录，保留用于追溯当时决策，不再作为当前执行入口或状态口径。
> 当前状态以 `docs/status/now.md` 为准。

> 这份**不是给用户看的**，是给下一个 Claude Code 接手人看的**过程性记录**。
>
> 同目录的 `session-review-2026-04-19.md` 记录的是**凌晨 P1/P2 基建 + 夜间审计**的过程性。
> 这份记录**下午 v0.1 执行阶段**：7 块薄片落成的决策链、踩过的坑、剩下 D+G 该怎么接。
>
> ## 必读顺序（接手人开场 10 分钟）
>
> 1. `HANDOFF.md` 开头"优先读这 3 份" → 指向 v0.1 spec / 这份 / references
> 2. `docs/superpowers/specs/2026-04-19-v0.1-scenario.md` **完整读**——北极星
> 3. 这份 —— 执行过程 + D/G 开工指南
> 4. `CLAUDE.md` 常见陷阱（午后又加 3 条：sandbox-exec / agentskills 格式 / intervene 事件三联）
>
> 开始 D 前按本文 §5 的检查清单逐项确认。

---

## 1. 下午一共干了什么（7 块薄片 + 5 个 commit）

分支 `feat/fuxi-v0.1`。按 commit 时序：

| commit | 内容 | 行数 |
|---|---|---|
| `fe3275b` | docs(v0.1): 场景锁定 + 过程性记录 + composio 对标精确化 | +645 -3 |
| `07a3c65` | feat(agent-cc): 薄片 H——cc WS --sdk-url 反连 + skills/xuannv + skills/dev | +862 -179 |
| `8b994b9` | feat(cli): 薄片 B+C——玄女工具子命令 + daemon Unix socket | +1056 -26 |
| `2175bfc` | feat(orchestrator): 薄片 I——介入事件三联 | +245 -8 |
| `37fa91a` | feat(orchestrator+cli): 薄片 F——task_blocked / task_resumed | +161 |
| `b605e66` | docs: HANDOFF 更新到 v0.1 进度 | +31 -1 |

**累计**：~3000 行新增 Rust + Markdown，15 个新单测，26 个 test block 全绿。

---

## 2. 设计决策链（为什么做成这样）

### 2.1 cc WS `--sdk-url` 反连 > stdio `--print`

**为什么升级**：用户在本 session 里明确："赶紧改成 ws，都给你源码了"。anya 的 `claude-code-backend.ts` (1066 行 TS) 是完整 reference。stdio 模式做不到：
- 打断 turn（v0.1 场景 §1 断言 17/18：用户看 diff 后说"第三个 case 命名改掉"）
- 工具审批 hook（`can_use_tool`，v0.1 全 allow，v0.2 做策略层）
- 细粒度事件（`tool_progress` / `keep_alive` / `rate_limit_event`）

**实装结构**（`crates/fuxi-agent-cc/src/ws_bridge.rs`）：
```
CcAgent::launch_with_id (async)
  ├── WsChannel::bind(sid)          ← 起 axum WS server @ 127.0.0.1:0
  ├── cfg.sdk_url = Some(channel.url())
  ├── spawn_claude(cfg)              ← claude --sdk-url ws://... (反连客户端)
  ├── select! { wait_connect / child early exit / timeout }
  └── pump task: loop channel.recv() → parse_line → translate → event_tx
```

**关键技术点**：
- `tokio::select!` 里 `child.wait()` future 会借走 child 直到作用域结束——用 `{}` block 限定借用作用域，否则 child 无法 move 进 Inner（踩过这个坑，见 `agent.rs:132-166` 的注释）。
- `outgoing_tx` 是 `mpsc::UnboundedSender`——连接建立前消息**自动入队**，reader 起来后 flush。
- `control_request { subtype: "can_use_tool" }` 现在一律回 allow（v0.1 策略层延后）。
- `control_request { subtype: "interrupt" }` 是**我们主动发**的，claude 回 `control_response` ack。

**未升级的路径**：Anthropic Agent SDK `can_use_tool` response 格式我**猜**是 `{behavior: "allow", updatedInput: {}}`——参照 anya:881-913。真跑过没？**没**。gated E2E (`real_cc_smoke`) 这次 session 没跑（要花 API $$）。**G 薄片第一件事就是跑它**。

### 2.2 CLI 子命令 = 玄女工具（不是人类界面）

**为什么这样分**：用户在本 session 里明确："我起不了门客呀，我的第一交互对象肯定是玄女啊"。

所以 `fuxi spawn --role dev`、`fuxi dispatch --to <id> ...`、`fuxi intervene ...` 这些**不是给用户的**，是**玄女的 Bash 工具**。

**具体落点**：玄女的 `skills/xuannv/SKILL.md` 里 `allowed-tools: Bash(fuxi:*) Read`——她只能调 `fuxi` 前缀的 Bash 命令 + 读文件。这由 cc 的工具 scope 语法（`Bash(pattern)`）限制。但 v0.1 bypass 模式下**所有工具审批回 allow**，这个限制靠 cc 自己的 prompt-layer 执行。

**用户视角**：`fuxi`（无参）应该进 REPL，直接跟玄女对话——这是 **D 薄片**的事，还没做。

### 2.3 daemon 单进程 vs 多进程

选了**单进程**：
- `fuxi up`（或未来 `fuxi` 无参）起一个进程，里面同时跑：
  - EventBus + SQLite
  - Firehose Hub (HTTP/WS/SSE @ 127.0.0.1:4100)
  - daemon Unix socket listener (@ /tmp/fuxi.sock)
  - Fuxi orchestrator（管玄女 + 门客）
  - 未来（D）：ratatui REPL TUI
- `fuxi spawn / dispatch / ...` 等子命令是**短命 client 进程**，连 socket 发一条 JSON 就断。

**为什么**：简单 > 正交。多进程架构（daemon 独立 + REPL 独立）更模块化但 IPC 复杂度上升 1 倍，v0.1 不值。v0.2 想拆再拆。

**副产物**：子进程（claude CC 实例）从父进程继承环境变量。所以玄女的 CC 实例能看到 `$FUXI_SOCK`，Bash 工具 `fuxi intervene ...` 就会连对 socket。**这是必须 work 的隐式约定**，别拆掉父子进程关系。

### 2.4 Skill 采纳 agentskills 格式

**为什么**：本 session 里和用户澄清——公开协议（https://github.com/agentskills/agentskills），Microsoft/Cursor/OpenCode 都采纳。fuxi 自造格式是纯损失。

**实装**：`crates/fuxi-cli/src/skill_loader.rs` 解析 frontmatter：
- `name` · 必须 = 父目录名（`xuannv` / `dev`），纯 ASCII lowercase
- `description` · ≤ 1024 字符
- `allowed-tools` · **空格分隔字符串**（不是数组！这是 agentskills spec 的坑，容易写错）

Body 作 `append_system_prompt` 注入到 cc CLI。**这里有一个 bug 潜伏**：cc 的 `--append-system-prompt` 是**追加**到 cc 内置的 system prompt，不是**替换**。内置 prompt 含"你是 Claude Code..."；我们追加"你是玄女..."。**这两份 prompt 同时在，可能冲突**。没验证过实际效果。**D 薄片或 G 薄片要确认**玄女能不能正常扮演调度者。

**skills 目录查找顺序**（`skill_loader.rs::skills_root`）：
1. `$FUXI_SKILLS_DIR`
2. git root + `/skills`
3. cwd + `/skills`

**这是个陷阱**：如果用户在 `~/team-anya/` 里跑 `fuxi`，git root 变成 anya 仓库，没有 `skills/` 目录 → 查找失败。**D 做完必须测这个**。建议 D 的做法：启动时如果 `skills_root()` 返回 None 且 `~/.fuxi/skills/` 不存在，提示用户 `export FUXI_SKILLS_DIR=/path/to/fuxi/skills`。

### 2.5 介入三联事件（薄片 I）

**UserInterventionSent** 原来只有 `target / text`，加了 `mode: String`（"append" / "interrupt"）——不加的话 TUI 和 audit log 分不清两种介入。**这是 EventKind 的 schema 破坏性变更**，任何外部订阅者（未来有）要感知。

**AgentInterrupted** / **TaskInterventionApplied** 全新。三联在 `Fuxi::intervene` 里顺序发：
1. UserInterventionSent（总发）
2. AgentInterrupted（仅 interrupt 模式）
3. TaskInterventionApplied（总发，wire 层收尾）

**注意**：这三条事件在 `Fuxi::intervene` 返回前就发完了——**不是**异步等 cc 真打断后才发。实际 cc 可能还要几百 ms 才真停 turn。Timing 敏感的测试要知道这点。

### 2.6 task_blocked / resumed（薄片 F）

**为什么独立做这块**：v0.1 scenario §1 断言 13（task 进 Blocked 等 commit 授权）+ 24（用户同意后 resumed）。玄女的请示-授权循环靠这两条事件观察。

**实装最简**：`Fuxi::block_task` / `resume_task` 只 `publish` 事件——**不动**任何运行时状态（ShelfStatus / TaskState）。因为：
- 门客实际的"等待"是 cc 自己停下来等 user 输入的状态
- 我们的事件只是**观察者信号**
- 下一任务继续时玄女会调 `fuxi dispatch` 或 `fuxi intervene`，那时 ShelfStatus 自然回到 Busy

**这是故意最小**。想加状态机的冲动要抑制——YAGNI（session-review-凌晨 §5 S2 教训：Blocked 不是 terminal）。

---

## 3. 踩过的坑（新适配器 / 下游注意）

### 3.1 axum 0.8 的 WS 握手路径语法
`Router::new().route("/ws/cli/:sid", ...)` 在 0.8 里报错——新语法是 `{sid}`（`/ws/cli/{sid}`）。参照 `ws_bridge.rs:141`。

### 3.2 tungstenite 0.26 的 Message::Text
`Message::Text(Utf8Bytes)` 而不是 String。构造时 `Message::Text(s.into())` 即可（AsRef<[u8]>）。

### 3.3 `AgentId::from_str` 不存在
`define_id!` 宏没生成 FromStr。只有 `From<Uuid>`。解析 "agent-<uuid>" → 先 `strip_prefix("agent-")`，再 `Uuid::parse_str`，再 `AgentId::from`。见 `daemon.rs::parse_agent_id`。

### 3.4 `tokio::select!` + `child.wait()` 的 borrow 陷阱
（见 §2.1）如果 `tokio::pin!(child.wait())` 在函数体顶层，child 的 mutable borrow 持续到函数末尾，后续 `child: Some(child)` 移动会报 E0505。解决：把 select 包进 `{}` block。

### 3.5 `EventKind` 加变体必触发一堆 exhaustive match fail
改过这些：
- `crates/fuxi-events/src/store.rs::kind_tag`
- `crates/fuxi-firehose/src/hub.rs::kind_tag`
- `crates/fuxi-firehose/src/tui.rs::summarize` + `color_for`

**下次加新变体 / 修字段，clippy -D warnings 会一口气报三处**。一次全改完。

### 3.6 format string escape
Rust format 字符串里想打 `{` / `}` 要双写 `{{` / `}}`。见 dispatch.rs 里 `TaskInterventionApplied {{ mode=append }}`。踩过一次。

### 3.7 cc `--append-system-prompt` 的追加语义
（见 §2.4）追加不替换。玄女 + 门客 Skill 的 body 里**不要**写"你是 Claude Code..."重复 cc 内置 prompt，直接写"你是玄女/dev 门客..."。

### 3.8 skills_root 的 cwd 依赖
（见 §2.4）用户在非 fuxi 仓跑 `fuxi`，skills 查找会失败。**D 必须处理这个**。

---

## 4. 当前端到端手动跑（未真跑过，但编译绿）

理论上这样能跑起来：

```bash
# 终端 1：起平台
cd /Users/e0_7/fuxi
cargo run -p fuxi-cli -- up --workspace-root /Users/e0_7/fuxi
# 输出：
# 伏羲 up
#   HTTP  http://127.0.0.1:4100  (WS /ws · SSE /sse · REST /events)
#   SOCK  /tmp/fuxi.sock (玄女工具口)
#   Ctrl-C 停止

# 终端 2：健康检查
cargo run -p fuxi-cli -- ping
# → {"status":"pong"}

# 起一个 dev 门客
cargo run -p fuxi-cli -- spawn --role dev
# → {"agent_id":"agent-<uuid>"}

# 派任务（会真调 claude，消耗 $$）
cargo run -p fuxi-cli -- dispatch --to agent-<uuid> --title "echo" Say hi

# 观察事件
cargo run -p fuxi-cli -- watch  # 或 curl http://127.0.0.1:4100/sse
```

**没真跑过**。D 做完前，E2E 验证阻塞。

---

## 5. 开 D 前必检清单

D（REPL TUI）在 v0.1-scenario.md §2.2 标为 1.5d。下面是我没写进 spec 但接手必须知道的：

### 5.1 D 的最小可行范围（v0.1）

**必须做**：
- `fuxi`（无参）进 REPL 模式
- 起 embedded daemon + 自动 spawn 玄女 CC（role=xuannv）
- ratatui 简化布局：顶部玄女对话 / 中部事件流 / 底部输入框
- 用户输入走 append 介入到玄女（`Fuxi::intervene(xuannv_id, false, text)`）
- 订阅 EventBus，事件实时上屏
- Ctrl-C / `q` 退出，先 shutdown 玄女再退
- TUI 退出后 terminal restore（crossterm raw mode / alternate screen / show cursor）

**defer（v0.2 再做）**：
- 左任务树 + 右任务元信息的三栏（用户今天原本想要但 v0.1 spec 砍到单区）
- 鼠标点击切 Switch 语义
- 事件流渲染的精致化（highlight / color / wrap）

### 5.2 用户对 TUI 的**具体期望**（本 session 对话提炼）

来自本 session 多轮对话：

1. **左侧任务树 + 中间对话区 + 右侧任务元信息**（三栏目标）——v0.2 再做
2. **鼠标点击 + 快捷键双通道**——v0.2
3. **Switch = 点左侧任务节点 → 主对话对象变该节点负责门客**——v0.2
4. **Esc 切回玄女 / Tab 切侧栏 view**——v0.2
5. 右侧 #3（当前任务元信息）是默认——v0.2
6. 工具审批 v0.1 全 allow，队列 v0.2
7. 介入右侧要标红高危——v0.2

**v0.1 D 只做骨架**：一屏对话 + 一屏事件 + 一行输入。但 API 和状态要**给 v0.2 留口**：
- 布局函数接受 `LayoutMode::Simple | ThreePane`
- 事件消费走 firehose Hub 的 WS client（不是直接订阅 EventBus）——给 v0.2 跨进程/跨机留路

### 5.3 实装 D 的关键组件

a) **ratatui + crossterm 的 terminal lifecycle 陷阱**
   - `fuxi up` 和 `fuxi`（无参）可能共存——后者要 `EnterAlternateScreen`/`EnableMouseCapture`/`enable_raw_mode`；但 `fuxi up` 不该。
   - D 的 main loop 要包在 `let terminal = setup()?; ...; restore(&mut terminal)?;` 且 **panic hook 也要 restore**——否则 crash 后 terminal 挂死。`crates/fuxi-cli/src/watch.rs` 或 `demo.rs` 应该已经有参考。

b) **D 里怎么拿到 firehose 事件**
   - 现在 `fuxi-firehose::Hub::subscribe()` 应该直接返回 `broadcast::Receiver<Event>`（未验证——先 grep 确认）
   - D 无参启动 = 和 daemon 同进程 → 可以直接拿 Hub。**不需要**走 WS 客户端（那是 `fuxi watch` 做的：连远程 Hub）。
   - 关键：ratatui 事件循环（`crossterm::event::poll`）+ 事件流（`broadcast::Receiver`）要 multiplex。用 `tokio::select!` 跑一个任务 `select!` 两条流喂 TUI 状态。

c) **玄女怎么启动**
   - D 里 embed daemon 和 orchestrator 之后，直接调 `Fuxi::spawn_worker(xuannv_profile, WorkerKind::Cc(xuannv_cfg))`——**不经过**IPC socket（那是其它 client 用的）。
   - 但**玄女自己发 Bash → `fuxi intervene`** 时走 socket。这是对的——她不知道自己在 D 进程内。

d) **玄女的对话区内容怎么来**
   - 订阅 EventBus 过滤 `agent == xuannv_id`。
   - 把 `AgentResponded { text }` 拿出来作为"玄女说的话"渲染。
   - **不要**自己解析她的 stdout——那是 cc stream-json 的底层，fuxi-agent-cc 已经翻译成事件。

e) **用户输入怎么到玄女**
   - 用户在底部输入框按 Enter →直接调 `Fuxi::intervene(xuannv_id, false, text)`（追加模式）
   - Fuxi::intervene 会走 `UserInterventionSent / TaskInterventionApplied` 事件——TUI 自己也会收到（订阅了）——可以渲染成"用户说: xxx"

f) **第一次启动玄女**
   - 玄女 spawn 后还没"在。想做什么？"——因为她是 cc headless，没 prompt 就不说话。
   - **解决方案**：D 启动时立刻 dispatch 一个伪任务 `Fuxi::dispatch(xuannv_id, Task::new("greet", "向用户问好"))`，让玄女说第一句。或者在 Skill body 里强制她启动就说第一句（但 cc --append-system-prompt 语义不保证她主动说话）。
   - **选前者更可控**。

### 5.4 skills 目录问题（见 §3.8）

D 第一件事：启动时 `ensure_skills_available()`：
- 检查 `skills_root()` 是否非 None
- 如果是 None：尝试 `env::set_var("FUXI_SKILLS_DIR", ...)` 指向 fuxi 仓库的 skills/；但需要知道 fuxi 仓路径。可从 `std::env::current_exe()` 向上找，或 hardcode `/Users/e0_7/fuxi/skills`（v0.1 单机 + 用户就自己） OR 提示用户。
- **最简**：D 启动 README 提示 "export FUXI_SKILLS_DIR=..." 如果 skills_root 返回 None。

### 5.5 G 做什么（D 之后）

**G = 跑 v0.1 spec §1 场景真跑一次**。33 个事件全到 SQLite 才算 v0.1。

- 需要真 claude CLI + $ API 额度
- 需要 team-anya 仓库（场景里拿它的 parser.test.ts 当目标）——本机有（`/Users/e0_7/team-anya`）
- E2E 脚本建议：`crates/fuxi-cli/tests/v01_story.rs`，gated by `FUXI_RUN_V01_STORY=1`
- 核心：启动 fuxi → spawn 玄女 → 模拟用户多轮输入（直接调 API 不走 TTY）→ 轮询 EventBus → 比对 33 断言

---

## 6. 当前仓库状态（git 上 + 磁盘上）

### 6.1 分支
- `main`：13 commits + 昨晚到 `22a983e`
- **`feat/fuxi-v0.1`**：main 之后 7 个 commit，下面列
  - `fe3275b` 文档
  - `07a3c65` H + Skills
  - `8b994b9` B + C
  - `2175bfc` I
  - `37fa91a` F
  - `b605e66` HANDOFF

未 push 到远程。

### 6.2 新/改的关键文件

**fuxi-agent-cc** (crates/fuxi-agent-cc/src/)
- 新增：`ws_bridge.rs` (~350 行)
- 重写：`agent.rs`（stdio → WS 反连）
- 改：`config.rs`（加 sdk_url 字段，build_args 插入 --sdk-url + -p ""）
- 改：`lib.rs` 导出 WsChannel/WsError

**fuxi-cli** (crates/fuxi-cli/src/)
- 新增：`ipc.rs` / `daemon.rs` / `client.rs` / `subcommands.rs` / `skill_loader.rs`
- 改：`main.rs`（挂 8 个玄女工具子命令）
- 改：`up.rs`（fuxi up 现在同时起 orchestrator + Hub + daemon）
- `Cargo.toml`：加 axum/futures-util/serde/uuid/tempfile

**fuxi-core**
- `event.rs`：EventKind 加 `AgentInterrupted` / `TaskInterventionApplied` / `TaskResumed`；`UserInterventionSent` 加 `mode` 字段

**fuxi-events / fuxi-firehose**
- 同步 kind_tag + TUI format/color 表

**fuxi-orchestrator** (crates/fuxi-orchestrator/src/fuxi.rs)
- 新增方法：`intervene` / `block_task` / `resume_task`

**skills/** （新增目录）
- `xuannv/SKILL.md`：玄女 agentskills profile
- `dev/SKILL.md`：dev 门客 agentskills profile

**docs/**
- 新增：`references.md`（composio 映射表）
- 新增：`session-review-2026-04-19.md`（昨晚过程性）
- 新增：`session-review-2026-04-19-afternoon.md` ← 本文
- 新增：`superpowers/specs/2026-04-19-v0.1-scenario.md`（v0.1 北极星）
- 改：`architecture-decisions.md` / `superpowers/specs/2026-04-19-伏羲-design.md`（5s→30s 订正）

**CLAUDE.md**：常见陷阱加 6 条

**HANDOFF.md**：开头加"优先读这 3 份" + v0.1 薄片进度表

---

## 7. 给 D 接手人的开局 Checklist

0. [ ] 读 HANDOFF.md 开头 3 份必读 + CLAUDE.md 常见陷阱
1. [ ] `git log feat/fuxi-v0.1 --oneline -7` 看清时序
2. [ ] `cargo test --workspace` 确认 baseline 全绿
3. [ ] `cargo run -p fuxi-cli -- up --workspace-root /Users/e0_7/fuxi` 手动跑一次 daemon，`cargo run -p fuxi-cli -- ping` 验证（§4）——如果这个不通，**先修**
4. [ ] 读 `crates/fuxi-cli/src/watch.rs`（已有的 ratatui TUI）和 `crates/fuxi-firehose/src/tui.rs`（FirehoseApp）作 D 的 starting point 参考
5. [ ] 读 `crates/fuxi-cli/src/up.rs` 了解当前怎么起 Fuxi orchestrator + Hub + daemon——D 要复用大部分
6. [ ] 设计 D 的架构：三个 async task 互相喂（crossterm 输入 / broadcast 事件 / UI 渲染），同步通过 `tokio::sync::Mutex<TuiState>` 或 channel 汇到 TUI 状态
7. [ ] 先做最简骨架（单区对话 + 输入），跑通 "用户输入 → 玄女收到 → 她用 Bash 调 fuxi spawn → 事件到 TUI"——**这就是 v0.1 ship 门槛**
8. [ ] 跑一次真 claude E2E（开始 G）

---

## 8. 用户这次 session 表达的重要偏好（记忆级）

已写入 `~/.claude/projects/-Users-e0-7-fuxi/memory/` 的 memory 系统（那边永久），但重点复述：

- **毕设不是 ddl**——最终蓝图是。不要用毕设论证任何取舍
- **讨厌 ceremony**——不要 plan 文档、不要分节 approval、不要问工程决策
- **TUI 对他很重要**——没 TUI 他没法实操。这意味着 **D 是 v0.1 的真正 ship gate**
- **要被反驳**——他不懂的地方你该直接指出 + 给理由，不讨好
- **"没歧义就一直往后推吧"**——不要反复确认
- **工具要白嫖**——能派 agent 去读 transcript / clone 外部 repo 就别让他手动做

---

## 9. 我自己的反思（本 session 后半段用户点到的）

用户中途批评了 HANDOFF 只是结论性的，没起作用——这是实质的：

- HANDOFF 应该 **触发**下次 session 去读**更深的 process 文档**（本文这种）
- 我今天凌晨 session 写的 session-review 也是"只捞大点"，缺"**下次 claude 做 X 时要先知道 Y**"的行动性指引
- 本文 §5 和 §7 就是对这个的校准

**给下次 claude 的呼吁**：写 session-review 时不要堆结论——写**下个 claude 接手时会掉的坑、会问的问题、会做错的决定**。每一条都对应一个"下次你会被绊住"的场景，不是"我今天做了什么"。
