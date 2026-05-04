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
use async_trait::async_trait;
use axum::Router;
use fuxi_core::CoreError;
use fuxi_core::task::Task;
use fuxi_events::EventBus;
use fuxi_im::nodes_provider::{
    NodeView, NodesProvider, ONLINE_HEARTBEAT_THRESHOLD_MS, home_workers_from_shelf,
};
use fuxi_orchestrator::dist_enqueuer::DistEnqueuer;
use fuxi_orchestrator::{Fuxi, Result as OrchResult};
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
/// home 自心跳间隔。比远端 worker 的 10s 略短，确保 30s online 阈值（spec gap c）
/// 内必有 ≥3 次心跳成功才掉线，给瞬时锁竞争留余量。
pub const HOME_HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 装配 dist controller + HMAC router。返回 `(controller, router)` 给 caller
/// merge 进 axum app。
///
/// `home_dir`：用于落 token / hmac key / dist_jobs.db 的根目录（生产 = `~/.fuxi`）。
/// caller 已 mkdir -p。
pub async fn build_dist_layer(home_dir: &std::path::Path, bus: EventBus) -> Result<DistLayer> {
    // 1. resolve secret——env 优先，缺失自生成 + 落盘
    //    返 (HmacSecret 给 HMAC layer, 明文 String 给 #56 setup-worker 派发)
    let (hmac_secret, hmac_secret_plain) =
        resolve_hmac_secret(home_dir).context("解析 dist HMAC secret")?;
    // 2. resolve dist token（暂只 register/heartbeat 用，HMAC 已是主鉴权；token
    //    保留作 controller URL 内嵌的 noise，跟 cli `--token` 兼容）
    let dist_token = resolve_dist_token(home_dir).context("解析 dist token")?;
    // β · #56 onboarding 端点把这两个明文派给本地 worker 写
    // ~/.fuxi/dist-worker.env，存进 DistLayer.secrets 由 caller 注入 AppState。
    let dist_token_plain = dist_token.clone();

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

    // 6a. home 自心跳——register 只设一次 last_seen，无人续 → online 阈值
    //     (30s spec gap c) 一过就被前端误判离线。daemon 同进程没有 dist worker
    //     loop 给自己发心跳，由 caller (im.rs) 起 `spawn_home_heartbeat_task`
    //     传 Arc<Fuxi> 进来——bug #77：心跳 inflight 取本地非 idle worker 数，
    //     让 home 节点 0/4 反映真实占用而不是固定 0。
    //     此处不再起，避免 build_dist_layer 跟 Fuxi 强耦合（测试零依赖）。

    // 7. router with HMAC layer
    let gate = HmacGate::new(hmac_secret);
    let router = router_with_hmac(ctrl.clone(), gate);

    Ok(DistLayer {
        ctrl,
        router,
        hmac_secret_plain,
        dist_token_plain,
    })
}

/// home 节点自心跳后台任务——每 [`HOME_HEARTBEAT_INTERVAL`] 调
/// `ctrl.heartbeat("home", vec![])` 刷 last_seen。
///
/// home 是 fuxi-im daemon 同进程注册的"虚"节点，没有外部 worker loop 给它
/// 发心跳；不自心跳则 30s 后 nodes_snapshot 报 last_seen_ms_ago > 30000
/// → /api/nodes online=false → 前端节点 tab 把 home 显示成"离线"灰圆点。
/// inflight 永远空：home dispatch 默认走本地 spawn 不进 dist queue，控制器端
/// home.inflight 不会被 pull 改写，传 vec![] 是真相。
///
/// 任务持续到进程退出——同 spawn_sweep_task，无 graceful shutdown，进程 abort。
///
/// bug #77：之前传 `Vec::new()` 当 inflight，所以节点页 home 永远 0/4
/// （用户实测「这个有显示了，但是还是0/4」）。改：从 fuxi.list_workers() 取
/// 非 idle 的 worker（除玄女自身）作 inflight ids。语义上 home 上每个跑着的
/// 鲁班 / 玄女子任务都算占了一槽，跟 max_concurrency=4 形成真实并发上限信号。
pub fn spawn_home_heartbeat_task(
    ctrl: Arc<DistController>,
    fuxi: Option<Arc<fuxi_orchestrator::Fuxi>>,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HOME_HEARTBEAT_INTERVAL);
        // 跳过首次 tick——register 已在同一启动序列里完成，第一次心跳等
        // HOME_HEARTBEAT_INTERVAL 后再发，避免启动期紧贴的 register+heartbeat
        // 各发一份 WorkerHeartbeatStateChanged 制造无意义事件 noise。
        tick.tick().await;
        loop {
            tick.tick().await;
            // bug #77：fuxi=Some 时取本地 shelf 全部 worker，过滤掉玄女（编排层
            // 不算 worker 槽位）+ 仅 idle 不算（idle = 待命可派活，槽位空）。
            // 其它状态都视为占用 home 槽。fuxi=None 是测试 / 早启动路径退化。
            let inflight: Vec<String> = match fuxi.as_ref() {
                Some(f) => {
                    let xuannv = f.xuannv_id().await;
                    f.list_workers()
                        .await
                        .into_iter()
                        .filter(|c| Some(c.id) != xuannv)
                        .filter(|c| !matches!(c.status, fuxi_core::agent::AgentStatus::Idle))
                        .map(|c| c.id.0.to_string())
                        .collect()
                }
                None => Vec::new(),
            };
            ctrl.heartbeat(HOME_NODE_ID, inflight).await;
        }
    });
}

