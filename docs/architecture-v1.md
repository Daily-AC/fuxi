# 伏羲 v1 架构蓝图 · M1 聚合

> **这份**是 R1-R5 五份 survey 聚合出的**开工契约**。C1-C5 编码 teammate 按此落地，不再回查 survey。
>
> 蓝图 = v1（取消 v0.1/v0.2 分法）。`feat/fuxi-v0.1` 分支最终合并目标改为 `feat/fuxi-v1`。

---

## 0 · 文化命名总表（先立住，全局一致）

| 英文 (crate / 表 / 变量) | 文化名 | 语义 |
|---|---|---|
| platform | 伏羲 | 画卦造字，秩序之源 |
| top orchestrator | 玄女 | 九天玄女，授兵策 |
| worker agent | 门客 | 战国四公子蓄士 |
| `fuxi-skills` crate / `~/.fuxi/skills/` | 点将台 | 玄女点将派门客 |
| `SKILL.md` bundle | 玉牒 | 身份谱系玉册 |
| `skills/<role>.staging/` | 榜文 | 招贤暂挂榜示众 |
| `skill_smith` role | 铸牒司 | 专职铸造玉牒的门客 |
| `hetu_patterns` 表 | 河图洛书 | 上古秘传图文，= 学到的模式/技能 |
| `oracle_facts` 表 | 甲骨 | 上古刻字，最早的"写下来" |
| `events` 表 (已有) | 简册 | 策简编联成册 |
| `fuxi-memory` crate | 策府 | 枢机之府，记忆总库 |
| `fuxi-scheduler` crate | 更漏 | 古之计时器 |
| `scheduler/watcher.rs` | 候吏 | 守候 trigger 的小吏 |
| `triggers` 表 | 候簿 | 候吏的值日簿 |
| `trigger_fires` 表 | 应期 | 到期应召记录 |
| role (dev) | 鲁班 `luban` | 工匠鼻祖 |
| role (pm) | 张良 `zhangliang` | 运筹帷幄 |
| role (research) | 仓颉 `cangjie` | 造字典藏 |
| role (test) | 皋陶 `gaoyao` | 司法断狱 |
| role (ops) | 造父 `zaofu` | 御马驾车 |
| role (comm) | 苏秦 `suqin` | 合纵外交 |
| role (skill smith) | 铸牒司 `zhudiesi` | 招贤生成玉牒 |
| ConversationSwitch | **让贤** | 贤人代言主对话权 |
| OrchestratorCcReceived | **呈报** | 门客给玄女的抄送 |

代码标识符走英文/拼音（agentskills `name` 字段规约：ASCII lowercase）。文档 / 注释 / UI 显示用中文名。

---

## 1 · 模块总览

```
crates/
├── fuxi-core/              ✓ 已有  基础 trait + Event + Task
├── fuxi-events/            ✓ 已有  EventBus (tokio broadcast + SQLite WAL)
├── fuxi-a2a/               ✓ 已有  A2A v1.0 subset
├── fuxi-workspace/         ✓ 已有  git worktree 隔离
├── fuxi-agent-cc/          ✓ 已有  cc 门客 (WS 反连，NO_PROXY 修了)
├── fuxi-agent-codex/       ✓ 已有
├── fuxi-orchestrator/      ✓ 已有  + v1 补课（抄送 / 让贤 / 死亡检测）
├── fuxi-firehose/          ✓ 已有
├── fuxi-skills/            ★ 新   点将台 (skill loader + 招贤流程)  ← 从 cli 挪 + 扩
├── fuxi-memory/            ★ 新   策府 (甲骨 / 河图 / BM25 检索)
├── fuxi-scheduler/         ★ 新   更漏 (cron + 响应式 trigger)
└── fuxi-cli/               ✓ 已有  + 三栏 TUI + 入装自动化
```

---

## 2 · 模块决策 · TDD 契约 · 落地步骤

### M1.1 · fuxi-memory（策府）

**决策**（来自 R1）：

- **三层分工**：
  1. 门客记忆 = cc `--resume <session-id>` （零成本白嫖，fuxi-agent-cc 已接口）
  2. 玄女长期记忆 = SQLite 表 `oracle_facts`（甲骨，key-value + subject/predicate 索引）
  3. 门客经验 = SQLite 表 `hetu_patterns`（河图洛书，pattern + outcome + confidence，可晋升为 `skills/*/SKILL.md` 中的 examples/）
