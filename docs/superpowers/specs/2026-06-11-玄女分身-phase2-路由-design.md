# 玄女分身 Phase 2 · 用户对话侧路由 · 设计

> 前作：`2026-06-10-玄女分身-持久队列-design.md`（块1-5 已 ship，覆盖门客事件侧）。
> 本篇兑现该 spec 第 9 行欠的另一半：**用户切 topic 从「kill+spawn+灌回顾」改为
> 「独立常驻分身进程 + current_topic 路由」**。玄女自己提的反馈（v1-session21 坐实）。
> 立于 2026-06-11。

## 1. 现状问题（2026-06-11 实勘代码）

门客事件侧已全部 pool-aware（bridge 按 topic 路由 / 懒启动 spawner / 持久队列 /
dormant 回收 / push 通知）。用户对话侧仍是 Phase 1 单分身，且有三个实勘发现：

1. **精神分裂**：`topic_switch.rs::switch_topic_to` 切到 topic B 后调 `set_xuannv(new_id)`，
   新分身被绑到池的 **general 槽位**而非 B 槽位。若 bridge 之前为 B 懒启动过分身，
   池里出现两个分身同时服务 B——用户消息进一个，门客完工事件进另一个。
2. **latent bug（比串味狠）**：非 general 分身的回复**不落消息库**——
   `conv_store::spawn_xuannv_sync` 与 `conv_ws` 都只认单值 `xuannv_id_watch`。
   dormant 补发后分身主动汇报「门客干完了」，用户只收到 push，PWA 历史里没有这条话。
3. **LRU cap 空文**：`XuannvPool::lru_victim_if_over_cap` 写了、测了，生产**零调用**——
   超 `FUXI_XUANNV_MAX_ACTIVE` 没人回收。且现实现会选中 general 当 victim，
   与「general 永不回收」公理（9151f54）冲突。

另有子 bug：`topic_switch.rs::load_recent_messages` 拉回顾不按 topic 过滤（自承
"Phase 1 v1 过渡"），随老路径删除一并消亡。

## 2. 已拍板（2026-06-11 用户拍）

- **多端模型 = 全局单值 current_topic**：手机切 B，桌面/jarvis 语音/桌宠全跟着。
  不做每端独立话题视图；intervene API 不加 topic_id 参数。
- **未读小红点本期做**：跨 topic 汇报落对方历史 + push（必做）之外，
  sidebar 加未读提示。

## 3. 核心取舍：xuannv_id 兼容壳去向

- **甲 · general 镜像不动（采用）**：`xuannv_id()` 继续 = general 分身镜像，
  平台级兜底语义（idle_gc fallback / bridge general 兜底 / 503 判断）零变化。
  用户消息路由全部改走 `current_topic → 池`。
- 乙 · 彻底删单值：vision/voice/context watcher/handoff watcher 全要动，收益边际，否。
- 丙 · 单值语义改「当前 topic 分身」：watch 每次切 topic 漂移，十几个订阅者语义全乱，否。

## 4. 改动六块

### 4.1 switch_topic 重写（杀 kill+spawn 老路径）

`switch_topic_to` 变成：`set_current_topic(B)` + `ensure_xuannv_for_topic(B)`。

- 池有活分身 → 毫秒级秒切，上下文真留内存，不重灌回顾。
- 池无 → 懒启动（复用 `TopicXuannvSpawner`：topic 过滤回顾 + FUXI_TOPIC env +
  drain 持久队列），HTTP handler 同步等 ensure 完成再返回。
- **不杀旧分身**（留给 idle_gc dormant）、**不等 idle**（旧分身忙就跑完，
  输出落它自己 topic）。
- `switch_topic_to` 不再调 `set_xuannv` / `shutdown_xuannv_for_handoff` /
  `wait_xuannv_idle`；`XuannvSwitcher` trait doc 契约同步改写。
- 前端 overlay「玄女正在重新接班（5-15s）」只在懒启动慢路径出现（热路径秒返，
  overlay 一闪而过即可，文案顺手改）。

### 4.2 用户消息路由

