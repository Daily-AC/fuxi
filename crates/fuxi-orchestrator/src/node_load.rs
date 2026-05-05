//! v2 跨节点 sandbox · 节点负载提供方 trait。
//!
//! ## 为啥要 trait
//!
//! `Fuxi::dispatch` 需要在 task 关联到 project 但未显式 pin 时，按节点当前负载
//! 选最闲那个 auto-pin。但 fuxi-orchestrator 不能依赖 fuxi-cli（fuxi-cli 反过来
//! 依赖 fuxi-orchestrator，循环）。同 `DistEnqueuer` / `RecallSink` 反向依赖
//! pattern：trait 定义在本 crate，production impl 由 fuxi-cli 包
//! `Arc<DistController>` 提供。
//!
//! ## 度量
//!
//! 用 inflight/max_concurrency 比值。max=0 归一为 1（防分母 0 + dist controller
//! 已对 register 期 0 做 saturation guard）。CPU 负载等更细信号留 v3 综合。

use async_trait::async_trait;

/// 单节点负载快照——dispatch 决策时一次拿走，不持锁。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeLoadSnapshot {
    pub node_id: String,
    pub inflight: usize,
    pub max_concurrency: u32,
    /// 心跳 lag 在阈值内（fuxi-im 用 30s）。离线节点不参与 auto-pin 候选。
    pub online: bool,
}

impl NodeLoadSnapshot {
    /// 衡量"忙不忙"——inflight/max_concurrency。
    /// max=0 归一为 1 防分母 0；inflight 远超 max 时返 >1（饱和度）。
    pub fn saturation(&self) -> f64 {
        let cap = self.max_concurrency.max(1) as f64;
        self.inflight as f64 / cap
    }
}

/// 节点负载数据源。production impl 由 fuxi-cli 包 `Arc<DistController>` 提供，
/// 测试可注入 stub 直接构造 `Vec<NodeLoadSnapshot>`。
#[async_trait]
pub trait NodeLoadProvider: Send + Sync {
    /// 当前所有已注册节点的负载视图。
    async fn snapshot(&self) -> Vec<NodeLoadSnapshot>;
}

/// 从 candidates 中选 saturation 最低且 online 的节点。
///
/// 都离线 / 都不在 candidates → None（caller 决定降级策略：fallback 本地
/// spawn 或 raise）。
pub fn pick_least_loaded<'a>(
    snapshots: &'a [NodeLoadSnapshot],
    candidates: &[String],
) -> Option<&'a NodeLoadSnapshot> {
    snapshots
        .iter()
        .filter(|s| s.online && candidates.iter().any(|c| c == &s.node_id))
        .min_by(|a, b| {
            a.saturation()
                .partial_cmp(&b.saturation())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_handles_zero_capacity() {
        let snap = NodeLoadSnapshot {
            node_id: "n".into(),
            inflight: 3,
            max_concurrency: 0,
            online: true,
        };
        // 0 max 归一为 1 → saturation = 3/1 = 3
        assert!((snap.saturation() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn pick_picks_lowest_saturation() {
        let snaps = vec![
            NodeLoadSnapshot {
                node_id: "home".into(),
                inflight: 3,
                max_concurrency: 4,
                online: true,
            },
            NodeLoadSnapshot {
                node_id: "mac".into(),
                inflight: 1,
                max_concurrency: 4,
                online: true,
            },
        ];
        let picked = pick_least_loaded(&snaps, &["home".to_string(), "mac".to_string()])
            .expect("应选到 mac");
        assert_eq!(picked.node_id, "mac");
    }

    #[test]
    fn pick_skips_offline_even_if_lowest() {
        let snaps = vec![
            NodeLoadSnapshot {
                node_id: "home".into(),
                inflight: 3,
                max_concurrency: 4,
                online: true,
            },
            NodeLoadSnapshot {
                node_id: "mac".into(),
                inflight: 0,
                max_concurrency: 8,
                online: false,
            },
        ];
        let picked = pick_least_loaded(&snaps, &["home".to_string(), "mac".to_string()])
            .expect("home 应胜出");
        assert_eq!(picked.node_id, "home");
    }

    #[test]
    fn pick_returns_none_when_no_candidate_online() {
        let snaps = vec![NodeLoadSnapshot {
            node_id: "home".into(),
            inflight: 0,
            max_concurrency: 4,
            online: false,
        }];
        let picked = pick_least_loaded(&snaps, &["home".to_string()]);
        assert!(picked.is_none());
    }

    #[test]
    fn pick_filters_out_non_candidates() {
        let snaps = vec![NodeLoadSnapshot {
            node_id: "stranger".into(),
            inflight: 0,
            max_concurrency: 4,
            online: true,
        }];
        // candidates 列表里没 stranger → 返 None
        let picked = pick_least_loaded(&snaps, &["home".to_string()]);
        assert!(picked.is_none());
    }
}