- **简册（事件流）**：复用现有 `events` 表，不抄 anya 的 `execution-log.jsonl`
- **检索**：SQLite FTS5 （< 10k 记忆 p95 < 10ms），不上向量 / 图
- **写入策略**：只 ADD（mem0 思想）。删除要显式 API，不自动 overwrite
- **提取 pipeline**：对话结束后 async 跑 extractor → 抽 facts → 入甲骨。参考 anya `chat-memory-extractor.ts` 结构

**TDD 契约**：
- **单测**：`oracle::insert/query` / `hetu::record/promote` / FTS5 match 正确性（10k 样本假数据）
- **Gated E2E**：真跑玄女 3 轮对话，抽 fact → 第 4 轮 `--resume` 能引用前 3 轮。`FUXI_RUN_MEMORY_E2E=1`
- **先写测后写实装**

**落地步骤**：
1. 新建 `crates/fuxi-memory/` crate + Cargo.toml
2. SQL migration：`oracle_facts` / `hetu_patterns` / FTS5 索引
3. Trait `OracleStore` + `HetuStore`，impl 在 sqlx + SQLite
4. `fuxi-agent-cc` 加 `cli_session_id` 参数到 launch config，`--resume` 实装
5. 玄女 Skill 里 wire `Read @oracle/<subject>` 取 fact（bash tool 调 `fuxi memory query`）
6. `fuxi-cli` 加子命令 `fuxi memory {query,record,list}`

**衔接**：EventBus 订阅 → 门客任务结束后触发 extractor → 写甲骨 / 河图。

---

### M1.2 · fuxi-skills（点将台）· 招贤

**决策**（来自 R2）：

- **skill_loader 从 fuxi-cli 挪到 fuxi-skills 独立 crate**。frontmatter 升 `serde_yaml`（承载嵌套 metadata）
- **全局优先**：`~/.fuxi/skills/<role>/` 为权威位；项目 `./skills/` 覆盖（开发用）。查找顺序已对齐（$FUXI_SKILLS_DIR > git-root/skills > cwd/skills > $HOME/.fuxi/skills）
- **招贤流程反对"玄女自由写 SKILL.md"**（cursor 社区反复踩 frontmatter 丢失），改走：
  1. 玄女识别 `NoRoleMatched` → 发 `TriggerSkill` 事件 → 起 **铸牒司**门客（role=`zhudiesi`）
  2. 铸牒司按 `templates/` 的 archetype（`dev.archetype.md` / `pm.archetype.md` / ...）套模板填槽
  3. 生成的玉牒先入 `榜文区` (`skills/<role>.staging/`)
  4. 玄女通过薄片 F (`task_blocked/resumed`) 请用户审核（默认开）
  5. 审过 → rename 成 `skills/<role>/`，入**贤士录**（`~/.fuxi/ledger.json` append-only log）
- **单版本**：玉牒唯一，改动前 `.bak` 留档
- **宪法约束**：铸牒司必须走 A2A 门客形式，**禁止 orchestrator 内嵌函数调直接生成**

**新 EventKind 变体**（加时同步更 Firehose 渲染 + EventStore kind_tag）：
- `SkillStaged { role, template, path }`
- `SkillApproved { role }`
- `SkillRejected { role, reason }`
- `SkillActivated { role }`
- `NoRoleMatched { need }` （玄女发）

**TDD 契约**：
- 单测：skill 查找顺序 / frontmatter yaml 解析 / staging → active 的 rename 原子性
- Gated E2E：玄女说「招一个画图门客」→ 铸牒司写榜文 → 用户同意 → 新 role 可 spawn

**落地步骤**：
1. 挪 `fuxi-cli/src/skill_loader.rs` → `fuxi-skills` crate
2. 升级 frontmatter 解析到 `serde_yaml`
3. 建 `templates/` 骨架（dev/pm/research archetype）
4. 新 `skills/zhudiesi/SKILL.md`（铸牒司 soul + 工具清单）
5. 扩展 EventKind + Firehose 渲染
6. `fuxi-cli` 加 `fuxi skill {list,stage,approve,reject}` 子命令
7. 玄女 skill 加工具：`fuxi skill stage --template <type> --role <name> --brief "<desc>"`

---

### M1.3 · fuxi-scheduler（更漏）· 定时 / 响应式

**决策**（来自 R3）：

