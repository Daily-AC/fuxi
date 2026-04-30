# Git 工作流规范

本仓库采用 **main trunk + 短分支 + release 稳定分支**。不要把长期产品线开发放在 `feat/*` 分支里。

## 分支角色

| 分支 | 用途 | 规则 |
|---|---|---|
| `main` | 永远可构建、可回滚的主干 | 只接受已经通过门禁的合并；不直接堆实验代码 |
| `feat/<scope>` | 单个功能或一组强相关改动 | 短生命周期；合并后删除 |
| `fix/<scope>` | bug 修复、回归修复 | 短生命周期；优先小 diff |
| `docs/<scope>` | 文档、规范、状态口径修正 | 不混入行为改动 |
| `chore/<scope>` | 构建、依赖、脚本、仓库维护 | 不混入产品语义 |
| `release/vX.Y` | 版本冻结、回归修复、发布说明 | 只收敛，不继续做新功能 |
| `hotfix/<scope>` | 从已发布版本切出的紧急修复 | 修完合回 `main` 和对应 `release/*` |

`feat/fuxi-v0.1` 这类分支名不再作为开发主线使用。它把“功能分支”和“版本线”混在一起，会导致所有开发都堆在一个长寿分支里，review、回滚、发布判断都会失真。

## 当前分支处置

当前 `feat/fuxi-v0.1` 只作为历史集成分支处理：

1. 已经在上面的改动可以继续收口并形成可 review 的提交。
2. 收口后合入 `main`，或如果还需要冻结测试，先切 `release/v0.1`。
3. 合并完成后删除 `feat/fuxi-v0.1`，后续新工作从 `main` 重新切短分支。

如果版本目标已经不是 `0.1`，不要继续改名为另一个 `feat/fuxi-vX`。应切 `release/vX.Y` 做发布稳定，功能开发仍然走短 `feat/*`。

## 分支命名

分支名使用小写短 slug：

```text
feat/im-dist-routing
fix/im-xuannv-fail-fast
docs/git-workflow
chore/ci-rust-gates
release/v0.1
hotfix/session-resume-panic
```

命名原则：

- `scope` 写行为边界，不写里程碑口号。
- 一个分支只服务一个可 review 的目标。
- 多 agent 并行开发时，每个 agent 使用独立 worktree 和独立分支，最后由主线负责人 cherry-pick 或 merge。

## 提交规范

提交信息使用：

```text
type(scope): 中文摘要
```

常用 type：

| type | 用途 |
|---|---|
| `feat` | 新能力 |
| `fix` | 行为修复 |
| `docs` | 文档 |
| `test` | 测试补充或修复 |
| `refactor` | 不改变行为的结构调整 |
| `chore` | 构建、脚本、依赖、配置 |
| `perf` | 性能改进 |

示例：

```text
fix(im): 玄女自启失败时终止启动
docs(status): 更新当前 live snapshot
refactor(cli): 拆分 repl 输入处理
```

提交要求：

- 每个提交应能解释“为什么这是一组改动”。
- 行为改动和文档同步可以在同一个提交里，但不要把无关清理混进去。
- 大重构先拆机械移动，再拆行为修改，便于 review。
- 提交前至少跑与改动相关的最小门禁；发布或合入 `main` 前跑完整门禁。

## 合并门禁

合入 `main` 或 `release/*` 前必须满足：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
(cd crates/fuxi-im/web && pnpm test && pnpm typecheck && pnpm lint)
git diff --check
```

如果某个门禁在本地环境不可运行，提交说明或 PR 描述必须写清楚：

- 没跑哪条命令；
- 为什么没跑；
- 已经用什么窄门禁替代；
- 剩余风险是什么。

## 发布流程

1. 从 `main` 切 `release/vX.Y`。
2. release 分支只接受回归修复、文档补齐、版本号和发布说明。
3. 完整门禁通过后打 tag：

```bash
git tag -a vX.Y.Z -m "vX.Y.Z"
```

4. release 分支上的修复必须合回 `main`，避免发布线和主干分叉。

## 禁止事项

- 不在 `feat/fuxi-v0.1` 这类长寿分支上无限继续开发。
- 不把多个不相关功能塞进一个分支。
- 不在 `release/*` 上做新功能。
- 不用 `wip`、`temp`、`backup` 作为长期远程分支。
- 不提交测试副作用产物，除非该产物本身就是本次变更的交付物。
