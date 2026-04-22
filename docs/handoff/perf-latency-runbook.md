# Perf Latency Runbook（bridge / xuannv 回报链路）

## 目标
- 量化“门客回报慢”是慢在 `bridge` 传递，还是慢在“玄女 busy 排队”。

## 开关
- `FUXI_TERMINAL_DRAIN_GRACE_MS`：dispatch terminal 后等待窗口（默认 50ms）。
- `FUXI_BRIDGE_INTERRUPT_WORKER_REPORTS`：是否允许慢回报打断玄女（`1/true` 开启，默认关闭）。
- `FUXI_BRIDGE_INTERRUPT_LAG_MS`：开启打断后触发阈值（默认 `3000` ms）。

## 手测步骤
1. 清空日志：`truncate -s 0 /tmp/fuxi.log`
2. 启动 fuxi（默认模式先跑）
3. 连续触发门客任务 + 在玄女侧持续输入，模拟“玄女 busy”
4. 退出后生成报告：`scripts/bridge-lag-report.sh /tmp/fuxi.log`

## 结果解读
- `bridge_forward_events`：桥层实际转发次数（门客终态/下线 -> 玄女）。
- `busy_enqueue_events`：玄女在 busy 状态被追加入队次数（值越高，主观等待感越强）。
- `lag_ms_*`：桥层从事件产生到桥转发的延迟统计。
- `interrupt_first_*`：本次是否用了“打断玄女”通道。

## 判断规则（当前）
- `lag_ms_avg` 低但 `busy_enqueue_events` 高：慢点在玄女 busy 队列，不在 bridge。
- `lag_ms_avg` 高且 `busy_enqueue_events` 低：慢点在 bridge/事件流。
- 两者都高：链路两端都拥塞，需要同时调参。

## 建议阈值
- 日常目标：`lag_ms_avg < 300ms`
- 可接受上限：`lag_ms_p95（近似看 max） < 1500ms`
- 如果 `busy_enqueue_events` 明显上升，先试：
  - 开启 `FUXI_BRIDGE_INTERRUPT_WORKER_REPORTS=1`
  - 阈值设 `FUXI_BRIDGE_INTERRUPT_LAG_MS=2000`
