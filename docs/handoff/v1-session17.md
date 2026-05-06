# Handoff · v1 · Session 17 → 18 开工指引

> 本 session（2026-05-07 凌晨）核心 = **task #9 PWA 4 tab + 「更多」hub 重构 + 三新 sub-page**。
> 用户拍 §2.2 方案 A：玄女 / 任务 / 通知 / 更多——原一级 tab 节点 / 项目 / 交付物
> 沉到 hub 二级，新加记忆 / 角色 / 更漏 / 设置共 8 张 tile。
>
> 上一份 handoff：`docs/handoff/v1-session16.md`（保留，§4 任务 #8 路线、§7 部署速记仍有效）。

---

## 1 · 本 session ship 了什么（HEAD 待 commit）

| 模块 | 内容 | 真测验证 |
|---|---|---|
| 后端 3 端点 | `/api/memory` `/api/roles` `/api/cron` + handler + 9 单测 + production wiring | home `curl` 三 endpoint 各返真数据 |
| PWA 4 tab | TabIndex 0\|1\|2\|3 + BASE_TABS 改成 [玄女/任务/通知/更多] + MoreSubRoute 状态 + setMoreSub | BottomTabBar 7 测 + ApiProvider-nav 9 测 |
| 「更多」hub | MorePage（8 tile grid） + MoreSubShell（公共 ‹ 更多 返回 wrapper） | MorePage 4 测 + MoreSubShell 3 测 |
| 三新页 + Settings stub | MemoryPage / RolesPage / CronPage / SettingsPage | Memory 2 测 + Roles 2 测 + Cron 2 测 |
| 既有页迁移 | NodesPage / ProjectsPage / DeliverablesPage 沉到 hub 二级；既有测试 initialTab + initialMoreSub 对齐；ProjectDetailPage navTo 仍走 navTo helper（atom 化 setActiveTab(3)+setMoreSub("deliverables")+navPush） | 350/350 unit + e2e smoke spec 改造 |

### 1.1 后端 9 测全绿

- `cargo test -p fuxi-im --lib handlers::memory::tests` · 3 测
- `cargo test -p fuxi-im --lib handlers::roles::tests` · 4 测
- `cargo test -p fuxi-im --lib handlers::cron::tests` · 2 测
- `cargo test -p fuxi-memory list_active` · 1 测（OracleStore::list_active 跨 subject 列现行事实新方法）

### 1.2 前端 350/350

```
$ pnpm -C crates/fuxi-im/web test
 Test Files  39 passed (39)
      Tests  350 passed (350)
```

新增测试文件：MemoryPage / RolesPage / CronPage / MorePage / MoreSubShell。

### 1.3 真测部署

```bash
$ ssh home 'md5sum ~/.local/bin/fuxi'
829faba6e610428032ad1e8c1c1074d2  /home/e0-7/.local/bin/fuxi

$ TOKEN=$(ssh home '$HOME/.cargo/bin/fuxi im issue-token | head -1')
$ ssh home "curl -s -H 'Cookie: fuxi_im_token=$TOKEN' http://127.0.0.1:9100/api/memory | head -c 300"
{"groups":[{"subject":"xuannv","facts":[{"id":"...","subject":"xuannv","predicate":"session_id","object":"21e2c080-...","source":"im-bootstrap",...}]},{"subject":"role-xuannv",...}], "total": ...}

$ # 同样 /api/roles 返 cangjie/extractor/luban/xuannv/zhudiesi 五卡
$ # /api/cron 返「1分钟到，喊以琳一声」once trigger
```

---

## 2 · 重要架构决策（task #9 落地路径）

### 2.1 4 tab 收口选择

放弃 5 tab（玄女/任务/通知/更多/设置）+ tile 7 项的方案——「更多」自带"杂物间"语义，
设置作为最后一张 tile 反而更整齐；通知是 first-class（每天看），值得 tab 一级。

### 2.2 navRoute / moreSub 双信号方案

`navRoute`（任务 thread / 项目 detail / 交付 detail）只管 L2 详情 push；`moreSub`
（hub L1 sub-page）独立信号。设计取舍：

- 不上 NavRoute 数组栈（pages 1+ 推叠）——NavigationStack 是 2 layer，加栈
  得递归 NavigationStack，复杂度↑而本期只有 hub→sub→detail 三层一种深度
- 拆成 (tab, moreSub, navRoute) 三段元组——清栈逻辑 setActiveTab/setMoreSub
  各管半边，pop 优先级 navRoute → moreSub → tab，未来扩展 sub 不破坏现状

### 2.3 navPush gating 规则

`navAllowed(tab, sub)`：
- tab 1（任务）→ 永远开（kind=task / worker）
- tab 3（更多） + sub=projects → 开（kind=project）
- tab 3（更多） + sub=deliverables → 开（kind=deliverable）
- 其他全 deny → noop（misuse 防御）

非匹配 (kind, location) 也 noop：`navPush({kind:"project"})` 在 sub=memory 下不会渲染。

### 2.4 离开「更多」自动清 sub

