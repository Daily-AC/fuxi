//! FCM（Firebase Cloud Messaging）原生 Android 推送通道。
//!
//! 与 [`super::notify`]（Web Push / VAPID）**完全平行**——同一套 hooks 触发
//! 时两路一起 fan-out。本模块四块组合：
//! - **store** —— `fcm_tokens` 表 CRUD（[`upsert_token`] / [`list_tokens`] /
//!   [`delete_token`]）
//! - **凭据 + OAuth2** —— [`FcmCredentials`] 从 service account JSON 加载，
//!   用 RSA 私钥签 RS256 JWT 换 access_token，带过期缓存
//! - **[`FcmPusher`] trait** —— 单条发送原语，mirror [`super::notify::PushSender`]
//! - **[`notify_fcm`]** —— fan-out 给所有 token，stale token 顺手清理
//!
//! WHY 复用 [`PushPayload`]：FCM notification 的 title/body 和 data.url 跟
//! Web Push payload 语义完全一致，新定义类型只会让 hooks 两路漂移。

use crate::error::{Error, Result};
use crate::push::notify::PushPayload;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ─── store ────────────────────────────────────────────────────────────

/// upsert 一条 FCM token——token 是主键，重复 register = 更新 device_name +
/// created_at（同 Web Push 的 endpoint upsert：客户端重装 / 重新拿 token 时
/// 不该堆僵尸行）。
pub async fn upsert_token(pool: &SqlitePool, token: &str, device_name: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO fcm_tokens (token, device_name, created_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(token) DO UPDATE SET
            device_name = excluded.device_name,
            created_at  = excluded.created_at
        "#,
    )
    .bind(token)
    .bind(device_name)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| Error::Internal(format!("upsert fcm_token: {e}")))?;
    Ok(())
}

/// 列出当前所有 FCM token——`notify_fcm` fan-out 遍历用。
///
/// 没有 silence_until 概念：Android 通知静音由系统通道管，后端不掺和。
pub async fn list_tokens(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT token FROM fcm_tokens ORDER BY created_at ASC")
            .fetch_all(pool)
            .await
            .map_err(|e| Error::Internal(format!("list_tokens: {e}")))?;
    Ok(rows.into_iter().map(|(t,)| t).collect())
}

/// 删除一条 token——FCM 返 UNREGISTERED（token 失效）时 `notify_fcm` 调它清理。
pub async fn delete_token(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM fcm_tokens WHERE token = ?1")
        .bind(token)
        .execute(pool)
        .await
        .map_err(|e| Error::Internal(format!("delete_token: {e}")))?;
    Ok(())
}

// ─── 凭据 + OAuth2 ────────────────────────────────────────────────────

/// FCM service account JSON env 覆盖键——缺省 `~/.fuxi/fcm_service_account.json`。
pub const FCM_SERVICE_ACCOUNT_ENV: &str = "FUXI_FCM_SERVICE_ACCOUNT";

/// service account JSON 的反序列化形。字段名跟 Google 下发的 JSON 对齐。
#[derive(Debug, Clone, Deserialize)]
struct ServiceAccountJson {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

/// RS256 JWT 的 claims——OAuth2 jwt-bearer grant 的 assertion。
///
/// 抽成 pub(crate) struct（而不是匿名 json!）让单测能离线断言 claims 形状：
/// iss/scope/aud 对不对、exp-iat 是不是 1h。
#[derive(Debug, Serialize)]
pub(crate) struct JwtClaims {
    /// 签发者——service account 的 client_email。
    pub iss: String,
    /// 申请的 scope——FCM 发消息固定这个。
    pub scope: String,
    /// 受众——service account 的 token_uri。
    pub aud: String,
    /// 签发时刻（Unix 秒）。
    pub iat: i64,
    /// 过期时刻（Unix 秒）——iat + 3600。
    pub exp: i64,
}

/// FCM 发消息申请的 OAuth2 scope。
pub(crate) const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

/// access_token 缓存提前刷新窗口——离过期还剩 <60s 就重新换。
const TOKEN_REFRESH_SLACK: Duration = Duration::from_secs(60);

impl JwtClaims {
    /// 按 service account + 当前时刻构造一份 1h 有效的 claims。
    pub(crate) fn build(client_email: &str, token_uri: &str, now_unix: i64) -> Self {
        Self {
            iss: client_email.to_string(),
            scope: FCM_SCOPE.to_string(),
            aud: token_uri.to_string(),
            iat: now_unix,
            exp: now_unix + 3600,
        }
    }
}

/// OAuth2 token 兑换端点的响应体。
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    /// 有效秒数——一般 3599。
    expires_in: u64,
}

