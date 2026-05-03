# Handoff · v1 · Session 8 开工指引

> 上一 session（2026-05-03 凌晨/早晨连续推）—— Decision 21/22 phase 3 全部
> 「下一动作」清单清零：B 跨节点 sandbox + C tag-aware spawn + D accept_deliverable
> 严格模式 + A 跨 tab 跳转 + ProjectDetailPage / DeliverableDetailPage。
> 上份 handoff: `docs/handoff/v1-session7.md`（保留）。

---

## 1 · 10 分钟必读

1. `CLAUDE.md` · 七公理（不变）+ agent team / decision-split 协作（3 min）
2. **`docs/decisions/21-workspace-design.md`** · 五层 + 三 phase（3 min）
3. **`docs/decisions/22-deliverables-storage.md`** · 文件级交付（3 min）
4. `roles/xuannv/instructions/dispatch-routing.md` · 派活路由 + L2/L3 + 跨节点
   project sandbox 的「玄女视角教学」（5 min）
5. 本文 §3 "下一动作"

---

## 2 · 上一 session ship 了什么

phase 3 收口（13 个 commit · session 7 后）：

后端跨节点 sandbox（最大块 ~350 LoC）：
- `DistJob` / `DistEnqueueReq` 加 `project: Option<String>` + `ephemeral_task: Option<String>`
  serde(default + skip_serializing_if) 全兼容老 wire
- `DistController::enqueue` 拆 wrapper：旧 10 参 enqueue 保兼容 ~20 测试 callsite，
  新 enqueue_with_project 走全形参
- `DistGatewayConfig` 加 role / project / ephemeral_task；spawn_by_role 在走
  dist gateway 路径前从 project_override / ephemeral_task_override 填进 cfg
- `DistGatewayAgent.dispatch` 把 project / ephemeral_task 写进 enqueue 请求
- worker 端 `resolve_project_sandbox_cwd(projects_root, job)` 反查本机
  ProjectRegistry 拿 sandbox 路径，run_codex_job / run_cc_job 设 Command.current_dir
- `DistWorkerArgs` 加 `--projects-root`（默认 $HOME/.fuxi/projects/）

后端 tag-aware spawn（Decision 21 phase 3 配套）：
- `apply_project_tag_to_required_tags` helper：dist 路径下 `--project erp`
  自动加 `project:erp` 到 required_tags，让 controller 路由到带该 tag 的 worker
- 玄女 dispatch-routing.md 补「跨节点项目 sandbox」段教 worker 端 `--tag project:erp`

后端 deliverable strict mode：
- `FUXI_DELIVERABLE_STRICT=1/true/yes/on` 开后 accept_deliverable 的 accepted_to
  必须在 $HOME 子树（双层 canonicalize 防 .. / symlink 绕过）
- 系统目录黑名单仍优先（双层防护）；$HOME 缺时 strict 开 → 500
- Web 端攻击者再也写不出 $HOME 范围（适合 systemd 跑 fuxi-im 暴露公网场景）

前端跨 tab 跳转（A polish）：
- ApiProvider 加 `navTo(route)`：按 kind 解析目标 tab + setActiveTab + navPush 原子
  绕过 setActiveTab 的"切到无 nav tab 时清栈"逻辑避免 race
- ProjectDetailPage DeliverableSummaryRow 改 button + navTo 真跳转

测试：
- 后端 cargo test --workspace 全绿（除 dist::cancel_in_silent_period_via_heartbeat_ack
  已知 timing flake）—— 470 → 477 passed
- PWA pnpm test 306/306（含新增 navTo 跨 tab 测试）
- 4 个新 resolve_project_sandbox_cwd · 1 个 strict mode (5 sub-cases) · 3 个
  apply_project_tag · 2 个 navTo

---

## 3 · 下一动作（按价值排）

### A. PWA polish（S）

- 项目 detail 加 "L2 promote → L3" 按钮（后端 `Fuxi::promote_l2_to_l3` 已就位）
- 交付 detail 加 manifest.json 原文折叠区
- L2 archive row 加 "复活 / restore" 按钮（先要后端实装
  `EphemeralWorkspaceManager::restore`）

### B. 节点 tab 显 project tags（S）

`fuxi dist worker --tag project:erp` 起的 worker，在 PWA 节点 tab 卡片上 surface
其 tags（含 `project:erp`）。让用户一眼知道该 worker "托管哪些项目"。后端
`/api/nodes` 已有 `tags: string[]` 字段，前端 NodesPage 可能没显示——确认 + 加。

