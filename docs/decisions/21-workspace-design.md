# Decision 21 · Workspace 五层架构

**日期**：2026-05-02
**状态**：设计已采纳，待分阶段实装

## 背景

Decision 18 / 19 定下 Agentic Engineering 协作原则与编码护栏，要求一切产物
进**隔离工作区**、留**审计**痕、可**恢复**、可**验证**。本决策把这四问落到
fuxi 的工作区（workspace）抽象上。

### 现状（v1 半成）

只有 `crates/fuxi-workspace/src/git.rs::GitWorktreeWorkspace`：

- 落地路径：`<workspace_root>/.fuxi/worktrees/<agent-id>/`
- 索引键：`agent-id`（每次任务新生成的 UUID）
- 生命周期：`create()` + `destroy()`，无自动 GC
- `workspace_root` 是单值（`fuxi im` 默认 `$HOME/fuxi-workspace`，`fuxi up` 默认 cwd）
- 无 project 概念、无门客长期 sandbox、无层级

### 三个根本断点

1. **没有 project 概念**：`workspace_root` 单一目录 → 多项目并行不支持，要换项目得重启 fuxi。
2. **没有 per-门客 长期 sandbox**：每次任务起 ephemeral worktree → build cache、未完 WIP 全丢，门客失去跨任务连续性。
3. **CC `isolation=worktree` 不可靠**：实测在 team-spawn 路径下 no-op（agent 共享 main 工作树）。fuxi 不能依赖 host 工具的 isolation 语义，必须自己定义 workspace contract。

## 决策

### 1. 五层 workspace lifecycle

| 层 | 用途 | 落地路径 | git | 生命周期 |
|---|---|---|---|---|
| **L-1** scratch | 跟 repo 无关的活 | `~/.fuxi/scratch/<task-id>/` | 无 | 任务完即删 |
| **L0** no workspace | 单条 Bash | 任意 cwd | 无 | 进程级 |
| **L1** read-only mount | 调研、检索 | mount canonical 或 sandbox 只读 | 不写 | 任务级 |
| **L2** ephemeral worktree | 一次性写 + 可能 commit | `~/.fuxi/projects/<p>/ephemeral/<task-id>/` | task branch | active → archived 24h → GC |
| **L3** persistent sandbox | 长期门客 | `~/.fuxi/projects/<p>/sandboxes/<role>/` | 长期 branch | LRU + 显式 retire |

### 2. Project 抽象

`workspace_root` 单值升 `projects[]`：

```
~/.fuxi/
├── events.db
├── im.db
├── projects/
│   ├── <project-name>/
│   │   ├── meta.json              # canonical_path, default_branch, owner, quota_bytes
│   │   ├── sandboxes/
│   │   │   └── <role>/            # L3，git worktree of canonical
│   │   ├── ephemeral/
│   │   │   └── <task-id>/         # L2，git worktree of canonical or L3 sandbox
│   │   └── archive/
│   │       └── <task-id>/         # L2 已归档，24h 后 GC
│   └── <other-project>/
└── scratch/
    └── <task-id>/                  # L-1
```

注册：`fuxi project add <canonical-path> [--name <slug>]`，写 `meta.json` + 验证 canonical 是 git repo（不是就报错，不自动 init）。

玄女 dispatch 时根据 task 推断 project（user @-mention / 关键词匹配 / 默认 fallback 到 user-active project）。

### 3. 五个产品口味决策（default 已锁，可后续推翻）

| # | 决策 | Default | 反方案 |
|---|---|---|---|
| 1 | sandbox 忙时来新任务 | fork off L2 ephemeral 给新任务 | 串行排队 |
| 2 | sandbox 有未 commit WIP 来新任务 | 自动 stash 成 `wip/<old-task-id>` 分支，PWA 可见 | 拒收新任务 |
| 3 | canonical 漂移（用户 git pull） | 任务前自动 rebase sandbox（只动 sandbox 不动 canonical），conflict 降级提示 | 始终先提示等用户确认 |
| 4 | 跨 project task | 支持，task 显式声明 `workspaces: [{project, layer}, ...]` | 不支持 |
| 5 | 跨节点 sandbox | per-node 独立，跨节点 = 新节点上从 canonical 重新 fork（首次慢） | 跨节点同步 sandbox state |

### 4. 中层默认（实装时按此走）

