//! handler 间共享的应用状态。
//!
//! 当前只持 `Arc<Fuxi>` 一项；β/γ/δ 会扩字段（设备 token store、push 订阅表、
//! WS 广播 hub 等）。**所有新字段都加 `Default` 友好的默认值**，不要破坏
//! `AppState::new(fuxi)` 的最小构造路径——骨架 smoke 测试和单元测试都依赖它。
//!
//! 为什么用 `Arc<Fuxi>` 而不是 owned：handler 是 `'static` 任务，必须 cheap clone。

use fuxi_orchestrator::Fuxi;
use std::sync::Arc;

/// 共享给所有 handler 的应用状态。`Clone` 廉价（内部都是 `Arc`）。
#[derive(Clone)]
pub struct AppState {
    /// 玄女编排句柄——`/api/intervene` / `/api/dispatch` / `/api/tasks` 全要它。
    pub fuxi: Arc<Fuxi>,
}

impl AppState {
    /// 用一个已经构造好的 `Fuxi` 句柄装配 state。
    pub fn new(fuxi: Arc<Fuxi>) -> Self {
        Self { fuxi }
    }
}
