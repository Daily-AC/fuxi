# NOW · 单页真相（2026-04-26 · IM v3 + β follow-up 三件套全 ship）

> 上一份 NOW 在同日早些时候，记 v2 重设计四件套（用户实测后否决）。本份覆盖 v3 重设计 + β follow-up backlog 清空。

## 一句话

fuxi-im **v3 重设计**（bottom tab bar + 任务=群聊 thread + chip @ 提及）全套已部署 home，HEAD `a2c5976`。v2 (horizontal pager + per-worker 私聊页) 在用户实测后被否决，本批是替代实装。同时 β 把三件 follow-up backlog 全清（#7/#12/#19）+ ζ 永久修了 rsync stale 部署坑。

## 关键节点

- **v3 spec**：`docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md`
- **v2 spec（已 superseded）**：`docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md`
- **决策 16**（v3 心智 / 否决 v2 路径）：`docs/decisions/16-im-tab-bar-task-thread.md`
- **决策 17**（IM 部署解耦中期排期）：`docs/decisions/17-im-deploy-decoupling.md`
- agent team `fuxi-im-v1`：α/β/γ/δ/ε/ζ + team-lead，全员 alive，可 SendMessage 续派
- 部署：`./deploy/im/install.sh --apply`（rust+web ~1min）/ `--apply --web-only`（dist ~30s）

## 本会话提交（按时间，21 条）

```
a2c5976 ζ install.sh rsync 加 -c checksum · 防 mtime collision stale
5a7d714 β follow-up 三件 (#19 dispatch pump 收尾 + #12 fact 两阶段 + #7 xuannv watch)
1a4d6d5 ε v3 #39 任务 thread mix 全成员
ce9a12c ε v3 #40 玄女 tab 删 sticky badge + 加 @ chip composer
d984d7d ε v3 #38 任务列表点卡 push thread
dc0a404 ε v3 #37 MentionChip + MentionAutocomplete 组件
ad220d6 ε v3 #36 App shell · BottomTabBar + 删 Pager
e58f188 β v3 #42 POST /api/intervene 加 mentions 字段
e8cf9bf β v3 #41 镜像端点 /api/tasks/:id/{events,conv}
71a788d v3 spec + 决策 16/17 + v2 spec superseded mark
cd73691 NOW handoff v2 全 ship 状态
fda219b ε v2 #31 玄女 sticky badge "✓ 抄送 N 门客"  ← v2 (现已 superseded)
9bd8ed6 β #23 upload multipart 详细诊断日志 + 0 字节支持 + PNG e2e
983a6bf ε v2 #30 私聊页 modal C 方案橙色识别 + 任务 banner ← v2
41eb8d8 β #25 GET /api/tasks 字段补齐 + filter user-turn
0fbfc7b β #27 镜像端点 /api/workers/:agent_id/{events,conv}
c52c908 ε v2 #29 任务树页 C 方案两级行卡片  ← v2
df189eb ζ install.sh 加 --web-only flag
ee34f8d ε v2 #28 重构 App shell · BottomSheet → horizontal pager + NavigationStack  ← v2
0a707b8 docs · 决策 15 + CLAUDE.md codex follow-up 注释修措辞
d35f653 v2 spec
```

## v3 重设计 ship 矩阵

| # | task | commit | 测试 |
|---|---|---|---|
| #36 | App shell · BottomTabBar + 删 Pager | `ad220d6` | unit + e2e |
| #37 | MentionChip + MentionAutocomplete 组件 | `dc0a404` | unit + e2e |
| #38 | 任务列表点卡 push thread | `d984d7d` | unit + e2e |
| #39 | 任务 thread mix 全成员 | `1a4d6d5` | 12 reducer + 9 page unit + 2 e2e |
| #40 | 玄女 tab 删 badge + 加 @ chip composer | `ce9a12c` | 13 unit + 2 e2e |
| #41 | β `/api/tasks/:id/{events,conv}` 镜像端点 | `e8cf9bf` | 8 filter + 4 e2e |
| #42 | β `POST /api/intervene` 加 mentions | `e58f188` | 4 单测 |

PWA 总：224 unit + 20 e2e；后端总：187 backend unit。

## β follow-up backlog 清空（`5a7d714`）

