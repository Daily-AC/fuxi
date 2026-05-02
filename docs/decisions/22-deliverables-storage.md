# Decision 22 · Deliverables 存放与领取

**日期**：2026-05-02
**状态**：设计已采纳，跟 Decision 21 phase 1 同期实装

## 背景

[Decision 13](./13-deliverable-boundary-handoff.md) 定下了"门客在 deliverable
完成时 nudge 玄女"的协议：

```rust
EventKind::AgentRequestReview {
    agent, task, deliverable_kind, summary, artifact_ref
}
```

但 **`artifact_ref` 物理落地在哪没说**。Decision 13 只解决了"何时通知"，没说
"东西放在哪"。

[Decision 21](./21-workspace-design.md) 定下了 5 层 workspace lifecycle，但
全部围绕 git 类产物（代码改动 → commit → merge → canonical）。**文件级
非代码产物**没明确归宿：

- 蒲松写完一章 markdown
- 神农跑完数据生成的 csv
- 调研任务产出的 markdown 报告 + 截图
- 鲁班生成的 OpenAPI spec / 配置文件 / 数据 dump

这类产物**不该跟 sandbox 一起 GC**（用户可能 24h 后才回来看），也**不该混进
canonical**（canonical 是用户的真项目，门客不该绕过 review 直接写）。

## 决策

### 1. 物理存放：项目级 `deliverables/<task-id>/`

```
~/.fuxi/projects/<p>/
├── meta.json
├── sandboxes/<role>/         # L3 工作区
├── ephemeral/<task-id>/      # L2 工作区
├── archive/<task-id>/        # L2 archived 24h
└── deliverables/<task-id>/   # ★ 本决策新增 ★
    ├── manifest.json         # { kind, files: [{name, sha256, size}], produced_at }
    ├── chapter-3.md
    └── data.csv
```

- 跟 sandbox / ephemeral **解耦**：sandbox GC 后 deliverables 仍在
- `manifest.json` 含每个文件的 sha256 + size，重复防错 + 审计校验
- `<task-id>` 子目录：1 task → N files → 单逻辑包

### 2. 复用 `uploads.rs`，不另起一套

物理文件实际落在 `<im_data_dir>/uploads/<sha256>/<original-name>`（已有
机制，自带去重）。`deliverables/<task-id>/` 用文件级 link / 引用指向
uploads 里的真实存储——避免双份基础设施。

逻辑层：IM 库新加 `deliverables` 表，绑 task_id ↔ multiple attachment_ids ↔
deliverable_kind ↔ 状态。

### 3. 三种领取模式（默认 Mixed）

| 模式 | 触发 | 流程 |
|---|---|---|
| **Inbox**（默认） | task 未声明 `target_path` | 文件落 `deliverables/<task-id>/`，PWA 通知用户，用户在 PWA 上 accept / reject |
| **Direct** | task 声明 `target_path: <path>` | agent 直接写到 target_path；fuxi 在 `deliverables/<task-id>/` 留 audit copy（sha256 + 副本） |
| **Mixed** | 全局策略 | Inbox 是默认，Direct 是显式声明的 escape hatch |

### 4. 用户三种动作

- **Accept**：文件复制到用户指定 path（Inbox 时点击"接收"才指定；Direct 时已写好）；deliverables/ 副本**不删**（保留审计）；事件 `DeliverableAccepted`
- **Reject**：标记拒绝，文件保留在 deliverables/；事件 `DeliverableRejected`
- **Inspect**：PWA 内预览 / 下载，不改状态

### 5. 两个产品口味决策（default 已锁）

| # | 决策 | Default | 反方案 |
|---|---|---|---|
| 1 | 用户领走前的存放生命周期 | **永久保留**直到用户显式接收 / 拒绝 / 删除（不自动 GC） | 30 天 auto-GC（省空间但可能丢东西） |
| 2 | "Accept" 动作的物理含义 | **复制到 target**，deliverables/ 副本保留（双份，多一份审计） | move 到 target，deliverables/ 删（省空间但失审计副本） |

### 6. EventBus 事件（必同步五处）

