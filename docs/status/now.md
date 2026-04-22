# Now Status (Live Snapshot)

更新时间：2026-04-22 23:30 CST  
分支：`feat/fuxi-v0.1`  
HEAD：`4d87cff`  
状态口径：**以当前代码工作区为准（包含未提交改动）**，不是以 handoff 历史文档为准。

---

## 1) 结论（先看这个）

1. 有些 handoff/roadmap 文档确实过时了。  
2. 你最近反馈的一批功能中，已经有不少落到代码里（但还未全部 commit）。  
3. 当前最真实状态是：**M4.5 基本落地，M5.1 正在收尾（task-bound 主路径已成形）**。
4. 本轮新增收敛：**任务树按 `task_id` 聚合、任务完成保留默认 120s、legacy 派工路径显式告警、dispatch 尾延迟默认窗口下调到 80ms**。

---

## 2) 已完成（代码已在当前工作区）

### 2.1 编排层：同一 task_id 绑定多个门客（父任务 fan-out 的基础能力）

- 已有 `dispatch_in_task(...)` 和 `dispatch_to_any_in_task(...)`
  - `crates/fuxi-orchestrator/src/fuxi.rs`
- CLI/IPC 已支持 `--task <task-id>`
  - `crates/fuxi-cli/src/subcommands.rs`
  - `crates/fuxi-cli/src/ipc.rs`
  - `crates/fuxi-cli/src/daemon.rs`
- 覆盖测试：
  - `dispatch_in_task_can_fan_out_same_parent_task_to_multiple_workers`
  - `dispatch_with_existing_task_id_reuses_parent_task`
  - `dispatch_with_invalid_task_id_returns_err`

### 2.2 TUI：Batch C 已落地的大部分

- C2 连续工具调用折叠：`collapse_consecutive_tools(...)`
- C3 玄女动词池：`verbs_xuannv()`
- C4 Ctrl+C 双击退出窗口
- C5 slash 的 Tab/Enter 行为分工 + `arg_names`
- `/tree on|off|toggle` 可配置左侧任务树

主要文件：
- `crates/fuxi-cli/src/repl.rs`
- `crates/fuxi-cli/src/spinner.rs`
- `crates/fuxi-cli/src/autocomplete.rs`
- `crates/fuxi-cli/src/command_registry.rs`

### 2.3 输入/复制/粘贴链路（你重点反馈项）

- bracketed paste
- 文件路径粘贴转绝对路径
- 图片粘贴引用 `[image #n]`（输入区不暴露长路径）
- 发送前按引用展开真实路径给后端
- 复制链路：OSC52 + 平台剪贴板

主要文件：
- `crates/fuxi-cli/src/repl.rs`
- `crates/fuxi-cli/src/clipboard.rs`
- `crates/fuxi-agent-cc/src/agent.rs`
- `crates/fuxi-agent-codex/src/agent.rs`

### 2.4 稳定性：scheduler memory 连接回收问题已修

- `connect_memory()` 固定单连接并禁掉连接回收，避免 `no such table: triggers`
  - `crates/fuxi-scheduler/src/store.rs`
- `fs_watch` 端到端测试稳定化（环境不可用时跳过，确定性入库链路由独立测试覆盖）
  - `crates/fuxi-scheduler/src/watcher.rs`

### 2.5 task-bound 收敛（本轮新增）

- `dispatch_to_any_in_task` 已改为严格 task-bound：同 task fanout 不再复用 idle
  - `crates/fuxi-orchestrator/src/fuxi.rs`
  - `crates/fuxi-orchestrator/tests/dispatch.rs`
- `repl` 已去掉 idle bucket 状态模型（`idle_workers`/`RosterRow` 移除）
  - `crates/fuxi-cli/src/repl.rs`
- extractor 派工改走 task-bound API
  - `crates/fuxi-cli/src/extractor_hook.rs`
- 任务树按 `task_id` 聚合（不再把同标题不同任务错误合并）
  - `crates/fuxi-cli/src/repl.rs`
- 任务完成后保留窗口改为默认 120s，支持 `FUXI_TASK_PRUNE_SECS`
  - `crates/fuxi-cli/src/repl.rs`
- `dispatch_to_any` 明确为 legacy 通道，并在调用时 `warn` 迁移到 task-bound API
  - `crates/fuxi-orchestrator/src/fuxi.rs`
- dispatch pump terminal drain 默认窗口从 120ms 下调到 80ms（可用 env 覆盖）
  - `crates/fuxi-orchestrator/src/fuxi.rs`
- `dispatch` 新增 `--print-task-id`，fan-out 脚本可直接捕获 task_id（不再 sed JSON）
  - `crates/fuxi-cli/src/subcommands.rs`
- 新增长消息 wrap 场景的 auto-follow 回归测试（防止“底部看不全”回归）
  - `crates/fuxi-cli/src/repl.rs`

---

## 3) 进行中 / 未完成

### 3.1 Decision 10 还没“全量完成”

以下仍未彻底落地：
- 彻底废除 orchestrator `dispatch_to_any` 兼容语义（目前仍保留给旧调用方）
- 完整 task-bound lifecycle（spawn 约束 / 生命周期治理 / 历史归档策略）
- 真正的“父任务 + 子任务树”全语义（当前是基础能力+UI过渡态）

### 3.2 C1（TeammateSpinnerTree）未看到完整闭环证据

- 有 task-rooted 分组与树渲染相关测试/逻辑，但尚不能认定“最终版 C1 全完成”。
- 需按你的最终 UI 验收标准再确认一次。

---

## 4) 当前门禁状态（本地实测）

已通过：
- `cargo test -p fuxi-cli`（243 passed）
- `cargo test -p fuxi-orchestrator`（16 + 29 passed）
- `cargo test -p fuxi-scheduler`（35 + 1 e2e passed）

---

## 5) 哪些文档已过时（相对当前代码）

### 5.1 `docs/handoff/v1-session5.md`

- 里面写“Batch C 待开工”已不准确。  
- 实际上 Batch C 的多项已在当前工作区代码中出现。

### 5.2 `docs/architecture-v1.1-roadmap.md`

- 里程碑方向仍有效。  
- 但“完成度叙述”与当前未提交实现存在时间差，需要按本文件更新口径。

### 5.3 `docs/decisions/11-tui-cc-learnings-v2.md`

- 决策本身有效。  
- 进度状态应从“计划态”更新为“部分实现态（C2/C3/C4/C5 已落地，C1 待验收）”。

---

## 6) 下一推（不再走回头路）

1. 继续收口 orchestrator 的兼容路径（减少/隔离 `dispatch_to_any` 入口）。  
2. 推进 M5.1 剩余语义：父子任务树、完成后归档与可视策略。  
3. 文档同步：roadmap/decision 的“计划态”更新为“实现态”。
