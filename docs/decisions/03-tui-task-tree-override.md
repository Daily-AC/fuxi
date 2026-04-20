# Decision 03 · TUI 左栏 override 为任务树（不是 agent roster）

**日期**：2026-04-20  
**状态**：已采纳；override `architecture-v1.md §M1.4` 中 C2 实装

## 背景

C2 teammate 的 R4 survey 产出 `docs/research/tui-3pane-design.md`，最初设计是：

```
左栏 · 门客 roster（agent list）
  🟢 玄女
  🔵 鲁班-#1 (Busy)
  🔵 鲁班-#2 (Idle)
```

用户实测后指出（2026-04-20）：「tui 左侧是不是任务树，是具体的 agent？」并回看了 `docs/session-review-2026-04-19-afternoon.md` §5.2 原话：

> 1. 左侧任务树 + 中间对话区 + 右侧任务元信息（三栏目标）
> 3. Switch = 点左侧任务节点 → 主对话对象变该节点负责门客

## 决策

左栏按 **task** 分组（task 是第一等），门客挂在 task 下面作执行者：

```
📋 任务
  🟢 玄女 · 总控              [持久顶部]

  📁 T001 · 写排序算法       [InProgress]
    └─ 🔵 鲁班-#1 Busy

  📁 T002 · 部署 CI           [Blocked]
    └─ 🟡 造父-#1 Idle

  📋 空闲门客                 [顶部下]
    └─ ⚪ 鲁班-#2 Idle

  ── 事件（F2 展开）──
```

Tab 循环：**玄女 → T001 负责人 → T002 负责人 → 空闲门客们 → 玄女**。

## 理由（为什么任务是第一等）

1. **用户心智模型**：用户关心"我派的这件事进展怎样"不是"哪个 agent 在干活"。agent 是执行者，**任务是承诺**
2. **多 worker 扩展性**：v2 可能一个 task 多 worker（鲁班 + 皋陶 配合）；任务树天然多子节点，roster 扁平就糊了
3. **状态折叠**：Done 后 5s 自动 prune 任务（用户看到"完成"标签再消失）；roster 里完活的 agent 无处安放
4. **点 task → 切 active**（Switch 原语）自然：点任务节点 = 主对话对象变该 task 的负责人；点扁平 roster 不对应"任务上下文"

## 影响面

- 原 C2 的 `Focus { Roster, Input }` / `dialogues: HashMap<ActiveTarget, _>` 大部分保留
- `Roster` struct 重写：`{ xuannv, tasks: Vec<TaskNode>, idle_workers: Vec<AgentCard> }`
- 事件订阅维护：`TaskDispatched` → 加节点；`TaskStateChanged: Done` → 5s prune；`AgentReady` → idle_workers；`AgentDead` → 任何位置移除
- 原 C2 的 13 个单测大部分需改写（roster → task_tree 语义）
- Fix-D teammate 负责全改

## 保留的东西

- `enum ActiveTarget { Xuannv, Worker(AgentId) }`（不变）
- 三栏布局 `Layout::horizontal([Length(26), Min(40), Length(28)])`（不变）
- 全局 key routing（Ctrl-C / Tab / Esc / F2）（不变）
- 中栏 dialogues 分桶 / 右栏 meta 切 active（不变）

## 新增交互（Fix-D 一起做）

- `tui-textarea` crate 替换自实现输入框（multi-line / 光标 / 粘贴）
- `crossterm::event::Paste` bracketed paste 支持
- `ratatui::ScrollbarState` 中栏历史消息上滑
- 连续同 speaker 消息**去前缀折叠**（`玄女>` 首行，续行 4 空格缩进）
- 事件流重新设计（按 agent 分组 / 折叠 cc_system_* / tool_call 带 args）

## 参考

- session-review-afternoon §5.2（原决策）
- R4 survey（v1 roster 是 compromise，v2 任务树是终态）—— 这次 fast-forward 到终态
