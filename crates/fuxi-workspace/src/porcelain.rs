//! 解析 `git worktree list --porcelain` 的输出。
//!
//! WHY：按公理 #5，文件系统只是快照，`git` 才是 worktree 元数据的权威出处。
//! 内存注册表只是加速缓存；每次 `list()` 都应该以 porcelain 输出为准。
//!
//! Porcelain 格式（稳定）：每个 worktree 由若干 `key value` 行组成，
//! 各 worktree 之间以空行分隔。关键字段：
//! - `worktree <absolute path>`   必有，第一行
//! - `HEAD <sha>`                 必有（除非 `bare`）
//! - `branch <refname>`           有分支时出现，detached 时不出现，取而代之的是 `detached`
//! - `bare` / `detached` / `locked [reason]` / `prunable [reason]` 是单行开关
//!
//! 参考：`git help worktree` → "Porcelain Format" 节。

use std::path::PathBuf;

/// Porcelain 解析后的单个 worktree 记录。
///
/// WHY：直接映射 `git worktree list --porcelain` 的字段，不做任何语义加工；
/// 语义化（转成 `WorkspaceHandle`）由调用方负责，这里只做忠实反序列化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainWorktree {
    /// worktree 的绝对路径。
    pub path: PathBuf,
    /// HEAD 指向的 commit SHA；bare worktree 没有。
    pub head: Option<String>,
    /// 分支全名（如 `refs/heads/feat/foo`）；detached 时为 `None`。
    pub branch: Option<String>,
    /// detached HEAD 状态。
    pub detached: bool,
    /// bare 仓库（主 worktree 可能是 bare 的）。
    pub bare: bool,
}

impl PorcelainWorktree {
    /// 分支的短名（去掉 `refs/heads/` 前缀），没有则 `None`。
    pub fn branch_short(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .and_then(|b| b.strip_prefix("refs/heads/").or(Some(b)))
    }
}

/// 解析完整的 porcelain 输出为一组记录。
///
/// WHY：输入为空串时返回空 `Vec`，不报错——`git` 在只有 bare 主仓库且没有 worktree 时
/// 仍然会打印主仓库那一段，但我们不把"空"当作异常。
pub fn parse(input: &str) -> Vec<PorcelainWorktree> {
    let mut out = Vec::new();
    let mut current: Option<PorcelainWorktree> = None;

    for line in input.lines() {
        if line.is_empty() {
            if let Some(w) = current.take() {
                out.push(w);
            }
            continue;
        }

        let (key, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, r),
            None => (line, ""),
        };

        match key {
            "worktree" => {
                if let Some(w) = current.take() {
                    out.push(w);
                }
                current = Some(PorcelainWorktree {
                    path: PathBuf::from(rest),
                    head: None,
                    branch: None,
                    detached: false,
                    bare: false,
                });
            }
            "HEAD" => {
                if let Some(w) = current.as_mut() {
                    w.head = Some(rest.to_string());
                }
            }
            "branch" => {
                if let Some(w) = current.as_mut() {
                    w.branch = Some(rest.to_string());
                }
            }
            "detached" => {
                if let Some(w) = current.as_mut() {
                    w.detached = true;
                }
            }
            "bare" => {
                if let Some(w) = current.as_mut() {
                    w.bare = true;
                }
            }
            // `locked`、`prunable` 等字段当前用不到，忽略而非报错——
            // 向前兼容 git 未来新增字段的策略。
            _ => {}
        }
    }

    if let Some(w) = current.take() {
        out.push(w);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真 `git worktree list --porcelain` 的典型输出（含主 + 2 个 worktree）。
    const FIXTURE_MULTI: &str = "\
worktree /home/user/repo
HEAD 0123456789abcdef0123456789abcdef01234567
branch refs/heads/main

worktree /home/user/repo/.fuxi/worktrees/agent-a
HEAD aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/fuxi/abcd1234-1700000000

worktree /home/user/repo/.fuxi/worktrees/agent-b
HEAD bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
detached
";

    const FIXTURE_BARE: &str = "\
worktree /srv/repos/foo.git
bare

worktree /srv/repos/foo-wt
HEAD cccccccccccccccccccccccccccccccccccccccc
branch refs/heads/dev
";

    #[test]
    fn parses_multi_worktree_fixture() {
        let wts = parse(FIXTURE_MULTI);
        assert_eq!(wts.len(), 3);

        assert_eq!(wts[0].path, PathBuf::from("/home/user/repo"));
        assert_eq!(wts[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(wts[0].branch_short(), Some("main"));
        assert!(!wts[0].detached);
        assert!(!wts[0].bare);

        assert_eq!(
            wts[1].path,
            PathBuf::from("/home/user/repo/.fuxi/worktrees/agent-a")
        );
        assert_eq!(wts[1].branch_short(), Some("fuxi/abcd1234-1700000000"));

        assert!(wts[2].detached);
        assert!(wts[2].branch.is_none());
        assert!(wts[2].branch_short().is_none());
    }

    #[test]
    fn parses_bare_fixture() {
        let wts = parse(FIXTURE_BARE);
        assert_eq!(wts.len(), 2);
        assert!(wts[0].bare);
        assert!(wts[0].head.is_none());
        assert_eq!(wts[1].branch_short(), Some("dev"));
    }

    #[test]
    fn empty_input_is_empty_vec() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn trailing_blank_lines_tolerated() {
        let input = "worktree /tmp/x\nHEAD deadbeef\n\n\n";
        let wts = parse(input);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].head.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn unknown_keys_ignored() {
        // 向前兼容：未来 git 可能加新字段。
        let input = "worktree /tmp/y\nHEAD cafebabe\nlocked because reasons\nbranch refs/heads/x\n";
        let wts = parse(input);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].branch_short(), Some("x"));
    }
}
