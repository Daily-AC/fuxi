# Handoff · v1 · Session 14 开工指引

> 上一 session（2026-05-06 凌晨~上午）核心是 **v2 收尾 + bug 三连修**：v2 跨节点
> 设计正解（home 内嵌 worker，codex 帮做的）+ orphan task sweep 启动期兜底 +
> codex 静默失败诊断 emit。全部 ship + home 实地真测验证。
>
> 上一份 handoff：`docs/handoff/v1-session13.md`（保留，§3 答辩 dry-run 步骤仍有效）。

---

## 1 · 上 session ship 了什么（按时间序）

**HEAD `872e6e9`**，全绿（fmt + clippy + tests）。

| commit | 内容 | 真测验证 |
|---|---|---|
| `e5253ca` | feat(home-worker)：fuxi-im 同进程内嵌 home dist worker（v2 P1 修） | ✓ home job 真被消费，cc spawn → done 4 秒 |
| `f3b1bb5` | fix(orphan-sweep)：启动期扫非终态 task 兜底 emit TaskCancelled | ✓ home 部署后扫到 5 条 orphan 全 cancel（含 v1-session12 §7 报的 task-fb7437a8） |
| `872e6e9` | fix(codex-agent)：codex 静默失败兜底 emit AgentResponded 诊断 + AgentDead | 单测覆盖（unix-only `/bin/sh -c` 模拟），真测路径要等下次 luban-codex 真失败时验 |

家底（部署在 home）：
- binary md5 `7e3b4bf8...` 在 `~/.local/bin/fuxi`
- fuxi-im 进程跑着，PWA 正常
- demo-site 项目存在，host_nodes=`[home, zyldemacbook-pro-local]`，可直接接答辩演示

---

## 2 · 当前剩余 bug 清单（按优先级）

### 🚨 P1 · task push back 没自动（**下 session 主线**）

**真因**：worker 跑完 task 不自动 `git push origin <branch>` 回 home，导致跨节点
协作 home 端**永远看不到 mac 改的代码**。v1-session13 §3 列过但没修。

**审查结论**（已审，确认未修）：
- `ephemeral_workspace.rs::archive` (L2)：只 rename + git worktree prune，无 push
- `persistent_sandbox.rs` (L3)：根本无 task-done hook
- `dist.rs::run_cc_job` / `run_codex_job`：跑完 → `ctrl.report` → done，无 push

**修法**（小）：
1. 加 `fuxi-workspace::try_push_default_branch(canonical, branch) -> bool` helper
   （对称 v2-session13 加的 `try_fetch_default_branch`，best-effort log warn）
2. dist worker 在 `ctrl.report` **之前**调一次 push（覆盖 L2 + L3 + 跨节点一条路径）
3. 失败非致命：home 端用户手动 `ssh mac 'git push'` 兜底

**TDD 入口**：tempdir 起两 repo（home 源 + worker clone），worker 写一条 commit
后调 push，验证 home 端能 `git log origin/<branch>` 看到。

**触发场景**：home 派 task 给 demo-site，auto-pin 路由到 mac，mac luban 改 code，
当前 home 端看不到改动；要的效果是 task 终态后立刻能 `cd ~/demo-site && git log
fuxi/<task-id>` 看到 mac 推回的 branch。

### P2 · cc agent idle latency（一次性角色 600s 才 dead）

cangjie / 单 task luban 这种"一锤子"角色跑完后等 idle GC 10 分钟才 emit AgentDead。
PWA 看到僵尸 idle 门客 ~10 分钟。

**真因**：cc adapter 是 long-lived 设计（支持 follow-up），等 stdin EOF 才退；
fuxi 这边没关 stdin，只能等 IdleGcTask ttl_secs=600 触发。

**修法选项**（要决策）：
- A. role-specific TTL：cangjie/extractor 等一次性角色 ttl=60s
- B. cc adapter 加"task done 后 N 秒无 follow-up 自杀"逻辑
- C. 不修（PWA 加"已完成但等清理"icon 提示）

不阻塞 v2 演示，留给用户决策。

### P2 · sia/ephemeral/task-86106710 物理 leftover

数据脏不影响功能。一刀清：`ssh home 'mv ~/.fuxi/projects/sia/ephemeral/task-86106710-* ~/.fuxi/projects/sia/archive/'`

### P3 · PWA composer 没 `@<slug>` mention parser

后端 `/api/dispatch` body.project 已支持（commit `ef6480e`）。前端 UX 缺。
功能补完，不是 bug。

### P3 · NodeLoadProvider 只看 inflight/concurrency

设计取舍，当前规模够。v3 综合 CPU 信号。

### P3 · `cancel_in_silent_period_via_heartbeat_ack` 测试 flaky

stash baseline 下也挂（与代码无关）。pickup 5s 超时在并行 cargo test 高 load
下偶发。需要从 timing 改设计——不阻塞。

---

## 3 · 已审查、确认非 bug

- 玄女 5/5 18:57 抱怨「agent 退出没 emit AgentDead」 —— 实际 14 秒后就 emit 了，
  是上面 P2 cc idle latency 问题，不是没 emit
- 「FUXI_CODEX_MODEL 静默失败」 —— `872e6e9` 已修
- 「orphan task 永卡 running」 —— `f3b1bb5` 已修

---

## 4 · 下 session 推荐起点

**主线**：修 P1 task push back 自动（30-50 行 + 单测 + 真测）。
**步骤**：
1. fuxi-workspace 加 `try_push_default_branch` helper（对称已有的 fetch 助手位置）
2. 单测：tempdir 双 repo + 验 push 成功
3. dist.rs `run_cc_job` / `run_codex_job` 在 ctrl.report 前调
4. 真测：home 派 task → mac 跑完 → 验 home 端 `git log origin/fuxi/<task-id>` 能看到
5. handoff 收尾

不重要的可顺手清：sia/ephemeral leftover 一行 mv 命令的事。

---

## 5 · 协作笔记

- 用户偏好已落 memory（feedback_full_bypass / no_ceremonies / keep_going / tdd_required）
- 上 session 全程 TDD 红→绿。本 session 三个 fix 都先写测试，cargo test 再实装
- **真测** 不靠运气：每条 fix 都 rsync home + cargo build + 重启 fuxi-im + 验 events.db / log。3 次撞到"binary 没真 cp 上"的坑（fuxi-im 跑着 cp 失败但 ssh exit 0），写 ssh 命令时记得 `pkill -9 + sleep 3 + cp -fv`
- agent team 模式没用——本 session 串行做，每修 ≤ 200 行紧密依赖前一步，团队拆开会 round-trip 多
