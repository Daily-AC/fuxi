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
use fuxi_core::{DeliverableKind, ProjectId, TaskId};
use fuxi_workspace::{DeliverablesManager, FileSystemProjectRegistry};
use std::path::PathBuf;
use std::str::FromStr;

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

/// `fuxi deliverable produce` —— 手动让某 project 的某 task 落一组文件作 deliverable。
///
/// v1 用途：跑测、手工验证 PWA 收件箱、模拟门客交付（agent hook 还没接通时
/// 让用户能端到端走通）。production agent 集成后，门客自己调 produce_deliverable
/// API 不走这个 CLI。
#[derive(Debug, Args)]
pub struct DeliverableProduceArgs {
    /// 项目 slug（必须已 `fuxi project add` 注册过）。
    #[arg(long)]
    pub project: String,
    /// 任务 id。`task-<uuid>` 或裸 uuid。不传走随机新生成（适合纯手工测）。
    #[arg(long)]
    pub task: Option<String>,
    /// deliverable 类型——Decision 13 五类：
    /// research_summary / code_change / test_result / decision_request / error_block。
    #[arg(long, default_value = "research_summary")]
    pub kind: String,
    /// 要交付的文件路径列表（必须是已存在的文件）。
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// 注册表 root 覆写（同 project add）。
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

pub async fn run_deliverable_produce(args: DeliverableProduceArgs) -> Result<()> {
    let registry = registry_for(args.registry_root.clone())?;
    let project_id = ProjectId::new(args.project.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.project))?;
    let project = registry
        .get(&project_id)
        .await
        .context("project lookup 失败")?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} 未注册——先跑 fuxi project add"))?;

    let task = parse_task_id(args.task.as_deref())?;
    let kind = parse_deliverable_kind(&args.kind)?;

    // canonicalize sources：相对路径转绝对，文件不存在或不是文件直接报错
    let mut sources = Vec::with_capacity(args.files.len());
    for f in &args.files {
        let canon = f
            .canonicalize()
            .with_context(|| format!("文件无法解析: {}", f.display()))?;
        if !canon.is_file() {
            anyhow::bail!("源不是文件: {}", canon.display());
        }
        sources.push(canon);
    }

    // 落地用 registry root（同样 root 下 deliverables/ 跟 projects/ 同级）
    let mgr = DeliverablesManager::new(project.id.clone(), registry.root());
    let handle = mgr
        .produce(task, kind, &sources)
        .await
        .context("produce_deliverable 失败")?;

    println!("已交付 deliverable");
    println!("  project: {}", handle.project);
    println!("  task: {}", handle.task);
    println!("  kind: {:?}", handle.kind);
    println!("  bucket: {}", handle.bucket_path.display());
    println!("  files:");
    for f in &handle.files {
        println!(
            "    {} ({} bytes, sha256={})",
            f.name,
            f.size_bytes,
            &f.sha256[..16]
        );
    }
    Ok(())
}

fn parse_task_id(s: Option<&str>) -> Result<TaskId> {
    match s {
        None => Ok(TaskId::new()),
        Some(raw) => {
            let trimmed = raw.strip_prefix("task-").unwrap_or(raw);
            uuid::Uuid::from_str(trimmed)
                .map(TaskId::from)
                .map_err(|e| anyhow::anyhow!("无效 task id {raw}: {e}"))
        }
    }
}

fn parse_deliverable_kind(s: &str) -> Result<DeliverableKind> {
    match s {
        "research_summary" => Ok(DeliverableKind::ResearchSummary),
        "code_change" => Ok(DeliverableKind::CodeChange),
        "test_result" => Ok(DeliverableKind::TestResult),
        "decision_request" => Ok(DeliverableKind::DecisionRequest),
        "error_block" => Ok(DeliverableKind::ErrorBlock),
        other => anyhow::bail!(
            "未知 deliverable kind: {other}（可选：research_summary / code_change / test_result / decision_request / error_block）"
        ),
    }
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
    async fn deliverable_produce_writes_files_to_bucket() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;

        // 先 add 一个 project
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 准备一个源文件
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("report.md");
        tokio::fs::write(&src, "# 调研结果").await.unwrap();

        let task = TaskId::new();
        run_deliverable_produce(DeliverableProduceArgs {
            project: "erp".into(),
            task: Some(task.to_string()),
            kind: "research_summary".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .expect("produce");

        // 文件应已落到 deliverables/<task>/
        let bucket = registry_root
            .path()
            .join("erp")
            .join("deliverables")
            .join(task.to_string());
        assert!(bucket.is_dir(), "bucket 应建好");
        assert!(bucket.join("report.md").is_file(), "文件应落地");
        assert!(bucket.join("manifest.json").is_file(), "manifest 应写");
    }

    #[tokio::test]
    async fn deliverable_produce_rejects_unknown_project() {
        let registry_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("x.md");
        tokio::fs::write(&src, "x").await.unwrap();
        let err = run_deliverable_produce(DeliverableProduceArgs {
            project: "no-such-project".into(),
            task: None,
            kind: "research_summary".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .expect_err("未注册 project 应拒");
        assert!(err.to_string().contains("未注册"), "got: {err}");
    }

    #[tokio::test]
    async fn deliverable_produce_rejects_unknown_kind() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_td, repo) = make_git_repo().await;
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("x.md");
        tokio::fs::write(&src, "x").await.unwrap();

        let err = run_deliverable_produce(DeliverableProduceArgs {
            project: "erp".into(),
            task: None,
            kind: "wrong_kind".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .expect_err("未知 kind 应拒");
        let full = format!("{err:#}");
        assert!(
            full.contains("未知 deliverable kind") || full.contains("research_summary"),
            "got: {full}"
        );
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
