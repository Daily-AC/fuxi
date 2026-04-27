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

use chrono::{DateTime, Duration, Utc};
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_core::task::TaskState;
use fuxi_events::EventBus;
use fuxi_orchestrator::Fuxi;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// β · #74 远端 dist 节点的 worker 列表 fallback——从 EventBus 历史里
/// 反查"source_node_id == 该节点 + 还没 task_state_changed 终态"的 cc agent。
///
/// dist worker 上的 cc 是 spawn-per-job 模型（不像 home 端 shelf 里的常驻 agent），
/// 一旦 task done 就销毁——所以 worker 列表只显"当下还在跑的"。已终态或 5 分钟
/// 内无新事件的视为已结束，从列表剔除。
///
/// 跟 `home_workers_from_shelf` 不同处：
/// - 数据源是 EventBus 历史，不是 shelf（远端 cc 不在 home shelf）
/// - 只显 busy，没 idle/dead——dist cc 没"挂着等任务"的 idle 态
/// - role 通过 AgentSpawning 事件推断（home 端发的 dist_job_dispatched 事件无 role 信息）
pub async fn dist_workers_from_events(bus: &EventBus, node_id: &str) -> Vec<WorkerView> {
    let recent = match bus.store().recent(2000).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let cutoff = Utc::now() - Duration::seconds(300);
    let mut role_by_agent: HashMap<AgentId, String> = HashMap::new();
    let mut last_at: HashMap<AgentId, DateTime<Utc>> = HashMap::new();
    let mut terminated: HashMap<AgentId, bool> = HashMap::new();

    for ev in &recent {
        if ev.meta.source_node_id.as_deref() != Some(node_id) {
            continue;
        }
        let Some(agent) = ev.meta.agent else {
            continue;
        };
        last_at
            .entry(agent)
            .and_modify(|t| {
                if ev.meta.at > *t {
                    *t = ev.meta.at;
                }
            })
            .or_insert(ev.meta.at);
        match &ev.kind {
            EventKind::AgentSpawning { role, .. } => {
                role_by_agent.entry(agent).or_insert_with(|| role.clone());
            }
            EventKind::TaskStateChanged {
                to: TaskState::Done | TaskState::Cancelled | TaskState::Delivering,
                ..
            } => {
                terminated.insert(agent, true);
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for (agent, role) in role_by_agent {
        if *terminated.get(&agent).unwrap_or(&false) {
            continue;
        }
        if last_at.get(&agent).is_none_or(|at| *at < cutoff) {
            continue;
        }
        out.push(WorkerView {
            agent_id: agent.to_string(),
            role_display: role_display(&role),
            role,
            status: "busy".to_string(),
        });
    }
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

    /// β · #74 真因回归：dist 节点的 workers 列表必须从 events 历史里反查，
    /// 否则用户看到 mac 节点 workers:[] 跟实测「明明有门客在干活」相悖。
    #[tokio::test]
    async fn dist_workers_from_events_returns_busy_agent_with_no_terminal_state() {
        use fuxi_core::event::{Event, EventMeta};
        use fuxi_core::id::TaskId;

        let bus = EventBus::with_memory_store().await.expect("bus");
        let agent = AgentId::new();
        let task = TaskId::new();

        // mac 节点上 cc spawn → 还在跑（无 terminal）
        let mut meta1 = EventMeta::now();
        meta1.agent = Some(agent);
        meta1.task = Some(task);
        meta1.source_node_id = Some("mac".into());
        bus.publish(Event {
            meta: meta1,
            kind: EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
        })
        .expect("publish");

        // 给 store 一点时间持久化
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let workers = dist_workers_from_events(&bus, "mac").await;
        assert_eq!(workers.len(), 1, "应有 1 个跑中的 worker");
        assert_eq!(workers[0].role, "luban");
        assert_eq!(workers[0].role_display, "鲁班");
        assert_eq!(workers[0].status, "busy");
    }

    #[tokio::test]
    async fn dist_workers_from_events_skips_after_task_state_done() {
        use fuxi_core::event::{Event, EventMeta};
        use fuxi_core::id::TaskId;

        let bus = EventBus::with_memory_store().await.expect("bus");
        let agent = AgentId::new();
        let task = TaskId::new();

        // mac 节点：spawn → done
        for kind in [
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
            EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        ] {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent);
            meta.task = Some(task);
            meta.source_node_id = Some("mac".into());
            bus.publish(Event { meta, kind }).expect("publish");
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let workers = dist_workers_from_events(&bus, "mac").await;
        assert_eq!(workers.len(), 0, "task done 后远端 cc 视为销毁，不应再列");
    }

    #[tokio::test]
    async fn dist_workers_from_events_filters_by_node_id() {
        use fuxi_core::event::{Event, EventMeta};
        use fuxi_core::id::TaskId;

        let bus = EventBus::with_memory_store().await.expect("bus");

        // mac 上一个跑中的 luban
        let mac_agent = AgentId::new();
        let mut meta1 = EventMeta::now();
        meta1.agent = Some(mac_agent);
        meta1.task = Some(TaskId::new());
        meta1.source_node_id = Some("mac".into());
        bus.publish(Event {
            meta: meta1,
            kind: EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
        })
        .expect("publish");

        // mbp-2 上一个跑中的 pusong
        let other_agent = AgentId::new();
        let mut meta2 = EventMeta::now();
        meta2.agent = Some(other_agent);
        meta2.task = Some(TaskId::new());
        meta2.source_node_id = Some("mbp-2".into());
        bus.publish(Event {
            meta: meta2,
            kind: EventKind::AgentSpawning {
                role: "pusong".into(),
                cli: "claude-code".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mac_workers = dist_workers_from_events(&bus, "mac").await;
        assert_eq!(mac_workers.len(), 1);
        assert_eq!(mac_workers[0].role, "luban");

        let mbp_workers = dist_workers_from_events(&bus, "mbp-2").await;
        assert_eq!(mbp_workers.len(), 1);
        assert_eq!(mbp_workers[0].role, "pusong");

        let none_workers = dist_workers_from_events(&bus, "no-such-node").await;
        assert_eq!(none_workers.len(), 0);
    }
}
