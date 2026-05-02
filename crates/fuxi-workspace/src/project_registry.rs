//! 文件系统持久化的 Project 注册表（Decision 21 phase 1）。
//!
//! 落盘约定：
//! - 注册表根：默认 `$HOME/.fuxi/projects/`，可由 `FileSystemProjectRegistry::new`
//!   覆写（测试用 tempdir）
//! - 单 project：`<root>/<id>/meta.json`，并预创 `sandboxes/` `ephemeral/`
//!   `archive/` `deliverables/` 四个空子目录（占位，后续 phase 用）
//!
//! `add()` 校验：canonical 存在 + 是 git repo（含 `.git` 目录或 `.git` worktree
//! 文件）；同 id 已存在则拒。

use chrono::Utc;
use fuxi_core::{Project, ProjectId, slug_from_path};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::WorkspaceError;

const META_FILENAME: &str = "meta.json";
const SUBDIRS: &[&str] = &["sandboxes", "ephemeral", "archive", "deliverables"];

/// 文件系统持久化的 ProjectRegistry。
///
/// WHY：用 trait 是 overkill——v1 只一种实现，后续要 mock 直接构 tempdir 实例
/// 即可。等真出现第二种实现（云端同步？）再抽 trait 不迟。
#[derive(Debug, Clone)]
pub struct FileSystemProjectRegistry {
    root: PathBuf,
}

