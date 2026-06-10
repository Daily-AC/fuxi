# Handoff · v1-session20（2026-06-10）

> 上一会话（2026-06-09 深夜→06-10 凌晨）做完：救活卡死玄女 + 修 3 条 PWA issue 上线 +
> 玄女分身/持久队列设计→计划→agent team 实装→部署上线全闭环。本文件给新会话开工指引。
> 细节见 memory `project_xuannv_clone_queue_2026_06_10` + spec/plan/FOLLOWUP（见文末）。

## 已上线状态（home dd7b965）

- **玄女救活**：之前「假活 Idle」僵尸（cc wedge，只剩 keep_alive 无 agent_responded，卡 28h），重启根治。诊断法：journalctl 看 cc 层只有 keep_alive 无真 turn + 进程 etime 跨天 → 重启 `fuxi-im.service`。
- **3 条 PWA issue 修复上线**（awaiting_test）：
  - `3b5b8f25` IM 图片附件 → in-app lightbox（fit-to-screen contain + 缩放/拖动/Esc）
  - `3871902a` bridge：AgentRequestReview 补内部 role 过滤 + AgentDead idle_ttl 不注入玄女
  - `1d816926` bridge：artifact 完工汇报无 DeliverableProduced 凭证 → 注入「未核实」打回门客
- **玄女分身 + 完工持久队列 core（块1-5）上线**：
  - 玄女单例 → 按 topic 分身池（懒启动 + dormant 回收 + LRU + 共享灵魂各自记忆）
  - 跨 topic 串味修复（`357da78a`，里程碑按 ev.meta.topic_id 路由到归属分身）
  - 完工信号持久队列 + respawn 补发（`a01cfab5`，dormant 分身完工落 `pending_xuannv_notifications` 表 + respawn drain）
  - general 镜像 reconciler 根治「玄女假活黑洞」
  - 烟测：玄女 general 分身 eager 自启 agent_ready、服务 active、migration 0009 落库
- **5 个 issue 全 awaiting_test**，等以琳 PWA 实测。

## 新会话待办（按优先级）

### P0 · 玄女分身 runtime 实测【最该先做】
昨晚无人值守部署只烟测了 boot（agent_ready），**分身真实运行行为没在 home 真 cc 上端到端验过**（单测绿 ≠ 线上真跑对）。必验：
1. PWA 开**第二个 topic** 发消息 → `fuxi list` 确认该 topic 玄女分身**懒启动**（出现第二只 xuannv）。
2. 某 topic 分身 idle 超 TTL → 确认 **dormant 回收**（进程没了不黑洞）→ 再发消息 → **respawn** 起来、不打死 id（昨晚 reconciler 修的黑洞，必须真验）。
3. topic A 派门客 → 切 topic B → 确认 A 完工**不串味**到 B；A 分身睡着时完工 → `sqlite3 ~/.fuxi/im.db "select * from pending_xuannv_notifications"` 看**落库** + respawn 后 **drain 补发**。

这是昨晚部署留的验证缺口，有问题趁早抓。

### P1 · 以琳实测 5 个 awaiting_test issue
PWA 逐个验（图片 viewer / 玄女噪音 / hallucinate 打回 / 完工不丢 / 不串味）。通过 → `fuxi issue close <id> --actor xuannv`；没过 → reopen。

### P2 · FU-2 worker-dispatch topic 精确归位（昨晚 revert 的 7.5）
**唯一动高危 cc spawn env 的活，必须白天有人盯。** 重做 `extra_env`/`FUXI_TOPIC`/`--topic` 链路（原 commit b9278fe，已被 revert b8cd32f），home 实测确认玄女不卡。现状无回归（worker 完工事件退化到 general 分身，跟单玄女基线一样）。⚠️ session-id 红线：spawn 一律 `session_id:None/resume:None`。指引见 FOLLOWUP.md FU-2。

### P3 · FU-1 push 通知
dormant 补发时发手机 push——现 push hooks（`fuxi-im::push::hooks::spawn`）绑死单 general xuannv_id，要改池感知（订阅 `xuannv_pool_watch` 全分身）或把 FCM sender 线进 `TopicXuannvSpawner` drain。纯增强不急。FOLLOWUP.md FU-1。

### P4 · 清 team 残留
`TeamDelete` 试清 `xuannv-clone`（上一会话 2 个 idle agent topic-field/queue-store 用文字回复没走 shutdown 协议强制不掉）。清不掉忽略，无害。

## 关键教训（多 agent 实装，下次注意）

1. **worktree 隔离对 team agent 没生效**：上一会话 4 个 agent 都在共享主检出跑，靠文件不重叠 + 顺序 commit 侥幸没冲突。下次并行实装**先确认隔离真生效**（`git worktree list` 看 agent 是否在独立 worktree），否则干脆串行。
2. **team agent 消息有 FIFO lag**（逐条慢半拍）：发指令要**幂等 + 少改主意**。上一会话 KEEP/revert 来回交叉导致 7.5 被 revert（最终接受，因 reverted 更安全）。
3. **高危 spawn 改动无人值守部署前 revert 是对的安全闸**：今早玄女才因 session bug 卡死，7.5 动 spawn 原语，深夜无人盯 → revert 出部署，白天重做。

## 文档指针

- spec：`docs/superpowers/specs/2026-06-10-玄女分身-持久队列-design.md`
- plan：`docs/superpowers/plans/2026-06-10-玄女分身-持久队列.md`（5 块 TDD）
- follow-up：`docs/superpowers/plans/2026-06-10-玄女分身-FOLLOWUP.md`（FU-1 push / FU-2 worker-dispatch topic）
- 关联 memory：`project_xuannv_clone_queue_2026_06_10` · `reference_home_deploy` · `reference_cc_version_pin`
