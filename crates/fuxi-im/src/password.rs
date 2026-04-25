//! 主密码鉴权（β · Task #9）—— 替代 PIN 当 PWA 登入主路。
//!
//! ## 协议
//!
//! - 用户在 home 跑 `fuxi im set-password` 交互式输完密码 → bcrypt cost 12 hash
//!   → 写 `~/.fuxi/im_password.bcrypt`（权限 0600）
//! - PWA 登入 POST `/api/auth/login` 带密码 → 服务端 bcrypt::verify → 通过签
//!   HMAC token + 写 device_tokens + Set-Cookie
//!
//! ## 文件格式
//!
//! ```json
//! { "version": 1, "hash": "$2b$12$..." }
//! ```
//!
//! `version` 留升级路径——v2 想换 argon2id 或加盐方案直接读 version 分派。
//!
//! ## 安全
//!
//! - bcrypt cost 12 = 单次 verify 约 250ms，攻击者 brute-force 1k req/s 也算划算
//!   防御。CPU 高的家用机用 cost 13 也可，留 const 给将来调整。
//! - 文件 0600 防同机其他用户偷 hash。
//! - 错密码不区分原因（统一 401），trace 区分；防 oracle。
//! - 暴力防御靠 `LoginGuard`（lockout.rs）IP 失败计数，本模块只管 hash/verify。

#![allow(dead_code)]

use crate::error::{Error, Result};
use bcrypt::{hash, verify};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// bcrypt cost——12 是 OWASP 2024 推荐下限。
pub const BCRYPT_COST: u32 = 12;

/// 密码最短长度。比"账号密码"严苛些（家用 IM 唯一关键凭据）。
pub const MIN_PASSWORD_LEN: usize = 8;

/// 文件格式版本。改动 hash 算法时 +1。
pub const FILE_VERSION: u32 = 1;

/// 默认文件名，相对 `~/.fuxi`。
pub const FILE_NAME: &str = "im_password.bcrypt";

/// 落盘的 JSON 结构。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasswordFile {
    pub version: u32,
    /// bcrypt hash（含 cost 和 salt）—— `$2b$12$<salt><hash>` 形式。
    pub hash: String,
}

/// 默认 path：`$HOME/.fuxi/im_password.bcrypt`。`$HOME` 缺时返 None。
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join(FILE_NAME))
}

/// 检查密码 trivial 性质（空 / 太短）。set-password CLI 入口校验。
pub fn validate_password_strength(plain: &str) -> Result<()> {
    if plain.is_empty() {
        return Err(Error::BadRequest("密码不能为空".into()));
    }
    if plain.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::BadRequest(format!(
            "密码长度必须 >= {MIN_PASSWORD_LEN} 字符"
        )));
    }
    Ok(())
}

/// hash + 写文件 + 0600。已存在则**覆盖**——满足"忘密码重设"语义。
pub fn write_password_file(path: &Path, plain: &str) -> Result<()> {
    validate_password_strength(plain)?;

    let h =
        hash(plain, BCRYPT_COST).map_err(|e| Error::Internal(format!("bcrypt hash 失败：{e}")))?;
    let body = PasswordFile {
        version: FILE_VERSION,
        hash: h,
    };
    let serialized = serde_json::to_vec_pretty(&body)?;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &serialized)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

/// 读文件并解析。文件不存在返 `Ok(None)`——上层 handler 区分"未设密码"应回 503。
pub fn read_password_file(path: &Path) -> Result<Option<PasswordFile>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let body: PasswordFile = serde_json::from_slice(&bytes).map_err(Error::Json)?;
            if body.version != FILE_VERSION {
                return Err(Error::Internal(format!(
                    "im_password.bcrypt 文件版本 {} 未支持（期望 {FILE_VERSION}）",
                    body.version
                )));
            }
            Ok(Some(body))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// 用 hash 比对明文密码。bcrypt::verify 自带常量时间比较。
pub fn check_password(plain: &str, file: &PasswordFile) -> Result<bool> {
    verify(plain, &file.hash).map_err(|e| Error::Internal(format!("bcrypt verify 失败：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validate_rejects_empty_and_short() {
        assert!(validate_password_strength("").is_err());
        assert!(validate_password_strength("1234567").is_err());
        // 8 个 ASCII = OK
        assert!(validate_password_strength("12345678").is_ok());
        // 8 个中文（每个 1 char）= OK
        assert!(validate_password_strength("一二三四五六七八").is_ok());
        // 7 个中文不够
        assert!(validate_password_strength("一二三四五六七").is_err());
    }

    #[test]
    fn write_then_check_password_roundtrip() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        write_password_file(&path, "secret-passphrase").expect("write");
        let file = read_password_file(&path).expect("read").expect("exists");
        assert_eq!(file.version, FILE_VERSION);
        assert!(
            file.hash.starts_with("$2b$12$"),
            "bcrypt 输出应是 $2b$12$ 开头：{}",
            file.hash
        );

        assert!(check_password("secret-passphrase", &file).expect("verify ok"));
        assert!(!check_password("wrong", &file).expect("verify ok-but-no-match"));
    }

    #[test]
    fn read_returns_none_when_file_missing() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("nope.bcrypt");
        let got = read_password_file(&path).expect("read");
        assert!(got.is_none());
    }

    #[test]
    fn write_is_idempotent_overwrite() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        write_password_file(&path, "first-pass").expect("write 1");
        let h1 = read_password_file(&path).unwrap().unwrap().hash;

        // 重写 → 同密码也会因为新 salt 出不同 hash（bcrypt 自带 salt）；
        // 同时验证旧密码不能再 verify
        write_password_file(&path, "second-pass").expect("write 2");
        let f2 = read_password_file(&path).unwrap().unwrap();
        assert_ne!(f2.hash, h1, "覆盖后 hash 应变");
        assert!(!check_password("first-pass", &f2).unwrap());
        assert!(check_password("second-pass", &f2).unwrap());
    }

    #[test]
    fn write_rejects_short_password() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        let err = write_password_file(&path, "short").expect_err("应拒短");
        assert!(matches!(err, Error::BadRequest(_)));
        // 失败时不应留半成品文件
        assert!(!path.exists(), "不应写文件");
    }

    #[cfg(unix)]
    #[test]
    fn file_is_0600_after_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        write_password_file(&path, "long-enough").expect("write");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "im_password.bcrypt 必须 0600，得到 {mode:o}");
    }

    #[test]
    fn read_rejects_unknown_version() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("bad.bcrypt");
        std::fs::write(&path, br#"{"version":999,"hash":"$2b$12$x"}"#).unwrap();
        let err = read_password_file(&path).expect_err("应报版本错");
        assert!(matches!(err, Error::Internal(_)));
    }

    #[test]
    fn file_format_is_json_with_version_and_hash() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        write_password_file(&path, "valid-pass").expect("write");

        let bytes = std::fs::read(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("文件应是合法 JSON");
        assert_eq!(parsed["version"], 1);
        let hash = parsed["hash"].as_str().expect("hash 字段必须是 string");
        assert!(hash.starts_with("$2b$12$"));
    }
}
