# NOW · 单页真相（2026-04-22）

## 已完成（带证据）
- task-bound 主干落地：同父任务 fanout 相关路径已在 orchestrator/cli/repl 接通。
  - 参考提交：`5deedf7`、`c5cdf04`、`3827d76`
- 回传尾延迟做过一轮收敛（terminal drain 默认窗口下调）。
  - 参考提交：`4d87cff`
- TUI 自动贴底回归测试已补（长消息换行场景）。
  - 参考提交：`9c62620`

## 进行中（唯一目标）
- 基础层收尾与上下文去噪：
  - 清理 legacy 派工语义（尤其 `dispatch_to_any` 的遗留表述/入口）
  - 收敛文档入口到本文件，避免 handoff 多版本来回跳

## 下一步（最多 3 条）
1. 建立“代码白名单”开发模式：仅在以下文件推进并提交
   - `crates/fuxi-orchestrator/src/fuxi.rs`
   - `crates/fuxi-cli/src/daemon.rs`
   - `crates/fuxi-cli/src/repl.rs`
   - `crates/fuxi-orchestrator/tests/dispatch.rs`
   - `crates/fuxi-cli/tests/*task*`
2. 对 legacy 入口做硬性约束（deprecate + 测试锁定），避免新旧语义并存。
3. 跑基线验证并固定报告模板：`fmt + clippy -D warnings + test(workspace)`。

## 明确不做（本轮）
- 不再做“仅状态同步”的文档提交。
- 不扩新 UI 主题/视觉分支（先收口语义和编排层）。
- 不引入新的并行调度抽象（先把当前 task-bound 语义闭环）。

## 上下文清理记录
- 已执行可逆清理：`git stash push -u -m "ctx-reset-2026-04-22T22:08+08:00"`
- stash：`stash@{0}`（需要恢复时可 `git stash apply stash@{0}`）
