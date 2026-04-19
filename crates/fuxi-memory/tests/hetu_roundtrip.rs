//! 河图洛书存取契约测试。

use fuxi_memory::{HetuStore, NewPattern};

async fn store() -> HetuStore {
    HetuStore::connect_memory().await.expect("connect memory")
}

#[tokio::test]
async fn record_then_query_by_role() {
    let s = store().await;
    let p = s
        .record(
            NewPattern::new("luban", "refactor", "先写小单测再开始", "success")
                .with_confidence(0.7),
        )
        .await
        .unwrap();

    assert_eq!(p.role, "luban");
    assert_eq!(p.task_type, "refactor");
    assert!(!p.promoted_to_skill);
    assert!((p.confidence - 0.7).abs() < 1e-4);

    let got = s.query("luban", "refactor").await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, p.id);
}

#[tokio::test]
async fn query_without_task_type_returns_all_role_rows() {
    let s = store().await;
    s.record(NewPattern::new("cangjie", "research", "x", "success"))
        .await
        .unwrap();
    s.record(NewPattern::new("cangjie", "write", "y", "partial"))
        .await
        .unwrap();
    s.record(NewPattern::new("luban", "refactor", "z", "success"))
        .await
        .unwrap();

    let rows = s.query("cangjie", "").await.unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn query_orders_by_confidence_desc() {
    let s = store().await;
    s.record(NewPattern::new("r", "t", "low", "partial").with_confidence(0.3))
        .await
        .unwrap();
    s.record(NewPattern::new("r", "t", "high", "success").with_confidence(0.9))
        .await
        .unwrap();
    let rows = s.query("r", "t").await.unwrap();
    assert_eq!(rows[0].pattern, "high", "高置信在前");
    assert_eq!(rows[1].pattern, "low");
}

#[tokio::test]
async fn promote_sets_flag_and_is_idempotent() {
    let s = store().await;
    let p = s
        .record(NewPattern::new("r", "t", "pp", "success"))
        .await
        .unwrap();
    assert!(!p.promoted_to_skill);

    let p2 = s.promote(p.id).await.unwrap();
    assert!(p2.promoted_to_skill);

    // 二次 promote 也 OK——幂等。
    let p3 = s.promote(p.id).await.unwrap();
    assert!(p3.promoted_to_skill);
}

#[tokio::test]
async fn promote_unknown_id_returns_not_found() {
    let s = store().await;
    let err = s.promote(uuid::Uuid::new_v4()).await.err().unwrap();
    assert!(matches!(err, fuxi_memory::Error::NotFound(_)));
}

#[tokio::test]
async fn record_rejects_bad_confidence() {
    let s = store().await;
    let err = s
        .record(NewPattern::new("r", "t", "p", "o").with_confidence(-0.1))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, fuxi_memory::Error::InvalidArgument(_)));
}

#[tokio::test]
async fn list_all_returns_recent_first() {
    let s = store().await;
    let a = s
        .record(NewPattern::new("r", "t", "first", "success"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let b = s
        .record(NewPattern::new("r", "t", "second", "success"))
        .await
        .unwrap();
    let rows = s.list_all(10).await.unwrap();
    assert_eq!(rows.len(), 2);
    // 最新的排前面（created_at DESC）
    assert_eq!(rows[0].id, b.id);
    assert_eq!(rows[1].id, a.id);
}