intervene（缺省 target）/ dispatch / voice / vision 的玄女 fallback：
`xuannv_id()` → `ensure_xuannv_for_topic(current_topic_id())`。

- dead-target fallback（@已下线门客转玄女）同样落当前 topic 分身。
- ensure 返 None（spawner 未注入 / spawn 失败）→ 503 语义保留。

### 4.3 精神分裂修复

switch 不再绑 general 槽位 → general 槽位永远真的是 general 分身。
每个 topic 只有自己槽位一个分身，用户消息和门客完工事件进同一个进程。

### 4.4 PWA 可见性 pool-aware（修 latent bug 2）

- `spawn_xuannv_sync`：过滤从「agent == 单值 xuannv_id」改「agent ∈ 池任一活分身」
  （订 `xuannv_pool_watch`）；消息 topic 归属优先 `ev.meta.topic_id`，
  缺失兜底 `pool.topic_of(agent)`，再兜底 current_topic。
- `conv_ws`：filter 改「事件归属 topic == current_topic 实时值」——B 分身的汇报
  不插进 A 的视图（走落库 + 小红点 + push），切 topic 后 WS 不断连自动跟随。
  503 判断继续用 general 镜像（general 永驻，语义不变）。

### 4.5 未读小红点

- 后端：topic 级 `last_read_at` 水位（im.db migration，跟现有表同库同 pattern）；
  `POST /api/topics/:id/read` 标已读；`GET /api/topics` 每行带 `unread_count`
  （按 `messages.topic_id` + `ts > last_read_at` 计数）。当前 topic 视为自动已读。
- 前端：sidebar badge + 进话题时调 read。

### 4.6 LRU cap 真执行

- 入池后（`set_xuannv_for_topic` 出口或 spawner 入池处）查
  `lru_victim_if_over_cap` → victim 走 dormant（pool.remove + shutdown）。
- 补两个豁免：**victim 选择排除 general**（与永驻公理对齐，改在
  `lru_victim_if_over_cap` 内）；**victim 正 busy 则本轮放过**（软上限，
  不丢正在跑的 turn，等 idle_gc 收）。busy 判断要 Shelf，执行逻辑放 Fuxi 层。

## 5. 不动的（YAGNI / 留 issue）

- `xuannv_context` 水位 watcher + handoff watcher 仍只盯 general 镜像——
  非 general 分身上下文爆了不触发 handoff。开 issue 留 follow-up
  （per-clone watcher 是另一套订阅架构）。
- 不做跨 topic 记忆共享 / 跨节点分身（前作 spec 非目标继承）。

## 6. 测试与验收

TDD 核心回归：

- switch 热路径分身 id 不变（不 kill、不重灌回顾）；dormant 切回懒启动 + drain pending。
- 缺省 intervene 路由到 current_topic 分身（非 general 镜像）。
- **非 general 分身 AgentResponded 落库带正确 topic**（latent bug 回归）。
- conv WS 跟随切 topic 实时换归属过滤；B 分身汇报不进 A 视图。
- LRU victim 排除 general / busy 放过；超限 idle victim 被 dormant。
- 未读：B 分身来消息 unread_count>0；read 后清零；当前 topic 不计未读。

收口 home 部署实测：手机 PWA 秒切、跨 topic 完工汇报落对方历史 + 小红点 + push、
general 永驻不回收。

## 7. 实装拆解（供 writing-plans / agent team）

1. **块A 路由与切换**：switch_topic_to 重写 + XuannvSwitcher 契约 + intervene/
   dispatch/voice/vision fallback 改 ensure(current_topic)。fuxi-cli + fuxi-im。
2. **块B PWA 可见性**：spawn_xuannv_sync pool-aware + conv_ws topic 归属过滤。fuxi-im。
3. **块C 未读小红点**：migration + topic_store 水位 + handlers + web sidebar。fuxi-im + web。
4. **块D LRU cap**：victim 豁免 + 执行点。fuxi-orchestrator。
5. **块E 集成收口**：跨块 e2e + home 实测。

依赖：A/B/C/D 相互独立可并行（A、D 都碰 fuxi.rs 但不同函数，冲突面小）；E 收口。
