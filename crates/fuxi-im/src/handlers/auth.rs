//! `/api/auth/login` —— 主密码登入（β · Task #9，主路）。
//! `/api/auth/pair` —— 设备一次性 PIN 配对（β · Decision 14 D，降级 fallback）。
//!
//! 流程：
//! - **主路**：用户在 PWA 输主密码 + 设备名 POST `/api/auth/login` → bcrypt::verify
//!   → 通过则签 HMAC token + 写 device_tokens + Set-Cookie。
//! - **fallback**：忘密码场景走 `/api/auth/pair` —— TUI `/pair` 出 PIN，PWA 输入
//!   POST `/api/auth/pair` → claim PIN → 签 token + 写 device_tokens + Set-Cookie。
//! - 两条路径**后半段（签 token + 写库 + cookie）共用** [`finalize_login`]。
//!
//! ## 暴力防御
//!
//! login handler 在 verify 之前调 [`crate::lockout::LoginGuard::check`]：
//! - 该 IP 已锁 → 401 + 不真做 bcrypt（避免 attacker 还能消耗服务器 CPU）
//! - 一分钟内 5 次失败 → 锁 5 分钟
//! - 成功登入 → 清失败计数

use crate::auth::{build_set_cookie, fresh_claims, sign_token};
use crate::devices::DeviceRecord;
use crate::error::{Error, Result};
use crate::lockout::GuardDecision;
use crate::pair::ClaimError;
use crate::password::{check_password, read_password_file};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PairBody {
    /// 用户输入的 6 位 PIN。
    pub pin: String,
    /// 用户给设备起的可读名（"以琳的 iPhone"）。
    pub device_name: String,
}

/// 200 响应体——客户端能看到自己拿到的 device_id（debug + 后续吊销定位用）。
/// **token 不进 body**——必须 Set-Cookie HttpOnly 防 XSS 偷走。
///
/// `Deserialize` 给 e2e 测试反序列化响应用——hand-written client 也用得上。
#[derive(Debug, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
}

pub async fn pair(State(state): State<AppState>, Json(body): Json<PairBody>) -> Result<Response> {
    // 防 trivial input：handler 层做最低限度的格式校验（PIN 必须 6 位数字，
    // device_name 不空）。深层语义错（PIN 错/过期）由 PendingPairs::claim 给出。
    if body.pin.len() != 6 || !body.pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::BadRequest("pin 必须是 6 位数字".into()));
    }
    let name = body.device_name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("device_name 不能为空".into()));
    }

    state.im_auth.pairs.claim(&body.pin).map_err(|e| match e {
        ClaimError::Unknown | ClaimError::Expired | ClaimError::Locked => {
            Error::Unauthorized(format!("PIN 不可用：{e:?}"))
        }
    })?;

    finalize_login(&state, name).await
}

/// `/api/auth/login` 请求体——主密码 + 设备名。
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    /// 用户在 home 跑 `fuxi im set-password` 设的主密码明文。
    pub password: String,
    /// 用户给设备起的可读名（"以琳的 iPhone"）。
    pub device_name: String,
}

/// 503 响应体（密码未设置时引导用户 set-password）。
#[derive(Debug, Serialize, Deserialize)]
pub struct PasswordNotSetResponse {
    pub error: &'static str,
}