### C. 跨节点 deliverable 拉回（M）

worker 节点跑 `fuxi deliverable produce` 写在 worker 本机 sandbox 里，home 端
PWA 看不到。需要：
- worker 端 produce 后通过 dist event bridge 把 DeliverableProduced 事件回流 home
- home 端 deliverable bucket "镜像"区（`deliverables/<task>/__from_<node>__/`）
- accept 时按 source_node 走 SCP / rsync 拉文件到 home（或 Web 端用户主动下载
  worker 直链——需要 worker HTTP 暴露同 fuxi-im /api/deliverables 路由）

phase 4（B 路径）大块。

### D. spawn_by_role 集成测试（S）

当前 daemon 单测只覆盖 build_dist_gateway_config + apply_project_tag 各自，没测
spawn_by_role 端到端"--project erp 走 dist 路径时 tag + cfg.project 都被填"。
加一个 integration test 用 mock Fuxi 验证。

### E. 玄女 PWA 节点匹配 hint（S）

玄女在 dispatch 之前可能想知道"哪些 worker 现在 advertise 了 project:erp"。
新加 `Fuxi::list_dist_workers_for_project(project_id) -> Vec<NodeView>` 让玄女
spawn 前先查；若空 → 不直接 spawn 而是先 nudge 用户去 worker 上加 tag。

---

## 4 · 用户实测路径（含 phase 3 全功能）

```bash
# === 本机场景（phase 1+2，沿用）===
ssh home && cd ~/fuxi && git pull && cargo build --release -p fuxi-cli
systemctl --user restart fuxi-im

fuxi project add ~/erp
fuxi spawn --role luban --project erp                          # L3 持久 sandbox
fuxi spawn --role luban --project erp --ephemeral --task $(uuidgen)  # L2 一次性

# === 跨节点场景（phase 3 新）===
# 在 worker（mac）上：
fuxi project add ~/erp
fuxi dist worker --node mac-laptop --tag project:erp \
    --tag local --controller https://home.qmledmq.cn

# 在 home 上派活：
fuxi spawn --role luban --project erp --node mac-laptop
# → controller 看 required_tags=[project:erp] → 派给带该 tag 的 worker
# → worker 解 project=erp 找本机 ~/.fuxi/projects/erp/sandboxes/luban/
# → cc 在那里跑

# 用户接收 deliverable + strict 模式：
FUXI_DELIVERABLE_STRICT=1 systemctl --user restart fuxi-im
# PWA 接收路径必须填 $HOME 子树否则 400
```

---

## 5 · 踩坑预防

- **跨节点 spawn 工作流强依赖 worker 上 `fuxi project add`**：home 派 job 带
  `project=erp`，worker 端 ProjectRegistry 没注册 → fail job 明确报错（vs silent
  fallback 让用户找不到为什么文件没落对地方）。错误信息提示用户去 worker 上跑
  `fuxi project add <path>` 注册同名 slug。
- **dist worker 必须 advertise `project:<slug>` tag** 才能拿到带该 tag 的 job。
  忘加 tag → controller 拿不到合规 worker → job 卡 queue（PWA 节点 tab 看到
  inflight=0 但 queue depth>0）。
- **strict mode env 不全局**：`FUXI_DELIVERABLE_STRICT` 通过 systemd 注入要写
  `Environment=` 不是 `ExecStart=` 命令行 export（subshell 不继承）。
- **L2 worktree 同 task 重复 idempotent**：worker restart / job 重发 corner 都
  复用现有 ephemeral 路径，不报 AlreadyExists。但 home 端 spawn 仍 per-task 一次，
  不应靠这个做去重。

---

## 6 · 决策快照（这次产生的）

无新公理 / 反公理。phase 3 三块（B 跨节点 / C tag-aware / D strict mode）全部
按 Decision 21/22 既定路线兑现。

设计取舍记录两条：
1. **跨节点 sandbox 走 strict fail vs silent fallback**：worker 节点没注册项目时，
   选 fail job 让 home 用户明确知道是 worker 配置缺失。silent fallback 到默认
   cwd（无 project sandbox）会让用户怎么也找不到 produce 的文件去哪了。
2. **strict mode 默认关闭**：本机自用场景大量用户存 `~/写作`、`/tmp/draft` 等
   多种路径。env 开关让公网部署能选择"只允许 $HOME"，本机自用免配置。
