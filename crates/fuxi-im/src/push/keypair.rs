//! VAPID 密钥对持久化（Decision 14 E）。
//!
//! 文件：`~/.fuxi/im_vapid.json`，权限 0600。结构：
//! ```json
//! { "private_pem": "-----BEGIN EC PRIVATE KEY-----\n…", "public_b64url": "B…" }
//! ```
//!
//! WHY 用 `openssl` shell 而不是引 rust crypto crate：web-push 自身已经依赖
//! 系统 / vendored OpenSSL；shell 一次产 PEM 在 0.x 秒内完成 + 0 额外依赖。
//! 公理 #4 "CLI 是工具层的唯一形态"在这里也成立——keygen 是一次性工具调用。
//!
//! WHY 同时存 public_b64url：前端 PWA `pushManager.subscribe()` 需要
//! `applicationServerKey`，原始 P-256 公钥的 uncompressed 65 字节 base64url。
//! 我们一次性算好，serve 给前端时直接读即可，不用每次再从 PEM 解。

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// VAPID keypair 在 disk 上的序列化形式。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VapidKeypair {
    /// EC 私钥 PEM（PKCS8 或 SEC1 都可，web-push 都能解）。
    pub private_pem: String,
    /// P-256 公钥的 uncompressed point base64url（无 padding）。前端
    /// `applicationServerKey` 要用这个串。
    pub public_b64url: String,
}

/// 解析默认 `~/.fuxi/im_vapid.json`。无 `$HOME` 返 None。
pub fn default_keypair_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("im_vapid.json"))
}

/// 加载已存在的 keypair；不存在则返 [`Error::NotFound`]——调用方决定要不要 generate。
pub fn load(path: impl AsRef<Path>) -> Result<VapidKeypair> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::NotFound(format!(
            "VAPID keypair not at {}",
            path.display()
        )));
    }
    let raw = std::fs::read_to_string(path).map_err(Error::Io)?;
    let kp: VapidKeypair = serde_json::from_str(&raw)?;
    Ok(kp)
}

/// 用 `openssl` 生成 P-256 keypair 并写盘（0600）。父目录不存在则创建。
///
/// 已存在不覆盖（返 [`Error::BadRequest`]）——不让一次手滑就把活订阅废了。
/// 如确需轮换，先手动删旧文件。
pub fn generate_at(path: impl AsRef<Path>) -> Result<VapidKeypair> {
    let path = path.as_ref();
    if path.exists() {
        return Err(Error::BadRequest(format!(
            "VAPID keypair 已存在于 {}；如需轮换请先删旧文件",
            path.display()
        )));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    let pem = run_openssl_genpkey()?;
    let public = derive_public_b64url(&pem)?;

    let kp = VapidKeypair {
        private_pem: pem,
        public_b64url: public,
    };
    write_secure(path, &kp)?;
    Ok(kp)
}

/// 加载或生成——首启的"懒生成"通路。
pub fn load_or_generate(path: impl AsRef<Path>) -> Result<VapidKeypair> {
    let path = path.as_ref();
    if path.exists() {
        load(path)
    } else {
        generate_at(path)
    }
}

fn write_secure(path: &Path, kp: &VapidKeypair) -> Result<()> {
    let body = serde_json::to_string_pretty(kp)?;
    std::fs::write(path, body).map_err(Error::Io)?;
    set_mode_0600(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perm = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perm).map_err(Error::Io)
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> Result<()> {
    // Windows 上 ACL 由系统继承——伏羲日常落点是 macOS / Linux，先 noop。
    Ok(())
}

/// 调 `openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256` 出 PEM。
fn run_openssl_genpkey() -> Result<String> {
    let out = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
        ])
        .output()
        .map_err(|e| Error::Internal(format!("spawn openssl genpkey: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(Error::Internal(format!("openssl genpkey 失败: {stderr}")));
    }
    let pem = String::from_utf8(out.stdout)
        .map_err(|e| Error::Internal(format!("openssl 输出非 utf8: {e}")))?;
    if !pem.contains("PRIVATE KEY") {
        return Err(Error::Internal(
            "openssl genpkey 输出不像 PEM 私钥".to_string(),
        ));
    }
    Ok(pem)
}

/// 从 PEM 私钥解出 P-256 公钥（uncompressed point）的 base64url-no-pad。
///
/// `openssl pkey -in <pem> -pubout -outform DER` 出来是 SubjectPublicKeyInfo
/// DER，最后 65 字节就是 uncompressed point（0x04 + X32 + Y32）。
fn derive_public_b64url(private_pem: &str) -> Result<String> {
    use base64::Engine;
    use std::io::Write;

    let mut child = Command::new("openssl")
        .args(["pkey", "-pubout", "-outform", "DER"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Internal(format!("spawn openssl pkey: {e}")))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("openssl pkey stdin 不可用".to_string()))?;
        stdin
            .write_all(private_pem.as_bytes())
            .map_err(|e| Error::Internal(format!("写 openssl stdin: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::Internal(format!("wait openssl pkey: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(Error::Internal(format!("openssl pkey 失败: {stderr}")));
    }
    let der = out.stdout;
    if der.len() < 65 {
        return Err(Error::Internal(format!(
            "openssl pkey DER 输出过短（{} 字节）",
            der.len()
        )));
    }
    // uncompressed P-256 公钥固定 65 字节（leading 0x04），位于 SPKI 末尾。
    let pubkey_raw = &der[der.len() - 65..];
    if pubkey_raw[0] != 0x04 {
        return Err(Error::Internal(format!(
            "VAPID 公钥首字节应为 0x04，实为 {:#x}",
            pubkey_raw[0]
        )));
    }
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pubkey_raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// `generate_at` 落盘 + reload 字节级一致；公钥串可解为恰好 65 字节首字节 0x04。
    #[test]
    fn generate_then_load_roundtrip() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_vapid.json");
        let kp1 = generate_at(&path).expect("gen");
        assert!(kp1.private_pem.contains("PRIVATE KEY"));
        assert!(!kp1.public_b64url.is_empty());

        // 公钥串解码 65 字节（0x04 + X + Y）
        use base64::Engine;
        let pub_raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&kp1.public_b64url)
            .expect("decode b64url");
        assert_eq!(pub_raw.len(), 65, "P-256 uncompressed point = 65 bytes");
        assert_eq!(pub_raw[0], 0x04, "uncompressed point header");

        let kp2 = load(&path).expect("load");
        assert_eq!(kp1, kp2);
    }

    /// 已存在文件不覆盖——防一次手滑废所有订阅。
    #[test]
    fn generate_refuses_to_overwrite() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_vapid.json");
        let _kp = generate_at(&path).expect("first gen");
        let err = generate_at(&path).expect_err("second gen 应拒");
        assert!(matches!(err, Error::BadRequest(_)));
    }

    /// `load_or_generate` 首次生成、二次加载。
    #[test]
    fn load_or_generate_works_both_paths() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_vapid.json");
        let kp1 = load_or_generate(&path).expect("first");
        let kp2 = load_or_generate(&path).expect("second");
        assert_eq!(kp1, kp2);
    }

    /// 文件权限 0600（仅 unix 测）。
    #[cfg(unix)]
    #[test]
    fn generated_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im_vapid.json");
        let _kp = generate_at(&path).expect("gen");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "VAPID 私钥文件必须 0600");
    }
}