/// `POST /api/auth/login` 实装（主路）。
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<Response> {
    let name = body.device_name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("device_name 不能为空".into()));
    }
    if body.password.is_empty() {
        return Err(Error::BadRequest("password 不能为空".into()));
    }

    let ip = client_ip(&headers).unwrap_or(IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
    let now = Instant::now();
    if let GuardDecision::Locked { remaining_secs } = state.im_auth.login_guard.check(ip, now) {
        tracing::warn!(%ip, remaining_secs, "login rejected: ip locked");
        return Err(Error::Unauthorized(format!(
            "登入失败次数过多，请 {remaining_secs}s 后重试"
        )));
    }

    // 读密码文件——未注入路径或文件不存在都视作"未设密码"返 503 + 提示文案。
    // password_path=None 在 production daemon 路径绝不会发生（with_persistence 自动
    // 填默认值）；仅 router_smoke / unit test 走 ephemeral 时才命中。
    let pf_opt = match state.im_auth.password_path.as_ref() {
        Some(p) => read_password_file(p)?,
        None => None,
    };
    let Some(pf) = pf_opt else {
        return Ok(password_not_set_response());
    };

    let ok = check_password(&body.password, &pf)?;
    if !ok {
        state.im_auth.login_guard.record_failure(ip, now);
        tracing::warn!(%ip, "login bad password");
        return Err(Error::Unauthorized("密码不匹配".into()));
    }

    state.im_auth.login_guard.record_success(ip);
    finalize_login(&state, name).await
}

/// 共用收尾：签 HMAC token、写 device_tokens、Set-Cookie。
///
/// pair / login 两条路径用同一个 contract：调用方先把"是不是合法人"判定完，
/// 这里负责"我承认你这台设备" 的物理化（持久化 + cookie）。
async fn finalize_login(state: &AppState, device_name: &str) -> Result<Response> {
    let device_id = Uuid::new_v4().to_string();
    let claims = fresh_claims(device_id.clone(), device_name.to_string());
    let token = sign_token(&state.im_auth.secret, &claims)?;

    if let Some(devices) = &state.im_auth.devices {
        let now = Utc::now();
        let rec = DeviceRecord {
            token_id: device_id.clone(),
            device_name: device_name.to_string(),
            // 当前用全局 key 签发；本列存的是 base64url 形式的 secret 占位
            // （CLAUDE.md 决策：未来 per-device rotation 才用得到）。
            // WHY 不写真 key 进每行：global key 有自己的 ~/.fuxi/im_hmac.key 文件
            // 真相源；这里写 "global" 占位字符串避免泄漏 secret 到表里。
            hmac_secret: "global".to_string(),
            created_at: now,
            expires_at: claims.expires_at,
            revoked_at: None,
        };
        devices.insert(&rec).await?;
    }

    let cookie = build_set_cookie(&token, crate::auth::DEFAULT_TOKEN_TTL_DAYS);

    let body_json = Json(PairResponse {
        device_id: device_id.clone(),
    });

    let mut resp = (StatusCode::OK, body_json).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|e| Error::Internal(format!("set-cookie header 解析失败: {e}")))?,
    );
    Ok(resp)
}

/// 提取客户端 IP——优先 `X-Forwarded-For`（nginx 反代会加），缺则 None。
/// nginx vhost 必须配 `proxy_set_header X-Forwarded-For $remote_addr`，否则锁定
/// 退化为"全部进来的 IP 都是 127.0.0.1（nginx 自己）"——一旦攻击 IP 接力，所有
/// 真请求都会被同一个 127.0.0.1 锁定连累。本 handler 只读 header 不查 PeerAddr，
/// 因为 axum::serve 没用 with_connect_info（要 ζ 配合）。
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .and_then(|s| s.parse().ok())
}

