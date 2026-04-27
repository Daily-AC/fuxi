//! `fuxi im start` 内嵌 dist controller 装配（β · #54，spec
//! `2026-04-27-im-dist-接通-design.md` gap a）。
//!
//! ## 为啥同进程
//!
//! - 共享 EventBus（dist worker 上报 events 直接进 home 玄女视野）
//! - 共享 SQLite WAL（dist_jobs 持久化）
//! - 决策 17 远期拆 binary 时可以拆，本设计不动 binary 边界
//!
//! ## 端口与路由分层
//!
//! `:9100` 端口共用，nginx 反代统一进 axum app；axum 内部按 path 分两条 layer：
//!
//! - `/api/*` 走 cookie auth layer（`fuxi-im::middleware::cookie_auth_layer`）
//! - `/dist/*` 走 HMAC layer（`fuxi-cli::dist_auth::hmac_layer`）
//! - 其他静态资源 `/`（PWA dist）由 fallback_service 兜底，两套 layer 都豁免
//!
//! cookie layer 的 `is_exempt` 分支已经把 `!path.starts_with("/api/")` 全放行，
//! `/dist/*` 自然不被它拦；HMAC layer 只挂在 `router_with_hmac` 内部对 `/dist/*`
//! 路由生效。两层互不干扰。
//!
//! ## 自启 home 节点
//!
//! controller 启动后立即 register 一个 `node_id="home"` 的自身节点（spec gap a：
//! "home 节点也要走 dist register 注册自己，保持 dist topology 视图统一"）。
//! tags 默认 `["home", "linux"]`，max_concurrency=4。这样 `/api/nodes` (#55) 看
//! 到 home 也是个真节点，不是特例。
//!
//! ## 配置来源
//!
//! - HMAC secret：`FUXI_DIST_HMAC_SECRET` env，缺失则**首启自生成**并落
//!   `~/.fuxi/dist_hmac.key`（同 fuxi-im::auth::HmacSecret 模式，0600 权限）
//! - dist token：`FUXI_DIST_TOKEN` env，缺失自生成落 `~/.fuxi/dist_token`
//! - JobPersistence DB：`~/.fuxi/dist_jobs.db`（独立于 events.db / im.db，避免
//!   schema 互相污染；JobPersistence 自带 schema init）
//!
//! 缺 secret/token 不 fail-fast——首启自动生成是部署友好。生产用户改了 env
//! 文件后重启自然覆盖。
//!
//! 落地点：`fuxi-cli/src/im.rs::run` 在 axum router merge 之前调
//! [`build_dist_layer`] 拿到 `(Arc<DistController>, Router)`，然后 `app.merge(dist_router)`。

use anyhow::{Context, Result};
use axum::Router;
use fuxi_events::EventBus;
use std::path::PathBuf;
use std::sync::Arc;

use crate::dist::{DIST_TOKEN_ENV, DistController, router_with_hmac, spawn_sweep_task};
use crate::dist_auth::{FUXI_DIST_HMAC_SECRET_ENV, HmacGate, HmacSecret};
use crate::dist_persistence::JobPersistence;

/// home 节点的固定 id——dist topology 视图里的稳定标识。
pub const HOME_NODE_ID: &str = "home";
/// home 节点声明的 capability tags——spec gap a 默认。
pub const HOME_NODE_TAGS: &[&str] = &["home", "linux"];
/// home 节点 max_concurrency 默认。后续若用户加 worker 实例可加 env 覆盖。
pub const HOME_NODE_MAX_CONCURRENCY: u32 = 4;

/// 装配 dist controller + HMAC router。返回 `(controller, router)` 给 caller
/// merge 进 axum app。
///
/// `home_dir`：用于落 token / hmac key / dist_jobs.db 的根目录（生产 = `~/.fuxi`）。
/// caller 已 mkdir -p。
pub async fn build_dist_layer(home_dir: &std::path::Path, bus: EventBus) -> Result<DistLayer> {
    // 1. resolve secret——env 优先，缺失自生成 + 落盘
    let hmac_secret = resolve_hmac_secret(home_dir).context("解析 dist HMAC secret")?;
    // 2. resolve dist token（暂只 register/heartbeat 用，HMAC 已是主鉴权；token
    //    保留作 controller URL 内嵌的 noise，跟 cli `--token` 兼容）
    let dist_token = resolve_dist_token(home_dir).context("解析 dist token")?;

    // 3. JobPersistence——独立 SQLite 文件
    let dist_db_path = home_dir.join("dist_jobs.db");
    let persistence = JobPersistence::connect_file(&dist_db_path)
        .await
        .with_context(|| format!("打开 dist_jobs SQLite {}", dist_db_path.display()))?;

    // 4. controller + persistence
    let ctrl = Arc::new(DistController::new_with_persistence(
        dist_token,
        bus,
        Arc::new(persistence),
    ));

    // restore in-flight from SQLite——controller 重启后保留旧 queue。
    let (queued_n, orphan_n) = ctrl.restore_from_persistence().await;
    if queued_n + orphan_n > 0 {
        tracing::info!(queued_n, orphan_n, "dist controller 重启 restore 完成");
    }

    // 5. sweep task——把 stale worker 的 inflight 翻回 queue
    spawn_sweep_task(ctrl.clone());

    // 6. self-register home node
    ctrl.register(
        HOME_NODE_ID.to_string(),
        HOME_NODE_TAGS.iter().map(|s| s.to_string()).collect(),
        HOME_NODE_MAX_CONCURRENCY,
    )
    .await;
    tracing::info!(
        node_id = HOME_NODE_ID,
        tags = ?HOME_NODE_TAGS,
        max_concurrency = HOME_NODE_MAX_CONCURRENCY,
        "dist controller: home 节点已自注册"
    );

    // 7. router with HMAC layer
    let gate = HmacGate::new(hmac_secret);
    let router = router_with_hmac(ctrl.clone(), gate);

    Ok(DistLayer { ctrl, router })
}

