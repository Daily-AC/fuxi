# Decision 10 · Task-bound Agent lifecycle · 任务树 UI · 命名/消歧

**日期**：2026-04-21
**状态**：已采纳（产品层拍板，实装排期见 roadmap）

## 背景

M4-REDUX 合之后，TUI 基础交互补齐。但接下来要推并行多 agent 场景时，当前架构和 UI 都暴露了不够：

1. **Agent lifecycle 是 role-bound 而非 task-bound**：spawn 一个 agent 它进 idle pool，dispatch 去跑 task，task 完了又回 pool。这和用户心智模型"**玄女派活 = 建任务 + 养门客**"不匹配
2. **任务树 UI 按 agent 扁平**（cc 风格）：伏羲和 cc 不同——cc 是"一个对话，多 teammate 协作"，伏羲是"**多任务，每任务有多门客**"。flat-by-role 会让 3 个 task 各自的鲁班混在一起看不出归属
3. **同 role 多实例消歧**：用户可能 3 个并行 task 都要鲁班，名字怎么取、UI 怎么区分未明确

## 决策

### A · Agent lifecycle 改为 task-bound

- **spawn 必须携带 task**：`Fuxi::spawn_in(task_id, role, ...)`。无 task 的 spawn 不合法
- **门客归属于 task**：`TaskNode.members: Vec<AgentId>`；`Agent.task_id: Option<TaskId>`（玄女 None，其余必有）
- **task 完 → 门客留在 task 下**直到 GC 或手动 kill
- **废除 idle pool 概念 + dispatch_to_any 的 "find idle same-role" 语义**
- 玄女本身是特例，不归任何 task，永远 pinned

### B · 任务树 UI 样式

Task-rooted：task 是根，门客是叶。详见 `docs/decisions/10-task-bound-agents.md` 草图。

```
●  玄女                                         idle
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
▾  ◉  修 auth bug                              0:47
   ├ ● 鲁班         Bash(cargo test)          1.2k
   └ ◉ 铸牒司       Edit(migrations/042)       800

▾  ◉  升级 rust 1.75                           3:20
   ├ ● 鲁班#2       Bash(cargo update)        2.1k
   └ ◉ 小乙 · unit  Read(Cargo.toml)           400
   └ ◉ 小乙 · integ Read(tests/)               380

▾  ✓  补 unit tests                         已完 2m
   └ ✓ 鲁班#3       +15 tests                 1.8k
```

- **玄女** pinned 顶部，`━━━` 分割线隔
- Task 节点：title + elapsed/done 标记；折叠/展开（`▾/▸`）
- 叶子门客：status glyph + 最近工具 + token 计数
- 完成 task 默认折叠 + `✓ Nm`（N 分钟后 auto-archive 到 `/history`，pin 可保住）
- 死门客：`✕ 鲁班 · 已故 OOM` 红 dim，30s 自动清

### C · 门客命名 · `#N` 持久计数器

- 第一个同 role 实例：`鲁班`
- 并发第二个起：`鲁班#2`、`鲁班#3` ...
- `#N` 是**整个进程生命周期持久**，死掉不复用号
- Suffix 3 位内（999 上限足够）

### D · 同 task 多 role 实例的子任务描述

Task split 场景：一个任务派 N 个同 role 并行，每个干一份。

```
▾  ◉  跑全量测试                              0:40
   ├ ◉ 鲁班#4 · unit     Bash(cargo test --lib)
   ├ ◉ 鲁班#5 · integ    Bash(cargo test --tests)
   └ ◉ 鲁班#6 · e2e      Bash(playwright)
```

- Task 可选 API：`task.split_into(Vec<SubtaskDesc>) -> Vec<(AgentId, Subtask)>`
- Subtask desc 是 **短 label**（≤ 10 字），**不是独立 task**（不占 task tree 层次）
- 单门客单 task 场景不必填

### E · `@` mention 消歧 popup

- `@luban` 单活 → 直接 DM
- 多活 → popup 带 **task context 列**：
  ```
  @luban 键入后 →
  ┌─ 三个鲁班 ─────────────────────────────┐
  │ 鲁班       修 auth bug     ◉ 1:20 busy │
  │ 鲁班#2     升级 rust 1.75   ● 3:20 busy │
  │ 鲁班#3     补 unit tests    ✓ 已完      │
  └─────────────────────────────────────────┘
  ```
- 用户看 task title 消歧，不靠记 `#N`

### F · user-turn 不进树

Decision 04 退化出的 "user-turn" task 归属"和玄女对话"**隐式节点**——不作为树节点显示（会污染视觉）。

### G · Task title 强制

玄女 dispatch 必须给 `--title "人话 3-10 字"`。`Task::new` 的第一参数 = title，要求 skill 教玄女写得像 Linear issue 标题。

## 为什么这么决策

1. **心智模型对齐**：用户想的是"任务 → 派的人"，不是"agent 池 → 找活干"
2. **demo 视觉**：task-rooted 树一眼看清"玄女管 4 个项目，每项目有多个门客"——差异化卖点直接呈现
3. **归属清晰**：Agent 死了用户知道是哪个 task 的事故；task 完了相关的门客状态自然归档
4. **和 Decision 01（并行 cc team）耦合**：多 agent 场景就是多 task × 多 role，现状没解答这组合怎么 UI

## 代价

- **破坏性重构**：`shelf.insert_agent / dispatch_to_any / spawn_worker` 全部要改；`~/.fuxi/memory.db` session 关联逻辑跟着改（sessions 过去是 by agent-id，之后可能 by task-id + agent-id）
- **回退路径缺**：v1.0 的 API 要不要保留 shim？暂不保留。全砍
- **测试重写**：涉及 agent 生命周期的测试几乎全要改

## 实施排期（Batch D · v1.1 末或 v1.2 初）

拆法见 roadmap §M5（M4-REDUX 之后的下一个大批次）。

## 参考

- Decision 03（TUI 任务树 override）—— 本 decision 细化其 UI
- Decision 04（intervene idle 退化 dispatch）—— user-turn 处理耦合
- Decision 06（文化命名）—— 鲁班/铸牒司/小乙/少司命 等 role 名
- Decision 11（cc TUI 借鉴）—— 部分 UI 要素同源
