//! L3 持久 sandbox（Decision 21 phase 1）—— per-门客 per-project 长期 git
//! worktree，跨任务保留 build cache + 未完 WIP。
//!
//! 与现有 `GitWorktreeWorkspace` 区别：
//! - 索引键：(project, role)，**不是** agent-id（同 role 跨任务复用）
//! - 落地路径：`<projects_root>/<project>/sandboxes/<role>/`
//! - 分支命名：`<role>/<project>-main`
//! - 生命周期：长期，不跟 agent / task 生死
//! - get-or-create 语义：第二次调用同 (project, role) 返回已有 handle，不报错
//!
//! 本模块**只**负责 git worktree 物理操作 + handle 暴露，不发 EventBus 事件
//! ——事件由 orchestrator 在调用本模块前后包装发出（保持 workspace crate 纯
//! file-system / git，不依赖 events crate）。

use fuxi_core::{Project, ProjectId, WorkspaceId};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::WorkspaceError;

/// L3 持久 sandbox 的句柄。
///
/// 与 `WorkspaceHandle`（agent-keyed）区别：本句柄按 (project, role) 索引，
/// 不绑 AgentId——同一个 role 的不同 task 共用同一 sandbox。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentSandboxHandle {
    pub project: ProjectId,
    pub role: String,
    /// 用户的 canonical repo 路径。
    pub canonical_path: PathBuf,
    /// `<projects_root>/<project>/sandboxes/<role>/`。
    pub sandbox_path: PathBuf,
    /// `<role>/<project>-main`。
    pub branch: String,
    /// 跨事件关联键：`<project>/L3/<role>`。
    pub workspace_id: WorkspaceId,
}

/// L3 sandbox 管理器（per-project）。
///
/// 一个 Project 对应一个 manager 实例；rentrant `get_or_create` 给该 project
/// 的多个 role 使用。manager 本身不持 mutable 状态——所有真相在文件系统 + git。
#[derive(Debug, Clone)]
pub struct PersistentSandboxManager {
    project: Project,
    /// `<projects_root>/<project>/sandboxes/`
    sandboxes_root: PathBuf,
}

impl PersistentSandboxManager {
    /// 从 project 和 projects_root 构造。
    /// 约定：sandboxes_root = `<projects_root>/<project_id>/sandboxes`。
    pub fn new(project: Project, projects_root: &Path) -> Self {
        let sandboxes_root = projects_root.join(project.id.as_str()).join("sandboxes");
        Self {
            project,
            sandboxes_root,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn sandboxes_root(&self) -> &Path {
        &self.sandboxes_root
    }

    /// 拿这个 role 的 sandbox handle；不存在就建。
    ///
    /// 第一次调用：`git worktree add -b <branch> <sandbox_path> <default_branch>`。
    /// 重复调用：返回已有 handle（沿 path 推断 branch，不重新跑 git）。
    pub async fn get_or_create(
        &self,
        role: &str,
    ) -> Result<PersistentSandboxHandle, WorkspaceError> {
        validate_role(role)?;

        let sandbox_path = self.sandboxes_root.join(role);
        let branch = branch_name(role, &self.project.id);
        let workspace_id = WorkspaceId::l3(&self.project.id, role);

        // 已存在 → 当作"已有 sandbox"返回（idempotent）
        // WHY：sandbox 的本质是 git worktree，物理目录是其唯一真相。第二次
        // get_or_create 不该重跑 `git worktree add`（会报 "already exists"），
        // 也不该删了重建（丢 build cache + WIP）。
        if sandbox_path.exists() {
            debug!(
                project = %self.project.id,
                role,
                path = %sandbox_path.display(),
                "持久 sandbox 已存在，复用"
            );
            return Ok(PersistentSandboxHandle {
                project: self.project.id.clone(),
                role: role.to_string(),
                canonical_path: self.project.canonical_path.clone(),
                sandbox_path,
                branch,
                workspace_id,
            });
        }

        // 不存在 → 建。先 mkdir parent，再 git worktree add
        tokio::fs::create_dir_all(&self.sandboxes_root).await?;

        let sandbox_path_str = sandbox_path
            .to_str()
            .ok_or_else(|| {
                WorkspaceError::Other(format!(
                    "sandbox path not valid utf-8: {}",
                    sandbox_path.display()
                ))
            })?
            .to_string();

        info!(
            project = %self.project.id,
            role,
            path = sandbox_path_str,
            branch = %branch,
            "创建 L3 持久 sandbox"
        );

        // 首选：`git worktree add -b <branch> <sandbox> <default-branch>`
        // 若 branch 已存在（前次 retire 没删干净 / 老 sandbox 残留），fallback
        // 到 `git worktree add <sandbox> <branch>`（attach 已有 branch）。
        let combined = run_git(
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &sandbox_path_str,
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
                    role,
                    "branch 已存在，fallback 到 attach 模式"
                );
                run_git(
                    &["worktree", "add", &sandbox_path_str, &branch],
                    &self.project.canonical_path,
                )
                .await?;
            } else {
                return Err(combined.err().unwrap());
            }
        } else {
            // bug #77 CI rust 1.95 clippy::question_mark
            combined?;
        }

        Ok(PersistentSandboxHandle {
            project: self.project.id.clone(),
            role: role.to_string(),
            canonical_path: self.project.canonical_path.clone(),
            sandbox_path,
            branch,
            workspace_id,
        })
    }

