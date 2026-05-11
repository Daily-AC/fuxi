//! Bearer token 鉴权——WS 升级握手时检查 `Authorization: Bearer <token>`。
//!
//! 设计：
//! - 启动期一次性把 token 读进内存；运行时 O(1) 比较。
//! - 比较走常量时间（防 timing oracle）；token 长度短、量小，差异可忽略，
//!   但对外暴露的鉴权点保持习惯——成本低。
//! - 文件权限 600 是契约，但本 crate 不强校验权限位（部署文档约束）。
//!   过严校验会让本地开发用 chmod 666 的临时 token 路径失效；trust the operator。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 启动期加载：从 `path` 读一行 token。
pub fn load_token(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读取 wake.token 失败：{}", path.display()))?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("wake.token 文件为空：{}", path.display());
    }
    Ok(trimmed)
}

/// 默认 token 路径——`~/.fuxi/wake.token`。`HOME` 取不到则报错（让用户显式 ENV
/// 覆盖比悄悄落到 cwd 安全）。
pub fn default_token_path() -> Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME 未设置——请用 --token-file 指定 wake.token 路径")?;
    Ok(PathBuf::from(home).join(".fuxi").join("wake.token"))
}

/// 常量时间字符串比较——长度不等返 false 后才进位运算，泄露的只有"长度差"，
/// 这点对 32 字节随机 token 来说不构成威胁。
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 解析 `Authorization: Bearer <token>`，返回 token 字符串。失败 = None。
pub fn parse_bearer(header: &str) -> Option<&str> {
    let token = header.strip_prefix("Bearer ")?;
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_ok() {
        assert_eq!(parse_bearer("Bearer abc123"), Some("abc123"));
        assert_eq!(parse_bearer("Bearer  spaced "), Some("spaced"));
    }

    #[test]
    fn parse_bearer_rejects_other_schemes() {
        assert_eq!(parse_bearer("Basic abc"), None);
        assert_eq!(parse_bearer("bearer lowercase"), None); // 大小写敏感
        assert_eq!(parse_bearer("Bearer "), None);
        assert_eq!(parse_bearer(""), None);
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn load_token_strips_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wake.token");
        std::fs::write(&p, "secret-xyz\n").unwrap();
        assert_eq!(load_token(&p).unwrap(), "secret-xyz");
    }

    #[test]
    fn load_token_rejects_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wake.token");
        std::fs::write(&p, "  \n  ").unwrap();
        assert!(load_token(&p).is_err());
    }

    #[test]
    fn load_token_missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nope.token");
        assert!(load_token(&p).is_err());
    }
}
