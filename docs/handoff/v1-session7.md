# Handoff · v1 · Session 7 开工指引

> 上一 session（2026-05-03 凌晨）通宵直推 Decision 21 phase 3 ——
> dispatch 自动 L2/L3 决策、AgentDead 自动归档 L2、L2 GC interval 任务、
> PWA 二层视图（项目 detail + 交付 detail）、磁盘 quota。
> 上份 handoff: `docs/handoff/v1-session6.md`（保留）。

---

## 1 · 10 分钟必读

1. `CLAUDE.md` · 七公理（不变）+ agent team / decision-split 协作（3 min）
2. **`docs/decisions/21-workspace-design.md`** · 工作区五层 + 三 phase（3 min）
3. **`docs/decisions/22-deliverables-storage.md`** · 文件级交付（3 min）
4. `docs/architecture/工作区-必知.md` · 用户视角入门
5. 本文 §3 "下一动作"

---

## 2 · 上一 session ship 了什么

phase 3 完整闭环（13 个 commit · session 6 直推后）：

后端：
- `Fuxi::archive_l2_workspace` AgentDead 自动钩子（bridge.rs::project_task_from_l2_path
  反查 worktree 路径形态 `<root>/<project>/ephemeral/<task-display>/...`）
- `Intervener` trait 扩 `archive_l2_workspace`（默认 silent Ok，Fuxi 实装走真路径）
- `crate::l2_gc::sweep_once` + `run` —— 周期扫所有 project archive，删过期 L2，发
  `WorkspaceCollected`（FUXI_L2_GC_INTERVAL_SECS / FUXI_L2_GC_MAX_AGE_SECS 可调）
- 磁盘 quota：`Fuxi::enforce_disk_quota` + `dir_size_bytes`（symlink-safe 递归）
  + 60s LRU 缓存 + `FUXI_PROJECT_DISK_QUOTA_BYTES`（默认 5GB，0 = 关闭）
- `GET /api/projects/{id}/ephemeral` —— 列 L2 active + archived

CLI:
- `fuxi spawn --project erp --ephemeral --task <task-id>` 走 L2 spawn 入口
- `Command::Spawn { ephemeral_task: Option<String> }` wire（serde default 兼容老）

PWA（fuxi-im/web）:
- `NavRoute` 扩 `project` / `deliverable` kind；NavigationStack 适用 tab 1/2/3
- ProjectsPage card → tap → `ProjectDetailPage`（项目元 + L3 + L2 active/archive + 交付汇总）
- DeliverablesPage card → tap → `DeliverableDetailPage`（manifest 全文 + 文件预览
  + accept 真路径输入框 + reject 理由）

玄女 prompt:
- `roles/xuannv/instructions/dispatch-routing.md` 加 「L2 vs L3：一次性 vs 持续」
  判定表（"调研一下" → L2 / "接着搞 feature" → L3）

测试：
- 后端 cargo test --workspace 全绿（含 2 个新 bridge 测试 + 2 个 l2_gc 测试 +
  2 个 dir_size_bytes 测试 + 1 个 ipc 测试）
- PWA pnpm test 304/304（含新 ProjectDetailPage 4 个 + DeliverableDetailPage 4 个）

---

## 3 · 下一动作（按价值排）

### A. PWA polish（S）

- ProjectDetailPage 「交付汇总」row tap 跨 tab 跳到 DeliverableDetailPage
  （v1 留 navPush no-op，需要 setActiveTab(3) + navPush 双步路由 helper）
- 项目 detail 加 "L2 promote → L3" 按钮（后端 `Fuxi::promote_l2_to_l3` 已就位）
- 交付 detail 加 manifest.json 原文折叠区
- L2 archive row 加 "复活 / restore" 按钮（v1 没实装 EphemeralWorkspaceManager::restore）

### B. 跨节点 sandbox（L）

