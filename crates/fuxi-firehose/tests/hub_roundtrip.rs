//! hub roundtrip —— 起真实 axum server，WS + SSE + `/events` 三条路都验一遍。
//!
//! 为什么用 ephemeral port：并发测试时 0.0.0.0:固定端口会冲突；ephemeral 吃系统分配。
//! 测试里做端到端才能暴露 frame 边界/keep-alive/查询参数解析等“只跑 unit 看不出来”的问题。

use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskState};
use fuxi_events::EventBus;
use fuxi_firehose::{Hub, router};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use url::Url;

/// spawn：返回 `(base_url, bus, server_handle)`. base_url 已含 http scheme。
async fn spawn_hub() -> (String, EventBus, tokio::task::JoinHandle<()>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let hub = Arc::new(Hub::new(bus.clone()));
    let router = router(hub);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let h = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let base = format!("http://{addr}");
    (base, bus, h)
}

fn mk(label: &str) -> Event {
    Event {
        meta: EventMeta::now(),
        kind: EventKind::Custom {
            label: label.into(),
            payload: serde_json::json!({}),
        },
    }
}

fn label_of(ev: &Event) -> String {
    match &ev.kind {
        EventKind::Custom { label, .. } => label.clone(),
        _ => "__other__".into(),
    }
}

#[tokio::test]
async fn ws_live_receives_published_events_in_order() {
    let (base, bus, _srv) = spawn_hub().await;
    let ws_url = base.replace("http://", "ws://") + "/ws";
    let url = Url::parse(&ws_url).expect("parse");

    let mut stream = fuxi_firehose::connect_ws(url).await.expect("connect");

    // 让 subscriber 真正接上——留点窗口。
    tokio::time::sleep(Duration::from_millis(100)).await;

    let labels = ["a", "b", "c"];
    for l in labels {
        bus.publish(mk(l)).expect("publish");
    }

    for l in labels {
        let got = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        assert_eq!(label_of(&got), l);
    }
}

#[tokio::test]
async fn sse_live_receives_published_events() {
    let (base, bus, _srv) = spawn_hub().await;
    let url = Url::parse(&format!("{base}/sse")).expect("parse");
    let mut stream = fuxi_firehose::connect_sse(url).await.expect("connect");

    tokio::time::sleep(Duration::from_millis(100)).await;
    bus.publish(mk("via-sse")).expect("publish");

    let got = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");
    assert_eq!(label_of(&got), "via-sse");
}

#[tokio::test]
async fn ws_from_id_replays_then_tails() {
    let (base, bus, _srv) = spawn_hub().await;

    // 先发 3 条历史；先拿第一条 id 做 anchor。
    let a = mk("a");
    bus.publish(a.clone()).expect("publish");
    bus.publish(mk("b")).expect("publish");
    bus.publish(mk("c")).expect("publish");

    // 等 writer flush。
    tokio::time::sleep(Duration::from_millis(200)).await;

    let ws_url = base.replace("http://", "ws://") + &format!("/ws?from_id={}", a.meta.id);
    let url = Url::parse(&ws_url).expect("parse");
    let mut stream = fuxi_firehose::connect_ws(url).await.expect("connect");

    // 应该先收到 b、c（严格 *之后* a），再收到 live d。
    let first = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("t1")
        .expect("closed")
        .expect("err");
    let second = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("t2")
        .expect("closed")
        .expect("err");
    assert_eq!(label_of(&first), "b");
    assert_eq!(label_of(&second), "c");

    // 再发一条 live。
    tokio::time::sleep(Duration::from_millis(100)).await;
    bus.publish(mk("d")).expect("publish");
    let third = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("t3")
        .expect("closed")
        .expect("err");
    assert_eq!(label_of(&third), "d");
}

#[tokio::test]
async fn rest_events_returns_paginated_history() {
    let (base, bus, _srv) = spawn_hub().await;

    for i in 0..5 {
        bus.publish(mk(&format!("ev-{i}"))).expect("publish");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let http = reqwest::Client::new();
    let url = format!("{base}/events?limit=3");
    let resp = http.get(&url).send().await.expect("get");
    assert!(resp.status().is_success());
    let events: Vec<Event> = resp.json().await.expect("json");
    assert_eq!(events.len(), 3, "limit=3");
    assert_eq!(label_of(&events[0]), "ev-0");
    assert_eq!(label_of(&events[2]), "ev-2");
}

#[tokio::test]
async fn rest_events_filters_by_agent() {
    let (base, bus, _srv) = spawn_hub().await;

    let a = AgentId::new();
    let b = AgentId::new();
    let mk_with_agent = |who: AgentId, label: &str| {
        let mut m = EventMeta::now();
        m.agent = Some(who);
        Event {
            meta: m,
            kind: EventKind::Custom {
                label: label.into(),
                payload: serde_json::json!({}),
            },
        }
    };
    bus.publish(mk_with_agent(a, "a1")).expect("publish");
    bus.publish(mk_with_agent(b, "b1")).expect("publish");
    bus.publish(mk_with_agent(a, "a2")).expect("publish");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let http = reqwest::Client::new();
    let url = format!("{}/events?agent={}", base, a);
    let resp = http.get(&url).send().await.expect("get");
    assert!(resp.status().is_success());
    let events: Vec<Event> = resp.json().await.expect("json");
    assert_eq!(events.len(), 2);
    assert_eq!(label_of(&events[0]), "a1");
    assert_eq!(label_of(&events[1]), "a2");
}

#[tokio::test]
async fn rest_events_rejects_bad_uuid() {
    let (base, _bus, _srv) = spawn_hub().await;
    let http = reqwest::Client::new();
    let url = format!("{base}/events?from_id=not-a-uuid");
    let resp = http.get(&url).send().await.expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ws_rejects_bad_query() {
    let (base, _bus, _srv) = spawn_hub().await;
    let ws_url = base.replace("http://", "ws://") + "/ws?from_id=garbage";
    let url = Url::parse(&ws_url).expect("parse");
    let err = fuxi_firehose::connect_ws(url).await.err();
    assert!(err.is_some(), "应被 BAD_REQUEST 打回");
}

#[tokio::test]
async fn ws_stream_parses_typed_event() {
    let (base, bus, _srv) = spawn_hub().await;
    let ws_url = base.replace("http://", "ws://") + "/ws";
    let url = Url::parse(&ws_url).expect("parse");
    let mut stream = fuxi_firehose::connect_ws(url).await.expect("connect");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 非 Custom 的事件——验证 tag 联合正常解析。
    let ev = Event {
        meta: EventMeta::now(),
        kind: EventKind::TaskStateChanged {
            from: TaskState::New,
            to: TaskState::Ready,
        },
    };
    bus.publish(ev.clone()).expect("publish");

    let got = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("err");

    match got.kind {
        EventKind::TaskStateChanged { from, to } => {
            assert_eq!(from, TaskState::New);
            assert_eq!(to, TaskState::Ready);
        }
        other => panic!("unexpected {other:?}"),
    }
}