/// 缓存中的 access_token——值 + 过期时刻。
struct CachedToken {
    value: String,
    /// 用 `Instant` 而不是 wall-clock：只关心"还能用多久"，不受系统时钟跳变影响。
    expires_at: Instant,
}

/// 从 service account JSON 加载的 FCM 凭据。
///
/// `access_token()` 带过期缓存——多条 token fan-out 时复用同一个 bearer，
/// 不会每条都去换。提前 60s 刷新避免边界上 token 刚过期。
pub struct FcmCredentials {
    project_id: String,
    client_email: String,
    /// RSA PKCS#8 私钥 PEM——签 RS256 JWT 用。
    private_key_pem: String,
    token_uri: String,
    /// 缓存的 access_token——`Mutex` 因为多 fan-out 并发会同时取。
    cached: Mutex<Option<CachedToken>>,
    http: reqwest::Client,
}

impl FcmCredentials {
    /// 解析默认 service account 路径：env `FUXI_FCM_SERVICE_ACCOUNT` 优先，
    /// 否则 `~/.fuxi/fcm_service_account.json`。无 `$HOME` 且无 env 时返 None。
    pub fn default_path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(FCM_SERVICE_ACCOUNT_ENV) {
            return Some(PathBuf::from(p));
        }
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join(".fuxi")
                .join("fcm_service_account.json")
        })
    }

    /// 从 service account JSON 文件加载凭据。文件缺失 / 字段不全 → `Err`，
    /// 由调用方决定退化（生产 wiring 会 log warn + no-op，不 panic）。
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|e| {
            Error::Internal(format!("读 FCM service account {}: {e}", path.display()))
        })?;
        let sa: ServiceAccountJson = serde_json::from_str(&raw)
            .map_err(|e| Error::Internal(format!("解析 FCM service account JSON: {e}")))?;
        Ok(Self {
            project_id: sa.project_id,
            client_email: sa.client_email,
            private_key_pem: sa.private_key,
            token_uri: sa.token_uri,
            cached: Mutex::new(None),
            http: reqwest::Client::new(),
        })
    }

    /// 用缺省路径加载——`load(default_path())` 的便捷包装。
    pub fn load_default() -> Result<Self> {
        let path = Self::default_path().ok_or_else(|| {
            Error::Internal("无法解析 FCM service account 路径：$HOME 未设置".into())
        })?;
        Self::load(path)
    }

    /// FCM project id——拼 messages:send URL 用。
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// 拿一个有效 access_token：缓存未过期直接返，否则换新。
    ///
    /// WHY 提前 60s 刷新：避免拿到一个再过 1s 就失效的 token，让 fan-out
    /// 整批用同一个 bearer 不会中途失效。
    pub async fn access_token(&self) -> Result<String> {
        {
            let cached = self.cached.lock().await;
            if let Some(c) = cached.as_ref()
                && c.expires_at > Instant::now() + TOKEN_REFRESH_SLACK
            {
                return Ok(c.value.clone());
            }
        }
        // 缓存失效——换新。锁外做 HTTP，换完再写回。
        let (token, expires_in) = self.exchange_token().await?;
        let mut cached = self.cached.lock().await;
        *cached = Some(CachedToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });
        Ok(token)
    }

    /// 签 JWT → POST token_uri 换 access_token。返 (token, expires_in_secs)。
    async fn exchange_token(&self) -> Result<(String, u64)> {
        let jwt = self.sign_jwt()?;
        let resp = self
            .http
            .post(&self.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", jwt.as_str()),
            ])
            .send()
            .await
            .map_err(|e| Error::Internal(format!("FCM OAuth2 token 请求失败: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "FCM OAuth2 token 兑换 {status}: {body}"
            )));
        }
        let tr: TokenResponse = resp
            .json()
            .await
            .map_err(|e| Error::Internal(format!("FCM OAuth2 token 响应解析失败: {e}")))?;
        Ok((tr.access_token, tr.expires_in))
    }

    /// 用 service account RSA 私钥签一个 RS256 JWT。
    fn sign_jwt(&self) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let claims = JwtClaims::build(&self.client_email, &self.token_uri, now);
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(self.private_key_pem.as_bytes())
            .map_err(|e| Error::Internal(format!("FCM RSA 私钥解析失败: {e}")))?;
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| Error::Internal(format!("FCM JWT 签名失败: {e}")))
    }
}

