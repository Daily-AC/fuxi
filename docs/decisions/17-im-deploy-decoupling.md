# Decision 17 · IM 部署解耦 · `fuxi-im` 拆独立 binary + systemd unit（中期排期，非本会话实装）

**日期**：2026-04-26
**状态**：方向已采纳，**实装排期 v1.x 之后**（本会话不做）
**触发**：用户 2026-04-26 反馈："现在部署 IM 需要把整个伏羲系统都重新构建部署吗？如果是的话，那是一个糟糕的设计。"

## 背景

当前 fuxi-im 跟 fuxi 主 daemon 的耦合关系：

```
fuxi-cli (单 binary)
  ├─ subcommand: fuxi up        → 跑主 daemon (玄女 + workers + EventBus)
  ├─ subcommand: fuxi im start  → 跑 IM axum server (端口 9100)
  └─ ...
```

`fuxi im start` 跟主 daemon **共进程 / 共 binary**。生产部署里：

- `fuxi-im.service` (systemd) → 跑 `fuxi im start` → 起一个 fuxi-cli 进程，里面同时有玄女 + IM
- 改 IM 代码 → 重 build fuxi-cli → 替换 binary → systemd restart fuxi-im → **整个进程重启 → 玄女死 → 用户对话上下文断**

## 痛点分层

用户原话有两层概念，需分清：

| 层次 | 现状 | 是否真痛 |
|---|---|---|
| **workspace build** | `cargo build -p fuxi-cli` 增量编译只编改了的 crate | OK，1min 范围内，不是真痛 |
| **部署 binary** | IM 跟主 daemon 同一个 fuxi-cli binary | 痛点中等：换 IM 必换全 binary |
| **进程重启** | systemd 重启 fuxi-im → 玄女 session 断 | **真痛点**——用户对话上下文丢失 |

## 决策（中期方向，非本会话实装）

### 拆 `fuxi-im` 成独立 binary + 独立 systemd unit

```
┌────────────────────────────────┐    ┌────────────────────────────────┐
│ fuxi-daemon.service            │    │ fuxi-im.service                │
│ exec: fuxi-cli up              │    │ exec: fuxi-im                  │
│   - 玄女 (xuannv)              │    │   - axum server :9100          │
│   - workers                    │◄──►│   - PWA 静态文件               │
│   - EventBus                   │    │   - WS clients                 │
│   - A2A server                 │    │   - A2A client → daemon        │
└────────────────────────────────┘    └────────────────────────────────┘
   长寿命，重启代价大                       短寿命，重启代价小
   改 IM 不动                              改 IM 仅重启此进程
```

通信路径：fuxi-im 通过 **fuxi-a2a JSON-RPC**（已有 crate）跟 daemon 对话。EventBus subscribe 也走 a2a streaming。

### 实装拆活（v1.x 后排期）

| 阶段 | 工作 |
|---|---|
| 阶段 1 | 把 `fuxi-im` 从 fuxi-cli 子命令拆成独立 binary `fuxi-im`（仍同 workspace；fuxi-cli 可选保留 `fuxi im start` 作为 forward shell wrapper 兼容旧脚本）|
| 阶段 2 | fuxi-im 改为通过 fuxi-a2a JSON-RPC 跟 daemon 通信（替代当前直接持有 `Fuxi` 句柄的实现）|
| 阶段 3 | 拆 systemd unit：`fuxi-daemon.service` + `fuxi-im.service` 独立；fuxi-im 依赖 daemon （`After=` + `Wants=`）|
| 阶段 4 | install.sh 增加部署模式：`--apply --im-only`（仅换 fuxi-im binary + restart fuxi-im.service，玄女不动）|
| 阶段 5 | TUI（fuxi up）也走 a2a 客户端 → IM 与 TUI 完全等价为"另一种 a2a 客户端"，"PWA 跟 TUI 信息等价"（用户 #3 反馈）在架构层兑现 |

## 砍掉 / 没考虑过的方案

| 方案 | 否决理由 |
|---|---|
| 短期内强行拆（本会话做）| 工作量 ≥ 1 天，跟当前任务=群聊重设计冲突，且当前 fuxi-cli 共进程不 fatal（仅是断 session）|
| 整 monorepo 重组成多 service / multi-process（如 microservices）| over-engineering；fuxi 体量不需要 |
| IM 进程内做"hot reload"（不重启进程换代码）| Rust 不友好，复杂度高，回报低 |
| daemon 跟 IM 用 unix socket / shared memory | 已有 a2a JSON-RPC，重复造轮 |

## 关联

- [决策 16](16-im-tab-bar-task-thread.md) —— 同时由用户提出
- [决策 12](12-dist-worker-true-concurrency.md) —— 远期分布式架构里 fuxi-im 跟 daemon 解耦更重要（IM 可能在网关节点跑，daemon 在工作节点跑）
- 公理 4（CLI 是工具层的唯一形态）—— 拆 binary 时保持 fuxi 工具调用走 shell 不走 MCP
- 公理 7（毕设不是 DDL）—— 部署解耦是长期工程优化，不为毕设答辩压缩
