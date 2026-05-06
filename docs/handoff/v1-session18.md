# Handoff · v1 · Session 18 → 19 开工指引

> 本 session（2026-05-07 凌晨）核心 = **task #8 玄女上下文管理**全链路 ship。
> 用户拍 §2.1 Q1=半自动B+C / Q2=≤500字 short / Q3=等 idle 再切。
>
> 上一份 handoff：`docs/handoff/v1-session17.md`（保留，§3 推荐起点指向 task #8）。
> 用户拍板节录在 `docs/handoff/v1-session16.md` §2.1 / §4。

---

## 1 · 本 session ship 了什么（HEAD 待 commit）

| 模块 | 内容 | 测试 |
|---|---|---|
| fuxi-core EventKind 三新变体 | `UsageReport` / `XuannvContextWatermark` / `XuannvHandoffWritten` + 5 处 kind_tag/summarize/color 同步 | 3 个 roundtrip 单测 |
| fuxi-agent-cc parser | 抓 cc result.usage 4 字段 + `UsageInfo` + 翻 `UsageReport` 事件（total = input+cache_creation+output 不算 cache_read）| 4 个新 parser 单测 |
| fuxi-agent-cc config | `resolve_default_window_size()`：`FUXI_CC_CONTEXT_WINDOW` env > 模型名启发式（含 `1m` → 1M / 含 `sonnet`/`haiku`/`opus` → 200k）> 兜底 1M | 2 个新 config 单测 |
| fuxi-orchestrator xuannv_context | 长跑 watcher：累加玄女 UsageReport，跨 35%/45% 触发 `intervene_system` 注入 `[CTX_*]` + emit `XuannvContextWatermark` 事件 | 4 个 watcher 单测（35%只触发一次/50%两都触发/spawn 周期重置/非玄女忽略）|
| fuxi-orchestrator Fuxi | 加 `shutdown_xuannv_for_handoff(id, reason)`——绕过玄女豁免，专给 handoff 路径用 | （走 shutdown_agent_inner 既有覆盖）|
| fuxi-cli xuannv_cmd | `fuxi xuannv handoff write '<≤2000字>'` + `read`；写 `~/.fuxi/xuannv-handoff.md` + emit `XuannvHandoffWritten` 事件 | （CLI smoke）|
| fuxi-cli xuannv_handoff | im 启动期装监听器：subscribe `XuannvHandoffWritten` → wait idle → kill 老玄女 → spawn 新副本（prepend handoff prelude 到 append_system_prompt）→ delete handoff 文件 → 注 `[CTX_HANDOFF_DONE]` 系统消息 | （e2e 难造，逻辑路径与 ensure_xuannv 重合，靠那边的覆盖兜底）|
| roles/xuannv | dispatch-routing.md 末尾加上下文水位 / handoff 必读段（35%/45% 触发后她要做什么 + 反模式）| - |

---

## 2 · 数据流速图

```
cc 子进程 result event ──parser→ Event{UsageReport{total,window,pct}}
                                   │
                                   ▼ EventBus broadcast
              ┌────────────────────┼────────────────────┐
              ▼                    ▼                    ▼
    Firehose / SQLite        xuannv_context        其他 subscriber
                              watcher（cli/im）
                              │
                              │ meta.agent == xuannv_id?
                              │ cumulative += total_tokens
                              ▼
                   ┌──── 跨 35% ───┐         ┌──── 跨 45% ────┐
                   │  emit Watermark         │ emit Watermark
                   │  intervene_system       │ intervene_system
                   │   "[CTX_ADDENDUM]..."   │  "[CTX_HANDOFF_OFFER]..."
                   └─────────────────────────┴─────────────────────┘
                                   │
                       玄女读到，走教学：长话短说 / 问用户
                                   │
                       用户回「换」→ 玄女 Bash:
                          fuxi xuannv handoff write '<md>'
                                   │
                       CLI 写 ~/.fuxi/xuannv-handoff.md
                       + emit XuannvHandoffWritten 事件
                                   │
                                   ▼
                       xuannv_handoff watcher（im）
                       1. wait idle（轮询 status_of，60s ceiling）
                       2. shutdown_xuannv_for_handoff（绕豁免）
                       3. spawn_with_prelude（prepend handoff body
                          到 append_system_prompt）
                       4. set_xuannv（触发 watch → ctx watcher 重置累加）
                       5. delete handoff 文件
                       6. intervene_system "[CTX_HANDOFF_DONE]" → 让新
                          玄女首句对用户说 "✻ 上下文已交接 · 新副本接班"
```

---

## 3 · 重要决策 / 为什么这样

### 3.1 公式 = input + cache_creation + output（**不**算 cache_read）

