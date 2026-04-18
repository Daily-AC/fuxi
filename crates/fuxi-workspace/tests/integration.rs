//! 集成测试：在 tempdir 里跑真 git，覆盖 create / destroy / list / 并发。
//!
//! WHY：`git` 是本 crate 的硬依赖；mock 掉等于没测。tempdir 保证互不干扰。

use std::path::{Path, PathBuf};
use std::process::Command;

use fuxi_core::id::AgentId;
use fuxi_core::workspace::Workspace;
use fuxi_workspace::GitWorktreeWorkspace;
use tempfile::TempDir;

/// 断言 `git` 可用——缺了 git 这个 crate 根本跑不起来。
fn require_git() {
    let ok = Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        panic!("git required for integration tests");
    }
}

/// 在 tempdir 里初始化一个仓库 + 一个提交，返回 (TempDir 保活, 仓库路径)。
fn init_repo() -> (TempDir, PathBuf) {
    require_git();
    let td = TempDir::new().expect("tempdir");
    let repo = td.path().to_path_buf();

    run(&repo, &["init", "-b", "main"]);
    run(&repo, &["config", "user.email", "fuxi@test.local"]);
    run(&repo, &["config", "user.name", "fuxi"]);
    run(&repo, &["config", "commit.gpgsign", "false"]);

    std::fs::write(repo.join("README.md"), "# test\n").unwrap();
    run(&repo, &["add", "README.md"]);
    run(&repo, &["commit", "-m", "init"]);

    (td, repo)
}

fn run(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
async fn create_single_worktree_in_tempdir() {
    let (_td, repo) = init_repo();
    let ws = GitWorktreeWorkspace::with_default_base(&repo);
    let agent = AgentId::new();

    let handle = ws.create(agent, "main").await.expect("create");
    assert!(handle.worktree_path.exists(), "worktree dir should exist");
    assert!(
        handle.worktree_path.join(".git").exists(),
        "nested .git file"
    );
    assert!(handle.branch.starts_with("fuxi/"));

    let listed = ws.list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].agent, agent);
    assert_eq!(listed[0].worktree_path, handle.worktree_path);
}

#[tokio::test]
async fn create_then_destroy() {
    let (_td, repo) = init_repo();
    let ws = GitWorktreeWorkspace::with_default_base(&repo);
    let agent = AgentId::new();
    let handle = ws.create(agent, "main").await.expect("create");
    let wt_path = handle.worktree_path.clone();
    let branch = handle.branch.clone();

    ws.destroy(&handle).await.expect("destroy");
    assert!(!wt_path.exists(), "worktree dir should be gone");

    // 分支应当被删（默认 delete_branch_on_destroy=true）。
    let show = Command::new("git")
        .args(["branch", "--list", &branch])
        .current_dir(&repo)
        .output()
        .expect("git branch");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(
        stdout.trim().is_empty(),
        "branch should be deleted, got: {stdout:?}"
    );

    let listed = ws.list().await.expect("list empty");
    assert!(listed.is_empty());
}

#[tokio::test]
async fn multiple_parallel_creates_ok() {
    let (_td, repo) = init_repo();
    let ws = std::sync::Arc::new(GitWorktreeWorkspace::with_default_base(&repo));
    let a = AgentId::new();
    let b = AgentId::new();
    let c = AgentId::new();

    let (ra, rb, rc) = tokio::join!(
        {
            let ws = ws.clone();
            async move { ws.create(a, "main").await }
        },
        {
            let ws = ws.clone();
            async move { ws.create(b, "main").await }
        },
        {
            let ws = ws.clone();
            async move { ws.create(c, "main").await }
        },
    );

    let ha = ra.expect("a");
    let hb = rb.expect("b");
    let hc = rc.expect("c");
    assert_ne!(ha.worktree_path, hb.worktree_path);
    assert_ne!(hb.worktree_path, hc.worktree_path);
    assert_ne!(ha.branch, hb.branch);

    let listed = ws.list().await.expect("list");
    assert_eq!(listed.len(), 3);
}

#[tokio::test]
async fn destroy_idempotent_on_already_removed() {
    // 选定行为：destroy 幂等——重复调用返回 Ok。
    // WHY：清理协程可能重试；更顺手的是幂等，而不是让调用方处理 NotFound。
    let (_td, repo) = init_repo();
    let ws = GitWorktreeWorkspace::with_default_base(&repo);
    let agent = AgentId::new();
    let handle = ws.create(agent, "main").await.expect("create");
    ws.destroy(&handle).await.expect("first destroy");
    ws.destroy(&handle)
        .await
        .expect("second destroy should be Ok (idempotent)");
}

#[tokio::test]
async fn base_branch_honored() {
    let (_td, repo) = init_repo();

    // 在 main 上再提交一个 commit，然后从旧 commit 上开 feature 分支。
    std::fs::write(repo.join("a.txt"), "v1").unwrap();
    run(&repo, &["add", "a.txt"]);
    run(&repo, &["commit", "-m", "v1"]);
    let commit_v1 = run(&repo, &["rev-parse", "HEAD"]).trim().to_string();

    std::fs::write(repo.join("a.txt"), "v2").unwrap();
    run(&repo, &["add", "a.txt"]);
    run(&repo, &["commit", "-m", "v2"]);

    // 从 commit_v1 拉一个分支，供 create() 使用。
    run(&repo, &["branch", "feature-x", &commit_v1]);

    let ws = GitWorktreeWorkspace::with_default_base(&repo);
    let agent = AgentId::new();
    let handle = ws.create(agent, "feature-x").await.expect("create");

    // 断言 worktree HEAD == commit_v1。
    let head_in_wt = {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&handle.worktree_path)
            .output()
            .expect("rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(
        head_in_wt, commit_v1,
        "worktree should start at feature-x tip (== commit_v1)"
    );

    // 文件内容也应当是 v1。
    let body = std::fs::read_to_string(handle.worktree_path.join("a.txt")).unwrap();
    assert_eq!(body, "v1");
}
