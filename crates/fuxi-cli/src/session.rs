//! 玄女 cc session 续写——把 `session_id` 锚在策府上。
//!
//! 设计：启动时读 `oracle_facts[subject=xuannv,predicate=session_id]`。
//! - 命中 → 走 `cc --resume <id>`（把 `CcLaunchConfig.resume_session_id` 填上）
//! - 未命中 → 生成新 uuid 作为 `CcLaunchConfig.session_id` 传给 cc，**同时**
//!   写回策府——因为我们是 session 源头，cc 会尊重外部指派的 id，下次启动
//!   走 `query_one` 直接命中无需订阅 `system/init` 事件。
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
/// | 未命中     | `(None, Some(new_uuid))`  | 新 session 并落盘 |
pub async fn resolve_xuannv_session(
    oracle: &OracleStore,
) -> Result<(Option<String>, Option<String>)> {
    if let Some(fact) = oracle.query_one(XUANNV_SUBJECT, SESSION_PREDICATE).await? {
        return Ok((Some(fact.object), None));
    }
    let new_id = Uuid::new_v4().to_string();
    oracle
        .insert(
            NewFact::new(XUANNV_SUBJECT, SESSION_PREDICATE, &new_id).with_source("repl-bootstrap"),
        )
        .await?;
    Ok((None, Some(new_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空库 → 生成 uuid v4、写回 oracle、返回 `session_id`。
    #[tokio::test]
    async fn first_run_generates_and_persists_new_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let (resume, session) = resolve_xuannv_session(&oracle).await.unwrap();

        assert!(resume.is_none(), "空库不应触发 resume");
        let sid = session.expect("空库应生成新 session_id");
        // uuid v4 的标准字符串形态 `8-4-4-4-12` = 36 字节
        assert_eq!(sid.len(), 36, "期望 uuid v4 长度 36，实际 {sid:?}");

        // 落盘校验：再查应命中同一个 id
        let fact = oracle
            .query_one(XUANNV_SUBJECT, SESSION_PREDICATE)
            .await
            .unwrap()
            .expect("首次生成后策府应有事实");
        assert_eq!(fact.object, sid);
        assert_eq!(fact.source, "repl-bootstrap", "source 标记便于后续 audit");
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

    /// 连跑两次：第一次应生成，第二次应走 resume 且 id 相同。
    #[tokio::test]
    async fn second_call_resumes_first_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();

        let (_, first_session) = resolve_xuannv_session(&oracle).await.unwrap();
        let first_id = first_session.unwrap();

        let (second_resume, second_session) = resolve_xuannv_session(&oracle).await.unwrap();
        assert_eq!(second_resume.as_deref(), Some(first_id.as_str()));
        assert!(second_session.is_none());
    }
}