- **承载**：`fuxi up` daemon 进程内（不拆独立 binary）。daemon 靠 launchd / systemd 保活
- **调度核心**：`croner` crate（Rust cron 解析）+ 自写 tokio tick loop（1s tick 精度，anya 生产验证）
- **统一表**：`triggers`（kind + spec JSON 四合一：cron / once / fs / webhook）+ `trigger_fires`（append-only 执行历史）
- **玄女唤醒机制**：
  - `triggers.intent` 存自然语言原句（不 JSON，不 skill）
  - 到期：claude `--session-id=<persisted> --resume` 复用玄女持久 session
  - 三段式 prompt：`[TRIGGER_FIRED id=X fired_at=...]\n<用户原意图>\n[INSTRUCTION: 判断是否执行 + 汇报]`
- **响应式入口**：webhook（HTTP POST 到 `/hook/<id>`） + fs watch（notify crate）+ `fuxi trigger fire <id>` → 统一 `TriggerFired` 事件
- **熔断**：抄 anya—`consecutive_failures >= max_failures(5)` 自动 pause 该 trigger
- **不用**：cc `CronCreate`（session-scoped 关会话就停）、cc Routines（云端违反本地 SQLite 单真相源公理）
- **边界**：cc `/loop`（7 天 resume） = **玄女手中的小刀**（短程守望如「盯 CI 15 分钟」），不是伏羲 trigger 的替代

**新 EventKind 变体**：
- `TriggerRegistered { id, kind, spec }`
- `TriggerFired { id, fired_at, cause }`
- `TriggerDispatched { id, to_agent }`
- `TriggerSkipped { id, reason }`（去重 / 熔断）
- `TriggerFailed { id, error }`

**TDD 契约**：
- 单测：cron 表达式解析 / spec JSON 往返 / 熔断计数
- 集成：mock tick 喂假时间 → 触发器按预期 fire
- Gated E2E：`FUXI_RUN_SCHED_E2E=1` 注册 `*/2 * * * * *` trigger → 5s 后看到 2 次 `TriggerFired` 事件

**落地步骤**：
1. 新 crate `fuxi-scheduler/` + Cargo.toml
2. SQL migration `triggers` + `trigger_fires`
3. `WatcherLoop` struct + tokio tick task
4. 响应式入口：axum 路由 `/hook/<id>` 复用 Firehose Hub 的 listener
5. 扩展 EventKind + Firehose 渲染
6. 玄女 skill 加工具：`fuxi cron {add,list,remove,fire}`
7. `fuxi-cli` / daemon 启动时自动 load 已注册 triggers

---

### M1.4 · 三栏 TUI（repl.rs 重构）· **Fix-D override**

> ⚠️ **2026-04-20 override · Fix-D**：C2 发布后用户实测列了 12 条 UX 问题。本节按 Fix-D
> 实装结果重写；旧 C2 版决策（roster 为扁平门客列表 / 自实现单行 input）作为决策 log 保留在
> 段末，方便追溯。决策背景详见 `docs/decisions/03-tui-task-tree-override.md`。

**决策（Fix-D 版）**：

- **任务树**（核心变更）：左栏不再是"扁平 agent 列表"——改为按 **task** 分组的树：
  ```
  🟢 玄女 · 总控               ← 持久顶部
  📁 <task 1 title>  🔵 鲁班   ← task 挂负责门客
  📁 <task 2 title>  🔵 造父
  ─ 空闲门客 ─
  🟢 <role>  <name>           ← 还没派活的门客
  ```
  模型：
  ```rust
  struct TaskNode {
      task_id: TaskId, title, description, state: TaskState,
      worker: AgentId, worker_role: String,
      dispatched_at: Instant, prune_after: Option<Instant>,  // Done/Cancelled 后 5s 后 prune
      thinking: bool, worktree: Option<PathBuf>,
      recent_tools: VecDeque<String>,  // 右栏"最近工具调用"
  }
  struct ReplApp {
      xuannv_id, xuannv_status, xuannv_thinking,
      active: ActiveTarget,  // Xuannv | Worker(AgentId)
      focus: Focus,          // Roster | Input（events 折叠靠 F2，不抢焦点）
      dialogues: HashMap<ActiveTarget, VecDeque<DialogueLine>>,
      tasks: Vec<TaskNode>, idle_workers: Vec<RosterRow>,
      events: FirehoseApp, events_visible: bool,
      input: tui_textarea::TextArea<'static>,  // 见下
      dialogue_scroll: u16, dialogue_auto_scroll: bool,
      prune_delay: Duration,
  }
  ```
