//! `GitWorktreeWorkspace`——`Workspace` trait 的 git worktree 实现。
//!
//! WHY：把一个 repo 切成 N 个 worktree，每个门客独占一个分支，彼此互不踩脚。
//! 按公理 #5，`list()` 以 `git worktree list --porcelain` 为真相源；内存 registry
//! 只是加速缓存，并非权威。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use fuxi_core::id::AgentId;
use fuxi_core::workspace::{Workspace, WorkspaceHandle};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::WorkspaceError;
use crate::porcelain;

/// 把 `repo_root` 当成"主"仓库，在 `root_base/<agent-id>` 下创建每个门客的 worktree。
///
/// WHY：
/// - `repo_root` 是 `git worktree add` 发起时的工作目录；主仓库持有 `.git`/objects。
/// - `root_base` 默认 `<repo>/.fuxi/worktrees/`，和其他伏羲运行时状态共置；允许覆写
///   以便把 worktree 放到 tmpfs / 快盘上。
/// - `registry` 只做缓存。`list()` 每次都重读 porcelain，避免"内存和文件系统脱钩"。
/// - `git_op_lock` 串行化 `create/destroy`，并非正确性必须（git index 自己锁），
///   而是为了在冲突时给出更清晰的伏羲侧错误，避免两个并发 `create` 把同名目录夹坏。
pub struct GitWorktreeWorkspace {
    repo_root: PathBuf,
    root_base: PathBuf,
    registry: Arc<RwLock<HashMap<AgentId, WorkspaceHandle>>>,
    git_op_lock: Mutex<()>,
    /// `destroy` 时是否连带把 feature 分支删掉。
    /// 默认 `true`——一次性 worktree 没必要留"僵尸分支"；需要保留时显式关掉。
    delete_branch_on_destroy: bool,
}

impl GitWorktreeWorkspace {
    /// 新建实例。`root_base` 不存在会在首次 `create()` 时自动建。
    pub fn new(repo_root: impl Into<PathBuf>, root_base: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            root_base: root_base.into(),
            registry: Arc::new(RwLock::new(HashMap::new())),
            git_op_lock: Mutex::new(()),
            delete_branch_on_destroy: true,
        }
    }

    /// 快捷构造：把 worktree 目录默认放在 `<repo>/.fuxi/worktrees/`。
    ///
    /// WHY：`.fuxi/` 是伏羲运行时状态的约定目录；不污染用户项目根。
    pub fn with_default_base(repo_root: impl Into<PathBuf>) -> Self {
        let repo_root = repo_root.into();
        let root_base = repo_root.join(".fuxi").join("worktrees");
        Self::new(repo_root, root_base)
    }

    /// 关掉"destroy 时自动删分支"。保留分支便于 post-mortem。
    pub fn keep_branch_on_destroy(mut self) -> Self {
        self.delete_branch_on_destroy = false;
        self
    }

    /// 返回主 repo 根路径（调试/测试用）。
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// 返回 worktree 目录的根（调试/测试用）。
    pub fn root_base(&self) -> &Path {
        &self.root_base
    }

    /// 运行 `git` 子命令，返回 stdout；非零退出码返回 `WorkspaceError::Git`。
    ///
    /// WHY：所有 git 调用走这一个口，便于统一打 tracing、统一错误转换。
    pub(crate) async fn run_git(
        &self,
        args: &[&str],
        cwd: &Path,
    ) -> Result<String, WorkspaceError> {
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

    /// 验证 `repo_root` 是一个 git 仓库；否则抛 `NotAGitRepo`。
    async fn ensure_repo(&self) -> Result<(), WorkspaceError> {
        if !self.repo_root.exists() {
            return Err(WorkspaceError::NotAGitRepo(self.repo_root.clone()));
        }
        let out = self
            .run_git(&["rev-parse", "--is-inside-work-tree"], &self.repo_root)
            .await;
        match out {
            Ok(s) if s.trim() == "true" => Ok(()),
            _ => Err(WorkspaceError::NotAGitRepo(self.repo_root.clone())),
        }
    }

    /// 构造一个短 agent id（首 8 个 hex 字符）——给分支名用。
    fn short_agent(agent: &AgentId) -> String {
        let raw = agent.0.simple().to_string();
        raw.chars().take(8).collect()
    }

    /// 计算 worktree 目录：`<root_base>/<agent-uuid-simple>`。
    ///
    /// WHY：用 simple（无连字符）形式避免任何平台对 `-` 过敏；这里的路径不给人读，
    /// 只给 git 和 agent runtime 消费。
    fn worktree_path_for(&self, agent: &AgentId) -> PathBuf {
        self.root_base.join(agent.0.simple().to_string())
    }

    /// 构造分支名 `fuxi/<short>-<unix-timestamp>`。
    ///
    /// WHY：
    /// - `fuxi/` 命名空间便于 `git branch -D fuxi/*` 批量清理。
    /// - 附 timestamp：同一个 agent 被重建时分支名不会撞。
    fn branch_name_for(agent: &AgentId) -> String {
        let ts = chrono::Utc::now().timestamp();
        format!("fuxi/{}-{}", Self::short_agent(agent), ts)
    }
}