    /// 列出当前 project 的所有 L3 sandbox（按 role 字典序）。
    ///
    /// 走文件系统扫 `sandboxes_root` 下子目录——子目录就是 role。不走 `git
    /// worktree list` 因为 git 视角是 path，反推 role 还要解 sandboxes_root
    /// 前缀，多绕一步。
    pub async fn list(&self) -> Result<Vec<PersistentSandboxHandle>, WorkspaceError> {
        if !self.sandboxes_root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.sandboxes_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let role = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue, // 非 utf8 role 名跳过
            };
            // 校验 role 合法性——历史脏数据跳过避免污染
            if validate_role(&role).is_err() {
                warn!(role, "skip sandbox dir with invalid role name");
                continue;
            }
            out.push(PersistentSandboxHandle {
                project: self.project.id.clone(),
                role: role.clone(),
                canonical_path: self.project.canonical_path.clone(),
                sandbox_path: path,
                branch: branch_name(&role, &self.project.id),
                workspace_id: WorkspaceId::l3(&self.project.id, &role),
            });
        }
        out.sort_by(|a, b| a.role.cmp(&b.role));
        Ok(out)
    }

    /// 退役一个 sandbox：移除 git worktree + 删 branch。
    ///
    /// **destructive**：未 commit 的 WIP 会丢。调用方负责确认（PWA 显式按钮）。
    /// 不存在 → Ok（idempotent）。
    pub async fn retire(&self, role: &str) -> Result<(), WorkspaceError> {
        validate_role(role)?;
        let sandbox_path = self.sandboxes_root.join(role);
        if !sandbox_path.exists() {
            return Ok(());
        }
        let sandbox_path_str = sandbox_path
            .to_str()
            .ok_or_else(|| {
                WorkspaceError::Other(format!(
                    "sandbox path not valid utf-8: {}",
                    sandbox_path.display()
                ))
            })?
            .to_string();
        let branch = branch_name(role, &self.project.id);

        info!(
            project = %self.project.id,
            role,
            path = sandbox_path_str,
            branch = %branch,
            "retire L3 持久 sandbox"
        );

        // git worktree remove --force（dirty 也强删，retire 是显式 destructive）
        let remove = run_git(
            &["worktree", "remove", "--force", &sandbox_path_str],
            &self.project.canonical_path,
        )
        .await;
        if let Err(WorkspaceError::Git { stderr, .. }) = &remove {
            let s = stderr.to_lowercase();
            // 幂等：worktree 已被 prune / 不存在
            let already_gone = s.contains("not a working tree")
                || s.contains("does not exist")
                || s.contains("no such");
            if !already_gone {
                return Err(remove.err().unwrap());
            }
            // 兜底 prune
            let _ = run_git(&["worktree", "prune"], &self.project.canonical_path).await;
        } else {
            // bug #77 CI rust 1.95 clippy::question_mark
            remove?;
        }

        // 删 branch（失败不致命——branch 可能从未建成）
        if let Err(err) = run_git(&["branch", "-D", &branch], &self.project.canonical_path).await {
            debug!(?err, branch = %branch, "branch -D skipped");
        }

        Ok(())
    }
}

fn branch_name(role: &str, project: &ProjectId) -> String {
    format!("{}/{}-main", role, project.as_str())
}

/// role 名校验：跟 ProjectId 同规则（`[a-z0-9_-]`，1..=64 字符）。
/// WHY：role 要拼进 git branch 名（`<role>/<project>-main`）和 sandbox 路径——
/// 反斜杠 / 空格 / shell 特殊字符都会爆炸。
fn validate_role(role: &str) -> Result<(), WorkspaceError> {
    if role.is_empty() || role.len() > 64 {
        return Err(WorkspaceError::Other(format!(
            "role 长度必须在 1..=64 字符，当前 {}",
            role.len()
        )));
    }
    if !role
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(WorkspaceError::Other(format!(
            "role 只允许 [a-z0-9_-]，得到 {role:?}"
        )));
    }
    Ok(())
}

