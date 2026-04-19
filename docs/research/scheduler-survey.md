# 伏羲定时 / 响应式触发——方案调研与设计

> 2026-04-19 · survey 输出，用于拍板伏羲的 trigger 子系统形态。
> 读者：明天早上的我 / 毕设答辩 reviewer。

## 一、问题空间

三类触发：

1. **时刻表**（cron）——"每周五 9 点 review 我的 PR"
2. **延迟**（one-shot）——"2 小时后提醒我 follow-up"
3. **响应式**（event / webhook）——"git push 到 main 自动触发"

约束：
- 伏羲是 Rust **本地 daemon**（`fuxi up`），已经有 EventBus + axum Hub + Unix socket。玄女是 cc headless（支持 `--session-id` / `--resume`）。
- 触发目标 = 唤醒玄女，让她**用自然语言读懂触发意图**，再 spawn 门客干活。
- 用户离线（屏幕未锁）时也要能触发、能收 macOS 通知。

## 二、候选方案定性

### 1. Claude Code 自带 `/loop` + `CronCreate/List/Delete`
- **本质**：cc **session 内**的 cron，scheduler 每秒 tick，task 在对话 turn 之间 fire。
- **致命伤**（对伏羲）：**session-scoped，只在 cc 进程跑时有效**，关掉终端就停。`--resume` 能恢复 7 天内的 recurring，但要求用户主动拉起会话。和"伏羲 daemon 主持一切"的架构正交。
- **能否外部触发**：不行。CronCreate 是 cc 内部工具，没有外部 RPC 入口。
- **判断**：**不用作承载层**，但可作为玄女的"短程 polling 辅助"（她在对话期间自己用 `/loop` 守一个短任务，完事就丢）。

### 2. Claude Code Routines（云端，2026-04-14 发）
- **本质**：Anthropic 托管，支持 schedule / API（HTTP POST `/fire`）/ GitHub webhook 三种触发。
- **优**：关机也跑。API trigger 给了外部 webhook 能力。
- **劣**：
  - **在云端**执行，和"本地文件系统、本地门客、本地 SQLite 单真相源"完全割裂。
  - 付费限流（Pro 5/天）。
  - 把伏羲对外部世界的观察权拱手让给 Anthropic。
- **判断**：**不用**。违反公理 5（SQLite 单一真相源）、公理 4（CLI 工具层）。

### 3. macOS `launchd` / LaunchAgent
- **本质**：plist 描述 job，`StartCalendarInterval` 做 cron、`StartInterval` 做 tick、`KeepAlive` 做保活、`WatchPaths` 做响应式。
- **优**：macOS 官方，cron 的正式继任者；**关机错过的 job 醒来会补跑一次**（coalesced）；`KeepAlive` 能让 daemon 崩了自动拉起。
- **劣**：
  - 配置 XML，不好动态增删（每次写 plist + `launchctl bootstrap`）。
  - 仅 macOS。Linux 要 systemd timer / cron 另写一套适配层。
  - 粒度只到"执行一条命令"，不回报结构化结果。
- **判断**：**用作 daemon 保活**（`fuxi up` 崩了自动重启），不用作业务触发器。

### 4. Linux systemd timer / cron
- **本质**：systemd `.timer` 单元 = launchd 对等物；cron 古董。
- **判断**：同上，Linux 上 daemon 保活用 systemd user service。业务触发不经它。

### 5. `tokio-cron-scheduler`（Rust，async）
- **本质**：tokio 原生，内部用 croner 解析表达式。支持秒级、NATS/Postgres 持久化、start/stop/removed 回调。
- **优**：和 fuxi 已有 tokio runtime 零摩擦；**进程内**，EventBus 无缝对接。
- **劣**：自带的持久化要拉 NATS/Postgres（伏羲不要这俩）；要自己写 SQLite adapter 或绕过持久化（reload from our table）。
- **判断**：**备选 A**。

### 6. `croner-rust`（底层 cron 解析 + DST 正确性）
- **本质**：只解析 + `next/prev tick`。不管怎么跑。
- **优**：DST / timezone 正确、OCPS 标准；最小依赖；只取你需要的那一层抽象。
- **劣**：要自己写 tokio 的 tick 循环。但 anya 就是这么用的（50 行搞定）。
- **判断**：**备选 B，首选**。anya 生产环境验证过，伏羲借思路即可（公理 6：借智慧不借路径，不过都是 Rust 这次可以直接 crate 复用）。

