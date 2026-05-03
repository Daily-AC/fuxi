//! 玄女自启 helper —— `fuxi im start` 在没有 REPL 的家用部署里自己把玄女拉起。
//!
//! 与 [`crate::repl`] 启动玄女的路径**逻辑等价**（同 role / 同 session resume /
//! 同 cc launch），但**故意不复用** repl::run 的私有路径——repl 还要做 firehose
//! hub / dist controller / SystemEventBridge / TUI 装饰，IM daemon 都不要。
//!
//! ## 幂等
//!
//! - 调用前 `Fuxi::xuannv_id().await` 已 Some → 直接返回该 id（不重 spawn）
//! - 调用后并发再调一次（极端 race）→ 后者发现已 Some 也直接返回；不会出现
//!   两个玄女抢同 set_xuannv（last-write-wins，也都指同一只其实——但我们防御
//!   写在前置 check 里，避免起多余 cc 进程）
//!
//! ## 决策 04 / 06 / 14 兼容
//!
//! - 玄女 role 默认 "xuannv"——和 repl Args::xuannv_role 默认一致，用户可改
//! - resume 走 [`crate::session::resolve_xuannv_session`]：仅读策府不落盘；
//!   spawn 成功后调 [`crate::session::record_xuannv_session`] 落 session_id
//!   （#12 修：spawn 失败时不留脏 fact，避免下次启动 cc --resume 一个不存在的
//!   session 直接死循环）。重启后命中 → cc --resume；与 REPL 启的玄女**共享**
//!   session 连续性（两边都从同一 oracle_facts 读）
//! - 本 helper 只负责 spawn + set_xuannv；`fuxi im start` 的 caller 在拿到 id 后
//!   统一装配 conv_store sync + SystemEventBridge，保持 IM 和 REPL 的系统事件语义一致。

use anyhow::{Context, Result};
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::id::AgentId;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{Fuxi, WorkerKind};
use fuxi_skills as skill_loader;

/// 默认玄女 role 名——和 repl `--xuannv-role` 默认一致。
pub const DEFAULT_XUANNV_ROLE: &str = "xuannv";

