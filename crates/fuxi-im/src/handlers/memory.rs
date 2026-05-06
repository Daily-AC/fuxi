//! `/api/memory` —— PWA「更多 → 记忆」页数据源。
//!
//! 把策府（OracleStore）当前生效（valid_until IS NULL）的事实拉出来按 subject 分组返。
//! 仅读不写——用户视角是"看一眼策府现在记着什么"，编辑走 fuxi-cli `oracle` 子命令或
//! 玄女自己 supersede。
//!
//! `Option` 为 None（`fuxi im start` 没注入 OracleStore）时返 503，跟其他端点同步。

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::state::AppState;
use fuxi_memory::OracleFact;

/// 单条事实的前端视图——用扁平 string subject/predicate/object，不抛 uuid。
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryFactView {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source: String,
    pub confidence: f32,
    pub updated_at: String,
}

impl From<OracleFact> for MemoryFactView {
    fn from(f: OracleFact) -> Self {
        Self {
            id: f.id.to_string(),
            subject: f.subject,
            predicate: f.predicate,
            object: f.object,
            source: f.source,
            confidence: f.confidence,
            updated_at: f.updated_at.to_rfc3339(),
        }
    }
}

/// 一组同 subject 的事实——前端按组列卡片，每组顶部 subject + 计数。
#[derive(Debug, Serialize, Deserialize)]
pub struct MemorySubjectGroup {
    pub subject: String,
    pub facts: Vec<MemoryFactView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryResponse {
    pub groups: Vec<MemorySubjectGroup>,
    /// 跨 subject 的总条数——前端 header 显「策府 共 N 条」用。
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// 跨所有 subject 的硬上限。默认 200——策府量级目前 < 1k 行，pwa 一屏够。
    pub limit: Option<i64>,
}

/// `GET /api/memory` —— 列现行事实，按 subject 分组（subject 内 updated_at desc）。
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<MemoryResponse>> {
    let oracle = state
        .oracle
        .as_ref()
        .ok_or_else(|| Error::Unavailable("策府未注入".into()))?;
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let facts = oracle
        .list_active(limit)
        .await
        .map_err(|e| Error::Internal(format!("oracle list_active: {e}")))?;
    let total = facts.len() as i64;

    // group by subject 保留 updated_at desc 的全局顺序（先到的 subject 先建组），
    // 同 subject 内事实保持来源顺序。HashMap 不保序，用 Vec + 线性查避免依赖额外 crate。
    let mut groups: Vec<MemorySubjectGroup> = Vec::new();
    for f in facts {
        if let Some(g) = groups.iter_mut().find(|g| g.subject == f.subject) {
            g.facts.push(MemoryFactView::from(f));
        } else {
            groups.push(MemorySubjectGroup {
                subject: f.subject.clone(),
                facts: vec![MemoryFactView::from(f)],
            });
        }
    }

    Ok(Json(MemoryResponse { groups, total }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use fuxi_events::EventBus;
    use fuxi_memory::{NewFact, OracleStore};
    use fuxi_orchestrator::Fuxi;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn build_app() -> (TempDir, Router, OracleStore) {
        let dir = tempfile::tempdir().expect("tmp");
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let oracle = OracleStore::connect_memory().await.expect("oracle");
        let state = AppState::new(fuxi).with_oracle(oracle.clone());
        let app = Router::new()
            .route("/api/memory", get(list))
            .with_state(state);
        (dir, app, oracle)
    }

    #[tokio::test]
    async fn list_groups_facts_by_subject() {
        let (_dir, app, oracle) = build_app().await;
        oracle
            .insert(NewFact::new("user", "prefers", "冰美式"))
            .await
            .unwrap();
        oracle
            .insert(NewFact::new("user", "name", "以琳"))
            .await
            .unwrap();
        oracle
            .insert(NewFact::new("luban", "role", "工匠"))
            .await
            .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/memory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: MemoryResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.total, 3);
        assert_eq!(body.groups.len(), 2);
        let user = body.groups.iter().find(|g| g.subject == "user").unwrap();
        assert_eq!(user.facts.len(), 2);
        let luban = body.groups.iter().find(|g| g.subject == "luban").unwrap();
        assert_eq!(luban.facts.len(), 1);
        assert_eq!(luban.facts[0].object, "工匠");
    }

    #[tokio::test]
    async fn list_excludes_superseded_facts() {
        let (_dir, app, oracle) = build_app().await;
        let stale = oracle
            .insert(NewFact::new("xuannv", "session_id", "old"))
            .await
            .unwrap();
        oracle
            .supersede(
                stale.id,
                NewFact::new("xuannv", "session_id", "new").with_source("agent"),
            )
            .await
            .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/memory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: MemoryResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.total, 1);
        assert_eq!(body.groups[0].facts[0].object, "new");
    }

    #[tokio::test]
    async fn handler_returns_503_when_oracle_not_injected() {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/memory", get(list))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/memory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
