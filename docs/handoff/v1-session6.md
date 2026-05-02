# Handoff · v1 · Session 6 开工指引

> 上一 session（2026-05-02 → 05-03 凌晨）专推 Decision 21/22（workspace + 交付
> 产物）—— 22 个 commit 全闭环，phase 1 + 部分 phase 2 已 ship。
> 上份 handoff: `docs/handoff/v1-session5.md`（保留）。

---

## 1 · 10 分钟必读

1. `CLAUDE.md` · 七公理（不变）+ agent team / decision-split 协作（3 min）
2. **`docs/decisions/21-workspace-design.md`** · 工作区五层（3 min）
3. **`docs/decisions/22-deliverables-storage.md`** · 文件级交付（3 min）
4. `docs/architecture/工作区-必知.md` · 用户视角入门（已写，给将来再次涉及 workspace 时复读）
5. 本文 §3 "下一动作"

---

## 2 · 上一 session ship 了什么

**22 个 commit**（自 `b6d51d6` 起到 `bcff713`）：

后端实体：
- `Project` / `ProjectId` / `FileSystemProjectRegistry`（fuxi-core + fuxi-workspace）
- `PersistentSandboxManager`（L3 持久 sandbox）
- `DeliverablesManager`（per-task bucket + sha256 + manifest）
- 11 个 EventKind 变体（`Workspace*` × 7、`Deliverable*` × 4）+ 五处同步
- `Fuxi::spawn_worker_in_project_sandbox` + `Fuxi::set_project_registry`
- bridge.rs sentinel hook publish `WorkspaceCommitted`

CLI:
- `fuxi project add | list | info | rm`
- `fuxi sandbox list | retire`
- `fuxi deliverable produce`
- `fuxi spawn --project <slug>` 走 L3 sandbox 路径

HTTP API（fuxi-im）:
- `GET /api/projects`、`POST /api/projects`、`DELETE /api/projects/:id`
- `GET /api/projects/:id/sandboxes`
- `GET /api/deliverables`
- `GET /api/deliverables/:p/:t/files/:name`
- `POST /api/deliverables/:p/:t/accept`（含 accepted_to 真实拷贝）
- `POST /api/deliverables/:p/:t/reject`

PWA UI（fuxi-im/web）:
- 「项目」tab：列表 + 注册 modal + 删除二次确认 + 内嵌 sandbox 列表
- 「交付」tab：收件箱 + 文件下载 + 接收/拒绝 + 状态徽章

Agent prompt:
- spawn 时给 profile.system_prompt 注入「项目身份」段（slug、sandbox path、branch）
- dispatch 时给 task.description 注入 `[FUXI_TASK_ID=task-...]`
- sentinel addendum 加「文件级交付」教学指向 `fuxi deliverable produce`
- 玄女 dispatch-routing.md 加 project 维度规则

事件流通：
- `WorkspaceCreated` → spawn_worker_in_project_sandbox（活）
- `WorkspaceMutated` → fuxi deliverable produce CLI cwd 反查（活）
- `WorkspaceCommitted` → bridge.rs sentinel `code_change` + `sha:` artifact_ref（活）
- `DeliverableProduced` / `Accepted` / `Rejected` → 全活
- `WorkspaceArchived` / `Collected` / `Promoted` / `QuotaExceeded` / `DeliverableExpired` —
  schema 已锁但**还是死字段**（depends on phase 2 lifecycle）

文档：
- `docs/decisions/21-workspace-design.md` · 五层架构 + 5 个产品口味决策 default
- `docs/decisions/22-deliverables-storage.md` · 收件箱 + 接收/拒绝
- `docs/architecture/工作区-必知.md` · 用户视角入门
- `docs/fable.md` · 寓言体系统介绍

测试：
- 后端 cargo test --workspace 全绿（除 dist::cancel_in_silent_period_via_heartbeat_ack
  已知 timing flake，跟本批改动无关）
- PWA pnpm test 296/296（含新 ProjectsPage / DeliverablesPage 单测）

---

## 3 · 下一动作（按价值排）

### A. L2 ephemeral lifecycle 真实装（M）

**目的**：让一次性 task 走 `<projects_root>/<project>/ephemeral/<task>/`
worktree fork off canonical（或 L3 sandbox），任务完后 archive 24h，然后 GC。

**当前状态**：
- 五层模型设计文档已写明（Decision 21）
- 路径骨架 FileSystemProjectRegistry 注册时已 mkdir `ephemeral/` 子目录
- **没有任何代码实际创建 / 用 / 清理 L2 worktree**
- `WorkspaceArchived` / `Collected` 事件类型已锁但无发布点