home / mac-local 分别独立 ProjectRegistry。当前 `fuxi spawn --project` 只在
本机起。dist worker 需要：
- dist register 时把节点的 ProjectRegistry root 报上来
- controller 路由 task 到 sandbox 所在节点（按 tag 匹配 + tag 含 `project:erp`）
- 跨节点 deliverable 拉回（用户在 PWA 看到的应是聚合 inbox）

### C. tag-aware spawn（M）

`fuxi spawn --project erp` 当前只看本机。期待：
- role.metadata.dist_node = `home` → 自动起在 home 上
- 玄女 dispatch-routing 已有 `["erp"]` tag 规则；spawn 端需对齐

### D. accept_deliverable 安全审计（S）

当前 validate_accept_target 黑名单 `/etc /sys /proc /usr /boot /bin /sbin /lib
/lib64 /var/{log,lib,run,cache,spool}`，放行 `/var/folders` (mac tmp) + `/var/tmp`。
建议加白名单模式（仅 `$HOME` 子树）作 strict mode env 开关。

---

## 4 · 用户实测路径（含 phase 3 新功能）

```bash
ssh home && cd ~/fuxi && git pull && cargo build --release -p fuxi-cli
systemctl --user restart fuxi-im

# Phase 1 仍如旧
fuxi project add ~/erp
fuxi spawn --role luban --project erp                 # L3 持久 sandbox

# Phase 3 新：一次性活
TASK_ID=$(uuidgen)
fuxi spawn --role luban --project erp --ephemeral --task $TASK_ID
fuxi dispatch --to <new-agent> --task $TASK_ID "调研一下 ERP 现有报告模块"
# → 鲁班在 ~/.fuxi/projects/erp/ephemeral/task-<uuid>/ 干活
# → 跑完 cc / 用户 fuxi kill → AgentDead 触发 bridge auto-archive
# → ~/.fuxi/projects/erp/archive/task-<uuid>/ 写 fuxi-archive-meta.json
# → 24h 后 fuxi-im L2 GC interval 自动 collect_expired 物理删

# PWA 二层视图：
# - 「项目」tab → 点 erp 卡 → 项目 detail（L3 + L2 + 交付）
# - 「交付」tab → 点 task 卡 → 交付 detail（接收路径 / 拒绝理由）

# 磁盘 quota（默认 5 GB / project）：
FUXI_PROJECT_DISK_QUOTA_BYTES=2147483648 fuxi im start  # 2 GB
# spawn 时若超过 → publish WorkspaceQuotaExceeded(DiskBytes) + 拒绝
# PWA 项目 tab cards 后续可 surface 此 event（v1 仅事件留痕）
```

---

## 5 · 踩坑预防

- **`FUXI_L2_GC_INTERVAL_SECS` 测试场景下设小值**：production 1h，e2e 测试可设 10s
  让 archive 几秒后就 GC 到。注意 `FUXI_L2_GC_MAX_AGE_SECS` 也要同步调小。
- **磁盘 quota 缓存 60s**：`FUXI_PROJECT_DISK_CACHE_SECS` 可调。GC 删大量文件后
  spawn 会用旧缓存值——若需立即生效，调 `Fuxi::invalidate_disk_quota_cache(project_id)`。
- **L2 路径反查严格匹配 `<root>/<project>/ephemeral/<task-display>/`**：自定义
  ProjectRegistry root 时若结构变形，bridge 不会触发自动 archive。
- **NavRoute kind 跨 tab 校验**：`navPush({ kind: "project", ... })` 在交付 tab
  下被 ApiProvider 拦截（防呆，避免 App.Switch 渲染错位）。
- **同 task 重复 fuxi spawn --ephemeral**：会触发 EphemeralWorkspaceManager::create
  AlreadyExists——CLI 会报错；用户应换 task-id 而非复用。

---

## 6 · 决策快照（这次产生的）

无新公理 / 反公理。phase 3 是 Decision 21/22 既定路线兑现。

新「产品口味题」无 —— phase 3 全部 default 沿用 phase 1/2 已敲定值。
