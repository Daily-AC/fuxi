# Now Status (Live Snapshot)

更新时间：2026-04-22 21:27 CST  
分支：`feat/fuxi-v0.1`  
HEAD：`cfe2590`  
状态口径：**以当前代码工作区为准（包含未提交改动）**，不是以 handoff 历史文档为准。

---

## 1) 结论（先看这个）

1. 有些 handoff/roadmap 文档确实过时了。  
2. 你最近反馈的一批功能中，已经有不少落到代码里（但还未全部 commit）。  
3. 当前最真实状态是：**M4.5 的不少点已实装，M5.1（Decision 10 的全量 task-bound 重构）还没完成**。

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

---

## 3) 进行中 / 未完成

### 3.1 Decision 10 还没“全量完成”

以下仍未彻底落地：
- 彻底废除 `idle pool` 语义（当前 `repl.rs` 仍有 `idle_workers` 相关逻辑）
- 完整 task-bound lifecycle（不仅是 dispatch 复用 task_id）
- 真正的“父任务 + 子任务树”全语义（当前是基础能力+UI过渡态）

### 3.2 C1（TeammateSpinnerTree）未看到完整闭环证据

- 有 task-rooted 分组与树渲染相关测试/逻辑，但尚不能认定“最终版 C1 全完成”。
- 需按你的最终 UI 验收标准再确认一次。

---

## 4) 当前门禁状态（本地实测）

已通过：
- `cargo test -p fuxi-cli`（243 passed）
- `cargo test -p fuxi-orchestrator`（16 + 28 passed）

失败：
- `cargo test -p fuxi-scheduler`
  - 失败用例：`watcher::tests::fs_watch_fires_on_file_create`
  - 单测单独复跑仍失败，非一次性波动

说明：
- 该失败位于 fs watch 测试，和本轮 UI/编排主改动不完全同域，但它目前确实使该 crate 门禁为红。

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

1. 先把当前 WIP 按里程碑拆成 1-2 个 commit（避免继续滚雪球）。  
2. 修复/隔离 `fuxi-scheduler` 的 fs-watch 失败，恢复基础门禁。  
3. 直推 M5.1：完成 Decision 10 全量 task-bound（生命周期 + 树语义 + UI 编排），结束“半过渡态”。

