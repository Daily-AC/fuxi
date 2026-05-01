# Decision 19 · Karpathy Coding Guardrails

**日期**：2026-05-01
**状态**：已采纳

## 背景

触发来源：<https://github.com/forrestchang/andrej-karpathy-skills/>

该仓库把 Andrej Karpathy 对 LLM coding 常见问题的观察整理成 Claude/Cursor
行为指南。核心问题包括：LLM 会静默假设、过度抽象、顺手改无关代码、缺少
可验证成功标准。

Decision 18 规定了 Fuxi 的 Agentic Engineering 协作原则。本决策把它落到
每次编码任务的执行护栏。

## 决策

Fuxi 后续代码开发默认遵守四条护栏：

1. **先钉成功标准**：每个非平凡任务都要有可验证目标，而不是只描述动作。
2. **最小充分实现**：不做未要求的功能、配置、抽象和未来兼容层。
3. **精确改动边界**：每一行 diff 都应能追溯到当前任务。
4. **困惑显性化**：真正影响方向的歧义必须先暴露；可逆细节由实现者直接判断。

## 对 Fuxi 的具体含义

### 成功标准优先

以后每个工作单元至少回答：

- 行为成功靠什么验证？
- 回归靠什么测试固定？
- 文档口径是否需要同步？
- CI 或本地门禁跑哪一组？
- 产物是否在对应 worktree / sandbox 内可审计？

例：不要把“加 sandbox”理解成“改 spawn 参数”。成功标准应是：

- cc/codex 子进程的 HOME/TMP/cache 被收拢进 agent worktree；
- 任务结束后审计事件能列出新增/修改/未跟踪文件；
- 逃逸路径被记录或阻断；
- 相关 adapter 测试覆盖 cc 与 codex。

### 最小充分实现

Fuxi 已有 `dist.rs`、`daemon.rs`、`repl.rs` 膨胀风险。新增能力时：

- 不为单一调用点引入大 framework。
- 不提前支持未接入的 adapter。
- 不把“以后可能需要”做成泛化配置。
- 如果新代码需要大量胶水，优先重审边界，而不是继续堆抽象。

复杂度必须服务当前闭环：workspace、sandbox、audit、recall、dist routing。

### 精确改动边界

默认不做顺手清理。允许清理的范围：

- 本次改动造成的 unused import / dead helper / 失效测试 fixture。
- 与当前行为直接冲突的旧文档口径。
- 为了让当前测试通过必须调整的局部接口。

不允许把无关格式化、命名偏好、历史死代码、邻近重构混进同一提交。

### 困惑显性化

需要停下来确认的情况只有一种：继续做会产出与用户意图相反的结果。

不需要停下来的情况：

- 可逆实现细节；
- 项目已有模式能给出答案；
- 测试能判断对错；
- 文档已经给出主线口径。

对于可逆细节，Codex 直接选择与代码库一致的方案，并在结果报告里说明理由。

## Review 清单

每次 review 先看这些问题：

- 是否存在未声明的产品/安全/权限假设？
- 是否有“为了未来”加出的配置、trait、adapter、状态机？
- diff 是否包含无关重构或格式漂移？
- 测试是否验证了真实风险，而不是只测 happy path？
- 文档是否和代码同一口径？
- 工作产物是否能归属到具体 agent/worktree/node？

## 和 Decision 18 的关系

Decision 18 是协作层：用户定目标和边界，Codex 交付可验证闭环。

本决策是执行层：Codex 在实现时必须压假设、压复杂度、压 diff 面积，并把
成功标准变成测试和审计证据。

## 何时重审

- 引入自动 red-team / multi-agent review 门禁后。
- sandbox/audit 成为默认强制策略后。
- Fuxi 支持更多 CLI adapter，且不同 adapter 的 workspace 语义明显分叉时。

