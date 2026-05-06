//! 甲骨存取契约测试（R1 / M1.1 TDD 契约 1-3 / 置信度）。

use fuxi_memory::{NewFact, OracleStore};

async fn store() -> OracleStore {
    OracleStore::connect_memory().await.expect("connect memory")
}

#[tokio::test]
async fn insert_then_query_returns_the_fact() {
    let s = store().await;
    let f = s
        .insert(
            NewFact::new("user", "prefers", "冰美式")
                .with_source("user")
                .with_confidence(0.9),
        )
        .await
        .expect("insert");

    assert_eq!(f.subject, "user");
    assert_eq!(f.predicate, "prefers");
    assert_eq!(f.object, "冰美式");
    assert!(f.valid_until.is_none());
    assert!((f.confidence - 0.9).abs() < 1e-4);

    let got = s.query("user", 10).await.expect("query");
    assert_eq!(got.len(), 1, "刚插入的条目应该能查到");
    assert_eq!(got[0].id, f.id);
}

#[tokio::test]
async fn list_active_returns_all_subjects_excluding_superseded() {
    // PWA「记忆」tab · 跨 subject 列现行事实。supersede 走掉的不应该出现。
    let s = store().await;
    s.insert(NewFact::new("user", "prefers", "冰美式"))
        .await
        .unwrap();
    s.insert(NewFact::new("luban", "role", "工匠"))
        .await
        .unwrap();
    let stale = s
        .insert(NewFact::new("xuannv", "session_id", "old"))
        .await
        .unwrap();
    s.supersede(
        stale.id,
        NewFact::new("xuannv", "session_id", "new").with_source("agent"),
    )
    .await
    .unwrap();

    let all = s.list_active(50).await.expect("list_active");
    let subjects: Vec<&str> = all.iter().map(|f| f.subject.as_str()).collect();
    assert_eq!(all.len(), 3, "user + luban + xuannv 各一现行");
    assert!(subjects.contains(&"user"));
    assert!(subjects.contains(&"luban"));
    assert!(subjects.contains(&"xuannv"));
    let xuannv = all.iter().find(|f| f.subject == "xuannv").unwrap();
    assert_eq!(xuannv.object, "new");
}

#[tokio::test]
async fn query_ignores_superseded_facts() {
    // 公理：query 只返现行（valid_until IS NULL）。
    let s = store().await;
    let old = s
        .insert(NewFact::new("xuannv", "session_id", "sess-old"))
        .await
        .unwrap();
    let _new = s
        .supersede(
            old.id,
            NewFact::new("xuannv", "session_id", "sess-new").with_source("agent"),
        )
        .await
        .unwrap();

    let got = s.query("xuannv", 10).await.unwrap();
    assert_eq!(got.len(), 1, "supersede 之后应只剩一条现行");
    assert_eq!(got[0].object, "sess-new");
}

#[tokio::test]
async fn supersede_marks_old_valid_until_and_inserts_new() {
    // 直接读 DB 验证老行的 valid_until 被置上，新行是独立 id。
    let s = store().await;
    let old = s
        .insert(NewFact::new("luban", "role_style", "硬核"))
        .await
        .unwrap();
    let new = s
        .supersede(old.id, NewFact::new("luban", "role_style", "温和"))
        .await
        .unwrap();
    assert_ne!(new.id, old.id, "supersede 必须分配新 id");

    // `get(id)` 不受 valid_until 约束——拿到老行看它的 valid_until 是否被填。
    let loaded_old = s.get(old.id).await.unwrap().expect("老行仍可按 id 查出");
    assert!(
        loaded_old.valid_until.is_some(),
        "老行 valid_until 应被填上"
    );
}

#[tokio::test]
async fn supersede_rejects_already_superseded() {
    let s = store().await;
    let a = s.insert(NewFact::new("s", "p", "v1")).await.unwrap();
    let _b = s
        .supersede(a.id, NewFact::new("s", "p", "v2"))
        .await
        .unwrap();
    // 第二次对同一老 id supersede 应失败。
    let err = s
        .supersede(a.id, NewFact::new("s", "p", "v3"))
        .await
        .expect_err("should fail");
    assert!(matches!(err, fuxi_memory::Error::NotFound(_)));
}

#[tokio::test]
async fn fts_search_hits_chinese_subject() {
    let s = store().await;
    s.insert(NewFact::new("玄女", "persona", "九天玄女，授兵策"))
        .await
        .unwrap();
    s.insert(NewFact::new("luban", "persona", "工匠鼻祖"))
        .await
        .unwrap();

    let hits = s.fts_search("玄女", 10).await.expect("fts");
    assert_eq!(hits.len(), 1, "FTS5 应命中含 '玄女' 的条目");
    assert_eq!(hits[0].subject, "玄女");
}

#[tokio::test]
async fn fts_search_matches_predicate_and_object() {
    // FTS5 应对 subject/predicate/object 三列都索引——predicate/object 也能命中。
    let s = store().await;
    s.insert(NewFact::new("user", "prefers", "冰美式无糖"))
        .await
        .unwrap();
    let hits = s.fts_search("冰美式", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    let hits2 = s.fts_search("prefers", 10).await.unwrap();
    assert_eq!(hits2.len(), 1);
}

#[tokio::test]
async fn fts_search_skips_superseded() {
    let s = store().await;
    let old = s
        .insert(NewFact::new("topic", "note", "唯一可识别词 kuichen"))
        .await
        .unwrap();
    s.supersede(old.id, NewFact::new("topic", "note", "another"))
        .await
        .unwrap();
    let hits = s.fts_search("kuichen", 10).await.unwrap();
    assert_eq!(hits.len(), 0, "失效条目不应出现在 FTS 结果中");
}

#[tokio::test]
async fn fts_search_empty_query_is_empty() {
    let s = store().await;
    s.insert(NewFact::new("a", "b", "c")).await.unwrap();
    let hits = s.fts_search("   ", 10).await.unwrap();
    assert!(hits.is_empty(), "空查询应直接返空列表（不应跑 FTS5）");
}

#[tokio::test]
async fn update_confidence_clamps_and_persists() {
    let s = store().await;
    let f = s
        .insert(NewFact::new("s", "p", "v").with_confidence(0.5))
        .await
        .unwrap();
    let up = s.update_confidence(f.id, 0.3).await.unwrap();
    assert!((up.confidence - 0.8).abs() < 1e-4);
    // 上越界 clamp 到 1.0
    let up2 = s.update_confidence(f.id, 1.0).await.unwrap();
    assert!((up2.confidence - 1.0).abs() < 1e-4);
    // 下越界 clamp 到 0.0
    let up3 = s.update_confidence(f.id, -5.0).await.unwrap();
    assert!((up3.confidence - 0.0).abs() < 1e-4);
}

#[tokio::test]
async fn insert_rejects_out_of_range_confidence() {
    let s = store().await;
    let err = s
        .insert(NewFact::new("s", "p", "v").with_confidence(1.5))
        .await
        .expect_err("should reject");
    assert!(matches!(err, fuxi_memory::Error::InvalidArgument(_)));
}

#[tokio::test]
async fn query_one_returns_latest_active() {
    let s = store().await;
    let old = s
        .insert(NewFact::new("xuannv", "session_id", "sess-1"))
        .await
        .unwrap();
    s.supersede(old.id, NewFact::new("xuannv", "session_id", "sess-2"))
        .await
        .unwrap();
    let got = s.query_one("xuannv", "session_id").await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().object, "sess-2");
}

#[tokio::test]
async fn get_returns_none_for_missing_id() {
    let s = store().await;
    let got = s.get(uuid::Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}
