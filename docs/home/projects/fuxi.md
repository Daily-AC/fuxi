# fuxi

> Rust 个人 AI agent 编排平台。玄女作为顶层 agent，调度 cc / codex / gemini-cli 门客干活。毕设主体 + 长期日常使用品。

## 状态
- 当前阶段：building（v1.1 roadmap 推进中）
- 主仓库：https://github.com/Daily-AC/fuxi
- 最近大事件：2026-06-30 home 重装为 Win11 + 公网入口迁 Caddy

## 部署
- 主跑机：当前**仍在老 Linux home**（已退役，重装为 winhome），fuxi-im.service 需迁
- 用到的 service：[[caddy]]（未来 reverse_proxy 到 fuxi-im backend）、[[sshd]]
- 子域名：`fuxi.qmledmq.cn` / `im.qmledmq.cn`（走 `*.qmledmq.cn` wildcard）

## 入口
- PWA：`https://im.qmledmq.cn:8443/`（待新 home 部署完接上）
- 开发：`cd /Users/e0_7/fuxi; cargo run -p fuxi-cli`

## 关键路径
- 源码：`/Users/e0_7/fuxi/`（mac），`~/fuxi/`（home，rsync 落地）
- events.db：`~/.local/share/fuxi/events.db`（home）
- IM web dist：`~/.local/share/fuxi/im-web/`（rsync from mac dist/）
- 凭据：CC token / Codex token / 飞书 / 讯飞 / FCM / VAPID / HMAC——见 [refs/secrets-locations.md](../refs/secrets-locations.md)

## 依赖
- 上游：cc / codex / gemini-cli（门客 binary）
- 外部 API：CC / Codex / FCM push / GPT-SoVITS 自托管 / 讯飞唤醒
- workspace crate：fuxi-core / events / a2a / orchestrator / im / agent-cc / agent-codex / firehose / cli / 等

## 已知 issue / 待办
- 🔴 **fuxi 服务从老 Linux 迁到 winhome / WSL Ubuntu 24.04**（block，home 重装后未迁）
- 详路线图：[../../architecture-v1.1-roadmap.md](../../architecture-v1.1-roadmap.md)

## 引用
- 设计：[../../superpowers/specs/2026-04-19-伏羲-design.md](../../superpowers/specs/2026-04-19-伏羲-design.md)
- 决策：[../../decisions/](../../decisions/)
- 路线图：[../../architecture-v1.1-roadmap.md](../../architecture-v1.1-roadmap.md)
- handoff：[../../handoff/](../../handoff/)
- 工程规范：项目根 CLAUDE.md
