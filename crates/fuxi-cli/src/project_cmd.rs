//! `fuxi project` 子命令家族（Decision 21 phase 1）。
//!
//! 子命令：
//! - `fuxi project add <canonical-path> [--name <slug>] [--branch <name>]`
//! - `fuxi project list`
//! - `fuxi project rm <id>`
//!
//! 走 `FileSystemProjectRegistry`，root 默认 `$HOME/.fuxi/projects/`，
//! `--registry-root` 覆写（测试 / 替代部署用）。
//!
//! 输出：人类可读纯文本（不上 banner / 颜色）——这是工具型命令，PWA 后续
//! 会有自己的 GUI 注册流。

use anyhow::{Context, Result};
use clap::Args;
use fuxi_core::ProjectId;
use fuxi_workspace::FileSystemProjectRegistry;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct ProjectAddArgs {
    /// 用户真项目的绝对或相对路径（必须是已存在的 git repo）。
    pub canonical_path: PathBuf,
    /// 项目 slug。不传从 canonical 末段 basename 派生；非 ASCII basename
    /// 派生失败时必须显式传。
    #[arg(long)]
    pub name: Option<String>,
    /// 默认基线 branch。不传默认 `main`。
    #[arg(long)]
    pub branch: Option<String>,
    /// 注册表 root 覆写。默认 `$HOME/.fuxi/projects/`。测试 / 多账号时用。
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProjectListArgs {
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProjectRemoveArgs {
    /// 要删的 project id（slug）。
    pub id: String,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

fn registry_for(root: Option<PathBuf>) -> Result<FileSystemProjectRegistry> {
    match root {
        Some(r) => Ok(FileSystemProjectRegistry::new(r)),
        None => {
            FileSystemProjectRegistry::with_default_root().context("无法构造默认 ProjectRegistry")
        }
    }
}

pub async fn run_add(args: ProjectAddArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let project = registry
        .add(args.canonical_path, args.name, args.branch)
        .await
        .context("project add 失败")?;
    println!(
        "已注册 project {} → {}",
        project.id,
        project.canonical_path.display()
    );
    println!("  default branch: {}", project.default_branch);
    println!(
        "  落盘: {}/{}/meta.json",
        registry.root().display(),
        project.id
    );
    Ok(())
}

pub async fn run_list(args: ProjectListArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let projects = registry.list().await.context("project list 失败")?;
    if projects.is_empty() {
        println!("（暂无注册 project；用 `fuxi project add <path>` 注册）");
        return Ok(());
    }
    // 简单两栏对齐：id 和 canonical_path
    let max_id_len = projects
        .iter()
        .map(|p| p.id.as_str().len())
        .max()
        .unwrap_or(0);
    for p in projects {
        println!(
            "  {:width$}  {}",
            p.id,
            p.canonical_path.display(),
            width = max_id_len
        );
    }
    Ok(())
}

pub async fn run_remove(args: ProjectRemoveArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let id = ProjectId::new(args.id.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.id))?;
    // 先 get 一下：不存在直接告诉用户，避免静默 noop 误以为删了
    if registry.get(&id).await?.is_none() {
        anyhow::bail!("project {id} 不存在");
    }
    registry.remove(&id).await.context("project remove 失败")?;
    println!("已删 project {id}");
    println!("  注意：sandboxes / ephemeral / archive / deliverables 子目录一并清掉");
    Ok(())
}

#[cfg(test)]
mod tests {
    //! e2e 走完整 add → list → remove 路径，验证 CLI 跟 registry 接通。
    //! 不验 stdout 文本格式（那是用户体验，不是契约）。

    use super::*;
    use tempfile::TempDir;

    async fn make_git_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
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
                .unwrap();
            assert!(out.status.success());
        }
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "x"])
            .output()
            .await;
        (dir, path)
    }

    #[tokio::test]
    async fn add_then_list_then_remove() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;

        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // list 不该 fail
        run_list(ProjectListArgs {
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 物理验证
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let projects = registry.list().await.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id.as_str(), "erp");

        // remove
        run_remove(ProjectRemoveArgs {
            id: "erp".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        let projects = registry.list().await.unwrap();
        assert!(projects.is_empty());
    }

    #[tokio::test]
    async fn remove_missing_id_errors() {
        let registry_root = tempfile::tempdir().unwrap();
        let err = run_remove(ProjectRemoveArgs {
            id: "ghost".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("不存在"), "got: {err}");
    }

    #[tokio::test]
    async fn add_invalid_slug_errors() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_td, repo) = make_git_repo().await;
        let err = run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("BAD-SLUG".into()), // 大写
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        // anyhow::Error 用 chain() 能拿到下游错误链——top-level 是 "project add 失败"，
        // 真因（"[a-z0-9_-]" 提示）在 chain 里。{:#} 也行但这里用 chain 更显式。
        let full = format!("{err:#}");
        assert!(
            full.contains("[a-z0-9_-]") || full.contains("BAD-SLUG"),
            "got: {full}"
        );
    }
}