### 7. `job_scheduler` / `schedule-rs`
- **本质**：老牌，非 async。`schedule-rs` 是 human-readable DSL 包装。
- **判断**：**不用**。年久失修，不适合 tokio 世界。

### 8. 响应式类：文件 watch / git hook / HTTP webhook
- `notify` crate（inotify/FSEvents/kqueue 统一 API）→ 监视 worktree / 特定目录变动。
- git hooks（`post-commit` / `post-merge`）→ 脚本调 `fuxi trigger fire <id>`。
- axum 已有 Hub → 加 `/trigger/:id/fire` endpoint 吃 webhook。
- **判断**：**全都要**，都只是"喂入口"，统一归到 trigger table 同一条消费链。

### 对比表

| 方案 | 本地/云 | 持久化 | 粒度 | 外部可触发 | 伏羲适配度 | 用处 |
|---|---|---|---|---|---|---|
| cc `/loop` | 本地 session | 7 天 resume | 1 min | 否 | 低 | 玄女会话内短 polling |
| cc Routines | 云端 | Anthropic 托管 | 1 hour | 是 (HTTP) | 极低 | 不用 |
| launchd | 本地系统 | plist 文件 | 1 s | `launchctl start` | 中 | daemon 保活 |
| systemd timer | 本地系统 | unit 文件 | 1 s | `systemctl start` | 中 | Linux daemon 保活 |
| `tokio-cron-scheduler` | 进程内 | 可插 | 1 s | EventBus | 高 | 业务层备选 |
| `croner-rust` | 进程内 | 自管 | 1 s | EventBus | 高 | **业务层首选** |
| `notify` / webhook / git hook | 进程内/入口 | - | 事件 | HTTP/fs 事件 | 高 | 响应式入口 |

## 三、推荐方案

### 总体形态：**「更漏」子系统**

双层结构，**伏羲 daemon 自己做调度，OS 只负责让 daemon 活着**：

```
┌──── launchd (macOS) / systemd user (Linux) ────┐
│  KeepAlive=true：fuxi up 崩了就拉起               │
└─────────────────────────────────────────────────┘
                     │
         启动 & 保活 │
                     ▼
┌──── fuxi daemon (长跑) ──────────────────────────┐
│                                                  │
│  ┌─ 更漏 Keeper (crate: fuxi-scheduler) ──────┐  │
│  │  · croner 解析 + tokio::time::sleep_until │  │
│  │  · 下一 tick 到点 → 读 triggers 表 → 命中？ │  │
│  │  · 命中 → emit EventKind::TriggerFired      │  │
│  └───────────────────────────────────────────┘  │
│                    │                              │
│  ┌─ 候吏 Watcher （入口层） ──────────────────┐  │
│  │  · notify (fs 变动) → TriggerFired         │  │
│  │  · axum /trigger/:id/fire → TriggerFired    │  │
│  │  · Unix sock `fuxi trigger fire <id>` 同上 │  │
│  └───────────────────────────────────────────┘  │
│                    │                              │
│                    ▼                              │
│  EventBus ── TriggerFired ──▶ Orchestrator       │
│                    │                              │
│                    ▼                              │
│   spawn/唤醒玄女（cc headless，带触发 prompt）    │
│                    │                              │
│                    ▼                              │
│   玄女读 trigger.intent（自然语言）→ 判断 → 派门客 │
└──────────────────────────────────────────────────┘
```

命名：
- **更漏**（`fuxi-scheduler`）：古代计时工具，cron tick。
- **候吏**（watcher/入口）：守候驿站接邮件的小吏，管 webhook / 文件监视 / CLI 触发。
- **triggers** 表（不起雅号，DB schema 直白为上）。
- 事件变体：`TriggerRegistered` / `TriggerFired` / `TriggerSkipped` / `TriggerFailed`（和现有 EventKind 风格一致）。

（候选雅名"候时""待机"也 OK，但"更漏"最能直译 cron 的时刻表语义；响应式入口用"候吏"对仗工整。采不采用听你定，代码里 crate/module 用 `fuxi-scheduler` + `watcher` 英文名。）

### 关键决策

