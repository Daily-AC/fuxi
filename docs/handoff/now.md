# NOW · 单页真相（2026-04-26 · IM v2 重设计四件套全 ship）

> 上份 NOW 是同日早些时候关于 IM v2 中段。本份覆盖任务 sheet → 任务树 + 私聊页重设计的完整交付。

## 一句话

fuxi-im v2 完成"任务 sheet → 任务树 + active target 私聊页"重设计，HEAD `fda219b`，全套已部署到 home（`https://im.qmledmq.cn:8443`）。剩 β #23 multipart 400 等用户重传日志。

## 关键节点

- **设计 spec**：`docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md`
- **决策记录**：`docs/decisions/15-im-task-tree-pager.md`
- agent team：`fuxi-im-v1`（α/β/γ/δ/ε/ζ + team-lead），全员 alive
- 部署入口：`./deploy/im/install.sh --apply`（rust+web ~1min）/ `--web-only`（dist 30s，已 commit 固化 `df189eb`）

## 本会话提交（按时间，10 条）

```
fda219b ε v2 玄女顶栏 sticky badge "✓ 抄送 N 门客" (#31)  ← HEAD
9bd8ed6 β #23 upload multipart 详细诊断日志 + 0 字节支持 + PNG e2e
983a6bf ε v2 私聊页 modal C 方案橙色识别 + 任务 banner (#30)
41eb8d8 β #25 GET /api/tasks 字段补齐 + filter user-turn
0fbfc7b β #27 镜像端点 /api/workers/:agent_id/{events,conv}
c52c908 ε v2 任务树页 C 方案两级行卡片 (#29)
df189eb ζ install.sh 加 --web-only flag · PWA dist 快速部署路径
ee34f8d ε v2 重构 App shell · BottomSheet → horizontal pager + NavigationStack (#28)
0a707b8 docs · 决策 15 + CLAUDE.md codex follow-up 注释修措辞
d35f653 docs · IM 任务 sheet 重设计 spec
```

## 重设计四件套（全 ship）

| # | task | commit | 测试 |
|---|---|---|---|
| #28 | App shell · pager + NavigationStack | `ee34f8d` | 14 unit + 3 e2e |
| #29 | 任务树 C 方案两级行卡片 | `c52c908` | 8 unit |
| #30 | 私聊页 modal 橙色识别 + 任务 banner | `983a6bf` | 13+10 unit + 4 e2e |
| #31 | 玄女 sticky badge "✓ 抄送 N 门客" | `fda219b` | 6 unit + 3 e2e |
| #25 | 任务字段 + filter user-turn | `41eb8d8` | 8 unit |
| #27 | 镜像端点 `/api/workers/:id/{events,conv}` | `0fbfc7b` | 12 unit |

总计 165 PWA unit + 24 e2e + 174 backend unit 全绿。

## 进行中（in_progress 等用户）

- **#23 β · upload multipart 400** — `9bd8ed6` 加入入口 info log + 错误 chain Debug 输出 + 0 字节支持 + PNG e2e。**等用户重传一次 iPhone 上传**，β 拿 `journalctl -u fuxi-im | grep -E "upload 入口|multer_error_debug"` 排真因。lib 单测证明 axum::extract::Multipart 本身没 bug，剩下嫌疑是 nginx buffering / Safari iOS chunked / 中间代理改 ct。

## pending follow-up（5 条非阻塞）

- **#7** xuannv id polling → EventBus subscribe（违反公理 3）
- **#12** xuannv_bootstrap fact 应在 spawn 成功后才 insert
- **#19** 玄女→门客 dispatch 后 task_completed 事件丢失
- **#34** 私聊页支持多 running task tab（#N3 v1 简化）
- **#35** ToolCallCard stdout 前 20 行截断 + 全文按钮（#N3 v1 简化）

## 视觉语言（钉死，沿用决策 14）

- 暖暗底 `#1F1E1B`，Anthropic 橙 accent `#D97757`，奶白 `#F5F1E8`
- 角色色：玄女紫 `#C4A8E8` / 鲁班琥珀 `#E5A547` / 蒲松绿 `#A0C277`
- **不用 emoji** · **不用 Unicode block 装饰**（▎┌─└→… 全 TUI 语言，PWA 不抄）
- 触控热区 ≥ 44px
- 等宽字体仅 code block / 工具输出 / agent_id

## 重设计后的 PWA 心智模型（实装兑现）

```
┌─ Pager (horizontal swipe, 三页固定) ──────────────────────┐
│   Page 1 [节点]   Page 2 [玄女]   Page 3 [任务树]         │
│                       ↑              │                    │
│              ✓抄送badge tap          ▼ tap 门客行          │
│                       │              │                    │
│                       │   ┌──────────┘ navPush             │
│                       └──→ [私聊页 modal · 橙色识别]       │
│                            "‹ 玄女" pop / 边缘左滑 pop      │
└────────────────────────────────────────────────────────────┘
```

- composer 路由由"当前 page"隐式决定，无全局 active state
- page 2 永远 → intervene(xuannv)
- 私聊页 → intervene(worker_id)，玄女抄送由后端 A2A 做

## 协作模式收获

新加的 `memory/feedback_team_lead_batch_dispatch.md` —— 自驱型 teammate（如 ε）派活模式应改"整批 spec + dep 一次给完，按 dep 自驱"，不要"ack→派下一条"轮转。本会话踩过 4 次 stale kickoff，源于 ε 完工速度比 team-lead ack→派的轮转更快。

## 给新会话的"5 分钟接班"清单

1. 读 `CLAUDE.md`（公理 + 工程规范，重点公理 7：日常体验第一）
2. 读本文件（NOW）
3. 读 `docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md`（重设计 spec）+ `docs/decisions/15-im-task-tree-pager.md`（决策依据）
4. `git log --oneline -15` 看 commit 链
5. `TaskList` 看现状（重点 #23 在等用户重传 + 5 pending follow-ups）
6. `git status` 确认 working tree 干净
7. SendMessage 各 teammate 探活（β 在等 #23 用户日志，ε/ζ 待命）

## 不该忘记

- ssh home 端口 2222 / 用户 e0-7 / DDNS / sudo 可用
- nginx 8443 + 通配符证书 + WS upgrade headers 在 sites-enabled/sia-gateway 模板里
- fuxi-im 绑 127.0.0.1:9100 + nginx im.qmledmq.cn:8443 反代
- `~/.fuxi/im_password.bcrypt` 主密码（用户私持），不要让任何 agent 知道
- 玄女 self-spawn 在 `fuxi im start` 启动时由 `xuannv_bootstrap::ensure_xuannv` 触发
- 镜像端点 `/api/workers/:agent_id/{events,conv}` filter 规则：`UserInterventionSent` 看 `target` 不看 `meta.agent`（抄送场景关键）；其它走 `meta.agent ==`，EventKind 白名单见 spec
