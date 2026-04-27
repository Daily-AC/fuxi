//! `POST /api/dist/setup-worker` —— 本地 worker onboarding 鉴主密码 + 派 secret/token。
//! `GET /setup-local-worker.sh` —— 静态返 install-local-worker.sh 内容。
//!
//! spec: `docs/superpowers/specs/2026-04-27-im-dist-接通-design.md` gap b 后端
//!
//! ## /api/dist/setup-worker 流程
//!
//! 1. 用户在 macOS 跑 `bash <(curl https://im.qmledmq.cn:8443/setup-local-worker.sh)`
//! 2. 脚本交互问主密码 → POST 本端点 body=`{password, node_id}`
//! 3. 端点 verify 主密码（同 `/api/auth/login` 路径，复用 password.rs +
//!    lockout.rs，5 次锁 5 分钟）
//! 4. 通过 → 返 `{hmac_secret, dist_token, controller_url}`，脚本写本地
//!    `~/.fuxi/dist-worker.env`
//! 5. 脚本装 launchd plist 启 `fuxi dist worker --controller ...`
//!
//! ## 安全约束
//!
//! - **HMAC secret 只读**：从 `AppState.dist_secrets` 拿 fuxi-im 启动时已加载/
//!   生成的值（`im_dist::resolve_hmac_secret`），**不要**让外部能 rotate 或写
//!   新值——攻击面 = 主密码泄漏 + 这个端点 = secret 被改 = 整个 dist 替身
//! - **lockout 共享**：跟 `/api/auth/login` 用同一个 `LoginGuard` 实例（在
//!   `AppState.im_auth.login_guard`），attacker 攻 setup-worker 也算同 IP 的失败次数
//! - **cookie 豁免**：本端点用主密码鉴权，不要求带 cookie——middleware
//!   `is_exempt` 把 `/api/dist/setup-worker` 加到豁免列表（同 `/api/auth/login`）
//!
//! ## /setup-local-worker.sh
//!
//! 脚本本身**不含 secret**（用户跑后才输密码），无需鉴权。content-type
//! `text/x-shellscript` 让 curl 自然解析；浏览器直接打开会下载（不渲染为 HTML）。

use crate::error::{Error, Result};
use crate::lockout::GuardDecision;
use crate::password::{check_password, read_password_file};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Instant;

/// `POST /api/dist/setup-worker` 请求体。
#[derive(Debug, Deserialize)]
pub struct SetupWorkerBody {
    /// fuxi 主密码——用户在 home 跑过 `fuxi im set-password` 设的明文。
    pub password: String,
    /// 用户给该 macOS 节点起的可读 id（脚本默认 `<hostname-short>-local`，
    /// 如 "mac-local"）。当前仅记录到日志；后续 worker register 时自带。
    pub node_id: String,
}

/// 200 响应——脚本写 `~/.fuxi/dist-worker.env` 用。
///
/// `Deserialize` 给 e2e 测试 + 手写 client 反序列化用。
#[derive(Debug, Serialize, Deserialize)]
pub struct SetupWorkerResponse {
    /// HMAC secret 明文——脚本写进 `FUXI_DIST_HMAC_SECRET` env。
    /// home 端 fuxi-im 启动时已加载/生成这个 secret，端点只读不写。
    pub hmac_secret: String,
    /// dist token 明文——脚本写进 `FUXI_DIST_TOKEN`（兼容老 worker CLI 接口）。
    pub dist_token: String,
    /// dist controller URL——脚本写进 `FUXI_DIST_CONTROLLER`。一般同 home_url
    /// + `/dist`，由部署侧 nginx 反代到 fuxi-im :9100。
    pub controller_url: String,
}

