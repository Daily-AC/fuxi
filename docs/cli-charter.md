# `fuxi` CLI charter

> 命令、动词、参数、弃用的统一规约。M3.3 出。
> 改动 CLI 表面前先看这份，破坏惯例要在这里登记。

## 1 · 用户视角铁律

**用户只跟玄女对话**。下面的子命令分两层受众：

- **玄女工具**：玄女的 cc 实例通过 Bash 调，人类 ops 救急也能调
- **平台命令**：人类用户启动 / 观察平台用

CLAUDE.md 公理 #4：CLI 是工具层的唯一形态。Agent 调工具直接 shell，不用 MCP。

## 2 · 命令分类

### 平台命令（人类用户）

| 命令 | 用途 |
|---|---|
| `fuxi`（无参） | 进 REPL，玄女对话入口 |
| `fuxi up` | 长跑 daemon + Hub + scheduler |
| `fuxi watch` | 连 Hub 开 TUI 观察 |
| `fuxi demo` | 端到端 cc 链路烟雾 |

### 玄女工具

| 命令 | 用途 |
|---|---|
| `fuxi spawn --role <r>` | 起新门客（[+召回 flag](#召回-flag)） |
| `fuxi dispatch --to <id> [--task <task-id>] [--title <title>] [--print-task-id] <msg>` | 派活（可复用父任务 / 只回 task_id） |
| `fuxi intervene --to <id> --mode <m> <msg>` | 介入（append/interrupt） |
| `fuxi status [--id <id>]` | 看门客状态 |
| `fuxi list` | 列所有门客 |
| `fuxi kill --id <id>` | 单杀（豁免玄女；不销毁 worktree——召回仍可用，见 Decision 07） |
| `fuxi block --task <id> --reason <r>` | 请示前标记 Blocked |
| `fuxi task unblock --task <id>` | 解锁 Blocked 任务 |
| `fuxi memory <verb>` | 甲骨 / 河图洛书 CRUD |
| `fuxi skill <verb>` | 点将台招贤 |
| `fuxi cron <verb>` | 更漏 trigger 管理 |

### 救急 / ops

| 命令 | 用途 |
|---|---|
| `fuxi events [--tail N] [--follow] [--filter <id>]` | **救急** 直读 SQLite 事件流；TUI 已实时渲染，**别当 poll 用** |

## 3 · 命令结构规约

### 动名结构 vs 动词独立

**动名（`fuxi <resource> <verb>`）** —— 资源有多动作时用：
- `fuxi task unblock` （未来还会有 `task list / task block`）
- `fuxi cron add / once / list / fire / remove`
- `fuxi memory query / record / supersede / search / list / learn / promote`
- `fuxi skill list / stage / approve / reject / activate`

**动词独立** —— 资源是 agent 且操作密集时（高频路径）：
- `spawn / dispatch / intervene / status / list / kill / block`
- 历史包袱 + 玄女 prompt 经常出现，强行套 `agent <verb>` 增加心智成本

**判断标准**：资源 ≥ 2 个 verb → 动名；只有 1 个或操作太频繁 → 动词独立。

## 4 · 标识符 flag 规约

| flag | 含义 | 用法 |
|---|---|---|
| `--id <agent-id>` | 选定门客 | `fuxi kill --id agent-abc` / `fuxi status --id agent-xyz` |
| `--to <agent-id>` | 消息 / 任务的目的门客 | `fuxi dispatch --to agent-abc 'msg'` / `fuxi intervene --to ...` |
| `--role <role-name>` | spawn 时指定门客角色 | `fuxi spawn --role luban` |
| `--task <task-id>` | 选定 task | `fuxi block --task task-x` / `fuxi task unblock --task task-y` |
| `--print-task-id` | 只打印 dispatch 返回的 task_id | `TID=$(fuxi dispatch --to ... --print-task-id '...')` |
| `--db <path>` | SQLite 路径覆盖 | `fuxi memory list --db /tmp/x.db` / `fuxi events --db ...` |
| `--tail <N>` / `--follow` | 流式输出参数 | `fuxi events --tail 100 --follow` |
| `--filter <s>` | 过滤（前缀匹配） | `fuxi events --filter agent-abc` |
| `--reason <text>` | 状态变更原因 | `fuxi block --task t --reason awaiting_commit` |
| `--input <text>` | 用户授权话 | `fuxi task unblock --task t --input 同意` |
| `--mode <m>` | intervene 模式 | `fuxi intervene --mode append/interrupt` |
| `--name <s>` | spawn 时门客名 | `fuxi spawn --role luban --name luban-frontend` |

### 召回 flag

P2 召回（Decision 07）专属：
- `--recall-task <task-id>` —— 续写指定 task 的 cc session（与 `--recall-role` 互斥）
- `--recall-role <role>` —— 取该 role 最近活动的 session（updated_at DESC）

## 5 · 子命令分组规约

`#[command(subcommand)]` 用于动名结构的资源。资源动作子枚举命名：`<Resource>Cmd`（如 `TaskCmd / CronCmd`）。

```rust
#[derive(Subcommand)]
enum Command {
    /// 【玄女工具】task 资源动作。
    #[command(subcommand)]
    Task(TaskCmd),
    // ...
}

#[derive(Subcommand)]
enum TaskCmd {
    Unblock(subcommands::TaskUnblockArgs),
}
```

## 6 · 输出规约

### stdout

- 成功：单行 JSON `{"key":"value", ...}`，方便玄女 parse
- 失败：单行 `Error: <msg>` 走 stderr，exit code 非零

### stderr

- 日志（tracing）：默认 `info,fuxi=debug`，`RUST_LOG` 覆盖
- 弃用警告：`warning: <old-cmd> 已弃用，请用 <new-cmd>；下个版本删除 alias`
- TUI 模式（`fuxi` REPL / `fuxi watch` / `fuxi demo --tui`）：**必须**在 init_tracing 之前 `dup2 fd 2 → /tmp/fuxi.log`，否则 alt screen 退出后 tracing 涌出污染屏幕（CLAUDE.md 已记踩坑）

## 7 · 弃用流程

破坏 CLI 表面前 **必须**走两步：
1. **新名上线 + 老名 alias**：`fuxi resume` → `fuxi task unblock` 一例。alias `run_<old>` 内部转 `run_<new>`，stderr 一行 deprecated warning + 标明新命令
2. **下一 minor 版本删 alias**：在本 charter 加一行 changelog 记录哪个版本删了什么

不允许"直接换名"——破坏玄女 skill 教学和用户习惯。

## 8 · 兼容矩阵 / 历史包袱

| 当前命名 | 历史名 | 删除版本 |
|---|---|---|
| `fuxi task unblock` | `fuxi resume` | v1.2 删 alias |
| `fuxi kill --id <id>` | `fuxi kill <id>`（位置参数） | M3.7 已改，无 alias（之前实装是 stub 返错误，无生产用法） |

## 9 · clap 实践

- 所有 `Args` 结构 `pub struct <X>Args` 放 `subcommands.rs`
- run 函数 `pub async fn run_<x>(args) -> Result<()>`
- 互斥 flag 用 `#[arg(conflicts_with = "...")]` 让 clap 在 parse 阶段就拒，daemon 端**也要**自己再守一次（因为 ipc 路径不走 clap）
- 默认值用 `default_value` / `default_value_t`，不在函数体里 `unwrap_or`

## 10 · 改 charter 的程序

- 改命名 / flag → PR 同时改这份文档
- 加新动名分组 → 这份文档加表 + 玄女 `tool-map.md` 加教学
- 弃用 → §7 流程 + §8 加行
