//! 候吏（响应式入口）· HTTP 版：`POST /hook/:trigger_id`
//!
//! 命中后查 `triggers` 表，若 kind=webhook 且 enabled 且（如有 secret）匹配，
//! 走 `Keeper::record_and_emit_fire(cause=Webhook)`。
//!
//! 为什么不直接塞进 fuxi-firehose 的 Router：firehose 不依赖 scheduler（避免循环），
//! 改由 `fuxi up` 在最外层把两个 Router `merge` 到同一 axum 监听端口。

use crate::store::FireCause;
use crate::{Error, Keeper, TriggerSpec, TriggerStore};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

/// Webhook router 的依赖注入——共享同一 store + keeper 句柄。
#[derive(Clone)]
pub struct WebhookState {
    pub store: TriggerStore,
    pub keeper: Arc<Keeper>,
}

/// 构造 axum Router，注册 `POST /hook/:id`。
pub fn router(state: WebhookState) -> Router {
    Router::new()
        .route("/hook/{id}", post(handle_hook))
        .with_state(state)
}

/// Hook 处理器——body 是任意 JSON（可选）；响应 202/404/403/410 等。
async fn handle_hook(
    State(state): State<WebhookState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> impl IntoResponse {
    let Ok(row) = state.store.get(&id).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
    };
    let Some(row) = row else {
        return (StatusCode::NOT_FOUND, "trigger not found").into_response();
    };
    if !row.enabled {
        return (StatusCode::GONE, "trigger disabled").into_response();
    }
    // 仅 webhook kind 接收
    let secret = match &row.spec {
        TriggerSpec::Webhook { secret } => secret.clone(),
        _ => return (StatusCode::BAD_REQUEST, "trigger kind mismatch").into_response(),
    };
    // 简单 secret 校验——请求头 `x-fuxi-secret` 或 body `secret` 字段
    if let Some(expected) = secret.as_deref() {
        let header_ok = headers
            .get("x-fuxi-secret")
            .and_then(|v| v.to_str().ok())
            .map(|s| s == expected)
            .unwrap_or(false);
        let body_ok = body
            .as_ref()
            .and_then(|b| b.get("secret"))
            .and_then(|v| v.as_str())
            .map(|s| s == expected)
            .unwrap_or(false);
        if !(header_ok || body_ok) {
            warn!(trigger_id = %id, "webhook secret 校验失败");
            return (StatusCode::FORBIDDEN, "invalid secret").into_response();
        }
    }
    let payload = body.map(|Json(v)| v);
    let fired_at = chrono::Utc::now();
    match state
        .keeper
        .record_and_emit_fire(&id, fired_at, FireCause::Webhook, payload)
        .await
    {
        Ok(_) => {
            info!(trigger_id = %id, "webhook 命中 → fire");
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"trigger_id": id, "fired_at": fired_at.to_rfc3339()})),
            )
                .into_response()
        }
        Err(Error::NotFound(_)) => (StatusCode::NOT_FOUND, "trigger not found").into_response(),
        Err(e) => {
            warn!(error = %e, "webhook fire 失败");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keeper::SystemClock;
    use crate::spec::TriggerSpec;
    use crate::store::NewTrigger;
    use crate::{Keeper, TriggerStore, new_trigger_id};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use fuxi_events::EventBus;
    use tower::util::ServiceExt;

    async fn fixture() -> (Router, TriggerStore, EventBus) {
        let store = TriggerStore::connect_memory().await.expect("store");
        let bus = EventBus::with_memory_store().await.expect("bus");
        let keeper = Arc::new(Keeper::new(
            store.clone(),
            bus.clone(),
            Arc::new(SystemClock),
        ));
        let router = router(WebhookState {
            store: store.clone(),
            keeper,
        });
        (router, store, bus)
    }

    #[tokio::test]
    async fn hook_404_when_trigger_missing() {
        let (app, _, _) = fixture().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/trg_nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn hook_accepts_valid_webhook_trigger() {
        let (app, store, bus) = fixture().await;
        let id = new_trigger_id();
        store
            .insert(NewTrigger {
                id: id.clone(),
                spec: TriggerSpec::Webhook { secret: None },
                intent: "收账".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("insert");

        let mut sub = bus.subscribe();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/hook/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"k":"v"}"#))
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        // 看到 bus 上的 TriggerFired cause=webhook
        use futures_util::StreamExt;
        use fuxi_core::EventKind;
        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        match ev.kind {
            EventKind::TriggerFired { id: fid, cause, .. } => {
                assert_eq!(fid, id);
                assert_eq!(cause, "webhook");
            }
            other => panic!("expected TriggerFired, got {other:?}"),
        }
        let fires = store.list_fires(&id).await.expect("fires");
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].cause, FireCause::Webhook);
    }

    #[tokio::test]
    async fn hook_rejects_wrong_secret() {
        let (app, store, _bus) = fixture().await;
        let id = new_trigger_id();
        store
            .insert(NewTrigger {
                id: id.clone(),
                spec: TriggerSpec::Webhook {
                    secret: Some("s3cr3t".into()),
                },
                intent: "x".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("insert");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/hook/{id}"))
                    .header("x-fuxi-secret", "wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn hook_410_when_disabled() {
        let (app, store, _bus) = fixture().await;
        let id = new_trigger_id();
        store
            .insert(NewTrigger {
                id: id.clone(),
                spec: TriggerSpec::Webhook { secret: None },
                intent: "x".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("insert");
        store.set_enabled(&id, false).await.expect("disable");
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/hook/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn hook_400_when_kind_not_webhook() {
        let (app, store, _bus) = fixture().await;
        let id = new_trigger_id();
        store
            .insert(NewTrigger {
                id: id.clone(),
                spec: TriggerSpec::Cron {
                    expr: "* * * * *".into(),
                    tz: None,
                },
                intent: "x".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("insert");
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/hook/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