- **布局**：`Layout::horizontal([Length(28), Min(40), Length(30)])` → 中栏
  `Vertical(dialogue=Min(5) + input=Length(5) + status=Length(1))`
- **事件 → 任务树**（在 `ReplApp::ingest` 维护）：
  - `TaskDispatched { to }` + `meta.task` → `upsert_task`；若该 worker 在 idle 桶，搬到 tasks
  - `TaskCreated { title, description }` → 找/建 task，写 title/desc
  - `TaskStateChanged { to: Done|Cancelled }` / `TaskDelivered` / `TaskCancelled` → 置 `prune_after = now + 5s`
  - `AgentReady` / `AgentSpawning` → idle_workers 入桶
  - `AgentDead` → 清空闲桶；活跃 task 置 prune；active 若指向它，`tick` 里自动回 Xuannv
  - `ToolCallStarted` → 挂到对应 task 的 `recent_tools`（右栏展示）
  - `ConversationHandoffRequested { to }` → 主对话权切到 `to`
  - `tick(now)` 每帧前调一次：扫 `prune_after`，过期则清 task（worker 回 idle）
- **输入框** = **`tui-textarea`**（community widget，ratatui 0.29 兼容）
  - 支持 multi-line：`Shift+Enter` 换行 / `Enter` 提交 / 光标移动 / `Ctrl+W` 删词 / 全选等
  - 粘贴：启用 crossterm `EnableBracketedPaste`，`Event::Paste(s)` 走 `app.handle_paste(&s)` 直接塞 textarea
- **Key routing**：全局（`Ctrl-C` / `Tab` / `Esc` / `F2` / `PageUp`|`PageDown` / `Home`|`End` when input 空）先拦 → 其余按 focus 分派
  - `Tab`：循环 Xuannv → 每个 task 的 worker → 每个 idle → Xuannv（跳过 IdleHeader）
  - `Esc`：速切回 Xuannv
  - `PageUp` / `PageDown`：对话区翻页，PgUp 冻结 `dialogue_auto_scroll`；到底部自动重贴底
  - `End` / `Home`（输入空时）：回贴底 / 跳顶
- **对话区渲染**：
  - 连续同 speaker 消息折叠前缀（`render_dialogue_collapsed`）——第一行显示 `玄女> `，后续行用等宽空白缩进
  - 滚动条用 `Paragraph::scroll((offset, 0))` + 自算 `last_dialogue_total/view`
- **事件面板**（F2 展开）：过滤噪声
  - 默认隐藏 `custom { label: "cc_system_*" | "cc_thinking_delta" | "rate_limit" | "cc_raw" }`、`thinking_started/finished`、`user_prompted`、`agent_responded`（这些已在对话区呈现）
  - 其余按 `[time] [who] [kind_tag] [summary]` 四列渲染
- **右栏 meta** · **任务级**：
  - Xuannv active：总控视图（agent/status/active task/tasks count/idle count）
  - Worker active 且挂 task：task title / desc / worker / role / state / elapsed / worktree / 最近工具调用
  - Worker active 且空闲：空闲门客视图（role / status / 等玄女派活）
- **CJK 宽度**：所有左栏/右栏/事件流截断用 `unicode-width::UnicodeWidthStr::width` 算 displayed width，不用 `str.len()` 或 `chars().count()`
- **shelf 加** `worktree_of(agent_id) -> Option<PathBuf>` 只读方法（已随 C2 进仓）
- **mouse**：v1 不上（纯键盘）；v2 考虑 hit-test

**TDD 契约**（Fix-D 落地时实际写了 24 个单测，含以下 hard 断言）：
- 任务树：`task_dispatched_event_appends_task_node` / `task_done_prunes_after_delay` /
  `idle_worker_shows_in_idle_bucket` / `tab_cycles_xuannv_tasks_idle_order` /
  `agent_dead_event_marks_tasks_for_prune`
- tui-textarea：`tui_textarea_enter_submits_shift_enter_newlines` /
  `bracketed_paste_fills_input` / `backspace_deletes_last_char`（迁移到 textarea 后仍保持）
