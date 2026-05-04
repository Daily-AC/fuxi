//! L2 ephemeral worktree（Decision 21 phase 2）—— per-task 一次性工作区，
//! 任务完后封存（archive 24h），再后被 GC 清理。
//!
//! 与 [`crate::PersistentSandboxManager`]（L3 sandbox）的差别：
//!
//! | 维度 | L3 持久 sandbox | L2 ephemeral worktree |
//! |---|---|---|
//! | 索引键 | (project, role) | (project, task_id) |
//! | 路径 | `<root>/<project>/sandboxes/<role>` | `<root>/<project>/ephemeral/<task>` |
//! | 分支 | `<role>/<project>-main`（长期） | `task/<task-uuid>`（一次性） |
//! | 生命周期 | 长期，跨 task 复用 | 任务完 → archive → 24h 后 GC |
//! | fork 起点 | canonical `<default_branch>` | canonical `<default_branch>`（v1）/ L3 sandbox（phase 3） |
//!
//! 本模块只管文件系统 + git 操作；事件发布（`WorkspaceCreated{layer=L2}` /
//! `WorkspaceArchived` / `WorkspaceCollected`）由 caller 在调用前后包装——
//! 跟 PersistentSandboxManager 同款约束（fuxi-workspace 不依赖 fuxi-events）。

use chrono::{DateTime, Duration, Utc};
use fuxi_core::{Project, ProjectId, TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::WorkspaceError;

/// L2 ephemeral worktree handle——caller 创建后拿到，destroy 不动 git（本管理器
/// 自己封存 / 清理 git 资源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EphemeralWorkspaceHandle {
    pub project: ProjectId,
    pub task: TaskId,
    pub canonical_path: PathBuf,
    /// `<root>/<project>/ephemeral/<task-display>/` 绝对路径。
    pub workspace_path: PathBuf,
    /// `task/<task-uuid>` —— 一次性 branch。
    pub branch: String,
    /// 跨事件关联键：`<project>/L2/<task-uuid>`。
    pub workspace_id: WorkspaceId,
}

/// archive/ 目录里每个 task 子目录的 metadata——记录何时被 archive 的，
/// 让 GC 知道是否过期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveMeta {
    pub archived_at: DateTime<Utc>,
    /// 原 ephemeral path（debug + 复活时定位用，但 v1 不实装"复活"）。
    pub original_path: PathBuf,
    pub branch: String,
}

const ARCHIVE_META_FILENAME: &str = "fuxi-archive-meta.json";

#[derive(Debug, Clone)]
pub struct EphemeralWorkspaceManager {
    project: Project,
    /// `<projects_root>/<project>/ephemeral/`
    ephemeral_root: PathBuf,
    /// `<projects_root>/<project>/archive/`
    archive_root: PathBuf,
}

