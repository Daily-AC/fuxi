//! 玄女 cc session 续写——把 `session_id` 锚在策府上。
//!
//! 设计：启动时读 `oracle_facts[subject=xuannv,predicate=session_id]`。
//! - 命中 → 走 `cc --resume <id>`（把 `CcLaunchConfig.resume_session_id` 填上）
//! - 未命中 → 生成新 uuid 作为 `CcLaunchConfig.session_id` 传给 cc。**spawn
//!   成功后**才把 id 写回策府——避免 spawn 失败留下指向不存在 session 的脏事实。
//!
//! ## #12 修：分两阶段，read-then-confirm
//!
//! 旧路径 [`resolve_xuannv_session`] 在生成 uuid 时就 insert，spawn 失败后下次
//! 启动走 resume 但 cc 历史文件不存在 → 立死循环。用户每次 redeploy 必 sqlite
//! delete 才能恢复，太脆。
//!
//! 现路径分开两个函数：
//! 1. [`resolve_xuannv_session`] 只**读**——命中返 resume，未命中生成 uuid 但
//!    **不**落盘，调用方拿到 uuid 用于 cc launch
//! 2. [`record_xuannv_session`] 调用方在 `spawn_worker` 成功后调一次——把首次
//!    生成的 uuid 落策府。idempotent：已有相同 fact 时 skip
//!
//! **v1 兜底缺口**：resume 指向的 cc 历史文件如被删 → cc 启动即死。v1 不处理，
//! 由门客死亡信号走正常 Dead 流程；v1.1 再加 fallback「resume 失败清掉旧事实重来」。

use anyhow::Result;
use fuxi_memory::{NewFact, OracleStore};
use uuid::Uuid;

/// 玄女在策府里的 subject 键。和 `docs/handoff/v1-session2.md` 约定一致。
pub const XUANNV_SUBJECT: &str = "xuannv";
/// session_id 在策府里的 predicate 键。
pub const SESSION_PREDICATE: &str = "session_id";

/// 为玄女 cc 解析启动 session 参数。
///
/// 返回值按顺序塞 [`fuxi_agent_cc::CcLaunchConfig`] 的
/// `resume_session_id` 和 `session_id`（两者互斥，最多一个 `Some`）：
///
/// | oracle 状态 | 返回                        | 行为              |
/// |------------|---------------------------|-------------------|
/// | 命中       | `(Some(id), None)`        | cc `--resume <id>`|
/// | 未命中     | `(None, Some(new_uuid))`  | 新 session（**不落盘**）|
///
/// 调用方拿到新 uuid 后传 cc launch；spawn 成功后再调
/// [`record_xuannv_session`] 把 uuid 落策府。spawn 失败则 uuid 自然丢弃，
/// oracle 状态保持纯净。
pub async fn resolve_xuannv_session(
    oracle: &OracleStore,
) -> Result<(Option<String>, Option<String>)> {
    if let Some(fact) = oracle.query_one(XUANNV_SUBJECT, SESSION_PREDICATE).await? {
        return Ok((Some(fact.object), None));
    }
    let new_id = Uuid::new_v4().to_string();
    Ok((None, Some(new_id)))
}

/// 让玄女**忘掉**当前 session record——下次 `ensure_xuannv` 走 fresh session
/// 路径，cc 会重读 `--append-system-prompt`（含 dispatch-routing.md 最新教学）。
///
/// 用途：dispatch-routing.md 教学更新后，旧玄女 cc 进程走 `--resume` 仍带旧
/// system prompt（cc 自身行为：resume 时 honor 老 session prompt）。用户跑
/// `fuxi xuannv refresh` → 调本函数 + 关掉当前玄女进程。
///
/// 返回失效的 fact 行数（0 = 之前就没 record；1 = 通常情况；>1 = 历史遗留多条）。
pub async fn forget_xuannv_session(oracle: &OracleStore) -> Result<u64> {
    Ok(oracle.invalidate(XUANNV_SUBJECT, SESSION_PREDICATE).await?)
}