cache_read tokens 是命中 prefix cache 的复用——它们已在上一轮 cache_creation 里
计过了，再算就重复。这条公式让 turn-by-turn 累加 ≈ 当前 context window 总占用。
反回归保护在 fuxi-core::usage_report_tag_and_total_excludes_cache_read 单测。

### 3.2 35% addendum 走 system_origin intervene 而不是改 system prompt

cc spawn 后 `--append-system-prompt` 是 **immutable**——mid-session 改不了
（除非 kill+respawn）。但 35% 只是软提示「长话短说」，没必要付 cc 重启代价。
方案：通过 `intervene_system(origin="ctx_addendum")` 注一条系统消息——前端
PWA reducer 看到 `system_origin` 渲染成左侧灰底气泡而非右侧 user bubble，
玄女把它当作上下文一部分自动收紧。

45% handoff_offer 同理走 intervene。**只有真要交接（用户回「换」）**才走 kill+spawn。

### 3.3 spawn 周期重置累加

`xuannv_id_watch` 变化即重置 `WatcherState.cumulative_total / fired_thresholds`。
这意味着 handoff 完成后新副本从 0 开始，下次跨 35% 才再触发——避免新副本一上线
立刻又触发 addendum 的死循环。

### 3.4 shutdown_xuannv_for_handoff 绕豁免的口子

`Fuxi::shutdown_agent` 默认拒杀玄女（公理 #4——GC / 误 kill 不该误伤她）。但
handoff 是用户主动交接，新口子专门绕豁免，命名带 `_for_handoff` 让 grep 时一眼
知道**不要在别处调**。其他路径（GC / 测试 / CLI fuxi kill）继续走 shutdown_agent
的玄女豁免。

### 3.5 window_size 启发式

`FUXI_CC_CONTEXT_WINDOW` env 显式覆盖 > 模型名启发式（`1m` → 1M / `sonnet`/`haiku`/
`opus` → 200k）> 兜底 1M。用户主账号 = opus-4-7-1m，启发式正确；改成 200k 模型时
35% 阈值会更早触发（70k vs 350k），更保守不出错。

---

## 4 · CLAUDE.md 该追加什么

```markdown
- **加 EventKind 新变体必须同步 5 处**（之前的提示再强调）：
  `events/store.rs::kind_tag` + `firehose/hub.rs::kind_tag` +
  `firehose/tui.rs::summarize + color_for` + `cli/subcommands.rs::event_summary` +
  fuxi-core 加 roundtrip 测试。task #8 加 UsageReport/Watermark/HandoffWritten
  时 6 处都改了——下次加新变体优先 grep `kind_tag` 找全。
```

（已在本 handoff 文档列出，CLAUDE.md 顺手加 cli/subcommands.rs::event_summary 那一处。）

---

## 5 · 下 session 推荐起点

### 5.0 部署 + e2e 已验证（本 session 完成）

本 session 已 deploy home + 真测 3 轮接班全成功（启动期检查 + 30s polling tick）。
md5 = `5114c037804c2abc6d6c5617df073548`。日志关键路径：

```
fs poll 命中 handoff 文件落档
→ shutdown_xuannv_for_handoff: 用户主动交接，绕过豁免
→ spawn 新玄女副本（注入 handoff prelude）
→ 玄女接班完成 new=agent-...
→ 玄女 id 变化，重置上下文累加状态
```

handoff 文件检测 → kill → spawn → delete file 全跑通；ctx_watcher 新副本归零成功。

### 5.1 还可以做的真测

- **35% / 45% 真触发**：让玄女做几轮长 task 累到 ~350k tokens，看 [CTX_ADDENDUM]
  系统消息出不出来。需要真 cc 跑活，本 session 没造样本。
- **优化 wait_idle 60s ceiling**：见 §6。

### 5.2 真测验收（重复跑）

```bash
# 1. macOS 本地编 release + codesign（v1-session15 §4 坑）
cargo build --release --bin fuxi
codesign --force --sign - target/release/fuxi

# 2. 推 home + restart
scripts/deploy-home.sh   # 内部已 rsync + systemctl restart fuxi-im

# 3. 从 macOS 本地手动跑一次 handoff 写入：
#    （模拟玄女撞 45% 后跑的命令）
ssh home '/home/e0-7/.local/bin/fuxi xuannv handoff write \
"## 当前活跃 task
session 18 接班验证 task #8 上下文管理整路 e2e

## 待用户拍板
- 无

## 用户偏好
- TDD 必做、no-emoji TUI、no_ceremonies、keep_going"'

# 4. 验证：
#    a) ssh home 'pgrep -lf "fuxi im start"' → PID 应该变了（玄女 cc 子进程已 respawn）
#    b) ssh home 'cat ~/.fuxi/xuannv-handoff.md' → 应不存在（接班后被删）
#    c) PWA 玄女对话页：应看到一条「✻ 上下文已交接 · 新副本接班」系统消息
#    d) ssh home 'sqlite3 ~/.fuxi/events.db "SELECT kind_tag,payload FROM events
#       WHERE kind_tag IN (\"xuannv_handoff_written\",\"xuannv_context_watermark\")
#       ORDER BY rowid DESC LIMIT 5;"'
```