impl EphemeralWorkspaceManager {
    pub fn new(project: Project, projects_root: &Path) -> Self {
        let ephemeral_root = projects_root.join(project.id.as_str()).join("ephemeral");
        let archive_root = projects_root.join(project.id.as_str()).join("archive");
        Self {
            project,
            ephemeral_root,
            archive_root,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn ephemeral_root(&self) -> &Path {
        &self.ephemeral_root
    }

    pub fn archive_root(&self) -> &Path {
        &self.archive_root
    }

    /// 给一个 task 起新的 L2 worktree。
    ///
    /// 路径：`ephemeral_root/<task-display>`，分支：`task/<task-uuid>`，
    /// fork 自 canonical 的 `default_branch`。同 task 重复 create → AlreadyExists。
    pub async fn create(&self, task: TaskId) -> Result<EphemeralWorkspaceHandle, WorkspaceError> {
        let workspace_path = self.path_for(task);
        if workspace_path.exists() {
            return Err(WorkspaceError::AlreadyExists(workspace_path));
        }
        tokio::fs::create_dir_all(&self.ephemeral_root).await?;
        let workspace_path_str = workspace_path
            .to_str()
            .ok_or_else(|| {
                WorkspaceError::Other(format!(
                    "ephemeral path not valid utf-8: {}",
                    workspace_path.display()
                ))
            })?
            .to_string();
        let branch = format!("task/{}", task.0);

        info!(
            project = %self.project.id,
            %task,
            path = workspace_path_str,
            branch = %branch,
            "创建 L2 ephemeral worktree"
        );

        let combined = run_git(
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &workspace_path_str,
                &self.project.default_branch,
            ],
            &self.project.canonical_path,
        )
        .await;
        if let Err(WorkspaceError::Git { stderr, .. }) = &combined {
            let s = stderr.to_lowercase();
            if s.contains("already exists") || s.contains("already used") {
                warn!(
                    project = %self.project.id,
                    %task,
                    "task branch 已存在，fallback 到 attach"
                );
                run_git(
                    &["worktree", "add", &workspace_path_str, &branch],
                    &self.project.canonical_path,
                )
                .await?;
            } else {
                return Err(combined.err().unwrap());
            }
        } else {
            // bug #77 CI rust 1.95 clippy::question_mark：单 if-let-Err 早返写 `?` 更 idiomatic
            combined?;
        }

        Ok(EphemeralWorkspaceHandle {
            project: self.project.id.clone(),
            task,
            canonical_path: self.project.canonical_path.clone(),
            workspace_path,
            branch,
            workspace_id: WorkspaceId::l2(&self.project.id, task),
        })
    }

    /// 列当前 active（在 ephemeral/ 下没归档的）worktree。
    pub async fn list_active(&self) -> Result<Vec<EphemeralWorkspaceHandle>, WorkspaceError> {
        if !self.ephemeral_root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.ephemeral_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.path().is_dir() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            // task display 形态 `task-<uuid>`——也兼容裸 uuid
            let trimmed = name.strip_prefix("task-").unwrap_or(&name);
            let task_uuid = match uuid::Uuid::parse_str(trimmed) {
                Ok(u) => TaskId::from(u),
                Err(_) => continue,
            };
            out.push(EphemeralWorkspaceHandle {
                project: self.project.id.clone(),
                task: task_uuid,
                canonical_path: self.project.canonical_path.clone(),
                workspace_path: entry.path(),
                branch: format!("task/{}", task_uuid.0),
                workspace_id: WorkspaceId::l2(&self.project.id, task_uuid),
            });
        }
        out.sort_by_key(|h| h.task.0);
        Ok(out)
    }

    /// 把 active worktree 归档到 archive/，写 metadata。
    ///
    /// 行为：
    /// - 若 ephemeral/<task>/ 存在 → 移到 archive/<task>/，写 ArchiveMeta
    /// - 若已不存在（被人手动删 / 已 archive）→ silent Ok（幂等）
    /// - 不动 git branch（task/<uuid> branch 仍存在；GC 时一起删）
    pub async fn archive(&self, task: TaskId) -> Result<(), WorkspaceError> {
        let active_path = self.path_for(task);
        if !active_path.exists() {
            debug!(
                project = %self.project.id,
                %task,
                "archive: ephemeral path 不存在，幂等 noop"
            );
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.archive_root).await?;
        let archive_path = self.archive_path_for(task);
        if archive_path.exists() {
            // 同 task 已 archived（罕见——按理不会重复 archive）。安全做法：先删
            // 旧的 archive，再 move。但 silent rm 可能掩盖 bug；保守先报错。
            return Err(WorkspaceError::AlreadyExists(archive_path));
        }
        info!(
            project = %self.project.id,
            %task,
            from = %active_path.display(),
            to = %archive_path.display(),
            "archive L2 worktree"
        );
        tokio::fs::rename(&active_path, &archive_path).await?;

        // 写 ArchiveMeta
        let meta = ArchiveMeta {
            archived_at: Utc::now(),
            original_path: active_path,
            branch: format!("task/{}", task.0),
        };
        let json = serde_json::to_string_pretty(&meta)
            .map_err(|e| WorkspaceError::Other(format!("ArchiveMeta 序列化失败: {e}")))?;
        tokio::fs::write(archive_path.join(ARCHIVE_META_FILENAME), json).await?;

        // git worktree prune——告诉 git 旧路径已经移走
        // （worktree 数据其实在 .git/worktrees/<name>/ 下，move 物理目录后
        //  git 仍指向旧路径，不 prune 会让 git worktree list 显示 "missing"）
        let _ = run_git(&["worktree", "prune"], &self.project.canonical_path).await;
        Ok(())
    }

