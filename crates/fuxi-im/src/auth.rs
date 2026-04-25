//! HMAC-SHA256 device token：sign / verify / 文件密钥管理。
//!
//! ## 协议
//!
//! token = `base64url(JSON(payload))` + `.` + `base64url(HMAC-SHA256(payload_json_bytes))`
//!
//! 其中 payload =
//! ```json
//! { "device_id": "<uuid>", "name": "<人类可读>", "expires_at": "<rfc3339>" }
//! ```
//!
//! 验证流程：split `.` → base64url decode 双方 → 用同 secret 重算 HMAC →
//! `subtle::ConstantTimeEq` 常量时间比对 → 成功则 deser payload + 检查 `expires_at > now`。
//!
//! 这不是 JWT——故意不照 JWT 那套 header/alg 字段，因为：
//! - 单用户 + 单算法（公理 #1 借鉴不抄路径），用不到 alg 协商
//! - JWT alg=none 漏洞史长，简化协议、固化算法更安全
//!
//! ## 共享密钥
//!
//! 服务端持唯一 32 字节 HMAC secret 在 `~/.fuxi/im_hmac.key`（首启随机生成、
//! 文件权限 0600）。所有 device token 共用同一 secret——和 device_tokens
//! 表里"每条 token 独立 hmac_secret"是正交的两层（表里那列保留给 future
//! per-device rotation；当前 verify 路径只用全局 secret）。

#![allow(dead_code)]

use crate::error::{Error, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// HMAC secret 字节数。32 = SHA-256 输出长度，常规推荐。
pub const HMAC_KEY_BYTES: usize = 32;

/// device token 默认 TTL = 1 年。
pub const DEFAULT_TOKEN_TTL_DAYS: i64 = 365;

/// cookie 名——middleware 和 handler 共用此常量。
pub const COOKIE_NAME: &str = "fuxi_im_token";

/// HMAC secret 文件相对 `~/.fuxi`。
pub const KEY_FILE_NAME: &str = "im_hmac.key";

/// token payload——序列化为 JSON 后参与 HMAC 计算。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenClaims {
    /// 设备主键，handler 入库 `device_tokens.token_id` 用同一 uuid。
    pub device_id: String,
    /// 用户取的名字（"以琳的 iPhone"）。
    pub name: String,
    /// rfc3339 过期时刻——超过即拒。
    pub expires_at: DateTime<Utc>,
}

/// 包裹 HMAC bytes 防 Debug / panic 路径泄漏。
#[derive(Clone)]
pub struct HmacSecret(SecretString);

impl HmacSecret {
    /// 直接用一段 ASCII / utf8 串作 secret——测试 + 显式注入路径用。
    pub fn from_string(s: String) -> Self {
        Self(SecretString::from(s))
    }

    /// 从 `~/.fuxi/im_hmac.key` 读，缺则随机生成 32 字节并写入（权限 0600）。
    ///
    /// daemon 启动期一次性调用——`fuxi im start` 子命令的入口。
    pub fn load_or_create_default() -> Result<Self> {
        let dir = default_fuxi_dir()?;
        let path = dir.join(KEY_FILE_NAME);
        Self::load_or_create(&path)
    }

    /// 显式路径版——单元测试和 daemon 都走它。
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            let raw = std::fs::read(path)?;
            // base64url 编码格式存在文件——人眼可读且只有 ASCII。
            let s = String::from_utf8(raw)
                .map_err(|e| Error::Internal(format!("im_hmac.key 不是 utf8：{e}")))?;
            return Ok(Self::from_string(s.trim().to_string()));
        }
        let mut buf = [0u8; HMAC_KEY_BYTES];
        rand::thread_rng().fill_bytes(&mut buf);
        let encoded = URL_SAFE_NO_PAD.encode(buf);
        std::fs::write(path, encoded.as_bytes())?;
        // 0600：rw-------；防同机其他用户偷 key。
        // WHY 仅 unix：windows 没相同概念；现阶段 fuxi 部署目标 = mac/linux。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(path)?.permissions();
            perm.set_mode(0o600);
            std::fs::set_permissions(path, perm)?;
        }
        Ok(Self::from_string(encoded))
    }

    fn expose_bytes(&self) -> &[u8] {
        self.0.expose_secret().as_bytes()
    }
}