- 滚动：`scroll_up_breaks_auto_scroll_then_end_resumes`
- 多行折叠：`consecutive_same_speaker_collapses_prefix` / `different_speakers_keep_prefix`
- CJK 宽度：`truncate_by_width_handles_cjk`
- 事件流噪声：`noise_filter_hides_low_value_events`
- Snapshot：`three_pane_snapshot_contains_expected_widgets`（断言左栏标题「任务」、"空闲门客" header、玄女字样、对话内容）

**落地**：单 crate 改动（`fuxi-cli/src/repl.rs` + `Cargo.toml`）；新增依赖 `tui-textarea = "0.7"` + `unicode-width = "0.2"`；无其它 crate 变动。

---

<details>
<summary>C2 旧决策 log（2026-04-19，被 Fix-D 替换，保留备忘）</summary>

- 扁平 `Vec<RosterRow>` 当左栏，`task_meta: HashMap<AgentId, TaskMeta>` 补元信息
- 单行 `String` 输入，自实现 Backspace/Char；Enter 直接 submit；无多行、无粘贴
- 事件面板不过滤噪声；`user_prompted` / `cc_system_*` 大量刷屏
- 对话区 `Paragraph::scroll` 用自算 offset 紧贴底，无滚上看历史的交互
- 左栏标题「门客」

实测暴露的 12 条 UX 问题（Fix-D 已解）：
1. 左栏应是任务树不是扁平 agent 列表
2. 单行输入太局促，IME 长粘贴丢字
3. 没有 bracketed paste
4. 对话超出可视区看不到历史
5. 同 speaker 连续消息重复前缀刷屏
6. 事件流信息密度极低
7. CJK 按 `str.len()` 算宽度错位
8. 某些 terminal 输入不聚焦 / IME 丢首字
9. 左栏标题应是「任务」
10. 右栏应展示任务级元信息，不是 agent 级
11. 本节文字与实装脱节
12. 原 13 个单测多数失效，需要 TDD 重写

</details>

---

### M1.5 · Orchestrator 补课（抄送 / 让贤 / 死亡检测）

**决策**（来自 R5）：

**1. 抄送（呈报）实装** —— `InterventionProxy` 从"设计中"到落地：
- 用户对门客直接说话时（TUI 切 active=门客 + Enter），玄女收副本
- Wire format：
  ```rust
  EventKind::OrchestratorCcReceived {
      from_user_to: AgentId,  // 门客 id
      text: String,
      original_intervention_id: Uuid,  // 关联原 UserInterventionSent
  }
  ```
- 发送路径：`Fuxi::intervene(target, false, text)` 之后**自动**再发一条 `OrchestratorCcReceived` 给玄女 id（玄女订阅事件流就能看到）
- 玄女对抄送**有知情权无否决权**（公理 #2），她可以后续主动 intervene 调整，但不阻塞

**2. 让贤（ConversationSwitch）实装** —— 主对话权转交：
- 参考 langgraph `Command(goto, payload)`
- Wire：
  ```rust
  EventKind::ConversationHandoffRequested {
      from: AgentId,      // 原主对话对象
      to: AgentId,        // 新主对话对象
      reason: String,
      brief: Option<String>,  // 简报 / 交接上下文
  }
  EventKind::ConversationReturned {  // 已存在，复用
      from: AgentId, to: AgentId, brief: Option<String>,
  }
  ```
- TUI 订阅 `ConversationHandoffRequested` 自动切 active
- 触发场景：鲁班干活时 PM (张良) 接管需求澄清，张良 resolve 后让贤回鲁班

**3. 门客死亡检测** —— 失败恢复空白补齐：
- `CcAgent` 加 health check task：每 2s 看 child pid 是否还活（`child.try_wait()`）+ WS 连接是否断
- 死掉后：发 `AgentDead { cause }` 事件，orchestrator 把 shelf 条目移除或 `ShelfStatus::Dead`
- 玄女订阅 `AgentDead`，如当前任务未完成 → 她决定是否重派给别的门客
- 抄 sia ForkManager 的 TTL + anya `cc_status` 表的简化版（存内存即可，暂不持久化）

**明确延后 v2**：门客间 A2A 直通（中心辐射是设计选择，不是缺陷；autogen 网状拓扑会让玄女失去知情权，违反公理 #2）。

**TDD 契约**：
- 单测：抄送事件发出 / 让贤状态切换 / 死亡检测触发 AgentDead
- 集成：
  - 三体场景：用户 → 门客A → 抄送玄女 → 玄女看到 ≤ 1s
  - 让贤：玄女 → 张良接管 → resolve → 让贤回玄女，主对话 target 跟着切
  - Kill 模拟：手动 `kill -9` 一个 cc 门客 → `AgentDead` 事件 ≤ 3s

