# Decision 07 · P2 召回 Scope · 通用 wire + cc 特化层

**日期**：2026-04-20
**状态**：已拍板，L2 已实装。当前代码里 codex 走 worktree-only recall；cc
额外带 CLI session resume。

## 背景

P2 召回首版（commit `8feb7eb`）走完三档 e2e：
- 档 1（wire smoke）✓ 互斥 flag、空记录错误、空 oracle 启动
- 档 2（sink 入库）✓ task-fact + role-fact 双写、object 同 session_id
- 档 3（真召回）✗ cc 启动死："No conversation found with session ID: <sid>"

档 3 死因不在 召回 wire，而是触发了一个**未曾考虑的 cc 行为**：

> **cc 把 session 文件按 cwd 索引存到 `~/.claude/projects/<mangled-cwd>/<sid>.jsonl`**

fuxi 给每次 spawn 分配新 worktree（`.fuxi/worktrees/<agent_id>`）→ round 2 的 cwd 和 round 1 不同 → cc 在新 cwd 对应的 projects 子目录下找不到 round 1 的 session 文件 → resume 失败。

## 暴露的更深问题

回头审视 P2 wire 的设计选择，发现几条**走偏 / 走窄**的痕迹：

1. **`Agent::session_id() -> Option<String>` 把 cc 概念泄漏进通用 trait**
   codex 永远 None，gemini 还不知道。默认 None 容忍度 OK，但暴露了我们把"召回 key"窄化成了"cc 那个 uuid"。当前实现已把召回真相扩到 worktree context，codex 通过 worktree-only 召回闭环。

2. **RecallSink trait 签名绑死 `session_id: String`**
   历史问题：`record_task_session(agent_id, task_id, session_id)` 把 session_id 设为 required，pump 用 `session_id.is_some()` 守门，导致 codex 门客不会进 sink。当前已改为 `RecallContext`，codex 可以只记录 worktree，不需要 CLI session。

3. **召回语义被窄化成"cc 对话线"**
   - 真相：召回 ≥ "整个工作环境"（cwd/worktree + CLI-specific session if any）
   - 玄女 cwd 稳定（= 用户 cwd）→ session resume 一直工作
   - 门客 cwd = worktree（每次变）→ session resume 必然失败
   - **这两路汇到 `CcLaunchConfig.resume_session_id` 但底下 cwd 假设是相反的**——P2 设计当初没画出这个矛盾

4. **worktree 生命周期没和 recall 绑**
   M2.4 idle GC 10min 杀 agent + 回收 worktree。**有 recall 价值的 worktree 被 GC 了 recall 就死**（worktree 不在 → cc 就算 resume 也没代码可看）。

5. **玄女 vs 门客两种 recall 语义不一致但路径混着走**
   - 玄女：跨重启的单例，`session.rs::resolve_xuannv_session` 走 oracle，cwd 稳定 → resume 真工作
   - 门客：可召回的多实例，新 P2 路径走 OracleRecallSink，cwd 不稳定 → resume 失败
   - 两个 wire 都通过 `CcLaunchConfig.resume_session_id`——容易让人误以为它们等价，实际不是

## 选项

### L1 · 最小补丁（保留窄 trait）
sink 多写一条 `task-<id>, predicate=worktree`；recall spawn 时复用 worktree。
- 1 session 工作量
- 修了 cc bug，**没修 trait 走窄**
- 只救 cc；不覆盖 codex 的 worktree-only recall，也不为 gemini 留干净接口

### L2 · trait 通用化（推荐）
RecallSink 签名改 `record(ctx: RecallContext)` 带完整 context；codex 走 worktree-only 召回；trait 不绑死 cc。

```rust
pub struct RecallContext {
    pub agent_id: AgentId,
    pub task_id: TaskId,
    pub role: String,
    pub worktree: Option<PathBuf>,         // None = 玄女这种不分 worktree 的
    pub cli_session_id: Option<String>,    // cc 才有
}
```

- 1.5 session 工作量
- wire 层干净，未来 gemini 加 variant 不改 core
- 修根本 bug

