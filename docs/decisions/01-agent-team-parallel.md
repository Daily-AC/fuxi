# Decision 01 · 并行 agent team 用独立 cc 进程，不用 subagent

**日期**：2026-04-19  
**状态**：已采纳，v1 全程使用

## 背景

用户原话：「你接下来起 agent team 去并行的推进，快速收敛到蓝图。**不是你起的 subagent 而是直接调用多个 cc**」。

我（主线 Claude Code）有两种并行方式：
1. **Task tool subagent**（`general-purpose` / `Explore` 等内置）—— 在我当前进程里跑，共享我 context 但独立 run
2. **独立 cc 进程**（`claude -p "$PROMPT"` 后台 + git worktree）—— 完全独立的 claude 实例，自己的 context/会话

## 决策

**编码任务用独立 cc 进程 + git worktree 隔离**；**研究/综述任务可用 Task subagent**（它偏向信息搜集）。

```
研究层 (Task subagent)  → 扫 repo / survey / 选方案 → 产出 md
                        ↓
编码层 (claude -p 后台) → 实装 / 写测 / 跑门禁 / commit → 产出 commit hash
                        ↓
聚合层 (我主线)         → cherry-pick / 解冲突 / 合到主分支
```

## 理由

1. **用户意图**：他明确区分了「subagent」vs「多个 cc」，要的是后者——独立上下文、真并行、不污染主线 context
2. **编码 scope 大**：C1-C5 每个改 1000+ 行 Rust，subagent 在我 context 里跑会占用我所有 token；独立 cc 开自己 context
3. **Worktree 天然隔离**：`git worktree add ../fuxi-c1 -b feat/x` 让每个 cc 在独立目录独立分支，文件系统级别不会踩
4. **失败可隔离**：一个 cc 挂了不影响其他；可独立 review / 重跑

## 代价

- **API 成本高**：每个 cc 独立会话，token 消耗是 subagent 的 3-5 倍
- **难度**：起 cc 要手动清 `CLAUDECODE*` env + 设 `NO_PROXY`（Clash TUN 坑）+ 写 `/tmp/fuxi-cN-done.txt` 通知机制
- **合并冲突**：多个 worktree 改 `crates/fuxi-cli/src/main.rs` / `repl.rs` 导致手工 resolve（C3/C5 都加 Command enum 变体就冲突）

## 何时不适用

- 只是信息搜集 → Task subagent 更便宜快
- 只有单个模块小改 → 我自己做更快（无需 worktree + assignment + monitor）

## 参考实施

- C1-C5 并行：`docs/session-review-2026-04-20.md` §2
- Fix-A/B/C 合并：同上 §3
- 起 cc 脚本模板：`/tmp/start-c-team.sh`（跑完后删）
