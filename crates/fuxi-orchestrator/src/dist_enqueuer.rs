//! β · #57 dispatch routing 钩子（spec gap e）。
//!
//! ## 为啥要 trait
//!
//! `Fuxi::dispatch` 决策树（pinned_node / required_tags / default 三分支）需要把
//! task 派到 dist controller，但 fuxi-orchestrator 不能依赖 fuxi-cli（fuxi-cli 反
//! 过来依赖 fuxi-orchestrator，循环）。
//!
//! 同 `bridge::Intervener` / `recall::RecallSink` 反向依赖 pattern：trait 定义在
//! fuxi-orchestrator，production impl 放 fuxi-cli 包 `Arc<DistController>`。
//!
//! ## 注入方式
//!
//! `Fuxi::set_dist_enqueuer(impl)` 在 `fuxi im start` 启动期一次性注入；
//! `Option`：未注入 = dist 路径 noop（dispatch 仍走本地 spawn fallback）。
//!
//! ## 已知缺口（v1）
//!
//! - cli/role 字段：dist::DistController.enqueue 要求 `cli` + `allowed_tools`，
//!   trait 入口暂走 default `cli="cc"` + 空 allowed_tools。后续 task 维度补充
//!   role → cli 映射后再传真值
//! - 没回调拉 dist 事件做"job done → mark task done"：dist worker 上报 events
//!   会自动 publish 到 home EventBus（共享 bus，#54 装配）—— 玄女直接订阅 events
//!   就能看到，不需要 trait 反馈

use crate::Result;
use async_trait::async_trait;
use fuxi_core::task::Task;

/// dist 路径 enqueue 时要透传给 worker 的 caller 上下文。
///
/// 抽 struct 避免 trait 签名按 1-by-1 加参数臃肿；后续若加更多 caller-side
/// 决定的字段（cli、allowed_tools、超时等）继续在此扩展，trait 签名稳定。
#[derive(Debug, Clone, Default)]
pub struct DistEnqueueOptions {
    /// 完整 worker system prompt（含 role 心智 + sentinel addendum）。
    /// worker 端 `run_cc_job` 转 `--append-system-prompt` 注 cc——决策 13
    /// deliverable nudge 教学经此字段才能到远端 cc。`None` = 走 worker 默认
    /// 裸 prompt（cc 不知道发 sentinel，玄女收不到 nudge）。
    pub system_prompt: Option<String>,
    /// home 端真相 task_id（`task-<uuid>` 全形式）——worker 端用此 id 给 cc/codex
    /// agent 跑出来的所有 events 填 meta.task。否则 worker 自生成 TaskId 跟 home
    /// 完全不同 → /api/tasks aggregate 永远拼不起来（#76 实测踩坑）。
    pub task_id: Option<String>,
    /// home 端 dispatch 时的目标 role（`"luban"` / `"pusong"` 等）——worker 端
    /// spawn cc 前用此 publish AgentSpawning，让 home aggregate 能查到 role 不
    /// fallback "unknown"（#77）。
    pub role: Option<String>,
}

/// dispatch routing 决策树命中 dist 路径时调的钩子。
///
/// 实装方按 `task.pinned_node` / `task.required_tags` 把 task 派到对应 dist
/// 节点；返回 `Ok(job_id)` 让 caller 记日志 / 关联事件。
#[async_trait]
pub trait DistEnqueuer: Send + Sync {
    /// 把 task 派给 dist controller。
    ///
    /// 实装应当：
    /// 1. 根据 `task.pinned_node` / `task.required_tags` 调
    ///    `DistController::enqueue` 入队
    /// 2. 透传 `opts.system_prompt` / `task_id` / `role` 给 DistJob，让 worker
    ///    端 spawn cc/codex 时能拿到 home 真相
    /// 3. dist worker 自己 pull 后跑 cc/codex，事件流回 EventBus（共享 bus）
    /// 4. 返 `job_id` 字串便于 caller 跟踪 / 关联 task_id
    async fn enqueue(&self, task: &Task, opts: DistEnqueueOptions) -> Result<String>;
}