    /// 列归档区里的所有 task。
    pub async fn list_archived(&self) -> Result<Vec<(TaskId, ArchiveMeta)>, WorkspaceError> {
        if !self.archive_root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.archive_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.path().is_dir() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let trimmed = name.strip_prefix("task-").unwrap_or(&name);
            let task = match uuid::Uuid::parse_str(trimmed) {
                Ok(u) => TaskId::from(u),
                Err(_) => continue,
            };
            let meta_path = entry.path().join(ARCHIVE_META_FILENAME);
            if !meta_path.exists() {
                continue;
            }
            let bytes = tokio::fs::read(&meta_path).await?;
            let meta: ArchiveMeta = match serde_json::from_slice(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        path = %meta_path.display(),
                        error = %e,
                        "skip archive entry with malformed meta"
                    );
                    continue;
                }
            };
            out.push((task, meta));
        }
        out.sort_by_key(|(t, _)| t.0);
        Ok(out)
    }

    /// GC：删除归档区里 archived_at 早于 (now - older_than) 的条目。
    ///
    /// 返回被删除的 (task_id, ArchiveMeta) 列表，让 caller 发
    /// `WorkspaceCollected` 事件。
    pub async fn collect_expired(
        &self,
        older_than: Duration,
    ) -> Result<Vec<(TaskId, ArchiveMeta)>, WorkspaceError> {
        let archived = self.list_archived().await?;
        let cutoff = Utc::now() - older_than;
        let mut collected = Vec::new();
        for (task, meta) in archived {
            if meta.archived_at >= cutoff {
                continue;
            }
            let archive_path = self.archive_path_for(task);
            info!(
                project = %self.project.id,
                %task,
                archived_at = %meta.archived_at,
                path = %archive_path.display(),
                "GC L2 archive 过期 → 删"
            );
            // 物理删整个目录
            if let Err(e) = tokio::fs::remove_dir_all(&archive_path).await {
                warn!(
                    project = %self.project.id,
                    %task,
                    error = %e,
                    "GC remove_dir_all 失败，跳过本条"
                );
                continue;
            }
            // 删 task/<uuid> branch（已无 worktree 持有；幂等失败 silent）
            let _ = run_git(
                &["branch", "-D", &meta.branch],
                &self.project.canonical_path,
            )
            .await;
            collected.push((task, meta));
        }
        Ok(collected)
    }

    /// 把 active L2 ephemeral 提升为 L3 持久 sandbox（Decision 21 决策树第 5 条）。
    ///
    /// 行为：
    /// - 物理 move ephemeral/<task>/ → sandboxes/<role>/
    /// - rename branch `task/<uuid>` → `<role>/<project>-main`
    /// - prune git 元数据
    /// - 失败前置校验：L2 必须存在；同 role 的 L3 已存在 → 拒（避免覆盖既有 sandbox）
    pub async fn promote_to_l3(&self, task: TaskId, role: &str) -> Result<(), WorkspaceError> {
        if role.is_empty() {
            return Err(WorkspaceError::Other("role 不能空".into()));
        }
        let active_path = self.path_for(task);
        if !active_path.exists() {
            return Err(WorkspaceError::Other(format!(
                "L2 ephemeral 不存在: task-{}",
                task.0
            )));
        }
        let sandboxes_root = self
            .ephemeral_root
            .parent()
            .map(|p| p.join("sandboxes"))
            .ok_or_else(|| WorkspaceError::Other("ephemeral_root 无父级".into()))?;
        let target_path = sandboxes_root.join(role);
        if target_path.exists() {
            return Err(WorkspaceError::AlreadyExists(target_path));
        }
        tokio::fs::create_dir_all(&sandboxes_root).await?;

        let new_branch = format!("{role}/{}-main", self.project.id.as_str());
        let old_branch = format!("task/{}", task.0);
        info!(
            project = %self.project.id,
            %task,
            role,
            from = %active_path.display(),
            to = %target_path.display(),
            new_branch = %new_branch,
            "promote L2 → L3"
        );

        // git branch rename 必须在 worktree move 之前——branch -m 旧名 新名
        // 不依赖 worktree 路径；rename 后再 fs::rename 物理目录。git worktree
        // prune 兜底元数据。
        if let Err(e) = run_git(
            &["branch", "-m", &old_branch, &new_branch],
            &self.project.canonical_path,
        )
        .await
        {
            warn!(error = %e, %old_branch, %new_branch, "branch rename 失败，promote 中止");
            return Err(e);
        }
        tokio::fs::rename(&active_path, &target_path).await?;
        let _ = run_git(&["worktree", "prune"], &self.project.canonical_path).await;
        Ok(())
    }

    fn path_for(&self, task: TaskId) -> PathBuf {
        self.ephemeral_root.join(task.to_string())
    }

    fn archive_path_for(&self, task: TaskId) -> PathBuf {
        self.archive_root.join(task.to_string())
    }
}