/// `build_dist_layer` 的产物——分块返便于 caller 各取所需：
/// - `ctrl` 给 daemon `with_dist` 用 + #55 `/api/nodes` 共享
/// - `router` 给 axum app `.merge()`
/// - `hmac_secret_plain` / `dist_token_plain` 给 #56 `/api/dist/setup-worker`
///   主密码鉴权后派给本地 worker 写 `~/.fuxi/dist-worker.env`
pub struct DistLayer {
    pub ctrl: Arc<DistController>,
    pub router: Router,
    pub hmac_secret_plain: String,
    pub dist_token_plain: String,
}

/// 解析 HMAC secret——env 优先；缺失则读 `~/.fuxi/dist_hmac.key`；都无则首启
/// 自生成 + 落盘（0600）。同 fuxi-im 主 auth 的 load-or-create 模式。
///
/// 返 `(HmacSecret 给 HMAC layer, 明文 String 给 #56 setup-worker 派发)`。
/// HmacSecret 包 SecretString 不允许 clone 出明文，所以这里在 secret 装包前
/// 多保留一份字符串副本——仅在 fuxi-im 启动期 in-memory 流转，不落第二份盘。
fn resolve_hmac_secret(home_dir: &std::path::Path) -> Result<(HmacSecret, String)> {
    if let Ok(plain) = std::env::var(FUXI_DIST_HMAC_SECRET_ENV)
        && !plain.is_empty()
    {
        tracing::info!("dist HMAC secret 来源：${FUXI_DIST_HMAC_SECRET_ENV}");
        return Ok((HmacSecret::new(plain.clone()), plain));
    }
    let key_path = home_dir.join("dist_hmac.key");
    if let Ok(content) = std::fs::read_to_string(&key_path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            tracing::info!(path = %key_path.display(), "dist HMAC secret 来源：本地文件");
            return Ok((HmacSecret::new(trimmed.clone()), trimmed));
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
    Ok((HmacSecret::new(generated.clone()), generated))
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

/// `Arc<DistController>` 包装为 fuxi-im 的 `NodesProvider` trait（β · #55）。
///
/// 这是 trait 反向依赖 pattern：fuxi-im 不能引 fuxi-cli（循环依赖），但需要查
/// dist controller 的 nodes_snapshot。trait 定义在 fuxi-im，impl 放在顶层
/// fuxi-cli 包 `Arc<DistController>`。
///
/// home 节点的 workers 列表从 `Fuxi.shelf` 拿（local 实例）；其他远端节点 v1
/// 始终空 `workers: []`——dist 协议当前只跟 job_id 不跟 agent_id，#57 dispatch
/// routing 加 worker→node 真映射后才能填。
pub struct DistControllerNodesProvider {
    pub ctrl: Arc<DistController>,
}

impl DistControllerNodesProvider {
    pub fn new(ctrl: Arc<DistController>) -> Self {
        Self { ctrl }
    }
}

#[async_trait]
impl NodesProvider for DistControllerNodesProvider {
    async fn list_nodes(&self, fuxi: &Fuxi) -> Vec<NodeView> {
        let snap = self.ctrl.nodes_snapshot().await;
        let mut out: Vec<NodeView> = Vec::with_capacity(snap.len());
        for s in snap {
            // online：heartbeat lag < 30s（spec gap c 严格阈值，比 dist STALE 60s 略严）
            let online = s
                .last_seen_ms_ago
                .map(|ms| ms < ONLINE_HEARTBEAT_THRESHOLD_MS)
                .unwrap_or(false);
            // workers：home 从 shelf 拿；远端 dist 节点从 events 历史反查
            // （`source_node_id == 节点` + 未终态的 cc agent，#74 实测修复用户
            // 反馈"mac 节点 workers:[] 跟现实不符"）。home_workers_from_shelf
            // 必须传 `online`，否则节点掉线时 idle worker 仍报 idle，dispatcher
            // claim_idle_by_role 会派活到死节点。
            let workers = if s.node_id == HOME_NODE_ID {
                home_workers_from_shelf(fuxi, online).await
            } else {
                fuxi_im::nodes_provider::dist_workers_from_events(fuxi.bus(), &s.node_id).await
            };
            out.push(NodeView {
                node_id: s.node_id,
                tags: s.tags,
                max_concurrency: s.max_concurrency,
                inflight_jobs: s.inflight_count,
                heartbeat_lag_ms: s.last_seen_ms_ago,
                online,
                registered_at_ms_ago: s.registered_at_ms_ago,
                workers,
            });
        }
        out
    }
}

/// β · #57 production `DistEnqueuer` 包装 `Arc<DistController>`——
/// `Fuxi::dispatch` 决策树命中 dist 路径时调本 impl 把 task 派到 dist controller。
///
/// 反向依赖 pattern：trait 在 fuxi-orchestrator，impl 放 fuxi-cli。
pub struct DistControllerEnqueuer {
    pub ctrl: Arc<DistController>,
}

impl DistControllerEnqueuer {
    pub fn new(ctrl: Arc<DistController>) -> Self {
        Self { ctrl }
    }
}

#[async_trait]
impl DistEnqueuer for DistControllerEnqueuer {
    async fn enqueue(
        &self,
        task: &Task,
        opts: fuxi_orchestrator::DistEnqueueOptions,
    ) -> OrchResult<String> {
        // node_id_hint：pinned_node 优先，否则空串（让 dist matcher 按 tags 派）
        let node_hint = task.pinned_node.clone().unwrap_or_default();
        // body：把 description 作为 prompt body 给 worker——worker 端跑 cc/codex
        // 时这是 user-turn 的输入。title 仅作展示。
        let body = if task.description.is_empty() {
            task.title.clone()
        } else {
            task.description.clone()
        };
        // cli wire 名 SoT = `"claude-code"`（CcAdapter::name()）；曾踩 `"cc"`
        // 简称 wire mismatch 隐 bug（#71）。allowed_tools 仍走 worker role-skill
        // 自补。task_id/role 是 #76/#77 修：worker 用 home 真相 task_id 让
        // events.meta.task 跨节点一致；publish AgentSpawning 让 home aggregate
        // 拿到 role 不 fallback "unknown"。
        let job_id = self
            .ctrl
            .enqueue(
                node_hint,
                task.title.clone(),
                body,
                opts.system_prompt,
                task.required_tags.clone(),
                task.pinned_node.clone(),
                "claude-code".to_string(),
                Vec::new(),
                opts.task_id,
                opts.role,
            )
            .await;
        tracing::info!(
            job_id = %job_id,
            task_id = %task.id,
            pinned_node = ?task.pinned_node,
            required_tags = ?task.required_tags,
            "dist enqueue 成功"
        );
        Ok(job_id)
    }
}

// 让 enqueuer 失败能上抛 anyhow——目前 dist::enqueue 不返 Result，永远 ok；
// 留 placeholder 万一未来 enqueue 改返 Result 时直接接进来。
#[allow(dead_code)]
fn map_dist_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Other(format!("dist enqueue 失败：{e}"))
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

    /// 真因回归 · #71：im_dist enqueue 写入的 cli 字段必须与 dist worker
    /// `select_adapter` 接受的 wire 名一致；否则 worker 收 job 后 factory_c
    /// 直接 Err，每个 job 走 `(false, "worker run error: ...")` fast-fail，
    /// dist_jobs.ok=0。错误只在 final report.output 里，schema 没存→看不见。
    /// 之前调试 v4 dist 接通时一直怀疑是 PATH/cc spawn，挖了一个月。
    #[tokio::test]
    async fn enqueuer_cli_field_must_match_worker_select_adapter_wire_name() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        let layer = build_dist_layer(dir.path(), bus).await.expect("build");
        layer
            .ctrl
            .register("mac".into(), vec!["local".into()], 1)
            .await;

        let enqueuer = DistControllerEnqueuer::new(layer.ctrl.clone());
        let task = fuxi_core::task::Task::new("ls", "ls ~/erp").with_pinned_node("mac");
        enqueuer
            .enqueue(&task, fuxi_orchestrator::DistEnqueueOptions::default())
            .await
            .expect("enqueue");

        let job = layer.ctrl.pull("mac").await.expect("应能 pull 到 job");

        // SoT：dist::select_adapter 只认 "codex"/""/"claude-code"。
        // 若改成别的字符串需同步更新 dist::select_adapter。
        assert_eq!(
            job.cli, "claude-code",
            "enqueuer cli={:?} 与 dist select_adapter 不匹配——worker 会 fast-fail ok=0",
            job.cli
        );
    }

    /// 真因回归 · home 节点自心跳：register 后无 worker loop 给 home 发心跳，
    /// 30s online 阈值后前端会把 home 显示为离线。spawn_home_heartbeat_task
    /// 必须把 last_seen 持续刷新——验证两轮心跳后 last_seen_ms_ago 远小于
    /// online 阈值 (30000ms)。
    ///
    /// bug #77：spawn 移出 build_dist_layer 由 caller (im.rs) 起，避免 fuxi 强耦合。
    /// 测试自己起，传 None 走退化空 inflight 路径。
    #[tokio::test]
    async fn home_self_heartbeat_keeps_home_online() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        let layer = build_dist_layer(dir.path(), bus).await.expect("build");
        // 模拟 im.rs caller 起心跳——传 None 是退化分支（旧测试兼容）
        spawn_home_heartbeat_task(layer.ctrl.clone(), None);
        // 等 2× HOME_HEARTBEAT_INTERVAL 让自心跳跑至少 1 次（首 tick 已 skip）。
        // 总共 ~10s 偏长，但拿真定时验真行为；CI 偶发慢机器加 2s 余量也仍 < 30s。
        tokio::time::sleep(HOME_HEARTBEAT_INTERVAL * 2 + std::time::Duration::from_secs(1)).await;
        let snap = layer.ctrl.nodes_snapshot().await;
        let home = snap
            .iter()
            .find(|n| n.node_id == HOME_NODE_ID)
            .expect("home 节点应在 snapshot");
        let last_seen = home
            .last_seen_ms_ago
            .expect("home self-heartbeat 必填 last_seen");
        assert!(
            last_seen < 30_000,
            "home last_seen_ms_ago={last_seen} 必须 < 30000，否则 NodesProvider 报 online=false"
        );
        // home 不接 dist job → inflight 必空（自心跳 vec![] 是真相）
        assert_eq!(home.inflight_count, 0);
    }

    /// dist enqueue → pull 后 controller 端 home.inflight 必 ++ → /api/nodes
    /// inflight_jobs=1，job 走完 report 后回 0。验证 PWA 节点卡的 "0/4" 数字
    /// 在真 dist 派活路径上的 lifecycle 是对的。
    ///
    /// 注：home 节点用户日常派活默认走本地 spawn（不带 pinned_node /
    /// required_tags），不贡献 home.inflight。要让 home 数字真动需 dispatch 带
    /// pinned_node="home"——本测显式 pull("home") 模拟那条路径。
    #[tokio::test]
    async fn home_inflight_increments_on_dist_pull_and_decrements_on_report() {
        use crate::dist::DistReportReq;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let dir = TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
            std::env::remove_var(DIST_TOKEN_ENV);
        }
        let layer = build_dist_layer(dir.path(), bus).await.expect("build");

        // 派一个 pinned home 的 job
        let enqueuer = DistControllerEnqueuer::new(layer.ctrl.clone());
        let task = fuxi_core::task::Task::new("ls", "ls -lh").with_pinned_node(HOME_NODE_ID);
        enqueuer
            .enqueue(&task, fuxi_orchestrator::DistEnqueueOptions::default())
            .await
            .expect("enqueue");

        // 初始状态 home.inflight 必为 0
        let snap0 = layer.ctrl.nodes_snapshot().await;
        let home0 = snap0
            .iter()
            .find(|n| n.node_id == HOME_NODE_ID)
            .expect("home");
        assert_eq!(home0.inflight_count, 0, "pull 前 home.inflight 应为 0");

        // 模拟 home worker pull 把 job 收走 → controller 端 home.inflight ++
        let job = layer
            .ctrl
            .pull(HOME_NODE_ID)
            .await
            .expect("应能 pull 到 job");
        let snap1 = layer.ctrl.nodes_snapshot().await;
        let home1 = snap1
            .iter()
            .find(|n| n.node_id == HOME_NODE_ID)
            .expect("home");
        assert_eq!(
            home1.inflight_count, 1,
            "pull 后 home.inflight 应为 1，PWA 卡数字 1/4"
        );

        // 模拟 worker 上报 done → controller 释放 inflight
        layer
            .ctrl
            .report(DistReportReq {
                job_id: job.id.clone(),
                node_id: HOME_NODE_ID.to_string(),
                ok: true,
                output: String::new(),
                duration_ms: 100,
            })
            .await;
        let snap2 = layer.ctrl.nodes_snapshot().await;
        let home2 = snap2
            .iter()
            .find(|n| n.node_id == HOME_NODE_ID)
            .expect("home");
        assert_eq!(
            home2.inflight_count, 0,
            "report 后 home.inflight 应回 0，PWA 卡数字 0/4"
        );
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