impl std::fmt::Debug for HmacSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HmacSecret(***)")
    }
}

/// 解析 `~` 为家目录；无 `HOME` 时回错。
pub fn default_fuxi_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| Error::Internal("HOME 未设置，无法定位 ~/.fuxi".to_string()))?;
    Ok(PathBuf::from(home).join(".fuxi"))
}

/// 给 claims 签 token：base64url(json) + "." + base64url(HMAC)。
pub fn sign_token(secret: &HmacSecret, claims: &TokenClaims) -> Result<String> {
    let body = serde_json::to_vec(claims)?;
    let mut mac =
        HmacSha256::new_from_slice(secret.expose_bytes()).expect("HMAC-SHA256 接受任意长度 key");
    mac.update(&body);
    let sig = mac.finalize().into_bytes();
    let body_enc = URL_SAFE_NO_PAD.encode(&body);
    let sig_enc = URL_SAFE_NO_PAD.encode(sig);
    Ok(format!("{body_enc}.{sig_enc}"))
}

/// 验签 + 过期检查；成功返回 claims，失败 `Error::Unauthorized`。
///
/// 各种失败原因对外都是 401（避免 oracle）；trace 日志区分原因便于排错。
pub fn verify_token(secret: &HmacSecret, token: &str) -> Result<TokenClaims> {
    let (body_enc, sig_enc) = token
        .split_once('.')
        .ok_or_else(|| Error::Unauthorized("token 缺分隔符".into()))?;

    let body = URL_SAFE_NO_PAD
        .decode(body_enc)
        .map_err(|_| Error::Unauthorized("token body base64 解码失败".into()))?;
    let provided_sig = URL_SAFE_NO_PAD
        .decode(sig_enc)
        .map_err(|_| Error::Unauthorized("token sig base64 解码失败".into()))?;

    let mut mac =
        HmacSha256::new_from_slice(secret.expose_bytes()).expect("HMAC-SHA256 接受任意长度 key");
    mac.update(&body);
    let expected = mac.finalize().into_bytes();

    if expected.len() != provided_sig.len() {
        return Err(Error::Unauthorized("token 签名长度不符".into()));
    }
    if expected.ct_eq(&provided_sig).unwrap_u8() != 1 {
        return Err(Error::Unauthorized("token 签名不匹配".into()));
    }

    let claims: TokenClaims = serde_json::from_slice(&body)
        .map_err(|e| Error::Unauthorized(format!("token payload 不合法：{e}")))?;
    if claims.expires_at <= Utc::now() {
        return Err(Error::Unauthorized("token 已过期".into()));
    }
    Ok(claims)
}

/// 用默认 TTL（1 年）造一份 claims——pair handler 调用。
pub fn fresh_claims(device_id: String, name: String) -> TokenClaims {
    TokenClaims {
        device_id,
        name,
        expires_at: Utc::now() + Duration::days(DEFAULT_TOKEN_TTL_DAYS),
    }
}

/// 拼好 `Set-Cookie` header 值。HttpOnly + Secure + SameSite=Lax + 1 年 Max-Age + Path=/。
///
/// WHY 手写而非用 cookie crate：单一固定字段集；引一个 crate 不划算。
pub fn build_set_cookie(token: &str, ttl_days: i64) -> String {
    let max_age = ttl_days.max(0) * 86_400;
    format!("{COOKIE_NAME}={token}; Path=/; Max-Age={max_age}; HttpOnly; Secure; SameSite=Lax")
}

