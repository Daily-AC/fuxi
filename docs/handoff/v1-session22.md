# Handoff · v1-session22（2026-06-10 深夜→11 凌晨）

> 本会话把 v1-session21 的头号待办「玄女分身 Phase 2」全量 ship：设计 → agent team
> 四块并行实装 → home 部署实测 → 实测中又抓到一个 CC 噪音 bug 当场修掉。
> spec `docs/superpowers/specs/2026-06-11-玄女分身-phase2-路由-design.md` ·
> plan `docs/superpowers/plans/2026-06-11-玄女分身-phase2-路由.md`。

## 已 ship 上线（main + home 部署，全 TDD + home 实测）

1. **`12f8612` 玄女分身 Phase 2 主 merge**（四块，agent team 并行）：
   - 块A：`switch_topic_to` 重写——**不杀进程**，`ensure_xuannv_for_topic` +
     `set_current_topic`；intervene/dispatch/vision 缺省玄女入口全改
     `current_topic → 池` 路由。顺手修了计划漏列的 `daemon.rs::handle_switch_topic`
     第二调用点 + 删 `wait_xuannv_idle` 死码。
   - 块B：conv sync / conv WS 改 pool-aware——**修「非 general 分身回复不落消息库」
     latent bug**（dormant 补发的主动汇报之前只有 push、PWA 历史里根本没有）。
     消息 topic 归属：`meta.topic_id` → 池槽位反查 → current_topic 兜底。
   - 块C：未读小红点——`topic_read_watermarks` 表（migration 0010）+
     `GET /api/topics` 带 `unread_count` + `POST /api/topics/:id/read` +
     sidebar badge（accent 变量、无 emoji）。
   - 块D：`FUXI_XUANNV_MAX_ACTIVE`（默认 3）**真执行**——此前 `lru_victim_if_over_cap`
     生产零调用。victim 排除 general（永驻公理）+ busy 软放过 + remove 先于
     shutdown（豁免顺序同 idle_gc）。`FuxiConfig.xuannv_max_active` 测试注入口。
2. **`a159fa5` 分身自我抄送噪音修**（home 实测当场抓的）：对分身 intervene 触发
   「抄送 general」→ general 分身白跑一轮回 "I'm caught up" + general 历史进 CC
   task_card。PWA 在非 general topic 说每句话都会触发。修：两处抄送点加
   `is_active_clone` 豁免；门客抄送照旧（公理 #2 反向回归测试兜底）。
3. **`1b7f7dc` CLI help 文案**随 Phase 2 改语义（**未部署**，下次部署自然带上）。

## home 实测证据（2026-06-11 00:52-01:07）

- 热切 **9ms / 8ms**，冷切懒启动 ~2s（`topic 切换完成（常驻分身，未 kill）`）。
- B 分身回复落 topic B 的消息库（latent bug 修复实锤，两轮验证）。
- dormant→respawn 的回顾记忆闭环：重启杀全分身后切回 B，新分身**记得上一轮对话**
  （topic 过滤 prelude 生效）。
- CC 修后：MARK 之后 general 0 新消息、0 `orchestrator_cc_received` 事件。
- LRU cap 实机触发：第 4 个活分身入池瞬间最老非 general 被 `xuannv_cap_lru` 回收，
  general 全程未动。
- 未读 SQL 链路：watermark 后非 user 消息 count=2。
- 服务重启后 general 自启正常（`pool_size=1 topic=general`）。

## 等以琳手机实测（PWA 用户侧）

1. 切话题应**秒切**（不再有 5-15s「重新接班」长等；懒启动冷路径文案已改
   「正在唤醒该话题的分身…」）。
2. 在 A 聊天时 B 话题来完工汇报：A 视图不插话、侧栏 B 亮**未读小红点**、push 照常、
   切到 B 能看到汇报历史。
3. 进话题后小红点消失（前端切换时自动 mark read）。

## 其他开口 / 注意

- **per-clone 水位 watcher 缺口**（spec §5 留的 issue，本会话已 `fuxi bug` 上报）：
  `xuannv_context` 35%/45% 水位监控 + handoff watcher 仍只盯 general 镜像——
  非 general 分身上下文爆了不触发 handoff。follow-up 要做 per-clone 订阅。
- **mac 主树有别人的 WIP**：`fuxi-wake-server/src/engine/xfyun/linux.rs`（xfyun
  session 回收，v1-session21 提的唤醒治本线）未提交；且 **main 上该文件 committed
  版本 `cargo fmt --check` 不过**（全 workspace fmt 门禁因它挂，按 crate 检查可绕）。
  等那条线 commit 时顺手 fmt。
- agent team 复盘：worktree 隔离部分失效（a/b 落在主树、c/d 共享一个 worktree），
  靠文件域不重叠 + 禁 `git add -A` 没出乱子。下次 spawn 前先验证 teammate 真进了
  独立 worktree。
- 冒烟产生的测试 topic（`phase2冒烟-0054` / `cap测试C` / `cap测试D`）留在 home
  im.db，以琳实测完可归档。

## 文档/memory 指针

- spec：`docs/superpowers/specs/2026-06-11-玄女分身-phase2-路由-design.md`
- plan：`docs/superpowers/plans/2026-06-11-玄女分身-phase2-路由.md`
- memory：`project_xuannv_topic_phase2_independent_process`（已更新为 ship 快照）
- 上一份 handoff：`docs/handoff/v1-session21.md`