// ─── FcmPusher trait ─────────────────────────────────────────────────

/// 单条 FCM 推送的结果——区分"投递成功" / "token 已失效需清理" / 其它错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FcmSendOutcome {
    /// FCM 接收了消息。
    Delivered,
    /// token 已失效（app 卸载 / 清数据）——`notify_fcm` 应 delete_token 清掉。
    Stale,
}

/// 单条 FCM 发送原语；mirror [`super::notify::PushSender`]。
///
/// 用 `#[async_trait]` 让 `Arc<dyn FcmPusher>` 可注入——hooks 测试注 mock，
/// 生产注 [`HttpFcmSender`]。
#[async_trait]
pub trait FcmPusher: Send + Sync {
    /// 推一条；返 [`FcmSendOutcome`] 让 `notify_fcm` 决定要不要清 stale token。
    /// 非 stale 的失败（5xx / 网络）走 `Err`，由 `notify_fcm` 吞掉只 log。
    async fn send_one(&self, token: &str, payload: &PushPayload) -> Result<FcmSendOutcome>;
}

/// 一个什么都不做的 [`FcmPusher`]——生产 wiring 在 service account 文件缺失时
/// 退化到它，让没配 FCM 的部署照常跑（不 panic）。
pub struct NoopFcmSender;

#[async_trait]
impl FcmPusher for NoopFcmSender {
    async fn send_one(&self, _token: &str, _payload: &PushPayload) -> Result<FcmSendOutcome> {
        // 无凭据——当作"投递成功"返回，避免 notify_fcm 把它当错误刷 warn。
        Ok(FcmSendOutcome::Delivered)
    }
}

// ─── HttpFcmSender ───────────────────────────────────────────────────

/// FCM HTTP v1 API 的 messages:send 端点模板。
const FCM_SEND_URL_TMPL: &str = "https://fcm.googleapis.com/v1/projects/{project}/messages:send";

/// 用 FCM HTTP v1 API 真发推送。
pub struct HttpFcmSender {
    creds: Arc<FcmCredentials>,
    http: reqwest::Client,
}

impl HttpFcmSender {
    pub fn new(creds: Arc<FcmCredentials>) -> Self {
        Self {
            creds,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl FcmPusher for HttpFcmSender {
    async fn send_one(&self, token: &str, payload: &PushPayload) -> Result<FcmSendOutcome> {
        let access_token = self.creds.access_token().await?;
        let url = FCM_SEND_URL_TMPL.replace("{project}", self.creds.project_id());
        // FCM HTTP v1 message：notification 渲染系统通知栏，data.url 给 app
        // 点击跳转，android.priority=high 让 doze 模式下也即时唤醒。
        let body = serde_json::json!({
            "message": {
                "token": token,
                "notification": { "title": payload.title, "body": payload.body },
                "data": { "url": payload.url },
                "android": { "priority": "high" }
            }
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&access_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("FCM messages:send 请求失败: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(FcmSendOutcome::Delivered);
        }
        let text = resp.text().await.unwrap_or_default();
        // token 失效：FCM 返 404 + error.status == UNREGISTERED。两条都命中
        // 才判 Stale——单看 404 可能是临时路由问题，误删活 token 代价大。
        if status.as_u16() == 404 && fcm_error_is_unregistered(&text) {
            return Ok(FcmSendOutcome::Stale);
        }
        Err(Error::Internal(format!(
            "FCM messages:send {status}: {text}"
        )))
    }
}

/// 判 FCM 错误响应体里 `error.status` 是不是 `UNREGISTERED`。
fn fcm_error_is_unregistered(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("status"))
                .and_then(|s| s.as_str())
                .map(|s| s == "UNREGISTERED")
        })
        .unwrap_or(false)
}

// ─── notify_fcm ──────────────────────────────────────────────────────

/// fan-out 一条推送给所有 FCM token。
///
/// 哲学跟 [`super::notify::notify`] 一致：单条失败只 log warn 不中断其余。
/// 额外动作——收到 [`FcmSendOutcome::Stale`] 就 `delete_token` 清掉死 token。
pub async fn notify_fcm(
    pool: &SqlitePool,
    sender: Arc<dyn FcmPusher>,
    payload: &PushPayload,
) -> Result<FcmNotifyOutcome> {
    let tokens = list_tokens(pool).await?;
    if tokens.is_empty() {
        return Ok(FcmNotifyOutcome {
            attempted: 0,
            delivered: 0,
            pruned: 0,
        });
    }
    let mut delivered = 0usize;
    let mut pruned = 0usize;
    for token in &tokens {
        match sender.send_one(token, payload).await {
            Ok(FcmSendOutcome::Delivered) => delivered += 1,
            Ok(FcmSendOutcome::Stale) => {
                // token 已死——清掉，下次 fan-out 不再尝试。
                if let Err(e) = delete_token(pool, token).await {
                    tracing::warn!(error = %e, "清理失效 FCM token 失败");
                } else {
                    pruned += 1;
                }
            }
            Err(e) => tracing::warn!(error = %e, "FCM fan-out 单条失败"),
        }
    }
    Ok(FcmNotifyOutcome {
        attempted: tokens.len(),
        delivered,
        pruned,
    })
}

/// FCM fan-out 结果——给调用方 / 测试断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FcmNotifyOutcome {
    /// 尝试发送的 token 数。
    pub attempted: usize,
    /// 成功投递的条数。
    pub delivered: usize,
    /// 因 Stale 被清理掉的 token 数。
    pub pruned: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        std::mem::forget(dir);
        pool
    }

