# 伏羲 REPL TUI 三栏化 —— 实装方案

> 目的：把 v0.1 的单区 REPL（`crates/fuxi-cli/src/repl.rs`）升级成「左门客列表 / 中对话 / 右元信息」的三栏界面，让用户可以在玄女与门客之间直接切换主对话对象。
> 前置材料：现有 `ReplApp`、`FirehoseApp` 已给出 ingest + draw 的架子；事件口、`Shelf::list_cards`、`ShelfStatus` 都能直接复用。

---

## 一、参考实现定性（按"能借什么"排序）

| 项目 | 语言/栈 | 值得借鉴的点 | 对伏羲的映射 |
|---|---|---|---|
| **gitui** (`gitui-org/gitui`) | Rust + ratatui | 每个 panel 是独立 `Component`，顶层维护 `focus: Focus` 枚举；键先给聚焦组件消费，不吃再冒泡到全局热键层 | 我们抄这套：`Focus::{Roster, Input, Events}`，Tab 循环，全局键（Ctrl-C / Esc / `/`）在分发前拦截 |
| **lazygit** (`jesseduffield/lazygit`) | Go | "当前 panel" 有明显高亮边框；切 panel 不丢输入缓冲；每 panel 有独立 keymap hint 条 | 切"主对话对象"时 **不清空输入框**；状态条根据 focus 动态换文案 |
| **k9s** | Go | 左导航树选择 → 右主区换视图；选中行用反色 + 边框 accent | 左栏门客列表用 `List + ListState`，选中行 accent 色，按 Enter 把"主对话对象"切过去 |
| **zellij** | Rust | split pane 的 `Layout::horizontal([Length, Min, Length])` 三段式 + 各自 Block border | 直接抄布局骨架；左/右用 `Length`，中间 `Min(40)` 自适应 |
| **bottom** (`ClementTsang/bottom`) | Rust + crossterm | mouse click → hit-test 回落到 widget 坐标 → 发 "focus X" 事件；多 widget 独立 `on_click(x,y)` | v1 **defer 鼠标**（键盘优先）；Rect 存 `self.last_rects` 里，v2 直接接就行 |
| **helix** | Rust | 编辑器 key dispatch：`Mode × KeyCode → Command`；明确的 modal boundary | 我们不做完整 modal，但 `handle_key` 第一步按 `focus` 分派，思路一致 |
| **tui-textarea** (rhysd) | ratatui crate | 现成多行文本 widget，支持光标 / 剪切板 / 选区 | v1 **不引入**——当前只需单行；换行、粘贴多行是 v0.3 才谈 |

结论：主骨架照 gitui + zellij，选择交互照 k9s，鼠标与多行留白。

---

## 二、推荐架构

### 2.1 布局骨架

```
┌─ roster (26 cols) ──┬─ center ──────────────┬─ meta (28 cols) ─┐
│ 🟢 玄女   xuannv    │  dialogue (Min(5))    │ active: T001     │
│   └ 闲置            │                       │ status: Busy     │
│ 🔵 鲁班#1 coder     │                       │ worktree: ...    │
│   └ T001 Busy       │                       │ pid: 48123       │
│ ──────── events ─── ├───────────────────────┤                  │
│ (折叠，F2 切展开)   │  input  (Length(3))   │                  │
└─────────────────────┴───────────────────────┴──────────────────┘
   status bar (1) · Tab/Esc/Enter/↑↓/Ctrl-C hints
```

代码骨架（draw 入口）：

```rust
let root = Layout::horizontal([
    Constraint::Length(26),   // 左栏
    Constraint::Min(40),      // 中栏
    Constraint::Length(28),   // 右栏
]).split(frame.area());

// 左栏：roster + 折叠式 events
let left = Layout::vertical([
    Constraint::Min(8),
    Constraint::Length(if self.events_open { 10 } else { 0 }),
]).split(root[0]);

// 中栏：dialogue + input
let center = Layout::vertical([
    Constraint::Min(5),
    Constraint::Length(3),
    Constraint::Length(1),  // status bar
]).split(root[1]);

self.draw_roster (f, left[0]);
self.draw_events (f, left[1]);   // height==0 时自动不画
self.draw_dialogue(f, center[0]);
self.draw_input  (f, center[1]);
self.draw_status (f, center[2]);
self.draw_meta   (f, root[2]);
```

### 2.2 State 重构