### L3 · 重做 cc cwd 策略
让 cc cwd 稳定到 session dir，worktree 通过 bash `cd` 切。
- 2-3 session
- 破坏 cc Read/Edit 默认相对路径，踩坑面大
- 和 D18「Resume 真回放 dialogue history」并档做更合算

## 决定 · L2

**理由**：
- L1 救火不救路——下次加 gemini 又得回来重做
- L3 太重，等 v1.2 D18 一起重新审 cc 集成层

**边界（避免 L2 滚成 L3）**：
1. **gemini 分支留接口不实装** —— 加 `cli_session_id: Option<String>` 字段就好，gemini-cli 集成时填上即可
2. **worktree 默认不销毁** —— `Fuxi::shutdown` / `shutdown_agent` 都只 stop process **不**调 `workspace.destroy`，让 worktree 跨 daemon 重启留作召回 stash。物理清理由 `fuxi worktree clean`（v1.2）显式做。这是行为变化，从"agent 死亡 = worktree 回收"改成"agent 死亡 = worktree 留地上"
3. **玄女 session resume 不接入 sink** —— 她跨重启的单例语义和门客 recall 不一样，强行统一会把 session.rs 改乱。`session.rs::resolve_xuannv_session` 保留独立路径
4. **codex 进 sink 但只记 worktree** —— `cli_session_id=None`，`--recall-task` 命中 codex 时走"复用 worktree + 新 spawn"语义；warn 一下但不报错
5. **borrowed worktree handle** —— 召回 spawn 用 `WorkspaceHandle.borrowed=true`；`workspace.destroy` 看到 borrowed 就 noop，避免下次也召回时把 worktree 删掉

**已完成的实装**：commit 跟在本 doc 后；档 1+2+3 全过——档 3 luban 第二轮答出 `/tmp/recall-test.txt`，证明 cc session resume 真接上了第一轮 history。

## 反驳点提前排雷

> "为啥 trait 不直接加 `worktree(&self) -> Option<PathBuf>`？"

不加。worktree 是**编排层**的概念（fuxi-workspace 分配的，agent 根本不该知道自己在哪个 worktree）。Fuxi.shelf 里有 `worktree_of(agent_id)`，pump 在调 sink 时去 shelf 里取——sink 拿到的是已注入的 context。

> "为啥 RecallContext 不放 fuxi-core？"

放 fuxi-orchestrator——它是召回入库**编排层**的契约。fuxi-core 的 trait 应保持纯（Agent/Task/Event 这些原语）。session_id 进 trait 已经是边缘选择，再塞 RecallContext 就走偏了。

> "玄女不用 sink 不会日后混乱吗？"

不会——session.rs 的 fact 用 `subject=xuannv`，sink 写的用 `subject=task-<id>` / `role-<role>`。subject 命名空间隔开了。

## 影响面

破坏性改动：
- `RecallSink` trait 签名 break（**外部无 user**——只有 fuxi-cli 的 `OracleRecallSink` impl）
- `Fuxi::set_recall_sink` 签名不变
- α 工作单元的 `CapturingRecallSink` 测试要改（dispatch.rs 内 3 个测试）
- 主线 `OracleRecallSink` 3 个测试要改 + 加 codex worktree-only 路径测试
- `fuxi spawn --recall-*` CLI flag 不变（用户视角无感）
- 玄女 skill 教学不需改（语义没变）

新增：
- `Fuxi::spawn_worker_in_worktree(profile, kind, worktree: PathBuf)` API（绕过 allocate）
- `RecallContext` struct 在 `fuxi-orchestrator::recall`
- sink 多写 `predicate=worktree` fact（codex 也能用）
- worktree GC 保护（最简："仍是 agent 持有"）

## 检验通过的标准

档 1 + 档 2 全过（兼容性）+ 档 3 真 e2e 跑通（cc 答出 `/tmp/recall-test.txt`）+ 加一个 codex 召回回归（只验 worktree 复用，不验 session 接续）。