**(1) 承载层——fuxi daemon 常驻，不外部 launchd 喂 CLI**
launchd 喂 `fuxi trigger fire` 这条路我否决：每次冷启 daemon 会丢失世界模型在内存里的 warmup、errno、还有 cc session 的复用优势；且响应式触发（webhook/fs）本就必须驻留。launchd 只负责保活（`KeepAlive=true` + `RunAtLoad=true`）。Linux 同位用 systemd user service。

**(2) 表设计——SQLite 新表 `triggers` + `trigger_fires`**

借鉴 anya 的 `cron_jobs` + `cron_runs`（实战验证过），精简：

```sql
-- 触发器定义（cron / one-shot / fs / webhook 四合一）
CREATE TABLE triggers (
  id              TEXT PRIMARY KEY,           -- trg_<uuid7>
  name            TEXT NOT NULL,              -- 用户给的人类名
  kind            TEXT NOT NULL,              -- 'cron' | 'once' | 'fs' | 'webhook'
  spec            TEXT NOT NULL,              -- kind 对应的 JSON：
                                              --   cron: {expr:"0 9 * * 5", tz:"Asia/Shanghai"}
                                              --   once: {at:"2026-04-19T21:00:00+08:00"}
                                              --   fs:   {path:"...", events:["modify"]}
                                              --   webhook: {secret:"..."}
  intent          TEXT NOT NULL,              -- **自然语言**交给玄女的原句
  target_agent    TEXT NOT NULL DEFAULT 'xuannv',  -- 默认玄女；预留直派门客
  overlap_policy  TEXT NOT NULL DEFAULT 'skip',    -- 'skip'|'queue'|'allow'
  max_failures    INTEGER NOT NULL DEFAULT 5,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  enabled         INTEGER NOT NULL DEFAULT 1,
  created_at      TEXT NOT NULL,
  deleted_at      TEXT
);

-- 每次触发的运行记录（append-only，和 events 表风格一致）
CREATE TABLE trigger_fires (
  id           TEXT PRIMARY KEY,     -- fire_<uuid7>
  trigger_id   TEXT NOT NULL REFERENCES triggers(id),
  fired_at     TEXT NOT NULL,
  status       TEXT NOT NULL,        -- 'queued'|'dispatched'|'done'|'skipped'|'failed'
  task_id      TEXT,                 -- 派给了哪个 Task
  session_id   TEXT,                 -- 玄女承接的 session
  error        TEXT
);
```

**为什么 intent 是自然语言，不是 JSON/skill**：
- 公理 1：headless agent 不显式沟通 = 没做。把触发意图翻译成 JSON 再给玄女读回中文，丢失语境、引入二次翻译 bug。
- 玄女本就是语言模型，"每周五早 9 点 review 我的 PR" 原句送进 prompt，她自己能决定拉谁、怎么拉。反过来硬塞 `skill_name:"pr-review"` 既割语义又锁死可组合性。
- 未来确实想模板化时（重复意图太多），再往 intent 上加 `{{variables}}` 插值即可，不需要动 schema。

**(3) 玄女怎么"被唤醒"读 trigger**

关键：玄女不常驻运行（cc headless 空转很贵）。Orchestrator 收到 `TriggerFired` 后：

1. 查 session 表：玄女有没有「持久 session_id」？
   - 有 → `claude --resume <id> -p "<拼装 prompt>"` 复用。
   - 无 → `claude --session-id <new> -p "<拼装 prompt>"` 新起，把 session_id 落库。
2. 拼装 prompt 三段式（借鉴 anya 的 pipeline prompt 经验）：

```
[TRIGGER_FIRED]
- trigger_id: trg_xxx
- kind: cron
- fired_at: 2026-04-25 09:00:00 +0800
- 用户原意图:
"""
每周五早上 9 点，review 我这周 push 到 main 的 PR，有问题直接 at 我。
"""

[INSTRUCTION]
请按照你作为玄女的一贯方式处理这次触发。若需要干活，spawn 相应门客。
完事在 A2A 上发一条小结给用户（via UserNotification 事件）。
```

3. cc 拿到 prompt → 正常 agent loop → 该派门客派门客，该通知通知。

**(4) 离线触发 / 未启动时的通知**

- `TriggerFired` 入 SQLite（append-only）即"已触发"定案，哪怕玄女此刻 spawn 失败也不丢。
- 触发后若玄女正常产出给用户的反馈：
  - Hub WebSocket 在线 → 推给 TUI/Firehose。
  - 不在线 → 写 `user_notifications` 表 + 调 macOS `osascript -e 'display notification ...'`（或 `terminal-notifier`）。TUI 下次起来从表里回放未读。