async fn run_git(args: &[&str], cwd: &Path) -> Result<String, WorkspaceError> {
    debug!(?args, cwd = %cwd.display(), "git invocation");
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        warn!(?args, stderr = %stderr, "git command failed");
        return Err(WorkspaceError::Git {
            command: format!("git {}", args.join(" ")),
            stderr,
        });
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn make_git_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await
                .unwrap();
        }
        tokio::fs::write(path.join("README.md"), "seed")
            .await
            .unwrap();
        let _ = Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await;
        let _ = Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "seed"])
            .output()
            .await;
        (dir, path)
    }

    fn make_project(canonical: PathBuf) -> Project {
        Project {
            id: ProjectId::new("erp").unwrap(),
            canonical_path: canonical,
            default_branch: "main".to_string(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_makes_worktree_with_task_branch() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());

        let task = TaskId::new();
        let h = mgr.create(task).await.expect("create");

        assert_eq!(h.task, task);
        assert_eq!(h.branch, format!("task/{}", task.0));
        assert!(h.workspace_id.as_str().starts_with("erp/L2/"));
        assert!(h.workspace_path.is_dir());
        assert!(h.workspace_path.join(".git").exists());
        assert!(h.workspace_path.join("README.md").is_file());
    }

    #[tokio::test]
    async fn create_rejects_duplicate_task() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());
        let task = TaskId::new();
        mgr.create(task).await.unwrap();
        let err = mgr.create(task).await.expect_err("dup 应失败");
        assert!(matches!(err, WorkspaceError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn list_active_returns_existing() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());
        let t1 = TaskId::new();
        let t2 = TaskId::new();
        mgr.create(t1).await.unwrap();
        mgr.create(t2).await.unwrap();

        let listed = mgr.list_active().await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[tokio::test]
    async fn archive_moves_active_to_archive_with_meta() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());
        let task = TaskId::new();
        let h = mgr.create(task).await.unwrap();

        mgr.archive(task).await.unwrap();
        assert!(!h.workspace_path.exists(), "active 路径应已 move 走");
        let archive_path = mgr.archive_root().join(task.to_string());
        assert!(archive_path.is_dir(), "archive 路径应有");
        assert!(archive_path.join(ARCHIVE_META_FILENAME).is_file());

        // list_active 现在应空，list_archived 该见
        assert!(mgr.list_active().await.unwrap().is_empty());
        let archived = mgr.list_archived().await.unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].0, task);
        assert_eq!(archived[0].1.branch, format!("task/{}", task.0));
    }

    #[tokio::test]
    async fn archive_noop_when_missing() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());
        let task = TaskId::new();
        // 没 create，直接 archive 应 ok
        mgr.archive(task).await.unwrap();
    }

    #[tokio::test]
    async fn collect_expired_only_removes_old_archives() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());

        // 起 + archive 两个 task
        let old_task = TaskId::new();
        let new_task = TaskId::new();
        mgr.create(old_task).await.unwrap();
        mgr.create(new_task).await.unwrap();
        mgr.archive(old_task).await.unwrap();
        mgr.archive(new_task).await.unwrap();

        // 手工把 old_task 的 archived_at 改为很早——模拟它过期
        let old_meta_path = mgr
            .archive_root()
            .join(old_task.to_string())
            .join(ARCHIVE_META_FILENAME);
        let mut old_meta: ArchiveMeta =
            serde_json::from_slice(&tokio::fs::read(&old_meta_path).await.unwrap()).unwrap();
        old_meta.archived_at = Utc::now() - Duration::hours(48);
        tokio::fs::write(
            &old_meta_path,
            serde_json::to_string_pretty(&old_meta).unwrap(),
        )
        .await
        .unwrap();

        // collect with 24h threshold → 应只删 old_task
        let collected = mgr.collect_expired(Duration::hours(24)).await.unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, old_task);

        let remaining = mgr.list_archived().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, new_task);
    }

    #[tokio::test]
    async fn promote_to_l3_moves_path_and_renames_branch() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project.clone(), projects_root.path());

        let task = TaskId::new();
        let h = mgr.create(task).await.unwrap();
        // 改一行让 sandbox 有内容
        tokio::fs::write(h.workspace_path.join("WIP.md"), "在做")
            .await
            .unwrap();

        mgr.promote_to_l3(task, "luban").await.expect("promote");

        // 旧路径应消失
        assert!(!h.workspace_path.exists());
        // 新路径应有
        let new_path = projects_root
            .path()
            .join("erp")
            .join("sandboxes")
            .join("luban");
        assert!(new_path.is_dir());
        assert!(new_path.join("WIP.md").is_file(), "WIP 应保留");

        // branch list 验：task/ 没了，luban/erp-main 在
        let out = Command::new("git")
            .current_dir(&project.canonical_path)
            .args(["branch", "--list"])
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("luban/erp-main"), "got: {stdout}");
        assert!(
            !stdout.contains(&format!("task/{}", task.0)),
            "old task branch 应没了"
        );
    }

    #[tokio::test]
    async fn promote_rejects_when_l3_already_exists() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project.clone(), projects_root.path());

        // 起 L3 sandbox 占位
        let l3_path = projects_root
            .path()
            .join("erp")
            .join("sandboxes")
            .join("luban");
        tokio::fs::create_dir_all(&l3_path).await.unwrap();

        let task = TaskId::new();
        mgr.create(task).await.unwrap();
        let err = mgr
            .promote_to_l3(task, "luban")
            .await
            .expect_err("L3 已存在应拒");
        assert!(matches!(err, WorkspaceError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn list_archived_skips_malformed_meta() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = EphemeralWorkspaceManager::new(project, projects_root.path());

        // 手工塞个坏 archive 目录
        let bad_path = mgr
            .archive_root()
            .join(format!("task-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&bad_path).await.unwrap();
        tokio::fs::write(bad_path.join(ARCHIVE_META_FILENAME), "{not json")
            .await
            .unwrap();

        let listed = mgr.list_archived().await.unwrap();
        assert!(listed.is_empty(), "坏 meta 应跳过");
    }
}