---

## 3 · 编码并行 C1-C5 契约

每个 C 独立 git worktree 分支，先写测试，再写实现，绿门禁 + gated E2E（必要时）才算完。

| # | 分支 | 负责 | 依赖 | 预估 |
|---|---|---|---|---|
| C1 | `feat/fuxi-install-soul` | `cargo install --path` 自动化 + soul-first skill 重写（玄女 + 鲁班两个全重写） | 无 | 0.5d |
| C2 | `feat/fuxi-tui-3pane` | 三栏 TUI（M1.4） + Shelf::worktree_of + orchestrator 补课 wire format（M1.5 的 EventKind） | 无（M1.5 的 EventKind 和 TUI 一起开在同分支） | 2d |
| C3 | `feat/fuxi-memory` | fuxi-memory crate + migration + cc --resume 接口 + `fuxi memory` CLI | 无 | 2d |
| C4 | `feat/fuxi-skills-zhaoxian` | fuxi-skills crate 挪出 + 铸牒司 skill + staging/approval 流程 | 等 C3（`fuxi-memory` crate 起来后跟 skill ledger 合并），但 skill 挪 crate 可先做 | 1.5d |
| C5 | `feat/fuxi-scheduler` | fuxi-scheduler crate + `triggers` 表 + webhook endpoint + `fuxi cron` CLI | 无 | 2d |

**中聚合**：
- M2 = C1 + C2（都改 cli + orchestrator，冲突高，先合）
- M3 = C3 + C4 + C5（新 crate 为主，冲突低）

**顶聚合**：
- T1 = M2 merge M3 → `feat/fuxi-v1`
- T2 = 独立 reviewer cc 对照本文档验收 + fix bugs

---

## 4 · 不变公理（跨所有 C 团队）

1. **Headless agent 不显式沟通 = 没做**（CLAUDE.md 公理 #1）
2. **玄女永远有知情权，无否决权**（抄送不得绕过）
3. **真实时，不轮询**（订阅 EventBus，不 poll）
4. **CLI 是工具层唯一形态**（公理 #4，不上 MCP）
5. **SQLite 单真相源**（WAL + append-only）
6. **借智慧不借代码**（语言隔离，Rust 重写，参考物注释明写文件+行号）

**加 EventKind 变体时必须同步更新**：
- `fuxi-events/src/store.rs::kind_tag`
- `fuxi-firehose/src/hub.rs::kind_tag`
- `fuxi-firehose/src/tui.rs::summarize` + `color_for`
- 相关持久化测试

---

## 5 · soul-first SKILL.md 结构规范

所有玉牒按此结构（R2 精神）：

```
skills/<role>/
├── SKILL.md              # 入口。frontmatter（name/description/allowed-tools/metadata） + body
│                         # body 顶部就是 **soul**：愿景 / 使命 / 价值观（3-5 段）
├── instructions/
│   ├── tool-map.md       # 具体工具怎么用
│   ├── how-to-work.md    # 工作流 / 流程
│   └── quality-bar.md    # 质量标准
├── resources/            # 参考资料（伏羲公理 / 项目规约）
└── examples/             # 历史范例（河图洛书晋升上来的）
```

**soul 必须回答 3 个问题**：我是谁 / 我为何存在 / 我的价值观是什么。对伏羲平台里每个门客都成立。

---

## 6 · Milestone 收敛判据

当以下全部满足，伏羲 v1 算 ship：

- [ ] 玄女真跑，用户问问题真对话，**玄女的 Bash 工具能调 `fuxi ...`**（PATH 通）
- [ ] 用户 TUI 中 Tab 切到鲁班，输入「写个排序」→ 鲁班真写 + 回话 + 右栏元信息实时
- [ ] 长期记忆：关 fuxi 再开，玄女记得上次约定过的偏好
- [ ] 定时任务：`fuxi cron add "*/5 * * * *" "..."` → 到点真触发
- [ ] 招贤：玄女识别无 role → 铸牒司起草 → 用户审 → 新 role 可用
- [ ] 死亡恢复：`kill -9` 一个门客 → 玄女知情 → 自然语言告知用户
- [ ] 场景 §1 33 个事件断言全到 SQLite

---

**这份文档落地后**：M1 阶段结束，启动 C1-C5。