| # | 修复 |
|---|---|
| #19 | `EventKind::TaskBlocked` 加进 dispatch pump `is_terminal` 白名单（cc ResultError / codex TurnFailed 翻译到 TaskBlocked 后 cc 进 Idle 但不发新事件，旧 pump 卡死） |
| #12 | `session.rs` 拆 `resolve_xuannv_session` (只读) + `record_xuannv_session` (spawn 成功后才落盘)。**实测**：ζ stop/start fuxi-im 后 journalctl 干净，永久消除手动 `sqlite DELETE oracle_facts xuannv` 运维负担 |
| #7 | `Fuxi::xuannv_id` 从 `Arc<RwLock>` 改 `tokio::sync::watch::Sender`；`wait_for_xuannv` 用 `.changed().await` 真实时唤醒（公理 3 兑现） |

## 进行中（in_progress 等用户）

- **#23 β · upload multipart 400** — `9bd8ed6` 加诊断日志已部署。**等用户重传一次 iPhone 上传**，β 拿 `journalctl -u fuxi-im | grep -E "upload 入口|multer_error_debug"` 排真因。

## pending follow-up（3 条非阻塞）

- **#35** ToolCallCard stdout 前 20 行截断 + 全文按钮（任务 thread 仍渲染 ToolCallCard，仍适用）
- **#43** UserMessage.mentions 历史回放还原 chip 视觉（数据已带，仅渲染层简化）
- **#23** 同上，等用户重传

## v3 心智模型（实装兑现）

```
┌─ MainShell ───────────────────────────────────────────────┐
│                                                           │
│  ┌─ activeTab content ─────────────────────────────────┐  │
│  │  玄女 tab：用户↔玄女 thread + composer with @       │  │
│  │            (autocomplete = all alive workers)       │  │
│  │  任务 tab Layer 1：任务列表（点卡 push）             │  │
│  │  任务 tab Layer 2：任务 thread（mix 全成员）         │  │
│  │  节点 tab：节点列表                                  │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
│  ┌─ BottomTabBar (56px) ───────────────────────────────┐  │
│  │  ● 玄女     ⚒ 任务     ⛁ 节点                       │  │
│  └─────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────────────┘
```

**composer @ 路由**：text 里第一个 @mention 为 `target`，无 @ 则默认（玄女 tab → 玄女；任务 thread → 玄女即任务发起人）。多 @ v1 简化为第一个为准 + toast 警示。

## v3 视觉一致性（沿用决策 14）

- 暖暗底 `#1F1E1B`，Anthropic 橙 accent `#D97757`
- 角色色：玄女紫 `#C4A8E8` / 鲁班琥珀 `#E5A547` / 蒲松绿 `#A0C277`
- chip 用**角色色**而非橙 accent（一眼区分 @ 谁）
- autocomplete 弹层 inline 紧贴 composer max-height 200px
- 不用 emoji / Unicode block / shadow / gradient
- 触控热区 ≥ 44px（tab bar 项 ≥ 48px）

## 协作模式两条收获

新 memory：

- `feedback_team_lead_batch_dispatch.md`（ε 自己写）—— 自驱型 teammate 派活：整批 spec + dep 一次给完，按 dep 自驱，不每件 ack 后再派下一条。worker 速度 >> coordinator 时省 round-trip。
- 实测验证：v3 batch 派活 ε 五件套连环 ship，β 两件并行 ship + 三件 follow-up，全程 zero stale。

## 给新会话的"5 分钟接班"清单

1. 读 `CLAUDE.md`（公理 + 工程规范）
2. 读本文件
3. 读 v3 spec `docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md` + 决策 16/17
4. `git log --oneline -25` 看 commit 链
5. `TaskList` 看现状（重点 #23 等用户 / #35 #43 follow-up）
6. `git status` 确认 working tree 干净
7. SendMessage 各 teammate 探活；β/ε 都已下班待命

## 不该忘记

- ssh home 端口 2222 / 用户 e0-7 / DDNS / sudo 可用
- nginx 8443 + 通配符证书 + WS upgrade headers 在 sites-enabled/sia-gateway 模板
- fuxi-im 绑 127.0.0.1:9100 + nginx im.qmledmq.cn:8443 反代
- `~/.fuxi/im_password.bcrypt` 主密码（用户私持）
- 玄女 self-spawn 在 `fuxi im start` 启动时由 `xuannv_bootstrap::ensure_xuannv` 触发（**#12 已修，spawn 失败不留 stale fact**）
- 镜像端点：worker per `/api/workers/:id/{events,conv}` 保留备用（v2 路线，决策 17 部署解耦后可能用上）；task per `/api/tasks/:id/{events,conv}` 是 v3 主线
- intervene 的 `mentions: [agent_ids]` 是 v3 新增字段，写入 `UserInterventionSent.mentions`，老事件读出空 Vec backward compat
- install.sh rsync 已加 `-c` checksum（`a2c5976`），mtime collision stale 永久修
