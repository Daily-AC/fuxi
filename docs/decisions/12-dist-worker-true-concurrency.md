# Decision 12 · dist worker 走真并发，消费 cancel_pending

**日期**：2026-04-25
**状态**：规划中（即将由 P1 修复 team 实装）

## 背景

审查 `feat/fuxi-v0.1` 时发现：dist 协议层 `max_concurrency` 字段已实装（controller 端
`pull_respects_max_concurrency_greater_than_one` 测试存在 · `dist.rs:2076`），但 worker
loop 实际是**单 job 串行**——每次 `pull → adapter.run().await → report` 顺序跑，即便
`max_concurrency=N` 也只能 hold 1 个 in-flight job。

伴生 bug：`dist.rs:965-967` worker 心跳忽略 ack 的 `cancel_pending` 字段，注释说
"未来 worker 支持并发多 job 时再用 ack 路径定向 kill 某个 child"。
真在 worker 并发起来之前，cancel 只能靠 `push_progress` 的 `should_cancel`——
worker 长时间无输出时段（codex reasoning 深思）cancel 延迟秒级。

协议已表，实装跟不上 = 内部分叉。

## 决策

**worker loop 改真并发：每个 job 走 `tokio::spawn` 独立任务 + JoinSet 跟踪 + 心跳
ack 的 `cancel_pending` 触发对应 task 的 `CancellationToken`。**

不选保守的 "把 max_concurrency 限到 1"。

## 理由

1. **方向一致**：Phase 3-4 的 dist 协议明显朝并发设计。controller 已有 inflight ≤
   max_concurrency 的派工逻辑、heartbeat 双向对账、`cancel_pending` 字段——回退到
   strict-1 是逆方向。
2. **B 路 vision 需要**：用户已确定下一步要"完善 agent 分布式节点"+"agent team
   可视化（看远程外挂节点）"，单 job worker 在多 agent role 场景下吞吐线性死。
3. **静默期 cancel 修法天然结合**：worker 拿到 heartbeat ack 的 cancel_pending →
   对应 token cancel → adapter 的 `tokio::select!` 分支退出 → 即使 codex 在 reasoning
   也能秒级中断。否则得另开 `/dist/cancel-poll` endpoint，多一层协议。
4. **复杂度可控**：JoinSet + per-job CancellationToken 是 tokio 教科书模式，~50 行
   change，不是新轮子。

## 代价

- worker 真并发后，per-worker 资源（内存/文件描述符/cc 子进程数）随 max_concurrency
  线性涨——用户机器上跑 max_concurrency=8 + 每 job spawn cc，可能爆 RAM。**缓解**：
  默认仍是 1，要并发用户得显式调；CLAUDE.md 建议补一行"max_concurrency 上调前确认
  机器能 host N 个 cc 子进程"
- progress 多路复用要序列化：现在 `progress` map 已按 `job_id` 分桶，但 `next_seq`
  per-job 已 OK，主要风险在 push_progress 的 lock 粒度——并发 N 个 worker 同时 push
  同一 controller 时 `inner.lock()` 串行化。短期可接受，长期要拆 sharded lock。
- 测试面变大：要补 `worker_runs_two_jobs_concurrently` + `cancel_in_silent_period_via_heartbeat_ack`
  至少两个 e2e。

## 何时不适用 / 何时重审

- 若 cc 子进程的 OS-level 并发开销证实成 P50 瓶颈（rand benchmark 过来才能定），
  考虑改 worker 内 worker pool 共享一个 cc session（要 cc 支持多 session 复用，目前
  不支持，所以这个 override 的前置是 cc 端有 session pool 能力）。
- 若发现 controller 端 `inner.lock()` 在 N>4 worker 时成瓶颈，重审：拆为
  per-node sharded lock 或 lock-free queue。

## 参考

- 触发审查：本会话 2026-04-25 全量审查报告，dist.rs:965-967 / 533-534 / 1008,1025 /
  daemon.rs:734-740
- 相关 commit：`cbdd006` cc 接入 dist worker · `2b22681` heartbeat+sweep 协议 ·
  `2cc87a2` cancel 通道贯通
- CLAUDE.md 公理 1（headless agent A2A 唯一出口）+ 公理 3（不轮询）—— 真并发不破公理