/// spawn 成功后调用——把首次生成的 session_id 锚到策府。
///
/// idempotent：若策府已有相同 (subject, predicate, object) → skip 不重写
/// （避免重复 audit 行）；若有但 object 不同 → 仍 insert 一条（latest-write-wins
/// 由 `query_one` 处理，新 session 自然覆盖旧）。
///
/// `source` 让审计能区分 IM daemon `im-bootstrap` vs REPL `repl-bootstrap`。
pub async fn record_xuannv_session(
    oracle: &OracleStore,
    session_id: &str,
    source: &str,
) -> Result<()> {
    if let Some(existing) = oracle.query_one(XUANNV_SUBJECT, SESSION_PREDICATE).await?
        && existing.object == session_id
    {
        return Ok(());
    }
    oracle
        .insert(NewFact::new(XUANNV_SUBJECT, SESSION_PREDICATE, session_id).with_source(source))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `forget_xuannv_session` 让下次 resolve 走 fresh path（None resume + Some 新 uuid）。
    #[tokio::test]
    async fn forget_then_resolve_yields_fresh_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        // 先 record 一个老 session
        record_xuannv_session(&oracle, "old-session-uuid", "test-bootstrap")
            .await
            .unwrap();
        let (resume, _) = resolve_xuannv_session(&oracle).await.unwrap();
        assert_eq!(resume.as_deref(), Some("old-session-uuid"));

        // forget 之后 resolve 应该走 fresh 路径
        let cleared = forget_xuannv_session(&oracle).await.unwrap();
        assert_eq!(cleared, 1, "应失效 1 条 record");

        let (resume2, fresh) = resolve_xuannv_session(&oracle).await.unwrap();
        assert!(resume2.is_none(), "forget 后不应再返 resume");
        assert!(fresh.is_some(), "forget 后应返 fresh uuid");
    }

    /// `forget_xuannv_session` 在没 record 时 noop（cleared=0），不报错。
    #[tokio::test]
    async fn forget_xuannv_session_noop_on_empty() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let cleared = forget_xuannv_session(&oracle).await.unwrap();
        assert_eq!(cleared, 0);
    }

    /// 空库 → 生成 uuid v4 但**不**落盘（#12 修：先返 id，spawn 成功后再 record）。
    #[tokio::test]
    async fn first_run_generates_uuid_but_does_not_persist() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let (resume, session) = resolve_xuannv_session(&oracle).await.unwrap();

        assert!(resume.is_none(), "空库不应触发 resume");
        let sid = session.expect("空库应生成新 session_id");
        // uuid v4 的标准字符串形态 `8-4-4-4-12` = 36 字节
        assert_eq!(sid.len(), 36, "期望 uuid v4 长度 36，实际 {sid:?}");

        // #12：spawn 前不落盘——策府仍空，下次 resolve 还会生成新 uuid
        let fact = oracle
            .query_one(XUANNV_SUBJECT, SESSION_PREDICATE)
            .await
            .unwrap();
        assert!(
            fact.is_none(),
            "spawn 前不应有事实落盘——避免 spawn 失败留脏数据"
        );
    }

    /// 预置事实 → 返回 resume、不重新生成、不二次写入。
    #[tokio::test]
    async fn subsequent_run_returns_resume_and_skips_write() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        oracle
            .insert(NewFact::new(
                XUANNV_SUBJECT,
                SESSION_PREDICATE,
                "existing-sess",
            ))
            .await
            .unwrap();

        let (resume, session) = resolve_xuannv_session(&oracle).await.unwrap();
        assert_eq!(resume.as_deref(), Some("existing-sess"));
        assert!(session.is_none(), "命中 resume 时不应重新生成 session_id");

        // 幂等性：策府里仍是单一生效事实（query_one 只回最新一条）。
        let fact = oracle
            .query_one(XUANNV_SUBJECT, SESSION_PREDICATE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fact.object, "existing-sess");
    }

    /// #12：旧"连跑两次第二次走 resume"行为仍要求——但现在依赖调用方先调
    /// `record_xuannv_session` 落盘。模拟 spawn 成功路径：resolve → record → 再
    /// resolve 应命中。
    #[tokio::test]
    async fn record_then_resolve_resumes_first_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();

        let (_, first_session) = resolve_xuannv_session(&oracle).await.unwrap();
        let first_id = first_session.unwrap();
        // 模拟 spawn_worker 成功后的落盘
        record_xuannv_session(&oracle, &first_id, "test-bootstrap")
            .await
            .unwrap();

        let (second_resume, second_session) = resolve_xuannv_session(&oracle).await.unwrap();
        assert_eq!(second_resume.as_deref(), Some(first_id.as_str()));
        assert!(second_session.is_none());
    }

    /// #12：spawn 失败路径——resolve 拿到 uuid 但**不 record**，下次 resolve 仍
    /// 是空库（重新生成）。这是 #12 的核心修复：脏 fact 不会留下。
    #[tokio::test]
    async fn no_record_after_failed_spawn_keeps_oracle_clean() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let (_, first) = resolve_xuannv_session(&oracle).await.unwrap();
        let _ = first.expect("应有 uuid");
        // 模拟 spawn 失败：不调 record_xuannv_session

        // 下次启动 resolve 仍是空库——新 uuid，不会指向不存在的 cc 历史
        let (resume, second) = resolve_xuannv_session(&oracle).await.unwrap();
        assert!(resume.is_none(), "spawn 失败后不应该走 resume");
        assert!(second.is_some(), "重新生成 uuid");
    }

    /// `record_xuannv_session` idempotent：同 session_id 重复调不重写
    /// （避免 audit 行无谓增长）。
    #[tokio::test]
    async fn record_is_idempotent_for_same_session_id() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        record_xuannv_session(&oracle, "sess-A", "src1")
            .await
            .unwrap();
        record_xuannv_session(&oracle, "sess-A", "src2")
            .await
            .unwrap();

        // 仍是同一条 fact，source 来自首次写入（不重写）
        let fact = oracle
            .query_one(XUANNV_SUBJECT, SESSION_PREDICATE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fact.object, "sess-A");
        assert_eq!(fact.source, "src1", "重复 session 不应重写 source");
    }

    /// `record_xuannv_session` 不同 session_id 时仍 insert（latest-write-wins）。
    #[tokio::test]
    async fn record_inserts_when_session_id_differs() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        record_xuannv_session(&oracle, "sess-A", "src1")
            .await
            .unwrap();
        record_xuannv_session(&oracle, "sess-B", "src2")
            .await
            .unwrap();
        let fact = oracle
            .query_one(XUANNV_SUBJECT, SESSION_PREDICATE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fact.object, "sess-B", "新 session 应 latest-write-wins");
    }
}