/// 走 git 子命令——本模块自带最简版，不复用 GitWorktreeWorkspace 的 run_git
/// 是因为后者 owns repo_root，本模块要支持任意 canonical 路径。
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

    /// 在 tempdir 里建一个最小 git repo（默认分支 main，一条 seed commit）。
    async fn make_git_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let out = Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
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
    async fn get_or_create_first_call_creates_worktree_with_expected_path_and_branch() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        let h = mgr.get_or_create("luban").await.expect("get_or_create");

        assert_eq!(h.project.as_str(), "erp");
        assert_eq!(h.role, "luban");
        assert_eq!(h.branch, "luban/erp-main");
        assert_eq!(h.workspace_id.as_str(), "erp/L3/luban");
        // path 物理存在 + 是 git worktree
        assert!(h.sandbox_path.is_dir(), "sandbox path 应存在");
        assert!(h.sandbox_path.join(".git").exists(), "应是 git worktree");
        assert!(
            h.sandbox_path.join("README.md").is_file(),
            "应继承 canonical 内容"
        );
    }

    #[tokio::test]
    async fn get_or_create_idempotent_returns_same_handle() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        let h1 = mgr.get_or_create("luban").await.unwrap();
        // 在 sandbox 里建个文件作为 WIP 标记——第二次 get_or_create 不该清掉
        tokio::fs::write(h1.sandbox_path.join("WIP.md"), "在做中")
            .await
            .unwrap();

        let h2 = mgr.get_or_create("luban").await.unwrap();
        assert_eq!(h1, h2, "重复 get_or_create 应返回同一 handle");
        assert!(
            h2.sandbox_path.join("WIP.md").exists(),
            "重复 get_or_create 不该清 sandbox 内容"
        );
    }

    #[tokio::test]
    async fn get_or_create_two_roles_in_same_project_have_different_branches() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        let luban = mgr.get_or_create("luban").await.unwrap();
        let pusong = mgr.get_or_create("pusong").await.unwrap();

        assert_ne!(luban.sandbox_path, pusong.sandbox_path);
        assert_eq!(luban.branch, "luban/erp-main");
        assert_eq!(pusong.branch, "pusong/erp-main");
    }

    #[tokio::test]
    async fn list_returns_existing_sandboxes_sorted() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        // 按 zebra → alpha → middle 顺序建，list 应按字典序返
        for role in ["zebra", "alpha", "middle"] {
            mgr.get_or_create(role).await.unwrap();
        }
        let listed = mgr.list().await.unwrap();
        let roles: Vec<&str> = listed.iter().map(|h| h.role.as_str()).collect();
        assert_eq!(roles, vec!["alpha", "middle", "zebra"]);
    }

    #[tokio::test]
    async fn list_empty_when_no_sandboxes() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        let listed = mgr.list().await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn retire_removes_worktree_and_branch() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        let h = mgr.get_or_create("luban").await.unwrap();
        assert!(h.sandbox_path.is_dir());

        mgr.retire("luban").await.unwrap();
        assert!(!h.sandbox_path.exists(), "retire 后 sandbox 应被删");

        // branch 应也被删
        let out = Command::new("git")
            .current_dir(&h.canonical_path)
            .args(["branch", "--list", "luban/erp-main"])
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.trim().is_empty(),
            "branch 应已被删；输出: {stdout:?}"
        );
    }

    #[tokio::test]
    async fn retire_noop_when_missing() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        // 没建过 luban，retire 不该 fail
        mgr.retire("luban").await.unwrap();
    }

    #[tokio::test]
    async fn invalid_role_rejected_in_get_or_create_and_retire() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());

        for bad in ["", "BAD", "with space", "with/slash", "中文"] {
            assert!(mgr.get_or_create(bad).await.is_err(), "role {bad:?} 应拒");
            assert!(mgr.retire(bad).await.is_err(), "retire {bad:?} 应拒");
        }
    }

    #[tokio::test]
    async fn workspace_id_format_aligns_with_decision_21() {
        let projects_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        let project = make_project(repo.canonicalize().unwrap());
        let mgr = PersistentSandboxManager::new(project, projects_root.path());
        let h = mgr.get_or_create("luban").await.unwrap();
        // Decision 21 约定：<project>/<layer>/<handle>
        assert_eq!(h.workspace_id.as_str(), "erp/L3/luban");
    }
}
