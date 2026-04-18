//! 门客注册表。
//!
//! 在进程内维护一份 `AgentId → ShelfEntry` 的 HashMap，读写都走
//! `tokio::sync::RwLock`：读多写少，dispatch_to_any 扫描是读、spawn
//! 插入是写。

use fuxi_core::agent::{Agent, AgentCard};
use fuxi_core::id::AgentId;
use fuxi_workspace::GitWorktreeWorkspace;
use std::collections::HashMap;
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

    /// 按角色找一个**空闲**门客——用于 `dispatch_to_any`。
    pub async fn find_idle_by_role(&self, role: &str) -> Option<AgentId> {
        self.inner
            .read()
            .await
            .values()
            .find(|e| e.status == ShelfStatus::Idle && e.card.profile.role == role)
            .map(|e| e.card.id)
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

/// 一个薄封装，把 workspace 和 shelf 放在 `Fuxi` 里统一持有。
///
/// 为什么不直接在 `Fuxi` 里持两个字段：未来 `Workspace` 可能换成泛型
/// `Arc<dyn Workspace>`，这里留一层便于演进。
pub struct WorkerDeps {
    pub workspace: Arc<GitWorktreeWorkspace>,
    pub shelf: Arc<Shelf>,
}