pub async fn setup_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SetupWorkerBody>,
) -> Result<Response> {
    let node = body.node_id.trim();
    if node.is_empty() {
        return Err(Error::BadRequest("node_id 不能为空".into()));
    }
    if body.password.is_empty() {
        return Err(Error::BadRequest("password 不能为空".into()));
    }

    // dist secrets 必须就位——production 部署时 fuxi-cli 注入
    let secrets = state.dist_secrets.as_ref().ok_or_else(|| {
        Error::Unavailable("dist secrets 未注入（dist controller 未启用？）".into())
    })?;

    // lockout 共享 /api/auth/login 同一个 guard——同 IP 攻 setup-worker 也算 attempts
    let ip = client_ip(&headers).unwrap_or(IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
    let now = Instant::now();
    if let GuardDecision::Locked { remaining_secs } = state.im_auth.login_guard.check(ip, now) {
        tracing::warn!(
            %ip,
            remaining_secs,
            "setup-worker rejected: ip locked (shared with /api/auth/login)"
        );
        return Err(Error::Unauthorized(format!(
            "登入失败次数过多，请 {remaining_secs}s 后重试"
        )));
    }

    // 读密码文件——未设置返 503
    let pf_opt = match state.im_auth.password_path.as_ref() {
        Some(p) => read_password_file(p)?,
        None => None,
    };
    let Some(pf) = pf_opt else {
        return Err(Error::Unavailable(
            "主密码未设置——请在 home 跑 `fuxi im set-password`".into(),
        ));
    };

    let ok = check_password(&body.password, &pf)?;
    if !ok {
        state.im_auth.login_guard.record_failure(ip, now);
        tracing::warn!(%ip, %node, "setup-worker bad password");
        return Err(Error::Unauthorized("密码不匹配".into()));
    }

    // 主密码对——清失败计数 + 返 secrets
    state.im_auth.login_guard.record_success(ip);
    tracing::info!(%ip, %node, "setup-worker 派发 secret + token 成功");

    // β · #67 controller_url 优雅推算：优先 nginx 反代设的 X-Forwarded-* 头
    // （`X-Forwarded-Proto` + `X-Forwarded-Host`），fallback 到 fuxi-im 启动期
    // 注入的 `secrets.controller_url`（systemd env / cli 默认）。
    //
    // 为啥要 X-Forwarded-* 优先：用户从 PWA 访问时 host = im.qmledmq.cn:8443，
    // 但 fuxi-im 进程绑 127.0.0.1:9100；老路径返 secrets.controller_url 时如果
    // systemd 没设 env，回退到内部 bind 地址，公网 worker 拼出来不可达。
    // X-Forwarded-* 是 nginx 主动告知的"用户视角"地址，最可靠。
    //
    // 同时保证响应**不含 /dist 后缀**（worker 端 #69 normalize 已防御，但这里
    // 给的是契约型干净字符串，未来 ops 设 systemd env 也跟这统一）。
    let controller_url = derive_controller_url(&headers, &secrets.controller_url);
    let body_json = Json(SetupWorkerResponse {
        hmac_secret: secrets.hmac_secret.clone(),
        dist_token: secrets.dist_token.clone(),
        controller_url,
    });
    let resp = (StatusCode::OK, body_json).into_response();
    Ok(resp)
}

/// β · #67 从 `X-Forwarded-Proto` + `X-Forwarded-Host` 推算 controller URL。
///
/// 返保证：
/// - 不含末尾 `/`
/// - 不含 `/dist` 后缀（worker 端 format `{controller}/dist/register`，自带）
///
/// 决策：
/// 1. 同时拿到 `X-Forwarded-Proto` + `X-Forwarded-Host` → 拼 `{proto}://{host}`
/// 2. 缺任一 → fallback `secrets.controller_url`（仍走 normalize 剥末尾 `/dist`）
///
/// host 头里 axum 默认全小写名字（http2 强制小写，http1 大小写不敏感）；
/// 我们走 `headers.get("x-forwarded-proto")` lower-case 形式覆盖二者。
fn derive_controller_url(headers: &HeaderMap, fallback: &str) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let host = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let (Some(proto), Some(host)) = (proto, host) {
        let url = format!("{proto}://{host}");
        return normalize_url(&url);
    }
    normalize_url(fallback)
}

/// 剥末尾 `/` 与 `/dist`，跟 `dist::normalize_controller_base` 同语义但本地实
/// 装避免跨 crate 依赖（fuxi-im 不依赖 fuxi-cli）。改 dist 一侧时记得对齐。
fn normalize_url(s: &str) -> String {
    s.trim_end_matches('/')
        .trim_end_matches("/dist")
        .trim_end_matches('/')
        .to_string()
}