#[async_trait]
impl Workspace for GitWorktreeWorkspace {
    async fn create(
        &self,
        agent: AgentId,
        base_branch: &str,
    ) -> fuxi_core::Result<WorkspaceHandle> {
        let _guard = self.git_op_lock.lock().await;

        self.ensure_repo()
            .await
            .map_err(WorkspaceError::into_core)?;

        let worktree_path = self.worktree_path_for(&agent);
        if worktree_path.exists() {
            return Err(WorkspaceError::AlreadyExists(worktree_path).into_core());
        }

        // 懒建 root_base——不强求用户提前 mkdir。
        if let Some(parent) = worktree_path.parent()
            && !parent.exists()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorkspaceError::Io(e).into_core())?;
        }

        let branch = Self::branch_name_for(&agent);
        let wt_path_str = worktree_path
            .to_str()
            .ok_or_else(|| {
                WorkspaceError::Other(format!(
                    "worktree path not valid utf-8: {}",
                    worktree_path.display()
                ))
            })
            .map_err(WorkspaceError::into_core)?;

        info!(
            agent = %agent,
            path = wt_path_str,
            branch = %branch,
            base = base_branch,
            "creating git worktree"
        );

        // 首选单步：`git worktree add -b <branch> <path> <base>`。
        let combined = self
            .run_git(
                &["worktree", "add", "-b", &branch, wt_path_str, base_branch],
                &self.repo_root,
            )
            .await;

        if let Err(err) = combined {
            warn!(
                ?err,
                "combined worktree add failed, falling back to two-step"
            );
            // 回落：先 detach add，再 switch -c。
            self.run_git(
                &["worktree", "add", "--detach", wt_path_str, base_branch],
                &self.repo_root,
            )
            .await
            .map_err(WorkspaceError::into_core)?;
            self.run_git(&["switch", "-c", &branch, base_branch], &worktree_path)
                .await
                .map_err(WorkspaceError::into_core)?;
        }

        let handle = WorkspaceHandle {
            agent,
            repo_root: self.repo_root.clone(),
            worktree_path,
            branch,
        };

        self.registry.write().await.insert(agent, handle.clone());

        Ok(handle)
    }

    async fn destroy(&self, handle: &WorkspaceHandle) -> fuxi_core::Result<()> {
        let _guard = self.git_op_lock.lock().await;

        let wt_path_str = handle.worktree_path.to_str().ok_or_else(|| {
            WorkspaceError::Other(format!(
                "worktree path not valid utf-8: {}",
                handle.worktree_path.display()
            ))
            .into_core()
        })?;

        info!(
            agent = %handle.agent,
            path = wt_path_str,
            branch = %handle.branch,
            "destroying git worktree"
        );

        // --force：agent 可能留了 dirty working tree；P2 阶段我们接受丢未推送改动。
        let remove = self
            .run_git(
                &["worktree", "remove", "--force", wt_path_str],
                &self.repo_root,
            )
            .await;

        if let Err(WorkspaceError::Git { stderr, .. }) = &remove {
            // 幂等：目录已不存在 / 已 prune 过，视为成功。
            // git 的报错串格式会变，采用宽松匹配。
            let s = stderr.to_lowercase();
            let already_gone = s.contains("is not a working tree")
                || s.contains("not a valid")
                || s.contains("no such")
                || s.contains("does not exist");
            if !already_gone {
                return Err(remove.err().unwrap().into_core());
            }
            warn!(stderr = %stderr, "worktree already gone; treating destroy as idempotent");
            // 兜底 prune，修复元数据残留。
            let _ = self.run_git(&["worktree", "prune"], &self.repo_root).await;
        } else if let Err(e) = remove {
            return Err(e.into_core());
        }

        if self.delete_branch_on_destroy {
            // 删分支失败不致命：分支可能根本没创建成功。
            if let Err(err) = self
                .run_git(&["branch", "-D", &handle.branch], &self.repo_root)
                .await
            {
                debug!(?err, "branch delete skipped");
            }
        }

        self.registry.write().await.remove(&handle.agent);
        Ok(())
    }

    async fn list(&self) -> fuxi_core::Result<Vec<WorkspaceHandle>> {
        // 公理 #5：git 才是真相源。以 porcelain 为准，用 registry 补齐 agent 归属。
        let raw = self
            .run_git(&["worktree", "list", "--porcelain"], &self.repo_root)
            .await
            .map_err(WorkspaceError::into_core)?;

        let parsed = porcelain::parse(&raw);
        let reg = self.registry.read().await;

        // WHY：路径用 canonicalize 规范化后再比较。
        // macOS 的 `/var/folders/...` 通过 `/private/var/...` 符号链接暴露，
        // `git worktree list --porcelain` 返回解析后的真实路径；而 registry 里
        // 存的是用户给的原始路径。不规范化就对不上。
        let canon = |p: &Path| -> PathBuf { p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) };

        let by_path: HashMap<PathBuf, &WorkspaceHandle> =
            reg.values().map(|h| (canon(&h.worktree_path), h)).collect();

        let repo_canon = canon(&self.repo_root);
        let mut out = Vec::new();
        for w in parsed {
            let w_canon = canon(&w.path);
            // 跳过主 repo（不是门客的 worktree）。
            if w_canon == repo_canon {
                continue;
            }
            if let Some(handle) = by_path.get(&w_canon) {
                out.push((*handle).clone());
            }
            // 未在 registry 的 worktree（可能是外部手动加的 / 重启后丢了 registry）——
            // 暂不合成假 handle：没有 AgentId 反推机制，给空跳过比捏造稳。
        }
        Ok(out)
    }
}