新增 `EventKind` 变体：

- `DeliverableProduced { task_id, project, deliverable_kind, files: Vec<FileMeta> }`
- `DeliverableAccepted { task_id, accepted_to: Option<PathBuf>, by: UserId }`
- `DeliverableRejected { task_id, by: UserId, reason: Option<String> }`
- `DeliverableExpired { task_id, expired_at }`（暂时用不上，但 schema 留住——若后续打开 GC 用）

实装时**必同步**：
1. `crates/fuxi-core/src/event.rs` —— EventKind 定义 + serde tag
2. `crates/fuxi-events/src/store.rs::kind_tag` —— 持久化映射
3. `crates/fuxi-firehose/src/hub.rs::kind_tag` —— Hub 路由
4. `crates/fuxi-firehose/src/tui.rs::summarize + color_for` —— TUI 渲染
5. `crates/fuxi-cli/src/subcommands.rs::event_summary` —— CLI 显示

跟 b6d51d6 / Decision 21 同样的五处坑。

### 7. Agent 侧 API

```rust
pub trait Workspace {
    // ... 已有方法 ...
    
    /// 门客把工作区内文件标为 deliverable 交付。
    ///
    /// 实装：复制（hardlink 优先，跨设备 fallback copy）到 deliverables/<task-id>/，
    /// 写 manifest.json，发 DeliverableProduced 事件，触发 Decision 13 的
    /// AgentRequestReview。
    async fn produce_deliverable(
        &self,
        task: TaskId,
        kind: DeliverableKind,
        files: Vec<PathBuf>,           // workspace 内相对路径
        target_path: Option<PathBuf>,  // 用户侧目标，None = Inbox 模式
    ) -> Result<DeliverableId>;
}
```

## Review 清单（Decision 18 四问）

- **归属是否隔离？** ✓ `deliverables/<task-id>/` 明确归属 + manifest.json 自带文件清单
- **行为是否可审计？** ✓ 4 个 Deliverable* 事件 + manifest 含 sha256
- **失败是否可恢复？** ✓ 文件在独立目录，sandbox GC 不影响；用户接收前 inspect 不限次；audit copy 留底
- **结果是否可验证？** ✓ sha256 校验 + 用户显式 Accept 才算 done

## 跟现有决策的关系

- **Decision 13**：本决策给 `artifact_ref` 落物理地址
- **Decision 21**：本决策跟 Workspace 平行——Workspace 管"门客在哪干"，Deliverables 管"门客交什么给你"
- **公理 1**（Headless agent 不显式沟通 = 没做）：Deliverables 是显式沟通的物质载体

## Roadmap

### Phase 1（跟 Decision 21 phase 1 同期）

- [ ] `crates/fuxi-workspace/` 加 `produce_deliverable` API
- [ ] `crates/fuxi-im/` 加 `deliverables` 表 + 复用 uploads.rs
- [ ] 4 个 DeliverableEventKind 变体 + 五处同步
- [ ] Decision 13 的 `AgentRequestReview.artifact_ref` 接通到本决策的存储

### Phase 2

- [ ] PWA 交付收件箱视图（待处理 / 已接收 / 已拒绝 三栏）
- [ ] PWA 内预览：md 渲染、csv 表格、图片直显、其他下载
- [ ] Direct 模式 `target_path` 支持
- [ ] 用户复审 2 个产品口味题

### Phase 3

- [ ] DeliverableExpired 自动 GC（如果用户后续推翻"永久保留"default）
- [ ] 跨节点 deliverable 同步（远端节点产出，home 落地）

## 何时重审

- 用户实测发现「永久保留」磁盘吃不消 → 改 30 天 auto-GC
- 用户实测发现「双份审计」冗余 → 改 move + 删 deliverables 副本
- 出现新的 deliverable_kind 类型不在 Decision 13 五类里
- 跨节点 deliverable 同步真的发生时

## 用户必知文档

加进 [`docs/architecture/工作区-必知.md`](../architecture/工作区-必知.md) 的"附录"段——deliverables 不另开必知文档（用户视角它就是"门客交活给你"的具体落地，不是新概念）。