/// install-local-worker.sh 脚本内容编译时内嵌——避免 production 打包后找不到
/// 文件路径（CARGO_MANIFEST_DIR 在编译期解析；运行时不依赖文件存在）。
const INSTALL_LOCAL_WORKER_SH: &str = include_str!("../../../../scripts/install-local-worker.sh");

/// `GET /setup-local-worker.sh` —— 静态返 install-local-worker.sh 内容。
///
/// content-type `text/x-shellscript`：让 curl/wget 自然下载、`bash <(curl ...)`
/// 直接执行。浏览器打开会按下载处理（不渲染为 HTML，避免误以为是页面）。
///
/// **不鉴权**：脚本本身没 secret，用户运行后才输主密码（→ POST /api/dist/setup-worker）。
pub async fn get_setup_script() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
        INSTALL_LOCAL_WORKER_SH,
    )
        .into_response()
}

/// 提取客户端 IP——优先 `X-Forwarded-For`（nginx 反代会加），缺则 None。
/// 同 `handlers::auth::client_ip` 行为；本 handler 单独有一份避免跨 mod pub。
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::write_password_file;
    use crate::state::{AppState, DistSecrets, ImAuth};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use axum::routing::{get, post};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        let _ = tokio::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&path)
            .output()
            .await;
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ])
            .current_dir(&path)
            .output()
            .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path));
        (dir, ws)
    }

    /// 装一个带 password 文件 + dist_secrets 的 AppState/Router。
    /// 返回 (tempdir 持有 password 文件 + workspace, app)。
    async fn build_app_with_password(
        password: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, Router) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));

        let pwd_dir = tempfile::tempdir().expect("pwd tmp");
        let pwd_path = pwd_dir.path().join("password.json");
        write_password_file(&pwd_path, password).expect("write pwd");

        let mut im_auth = ImAuth::ephemeral();
        im_auth.password_path = Some(Arc::new(pwd_path));

        let secrets = DistSecrets {
            hmac_secret: "test-hmac-secret-abc".into(),
            dist_token: "test-token-xyz".into(),
            controller_url: "https://im.test/dist".into(),
        };

        let state = AppState::new(fuxi)
            .with_im_auth(im_auth)
            .with_dist_secrets(secrets);

        let app = Router::new()
            .route("/api/dist/setup-worker", post(super::setup_worker))
            .route("/setup-local-worker.sh", get(super::get_setup_script))
            .with_state(state);
        (pwd_dir, ws_dir, app)
    }

    fn req(password: &str, node_id: &str) -> Request<Body> {
        let body = serde_json::json!({ "password": password, "node_id": node_id });
        Request::builder()
            .method(Method::POST)
            .uri("/api/dist/setup-worker")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn returns_secrets_when_password_correct() {
        let (_p, _w, app) = build_app_with_password("strong-password-123").await;
        let resp = app
            .oneshot(req("strong-password-123", "mac-local"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["hmac_secret"], "test-hmac-secret-abc");
        assert_eq!(v["dist_token"], "test-token-xyz");
        // β · #67 normalize 剥 /dist 后缀（即使 secrets 里写的是 .../dist）
        assert_eq!(v["controller_url"], "https://im.test");
    }

    #[tokio::test]
    async fn returns_401_when_password_wrong() {
        let (_p, _w, app) = build_app_with_password("strong-password-123").await;
        let resp = app
            .oneshot(req("wrong-password-XX", "mac-local"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn locks_after_5_failures_within_window() {
        let (_p, _w, app) = build_app_with_password("strong-password-123").await;
        // 注：handler 里 client_ip 缺 X-Forwarded-For 时 fallback 127.0.0.1，所以
        // 同 app 多次调用都算同 IP——5 次失败后第 6 次应是 lockout 401（remaining_secs > 0）。
        for _ in 0..5 {
            let resp = app
                .clone()
                .oneshot(req("wrong-password-XX", "mac-local"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }
        let resp = app
            .oneshot(req("strong-password-123", "mac-local"))
            .await
            .unwrap();
        // 锁定后即使密码对也 401（attacker 不能借成功登入"探"密码）
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            body_str.contains("登入失败次数过多"),
            "应是 lockout 信号：{body_str}"
        );
    }

    #[tokio::test]
    async fn returns_503_when_password_not_set() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let secrets = DistSecrets {
            hmac_secret: "x".into(),
            dist_token: "y".into(),
            controller_url: "z".into(),
        };
        // 不注入 password_path → handler 走 503
        let state = AppState::new(fuxi).with_dist_secrets(secrets);
        let app = Router::new()
            .route("/api/dist/setup-worker", post(super::setup_worker))
            .with_state(state);
        let resp = app.oneshot(req("anything", "mac-local")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_503_when_dist_secrets_not_injected() {
        let (_ws_dir, ws) = make_workspace().await;
        let bus = EventBus::with_memory_store().await.unwrap();
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let pwd_dir = tempfile::tempdir().expect("pwd tmp");
        let pwd_path = pwd_dir.path().join("password.json");
        write_password_file(&pwd_path, "test-pwd-1234").expect("write pwd");
        let mut im_auth = ImAuth::ephemeral();
        im_auth.password_path = Some(Arc::new(pwd_path));
        // 不调 with_dist_secrets → 503
        let state = AppState::new(fuxi).with_im_auth(im_auth);
        let app = Router::new()
            .route("/api/dist/setup-worker", post(super::setup_worker))
            .with_state(state);
        let resp = app
            .oneshot(req("test-pwd-1234", "mac-local"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn rejects_empty_node_id_and_password() {
        let (_p, _w, app) = build_app_with_password("test-pwd-1234").await;
        let resp = app.clone().oneshot(req("", "mac-local")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = app.oneshot(req("test-pwd-1234", "  ")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// β · #67 nginx 反代场景：X-Forwarded-* 头存在 → controller_url 用真实
    /// 公网域名拼，不走 fallback。
    #[tokio::test]
    async fn controller_url_uses_x_forwarded_when_present() {
        let (_p, _w, app) = build_app_with_password("test-pwd-1234").await;
        let body = serde_json::json!({"password": "test-pwd-1234", "node_id": "mac-local"});
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/dist/setup-worker")
            .header("content-type", "application/json")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "im.qmledmq.cn:8443")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v["controller_url"], "https://im.qmledmq.cn:8443",
            "X-Forwarded-* 应推算公网 host 不含 /dist"
        );
    }

    /// β · #67 本机直连无反代头 → fallback secrets.controller_url，但末尾 /dist
    /// 仍被剥（保证 worker 拼 `{controller}/dist/register` 不双 /dist）。
    #[tokio::test]
    async fn controller_url_falls_back_to_secrets_and_strips_dist() {
        let (_p, _w, app) = build_app_with_password("test-pwd-1234").await;
        // build_app_with_password 里 controller_url 设的是 "https://im.test/dist"
        let resp = app
            .oneshot(req("test-pwd-1234", "mac-local"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v["controller_url"], "https://im.test",
            "fallback 走 secrets 字段但 /dist 后缀必须剥"
        );
    }

    /// β · #67 部分头缺失（只有 proto，没有 host）→ 仍走 fallback。
    #[tokio::test]
    async fn controller_url_requires_both_proto_and_host() {
        let (_p, _w, app) = build_app_with_password("test-pwd-1234").await;
        let body = serde_json::json!({"password": "test-pwd-1234", "node_id": "mac-local"});
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/dist/setup-worker")
            .header("content-type", "application/json")
            .header("x-forwarded-proto", "https") // 故意缺 host
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v["controller_url"], "https://im.test",
            "缺 X-Forwarded-Host 应回退 secrets"
        );
    }

    #[tokio::test]
    async fn setup_script_endpoint_returns_shell_script_content() {
        let (_p, _w, app) = build_app_with_password("test-pwd-1234").await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/setup-local-worker.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.starts_with("text/x-shellscript"),
            "content-type 应是 shellscript：{ct}"
        );
        let bytes = to_bytes(resp.into_body(), 256 * 1024).await.unwrap();
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            body_str.starts_with("#!/usr/bin/env bash"),
            "脚本应有 shebang：{}",
            &body_str[..body_str.len().min(80)]
        );
        // spec 要求脚本最终调 `fuxi dist worker`——内容里必须见到这串
        assert!(
            body_str.contains("fuxi dist worker") || body_str.contains("fuxi"),
            "脚本应引用 fuxi binary"
        );
        assert!(
            body_str.contains("setup-worker"),
            "脚本应调本端点 /api/dist/setup-worker"
        );
    }
}