    // ── store CRUD ──

    /// upsert 新 token + list 看到。
    #[tokio::test]
    async fn upsert_then_list() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-1", "Pixel").await.expect("upsert");
        let tokens = list_tokens(&pool).await.expect("list");
        assert_eq!(tokens, vec!["tok-1".to_string()]);
    }

    /// 同 token 重复 upsert = 更新 device_name，不新增行。
    #[tokio::test]
    async fn upsert_is_idempotent_on_token() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-1", "旧名").await.unwrap();
        upsert_token(&pool, "tok-1", "新名").await.unwrap();
        let tokens = list_tokens(&pool).await.unwrap();
        assert_eq!(tokens.len(), 1, "同 token 应 upsert 不新增");

        let name: (String,) = sqlx::query_as("SELECT device_name FROM fcm_tokens WHERE token=?1")
            .bind("tok-1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name.0, "新名");
    }

    /// delete_token 后 list 不再含它。
    #[tokio::test]
    async fn delete_removes_token() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-1", "a").await.unwrap();
        upsert_token(&pool, "tok-2", "b").await.unwrap();
        delete_token(&pool, "tok-1").await.unwrap();
        let tokens = list_tokens(&pool).await.unwrap();
        assert_eq!(tokens, vec!["tok-2".to_string()]);
    }

    /// 空表 list 返空 vec，不报错。
    #[tokio::test]
    async fn list_empty_is_ok() {
        let pool = fresh_pool().await;
        assert!(list_tokens(&pool).await.unwrap().is_empty());
    }

    // ── JWT claims 形状 ──

    /// claims：iss=client_email、scope 固定、aud=token_uri、exp-iat=1h。
    #[test]
    fn jwt_claims_shape_is_correct() {
        let claims = JwtClaims::build(
            "svc@fuxi-im.iam.gserviceaccount.com",
            "https://oauth2.googleapis.com/token",
            1_700_000_000,
        );
        assert_eq!(claims.iss, "svc@fuxi-im.iam.gserviceaccount.com");
        assert_eq!(claims.scope, FCM_SCOPE);
        assert_eq!(claims.aud, "https://oauth2.googleapis.com/token");
        assert_eq!(claims.iat, 1_700_000_000);
        assert_eq!(claims.exp - claims.iat, 3600, "JWT 应 1h 有效");

        // 序列化形状——OAuth2 jwt-bearer assertion 要这五个字段。
        let v = serde_json::to_value(&claims).unwrap();
        for k in ["iss", "scope", "aud", "iat", "exp"] {
            assert!(v.get(k).is_some(), "claims 缺字段 {k}");
        }
    }

    // ── fcm_error_is_unregistered ──

    /// FCM 404 + UNREGISTERED → 判 stale。
    #[test]
    fn unregistered_error_body_is_recognized() {
        let body = r#"{"error":{"code":404,"message":"Requested entity was not found.","status":"UNREGISTERED"}}"#;
        assert!(fcm_error_is_unregistered(body));
    }

    /// 别的 error.status（如 INVALID_ARGUMENT）不判 stale。
    #[test]
    fn non_unregistered_error_is_not_stale() {
        let body = r#"{"error":{"code":400,"status":"INVALID_ARGUMENT"}}"#;
        assert!(!fcm_error_is_unregistered(body));
    }

    /// 非 JSON / 空 body 不判 stale（不误删 token）。
    #[test]
    fn garbage_body_is_not_stale() {
        assert!(!fcm_error_is_unregistered(""));
        assert!(!fcm_error_is_unregistered("upstream timeout"));
    }

    // ── notify_fcm with mock pusher ──

    /// mock pusher：记录调用，可指定某些 token 返 Stale / Err。
    #[derive(Default)]
    struct MockFcmSender {
        calls: StdMutex<Vec<(String, PushPayload)>>,
        stale: StdMutex<Vec<String>>,
        fail: StdMutex<Vec<String>>,
    }

    #[async_trait]
    impl FcmPusher for MockFcmSender {
        async fn send_one(&self, token: &str, payload: &PushPayload) -> Result<FcmSendOutcome> {
            self.calls
                .lock()
                .unwrap()
                .push((token.to_string(), payload.clone()));
            if self.fail.lock().unwrap().iter().any(|t| t == token) {
                return Err(Error::Internal("mock fail".into()));
            }
            if self.stale.lock().unwrap().iter().any(|t| t == token) {
                return Ok(FcmSendOutcome::Stale);
            }
            Ok(FcmSendOutcome::Delivered)
        }
    }

    /// 0 token 时 notify_fcm Ok 但 attempted=0。
    #[tokio::test]
    async fn notify_fcm_no_tokens_is_ok() {
        let pool = fresh_pool().await;
        let mock = Arc::new(MockFcmSender::default());
        let out = notify_fcm(&pool, mock.clone(), &PushPayload::new("t", "b", "/"))
            .await
            .expect("notify");
        assert_eq!(
            out,
            FcmNotifyOutcome {
                attempted: 0,
                delivered: 0,
                pruned: 0
            }
        );
        assert!(mock.calls.lock().unwrap().is_empty());
    }

    /// 多 token → 各推一次，payload 透传。
    #[tokio::test]
    async fn notify_fcm_fans_out_to_all() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-1", "a").await.unwrap();
        upsert_token(&pool, "tok-2", "b").await.unwrap();
        let mock = Arc::new(MockFcmSender::default());
        let payload = PushPayload::new("任务完成", "ERP-1066", "/#/task/abc");
        let out = notify_fcm(&pool, mock.clone(), &payload)
            .await
            .expect("notify");
        assert_eq!(
            out,
            FcmNotifyOutcome {
                attempted: 2,
                delivered: 2,
                pruned: 0
            }
        );
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1.title, "任务完成");
    }

    /// 单条 Err 不中断其余——delivered < attempted。
    #[tokio::test]
    async fn notify_fcm_continues_on_single_failure() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-1", "a").await.unwrap();
        upsert_token(&pool, "tok-2", "b").await.unwrap();
        let mock = Arc::new(MockFcmSender::default());
        mock.fail.lock().unwrap().push("tok-1".into());
        let out = notify_fcm(&pool, mock.clone(), &PushPayload::new("t", "b", "/"))
            .await
            .expect("notify");
        assert_eq!(out.attempted, 2);
        assert_eq!(out.delivered, 1);
        assert_eq!(out.pruned, 0);
    }

    /// Stale token → notify_fcm 清掉它，下次 list 不再含。
    #[tokio::test]
    async fn notify_fcm_prunes_stale_token() {
        let pool = fresh_pool().await;
        upsert_token(&pool, "tok-stale", "a").await.unwrap();
        upsert_token(&pool, "tok-live", "b").await.unwrap();
        let mock = Arc::new(MockFcmSender::default());
        mock.stale.lock().unwrap().push("tok-stale".into());

        let out = notify_fcm(&pool, mock.clone(), &PushPayload::new("t", "b", "/"))
            .await
            .expect("notify");
        assert_eq!(out.attempted, 2);
        assert_eq!(out.delivered, 1);
        assert_eq!(out.pruned, 1, "失效 token 应被清理");

        let remaining = list_tokens(&pool).await.unwrap();
        assert_eq!(remaining, vec!["tok-live".to_string()], "死 token 已清");
    }

    /// NoopFcmSender 永远返 Delivered，不报错。
    #[tokio::test]
    async fn noop_sender_is_inert() {
        let out = NoopFcmSender
            .send_one("any", &PushPayload::new("t", "b", "/"))
            .await
            .expect("noop");
        assert_eq!(out, FcmSendOutcome::Delivered);
    }
}
