# 参考项目 · 读书笔记

> 伏羲设计/实现过程中值得读、借鉴、clone 下来对照的外部项目。
>
> 公理 #6（CLAUDE.md）：**借鉴设计智慧，不借代码路径**。语言不同（TS vs Rust）天然隔离，读源码是学思路，不是抄。
>
> 链接未验证的标 `(需验证)`，用户确认后去掉标记。

---

## 本地已有副本（可直接 grep / 读）

### ComposioHQ/agent-orchestrator · `/Users/e0_7/_refs/agent-orchestrator/` **← fuxi 真正对标物**

**独立 repo**，不是 `composio` 主仓的子目录。fuxi CLAUDE.md / 设计文档里"借设计智慧"指的就是这个。

| 关键路径 | 是什么 |
|---|---|
| `ARCHITECTURE.md`（7K 行） | 架构总览，必读 |
| `DESIGN.md`（24K 行） | 详细设计 |
| `CLAUDE.md`（23K 行） | 项目自身规范，结构可参考 |
| `packages/core/src/types.ts`（1722 行，**核心接口全在这一个文件**） | 7 插件槽 trait + 30+ 事件类型 + 状态机全都在这 |
| `packages/core/src/lifecycle-manager.ts`（1997 行） | 轮询循环全貌（行 1973：`start(intervalMs = 30_000)` 默认 30 秒，不是我们原本以为的 5 秒） |
| `packages/core/src/lifecycle-state.ts` | 三维状态机 schema（session × pr × runtime） |
| `packages/core/src/plugin-registry.ts`（527 行） | 动态 registry 契约 |
| `packages/plugins/agent-claude-code/` | cc 适配器参考（最相关） |
| `packages/plugins/runtime-tmux/` | Runtime 参考 |
| `packages/plugins/` 共 22 个 | 7 插件槽的 official 实现 |

**composio ↔ fuxi 精确对标**见本文下方的"对标映射表"。

### ComposioHQ/composio · `/Users/e0_7/_refs/composio/`

**跟 fuxi 几乎无直接映射**。这个是 composio 公司的 API tool ecosystem 产品，和 agent-orchestrator 是两个项目。放在这只是为了备查，别把时间花在读主仓上。

### team-anya · `/Users/e0_7/team-anya/`
用户自己的 TS agent broker 项目。伏羲最直接的参照源，很多坑已被踩过。

| 关键路径 | 给 fuxi 什么 |
|---|---|
| `apps/server/src/broker/backends/claude-code-backend.ts` | cc `--sdk-url` WS 反连模式完整实现（fuxi-agent-cc 升级的 reference） |
| `apps/server/src/broker/backends/` | 多 backend 架构（claude-code / opencode / kimi-code / pi） |
| `apps/server/src/broker/role-mcp-server.ts` | 把工具层封装成 MCP HTTP server 注入给门客 |
| `apps/server/src/loid/` | 长期记忆系统（brain / chat-memory-extractor / person-memory-consolidator / profile-updater） |
| `apps/server/src/franky/` | 编排层（topic-router / context-builder / franky-orchestrator） |
| `apps/usage-reporter/` | 成本/限流统计 + 降级策略 |
| `apps/server/src/broker/prompt-assembler.ts` | 动态 prompt 拼装（玄女招安新 role 时可参考） |
| `docs/核心功能/02-AgentBroker多后端架构.md` | 多后端抽象设计文 |
| `Dockerfile.runtime` | Docker 沙箱参考（门客容器化时用） |
| `CLAUDE.md` | anya 项目自身的规范 |

---

## 要 clone / 在线读的

### cc 反连 / WS 模式

- **"companion"** · 源码位置不明（待定位）
  - anya 的 `claude-code-backend.ts` 里 15+ 处注释引用，给的是 `cli-launcher.ts:LN-LN` / `ws-bridge.ts:LN-LN` 具体行号
  - 功能：Claude Code CLI `--sdk-url` WebSocket 反连模式的宿主实现（spawn / 队列 / 工具审批 / 消息路由 / 会话恢复 / 生命周期）
  - 可能来源（按概率）：Anthropic 内部 SDK reference / `@anthropic-ai/claude-agent-sdk` 打包前的源 / `anthropics/claude-agent-sdk-demos` 其中之一
  - **实操**：读 anya 的 `apps/server/src/broker/backends/claude-code-backend.ts`（1066 行）已是 companion 的完整二次呈现版，不一定要找到原源
- **Anthropic Claude Agent SDK (TS)** · https://github.com/anthropics/claude-agent-sdk-demos
  - 公开 demo 集合，可能含 WS / 反连示例
  - npm 包：`@anthropic-ai/claude-agent-sdk`
- **Anthropic Claude Agent SDK (Python)** · https://github.com/anthropics/claude-agent-sdk-python
  - Python 实现，对伏羲 Rust 实现是对比素材

### 沙箱

- **openai/codex** · https://github.com/openai/codex
  - `codex-rs/macos-seatbelt/` — macOS sandbox-exec profile（fuxi 直接抄）
  - `codex-rs/linux-sandbox/` — Linux landlock + seccomp
  - 同时也是 codex CLI 本体，我们的 `fuxi-agent-codex` 适配器就喂它