**实装范围**：
1. `Fuxi::spawn_worker_in_ephemeral_workspace(project, task)` 入口（symmetric to L3）
2. PersistentSandboxManager 的 ephemeral 兄弟（`EphemeralWorkspaceManager`）
3. 一个 scheduler tick task（每 N 分钟扫一次 archive/，超 24h 删）
4. 玄女 dispatch 路径决策：什么时候走 L2 vs L3
5. WorkspaceArchived / Collected 事件发布

**风险**：影响 dispatch 决策树语义，跟现有 spawn_worker 老路径互动复杂。

### B. WorkspaceQuotaExceeded 实装（S）

**目的**：用户配 quota（每 project 5GB / 每节点 8 active sandbox），超过拒
新建 + publish QuotaExceeded。

**实装范围**：
1. Project meta.json 加 `quota_bytes` / `max_concurrent_sandboxes`
2. spawn_worker_in_project_sandbox 入口先检查 quota
3. 命中 → 拒绝 + publish 事件 + CLI / API 返 429

**风险**：低，孤立功能。

### C. 跨节点 sandbox（L）

**目的**：mac sandbox vs home sandbox 是 per-node 独立体（per Decision 21
default 5）。当前 fuxi spawn --project 只在本机 spawn——跨节点没考虑。

**实装范围**：完整跨节点路由 + dist worker 拉项目 sandbox 起 cc → 更复杂。
建议 phase 3 单独动。

### D. PWA 项目 detail Layer 2（M）

**目的**：项目卡 tap → 进二层页，显示该项目 sandboxes + 交付 + 历史
事件流。

**当前状态**：sandboxes 已 inline 显示。Layer 2 navigation 需要扩 NavigationStack
到项目 tab（当前只在任务 tab）。

---

## 4 · 用户实测路径（已验通）

home 上的真路径：

```bash
ssh home && cd ~/fuxi && git pull && cargo build --release -p fuxi-cli
systemctl --user restart fuxi-im

fuxi project add ~/erp                   # PWA 项目 tab 应见 erp
fuxi spawn --role luban --project erp    # → ~/.fuxi/projects/erp/sandboxes/luban/ 起 cc
fuxi sandbox list --project erp          # 应见 luban
fuxi project info erp                    # 一屏综合信息

# 派活给鲁班（自动注入 [FUXI_TASK_ID=...] + 项目身份段）
TASK_ID=$(fuxi dispatch --to <agent-id> --print-task-id "请生成 ERP 模块调研报告并 produce")

# 鲁班 cc 在 sandbox cwd 里写 report.md，bash 跑：
#   fuxi deliverable produce --project erp --task <task-id> --kind research_summary report.md
# → DeliverableProduced + WorkspaceMutated 双事件 publish
# → PWA 交付 tab 见该卡

# 用户在 PWA 点接收 + accepted_to=~/写作/
# → ~/写作/report.md 真实落地
# → 卡片状态翻"已接收"
```

---

## 5 · 踩坑预防

- **registry 同源**：fuxi-im 同时把 `FileSystemProjectRegistry` 注入 `AppState`
  + `Fuxi`（im.rs:228-230）。如果两边走不同 root → PWA 看到 erp 但 spawn
  报"未注册"。**production 单源 `$HOME/.fuxi/projects/`**。
- **L3 sandbox 长期共享**：同 (project, role) 重复 spawn 复用同一 sandbox，
  agent_id 不同但路径相同。dispatch 派活到任一 agent 都跑在同 sandbox（cwd
  共享 → file race 风险，v1 接受）。
- **task_id 注入只对非 xuannv/extractor**（黑名单见 `Fuxi::maybe_inject_task_id`）。
  玄女自己 dispatch 不会被注入污染。
- **WorkspaceMutated 反查 cwd 失败 silent skip**：CLI 也可能从普通 worktree
  跑（用户手动测），不强求命中。期望命中场景仅 agent bash 自报 cwd 时。
- **accept_deliverable 系统目录黑名单**：`/etc /sys /proc /usr /boot /bin
  /sbin /lib /lib64 /var/{log,lib,run,cache,spool}`。`/var/folders` 是 macOS
  tmp 必须放行；`/var/tmp` 也是合法。

---

## 6 · 决策快照（这次产生的）

无新公理 / 反公理，全部按 Decision 18/19/21/22 在落地。

唯二的"产品口味题"待用户主动复审：
1. accepted_to 文件拷贝是 copy（保留 deliverables/ 副本）—— 可改 move 省空间但失审计
2. deliverables 永久保留（不 auto-GC）—— 可开 30 天 auto-GC 但要先实装
   `DeliverableExpired` 调度

两条都已落 default 值，用户没说要改就保持。