/// `build_dist_layer` 的产物——分两块返便于 caller 各取所需：
/// - `ctrl` 给 daemon `with_dist` 用 + 后续 #55 `/api/nodes` 共享
/// - `router` 给 axum app `.merge()`
pub struct DistLayer {
    pub ctrl: Arc<DistController>,
    pub router: Router,
}

/// 解析 HMAC secret——env 优先；缺失则读 `~/.fuxi/dist_hmac.key`；都无则首启
/// 自生成 + 落盘（0600）。同 fuxi-im 主 auth 的 load-or-create 模式。
fn resolve_hmac_secret(home_dir: &std::path::Path) -> Result<HmacSecret> {
    if let Ok(secret) = HmacSecret::from_env() {
        tracing::info!("dist HMAC secret 来源：${FUXI_DIST_HMAC_SECRET_ENV}");
        return Ok(secret);
    }
    let key_path = home_dir.join("dist_hmac.key");
    if let Ok(content) = std::fs::read_to_string(&key_path) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            tracing::info!(path = %key_path.display(), "dist HMAC secret 来源：本地文件");
            return Ok(HmacSecret::new(trimmed.to_string()));
        }
    }
    // 自生成 + 落盘
    let generated = generate_random_token(64);
    write_secret_file(&key_path, &generated)
        .with_context(|| format!("落 dist HMAC secret 文件 {}", key_path.display()))?;
    tracing::warn!(
        path = %key_path.display(),
        "dist HMAC secret 自动生成——首启场景；后续可改 ${FUXI_DIST_HMAC_SECRET_ENV} 或本地文件"
    );
    Ok(HmacSecret::new(generated))
}

/// 解析 dist token——env 优先；缺失则读 `~/.fuxi/dist_token`；都无自生成。
fn resolve_dist_token(home_dir: &std::path::Path) -> Result<String> {
    if let Ok(t) = std::env::var(DIST_TOKEN_ENV)
        && !t.is_empty()
    {
        tracing::info!("dist token 来源：${DIST_TOKEN_ENV}");
        return Ok(t);
    }
    let token_path = home_dir.join("dist_token");
    if let Ok(content) = std::fs::read_to_string(&token_path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            tracing::info!(path = %token_path.display(), "dist token 来源：本地文件");
            return Ok(trimmed);
        }
    }
    let generated = generate_random_token(32);
    write_secret_file(&token_path, &generated)
        .with_context(|| format!("落 dist token 文件 {}", token_path.display()))?;
    tracing::warn!(
        path = %token_path.display(),
        "dist token 自动生成——首启场景"
    );
    Ok(generated)
}

/// 简单随机串（hex）——用 OS RNG 而非 uuid 避免 dependency；secret/token 都用。
fn generate_random_token(byte_len: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; byte_len];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 写 secret 文件 + 设 0600 权限（仅 unix）。
fn write_secret_file(path: &PathBuf, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_events::EventBus;
    use tempfile::TempDir;

    #[tokio::test]
    async fn build_dist_layer_creates_controller_and_self_registers_home() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        // env 必须是干净的，否则会走 env 路径 → 不创建文件
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        let layer = build_dist_layer(dir.path(), bus).await.expect("build");
        // home 节点应在 nodes_snapshot 里（自注册成功）
        let snap = layer.ctrl.nodes_snapshot().await;
        assert_eq!(snap.len(), 1, "home 节点应已自注册");
        assert_eq!(snap[0].node_id, HOME_NODE_ID);
        assert_eq!(
            snap[0].tags,
            HOME_NODE_TAGS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // secret 和 token 都落了盘（env 不存在，走自生成路径）
        assert!(dir.path().join("dist_hmac.key").exists());
        assert!(dir.path().join("dist_token").exists());
        // dist_jobs.db 也建了（JobPersistence::connect_file create_if_missing）
        assert!(dir.path().join("dist_jobs.db").exists());
    }

    #[tokio::test]
    async fn build_dist_layer_reads_existing_files_on_second_call() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        // 首次跑生成文件
        let _ = build_dist_layer(dir.path(), bus).await.expect("build 1");
        let secret_v1 = std::fs::read_to_string(dir.path().join("dist_hmac.key")).unwrap();
        let token_v1 = std::fs::read_to_string(dir.path().join("dist_token")).unwrap();

        // 第二次跑应读已有文件，不重生成
        let bus2 = EventBus::with_memory_store().await.expect("bus");
        let _ = build_dist_layer(dir.path(), bus2).await.expect("build 2");
        let secret_v2 = std::fs::read_to_string(dir.path().join("dist_hmac.key")).unwrap();
        let token_v2 = std::fs::read_to_string(dir.path().join("dist_token")).unwrap();
        assert_eq!(secret_v1, secret_v2, "HMAC secret 不应被重生成");
        assert_eq!(token_v1, token_v2, "dist token 不应被重生成");
    }

    #[tokio::test]
    async fn build_dist_layer_writes_0600_permissions() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        let _ = build_dist_layer(dir.path(), bus).await.expect("build");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let secret_perm = std::fs::metadata(dir.path().join("dist_hmac.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(secret_perm & 0o777, 0o600, "secret 文件应是 0600");
            let token_perm = std::fs::metadata(dir.path().join("dist_token"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(token_perm & 0o777, 0o600);
        }
    }
}