- **containers/bubblewrap** · https://github.com/containers/bubblewrap
  - Linux 用户态沙箱基础（Flatpak 底层）
- **All-Hands-AI/OpenHands** · https://github.com/All-Hands-AI/OpenHands
  - Docker sandbox 策略 + runtime 容器化参考

### 自定义 agent / role 系统（"招安"机制偷学）

- **sst/opencode** · https://github.com/sst/opencode
  - TS 开源，SST 出品
  - agents / commands / modes 系统完整开源
  - 偷其 profile 格式 + 触发机制
  - 同时也是我们未来 opencode 适配器的喂料对象
- **openai/codex** · （同上，复用）
  - codex 的 agent profile 配置系统

### A2A 协议

- **A2A canonical spec** · https://github.com/a2aproject/A2A
  - Linux Foundation 下的开源协议（Google 捐赠给 LF）
  - 旧址 `google/A2A` 是同一项目迁移前的地址
  - v1.0 协议原文，fuxi-a2a 的 schema 基线
- **A2A Python 官方 SDK** · https://github.com/a2aproject/a2a-python
  - Rust 没有官方 SDK（所以我们自实现——ADR-002 依据）
  - 当对比学习材料：schema 取名 / 错误码映射 / streaming 细节
- **A2A samples** · https://github.com/a2aproject/a2a-samples
  - 多个实例代码，适合当验证 fuxi-a2a 兼容性的参考

### 设计智慧（只读思路，不抄代码）

- **ComposioHQ/agent-orchestrator** · https://github.com/ComposioHQ/agent-orchestrator
  - **独立 repo**，已 clone 到 `/Users/e0_7/_refs/agent-orchestrator/`
  - 昨晚审计的具体 commit：`509fb12`（PR #1300 merge 点）
  - 7 插件槽分类法 / 事件词汇表 / 三维状态机的直接来源
  - 参见下方"对标映射表"精确到文件/行号
- **ComposioHQ/composio** · https://github.com/ComposioHQ/composio
  - 主仓，业务是 API tool ecosystem，**和 fuxi 无直接映射**
  - 放在参考清单里只为避免名字混淆
- **Anthropic Agent Skills · 开源公共协议** · **极其相关**
  - 规范仓库：https://github.com/agentskills/agentskills
  - 官方规范站：https://agentskills.io/specification
  - Anthropic 参考实现：https://github.com/anthropics/skills
  - License：Apache 2.0 (code) + CC-BY-4.0 (docs)
  - 已采纳：Microsoft (VS Code) / Cursor / Goose / Amp / OpenCode……
  - **对伏羲的意义**：玄女"招安"生成的 role profile **应直接用 agentskills 格式**（folder = system prompt + instructions + resources + metadata）。这样 cc / codex / opencode 门客读同一份 Skill → profile 跨门客天然复用。不是"借鉴"，是**应采纳为 profile 格式标准**。

---

## 记录原则

- **链接 + 关键子路径** —— 项目大的给出 "进去先看哪个目录"
- **价值一句话** —— 说明"它给 fuxi 什么"，避免日后忘记为啥收藏
- **`(需验证)`** —— 我不 100% 确定的链接，用户核实后去标
- **追加制** —— 新读到的加进来；废弃的标 `(已弃)` 但不删（留着知道为啥弃）

## 未分类 / 待补

- （留空）本处给下次读到值得记的项目临时落脚，定期归档到上面的分区

---

## composio agent-orchestrator ↔ fuxi 对标映射表

### 7 插件槽

| composio（`packages/core/src/types.ts`） | 它是什么 | fuxi 对应 | 姿态 |
|---|---|---|---|
| `:391` `interface Runtime` · create/destroy/sendMessage/isAlive | Runtime 插件抽象 | `crates/fuxi-core/src/runtime.rs` `trait Runtime` | **借 trait 形状**，砍掉 sendMessage/getOutput（由 A2A + EventBus 替代） |
| `:456` `interface Agent` · getLaunchCommand/detectActivity/getActivityState/getSessionInfo | Agent 适配器 | `crates/fuxi-core/src/agent.rs` `trait Agent` | **借名字和 "per-CLI 适配器"思想**；签名重设计：fuxi 是任务驱动 (dispatch/send_message/cancel/shutdown)，composio 是"启动+轮询探测"驱动 |
| `:601` `interface Workspace` · create/destroy/list/postCreate | worktree 隔离 | `crates/fuxi-core/src/workspace.rs` `trait Workspace` | **几乎 1:1 借** |
| `:644` `interface Tracker` · GitHub Issues/Linear/Jira | 需求源 | **fuxi 无对应** | 砍掉。玄女任务来源只是"用户说" |
| `:718` `interface SCM` · PR + CI + Reviews + Webhook（~80 方法） | SCM 集成 | **fuxi 无对应** | 砍掉。fuxi 不做 SCM（composio 最胖的 interface） |
| `:994` `interface Notifier` · notify/notifyWithActions | 推送给人 | 无专门 trait | 折成两层：`fuxi-firehose`（实时观察） + 未来抄送机制 |
| `:1028` `interface Terminal` · 打开 iTerm/web tab | 让人接管 | **fuxi 无对应** | ratatui TUI 替代，不打开外部终端 |

