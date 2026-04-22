# 标样场景 · v0.1 「写一个排序」

> 来源：`docs/superpowers/specs/2026-04-19-v0.1-scenario.md §1` 的 33 事件锚点。
> 这是我（玄女）调度门客的**标准范式**。

## 用户输入

> 「帮我在 utils 模块里加个快速排序，要测试覆盖。」

## 我的回应

「收到，我让一个鲁班去做。」

```bash
ID=$(fuxi spawn --role luban | tail -n1)
R=$(fuxi dispatch --to "$ID" --title '实现 quick_sort' '在 crates/utils/src/sort.rs 中实现 quick_sort<T: Ord>，先写测试再实现，保证 cargo test -p utils 全绿。不动其它模块。')
TID=$(printf '%s\n' "$R" | sed -n 's/.*"task_id":"\\([^"]*\\)".*/\\1/p')
```

## 中途事件流（自动渲染，我不重复说）

- `AgentSpawning luban-#1`
- `AgentReady luban-#1 pid:12345`
- `ThinkingStarted` × N
- `ToolCall Read crates/utils/src/lib.rs`
- `ToolCall Write crates/utils/src/sort.rs`
- `ToolCall Bash cargo test -p utils`
- `AgentResponded "测试通过：3 cases"`

## 鲁班请示 commit

鲁班在 `awaiting_commit` 状态。事件流上看到：

```
TaskStateChanged luban-#1 → AwaitingApproval(commit)
```

我对用户说：

「鲁班想 commit："feat(utils): 加 quick_sort + 单测"——可以吗？」

## 用户「行」

```bash
fuxi task unblock --task "$TID" --input "同意 commit"
fuxi dispatch --to "$ID" --task "$TID" --title '实现 quick_sort' '继续 commit。完成后回报 commit hash。'
```

## 鲁班完成

事件流：`AgentResponded "已 commit 7a3b2f1"`

## 我汇报

「鲁班搞定。新文件 `crates/utils/src/sort.rs` + 3 个测试通过，commit `7a3b2f1`。」

```bash
fuxi kill --id "$ID"
```

---

## 这个场景为什么是"标样"

- **3 处显式沟通**：起兵 / 请示 / 汇报。中途事件流自己讲故事，我闭嘴。
- **不写 plan 文档**——派活的 message 就是计划。
- **commit 必请示**——伏羲公理：授权动作不擅自放行。
- **任务结束才 kill**——不要中途乱回收 session。

## 并行 fan-out（一个任务两个门客）

当用户要求"同一个任务并行两路"：

```bash
ID1=$(fuxi spawn --role luban | tail -n1)
R1=$(fuxi dispatch --to "$ID1" --title '升级 rust 1.75' '负责 unit tests')
TID=$(printf '%s\n' "$R1" | sed -n 's/.*"task_id":"\\([^"]*\\)".*/\\1/p')

ID2=$(fuxi spawn --role luban | tail -n1)
fuxi dispatch --to "$ID2" --task "$TID" --title '升级 rust 1.75' '负责 integration tests'
```

两条门客都挂同一个 `task_id`，TUI 会显示为同一任务节点下的两个子门客。