切 tab 0/1/2 → 强制清 moreSub 回 hub 首页。再点 tab 3 进入 = 回首页（不记上次 sub）。
理由：移动端用户预期"再点 tab = 回首页"，记忆上次深处反而难退出。

---

## 3 · 下 session 推荐起点

用户在 session 16 末尾的两条候选里 **task #8 上下文管理** 是剩下的核心。
session 16 §4 已写好 task #8 实装路线，本 session 完成 task #9 后，PWA 「通知」
tab 已稳，task #8 emit 的 `kind=context_handoff_offer` 通知可以直接走通知 tab。

详见 `docs/handoff/v1-session16.md` §4——4 块（events / orchestrator / cli / agent-cc）
~250-350 行，35% / 45% 跨阈值不同动作，handoff 写入 → 检测落档 → kill old + spawn new
注入 prelude → 新玄女接班。

---

## 4 · 「更多」hub 内部布局速记（前端起新页参照）

```
tab 3 = 更多
├─ moreSub=null → MorePage (tile grid 8 张)
├─ moreSub=nodes → MoreSubShell{hideTitle} > NodesPage (子页自带 header)
├─ moreSub=projects → MoreSubShell{hideTitle} > NavigationStack(ProjectsPage, top=ProjectDetailPage if navRoute=project)
├─ moreSub=workers → MoreSubShell > 引导 stub (节点入口)
├─ moreSub=deliverables → MoreSubShell{hideTitle} > NavigationStack(DeliverablesPage, top=DeliverableDetailPage)
├─ moreSub=memory → MoreSubShell > MemoryPage
├─ moreSub=roles → MoreSubShell > RolesPage
├─ moreSub=cron → MoreSubShell > CronPage
└─ moreSub=settings → MoreSubShell > SettingsPage
```

`hideTitle=true` 给已有 `<header><h1>` 的子页防双标题；新页（Memory/Roles/Cron/Settings）
没自带 header，由 shell 渲 title。

---

## 5 · 已知差距（继承 session 16 §6）

本 session 没改：
- **P2 home inflight=4 stale leftover** —— worker crash 不 report 的残留
- **P2 sia/ephemeral/task-86106710 物理 leftover** —— 一刀清
- **P3 cancel_in_silent_period_via_heartbeat_ack** —— 仍 flaky（本 session 跑也挂，不阻塞）
- **P3 NodeLoadProvider** —— 仅 inflight/concurrency

新出现：
- **`fuxi cron list` PWA 端 phase 2** —— 当前只读，CRUD 仍走 CLI。移动端添加
  trigger 体验差（cron expr 难输），暂保留 CLI-only。
- **MemoryPage 仅展示 OracleFact** —— 不涉 hetu_patterns / user_profile。
  二者前端展示要看 hetu_patterns 设计未定（"心法"展示形式？）
- **PWA 「更多 → 工作者」是 stub** —— 引导用户去节点 tile。后续若加 worker
  全局列表（脱节点视角），重写本 stub。

---

## 6 · 协作笔记 / 部署速记

- **TDD 全程**：handler 后端 9 测先于真注入；前端 350 测先于 home 部署
- **fuxi-memory + fuxi-scheduler 加进 fuxi-im deps**：原本 fuxi-im 独立于这两 crate，
  本期为 PWA「更多」hub 数据源加上。无循环依赖（fuxi-cli 顶层依赖 fuxi-im + fuxi-memory + fuxi-scheduler）。
- **`scripts/deploy-home.sh` patch**：第 3 步 pnpm install 在无 TTY 下需 `CI=true`
  跳过 modules 删除确认提示，否则 ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY
- **OracleStore 加 list_active 方法**：跨 subject 拉现行事实，不抛 pool。在
  fuxi-im 端不 sqlx 直查避免破封装。
- **roles_root 注入路径**：`fuxi-cli/src/im.rs` 默认 `cwd/roles`，env `FUXI_ROLES_ROOT`
  覆盖。home rsync 把 `roles/` 全 sync 过去，cwd `/home/e0-7/fuxi` 时 `./roles/*` 可读。
- **ssh home ProxyCommand 抖动（v1-session15 §4）**：本 session 又踩——第一次 deploy
  rsync 失败，重试通过。下次再 fail 优先 retry，不要紧追 root cause。

### 部署状态快照（接班可直接验）

```bash
# home fuxi-im 进程
ssh home 'pgrep -lf "fuxi im start"; md5sum ~/.local/bin/fuxi'
# 期望：md5 = 829faba6e610428032ad1e8c1c1074d2, PID > 2762577

# 新端点 smoke
TOKEN=$(ssh home '$HOME/.cargo/bin/fuxi im issue-token' | head -1)
ssh home "curl -s -H 'Cookie: fuxi_im_token=$TOKEN' http://127.0.0.1:9100/api/memory | head -c 200"
ssh home "curl -s -H 'Cookie: fuxi_im_token=$TOKEN' http://127.0.0.1:9100/api/roles | head -c 200"
ssh home "curl -s -H 'Cookie: fuxi_im_token=$TOKEN' http://127.0.0.1:9100/api/cron | head -c 200"
```

PWA 强刷：DevTools → Application → Service Workers → Unregister → 刷新；或卸载重装 PWA。