### 事件词汇

| composio | fuxi | diff |
|---|---|---|
| `types.ts:1049` `type EventType` 字符串联合（29 变体：session.* × 8, pr.* × 4, ci.* × 4, review.* × 5, automated_review.* × 2, merge.* × 3, reaction.* × 2, summary.* × 1） | `crates/fuxi-core/src/event.rs` `enum EventKind` tagged union（~25 变体） | `event.rs:7` 注释明注 "Inspired by ComposioHQ's 30+ OrchestratorEvent variants"。**形状借、变体不一样**：砍 pr/ci/review/merge 整个子集，加 A2A 对话类（MessageSent/Received、UserInterventionSent、OrchestratorCcReceived、ConversationTransferred）——这些是 fuxi 架构独有 |
| `OrchestratorEvent { id, type, priority, sessionId, projectId, timestamp, message, data }` | `Event { meta: EventMeta, kind: EventKind }` | fuxi 去掉 priority（push 一律上 bus）；加 `agent` / `task` 维度；Rust enum 让 firehose 的 match 有穷尽检查 |

### 状态机

| composio | fuxi |
|---|---|
| `packages/core/src/lifecycle-state.ts` **三维状态机**：`session.state`（8 态）× `pr.state`（4 态）× `runtime.state`（5 态），每维配 `reason` 枚举 | `crates/fuxi-core/src/task.rs` **单维** `TaskState`（8 态：New/Ready/InProgress/AwaitingInput/Delivering/Done/Blocked/Cancelled）+ `can_transition_to()` |
| 转场带 `reason` 字段（共 16 + 9 + 6 = 31 个 reason 枚举值） | fuxi 转场**无 reason 字段**（可能未来需要） |
| composio 同时追 agent 活性 / PR 状态 / 任务进度，三条轴独立推进 | fuxi 目前只追 task；agent 状态只在 `agent.rs:45` 有个扁平 enum，没持久状态机 |

### 轮询 vs. 推送（fuxi 独创赌注 #1 的精确对标）

| composio | fuxi |
|---|---|
| `lifecycle-manager.ts:1973` `start(intervalMs = 30_000)` · 默认 **30 秒**轮询 `pollAll()` · web 前端另加 ~5s 刷新 | `crates/fuxi-events/src/bus.rs` · tokio broadcast + SQLite WAL replay · **全 push** |
| `populatePREnrichmentCache` 批量预取 + 审计 backlog 2 分钟节流 | 不轮询就**没有这些节流代码** |
| `Agent.detectActivity(terminalOutput)` 解析 tmux 输出 + `.ao/activity.jsonl` 回读（`activity-log.ts`） | cc headless stream-json（见 memory `reference_cc_stream_json`），事件直接进 EventBus |

> **订正**：原 CLAUDE.md / 设计文档说"composio 5 秒轮询"是错的。精确值是 **30 秒**（lifecycle 核心循环）；5 秒是 web 前端刷新。已更新各处引用。

### Session/Worker 管理

| composio | fuxi |
|---|---|
| `session-manager.ts`（2752 行！）+ `types.ts:1562` `interface SessionManager`（spawn/restore/kill/cleanup/send/claimPR 等 15+ 方法） | **fuxi 还没有 SessionManager** —— Agent trait 直接 dispatch，没有独立的会话生命周期抽象 |
| session 元数据存成 `worktree/.ao/*.meta` flat file（`metadata.ts`，key=value） | SQLite 单一真相源（WAL + append-only）—— **架构差异**：composio 双写（文件系统 + 进程状态），fuxi 单写 |
| 插件通过 `plugin-registry.ts`（527 行）按 `PluginSlot × name` 注册 | 通过 crate 静态链接，无 registry |

### LoC 测试比实数核对

| 指标 | composio 核心 | composio 全 packages | fuxi |
|---|---|---|---|
| src / tests LoC 比 | 1.208 | 1.271 | ~1.1（OVERNIGHT 记录） |

文档里说的 1.41× 对不上——可能是早期子目录快照。**修订**："对标 composio 的测试覆盖"在量级上 1.2-1.3 已算及格，不必死盯 1.41。

### 一句话 diagnosis

**composio = CI/PR-centric 的轮询编排器**（以 SCM 为核心、Notifier 通知人、30s polling + tmux 输出解析）  
**fuxi = A2A-centric 的 push 对话编排器**（以玄女-门客会话为核心、EventBus 直连 Firehose、tokio broadcast + stream-json）

砍掉 Tracker/SCM/Terminal 三个槽、三维状态机、30s 轮询；换进 A2A 协议、ConversationSwitch/InterventionProxy 语义、纯 push 推送。Runtime/Agent/Workspace 的 **trait 形状借了但编程范式完全不同**：fuxi 是事件流式 (`Agent.dispatch(Task) -> Receiver<Event>`)，composio 是 shell 命令+轮询探测 (`Agent.getLaunchCommand() + detectActivity()`)。