```rust
enum Focus { Roster, Input }   // v1 只两态；events 折叠靠 F2，不抢焦点

/// 主对话对象——对谁打字、右栏显示谁
enum ActiveTarget {
    Xuannv,
    Worker(AgentId),
}

struct ReplApp {
    // ── 已有
    xuannv_id: AgentId,
    input: String,
    should_quit: bool,
    confirm_quit: bool,
    events: FirehoseApp,           // 复用：ingest + visible_rows

    // ── 新增
    shelf: Arc<Shelf>,             // 从 Fuxi.clone_shelf() 拿
    focus: Focus,
    active: ActiveTarget,
    roster: Vec<RosterRow>,        // 每帧开头 refresh
    roster_state: ListState,
    events_open: bool,

    /// 按对话对象分桶的 dialogue——v0.1 把所有人塞一条，现在必须分开
    dialogues: HashMap<ActiveTarget, VecDeque<DialogueLine>>,

    /// 每个 worker 的当前任务、开始时间，用来画右栏
    task_meta: HashMap<AgentId, WorkerMeta>,
}

struct RosterRow { id: AgentId, role: String, label: String, status: ShelfStatus, active_task: Option<TaskId> }
struct WorkerMeta { task_id: TaskId, title: String, started_at: Instant, worktree: Option<PathBuf> }
```

`refresh_roster` 每帧调一次（轻量，异步读 shelf）：

```rust
async fn refresh_roster(&mut self) {
    let cards = self.shelf.list_cards().await;
    self.roster = cards.into_iter().map(|c| {
        let status = self.shelf.status_of(c.id).await.unwrap_or(ShelfStatus::Dead);
        // 玄女排最前；其它按 role 再按 spawn 顺序
        RosterRow { id: c.id, role: c.profile.role.clone(), /* ... */ }
    }).collect();
}
```

> 为避免每帧 await，实际把 `refresh_roster` 放到 select! 的事件分支里：每次 `AgentSpawning/AgentDead/TaskStateChanged/TaskDispatched` 才刷新。其余事件不触发。

### 2.3 Key Routing（分两层）

**Layer 1 全局键**（任何 focus 都先看）：

| 键 | 动作 |
|---|---|
| `Ctrl-C` ×2 | 退出（保留现有两段式） |
| `Tab` | `focus = next(focus)`；在 Roster / Input 之间循环 |
| `Esc` | 快捷切回玄女：`active = Xuannv`，`focus = Input` |
| `F2` | 折叠/展开左下事件流 |
| `Ctrl-L` | 清屏当前 dialogue 桶（开发期调试用，生产可去掉） |

**Layer 2 focus 局部键**：

- `Focus::Roster`：
  - `↑/↓` 或 `k/j`：移动 `roster_state.selected`
  - `Enter`：`active = Worker(roster[sel].id)`，`focus = Input`
  - 任何可打印字符：**自动转 Input**（忍让式），首字符不丢
- `Focus::Input`：
  - `Enter`：提交（下同 §2.4）
  - `Backspace` / `Char`：沿用现逻辑
  - `↑/↓`：v1 **不接**；v0.2 再做"历史输入翻页"

> events 目前的 `/` 过滤快捷键在三栏版暂时让位——事件流折叠在左下，`/` 在 Input focus 下是普通字符。若要保留可改成 `Alt-/` 专用。

### 2.4 提交动作

提交路径和现在基本一样，只是**分对象 dispatch**：

```rust
match self.active {
    ActiveTarget::Xuannv => fuxi.dispatch(self.xuannv_id, Task::new("user-turn", &text)).await,
    ActiveTarget::Worker(id) => {
        // 这是"介入"——不是新 task，是给正在跑的门客追加 prompt
        fuxi.intervene(id, InterventionMode::Queue, &text).await
        // 若 orchestrator 没有这个接口（当前只有 block/resume），退化为
        //   走 daemon socket 的 `fuxi intervene <agent> ...`，等价于玄女 Bash 调用
    }
}
```

对门客说话 = `UserInterventionSent` 事件（薄片 I 已有）。TUI ingest 时把它渲染到 `Worker(id)` 的 dialogue 桶。

### 2.5 Dialogue 分桶

```rust
fn target_of(&self, ev: &Event) -> Option<ActiveTarget> {
    match &ev.kind {
        EventKind::UserPrompted { .. } => Some(self.active),  // 用户说的挂到"当前主对象"
        EventKind::AgentResponded { .. } |
        EventKind::ThinkingStarted |
        EventKind::ThinkingFinished |
        EventKind::AgentDead { .. } => {
            ev.meta.agent.map(|a| if a == self.xuannv_id { ActiveTarget::Xuannv } else { ActiveTarget::Worker(a) })
        }
        EventKind::UserInterventionSent { target, .. } => Some(ActiveTarget::Worker(*target)),
        _ => None,   // 其余事件只进 events 面板
    }
}
```

### 2.6 右栏 Meta 渲染

```rust
fn draw_meta(&self, f, area) {
    let para = match self.active {
        ActiveTarget::Xuannv => xuannv_meta_lines(&self.shelf, self.xuannv_id),
        ActiveTarget::Worker(id) => {
            let m = self.task_meta.get(&id);
            vec![
                Line::from(format!("agent   {}", short_id(id))),
                Line::from(format!("role    {}", ...)),
                Line::from(format!("status  {:?}", ...)),
                Line::from(format!("task    {}", m.map(|x|&*x.title).unwrap_or("-"))),
                Line::from(format!("elapsed {}", m.map(|x| humanize(x.started_at.elapsed())).unwrap_or("-".into()))),
                Line::from(format!("worktree {}", m.and_then(|x|x.worktree.as_ref()).map(|p| p.display().to_string()).unwrap_or("-".into()))),
            ]
        }
    };
    f.render_widget(Paragraph::new(para).block(Block::default().borders(Borders::ALL).title(" 元信息 ")), area);
}
```