- **Branch 命名**：L3 = `<role>/<project>-main`（如 `luban/erp-main`）；L2 = `task/<task-id>`
- **Commit 身份**：author = `<role-display> <<role>@fuxi.local>`（如 `鲁班 <luban@fuxi.local>`）；body 自动加 `Co-Authored-By: <user-display> <user-email>`
- **Git config 继承**：sandbox 创建时 `git config` 复制 canonical 的 `user.email` / `user.signingkey` / `core.editor`，不复制 remote
- **WorkspaceError 变体**：`CreateFailed`、`DirtyOnRead`、`LockTimeout`、`CanonicalGone`、`QuotaExceeded`、`RebaseConflict`
- **Quota**：默认每 project 5GB、每节点 8 个 active sandbox（含 ephemeral）。超就拒新建，事件流发 `WorkspaceQuotaExceeded`
- **L1 实现**：cap 限制（agent 工具层 read-only 强制）+ 门客自律。**不**用 bind mount（跨平台麻烦、Mac 上要 osxfuse）

### 5. EventBus 集成（核心，必同步五处）

新增 `EventKind` 变体：

- `WorkspaceCreated { project, role, layer, path }`
- `WorkspaceMutated { workspace_id, files_changed: u32 }`
- `WorkspaceCommitted { workspace_id, commit_sha, branch }`
- `WorkspaceArchived { workspace_id, reason: ArchiveReason }`
- `WorkspaceCollected { workspace_id, archived_at }`
- `WorkspaceQuotaExceeded { project, kind: QuotaKind, requested, limit }`
- `WorkspacePromoted { from: ephemeral_id, to: persistent_role, project }`

实装时**必同步**：
1. `crates/fuxi-core/src/event.rs` —— EventKind 定义 + serde tag
2. `crates/fuxi-events/src/store.rs::kind_tag` —— 持久化映射
3. `crates/fuxi-firehose/src/hub.rs::kind_tag` —— Hub 路由
4. `crates/fuxi-firehose/src/tui.rs::summarize + color_for` —— TUI 渲染
5. `crates/fuxi-cli/src/subcommands.rs::event_summary` —— CLI 显示

漏一处编译 / clippy 立刻报错（这是 b6d51d6 的教训）。

## Review 清单（按 Decision 18 四问对全设计）

- **归属是否隔离？** ✓ 每层都有明确 path、owner、index key（project + role + layer）
- **行为是否可审计？** ✓ EventBus 上 7 个 Workspace* 事件，全 append-only
- **失败是否可恢复？** ✓ archive 24h 缓冲 + dirty WIP 自动 stash + LRU 不强删 + RebaseConflict 降级提示
- **结果是否可验证？** ✓ L2 commit 走玄女 review pipeline；merge canonical 前必有测试 / review 证据

## Roadmap（分阶段，不卡毕设）

### Phase 1（短期，闭环最小可用）

- [ ] `workspace_root` 单值升 `projects[]`
- [ ] L3 持久 sandbox（per-门客 per-project）
- [ ] L2 fork off L3 时复用 build cache（git worktree 自然支持）
- [ ] L2 archive 24h GC
- [ ] WorkspaceEventKind 7 个变体 + 五处同步
- [ ] `fuxi project add` CLI

### Phase 2（中期）

- [ ] L1 read-only 抽象（cap 限制方案）
- [ ] 跨 project task 支持
- [ ] 跨节点 sandbox 切换
- [ ] PWA "最近的工作区" 列表（archive 状态可见 / inspect / promote / drop）
- [ ] 5 个产品口味题用户复审，按需推翻 default

### Phase 3（长期）

- [ ] 沙箱硬隔离（OS 级，按 role/node 策略开启）
- [ ] L-1 scratch 抽象
- [ ] Quota + admission control 完善
- [ ] 跨节点工作区状态迁移（如果 phase 2 per-node 独立够用就跳过）

## 与现有决策的关系

- **Decision 18**（Agentic Engineering 协作）：本决策落地"workspace-first"原则
- **Decision 19**（Karpathy 编码护栏）：本决策的"精确改动边界"靠 workspace 物理隔离
- **Decision 13**（交付边界 handoff）：handoff 的产物归属用本决策的 workspace path 表达
- **Decision 12**（dist worker 真并发）：跨节点 sandbox 切换基于 dist node 拓扑

## 何时重审

- 真接到第二个 project（多项目假设要打实）
- 跨节点 sandbox 真发生切换时
- L-1 出现真实场景（现在猜测，可能不会出现）
- 沙箱从软变硬隔离前
- 用户复审 5 个口味题后推翻任一 default

## 用户必知文档

配套：`docs/architecture/工作区-必知.md`——给以琳的入门文档，全中文白话、术语全翻、不讲实现。读完能用 Decision 18 四问扫所有 workspace 方案。