**(5) 失败重试 / 并发去重 / 历史**

- **重试**：一次触发跑砸（cc spawn 失败 / 玄女 panic）→ `consecutive_failures += 1`；到 `max_failures` 自动熔断（enabled 不变，但 skip fire，留下 `trigger_fires.status='skipped'` + error="circuit_open"）。恢复要用户手动 `fuxi trigger reset <id>`。这是 anya 的 `max_failures=5` 经验，对"凌晨默默失败 10 次"有救。
- **去重**：`overlap_policy=skip` 默认，trigger 上一发还在 `dispatched` 状态就直接 skip（记一条 `status=skipped`）。响应式触发（fs/webhook）在 watcher 那一层还要加 debounce（`notify` 的 debouncer 或自写 500ms 合并），防抖动重复派单。
- **历史**：`trigger_fires` 表 + `events` 表里的 `TriggerFired` 双写（表给人查，事件给 Firehose 渲染），30 天外自动 rotate（沿用现有 events 表的 cursor 策略）。

### 和 EventBus / Hub 衔接

**新增 EventKind 变体**（在 `fuxi-core/src/event.rs`）：

```rust
// ── scheduling / triggers ────────────────
TriggerRegistered { trigger_id: String, kind: String, name: String },
TriggerFired      { trigger_id: String, fire_id: String, kind: String, intent_preview: String },
TriggerDispatched { fire_id: String, task: TaskId },
TriggerSkipped    { trigger_id: String, fire_id: String, reason: String },
TriggerFailed     { trigger_id: String, fire_id: String, error: String },
```

- Firehose 渲染加 case（公理约束：加变体必更 Firehose）。
- Hub 加 `GET /triggers`、`POST /triggers`、`DELETE /triggers/:id`、`POST /triggers/:id/fire`（手动触发）、`POST /webhooks/:trigger_id`（响应式入口）。前四个镜像到 CLI：`fuxi trigger list/add/rm/fire`。
- 玄女自己也有「给自己加 trigger」的工具——用户说"每周五 review PR"，她调 CLI `fuxi trigger add --cron "0 9 * * 5" --intent "..."`。符合公理 4（CLI 是工具层）。

### 实施建议（不是本调研 scope，但顺手列）

分三薄片：
1. **更漏 cron + 一次性**：croner + SQLite + 最小 `fuxi trigger add/list/rm`，端到端跑通「每周五 9 点唤醒玄女说一句话」。
2. **响应式入口**：webhook (axum) + fs watch (notify) 并入同一 fire 通路。
3. **熔断 / 去重 / macOS 通知**：稳定性补齐，加 launchd plist 给毕设 demo。

---

**结论**：业务层用 `croner` + 自写 tokio tick loop（进程内、SQLite 落库、EventBus 广播）；OS 层用 launchd/systemd 保 daemon 活。trigger 意图存自然语言直接喂玄女。不走 CC Routines 云端，不走 cc `/loop` session 内。

Sources:
- [Claude Code Scheduled Tasks (/loop, CronCreate/List/Delete)](https://code.claude.com/docs/en/scheduled-tasks)
- [Claude Code Routines (cloud, API/Schedule/GitHub triggers)](https://code.claude.com/docs/en/routines)
- [launchd tutorial (KeepAlive, StartCalendarInterval)](https://www.launchd.info/)
- [Apple launchd docs: Creating Launch Daemons and Agents](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)
- [cron is dead, long live launchd](https://blog.jan-ahrens.eu/2017/01/13/cron-is-dead-long-live-launchd.html)
- [croner-rust GitHub (DST, OCPS, timezone)](https://github.com/Hexagon/croner-rust)
- [tokio-cron-scheduler crates.io](https://crates.io/crates/tokio-cron-scheduler)
- [Letta scheduling docs (cron, one-off, silent mode)](https://docs.letta.com/guides/agents/scheduling/)
- [n8n Schedule Trigger node](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.scheduletrigger/)
- [n8n Webhook node](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook/)
- [Home Assistant automation triggers (state/event/calendar)](https://www.home-assistant.io/docs/automation/trigger/)
- anya 源码：`/Users/e0_7/team-anya/apps/server/src/bond/cron-scheduler.ts` + `db/schema/cron-jobs.ts` + `cron-runs.ts`