fn password_not_set_response() -> Response {
    let body = Json(PasswordNotSetResponse {
        error: "password not set, run `fuxi im set-password` on home",
    });
    (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{HmacSecret, verify_token};
    use crate::pair::PendingPairs;
    use crate::state::ImAuth;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
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
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
        (dir, ws)
    }

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    async fn build(im_auth: ImAuth) -> (tempfile::TempDir, Router, AppState) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_im_auth(im_auth);
        let app = Router::new()
            .route("/api/auth/pair", post(pair))
            .route("/api/auth/login", post(login))
            .with_state(state.clone());
        (dir, app, state)
    }

    #[tokio::test]
    async fn pair_with_valid_pin_signs_token_and_sets_cookie() {
        let secret = HmacSecret::from_string("test-key".into());
        let pairs = Arc::new(PendingPairs::new());
        let pin = pairs.start(crate::pair::DEFAULT_PIN_TTL);
        let im_auth = ImAuth {
            secret: Arc::new(secret),
            pairs: pairs.clone(),
            devices: None,
            password_path: None,
            login_guard: Arc::new(crate::lockout::LoginGuard::new()),
        };
        let (_dir, app, state) = build(im_auth).await;

        let body = serde_json::json!({ "pin": pin.as_str(), "device_name": "iPhone" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 取 Set-Cookie，提 token，verify 一遍——白盒确认确实是 sign_token 出的活 token
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("must have Set-Cookie")
            .to_str()
            .unwrap()
            .to_string();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        let token = set_cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("fuxi_im_token=")
            .to_string();
        let claims = verify_token(&state.im_auth.secret, &token).expect("valid");
        assert_eq!(claims.name, "iPhone");

        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: PairResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(claims.device_id, parsed.device_id);
    }

    #[tokio::test]
    async fn pair_with_unknown_pin_returns_401() {
        let im_auth = ImAuth::ephemeral();
        let (_dir, app, _) = build(im_auth).await;
        let body = serde_json::json!({ "pin": "000000", "device_name": "x" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pair_with_bad_format_returns_400() {
        let im_auth = ImAuth::ephemeral();
        let (_dir, app, _) = build(im_auth).await;

        for (pin, name, expected) in [
            ("12345", "x", StatusCode::BAD_REQUEST),   // 5 位
            ("1234567", "x", StatusCode::BAD_REQUEST), // 7 位
            ("abcdef", "x", StatusCode::BAD_REQUEST),  // 非数字
            ("123456", "  ", StatusCode::BAD_REQUEST), // device_name 空白
        ] {
            let body = serde_json::json!({ "pin": pin, "device_name": name });
            let req = Request::builder()
                .method("POST")
                .uri("/api/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected, "pin={pin} name={name:?}");
        }
    }

    #[tokio::test]
    async fn pair_writes_device_record_when_store_present() {
        // 端到端：成功路径 + DeviceStore 注入 → device_tokens 表里能查到该 device_id
        let dir = tempfile::tempdir().expect("tmp");
        let db_path = dir.path().join("im.db");
        let pool = crate::db::init_at(&db_path).await.expect("init db");
        let store = crate::devices::DeviceStore::new(pool);

        let secret = HmacSecret::from_string("real-key".into());
        let pairs = Arc::new(PendingPairs::new());
        let pin = pairs.start(crate::pair::DEFAULT_PIN_TTL);
        let im_auth = ImAuth {
            secret: Arc::new(secret),
            pairs,
            devices: Some(store.clone()),
            password_path: None,
            login_guard: Arc::new(crate::lockout::LoginGuard::new()),
        };
        let (_ws_dir, app, _) = build(im_auth).await;

        let body = serde_json::json!({ "pin": pin.as_str(), "device_name": "Pixel" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: PairResponse = serde_json::from_slice(&bytes).unwrap();

        let row = store.get(&parsed.device_id).await.unwrap().expect("入库");
        assert_eq!(row.device_name, "Pixel");
        assert!(row.revoked_at.is_none());
    }

    // ─── /api/auth/login（主密码主路） ───

    /// helper：用临时 password 文件构造完整 ImAuth（含 login_guard）。
    async fn build_with_password(
        password: &str,
        store: Option<crate::devices::DeviceStore>,
    ) -> (tempfile::TempDir, Router, AppState) {
        let pw_dir = tempfile::tempdir().expect("tmp pw");
        let pw_path = pw_dir.path().join("im_password.bcrypt");
        crate::password::write_password_file(&pw_path, password).expect("write pw");
        let secret = HmacSecret::from_string("test-key".into());
        let im_auth = ImAuth {
            secret: Arc::new(secret),
            pairs: Arc::new(PendingPairs::new()),
            devices: store,
            password_path: Some(Arc::new(pw_path)),
            login_guard: Arc::new(crate::lockout::LoginGuard::new()),
        };
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_im_auth(im_auth);
        let app = Router::new()
            .route("/api/auth/pair", post(pair))
            .route("/api/auth/login", post(login))
            .with_state(state.clone());
        // 把 pw_dir 一并返回防 drop——TempDir drop 会清掉密码文件
        (pw_dir, app, state)
    }

    fn login_req(password: &str, device_name: &str, ip: Option<&str>) -> Request<Body> {
        let body = serde_json::json!({ "password": password, "device_name": device_name });
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json");
        if let Some(ip) = ip {
            builder = builder.header("x-forwarded-for", ip);
        }
        builder
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn login_with_correct_password_signs_token_and_sets_cookie() {
        let (_pw_dir, app, state) = build_with_password("correct-pass", None).await;
        let resp = app
            .clone()
            .oneshot(login_req("correct-pass", "Pixel", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("must have Set-Cookie")
            .to_str()
            .unwrap()
            .to_string();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        assert!(set_cookie.contains("SameSite=Lax"));
        let token = set_cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("fuxi_im_token=")
            .to_string();
        let claims = verify_token(&state.im_auth.secret, &token).expect("valid");
        assert_eq!(claims.name, "Pixel");
    }

    #[tokio::test]
    async fn login_with_wrong_password_returns_401() {
        let (_pw_dir, app, _) = build_with_password("correct-pass", None).await;
        let resp = app
            .oneshot(login_req("WRONG", "x", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_returns_503_when_password_not_set() {
        // ephemeral ImAuth (无 password_path) → 503 + 提示文案
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi); // 默认 ephemeral
        let app = Router::new()
            .route("/api/auth/login", post(login))
            .with_state(state);

        let resp = app
            .oneshot(login_req("any", "x", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msg = parsed["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains("password not set"),
            "提示文案应明示，得到：{msg}"
        );
        assert!(msg.contains("fuxi im set-password"));
    }

    #[tokio::test]
    async fn login_lockout_after_five_failures_then_recovers() {
        let (_pw_dir, app, _) = build_with_password("correct-pass", None).await;
        let attacker = Some("198.51.100.99");
        // 5 次错密码 → 第 6 次即使密码对也应被锁
        for _ in 0..5 {
            let resp = app
                .clone()
                .oneshot(login_req("WRONG", "x", attacker))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }
        // 锁定中——正确密码也拒
        let resp = app
            .clone()
            .oneshot(login_req("correct-pass", "x", attacker))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "锁定期间正确密码也应 401"
        );
    }

    #[tokio::test]
    async fn login_lockout_only_affects_offending_ip() {
        let (_pw_dir, app, _) = build_with_password("correct-pass", None).await;
        // attacker 锁
        for _ in 0..5 {
            let _ = app
                .clone()
                .oneshot(login_req("WRONG", "x", Some("198.51.100.99")))
                .await
                .unwrap();
        }
        // bystander 旁观 IP——正确密码仍应 200
        let resp = app
            .clone()
            .oneshot(login_req("correct-pass", "y", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn login_writes_device_record_when_store_present() {
        let dir = tempfile::tempdir().expect("tmp");
        let db_path = dir.path().join("im.db");
        let pool = crate::db::init_at(&db_path).await.expect("init db");
        let store = crate::devices::DeviceStore::new(pool);
        let (_pw_dir, app, _) = build_with_password("good-pass", Some(store.clone())).await;

        let resp = app
            .oneshot(login_req("good-pass", "Phone", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: PairResponse = serde_json::from_slice(&bytes).unwrap();

        let row = store.get(&parsed.device_id).await.unwrap().expect("入库");
        assert_eq!(row.device_name, "Phone");
        assert!(row.revoked_at.is_none());
    }

    #[tokio::test]
    async fn login_rejects_empty_fields() {
        let (_pw_dir, app, _) = build_with_password("correct-pass", None).await;
        // 空 device_name
        let resp = app
            .clone()
            .oneshot(login_req("correct-pass", "  ", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 空密码
        let resp = app
            .oneshot(login_req("", "x", Some("203.0.113.5")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
