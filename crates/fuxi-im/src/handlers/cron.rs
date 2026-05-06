//! `/api/cron` —— PWA「更多 → 更漏」页数据源。
//!
//! 把 [`fuxi_scheduler::TriggerStore`] 里登记过的 trigger 列出来——cron / once /
//! fs_watch / webhook 各一条。仅读：CRUD 走 `fuxi cron *` CLI。
//!
//! 字段是给前端可显示用的扁平化 view（spec 拍扁成 kind + expr/path/at/...），
//! 避免前端再去理解 `TriggerSpec` 的 tag 联合形式。

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::state::AppState;
use fuxi_scheduler::{TriggerRow, TriggerSpec};

#[derive(Debug, Serialize, Deserialize)]
pub struct CronEntryView {
    pub id: String,
    /// "cron" / "once" / "fs_watch" / "webhook"——前端按 kind 渲染不同摘要。
    pub kind: String,
    /// 显式 enabled 标识——disabled 卡片置灰。
    pub enabled: bool,
    /// 一句话描述（intent）——用户登记时写的目标。
    pub intent: String,
    /// cron expr / once 时间点 / fs path / webhook 提示——按 kind 派用。
    pub summary: String,
    /// 时区（cron only）。
    pub tz: Option<String>,
    /// 失败计数（≥ max_failures 时调度器自动 disable）。
    pub consecutive_failures: i64,
    pub max_failures: i64,
    /// 上次 fire（任意 cause），无则 None。
    pub last_fired_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CronResponse {
    pub triggers: Vec<CronEntryView>,
}

impl From<TriggerRow> for CronEntryView {
    fn from(r: TriggerRow) -> Self {
        let kind = r.spec.kind_str().to_string();
        let (summary, tz) = match &r.spec {
            TriggerSpec::Cron { expr, tz } => (expr.clone(), tz.clone()),
            TriggerSpec::Once { at } => (at.to_rfc3339(), None),
            TriggerSpec::FsWatch { path, events } => {
                let p = path.display().to_string();
                if events.is_empty() {
                    (p, None)
                } else {
                    (format!("{p} [{}]", events.join(",")), None)
                }
            }
            TriggerSpec::Webhook { .. } => ("POST /hook/<id>".to_string(), None),
        };
        Self {
            id: r.id,
            kind,
            enabled: r.enabled,
            intent: r.intent,
            summary,
            tz,
            consecutive_failures: r.consecutive_failures,
            max_failures: r.max_failures,
            last_fired_at: r.last_fired_at.map(|t| t.to_rfc3339()),
            created_at: r.created_at.to_rfc3339(),
        }
    }
}

/// `GET /api/cron` —— 列所有 trigger（含 disabled）。
pub async fn list(State(state): State<AppState>) -> Result<Json<CronResponse>> {
    let store = state
        .triggers
        .as_ref()
        .ok_or_else(|| Error::Unavailable("更漏未注入".into()))?;
    let rows = store
        .list_all()
        .await
        .map_err(|e| Error::Internal(format!("trigger list_all: {e}")))?;
    let triggers = rows.into_iter().map(CronEntryView::from).collect();
    Ok(Json(CronResponse { triggers }))
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
    use fuxi_orchestrator::Fuxi;
    use fuxi_scheduler::TriggerStore;
    use fuxi_scheduler::store::NewTrigger;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn build_app() -> (TempDir, Router, TriggerStore) {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let store = TriggerStore::connect_memory().await.unwrap();
        let state = AppState::new(fuxi).with_triggers(store.clone());
        let app = Router::new()
            .route("/api/cron", get(list))
            .with_state(state);
        (dir, app, store)
    }

    #[tokio::test]
    async fn list_returns_all_triggers_with_kind_summary() {
        let (_dir, app, store) = build_app().await;
        store
            .insert(NewTrigger {
                id: fuxi_scheduler::new_trigger_id(),
                spec: TriggerSpec::Cron {
                    expr: "0 9 * * *".into(),
                    tz: Some("Asia/Shanghai".into()),
                },
                intent: "晨报".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .unwrap();
        store
            .insert(NewTrigger {
                id: fuxi_scheduler::new_trigger_id(),
                spec: TriggerSpec::Webhook { secret: None },
                intent: "webhook 入口".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cron")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: CronResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.triggers.len(), 2);
        let cron = body.triggers.iter().find(|e| e.kind == "cron").unwrap();
        assert_eq!(cron.summary, "0 9 * * *");
        assert_eq!(cron.tz.as_deref(), Some("Asia/Shanghai"));
        assert!(cron.enabled);
        let webhook = body.triggers.iter().find(|e| e.kind == "webhook").unwrap();
        assert_eq!(webhook.summary, "POST /hook/<id>");
    }

    #[tokio::test]
    async fn handler_returns_503_when_triggers_not_injected() {
        let dir = TempDir::new().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/cron", get(list))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cron")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
