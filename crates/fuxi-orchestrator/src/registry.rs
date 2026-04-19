//! 门客注册表。
//!
//! 在进程内维护一份 `AgentId → ShelfEntry` 的 HashMap，读写都走
//! `tokio::sync::RwLock`：读多写少，dispatch_to_any 扫描是读、spawn
//! 插入是写。

use fuxi_core::agent::{Agent, AgentCard};
use fuxi_core::id::AgentId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 单个门客的 shelf 记录。
///
/// WHY 把 agent 和 worktree 绑到一起：销毁门客时需要同步销毁 worktree，
/// 不然会残留 git worktree 垃圾。shutdown path 在一处处理。
pub struct ShelfEntry {
    pub card: AgentCard,
    /// 为什么 `Arc<dyn Agent>`：调用方可能并发 dispatch（它跑在一个后台 task
    /// 里）——trait object 用 Arc 共享最直接。
    pub agent: Arc<dyn Agent>,
    pub status: ShelfStatus,
    /// 分配给该门客的 worktree。None 表示"在主目录上干活"（不推荐，但支持）。
    pub worktree: Option<fuxi_core::workspace::WorkspaceHandle>,
}

/// 门客的局部可观测状态。**不是** `AgentStatus`——那是 agent 自报、
/// 可能滞后；这个是玄女基于 dispatch/complete/dead 事件维护的视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfStatus {
    /// 刚 spawn，还没接过 task。
    Idle,
    /// 正在处理一个 task（发过 dispatch 但还没见到终结事件）。
    Busy,
    /// 进程结束或被 shutdown。
    Dead,
}

/// 并发安全的 shelf，整个玄女共享一份 `Arc`。
#[derive(Default)]
pub struct Shelf {
    inner: RwLock<HashMap<AgentId, ShelfEntry>>,
}

impl Shelf {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入新门客——调用方要保证 id 唯一。
    pub async fn insert(&self, entry: ShelfEntry) {
        let id = entry.card.id;
        self.inner.write().await.insert(id, entry);
    }

    /// 取门客的 handle（Arc 克隆）——找不到返回 None。
    pub async fn get_agent(&self, id: AgentId) -> Option<Arc<dyn Agent>> {
        self.inner.read().await.get(&id).map(|e| e.agent.clone())
    }

    /// 更新状态。id 不存在时静默忽略（竞态：门客可能刚被 destroy）。
    pub async fn set_status(&self, id: AgentId, new: ShelfStatus) {
        if let Some(e) = self.inner.write().await.get_mut(&id) {
            e.status = new;
        }
    }

    /// 查状态——不存在返回 None。
    pub async fn status_of(&self, id: AgentId) -> Option<ShelfStatus> {
        self.inner.read().await.get(&id).map(|e| e.status)
    }

    /// **原子地**按角色找一个 Idle 门客并置为 Busy——解决 `dispatch_to_any` 的
    /// TOCTOU 窗口（find + set_status 分两步会让并发调用拿到同一个 id）。
    ///
    /// 返回 `Some(id)` 意味着调用方已独占该门客；外层再调 `dispatch` 的
    /// `set_status(Busy)` 是幂等无害的。
    pub async fn claim_idle_by_role(&self, role: &str) -> Option<AgentId> {
        let mut guard = self.inner.write().await;
        let pick = guard
            .iter_mut()
            .find(|(_, e)| e.status == ShelfStatus::Idle && e.card.profile.role == role);
        if let Some((id, e)) = pick {
            e.status = ShelfStatus::Busy;
            Some(*id)
        } else {
            None
        }
    }

    /// 按角色找一个**空闲**门客（**非原子**，仅供只读观察 / 调试）。
    ///
    /// 调度路径请用 `claim_idle_by_role`——否则会有 TOCTOU race。
    pub async fn find_idle_by_role(&self, role: &str) -> Option<AgentId> {
        self.inner
            .read()
            .await
            .values()
            .find(|e| e.status == ShelfStatus::Idle && e.card.profile.role == role)
            .map(|e| e.card.id)
    }

    /// 返回指定门客分配的 worktree 路径——用于 TUI 右栏元信息展示。
    /// 没登记或没分配 worktree 都返回 None。
    pub async fn worktree_of(&self, id: AgentId) -> Option<PathBuf> {
        self.inner
            .read()
            .await
            .get(&id)
            .and_then(|e| e.worktree.as_ref().map(|h| h.worktree_path.clone()))
    }

    /// 列出所有 card（用于对外展示 / API 查询）。
    pub async fn list_cards(&self) -> Vec<AgentCard> {
        self.inner
            .read()
            .await
            .values()
            .map(|e| e.card.clone())
            .collect()
    }

    /// 取出门客——用于 destroy path。取出后不在表里，避免悬挂引用。
    pub async fn take(&self, id: AgentId) -> Option<ShelfEntry> {
        self.inner.write().await.remove(&id)
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// 是否为空——避免测试里 `len() == 0` 的冗长。
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}