`task_meta` 由 `ingest` 从 `TaskDispatched / TaskStateChanged / TaskDelivered` 里维护；`worktree` 从 `ShelfEntry.worktree` 取（需要在 `Shelf` 加一个 `worktree_of(id) -> Option<PathBuf>` 的只读方法，不破原子性）。

### 2.7 鼠标 & 输入 widget 决策

- **鼠标**：v1 **不启用** `EnableMouseCapture`。原因：键盘流在开发闭环里更快；鼠标要正确落点 hit-test 需要缓存每个 panel 的 `Rect`，目前 state 还在成形，先稳住。v0.2 再加，代价只是一个 `MouseEvent` 分派层。
- **输入 widget**：v1 **自实现**（即保留当前 `String` + Backspace/Char）。理由：
  1. 当前场景是单行提交，tui-textarea 的多行/剪切板/选区都用不上
  2. 它的 `Widget` 渲染是 stateful，和我们的 dialogue-scroll 模型不冲但增加心智
  3. 依赖早加晚加无区别——v0.3 要跑 `/edit <file>` 之类 REPL 指令再换

---

## 三、和 FirehoseApp 的关系

- `FirehoseApp` **保留**，但在 ReplApp 里它只剩两个用法：`ingest(&ev)` 和 `visible_rows()`。现有事件流面板原样复用。
- 它自己的 `handle_key` 在 REPL 里**完全不调用**（当前代码已经绕开了它）。`/` 过滤在 v1 三栏下暂不暴露；若需要，做法是 F2 展开 events 面板后再额外引一个 `Focus::Events` 态（v1.5）。
- `fuxi watch` 子命令仍然直接裸跑 `FirehoseApp`，不受影响。

---

## 四、落地顺序（给主线 Claude）

1. **重构 ReplApp state**：加 `focus / active / dialogues / roster / task_meta`；把现有单 `dialogue` 字段替换成按 target 分桶的 map。测试层保留既有 8 个单元测试——大多数逻辑不变，只是 `push_line` 要带 target 参数。
2. **布局重绘**：把现 `draw_*` 拆成 `draw_roster / draw_dialogue / draw_input / draw_meta / draw_events / draw_status`，总体用 §2.1 骨架。
3. **key routing**：`handle_key` 先走全局键 → 再按 `focus` 分派。
4. **ingest 分桶**：在现 `ingest` 里按 §2.5 的 `target_of` 把对话事件挂到对应桶；`UserInterventionSent / TaskInterventionApplied` 必须接上。
5. **meta 维护**：在 ingest 里维护 `task_meta`；`TaskDispatched` 插入，`TaskDelivered / TaskCancelled` 清除。
6. **选 worker 提交 = 介入**：`drive_tui` 提交分支按 `active` 分派到 `dispatch` 或 `intervene`。后者走 daemon socket 最简单（玄女本来就用这条），内部调 `fuxi_cli::ipc::intervene(...)`。
7. **rosterr refresh 驱动**：订阅 `AgentSpawning/AgentDead/ShelfStatus 相关` 事件时才 refresh，避免每帧 `.await`。
8. **测试**：新增 `focus_tab_cycles`、`esc_returns_to_xuannv`、`enter_on_roster_switches_active`、`worker_responded_goes_to_its_bucket`、`dialogue_bucket_cap` 五个单元测试。TestBackend snapshot 保留一个三栏基线。

> 预估改动量：`repl.rs` ~400 行新增/替换；`Shelf` 加一个 `worktree_of` 只读方法；无新 crate 依赖。

---

## 五、Open Questions（主线 Claude 如遇到请回我）

1. "对门客说话"到底走 `intervene`（当前 daemon 接口）还是新增 `Fuxi::send_message_to_worker`？前者零改动但强依赖 daemon 启动；后者更干净。
2. 要不要让左栏显示 **"pending tasks"**？当前 `Fuxi` 没有 task 列表持久视图——需要新加 `list_active_tasks()`，不然右栏 elapsed 算不准。
3. `Focus::Events`（v1.5）上还是 defer 到有人喊？

---

Sources:
- [Ratatui Layout](https://ratatui.rs/concepts/layout/)
- [Ratatui Component Architecture](https://ratatui.rs/concepts/application-patterns/component-architecture/)
- [Ratatui Event Handling](https://ratatui.rs/concepts/event-handling/)
- [Ratatui focusable crate 讨论](https://forum.ratatui.rs/t/focusable-crate-manage-focus-state-for-your-widgets/73)
- [gitui repo](https://github.com/gitui-org/gitui)
- [tui-textarea](https://github.com/rhysd/tui-textarea)
- [Best practices for ratatui apps (#220)](https://github.com/ratatui/ratatui/discussions/220)
