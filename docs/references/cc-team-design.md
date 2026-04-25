# cc agent team 设计参考

> 不复制 cc 源码（闭源 + 更新快 + 跟 fuxi 架构差异大）。本文是**架构地图**，
> 让伏羲想借鉴 cc agent team 设计时直接定位到关键文件 + 看清概念映射。
>
> **本地路径**：`/Users/e0_7/Downloads/claude-code-source/`
> （CLAUDE.md 早期版本提到过 `/tmp/cc-source/`，那个是早期解压树；以下统一用 Downloads 路径）

## cc team 子系统规模

- 38 个 team-related 文件，~3k 行 TS
- 核心 5 个文件就够看懂模型，不用啃 38 个

## 核心 5 文件（按学习顺序）

| 顺序 | 路径（相对 cc 源码根） | 行数 | 看什么 |
|---|---|---|---|
| 1 | `src/utils/teamDiscovery.ts` | 81 | 入门：team / teammate 数据结构（最小） |
| 2 | `src/utils/teammate.ts` | 292 | 身份解析双轨：AsyncLocalStorage（in-process）vs CLI args（tmux）|
| 3 | `src/utils/teammateMailbox.ts` | 1183 | **核心**：消息总线 = 文件系统 + 类型化 envelope |
| 4 | `src/utils/swarm/teamHelpers.ts` | 683 | team 创建/cleanup/file lock 操作 |
| 5 | `src/tasks/InProcessTeammateTask/InProcessTeammateTask.tsx` | — | UI 视图（Ink + React），跟 fuxi 无关 |

## cc 的关键架构决策

1. **Mailbox = 文件系统**：`~/.claude/teams/<name>/` 存 team 配置；
   `~/.claude/tasks/<name>/` 存 task 列表 + mailbox。**不用 socket / broker / DB**。
   优雅简化：fs notify 就够；多进程也共享。
2. **两套 backend**：in-process (AsyncLocalStorage 上下文隔离) + tmux pane（CLI args
   传身份）。同一 team 可混跑两种。
3. **TypedEnvelope**：消息不是裸 string，是 typed union——`IdleNotification` /
   `PermissionRequest/Response` / `SandboxPermissionRequest/Response` /
   `PlanApprovalRequest/Response` / `ShutdownRequest`。**每种消息类型一段 schema**，
   接收方按类型分发。
4. **Leader-teammate 显式分离**：`team-lead` 名字硬编（`swarm/constants.ts::TEAM_LEAD_NAME`），
   teammate 列表把 lead 显式 filter 掉（`teamDiscovery.ts:50`）。
5. **idle notification 是 first-class**：teammate 闲下来主动 push leader——不是 leader poll。
   对应 `teammateMailbox.ts::createIdleNotification` 和 `useTeammateShutdownNotification.ts`。

## 概念映射 cc → fuxi

| cc | fuxi 对应 / 现状 | 借鉴判断 |
|---|---|---|
| team-lead | 玄女（xuannv） | 一致，无需改 |
| teammate | 门客（cc / codex / luban / ...） | 一致 |
| Mailbox（fs） | EventBus + SQLite events | **fuxi 已更先进**；events 表 + broadcast 是更好的 mailbox。不借 |
| TypedEnvelope | EventKind union | **正在做**：Decision 13 的 `AgentRequestReview` 沿这条路。**继续借**——把 PermissionRequest/PlanApproval 等 typed message pattern 加到 fuxi 的 Event 词汇 |
| Idle notification | `idle_gc.rs` 监听 idle | 有，但 cc 是**门客主动 push**，fuxi 现在是**编排层 poll**——可参考 cc 改成 push（公理 3 一致）|
| AsyncLocalStorage 身份 | `AgentId` + Worker process | 不借（语言/进程模型差异） |
| tmux pane backend | ratatui TUI | 不借 |
| `~/.claude/teams/<name>/config.json` | （fuxi 没有静态 team 概念）| 可借**思路**——M5 引入"persistent team"时这是参考 |
| `team_allowed_paths`（per-team 文件权限） | （fuxi 无）| **可借**——分布式 worker scope 限制时用 |

## 借鉴 / 不借的清单（落地建议）

**借**：
- TypedEnvelope 模式扩展 EventKind（PermissionRequest/Response、PlanApproval...）
- Idle notification 改 push 模型（teammate 主动 ping leader idle）
- per-team allowed_paths 思路用到 worker scope 限制

**不借**：
- 文件系统当 mailbox（fuxi EventBus 更适合分布式）
- AsyncLocalStorage 身份隔离（多进程模型用 AgentId 就够）
- tmux pane backend（ratatui 单 pane 走 task tree UI）

## 看 cc 源码的入口命令

```bash
# 全 team 文件总览
find /Users/e0_7/Downloads/claude-code-source -name '*team*' -o -name '*Team*' | grep -v node_modules

# 看消息类型
grep -E '^export (type|function (create|is)) ' /Users/e0_7/Downloads/claude-code-source/src/utils/teammateMailbox.ts | head -30

# 看 idle notification 触发链
grep -rln 'createIdleNotification\|isIdleNotification' /Users/e0_7/Downloads/claude-code-source/src
```

## 何时回头读这份

- 设计 `AgentRequestReview` 实装时（Decision 13）：照 cc 的 `PermissionRequest/Response` 配对模式抄结构
- 设计 fuxi"持久化 team"概念时（M5 后）：照 `~/.claude/teams/<name>/config.json` schema 抄字段
- 设计 worker scope 权限时：照 `team_allowed_paths` 抄
