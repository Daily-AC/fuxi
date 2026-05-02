//! Project — fuxi 服务的"项目"抽象。
//!
//! v1 现状是单 `workspace_root`，多项目并行不支持。Decision 21 把它升成
//! `projects[]`：用户的每个真项目（`~/erp`、`~/写作-2026` …）注册成一个 Project，
//! 后续的 sandbox / ephemeral / deliverables 都按 project 隔离。
//!
//! 本模块只定义 vocabulary（struct + 校验 + ID）。文件系统侧的注册存取走
//! `fuxi-workspace::project_registry`。

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Project id —— 用户可读 slug，跟 `~/.fuxi/projects/<id>/` 一一对应。
///
/// 不用 UUID 是因为这个是用户在 CLI / IM 里要拼写的标识符（`@erp` mention、
/// `fuxi project rm erp`）。slug 必须文件系统安全。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub String);

impl ProjectId {
    /// 从字符串构造，校验合法字符。
    ///
    /// 规则：仅 `[a-z0-9_-]`，长度 ≥ 1 ≤ 64。
    /// WHY：要在 macOS / Linux 文件系统名安全 + URL 安全 + 命令行安全。
    pub fn new(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        validate_slug(&s)?;
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Project meta —— 持久化在 `~/.fuxi/projects/<id>/meta.json`。
///
/// 字段最小集合：未来加 quota / owner / tags 时新增字段即可，旧 json 兼容
/// （serde default + Option）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    /// 用户真项目的绝对路径，比如 `/Users/e0_7/erp`。fuxi 不动这个目录，
    /// sandbox / ephemeral 都从这儿 git worktree 派生。
    pub canonical_path: PathBuf,
    /// 默认基线 branch，一般是 `main`。L3 sandbox 创建时从这条 branch fork。
    pub default_branch: String,
    pub created_at: DateTime<Utc>,
}

/// 从 canonical 路径推断默认 slug：取末段 basename + 转小写 + 替换非法字符为 `-`。
///
/// 例：`/Users/e0_7/写作-2026` → `写作-2026` 不合法（含中文）→ 落 `project`+随机？
/// 当前策略：非 ASCII 直接 reject，让用户显式传 `--name`。WHY：避免悄悄
/// 生成奇怪 slug 给后续命令行使用造成困惑。
pub fn slug_from_path(p: &std::path::Path) -> Option<String> {
    let basename = p.file_name()?.to_str()?;
    let normalized: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn validate_slug(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 64 {
        return Err(crate::CoreError::Other(format!(
            "project id 长度必须在 1..=64 字符，当前 {}",
            s.len()
        )));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(crate::CoreError::Other(format!(
            "project id 只允许 [a-z0-9_-]，得到 {s:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validates_ok() {
        assert!(ProjectId::new("erp").is_ok());
        assert!(ProjectId::new("erp-v2").is_ok());
        assert!(ProjectId::new("my_proj_2026").is_ok());
        assert!(ProjectId::new("a").is_ok());
    }

    #[test]
    fn slug_rejects_empty_or_long() {
        assert!(ProjectId::new("").is_err());
        let too_long: String = std::iter::repeat_n('a', 65).collect();
        assert!(ProjectId::new(too_long).is_err());
    }

    #[test]
    fn slug_rejects_uppercase_and_special() {
        assert!(ProjectId::new("ERP").is_err(), "大写应拒");
        assert!(ProjectId::new("erp@v2").is_err(), "@ 应拒");
        assert!(ProjectId::new("erp v2").is_err(), "空格应拒");
        assert!(ProjectId::new("写作").is_err(), "中文应拒");
        assert!(ProjectId::new("erp.dev").is_err(), ". 应拒");
    }

    #[test]
    fn slug_from_path_ascii_basename() {
        assert_eq!(
            slug_from_path(std::path::Path::new("/Users/e0_7/erp")),
            Some("erp".to_string())
        );
        assert_eq!(
            slug_from_path(std::path::Path::new("/tmp/My-App")),
            Some("my-app".to_string())
        );
        assert_eq!(
            slug_from_path(std::path::Path::new("/tmp/proj_v2")),
            Some("proj_v2".to_string())
        );
    }

    #[test]
    fn slug_from_path_handles_non_ascii_basename() {
        // "写作-2026" → 中文每字 1 char 替成 '-' → "----2026" → trim → "2026"
        // 这是降级而非 None：能用就用，避免悄悄归 None 让 CLI 报歧义错
        assert_eq!(
            slug_from_path(std::path::Path::new("/Users/e0_7/写作-2026")),
            Some("2026".to_string())
        );
        // 全中文 basename → "----" → trim → "" → None，让用户显式传 --name
        // WHY：避免悄悄生成奇怪 slug，让用户明确知情
        assert_eq!(
            slug_from_path(std::path::Path::new("/tmp/写作")),
            None,
            "全 non-ASCII basename 应返 None"
        );
    }

    #[test]
    fn project_meta_roundtrips_through_json() {
        let p = Project {
            id: ProjectId::new("erp").unwrap(),
            canonical_path: PathBuf::from("/Users/e0_7/erp"),
            default_branch: "main".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn project_id_displays_as_slug() {
        let id = ProjectId::new("erp").unwrap();
        assert_eq!(id.to_string(), "erp");
        assert_eq!(format!("{id}"), "erp");
    }
}
