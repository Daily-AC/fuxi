//! 节点拓扑数据提供方 trait（β · #55，spec gap c）。
//!
//! ## 为啥要 trait
//!
//! `/api/nodes` 端点需要查 dist controller 的 nodes_snapshot，但 fuxi-im 不
//! 能依赖 fuxi-cli（fuxi-cli 反过来依赖 fuxi-im，会循环）。所以仿
//! `fuxi-orchestrator::bridge::Intervener` 模式：trait 定义在 fuxi-im，
//! 生产 impl 放在 fuxi-cli（顶层 crate 持有 DistController 句柄）。
//!
//! ## wire schema
//!
//! [`NodeView`] 是 wire 格式（serde + 中文 role_display 在前端就绪）。
//! 它**包装**了 dist 的 `NodeSnapshot`（已含 inflight / lag / status），
//! 多加 `workers: Vec<WorkerView>` 字段——每节点上跑的门客实例。
//!
//! ## v1 范围 + 已知缺口
//!
//! - **home 节点**：`workers` 从 `Fuxi.shelf.list_workers()` 拿（filter xuannv）。
//!   home 节点是 fuxi-im 同进程注册的，所有本地 spawn 的 cc/codex 都在 home 上。
//! - **远端节点**（如 mac-local）：v1 `workers` 始终空。dist 协议当前只跟 job_id
//!   不跟 agent_id；要等 #57 dispatch routing 加 worker→node 真映射后才能填。
//!   ε 端 #58 节点 tab 设计已考虑 `workers: []` 的合法形态，前端不破。

use fuxi_orchestrator::Fuxi;
use serde::{Deserialize, Serialize};

/// `/api/nodes` 响应里的单条节点视图。
///
/// `node_id`：dist controller 注册时声明的 id（home 节点恒为 "home"）。
/// `online`：heartbeat lag < 30s。前端 dot 颜色按此切。
/// `workers`：节点上当前跑的门客实例列表（v1：home 真实，其他空）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    pub node_id: String,
    pub tags: Vec<String>,
    pub max_concurrency: u32,
    pub inflight_jobs: usize,
    pub heartbeat_lag_ms: Option<u64>,
    pub online: bool,
    pub registered_at_ms_ago: Option<u64>,
    pub workers: Vec<WorkerView>,
}

/// 节点上单个门客实例视图——前端在节点卡内渲染 worker 行用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerView {
    pub agent_id: String,
    /// 角色 internal id，如 "luban"——前端 chip 颜色查表用。
    pub role: String,
    /// 中文显示名，如 "鲁班"——前端直接渲。
    pub role_display: String,
    /// `"busy"` / `"idle"` / `"dead"`，跟 `MemberCard.status` 同语义。
    pub status: String,
}

/// `/api/nodes` 整体响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodesResponse {
    pub nodes: Vec<NodeView>,
}

/// 节点拓扑数据源——production impl 由 fuxi-cli 包 `Arc<DistController>` 提供。
///
/// 测试可注入一个 stub 直接构造 `Vec<NodeView>` 验 handler 行为。
#[async_trait::async_trait]
pub trait NodesProvider: Send + Sync {
    /// 返回当前所有节点 + 各节点上的门客实例。
    ///
    /// `fuxi`：用来拿 home 节点上的 worker 列表（local shelf）。trait impl 自决
    /// 哪些 node_id 算 home（生产恒为 `"home"`）。
    async fn list_nodes(&self, fuxi: &Fuxi) -> Vec<NodeView>;
}

/// `online` 阈值：dist controller 用 60s 算 stale；前端按 30s 算"在线"
/// （spec gap c）——略严，给用户更早看到节点掉线信号。
pub const ONLINE_HEARTBEAT_THRESHOLD_MS: u64 = 30_000;

/// 在 home 节点上根据 `Fuxi.shelf` 汇出 worker 列表。
///
/// 抽函数让 production impl 直接复用——只是把 `list_workers` 转 `WorkerView`，
/// 跳过玄女（不算门客实例）。前端节点卡按 status 排序，所以 status 字段必须准。
pub async fn home_workers_from_shelf(fuxi: &Fuxi) -> Vec<WorkerView> {
    let xuannv_id = fuxi.xuannv_id().await;
    let cards = fuxi.list_workers().await;
    let mut out = Vec::with_capacity(cards.len().saturating_sub(1));
    for card in cards {
        if Some(card.id) == xuannv_id {
            continue;
        }
        let role = card.profile.role.clone();
        let status = match fuxi.status_of(card.id).await {
            Some(fuxi_orchestrator::ShelfStatus::Idle) => "idle",
            Some(fuxi_orchestrator::ShelfStatus::Busy) => "busy",
            Some(fuxi_orchestrator::ShelfStatus::Dead) => "dead",
            None => "dead",
        }
        .to_string();
        out.push(WorkerView {
            agent_id: card.id.to_string(),
            role_display: role_display(&role),
            role,
            status,
        });
    }
    // busy 排前 → idle → dead（spec gap c "按 status 排序 busy > idle"）
    out.sort_by(|a, b| status_rank(&a.status).cmp(&status_rank(&b.status)));
    out
}

fn status_rank(s: &str) -> u8 {
    match s {
        "busy" => 0,
        "idle" => 1,
        _ => 2,
    }
}

/// role_display 中文映射——同 `tasks_view::role_display`，保持映射表一致。
/// 后续若加 role 走 ROLE.md `display_name` frontmatter 字段。
pub fn role_display(role: &str) -> String {
    match role {
        "xuannv" => "玄女".into(),
        "luban" => "鲁班".into(),
        "pusong" => "蒲松".into(),
        "moshu" => "墨术".into(),
        "shennong" => "神农".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_display_matches_tasks_view_table() {
        assert_eq!(role_display("luban"), "鲁班");
        assert_eq!(role_display("xuannv"), "玄女");
        assert_eq!(role_display("pusong"), "蒲松");
        assert_eq!(role_display("custom"), "custom");
    }

    #[test]
    fn status_rank_orders_busy_first() {
        assert!(status_rank("busy") < status_rank("idle"));
        assert!(status_rank("idle") < status_rank("dead"));
        assert!(status_rank("dead") == status_rank("unknown")); // 都归到 2
    }

    #[test]
    fn online_threshold_is_30s() {
        // spec gap c 严格 30s——比 dist STALE_THRESHOLD (60s) 略严
        assert_eq!(ONLINE_HEARTBEAT_THRESHOLD_MS, 30_000);
    }
}