### 5.2 35% / 45% 真触发路径

人为推高玄女 cumulative：
```bash
# 玄女自身做几轮长 task（如让她 Read 长文件后总结）累到 ~350k tokens
# 然后看 PWA 应有 [CTX_ADDENDUM] 系统消息 + Watermark 事件落档
```

### 5.3 PWA 通知 tab 桥接（可选）

当前 watermark / handoff 事件只走 EventBus + intervene 注入玄女对话。如果想让
PWA「通知」tab 也亮红点提醒用户视角对齐，可加一个小 hook：fuxi-im 订阅
`XuannvContextWatermark{action="handoff_offer"}` → 写 NotificationStore 一条
`kind=context_handoff_offer`。**不阻塞 task #8**，下 session 顺手加。

---

## 6 · 已知差距 / 限制

- **35% addendum 的"下一 turn 起"严格性**：addendum 注入的是 system_origin 消息，
  cc 看完整 context 决定回复，没法保证她"下一 turn 才生效"——可能这一轮就开始收紧。
  实测够用就行，不强制时序。
- **handoff e2e 长 turn 等 idle 60s ceiling**：玄女单 turn 跑超 60s 时（罕见
  长复杂 task）会被强 kill。代价 = 那 turn 的回复丢失。可调 `IDLE_WAIT_CEILING_SECS`。
  **本 session 实测**：polling 路径 + 新 spawn 副本仍在跑 init turn 时，wait_idle
  会等满 60s ceiling 后强 kill 接班——结果正确但延迟大。下 session 可优化：
  排除"刚 spawn 起来 < N 秒"的 Busy 视为可立即 kill。
- **prelude 长度上限 2000 chars**：CLI 校验拒 > 2000；500 字中文 ≈ 500 chars
  足够。超出说明玄女写跑题。
- **未做：cumulative 持久化**。fuxi-im 重启后 `WatcherState.cumulative_total` 归零
  ——cc session 仍然继续（resume），但水位监控会等下一轮 UsageReport 再起算。这意
  味着重启正好夹在 40% 时不会立刻触发 addendum，要再过 35k tokens 才会。可接受。
- **跨进程 broadcast 不通**（本 session 实测踩过）：CLI `fuxi xuannv handoff write`
  直写 SQLite events.db，**不**经过 fuxi-im 进程内 EventBus broadcast——后端 watcher
  靠 30s fs polling tick 兜底检测落档（同时保留 bus.subscribe() 给同进程内事件用）。
  延迟 ≤ 30s 加 60s ceiling = 极端最坏 90s。下 session 可考虑用 `notify` crate
  inotify watch 文件实时检测，或者 daemon socket IPC 通知。

---

## 7 · 部署速记

- **systemd 重启**：`sudo systemctl restart fuxi-im`；偶发 EADDRINUSE 时 `sudo
  pkill -9 -f "fuxi im start"` 兜底
- **codesign**（v1-session15 §4 坑仍要注意）：`cargo build --release` 出来的
  binary cp 后必跑 `codesign --force --sign -`
- **新加 EventKind 同步 5 处**（v1-session17 §1 已说，本 session 改了 6 处含
  `cli/subcommands.rs::event_summary`——下次记得 grep `kind_tag` 看全）

### 部署状态快照（接班可直接验）

```bash
ssh home 'pgrep -lf "fuxi im start"; md5sum ~/.local/bin/fuxi'
# 期望：PID 在跑（systemd 维护），md5 待 deploy 后更新

# task #8 新增 CLI subcommand 验证
ssh home '/home/e0-7/.local/bin/fuxi xuannv handoff --help'
# 期望：列出 write / read 两个子命令

# 端到端：写一段 handoff
ssh home '/home/e0-7/.local/bin/fuxi xuannv handoff write "测试内容"'
ssh home 'ls -la ~/.fuxi/xuannv-handoff.md'   # 应存在
sleep 5
ssh home 'ls -la ~/.fuxi/xuannv-handoff.md'   # 应不存在（被 watcher 接班后删）
ssh home 'pgrep -fc "claude.*xuannv"'         # 玄女 cc 子进程 PID 应变了
```

PWA 强刷：DevTools → Application → Service Workers → Unregister → 刷新；或卸载重装 PWA。
