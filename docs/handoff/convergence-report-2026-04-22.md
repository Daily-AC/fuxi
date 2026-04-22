# Convergence Report（2026-04-22）

## 改了什么
- 上下文治理：建立唯一入口 `docs/handoff/now.md`，历史 handoff 全标注 `stale`。
- 执行治理：新增 `docs/handoff/context-reset-checklist.md` 与 `docs/handoff/whitelist-files.txt`。
- 语义收口：`dispatch_to_any` 从“legacy 复用 idle”收口为“legacy 壳，内部转 task-bound”。
- CLI 派工对齐：`dispatch` 增加 `task_id` 分支，走 `dispatch_in_task`。
- 测试同步：orchestrator dispatch 相关用例改为断言 task-bound 行为。

## 证据
- 命令门禁：`cargo fmt --all`、`cargo test --workspace` 全部通过。
- 关键测试：
  - `dispatch_to_any_is_legacy_shell_and_spawns_task_bound_worker`
  - `concurrent_dispatch_to_any_spawns_distinct_task_bound_workers`
  - `dispatch_in_task_can_fan_out_same_parent_task_to_multiple_workers`
- 关键提交：
  - `569631a` docs(handoff): 新增now单页真相并记录上下文清理
  - `e9c03e5` docs(handoff): 标注历史stale并新增context reset执行板
  - `3e20522` fix(task-bound): 收口legacy dispatch并恢复清理后编译基线

## 剩余 1 条
- 继续基础层收尾：把“白名单开发”固化为日常流程（提交前自动检查变更是否越界），彻底避免后续上下文回污染。
