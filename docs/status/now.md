# Now Status (Live Snapshot)

更新时间：2026-04-30 CST
分支：`main`
HEAD：以当前 `main` 为准，运行 `git rev-parse --short HEAD` 获取精确提交。
状态口径：以当前代码和本地验证为准；历史 handoff 只作背景。

---

## 结论

当前主线是 **IM v4 + dist 接通后的修复收敛态**：

1. Rust workspace 编译和测试闭环可用。
2. IM PWA 前端 typecheck / lint / unit tests 可用。
3. `fuxi im start` 是 home 部署组合根：同进程持有 EventBus、Fuxi、scheduler、IM API/PWA、dist controller。
4. 玄女自启是 IM 可用性的硬前置；自启失败不应继续提供半可用 HTTP 服务。
5. 长期记忆默认走显式 `fuxi memory record`；自动 extractor 是 opt-in。
6. `feat/fuxi-v0.1` 已合入 `main` 并删除；后续按 `docs/git-workflow.md` 走短分支。

---

## 当前架构事实

### 编排与事件

- `fuxi-orchestrator` 的旧 `dispatch_to_any` 已是 legacy 兼容壳。
- 主路径是 `dispatch_in_task` / `dispatch_to_any_in_task` / `dispatch`。
- `AgentDead` 状态不会再被 dispatch pump 退回 Idle。
- `SystemEventBridge` 负责把 `AgentRequestReview`、`AgentDead`、`TriggerFired` 等系统事件注入玄女注意力。

### IM 与 dist

- `fuxi im start` 内嵌 dist controller。
- `/api/*` 走 IM cookie auth。
- `/dist/*` 走 HMAC auth。
- `/api/nodes` 通过 `NodesProvider` 读取 dist topology。
- `Fuxi::dispatch` 命中 `pinned_node` 或 `required_tags` 时通过 `DistEnqueuer` 入 dist 队列。

### 记忆

- `fuxi-memory` 提供 `OracleStore` / `HetuStore` / extractor。
- extractor 默认关闭：`ExtractorConfig::default().enabled == false`。
- 自动抽取需 `FUXI_EXTRACTOR_ENABLED=1`。

---

## 当前验证

- `cargo test --workspace --all-targets`：通过。
- `crates/fuxi-im/web pnpm test`：29 files / 280 tests，通过。
- `crates/fuxi-im/web pnpm typecheck`：通过。
- `crates/fuxi-im/web pnpm lint`：通过。

---

## 仍需收敛

1. 文档归档：旧 `docs/handoff/v1-session*.md`、`docs/session-review-*.md`、`docs/audit/*.md` 应标注历史状态或移入 archive。
2. `fuxi-cli/src/repl.rs`、`dist.rs`、`daemon.rs` 已明显膨胀，后续应继续把组合层和领域逻辑拆开。