/// 如果玄女尚未 spawn，则起一只；已就绪直接返回 id。
///
/// `role` 取值同 repl `args.xuannv_role`（默认 "xuannv"）。`oracle` 给 session
/// resume 用——和 repl 共享同一份策府保证 session 续写。
///
/// 错误传染语义：role 找不到 / cc launch 失败时，IM daemon 应该让用户感知（眼前
/// 一片白花花的玄女不存在错），而非静默吞掉——返回 `Err` 让上层 main fail-fast。
pub async fn ensure_xuannv(fuxi: &Fuxi, oracle: &OracleStore, role: &str) -> Result<AgentId> {
    if let Some(existing) = fuxi.xuannv_id().await {
        tracing::info!(xuannv = %existing, "玄女已就绪，跳过 spawn");
        return Ok(existing);
    }

    let loaded = skill_loader::load(role).with_context(|| format!("加载 roles/{role}/ROLE.md"))?;
    let xuannv_profile = loaded.profile.clone();

    let (resume_session_id, session_id) = crate::session::resolve_xuannv_session(oracle)
        .await
        .context("解析玄女 session_id")?;
    // #12：留住 fresh uuid（spawn 成功后才落盘）。已有 resume → session_id 是 None
    // 不需要 record；新 session → spawn ok 后写。
    let fresh_session_to_record = session_id.clone();

    let cc_cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        disallowed_tools: loaded.disallowed_tools,
        resume_session_id,
        session_id,
        ..Default::default()
    };

    let xuannv_id = fuxi
        .spawn_worker(xuannv_profile, WorkerKind::Cc(cc_cfg))
        .await
        .context("玄女 spawn 失败")?;
    fuxi.set_xuannv(xuannv_id).await;

    // #12：仅在新 session 路径（首次启动）才 record——spawn 成功 ⇒ 落盘安全
    if let Some(sid) = fresh_session_to_record
        && let Err(e) = crate::session::record_xuannv_session(oracle, &sid, "im-bootstrap").await
    {
        // record 失败不应让玄女进程跟着死——下次启动会重新生成 uuid（等于 fresh
        // start），代价是丢历史 session。warn 级提示运维。
        tracing::warn!(
            error = %e,
            session_id = %sid,
            "玄女 session 落策府失败——下次启动会作为新 session 重启（丢历史）"
        );
    }

    tracing::info!(xuannv = %xuannv_id, role, "玄女已自启");
    Ok(xuannv_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::id::AgentId;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tokio::process::Command;

    /// 造一份 Fuxi 句柄（空 git workspace，零依赖于 cc 实际启动）。
    /// 注意：本 helper 测试**只覆盖幂等分支**——已 set_xuannv 时直接返回。
    /// "首次 spawn" 路径要真起 cc 进程（unit test 太重），交 e2e 测试做。
    async fn make_fuxi() -> (tempfile::TempDir, Arc<Fuxi>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]).await;
        tokio::fs::write(path.join("README.md"), "seed")
            .await
            .unwrap();
        run_git(path, &["add", "-A"]).await;
        run_git(
            path,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .await;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        (dir, fuxi)
    }

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn returns_existing_id_when_xuannv_already_set() {
        let (_dir, fuxi) = make_fuxi().await;
        let oracle = OracleStore::connect_memory().await.expect("oracle");

        // 预置玄女 id —— 模拟 REPL 已先起过的场景
        let preset = AgentId::new();
        fuxi.set_xuannv(preset).await;

        let got = ensure_xuannv(&fuxi, &oracle, "irrelevant-role")
            .await
            .expect("应在已存在时直接返回");
        assert_eq!(got, preset, "幂等：返回的应是已经设置的 id");
        // 仍只此一玄女
        assert_eq!(fuxi.xuannv_id().await, Some(preset));
    }

    #[tokio::test]
    async fn idempotent_under_two_sequential_calls() {
        // 已就绪场景：连调两次都返回同一个 id，且 set_xuannv 不被错误地刷成新值。
        let (_dir, fuxi) = make_fuxi().await;
        let oracle = OracleStore::connect_memory().await.expect("oracle");

        let preset = AgentId::new();
        fuxi.set_xuannv(preset).await;

        let a = ensure_xuannv(&fuxi, &oracle, "x").await.unwrap();
        let b = ensure_xuannv(&fuxi, &oracle, "x").await.unwrap();
        assert_eq!(a, preset);
        assert_eq!(b, preset);
    }

    /// "未 set_xuannv 时尝试加载 role"——验证错误路径会带上明确的 role 信息，
    /// 不会静默 fallback 假玄女。这条不真起 cc，所以指定一个肯定不存在的 role。
    #[tokio::test]
    async fn missing_role_yields_explicit_error() {
        let (_dir, fuxi) = make_fuxi().await;
        let oracle = OracleStore::connect_memory().await.expect("oracle");
        // 不预置 xuannv_id → 进入加载分支
        let err = ensure_xuannv(&fuxi, &oracle, "definitely-not-a-real-role-12345")
            .await
            .expect_err("未知 role 应返错");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("definitely-not-a-real-role-12345"),
            "错误信息应含具体 role 名以便排查：{msg}"
        );
        // 未起成 → xuannv_id 仍为 None，不会留半途状态
        assert!(fuxi.xuannv_id().await.is_none());
    }

    /// #12 边界：role load / spawn 失败时不能留下 session 脏 fact。
    /// 本测能直接覆盖 role-load 路径（`skill_loader::load` 在 resolve 之前
    /// fail-fast），间接证明早期失败不污染策府。spawn 路径的失败覆盖在
    /// `session.rs::tests::no_record_after_failed_spawn_keeps_oracle_clean`：
    /// 那里直接断言 resolve 拿 uuid 后不调 record，oracle 仍空。
    #[tokio::test]
    async fn role_load_failure_leaves_oracle_session_unrecorded() {
        let (_dir, fuxi) = make_fuxi().await;
        let oracle = OracleStore::connect_memory().await.expect("oracle");

        let _err = ensure_xuannv(&fuxi, &oracle, "another-nonexistent-role")
            .await
            .expect_err("未知 role 应返错");

        let fact = oracle
            .query_one(
                crate::session::XUANNV_SUBJECT,
                crate::session::SESSION_PREDICATE,
            )
            .await
            .unwrap();
        assert!(
            fact.is_none(),
            "role load 失败应保持策府空——避免 redeploy 时 sqlite delete"
        );
    }
}