impl FileSystemProjectRegistry {
    /// 用指定 root 构造（测试 / 替代部署用）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 用 `$HOME/.fuxi/projects/` 默认 root 构造。无 `$HOME` → Err。
    pub fn with_default_root() -> Result<Self, WorkspaceError> {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            WorkspaceError::Other("$HOME 未设置，无法定位默认 project 注册表根".into())
        })?;
        Ok(Self::new(PathBuf::from(home).join(".fuxi").join("projects")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 注册一个 project：校验 canonical + 派生 / 校验 id + 写 meta.json + 建子目录骨架。
    ///
    /// `name` 为 `None` 时从 canonical 末段 basename 派生 slug；非 ASCII basename
    /// 派生失败时返 Err 让用户显式传 name（避免悄悄编造）。
    pub async fn add(
        &self,
        canonical_path: PathBuf,
        name: Option<String>,
        default_branch: Option<String>,
    ) -> Result<Project, WorkspaceError> {
        // 1. canonical 校验
        let canonical = canonical_path
            .canonicalize()
            .map_err(|e| WorkspaceError::Other(format!("canonical 路径无法解析: {e}")))?;
        if !canonical.is_dir() {
            return Err(WorkspaceError::Other(format!(
                "canonical 不是目录: {}",
                canonical.display()
            )));
        }
        if !is_git_repo(&canonical).await {
            return Err(WorkspaceError::NotAGitRepo(canonical));
        }

        // 2. id 派生 / 校验
        let id = match name {
            Some(s) => ProjectId::new(s).map_err(|e| WorkspaceError::Other(e.to_string()))?,
            None => {
                let slug = slug_from_path(&canonical).ok_or_else(|| {
                    WorkspaceError::Other(format!(
                        "无法从 {} 派生 slug——请显式传 --name",
                        canonical.display()
                    ))
                })?;
                ProjectId::new(slug).map_err(|e| WorkspaceError::Other(e.to_string()))?
            }
        };

        // 3. 唯一性
        let proj_dir = self.root.join(id.as_str());
        if proj_dir.exists() {
            return Err(WorkspaceError::AlreadyExists(proj_dir));
        }

        // 4. 写盘
        fs::create_dir_all(&proj_dir).await?;
        for sub in SUBDIRS {
            fs::create_dir_all(proj_dir.join(sub)).await?;
        }
        let project = Project {
            id: id.clone(),
            canonical_path: canonical,
            default_branch: default_branch.unwrap_or_else(|| "main".to_string()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string_pretty(&project)
            .map_err(|e| WorkspaceError::Other(format!("meta.json 序列化失败: {e}")))?;
        fs::write(proj_dir.join(META_FILENAME), json).await?;

        Ok(project)
    }

    /// 列出所有已注册 project。
    ///
    /// 漏掉损坏 meta 的 project（log 警告但不 fail），避免单条脏数据搞瘫整个列表。
    pub async fn list(&self) -> Result<Vec<Project>, WorkspaceError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join(META_FILENAME);
            if !meta_path.exists() {
                continue;
            }
            match load_meta(&meta_path).await {
                Ok(p) => out.push(p),
                Err(e) => {
                    tracing::warn!(
                        "skip malformed project meta {}: {e}",
                        meta_path.display()
                    );
                }
            }
        }
        out.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(out)
    }

    /// 按 id 拿单条 project。不存在 → Ok(None)。
    pub async fn get(&self, id: &ProjectId) -> Result<Option<Project>, WorkspaceError> {
        let meta_path = self.root.join(id.as_str()).join(META_FILENAME);
        if !meta_path.exists() {
            return Ok(None);
        }
        Ok(Some(load_meta(&meta_path).await?))
    }

    /// 删 project：递归删 `<root>/<id>/`。
    ///
    /// **注意**：会一起删掉 sandboxes / ephemeral / archive / deliverables——
    /// 这是 destructive 操作，调用方负责确认。phase 1 不做"非空就拒"逻辑，
    /// 后续若用户实测踩到再加。
    pub async fn remove(&self, id: &ProjectId) -> Result<(), WorkspaceError> {
        let proj_dir = self.root.join(id.as_str());
        if !proj_dir.exists() {
            return Ok(());
        }
        fs::remove_dir_all(&proj_dir).await?;
        Ok(())
    }
}

async fn load_meta(path: &Path) -> Result<Project, WorkspaceError> {
    let bytes = fs::read(path).await?;
    let project: Project = serde_json::from_slice(&bytes)
        .map_err(|e| WorkspaceError::Other(format!("meta.json 解析失败: {e}")))?;
    Ok(project)
}

/// 一个目录是不是 git repo：含 `.git`（不管是 dir 还是 worktree marker file）。
async fn is_git_repo(path: &Path) -> bool {
    fs::metadata(path.join(".git")).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 在 tempdir 里建一个最小 git repo（用 `git init -q -b main` + 一条 seed commit）。
    async fn make_git_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let out = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        }
        tokio::fs::write(path.join("README.md"), "seed")
            .await
            .unwrap();
        let out = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await
            .expect("git add");
        assert!(out.status.success());
        let out = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "seed"])
            .output()
            .await
            .expect("git commit");
        assert!(out.status.success());
        (dir, path)
    }

    #[tokio::test]
    async fn add_with_inferred_slug_works() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_repo_td, repo) = make_git_repo().await;
        // tempdir basename 一般是 random hex chars——slug_from_path 能 normalize
        let project = registry
            .add(repo.clone(), None, None)
            .await
            .expect("add");

        // slug 派生符合规则
        assert!(
            !project.id.as_str().is_empty(),
            "id 应该派生出来: {}",
            project.id
        );
        // 跟 canonicalize 后的 path 一致（macOS /var → /private/var 处理）
        assert_eq!(project.canonical_path, repo.canonicalize().unwrap());
        assert_eq!(project.default_branch, "main");

        // 物理存在
        assert!(
            registry_root
                .path()
                .join(project.id.as_str())
                .join(META_FILENAME)
                .exists()
        );
        // 子目录骨架建好
        for sub in SUBDIRS {
            assert!(
                registry_root.path().join(project.id.as_str()).join(sub).is_dir(),
                "subdir {sub} 应预创"
            );
        }
    }

    #[tokio::test]
    async fn add_with_explicit_name_overrides_inferred() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_repo_td, repo) = make_git_repo().await;
        let project = registry
            .add(repo, Some("my-erp".into()), None)
            .await
            .expect("add");

        assert_eq!(project.id.as_str(), "my-erp");
    }

    #[tokio::test]
    async fn add_rejects_duplicate_id() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_td1, repo1) = make_git_repo().await;
        let (_td2, repo2) = make_git_repo().await;

        registry
            .add(repo1, Some("dup".into()), None)
            .await
            .expect("first add");
        let err = registry
            .add(repo2, Some("dup".into()), None)
            .await
            .expect_err("second add 应失败");
        assert!(
            matches!(err, WorkspaceError::AlreadyExists(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn add_rejects_non_git_dir() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let non_git = tempfile::tempdir().unwrap();
        let err = registry
            .add(non_git.path().to_path_buf(), Some("x".into()), None)
            .await
            .expect_err("非 git 目录应拒");
        assert!(
            matches!(err, WorkspaceError::NotAGitRepo(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn add_rejects_invalid_slug() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_td, repo) = make_git_repo().await;

        let err = registry
            .add(repo, Some("ERP".into()), None)
            .await
            .expect_err("大写 slug 应拒");
        assert!(matches!(err, WorkspaceError::Other(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn list_returns_added_projects_sorted() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_t1, r1) = make_git_repo().await;
        let (_t2, r2) = make_git_repo().await;
        let (_t3, r3) = make_git_repo().await;
        registry
            .add(r1, Some("zebra".into()), None)
            .await
            .unwrap();
        registry
            .add(r2, Some("alpha".into()), None)
            .await
            .unwrap();
        registry
            .add(r3, Some("middle".into()), None)
            .await
            .unwrap();

        let listed = registry.list().await.unwrap();
        let ids: Vec<_> = listed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "middle", "zebra"]);
    }

    #[tokio::test]
    async fn list_skips_malformed_meta() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_td, repo) = make_git_repo().await;
        registry
            .add(repo, Some("good".into()), None)
            .await
            .unwrap();

        // 手工在 root 里塞一个坏 meta 目录
        let bad_dir = registry_root.path().join("bad");
        fs::create_dir_all(&bad_dir).await.unwrap();
        fs::write(bad_dir.join(META_FILENAME), "{not json").await.unwrap();

        let listed = registry.list().await.unwrap();
        let ids: Vec<_> = listed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["good"], "坏 meta 应被跳过，good 仍在列");
    }

    #[tokio::test]
    async fn list_empty_when_no_root() {
        // 故意指向不存在的 root——不该 fail，返空列表
        let nope = tempfile::tempdir().unwrap();
        let registry =
            FileSystemProjectRegistry::new(nope.path().join("nonexistent"));
        let listed = registry.list().await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn get_returns_none_when_missing() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let id = ProjectId::new("nope").unwrap();
        assert!(registry.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_returns_some_after_add() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_td, repo) = make_git_repo().await;
        let added = registry
            .add(repo, Some("erp".into()), None)
            .await
            .unwrap();
        let fetched = registry
            .get(&added.id)
            .await
            .unwrap()
            .expect("get should find it");
        assert_eq!(fetched, added);
    }

    #[tokio::test]
    async fn remove_deletes_project_dir() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        let (_td, repo) = make_git_repo().await;
        let added = registry
            .add(repo, Some("doomed".into()), None)
            .await
            .unwrap();

        let proj_dir = registry_root.path().join(added.id.as_str());
        assert!(proj_dir.exists());

        registry.remove(&added.id).await.unwrap();
        assert!(!proj_dir.exists());
        assert!(registry.get(&added.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn remove_noop_when_missing() {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let id = ProjectId::new("ghost").unwrap();
        // 不该 fail
        registry.remove(&id).await.unwrap();
    }
}