/// 从 `Cookie:` header 字符串里抠出 token；找不到返 None。
///
/// `Cookie` header 是 `name=value; name2=value2` 形式；按 `; ` 切再找前缀。
pub fn extract_token_from_cookie_header(header: &str) -> Option<&str> {
    for part in header.split(';') {
        let trimmed = part.trim();
        if let Some(value) = trimmed.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_secret() -> HmacSecret {
        HmacSecret::from_string("test-key-not-random".into())
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let secret = fixture_secret();
        let claims = fresh_claims("dev-001".into(), "以琳的 iPhone".into());
        let token = sign_token(&secret, &claims).expect("sign");
        let got = verify_token(&secret, &token).expect("verify");
        assert_eq!(got.device_id, "dev-001");
        assert_eq!(got.name, "以琳的 iPhone");
        // expires_at 经 rfc3339 序列化-解析的精度损失需容忍——比较一秒内即可。
        let drift = (got.expires_at - claims.expires_at).num_seconds().abs();
        assert!(drift <= 1, "rfc3339 精度漂移过大：{drift}s");
    }

    #[test]
    fn verify_rejects_expired_token() {
        let secret = fixture_secret();
        let claims = TokenClaims {
            device_id: "dev-001".into(),
            name: "stale".into(),
            // 1 秒前过期
            expires_at: Utc::now() - Duration::seconds(1),
        };
        let token = sign_token(&secret, &claims).expect("sign");
        let err = verify_token(&secret, &token).unwrap_err();
        assert!(
            matches!(err, Error::Unauthorized(ref m) if m.contains("过期")),
            "预期过期错误，得到 {err:?}"
        );
    }

    #[test]
    fn verify_rejects_tampered_payload() {
        let secret = fixture_secret();
        let claims = fresh_claims("dev-001".into(), "原".into());
        let token = sign_token(&secret, &claims).expect("sign");
        // 拼一个改了 payload 但保留原签名的 token——签名必然不匹配。
        let mut tampered_claims = claims.clone();
        tampered_claims.name = "假".into();
        let evil_body = serde_json::to_vec(&tampered_claims).unwrap();
        let (_, sig_enc) = token.split_once('.').unwrap();
        let evil_token = format!("{}.{}", URL_SAFE_NO_PAD.encode(&evil_body), sig_enc);
        let err = verify_token(&secret, &evil_token).unwrap_err();
        assert!(
            matches!(err, Error::Unauthorized(ref m) if m.contains("签名")),
            "预期签名错误，得到 {err:?}"
        );
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let claims = fresh_claims("dev-001".into(), "x".into());
        let token = sign_token(&fixture_secret(), &claims).expect("sign");
        let other = HmacSecret::from_string("another-key".into());
        let err = verify_token(&other, &token).unwrap_err();
        assert!(
            matches!(err, Error::Unauthorized(_)),
            "预期 401，得到 {err:?}"
        );
    }

    #[test]
    fn verify_rejects_malformed_token() {
        let secret = fixture_secret();
        for bad in ["", "no-dot", ".", "a.b.c"] {
            let err = verify_token(&secret, bad).unwrap_err();
            assert!(
                matches!(err, Error::Unauthorized(_)),
                "{bad:?} 应 401，得到 {err:?}"
            );
        }
    }

    #[test]
    fn load_or_create_persists_key_across_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("im_hmac.key");
        let s1 = HmacSecret::load_or_create(&path).expect("first");
        // 第二次调用必须读出同样的 key——文件已存在路径。
        let s2 = HmacSecret::load_or_create(&path).expect("second");
        assert_eq!(s1.expose_bytes(), s2.expose_bytes());

        // 同一 secret 才能 verify 同一 token——证明 key 真的复用。
        let claims = fresh_claims("dev-001".into(), "phone".into());
        let token = sign_token(&s1, &claims).unwrap();
        verify_token(&s2, &token).expect("token 应在重启后仍可验");
    }

    #[cfg(unix)]
    #[test]
    fn load_or_create_writes_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("im_hmac.key");
        let _ = HmacSecret::load_or_create(&path).expect("create");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "im_hmac.key 必须 0600，得到 {mode:o}");
    }

    #[test]
    fn build_set_cookie_has_required_attrs() {
        let cookie = build_set_cookie("abc.def", 365);
        assert!(cookie.contains("fuxi_im_token=abc.def"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
        // 365 天 = 31_536_000 秒
        assert!(cookie.contains("Max-Age=31536000"));
    }

    #[test]
    fn extract_token_from_cookie_header_handles_multi_cookie() {
        let h = "other=foo; fuxi_im_token=tok.sig; sessionid=zzz";
        assert_eq!(extract_token_from_cookie_header(h), Some("tok.sig"));
        let only = "fuxi_im_token=just";
        assert_eq!(extract_token_from_cookie_header(only), Some("just"));
        assert_eq!(extract_token_from_cookie_header("none"), None);
    }
}
